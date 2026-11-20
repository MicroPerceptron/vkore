//! Host image acquisition. Downloads are private, bounded, inspected, synced,
//! then published without replacement. No guest protocol or volume parsing.
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use vkore_vm::BootMedium;

use crate::{MachineConfig, MachineStore, config, state::private_dir};

/// Includes model-bearing ISOs; never trust Content-Length as the bound.
pub const MAX_IMAGE_BYTES: u64 = 64 << 30;

/// Import progress is throttled, so a fast download cannot flood the UI queue.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub phase: Phase,
    pub bytes: u64,
    pub total: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Downloading,
    Verifying,
    Cached,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Downloading => "Downloading image",
            Self::Verifying => "Verifying image",
            Self::Cached => "Using verified cached image",
        }
    }
}

/// Shared desktop/CLI creation path. Existing configurations still use local
/// paths. A downloaded image remains reusable after its machine is removed.
pub fn create(
    store: &MachineStore,
    config: MachineConfig,
    source: &str,
    sha256: Option<&str>,
    cancelled: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<MachineConfig, String> {
    create_from_source(
        store,
        config,
        Source {
            location: source,
            sha256,
            bytes: None,
        },
        cancelled,
        progress,
    )
}

/// Resolve a catalog ID again in the runtime; the view cannot substitute a URL
/// or omit verification. Its selected digest also detects a changed artifact.
pub fn create_official(
    store: &MachineStore,
    mut config: MachineConfig,
    image_id: &str,
    selected_sha256: Option<&str>,
    cancelled: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<MachineConfig, String> {
    if !config::valid_name(image_id) {
        return Err("select an available official image".into());
    }
    let catalog = crate::catalog::fetch(cancelled)?;
    let image = catalog
        .get(image_id)
        .ok_or("select an available official image")?;
    if selected_sha256.is_some_and(|digest| digest != image.sha256) {
        return Err(
            "The selected image changed. Refresh official images and select it again.".into(),
        );
    }
    config.requirements = image.minimum;
    crate::hardware::validate(&config)?;
    create_from_source(
        store,
        config,
        Source {
            location: &image.url,
            sha256: Some(&image.sha256),
            bytes: Some(image.bytes),
        },
        cancelled,
        progress,
    )
}

struct Source<'a> {
    location: &'a str,
    sha256: Option<&'a str>,
    bytes: Option<u64>,
}

fn create_from_source(
    store: &MachineStore,
    mut config: MachineConfig,
    source: Source<'_>,
    cancelled: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<MachineConfig, String> {
    let expected_bytes = source.bytes;
    let sha256 = source.sha256;
    let source = source.location;
    // Validate the rest of the form before doing any network work.
    config.iso =
        std::path::absolute(store.state().root().join("pending.iso")).map_err(|e| e.to_string())?;
    config::validate(&config).map_err(|e| e.to_string())?;
    if store.state().machine(&config.name).config.exists() {
        return Err(format!("a machine named {:?} exists", config.name));
    }
    let expected = sha256.filter(|s| !s.is_empty());
    if expected.is_some_and(|s| s.len() != 64 || !s.bytes().all(|c| c.is_ascii_hexdigit())) {
        return Err("SHA-256 must contain exactly 64 hexadecimal characters".into());
    }
    let source = source.trim();
    let medium = if source.contains("://") {
        let url = url::Url::parse(source).map_err(|_| "invalid image URL")?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err("use an HTTPS image link without embedded credentials".into());
        }
        download(
            store,
            url.as_str(),
            expected,
            expected_bytes,
            cancelled,
            &progress,
        )?
    } else {
        let path = fs::canonicalize(source).map_err(|e| format!("image: {e}"))?;
        let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("image must be a regular ISO file".into());
        }
        if metadata.len() > MAX_IMAGE_BYTES {
            return Err("image exceeds the 64 GiB limit".into());
        }
        let medium = BootMedium::inspect(&path).map_err(|e| e.to_string())?;
        verify(&medium, expected)?;
        medium
    };
    check_cancelled(cancelled)?;
    config.iso = medium.path;
    // This records the imported identity; the boot preflight still rechecks it.
    config.iso_sha256 = Some(medium.sha256);
    store.create(&config).map_err(|e| e.to_string())?;
    Ok(config)
}

fn verify(medium: &BootMedium, expected: Option<&str>) -> Result<(), String> {
    if expected.is_some_and(|expected| !medium.sha256.eq_ignore_ascii_case(expected)) {
        return Err("image SHA-256 does not match the expected digest".into());
    }
    Ok(())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("image import cancelled".into())
    } else {
        Ok(())
    }
}

