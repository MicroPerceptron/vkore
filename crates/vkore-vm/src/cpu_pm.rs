//! Opt-in native guest waits. Affinity is required, but is not exclusive CPU ownership.
use crate::qmp::{Qmp, QmpError};

/// Read-only host check. QEMU must still enable the capability and realize MONITOR.
pub fn available() -> Result<(), String> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        if !raw_cpuid::CpuId::new()
            .get_feature_info()
            .is_some_and(|info| info.has_monitor_mwait())
        {
            return Err("This host CPU does not advertise MONITOR/MWAIT.".into());
        }
        let kvm = kvm_ioctls::Kvm::new()
            .map_err(|error| format!("Cannot check native CPU waits: /dev/kvm: {error}"))?;
        let mask = kvm.check_extension_raw(kvm_bindings::KVM_CAP_X86_DISABLE_EXITS.into());
        if mask <= 0 || mask as u32 & kvm_bindings::KVM_X86_DISABLE_EXITS_MWAIT == 0 {
            return Err("KVM does not allow native MONITOR/MWAIT on this host.".into());
        }
        Ok(())
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    Err("Native CPU waits require an x86-64 Linux host with KVM.".into())
}

/// Check the realized CPUs while stopped, before allowing any guest instructions.
pub(crate) fn verify(qmp: &mut Qmp, cpus: &[qapi_qmp::CpuInfoFast]) -> Result<(), QmpError> {
    for cpu in cpus {
        let base = match cpu {
            qapi_qmp::CpuInfoFast::x86_64(base) | qapi_qmp::CpuInfoFast::i386(base) => base,
            _ => {
                return Err(QmpError::Protocol(
                    "Native CPU waits require x86 CPUs.".into(),
                ));
            }
        };
        let monitor = qmp.execute(&qapi_qmp::qom_get {
            path: base.qom_path.clone(),
            property: "monitor".into(),
        })?;
        if monitor.as_bool() != Some(true) {
            return Err(QmpError::Protocol(format!(
                "QEMU did not expose MONITOR/MWAIT on guest CPU {}.",
                base.cpu_index
            )));
        }
    }
    Ok(())
}
