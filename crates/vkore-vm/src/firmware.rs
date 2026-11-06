//! UEFI firmware for a Kore machine, found the way QEMU documents it: the
//! JSON firmware descriptors distributions install
//! (`docs/interop/firmware.json`), not a list of guessed file names.
//!
//! Kore boots through Limine's unsigned EFI image, so the firmware must be
//! UEFI for x86_64 `q35`, split into executable and variable-store flash
//! (the machine keeps its own copy of the variables), and must not enforce
//! Secure Boot, enroll keys, require SMM, or launch a confidential guest.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

/// A firmware the machine boots.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Firmware {
    /// The descriptor it came from, when found by discovery.
    pub descriptor: Option<PathBuf>,
    /// The executable flash, mapped read-only.
    pub code: PathBuf,
    /// The variable-store template each machine copies.
    pub vars_template: PathBuf,
    /// `raw` or `qcow2`.
    pub format: String,
}

/// Where QEMU's firmware descriptors live, in QEMU's search order (a later
/// directory's same-named descriptor is overridden by an earlier one's).
pub fn descriptor_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/etc/qemu/firmware")];
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        dirs.push(PathBuf::from(config).join("qemu/firmware"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config/qemu/firmware"));
    }
    dirs.push(PathBuf::from("/usr/share/qemu/firmware"));
    // Homebrew's QEMU installs its descriptors under its own prefix.
    dirs.push(PathBuf::from("/opt/homebrew/share/qemu/firmware"));
    dirs.push(PathBuf::from("/usr/local/share/qemu/firmware"));
    dirs
}

/// Features a Kore machine cannot boot with.
const REFUSED_FEATURES: &[&str] = &[
    "secure-boot",
    "enrolled-keys",
    "requires-smm",
    "amd-sev-es",
    "amd-sev-snp",
    "intel-tdx",
];

/// The firmware a descriptor names, if a Kore machine can boot it.
pub fn from_descriptor(path: &Path, descriptor: &Value) -> Option<Firmware> {
    let strings = |value: &Value| {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    if !strings(&descriptor["interface-types"])
        .iter()
        .any(|kind| kind == "uefi")
    {
        return None;
    }
    let mapping = &descriptor["mapping"];
    if mapping["device"].as_str() != Some("flash") {
        return None;
    }
    if mapping["mode"].as_str().is_some_and(|mode| mode != "split") {
        return None;
    }
    let targets_q35 = descriptor["targets"].as_array().is_some_and(|targets| {
        targets.iter().any(|target| {
            target["architecture"].as_str() == Some("x86_64")
                && strings(&target["machines"])
                    .iter()
                    .any(|machine| machine.starts_with("pc-q35-"))
        })
    });
    if !targets_q35 {
        return None;
    }
    let features = strings(&descriptor["features"]);
    if features
        .iter()
        .any(|feature| REFUSED_FEATURES.contains(&feature.as_str()))
    {
        return None;
    }
    let code = mapping["executable"]["filename"].as_str()?;
    let vars = mapping["nvram-template"]["filename"].as_str()?;
    let format = mapping["executable"]["format"].as_str().unwrap_or("raw");
    if mapping["nvram-template"]["format"]
        .as_str()
        .unwrap_or("raw")
        != format
        || !matches!(format, "raw" | "qcow2")
    {
        return None;
    }
    Some(Firmware {
        descriptor: Some(path.to_path_buf()),
        code: code.into(),
        vars_template: vars.into(),
        format: format.into(),
    })
}

/// The first bootable firmware the installed descriptors name: descriptors
/// by file name across the search directories (an earlier directory's file
/// overriding a later one's of the same name), then the first whose files
/// exist.
pub fn discover() -> Option<Firmware> {
    discover_in(&descriptor_dirs())
}

pub fn discover_in(dirs: &[PathBuf]) -> Option<Firmware> {
    let mut by_name = std::collections::BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if let Some(name) = path.file_name().map(|name| name.to_owned()) {
                by_name.entry(name).or_insert(path);
            }
        }
    }
    by_name.into_values().find_map(|path| {
        let descriptor = serde_json::from_slice::<Value>(&fs::read(&path).ok()?).ok()?;
        let firmware = from_descriptor(&path, &descriptor)?;
        (firmware.code.is_file() && firmware.vars_template.is_file()).then_some(firmware)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(features: &[&str], with_vars: bool) -> Value {
        let mut mapping = serde_json::json!({
            "device": "flash",
            "executable": {"filename": "/fw/CODE.fd", "format": "raw"},
        });
        if with_vars {
            mapping["nvram-template"] =
                serde_json::json!({"filename": "/fw/VARS.fd", "format": "raw"});
        }
        serde_json::json!({
            "interface-types": ["uefi"],
            "mapping": mapping,
            "targets": [{"architecture": "x86_64", "machines": ["pc-i440fx-*", "pc-q35-*"]}],
            "features": features,
        })
    }

    #[test]
    fn a_plain_split_uefi_firmware_is_taken_and_secure_or_confidential_ones_are_not() {
        let path = Path::new("/d/60-edk2-x86_64.json");
        let firmware = from_descriptor(
            path,
            &descriptor(&["acpi-s3", "amd-sev", "verbose-dynamic"], true),
        )
        .unwrap();
        assert_eq!(firmware.code, PathBuf::from("/fw/CODE.fd"));
        assert_eq!(firmware.vars_template, PathBuf::from("/fw/VARS.fd"));
        assert_eq!(firmware.format, "raw");
        for refused in [
            "secure-boot",
            "requires-smm",
            "enrolled-keys",
            "amd-sev-snp",
        ] {
            assert!(
                from_descriptor(path, &descriptor(&[refused], true)).is_none(),
                "{refused}"
            );
        }
        // A stateless image has no variable store to give each machine.
        assert!(from_descriptor(path, &descriptor(&[], false)).is_none());
        let mut bios = descriptor(&[], true);
        bios["interface-types"] = serde_json::json!(["bios"]);
        assert!(from_descriptor(path, &bios).is_none());
    }

    #[test]
    fn discovery_takes_descriptors_by_name_and_skips_missing_files() {
        let root = tempfile::tempdir().unwrap();
        let (high, low) = (root.path().join("etc"), root.path().join("usr"));
        fs::create_dir_all(&high).unwrap();
        fs::create_dir_all(&low).unwrap();
        let code = root.path().join("CODE.fd");
        let vars = root.path().join("VARS.fd");
        fs::write(&code, b"").unwrap();
        fs::write(&vars, b"").unwrap();
        let write = |dir: &Path, name: &str, code: &Path| {
            let mut value = descriptor(&[], true);
            value["mapping"]["executable"]["filename"] = code.to_str().unwrap().into();
            value["mapping"]["nvram-template"]["filename"] = vars.to_str().unwrap().into();
            fs::write(dir.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
        };
        write(&low, "50-missing.json", &root.path().join("absent.fd"));
        write(&low, "60-edk2.json", &code);
        let found = discover_in(&[high.clone(), low.clone()]).unwrap();
        assert_eq!(found.code, code);
        assert_eq!(found.descriptor.unwrap(), low.join("60-edk2.json"));
        // The same name in an earlier directory overrides the later one.
        write(&high, "60-edk2.json", &root.path().join("absent.fd"));
        assert!(discover_in(&[high, low]).is_none());
    }
}