fn download(
    store: &MachineStore,
    url: &str,
    expected: Option<&str>,
    expected_bytes: Option<u64>,
    cancelled: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<BootMedium, String> {
    check_cancelled(cancelled)?;
    let dir =
        std::path::absolute(store.state().root().join("images")).map_err(|e| e.to_string())?;
    private_dir(&dir).map_err(|e| e.to_string())?;
    if let Some(digest) = expected {
        let path = dir.join(format!("{}.iso", digest.to_ascii_lowercase()));
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_file()
                    || metadata.len() > MAX_IMAGE_BYTES
                    || expected_bytes.is_some_and(|bytes| bytes != metadata.len())
                {
                    return Err("cached image is not the expected regular file; remove the damaged cache entry and retry".into());
                }
                progress(Progress {
                    bytes: metadata.len(),
                    total: expected_bytes,
                    phase: Phase::Verifying,
                });
                let medium = BootMedium::inspect(&path).map_err(|e| e.to_string())?;
                verify(&medium, Some(digest))?;
                check_cancelled(cancelled)?;
                progress(Progress {
                    bytes: medium.bytes,
                    total: Some(medium.bytes),
                    phase: Phase::Cached,
                });
                return Ok(medium);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cached image: {error}")),
        }
    }
    let tls =
        native_tls::TlsConnector::new().map_err(|e| format!("Cannot initialize HTTPS: {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .https_only(true)
        .redirects(5)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(15))
        .timeout(Duration::from_secs(3600))
        .build();
    // Do not include a possibly signed download URL in error messages.
    let response = agent
        .get(url)
        .call()
        .map_err(|_| "image download failed (HTTPS, connection or HTTP status)")?;
    if response.status() != 200 {
        return Err("image download did not return a complete image (HTTP 200)".into());
    }
    let total = response
        .header("Content-Length")
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|_| "invalid image Content-Length")?;
    let total = verified_length(total, expected_bytes)?;
    if total.is_some_and(|n| n > MAX_IMAGE_BYTES) {
        return Err("image exceeds the 64 GiB limit".into());
    }
    publish(
        response.into_reader(),
        &dir,
        total,
        expected,
        cancelled,
        progress,
    )
}

fn verified_length(header: Option<u64>, expected: Option<u64>) -> Result<Option<u64>, String> {
    if let (Some(header), Some(expected)) = (header, expected)
        && header != expected
    {
        return Err("image Content-Length does not match the official catalog".into());
    }
    Ok(expected.or(header))
}

fn publish(
    reader: impl Read,
    dir: &std::path::Path,
    total: Option<u64>,
    expected: Option<&str>,
    cancelled: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<BootMedium, String> {
    let mut partial = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    copy_bounded(
        reader,
        partial.as_file_mut(),
        total,
        MAX_IMAGE_BYTES,
        cancelled,
        progress,
    )?;
    partial.as_file().sync_all().map_err(|e| e.to_string())?;
    check_cancelled(cancelled)?;
    progress(Progress {
        bytes: partial
            .as_file()
            .metadata()
            .map_err(|e| e.to_string())?
            .len(),
        total,
        phase: Phase::Verifying,
    });
    let mut medium = BootMedium::inspect(partial.path()).map_err(|e| e.to_string())?;
    verify(&medium, expected)?;
    check_cancelled(cancelled)?;
    let destination: PathBuf = dir.join(format!("{}.iso", medium.sha256));
    match partial.persist_noclobber(&destination) {
        Ok(_) => {}
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&destination).map_err(|e| e.to_string())?;
            if !metadata.file_type().is_file() || metadata.len() != medium.bytes {
                return Err("cached image is not the expected regular file".into());
            }
            let cached = BootMedium::inspect(&destination).map_err(|e| e.to_string())?;
            verify(&cached, Some(&medium.sha256))?;
        }
        Err(e) => return Err(e.error.to_string()),
    }
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    medium.path = destination;
    Ok(medium)
}

