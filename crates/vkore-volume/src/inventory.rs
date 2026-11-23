//! Host labels are descriptive metadata, never evidence of a DagFS volume.
//! Only recovery through Kore's engine confirms DagFS and its format version.

#[cfg(any(target_os = "linux", test))]
use crate::{Browser, Disk, claim::Claim};
use crate::{Error, MountStatus};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "linux", test))]
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct HostDiskMetadata {
    #[serde(default)]
    pub partlabel: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub fstype: Option<String>,
    #[serde(default)]
    pub fsver: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DagfsPresence {
    Detected(MountStatus),
    NeedsAuthorization,
    Blank,
    NotDetected,
    Unavailable(String),
}

#[derive(Clone, Debug)]
pub struct PhysicalDisk {
    pub path: PathBuf,
    pub description: String,
    pub bytes: u64,
    pub host: HostDiskMetadata,
    pub dagfs: DagfsPresence,
}

impl PhysicalDisk {
    /// The host's partition name first, then its filesystem label. Neither
    /// is used to decide whether the medium contains DagFS.
    pub fn name(&self) -> Option<&str> {
        self.host
            .partlabel
            .as_deref()
            .or(self.host.label.as_deref())
    }
}

#[derive(Clone, Debug, Default)]
pub struct PhysicalDisks {
    pub disks: Vec<PhysicalDisk>,
    pub metadata_warning: Option<String>,
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn probe(disk: &Disk) -> DagfsPresence {
    // A permission probe opens then closes a descriptor, so it must hold the
    // local claim: POSIX locks must not be released under an existing reader.
    let claim = match Claim::take(disk) {
        Ok(claim) => claim,
        Err(error) => return DagfsPresence::Unavailable(error.to_string()),
    };
    match std::fs::File::open(disk.path()) {
        Ok(file) => drop(file),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return DagfsPresence::NeedsAuthorization;
        }
        Err(error) => return DagfsPresence::Unavailable(error.to_string()),
    }
    drop(claim);
    match Browser::open(disk) {
        Ok(browser) => DagfsPresence::Detected(browser.status()),
        Err(error) if error.is_blank() => DagfsPresence::Blank,
        Err(Error::Volume(kore_host::volume::VolError::NoVolume(_))) => DagfsPresence::NotDetected,
        // Busy, incompatible and corrupt volumes are not "not DagFS".
        Err(error) => DagfsPresence::Unavailable(error.to_string()),
    }
}

/// Retry only permission-blocked disks in one authorized, read-only batch.
/// Names and filesystem labels do not influence admission or recovery.
pub fn identify_disks(inventory: &mut PhysicalDisks) -> Result<(), Error> {
    #[cfg(target_os = "linux")]
    {
        let disks = inventory
            .disks
            .iter()
            .filter(|disk| disk.dagfs == DagfsPresence::NeedsAuthorization)
            .map(|disk| Disk::new(&disk.path))
            .collect::<Result<Vec<_>, _>>()?;
        if disks.is_empty() {
            return Ok(());
        }
        let results = crate::privileged::identify(&disks)?;
        for (disk, result) in disks.iter().zip(results) {
            if let Some(entry) = inventory
                .disks
                .iter_mut()
                .find(|entry| entry.path == disk.path())
            {
                entry.dagfs = result;
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = inventory;
        Err(Error::Path(
            "Physical disk identification requires Linux.".into(),
        ))
    }
}

/// Query host labels once; recover each readable medium read-only. No auth
/// prompts occur during enumeration. Permission-blocked devices stay unknown.
pub fn physical_disks() -> Result<PhysicalDisks, Error> {
    #[cfg(target_os = "linux")]
    {
        use std::{fs, os::unix::fs::FileTypeExt, path::Path};
        let root = Path::new("/sys/class/block");
        let mut inventory = PhysicalDisks::default();
        let mut metadata = match host_metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                inventory.metadata_warning = Some(error);
                BTreeMap::new()
            }
        };
        for entry in fs::read_dir(root)
            .map_err(|error| Error::host(root, error))?
            .take(256)
        {
            let entry = entry.map_err(|error| Error::host(root, error))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
                continue;
            }
            let sys = entry.path();
            let sectors = fs::read_to_string(sys.join("size"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0);
            if sectors == 0 {
                continue;
            }
            let path = Path::new("/dev").join(name.as_ref());
            if !fs::metadata(&path).is_ok_and(|m| m.file_type().is_block_device()) {
                continue;
            }
            let model = fs::read_to_string(sys.join("device/model")).unwrap_or_default();
            let kind = if sys.join("partition").exists() {
                "Partition"
            } else {
                "Disk"
            };
            let dagfs = match Disk::new(&path) {
                Ok(disk) => probe(&disk),
                Err(error) => DagfsPresence::Unavailable(error.to_string()),
            };
            inventory.disks.push(PhysicalDisk {
                host: metadata.remove(&path).unwrap_or_default(),
                path,
                description: if model.trim().is_empty() {
                    kind.into()
                } else {
                    format!("{kind} · {}", model.trim())
                },
                bytes: sectors.saturating_mul(512),
                dagfs,
            });
        }
        inventory.disks.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(inventory)
    }
    #[cfg(not(target_os = "linux"))]
    Err(Error::Path(
        "Physical disks are supported on Linux. Choose a disk image on this host.".into(),
    ))
}

#[cfg(any(target_os = "linux", test))]
fn parse_metadata(bytes: &[u8]) -> Result<BTreeMap<PathBuf, HostDiskMetadata>, String> {
    #[derive(Deserialize)]
    struct Row {
        path: PathBuf,
        #[serde(flatten)]
        metadata: HostDiskMetadata,
    }
    #[derive(Deserialize)]
    struct Output {
        blockdevices: Vec<Row>,
    }
    if bytes.len() > 1 << 20 {
        return Err("Host disk metadata exceeds 1 MiB.".into());
    }
    let rows: Output =
        serde_json::from_slice(bytes).map_err(|e| format!("Host disk metadata: {e}"))?;
    if rows.blockdevices.len() > 1024 {
        return Err("Too many host disk metadata records.".into());
    }
    let mut result = BTreeMap::new();
    for row in rows.blockdevices {
        let mut metadata = row.metadata;
        for field in [
            &mut metadata.partlabel,
            &mut metadata.label,
            &mut metadata.fstype,
            &mut metadata.fsver,
        ] {
            if field.as_ref().is_some_and(|text| text.is_empty()) {
                *field = None;
            }
            if field.as_ref().is_some_and(|text| text.len() > 4096) {
                return Err("A host disk metadata field exceeds 4 KiB.".into());
            }
        }
        result.insert(row.path, metadata);
    }
    Ok(result)
}

#[cfg(target_os = "linux")]
fn host_metadata() -> Result<BTreeMap<PathBuf, HostDiskMetadata>, String> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        sync::mpsc,
        thread,
        time::Duration,
    };
    // A single bounded unprivileged query uses util-linux's existing udev/
    // blkid integration, avoiding a libudev development dependency or a
    // second implementation of partition-table/filesystem metadata parsing.
    let binary = ["/usr/bin/lsblk", "/bin/lsblk"]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .ok_or("Partition names unavailable: install util-linux (lsblk).")?;
    let mut child = Command::new(binary)
        .args([
            "--json",
            "--list",
            "--output",
            "PATH,PARTLABEL,LABEL,FSTYPE,FSVER",
        ])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Cannot query partition names: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Host metadata pipe unavailable.")?;
    let (send, receive) = mpsc::sync_channel(1);
    let reader = thread::Builder::new()
        .name("vkore-disk-labels".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take((1 << 20) + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = send.send(result);
        });
    if let Err(error) = reader {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Cannot read partition names: {error}"));
    }
    let bytes = receive.recv_timeout(Duration::from_secs(3));
    // Even on success, close a producer that filled the bounded pipe without
    // exiting. All complete output has already been received.
    let exited = child.try_wait().ok().flatten();
    if exited.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let bytes = bytes
        .map_err(|_| "Host disk metadata timed out.".to_owned())?
        .map_err(|e| format!("Cannot read host disk metadata: {e}"))?;
    if exited.is_some_and(|status| !status.success()) {
        return Err(
            "lsblk could not report partition metadata; update util-linux if FSVER is unsupported."
                .into(),
        );
    }
    parse_metadata(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_labels_do_not_claim_dagfs_and_missing_values_remain_unknown() {
        let mut rows = parse_metadata(br#"{"blockdevices":[
            {"path":"/dev/sda1","partlabel":"Kore data","label":"work","fstype":"ext4","fsver":"1.0"},
            {"path":"/dev/sda2","partlabel":null,"label":"","fstype":null,"fsver":null}
        ]}"#).unwrap();
        let disk = PhysicalDisk {
            path: "/dev/sda1".into(),
            description: "Partition".into(),
            bytes: 0,
            host: rows.remove(&PathBuf::from("/dev/sda1")).unwrap(),
            dagfs: DagfsPresence::NeedsAuthorization,
        };
        assert_eq!(disk.name(), Some("Kore data"));
        assert_eq!(disk.host.fstype.as_deref(), Some("ext4"));
        assert_eq!(disk.dagfs, DagfsPresence::NeedsAuthorization);
        assert!(rows[&PathBuf::from("/dev/sda2")].label.is_none());
        assert!(parse_metadata(&vec![b' '; (1 << 20) + 1]).is_err());
        assert!(parse_metadata(b"not JSON").is_err());
    }
    #[test]
    fn detection_recovers_the_engine_version_and_leaves_blank_media_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Disk::create_image(dir.path().join("disk.raw"), 32 << 20).unwrap();
        assert_eq!(probe(&disk), DagfsPresence::Blank);
        assert!(disk.is_blank().unwrap());
        disk.format().unwrap();
        let DagfsPresence::Detected(status) = probe(&disk) else {
            panic!("must detect formatted DagFS")
        };
        assert_eq!(status.format_version, kore_host::volume::FORMAT_VERSION);
        let browser = Browser::open(&disk).unwrap();
        assert!(matches!(probe(&disk), DagfsPresence::Unavailable(_)));
        assert_eq!(browser.status(), status);
    }
}
