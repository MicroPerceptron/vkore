//! Relocatable, release-built QEMU and firmware. Host capability probes still
//! run: a packaged binary cannot promise KVM permissions or host CPU features.
//!
//! One bundle may carry an emulator per guest architecture
//! ([`GuestArch::bundle_files`]); a bundle is asked for one architecture and
//! must hold that one's emulator and both firmware halves, so a bundle
//! built before aarch64 guests refuses one in words instead of booting the
//! wrong firmware.
use crate::{Firmware, arch::GuestArch};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

const MANIFEST: &str = "bundle.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    version: String,
    source_sha256: String,
    host_os: String,
    host_arch: String,
    files: BTreeMap<String, String>,
}

pub struct Runtime {
    pub firmware: Firmware,
    pub data: PathBuf,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// The `arch` emulator of the explicit bundle, the installed sibling, then
/// the macOS app's Resources. Returning a damaged installation is
/// intentional: preflight must explain it, not silently substitute a
/// different emulator from PATH.
pub fn bundled_qemu(
    executable: &Path,
    explicit: Option<PathBuf>,
    arch: GuestArch,
) -> Option<PathBuf> {
    let binary = arch.bundle_files()[0];
    if let Some(root) = explicit {
        return Some(root.join(binary));
    }
    let parent = executable.parent()?;
    [
        parent.join("../libexec/vkore/qemu-runtime"),
        parent.join("../Resources/qemu-runtime"),
    ]
    .into_iter()
    .find(|root| root.exists())
    .map(|root| root.join(binary))
}

impl Runtime {
    /// The bundle `qemu` belongs to, verified, for a guest of `arch`; `None`
    /// when `qemu` is not a bundled emulator.
    pub fn for_qemu(qemu: &Path, arch: GuestArch) -> io::Result<Option<Self>> {
        let [binary, code, vars] = arch.bundle_files();
        let Some(root) = qemu.parent().and_then(Path::parent) else {
            return Ok(None);
        };
        let manifest_path = root.join(MANIFEST);
        let explicit = std::env::var_os("VKORE_QEMU_BUNDLE")
            .is_some_and(|path| PathBuf::from(path).join(binary) == qemu);
        if !manifest_path.exists()
            && !explicit
            && root.file_name().is_none_or(|n| n != "qemu-runtime")
        {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        fs::File::open(&manifest_path)?
            .take((1 << 20) + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 1 << 20 {
            return Err(invalid("QEMU bundle manifest exceeds 1 MiB"));
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
        if manifest.schema != 1
            || manifest.version.is_empty()
            || manifest.source_sha256.len() != 64
            || manifest.host_os != std::env::consts::OS
            || manifest.host_arch != std::env::consts::ARCH
        {
            return Err(invalid(
                "QEMU bundle version or host platform is incompatible",
            ));
        }
        for required in [binary, code, vars] {
            if !manifest.files.contains_key(required) {
                return Err(invalid(format!(
                    "QEMU bundle omits {required}, which {arch} guests need"
                )));
            }
        }
        let mut total_bytes = 0u64;
        for (name, expected) in &manifest.files {
            let path = Path::new(name);
            if path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
                || path.as_os_str().is_empty()
            {
                return Err(invalid("QEMU bundle contains an unsafe path"));
            }
            let mut full = root.to_path_buf();
            for part in path.components() {
                full.push(part);
                if fs::symlink_metadata(&full)?.file_type().is_symlink() {
                    return Err(invalid("QEMU bundle contains a symlink"));
                }
            }
            let mut file = fs::File::open(&full)?;
            if !file.metadata()?.is_file() {
                return Err(invalid("QEMU bundle asset is not a regular file"));
            }
            total_bytes = total_bytes.saturating_add(file.metadata()?.len());
            if total_bytes > 1 << 30 {
                return Err(invalid("QEMU bundle assets exceed 1 GiB"));
            }
            let mut hash = Sha256::new();
            let mut buffer = [0; 64 << 10];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            if format!("{:x}", hash.finalize()) != *expected {
                return Err(invalid(format!("QEMU bundle checksum mismatch: {name}")));
            }
        }
        Ok(Some(Self {
            firmware: Firmware {
                descriptor: Some(manifest_path),
                code: root.join(code),
                vars_template: root.join(vars),
                format: "raw".into(),
            },
            data: root.join("share/qemu"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const X86: GuestArch = GuestArch::X86_64;

    #[test]
    fn relocated_bundle_is_verified_and_corruption_is_refused() {
        let [binary, code, _] = X86.bundle_files();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("qemu-runtime");
        let mut files = BTreeMap::new();
        for name in X86.bundle_files() {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
            files.insert(name, format!("{:x}", Sha256::digest(name.as_bytes())));
        }
        let value = serde_json::json!({"schema":1,"version":"test","source_sha256":"a".repeat(64),
            "host_os":std::env::consts::OS,"host_arch":std::env::consts::ARCH,"files":files});
        fs::write(root.join(MANIFEST), serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            Runtime::for_qemu(&root.join(binary), X86)
                .unwrap()
                .unwrap()
                .firmware
                .code,
            root.join(code)
        );
        // An x86-only bundle has no aarch64 emulator or firmware to offer.
        let aarch64 = GuestArch::Aarch64.bundle_files()[0];
        let refused = Runtime::for_qemu(&root.join(aarch64), GuestArch::Aarch64)
            .err()
            .unwrap();
        assert!(
            refused.to_string().contains("aarch64 guests need"),
            "{refused}"
        );
        fs::write(root.join(code), "changed").unwrap();
        assert!(Runtime::for_qemu(&root.join(binary), X86).is_err());
        fs::remove_file(root.join(MANIFEST)).unwrap();
        assert!(Runtime::for_qemu(&root.join(binary), X86).is_err());
    }

    #[test]
    fn one_bundle_serves_each_architecture_it_carries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("qemu-runtime");
        let mut files = BTreeMap::new();
        for name in GuestArch::ALL.into_iter().flat_map(GuestArch::bundle_files) {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
            files.insert(name, format!("{:x}", Sha256::digest(name.as_bytes())));
        }
        let value = serde_json::json!({"schema":1,"version":"test","source_sha256":"a".repeat(64),
            "host_os":std::env::consts::OS,"host_arch":std::env::consts::ARCH,"files":files});
        fs::write(root.join(MANIFEST), serde_json::to_vec(&value).unwrap()).unwrap();
        for arch in GuestArch::ALL {
            let [binary, code, vars] = arch.bundle_files();
            let runtime = Runtime::for_qemu(&root.join(binary), arch)
                .unwrap()
                .unwrap();
            assert_eq!(runtime.firmware.code, root.join(code));
            assert_eq!(runtime.firmware.vars_template, root.join(vars));
        }
    }
}
