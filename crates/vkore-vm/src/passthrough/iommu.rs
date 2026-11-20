//! Choose a VFIO-compatible Intel vIOMMU by realizing it in QEMU. Its
//! effective width comes from the attached device's kernel IOVA ranges, not
//! CPU physical-address bits, PCI identity or raw VT-d capability registers.
use super::Selection;

pub(super) struct IntelProbe {
    position: usize,
    base: String,
    firmware: String,
    cpu: String,
    cpu_position: usize,
    cpu_power_management: bool,
}
impl IntelProbe {
    pub(super) fn plan(args: &[String]) -> Result<Option<Self>, String> {
        let devices: Vec<_> = args
            .windows(2)
            .enumerate()
            .filter(|(_, a)| a[0] == "-device" && a[1].starts_with("intel-iommu,"))
            .collect();
        if devices.is_empty() {
            return Ok(None);
        }
        if devices.len() != 1 {
            return Err("Ambiguous Intel IOMMU configuration.".into());
        }
        let (position, pair) = devices[0];
        let base = &pair[1];
        let mut names = std::collections::BTreeSet::new();
        for option in base.split(',').skip(1) {
            let (name, _) = option
                .split_once('=')
                .ok_or("Invalid Intel IOMMU option.")?;
            if !names.insert(name) {
                return Err("Duplicate Intel IOMMU option.".into());
            }
        }
        if base
            .split(',')
            .filter(|s| matches!(*s, "scalable-mode=on" | "x-scalable-mode=on"))
            .count()
            != 1
        {
            return Err("Managed Intel passthrough requires scalable mode.".into());
        }
        // Reject unknown layouts rather than rewriting unrelated options.
        for required in [
            "intremap=on",
            "caching-mode=on",
            "device-iotlb=on",
            "snoop-control=off",
            "dma-translation=on",
            "aw-bits=48",
        ] {
            if base.split(',').filter(|s| *s == required).count() != 1 {
                return Err(format!("Managed Intel passthrough requires {required}."));
            }
        }
        if base
            .split(',')
            .any(|s| s == "x-flts=on" || s == "x-pasid-mode=on")
        {
            return Err("Managed passthrough uses second-level DMA translation, without PASID/first-level translation.".into());
        }
        let option = |flag: &str, predicate: fn(&str) -> bool| -> Result<String, String> {
            let values: Vec<_> = args
                .windows(2)
                .filter(|a| a[0] == flag && predicate(&a[1]))
                .collect();
            if values.len() != 1 {
                return Err(format!("Missing or ambiguous {flag} for the IOMMU probe."));
            }
            Ok(values[0][1].clone())
        };
        let cpu = option("-cpu", |_| true)?;
        if cpu.split(',').skip(1).any(|p| {
            matches!(
                p.split('=').next(),
                Some("phys-bits" | "host-phys-bits" | "host-phys-bits-limit")
            )
        }) {
            return Err("Guest physical address width is owned by DMA negotiation.".into());
        }
        let cpu_position = args.iter().position(|a| a == "-cpu").unwrap() + 1;
        Ok(Some(Self {
            position: position + 1,
            base: base.clone(),
            firmware: option("-drive", |s| {
                s.starts_with("if=pflash,") && s.split(',').any(|p| p == "readonly=on")
            })?,
            cpu,
            cpu_position,
            cpu_power_management: args.windows(2).any(|pair| {
                pair[0] == "-overcommit" && pair[1].split(',').any(|option| option == "cpu-pm=on")
            }),
        }))
    }
    fn device(&self, width: u8) -> String {
        self.base.replace("aw-bits=48", &format!("aw-bits={width}"))
    }
    fn cpu(&self, width: u8) -> String {
        // Firmware places RAM-backed 64-bit BARs using CPUID physical bits.
        // Keep those mappings inside the same aperture as guest DMA.
        format!(
            "{},host-phys-bits=on,host-phys-bits-limit={width}",
            self.cpu
        )
    }
    pub(super) fn arguments(&self, width: u8, selected: &[Selection]) -> Vec<String> {
        let mut args: Vec<String> = [
            "-machine",
            "q35,kernel_irqchip=split",
            "-accel",
            "kvm",
            "-cpu",
            &self.cpu(width),
            "-m",
            "128",
            "-smp",
            "1",
            "-S",
            "-no-user-config",
            "-nodefaults",
            "-display",
            "none",
            "-monitor",
            "none",
            "-qmp",
            "stdio",
            "-drive",
            &self.firmware,
            "-device",
            &self.device(width),
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        // MONITOR is only realizable under KVM when cpu-pm is enabled. Carry
        // the policy into the paused DMA probe, without locking probe memory.
        if self.cpu_power_management {
            args.extend(["-overcommit".into(), "cpu-pm=on".into()]);
        }
        args.extend(super::arguments(selected));
        args
    }
    /// QEMU's second-level VT-d supports 48 or 39 bits. Realize the widest
    /// supported choice; each attempt owns fresh, independently closed FDs.
    pub(super) fn choose(
        &self,
        args: &mut [String],
        memory_bytes: u64,
        mut realize: impl FnMut(u8) -> Result<(), String>,
    ) -> Result<u8, String> {
        let mut failures = Vec::new();
        for width in [48, 39] {
            // Reserve the full below-4-GiB hole when bounding guest RAM. This
            // deliberately rejects configurations needing a larger DMA space.
            if memory_bytes
                .checked_add(4 << 30)
                .is_none_or(|n| n > 1u64 << width)
            {
                failures.push(format!(
                    "{width} bits cannot accommodate guest RAM plus the PCI address hole"
                ));
                continue;
            }
            match realize(width) {
                Ok(()) => {
                    args[self.position] = self.device(width);
                    args[self.cpu_position] = self.cpu(width);
                    return Ok(width);
                }
                Err(error) => failures.push(format!("{width} bits: {error}")),
            }
        }
        Err(format!(
            "No compatible Intel IOMMU configuration: {}. See the QEMU log for probe diagnostics.",
            failures.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn original() -> Vec<String> {
        ["-cpu", "host", "-drive", "if=pflash,format=raw,readonly=on,file=/code.fd",
        "-drive", "if=pflash,format=raw,file=/vars.fd", "-drive", "file=/guest.raw,if=none,id=nvme0",
        "-device", "intel-iommu,intremap=on,caching-mode=on,device-iotlb=on,x-scalable-mode=on,snoop-control=off,dma-translation=on,aw-bits=48"]
        .into_iter().map(str::to_owned).collect()
    }
    #[test]
    fn selects_widest_realized_width_including_gigatron_case() {
        for (limit, expected, attempts) in [(48, 48, vec![48]), (41, 39, vec![48, 39])] {
            let mut args = original();
            let plan = IntelProbe::plan(&args).unwrap().unwrap();
            let mut tried = Vec::new();
            assert_eq!(
                plan.choose(&mut args, 4 << 30, |width| {
                    tried.push(width);
                    if width <= limit {
                        Ok(())
                    } else {
                        Err("host IOVA width is smaller".into())
                    }
                })
                .unwrap(),
                expected
            );
            assert_eq!(tried, attempts);
            assert_eq!(
                args[plan.cpu_position],
                format!("host,host-phys-bits=on,host-phys-bits-limit={expected}")
            );
            assert!(
                plan.arguments(expected, &[])
                    .contains(&args[plan.cpu_position])
            );
            let device = &args[plan.position];
            assert!(device.ends_with(&format!("aw-bits={expected}")));
            for flag in [
                "intremap=on",
                "dma-translation=on",
                "caching-mode=on",
                "snoop-control=off",
            ] {
                assert!(device.contains(flag));
            }
        }
    }
    #[test]
    fn no_supported_candidate_or_insufficient_width_fails_without_mutation() {
        let mut args = original();
        let plan = IntelProbe::plan(&args).unwrap().unwrap();
        assert!(
            plan.choose(&mut args, 4 << 30, |_| Err("unsupported".into()))
                .is_err()
        );
        assert_eq!(args, original());
        let error = plan
            .choose(&mut args, 512 << 30, |width| {
                assert_eq!(width, 48); // Too much RAM for 39 bits: never try it.
                Err("48 bits unavailable".into())
            })
            .unwrap_err();
        assert!(error.contains("guest RAM"));
        assert_eq!(args, original());
    }
    #[test]
    fn probe_is_paused_and_has_no_writable_storage_or_guest_firmware_vars() {
        let plan = IntelProbe::plan(&original()).unwrap().unwrap();
        let selected = vec![Selection {
            address: "0000:00:0b.0".into(),
            vendor_id: 0x8086,
            device_id: 0x643e,
            physical_function: None,
        }];
        let args = plan.arguments(39, &selected);
        assert!(args.iter().any(|a| a == "-S"));
        assert!(args.contains(&"host,host-phys-bits=on,host-phys-bits-limit=39".into()));
        assert!(args.windows(2).any(|a| a == ["-m", "128"]));
        assert!(args.windows(2).any(|a| a == ["-qmp", "stdio"]));
        assert_eq!(args.iter().filter(|a| *a == "-drive").count(), 1);
        assert!(
            !args
                .iter()
                .any(|a| a.contains("guest.raw") || a.contains("vars.fd"))
        );
        assert!(
            args.iter()
                .any(|a| a.starts_with("vfio-pci,host=0000:00:0b.0,"))
        );
    }
    #[test]
    fn ambiguous_or_incompatible_probe_input_is_rejected() {
        for (old, new) in [
            ("snoop-control=off", "snoop-control=on"),
            ("aw-bits=48", "aw-bits=48,aw-bits=39"),
            ("aw-bits=48", "aw-bits=48,x-flts=on"),
            ("x-scalable-mode=on", "x-scalable-mode=off"),
        ] {
            let mut args = original();
            let last = args.last_mut().unwrap();
            *last = last.replace(old, new);
            assert!(IntelProbe::plan(&args).is_err());
        }
        assert!(
            IntelProbe::plan(&["-device".into(), "amd-iommu,intremap=on".into()])
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn dma_probe_preserves_native_waits_without_inheriting_memory_locking() {
        let mut args = original();
        args[1] = "host,monitor=on,kvm-pv-unhalt=off,enforce".into();
        args.extend(["-overcommit".into(), "cpu-pm=on,mem-lock=on".into()]);
        let plan = IntelProbe::plan(&args).unwrap().unwrap();
        let probe = plan.arguments(39, &[]);
        assert!(probe.contains(&"cpu-pm=on".into()));
        assert!(!probe.iter().any(|arg| arg.contains("mem-lock")));
        plan.choose(&mut args, 4 << 30, |_| Ok(())).unwrap();
        assert!(args[1].starts_with("host,monitor=on,kvm-pv-unhalt=off,enforce,"));
        assert!(args.contains(&"cpu-pm=on,mem-lock=on".into()));
    }

    #[test]
    fn negotiated_physical_width_preserves_features_and_rejects_conflicting_caps() {
        let mut args = original();
        args[1] = "host,+la57,enforce".into();
        let plan = IntelProbe::plan(&args).unwrap().unwrap();
        plan.choose(&mut args, 4 << 30, |_| Ok(())).unwrap();
        assert_eq!(
            args[1],
            "host,+la57,enforce,host-phys-bits=on,host-phys-bits-limit=48"
        );
        for option in [
            "phys-bits=46",
            "host-phys-bits=off",
            "host-phys-bits-limit=39",
        ] {
            let mut args = original();
            args[1] = format!("host,{option}");
            assert!(IntelProbe::plan(&args).is_err());
        }
    }

    #[test]
    fn real_qemu_realizes_paused_candidates_without_guest_execution() {
        let Some(qemu) = std::env::var_os("VKORE_TEST_QEMU") else {
            return;
        };
        let qemu = std::path::Path::new(&qemu);
        let runtime = crate::runtime::Runtime::for_qemu(qemu)
            .unwrap()
            .expect("packaged QEMU runtime");
        let mut args = original();
        args[3] = format!(
            "if=pflash,format={},readonly=on,file={}",
            runtime.firmware.format,
            runtime.firmware.code.display()
        );
        let plan = IntelProbe::plan(&args).unwrap().unwrap();
        for (width, supported) in [(48, true), (39, true), (38, false)] {
            let mut arguments = plan.arguments(width, &[]);
            // This tests the production probe transport/profile under TCG, not VFIO.
            for pair in arguments.chunks_mut(1) {
                if pair[0] == "kvm" {
                    pair[0] = "tcg".into();
                }
                if pair[0].starts_with("host,") {
                    // TCG has no host CPUID; exercise an explicit equivalent cap.
                    pair[0] = format!("max,phys-bits={width}");
                }
            }
            let (stream, emulator) = std::os::unix::net::UnixStream::pair().unwrap();
            let mut command = std::process::Command::new(qemu);
            command
                .args(arguments)
                .stdin(std::process::Stdio::from(std::os::fd::OwnedFd::from(
                    emulator.try_clone().unwrap(),
                )))
                .stdout(std::process::Stdio::from(std::os::fd::OwnedFd::from(
                    emulator,
                )))
                .stderr(std::process::Stdio::inherit());
            let child = command.spawn().unwrap();
            drop(command);
            assert_eq!(
                super::super::supervision::probe_paused(child, stream).is_ok(),
                supported,
                "aw-bits={width}: a greeting alone must not count as realization"
            );
        }
    }
}
