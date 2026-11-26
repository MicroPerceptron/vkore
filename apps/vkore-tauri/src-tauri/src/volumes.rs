//! Browsing a machine's volume, read-only.
//!
//! The volume is DagFS, and no host kernel mounts it: `vkore-volume`'s `Browser`
//! drives Kore's own engine on a thread of its own, so what the window lists and
//! previews is what the machine sees. Nothing here parses volume bytes.
//!
//! The rules the engine's callers keep, kept here:
//! - **A disk has one owner.** A running machine owns its disk, so a volume is
//!   never opened under one, and starting a machine closes its open volume first,
//!   under the same lock, so the two cannot interleave. One volume is open at a
//!   time, which also keeps the engine's handles (32 a session) far from their end.
//! - **Everything is bounded.** A listing returns at most [`MAX_ENTRIES`]
//!   entries, a preview reads at most [`PREVIEW_BYTES`] of a file's head, an
//!   image is read whole only up to [`MAX_IMAGE_BYTES`] and [`MAX_PIXELS`].
//! - **Guest text is data.** File names and contents come from the guest. Each
//!   name is sent twice, raw (to navigate by, never drawn) and escaped (to
//!   draw); contents go through [`escape_text`].

use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use vkore_volume::{Browser, Disk, EntryKind, Listing, Preview, VolPath, bounds, preview};

use crate::runtime::{Refusal, Runtime};
use crate::shown::{escape_text, shown};

/// The most entries one listing returns.
pub const MAX_ENTRIES: usize = 2000;
/// The most leading bytes of a file a preview reads.
pub const PREVIEW_BYTES: usize = 32 << 10;
/// The largest image read whole, to be shown.
pub const MAX_IMAGE_BYTES: u64 = 2 << 20;
/// The most pixels an image may have to be shown.
pub const MAX_PIXELS: u64 = 16_000_000;
/// The terminals' homes the picker offers.
const FACES: u8 = 16;

/// A path on the volume as the window names it. The engine checks every name.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Where {
    /// `"root"`, or `"home"` for a terminal's home.
    pub place: String,
    #[serde(default)]
    pub face: u8,
    pub names: Vec<String>,
}