fn copy_bounded(
    mut reader: impl Read,
    writer: &mut impl Write,
    total: Option<u64>,
    limit: u64,
    cancelled: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<(), String> {
    let mut bytes = 0u64;
    let mut buffer = [0; 64 * 1024];
    let mut last = Instant::now();
    progress(Progress {
        bytes,
        total,
        phase: Phase::Downloading,
    });
    loop {
        check_cancelled(cancelled)?;
        let n = reader
            .read(&mut buffer)
            .map_err(|_| "image download interrupted")?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > limit || total.is_some_and(|total| bytes > total) {
            return Err("image download exceeds its byte limit".into());
        }
        writer.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        if last.elapsed() >= Duration::from_millis(250) {
            progress(Progress {
                bytes,
                total,
                phase: Phase::Downloading,
            });
            last = Instant::now();
        }
    }
    if bytes == 0 || total.is_some_and(|total| bytes != total) {
        return Err("image download is incomplete".into());
    }
    progress(Progress {
        bytes,
        total,
        phase: Phase::Downloading,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_identity_and_length_are_required_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let store = MachineStore::new(crate::State::open(root.path()).unwrap());
        assert!(
            create_official(
                &store,
                MachineConfig::new("test", "/pending"),
                "https://example.invalid/not-an-id",
                None,
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
        assert!(store.list().unwrap().is_empty());
        assert_eq!(verified_length(None, Some(12)).unwrap(), Some(12));
        assert_eq!(verified_length(Some(12), Some(12)).unwrap(), Some(12));
        assert!(verified_length(Some(13), Some(12)).is_err());
    }

    // An ISO descriptor fixture, not a bootable Kore image. The VM parser's
    // tests own Joliet semantics; these tests exercise publication and identity.
    fn image() -> Vec<u8> {
        let mut bytes = vec![0u8; 18 * 2048];
        for (sector, kind) in [(16, 1), (17, 255)] {
            let at = sector * 2048;
            bytes[at] = kind;
            bytes[at + 1..at + 6].copy_from_slice(b"CD001");
            bytes[at + 6] = 1;
        }
        bytes
    }

    #[test]
    fn publication_is_verified_atomic_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let bytes = image();
        let import = |expected: Option<&str>| {
            publish(
                &bytes[..],
                dir.path(),
                Some(bytes.len() as u64),
                expected,
                &cancel,
                &|_| {},
            )
        };
        assert!(import(Some(&"0".repeat(64))).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        let first = import(None).unwrap();
        assert_eq!(fs::read(&first.path).unwrap(), bytes);
        assert_eq!(import(Some(&first.sha256)).unwrap().path, first.path);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        fs::write(&first.path, b"damaged").unwrap();
        assert!(import(None).is_err());
        assert_eq!(fs::read(&first.path).unwrap(), b"damaged");
        fs::remove_file(&first.path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), &first.path).unwrap();
        assert!(import(None).is_err());
        assert!(!dir.path().join("elsewhere").exists());
    }

    #[test]
    fn cached_download_is_reverified_without_a_network_request() {
        let root = tempfile::tempdir().unwrap();
        let store = MachineStore::new(crate::State::open(root.path()).unwrap());
        let dir = store.state().root().join("images");
        fs::create_dir(&dir).unwrap();
        let cancel = AtomicBool::new(false);
        let bytes = image();
        let first = publish(
            &bytes[..],
            &dir,
            Some(bytes.len() as u64),
            None,
            &cancel,
            &|_| {},
        )
        .unwrap();
        let phases = std::cell::RefCell::new(Vec::new());
        let cached = download(
            &store,
            "https://127.0.0.1:1/unreachable",
            Some(&first.sha256),
            Some(first.bytes),
            &cancel,
            &|p| phases.borrow_mut().push(p.phase),
        )
        .unwrap();
        assert_eq!(cached.path, first.path);
        assert_eq!(*phases.borrow(), [Phase::Verifying, Phase::Cached]);
        let mut damaged = bytes;
        damaged[0] = 1;
        fs::write(&first.path, damaged).unwrap();
        assert!(
            download(
                &store,
                "https://127.0.0.1:1/unreachable",
                Some(&first.sha256),
                Some(first.bytes),
                &cancel,
                &|_| {}
            )
            .unwrap_err()
            .contains("SHA-256")
        );
    }

    #[test]
    fn interrupted_or_foreign_downloads_leave_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        for (bytes, total) in [(&b"partial"[..], Some(100)), (&b"not an ISO"[..], None)] {
            assert!(publish(bytes, dir.path(), total, None, &cancel, &|_| {}).is_err());
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn local_image_is_inspected_and_recorded_without_copying() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local.iso");
        fs::write(&path, image()).unwrap();
        let store = MachineStore::new(crate::State::open(dir.path().join("state")).unwrap());
        let cancel = AtomicBool::new(false);
        let config = create(
            &store,
            MachineConfig::new("demo", "/pending"),
            path.to_str().unwrap(),
            None,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(config.iso, fs::canonicalize(path).unwrap());
        assert_eq!(config.iso_sha256.as_ref().unwrap().len(), 64);
        assert_eq!(store.get("demo").unwrap(), config);
    }

    #[test]
    fn streamed_bytes_are_bounded_even_without_content_length() {
        let cancelled = AtomicBool::new(false);
        let mut out = Vec::new();
        assert!(copy_bounded(&b"12345"[..], &mut out, None, 4, &cancelled, &|_| {}).is_err());
        assert!(out.is_empty());
        assert!(copy_bounded(&b"12"[..], &mut out, Some(3), 4, &cancelled, &|_| {}).is_err());
        cancelled.store(true, Ordering::Relaxed);
        assert!(copy_bounded(&b"12"[..], &mut out, Some(2), 4, &cancelled, &|_| {}).is_err());
    }
    #[test]
    fn invalid_sources_never_create_a_machine() {
        let dir = tempfile::tempdir().unwrap();
        let store = MachineStore::new(crate::State::open(dir.path()).unwrap());
        for source in [
            "http://example.com/k.iso",
            "https://user:secret@example.com/k.iso",
            "file:///k.iso",
        ] {
            assert!(
                create(
                    &store,
                    MachineConfig::new("demo", "/unused"),
                    source,
                    None,
                    &AtomicBool::new(false),
                    |_| {}
                )
                .is_err()
            );
        }
        assert!(store.list().unwrap().is_empty());
    }
}