impl Where {
    fn path(&self) -> Result<VolPath, Refusal> {
        if self.names.len() > bounds::MAX_PATH_DEPTH {
            return Err(Refusal::State("that path is too deep".into()));
        }
        let start = match self.place.as_str() {
            "root" => Ok(VolPath::root()),
            "home" => VolPath::home(self.face),
            _ => {
                return Err(Refusal::State(
                    "a path starts at the root or a terminal's home".into(),
                ));
            }
        }
        .map_err(|error| Refusal::State(error.to_string()))?;
        self.names.iter().try_fold(start, |path, name| {
            path.join(name)
                .map_err(|error| Refusal::State(error.to_string()))
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeView {
    pub machine: String,
    pub generation: u64,
    pub format_version: u32,
    /// Recovery took the fallback root: Kore keeps such a volume read-only.
    pub used_fallback_root: bool,
    /// The terminals' homes that can be browsed.
    pub faces: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    /// The name as the volume holds it: to navigate by, never to draw.
    pub name: String,
    /// The name made safe to draw.
    pub label: String,
    pub directory: bool,
    pub len: u64,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ListingView {
    Directory {
        entries: Vec<EntryView>,
        truncated: bool,
    },
    File {
        len: u64,
    },
    EmptyHome,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureView {
    pub software: Option<String>,
    pub created: Option<String>,
    pub kernel_build: Option<String>,
    pub uptime_ms: Option<String>,
    pub mode: Option<String>,
    pub app: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PreviewView {
    Image {
        width: u32,
        height: u32,
        len: u64,
        /// The image itself, when it is small enough to show.
        data_url: Option<String>,
        capture: Option<CaptureView>,
    },
    Text {
        text: String,
        lines: usize,
        truncated: bool,
        len: u64,
    },
    Binary {
        len: u64,
        head_hex: String,
    },
}

struct Open {
    machine: String,
    browser: Browser,
}

/// The one open volume, if any. Its lock is also taken to start a machine.
#[derive(Clone, Default)]
pub struct Volumes {
    open: Arc<Mutex<Option<Open>>>,
}

fn clip(value: Option<String>) -> Option<String> {
    value.map(|value| shown(&value))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

impl Volumes {
    pub fn lock(&self) -> VolumeGuard<'_> {
        VolumeGuard(
            self.open
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

/// The open volume, held. Holding it keeps a volume from opening or closing.
pub struct VolumeGuard<'a>(MutexGuard<'a, Option<Open>>);

impl VolumeGuard<'_> {
    /// Close the volume if it is machine `name`'s, releasing its disk. Called,
    /// with this guard held, immediately before the machine starts.
    pub fn close_for(&mut self, name: &str) {
        if self.0.as_ref().is_some_and(|open| open.machine == name) {
            self.0.take();
        }
    }

    pub fn close(&mut self) {
        self.0.take();
    }

    /// Open machine `name`'s volume, or keep it if it is already the open one.
    pub fn open(&mut self, runtime: &Runtime, name: &str) -> Result<VolumeView, Refusal> {
        if !vkored::config::valid_name(name) {
            return Err(Refusal::BadName);
        }
        if runtime.is_active(name) {
            return Err(Refusal::State(
                "a running machine owns its disk; stop it to look inside".into(),
            ));
        }
        if self.0.as_ref().is_none_or(|open| open.machine != name) {
            // Release any other disk before taking this one.
            self.0.take();
            let store = runtime.store()?;
            store
                .get(name)
                .map_err(|error| Refusal::State(error.to_string()))?;
            let disk = Disk::new(store.state().machine(name).disk)
                .map_err(|error| Refusal::State(error.to_string()))?;
            let browser = Browser::open(&disk).map_err(|error| {
                Refusal::State(if error.is_blank() {
                    "this disk is blank: the machine formats it at its first boot".into()
                } else if error.is_in_use() {
                    "in use: a machine is running on this disk; stop it first".into()
                } else {
                    error.to_string()
                })
            })?;
            *self.0 = Some(Open {
                machine: name.to_owned(),
                browser,
            });
        }
        let open = self.0.as_ref().expect("opened above");
        let status = open.browser.status();
        Ok(VolumeView {
            machine: name.to_owned(),
            generation: status.generation,
            format_version: status.format_version,
            used_fallback_root: status.used_fallback_root,
            faces: (0..FACES)
                .filter(|face| VolPath::home(*face).is_ok())
                .collect(),
        })
    }

    fn browser(&mut self, runtime: &Runtime, name: &str) -> Result<&Browser, Refusal> {
        // Open on demand, so a close that raced a request never strands the page.
        self.open(runtime, name)?;
        Ok(&self.0.as_ref().expect("opened above").browser)
    }

    pub fn list(
        &mut self,
        runtime: &Runtime,
        name: &str,
        at: &Where,
    ) -> Result<ListingView, Refusal> {
        let path = at.path()?;
        let listing = self
            .browser(runtime, name)?
            .list(&path)
            .map_err(|error| Refusal::State(error.to_string()))?;
        Ok(match listing {
            Listing::Directory { entries } => {
                let truncated = entries.len() > MAX_ENTRIES;
                ListingView::Directory {
                    entries: entries
                        .into_iter()
                        .take(MAX_ENTRIES)
                        .map(|entry| EntryView {
                            label: shown(&entry.name),
                            name: entry.name,
                            directory: entry.kind == EntryKind::Directory,
                            len: entry.len,
                        })
                        .collect(),
                    truncated,
                }
            }
            Listing::File { len } => ListingView::File { len },
            Listing::EmptyHome => ListingView::EmptyHome,
        })
    }

    pub fn preview(
        &mut self,
        runtime: &Runtime,
        name: &str,
        at: &Where,
    ) -> Result<PreviewView, Refusal> {
        let path = at.path()?;
        let browser = self.browser(runtime, name)?;
        let state = |error: vkore_volume::Error| Refusal::State(error.to_string());
        let Listing::File { len } = browser.list(&path).map_err(state)? else {
            return Err(Refusal::State("that is not a file".into()));
        };
        let head = browser.head(&path, PREVIEW_BYTES).map_err(state)?;
        Ok(match preview::preview(&head, len) {
            Preview::Image {
                width,
                height,
                capture,
                ..
            } => {
                let pixels = u64::from(width) * u64::from(height);
                let data_url = if len <= MAX_IMAGE_BYTES && pixels <= MAX_PIXELS {
                    browser.read(&path, MAX_IMAGE_BYTES).ok().map(|bytes| {
                        format!(
                            "data:image/png;base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        )
                    })
                } else {
                    None
                };
                PreviewView::Image {
                    width,
                    height,
                    len,
                    data_url,
                    capture: capture.map(|capture| CaptureView {
                        software: clip(capture.software),
                        created: clip(capture.created),
                        kernel_build: clip(capture.kernel_build),
                        uptime_ms: clip(capture.uptime_ms),
                        mode: clip(capture.mode),
                        app: clip(capture.app),
                    }),
                }
            }
            Preview::Text {
                text,
                lines,
                truncated,
            } => PreviewView::Text {
                text: escape_text(&text),
                lines,
                truncated,
                len,
            },
            Preview::Binary { len, head } => PreviewView::Binary {
                len,
                head_hex: hex(&head),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(place: &str, face: u8, names: &[&str]) -> Where {
        Where {
            place: place.into(),
            face,
            names: names.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    #[test]
    fn a_path_is_checked_by_the_engine_before_anything_opens() {
        assert!(at("home", 0, &["work", "a.txt"]).path().is_ok());
        assert!(at("root", 0, &[]).path().is_ok());
        for bad in [
            at("elsewhere", 0, &[]),
            at("home", 200, &[]),
            at("home", 0, &[".."]),
            at("home", 0, &["a/b"]),
            at("home", 0, &[""]),
            at("home", 0, &[&"n".repeat(64)]),
        ] {
            assert!(bad.path().is_err(), "{bad:?}");
        }
        let deep: Vec<&str> = vec!["d"; bounds::MAX_PATH_DEPTH + 1];
        assert!(at("home", 0, &deep).path().is_err());
    }

    #[test]
    fn a_bad_machine_name_is_refused_before_the_store_is_opened() {
        let runtime = Runtime::new("/nonexistent/state".into(), None);
        let volumes = Volumes::default();
        assert_eq!(
            volumes.lock().open(&runtime, "../x").unwrap_err(),
            Refusal::BadName
        );
    }

    #[test]
    fn a_machine_that_does_not_exist_opens_nothing() {
        let root =
            std::env::temp_dir().join(format!("vkore-tauri-vol-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.clone(), None);
        let volumes = Volumes::default();
        assert!(matches!(
            volumes.lock().open(&runtime, "ghost"),
            Err(Refusal::State(_))
        ));
        assert!(volumes.lock().0.is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn closing_for_another_machine_keeps_the_volume() {
        let volumes = Volumes::default();
        let mut guard = volumes.lock();
        guard.close_for("anything");
        assert!(guard.0.is_none());
    }

    #[test]
    fn bytes_are_shown_as_hex() {
        assert_eq!(hex(&[0, 1, 0xab, 0xff]), "0001abff");
    }

    const PNG_1X1: [u8; 67] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// A machine whose formatted disk holds a small project in terminal 0's home.
    fn machine_with_files(tag: &str) -> (std::path::PathBuf, Runtime, Disk) {
        let root =
            std::env::temp_dir().join(format!("vkore-tauri-vol-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.clone(), None);
        let store = runtime.store().unwrap();
        store
            .create(&vkored::MachineConfig::new("kore-dev", "/images/kore.iso"))
            .unwrap();
        let disk = Disk::create_image(store.state().machine("kore-dev").disk, 64 << 20).unwrap();
        disk.format().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join("sub")).unwrap();
        std::fs::write(project.join("notes.txt"), "line one\nline \u{202E}two\n").unwrap();
        std::fs::write(project.join("blob.bin"), [0u8, 1, 2, 255, 0, 7]).unwrap();
        std::fs::write(project.join("dot.png"), PNG_1X1).unwrap();
        std::fs::write(project.join("sub").join("inner.txt"), "inner").unwrap();
        std::fs::write(project.join("evil\u{202E}.txt"), "x").unwrap();
        disk.install(&project, &VolPath::parse(0, "proj").unwrap(), false)
            .unwrap();
        (root, runtime, disk)
    }

    #[test]
    fn a_stored_volume_is_listed_and_previewed_as_the_window_sees_it() {
        let (root, runtime, disk) = machine_with_files("view");
        let volumes = Volumes::default();
        let mut open = volumes.lock();
        let view = open.open(&runtime, "kore-dev").unwrap();
        crate::assert_camel_keys(&serde_json::to_value(&view).unwrap());
        assert_eq!(view.machine, "kore-dev");
        assert!(view.faces.contains(&0));

        let home = Where {
            place: "home".into(),
            face: 0,
            names: vec!["proj".into()],
        };
        let listed = open.list(&runtime, "kore-dev", &home).unwrap();
        crate::assert_camel_keys(&serde_json::to_value(&listed).unwrap());
        let ListingView::Directory { entries, truncated } = listed else {
            panic!("a directory");
        };
        assert!(!truncated);
        let names: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(
            names.contains(&"notes.txt") && names.contains(&"sub"),
            "{names:?}"
        );
        // Directories come first, and a hidden character is escaped in the label but kept in the name.
        assert!(entries[0].directory);
        let evil = entries
            .iter()
            .find(|e| e.name.contains('\u{202E}'))
            .unwrap();
        assert!(
            evil.label.contains("\\u{202E}") && !evil.label.contains('\u{202E}'),
            "{}",
            evil.label
        );

        let file = |name: &str| Where {
            place: "home".into(),
            face: 0,
            names: vec!["proj".into(), name.into()],
        };
        // The page reads these names exactly: a snake_case key arrives as `undefined`.
        for name in ["notes.txt", "dot.png", "blob.bin"] {
            let wire =
                serde_json::to_value(open.preview(&runtime, "kore-dev", &file(name)).unwrap())
                    .unwrap();
            crate::assert_camel_keys(&wire);
        }
        let png = serde_json::to_value(
            open.preview(&runtime, "kore-dev", &file("dot.png"))
                .unwrap(),
        )
        .unwrap();
        assert!(
            png["dataUrl"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png"),
            "{png}"
        );
        let bin = serde_json::to_value(
            open.preview(&runtime, "kore-dev", &file("blob.bin"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(bin["headHex"], "000102ff0007");
        match open
            .preview(&runtime, "kore-dev", &file("notes.txt"))
            .unwrap()
        {
            PreviewView::Text {
                text,
                lines,
                truncated,
                ..
            } => {
                assert!(text.starts_with("line one"));
                assert!(
                    text.contains("\\u{202E}") && !text.contains('\u{202E}'),
                    "{text:?}"
                );
                assert_eq!((lines, truncated), (2, false));
            }
            other => panic!("text, got {other:?}"),
        }
        match open
            .preview(&runtime, "kore-dev", &file("dot.png"))
            .unwrap()
        {
            PreviewView::Image {
                width,
                height,
                data_url,
                ..
            } => {
                assert_eq!((width, height), (1, 1));
                assert!(
                    data_url
                        .unwrap()
                        .starts_with("data:image/png;base64,iVBORw0KGgo")
                );
            }
            other => panic!("image, got {other:?}"),
        }
        match open
            .preview(&runtime, "kore-dev", &file("blob.bin"))
            .unwrap()
        {
            PreviewView::Binary { len, head_hex } => {
                assert_eq!((len, head_hex.as_str()), (6, "000102ff0007"))
            }
            other => panic!("binary, got {other:?}"),
        }
        // A directory is not previewed, and a path that is not there is refused.
        assert!(open.preview(&runtime, "kore-dev", &home).is_err());
        assert!(open.list(&runtime, "kore-dev", &file("missing")).is_err());

        // The open volume holds the disk; closing it releases the disk to its next owner.
        assert!(
            disk.install(
                &root.join("project"),
                &VolPath::parse(0, "again").unwrap(),
                false
            )
            .is_err()
        );
        open.close_for("someone-else");
        assert!(open.0.is_some());
        open.close_for("kore-dev");
        assert!(open.0.is_none());
        disk.install(
            &root.join("project"),
            &VolPath::parse(0, "again").unwrap(),
            false,
        )
        .unwrap();
        drop(open);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_blank_disk_is_explained_and_nothing_stays_open() {
        let root =
            std::env::temp_dir().join(format!("vkore-tauri-vol-blank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.clone(), None);
        let store = runtime.store().unwrap();
        store
            .create(&vkored::MachineConfig::new("kore-dev", "/images/kore.iso"))
            .unwrap();
        Disk::create_image(store.state().machine("kore-dev").disk, 64 << 20).unwrap();
        let volumes = Volumes::default();
        let Err(Refusal::State(message)) = volumes.lock().open(&runtime, "kore-dev") else {
            panic!("a blank disk is refused");
        };
        assert!(message.contains("blank"), "{message}");
        assert!(volumes.lock().0.is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    // Real-sized PNGs: a stored-deflate encoder, so no new dependency is needed to test.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A valid RGB PNG of `width` x `height` with stored (uncompressed) deflate blocks.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let chunk = |kind: &[u8; 4], data: &[u8]| {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
            out
        };
        let mut raw = Vec::new();
        for y in 0..height {
            raw.push(0);
            for x in 0..width {
                raw.extend_from_slice(&[(x * 7) as u8, (y * 3) as u8, (x ^ y) as u8]);
            }
        }
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
        for (i, block) in blocks.iter().enumerate() {
            z.push(u8::from(i + 1 == blocks.len()));
            z.extend_from_slice(&(block.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
            z.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for byte in &raw {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut ihdr = width.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(b"IDAT", &z));
        out.extend(chunk(b"IEND", &[]));
        out
    }

    #[test]
    fn images_within_the_limit_are_shown_and_larger_ones_are_not() {
        let root =
            std::env::temp_dir().join(format!("vkore-tauri-vol-pics-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.clone(), None);
        let store = runtime.store().unwrap();
        store
            .create(&vkored::MachineConfig::new("kore-dev", "/images/kore.iso"))
            .unwrap();
        let disk = Disk::create_image(store.state().machine("kore-dev").disk, 256 << 20).unwrap();
        disk.format().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        // 0.34 MiB, 1.2 MiB, and 2.6 MiB (over the 2 MiB limit): real, valid PNGs.
        for (name, w, h) in [
            ("small.png", 400, 300),
            ("mid.png", 700, 600),
            ("big.png", 960, 960),
        ] {
            std::fs::write(project.join(name), png(w, h)).unwrap();
        }
        disk.install(&project, &VolPath::parse(0, "pics").unwrap(), false)
            .unwrap();
        let volumes = Volumes::default();
        let mut open = volumes.lock();
        let shown = |open: &mut VolumeGuard<'_>, name: &str| {
            let at = Where {
                place: "home".into(),
                face: 0,
                names: vec!["pics".into(), name.into()],
            };
            match open.preview(&runtime, "kore-dev", &at).unwrap() {
                PreviewView::Image {
                    width,
                    height,
                    data_url,
                    ..
                } => (width, height, data_url),
                other => panic!("{name}: an image, got {other:?}"),
            }
        };
        let (w, h, url) = shown(&mut open, "small.png");
        assert_eq!((w, h), (400, 300));
        assert!(
            url.unwrap()
                .starts_with("data:image/png;base64,iVBORw0KGgo")
        );
        let (_, _, url) = shown(&mut open, "mid.png");
        assert!(url.is_some());
        let (w, h, url) = shown(&mut open, "big.png");
        assert_eq!((w, h), (960, 960));
        assert!(
            url.is_none(),
            "a file over the image limit is described, not sent"
        );
        drop(open);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
