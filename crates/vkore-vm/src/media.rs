//! What a Kore boot medium asks of its machine, read before it boots.
//!
//! Kore's ISO is built by `xorriso -as mkisofs -R -r -J -joliet-long` with
//! Limine's configuration at `/boot/limine/limine.conf`, whose
//! `paging_mode` the kernel then requires exactly: a five-level ISO on a CPU
//! without LA57 stops in the loader, where no serial line explains it. This
//! module reads that one file through the ISO's Joliet directory tree, the
//! only part of ISO 9660 it needs, so the refusal comes before boot and in
//! words. Every offset and length is checked against the image; directory
//! extents and the file itself are bounded.
//!
//! No crate was adopted for this: those vetted either drag a FUSE server and
//! a CLI parser into the library (`cdfs`), read only the lossy primary 8.3
//! names (`iso9660`), or are pre-release with their own I/O stack
//! (`hadris-iso`).

use std::{
    fmt,
    fs::File,
    io::{self, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::profile::Paging;

const SECTOR: u64 = 2048;
/// Volume descriptors start at sector 16; a real image has a handful.
const DESCRIPTOR_LIMIT: u64 = 32;
const MAX_DIRECTORY_BYTES: u32 = 1 << 20;
const MAX_CONFIG_BYTES: u32 = 64 << 10;
const LIMINE_CONF: [&str; 3] = ["boot", "limine", "limine.conf"];

/// A boot medium, inspected.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BootMedium {
    pub path: PathBuf,
    pub bytes: u64,
    /// SHA-256 of the whole image, as Kore releases publish it.
    pub sha256: String,
    /// The paging mode its Limine configuration names, when it has one.
    pub paging: Option<Paging>,
}

/// Why a medium could not be read.
#[derive(Debug)]
pub enum MediaError {
    Io(PathBuf, io::Error),
    /// Not an ISO 9660 image Kore's build would make.
    Format(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
            Self::Format(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for MediaError {}

impl BootMedium {
    /// Inspect the ISO at `path`: its digest, and the paging mode Limine
    /// will require.
    pub fn inspect(path: &Path) -> Result<Self, MediaError> {
        let io = |error| MediaError::Io(path.to_path_buf(), error);
        let mut file = File::open(path).map_err(io)?;
        let bytes = file.metadata().map_err(io)?.len();
        let mut hasher = Sha256::new();
        io::copy(&mut file, &mut hasher).map_err(io)?;
        let sha256 = format!("{:x}", hasher.finalize());
        let config =
            read_joliet_file(&mut file, bytes, &LIMINE_CONF).map_err(|error| match error {
                ReadError::Io(error) => io(error),
                ReadError::Format(message) => {
                    MediaError::Format(format!("{}: {message}", path.display()))
                }
            })?;
        let paging = config
            .as_deref()
            .and_then(|text| paging_mode(&String::from_utf8_lossy(text)));
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            sha256,
            paging,
        })
    }
}

/// The `paging_mode` the configuration's entry names.
pub fn paging_mode(config: &str) -> Option<Paging> {
    config.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        if key.trim() != "paging_mode" {
            return None;
        }
        match value.trim() {
            "4level" => Some(Paging::FourLevel),
            "5level" => Some(Paging::FiveLevel),
            _ => None,
        }
    })
}

enum ReadError {
    Io(io::Error),
    Format(String),
}

impl From<io::Error> for ReadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn format_error(message: impl Into<String>) -> ReadError {
    ReadError::Format(message.into())
}

fn read_at<R: io::Read + Seek>(
    image: &mut R,
    len: u64,
    at: u64,
    buffer: &mut [u8],
) -> Result<(), ReadError> {
    at.checked_add(buffer.len() as u64)
        .filter(|end| *end <= len)
        .ok_or_else(|| format_error("a structure lies past the end of the image"))?;
    image.seek(SeekFrom::Start(at))?;
    image.read_exact(buffer)?;
    Ok(())
}

/// A directory record's extent and size, and whether it is a directory.
struct Record {
    extent: u32,
    size: u32,
    directory: bool,
    name: String,
}

fn parse_record(record: &[u8]) -> Result<Record, ReadError> {
    if record.len() < 33 {
        return Err(format_error("a directory record shorter than its header"));
    }
    let name_len = usize::from(record[32]);
    let name = record
        .get(33..33 + name_len)
        .ok_or_else(|| format_error("a directory record's name runs past it"))?;
    // Joliet names are UCS-2 big-endian; `.` and `..` are the single bytes
    // 0 and 1.
    let name = if name_len == 1 {
        String::from(match name[0] {
            0 => ".",
            1 => "..",
            _ => "",
        })
    } else {
        let units = name
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair))
            .collect::<Vec<_>>();
        let mut text = String::from_utf16_lossy(&units);
        if let Some(stripped) = text.strip_suffix(";1") {
            text = stripped.to_owned();
        }
        text
    };
    Ok(Record {
        extent: u32::from_le_bytes(record[2..6].try_into().unwrap()),
        size: u32::from_le_bytes(record[10..14].try_into().unwrap()),
        directory: record[25] & 0x02 != 0,
        name,
    })
}

/// Read the file at `path` through the image's Joliet tree; `None` when the
/// image has no Joliet tree or no such file.
fn read_joliet_file<R: io::Read + Seek>(
    image: &mut R,
    len: u64,
    path: &[&str],
) -> Result<Option<Vec<u8>>, ReadError> {
    let mut descriptor = [0u8; SECTOR as usize];
    let mut root = None;
    for index in 0..DESCRIPTOR_LIMIT {
        read_at(image, len, (16 + index) * SECTOR, &mut descriptor)?;
        if &descriptor[1..6] != b"CD001" {
            return Err(format_error("not an ISO 9660 image (no CD001 descriptor)"));
        }
        match descriptor[0] {
            // A supplementary descriptor with a Joliet escape sequence.
            2 if matches!(&descriptor[88..91], b"%/@" | b"%/C" | b"%/E") => {
                root = Some(parse_record(&descriptor[156..190])?);
                break;
            }
            255 => break,
            _ => {}
        }
    }
    let Some(mut at) = root else {
        return Ok(None);
    };
    for (depth, part) in path.iter().enumerate() {
        if !at.directory || at.size > MAX_DIRECTORY_BYTES {
            return Err(format_error("a directory larger than an ISO's should be"));
        }
        let mut extent = vec![0u8; at.size as usize];
        read_at(image, len, u64::from(at.extent) * SECTOR, &mut extent)?;
        let mut found = None;
        let mut offset = 0usize;
        while offset < extent.len() {
            let record_len = usize::from(extent[offset]);
            if record_len == 0 {
                // Records never cross a sector: skip to the next one.
                offset = (offset / SECTOR as usize + 1) * SECTOR as usize;
                continue;
            }
            let record = extent
                .get(offset..offset + record_len)
                .ok_or_else(|| format_error("a directory record runs past its extent"))?;
            let parsed = parse_record(record)?;
            if parsed.name == *part {
                found = Some(parsed);
                break;
            }
            offset += record_len;
        }
        let Some(next) = found else {
            return Ok(None);
        };
        let last = depth + 1 == path.len();
        if last {
            if next.directory || next.size > MAX_CONFIG_BYTES {
                return Err(format_error("the Limine configuration is not a small file"));
            }
            let mut content = vec![0u8; next.size as usize];
            read_at(image, len, u64::from(next.extent) * SECTOR, &mut content)?;
            return Ok(Some(content));
        }
        at = next;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A directory record: `name` in UCS-2 big-endian (or `.`/`..` bytes).
    fn record(name: &[u8], extent: u32, size: u32, directory: bool) -> Vec<u8> {
        let mut out = vec![0u8; 33];
        out[2..6].copy_from_slice(&extent.to_le_bytes());
        out[6..10].copy_from_slice(&extent.to_be_bytes());
        out[10..14].copy_from_slice(&size.to_le_bytes());
        out[14..18].copy_from_slice(&size.to_be_bytes());
        out[25] = if directory { 2 } else { 0 };
        out[32] = name.len() as u8;
        out.extend_from_slice(name);
        if out.len() % 2 == 1 {
            out.push(0);
        }
        out[0] = out.len() as u8;
        out
    }

    fn ucs2(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    /// A minimal ISO: a primary descriptor, a Joliet descriptor, the
    /// terminator, and `/boot/limine/limine.conf`.
    fn image(config: &[u8]) -> Vec<u8> {
        let sector = SECTOR as usize;
        let mut iso = vec![0u8; 26 * sector];
        let descriptor = |iso: &mut Vec<u8>, index: usize, kind: u8| {
            let at = (16 + index) * sector;
            iso[at] = kind;
            iso[at + 1..at + 6].copy_from_slice(b"CD001");
            iso[at + 6] = 1;
        };
        descriptor(&mut iso, 0, 1);
        descriptor(&mut iso, 1, 2);
        descriptor(&mut iso, 2, 255);
        let joliet = 17 * sector;
        iso[joliet + 88..joliet + 91].copy_from_slice(b"%/E");
        let root_record = record(&[0], 20, SECTOR as u32, true);
        iso[joliet + 156..joliet + 156 + root_record.len()].copy_from_slice(&root_record);
        let directory = |iso: &mut Vec<u8>, at_sector: usize, entries: &[Vec<u8>]| {
            let mut at = at_sector * sector;
            for entry in entries {
                iso[at..at + entry.len()].copy_from_slice(entry);
                at += entry.len();
            }
        };
        directory(
            &mut iso,
            20,
            &[
                record(&[0], 20, 2048, true),
                record(&[1], 20, 2048, true),
                record(&ucs2("boot"), 21, 2048, true),
            ],
        );
        directory(
            &mut iso,
            21,
            &[
                record(&[0], 21, 2048, true),
                record(&[1], 20, 2048, true),
                record(&ucs2("limine"), 22, 2048, true),
            ],
        );
        directory(
            &mut iso,
            22,
            &[
                record(&[0], 22, 2048, true),
                record(&[1], 21, 2048, true),
                record(&ucs2("limine-uefi-cd.bin"), 24, 0, false),
                record(&ucs2("limine.conf;1"), 23, config.len() as u32, false),
            ],
        );
        iso[23 * sector..23 * sector + config.len()].copy_from_slice(config);
        iso
    }

    #[test]
    fn the_limine_configuration_reads_through_the_joliet_tree() {
        let config = b"timeout: 0\n/kore\n    protocol: limine\n    paging_mode: 5level\n";
        let iso = image(config);
        let read = read_joliet_file(&mut Cursor::new(&iso), iso.len() as u64, &LIMINE_CONF)
            .ok()
            .flatten()
            .unwrap();
        assert_eq!(read, config);
        assert_eq!(
            paging_mode(&String::from_utf8_lossy(&read)),
            Some(Paging::FiveLevel)
        );
        assert_eq!(
            paging_mode("    paging_mode: 4level"),
            Some(Paging::FourLevel)
        );
        assert_eq!(paging_mode("kaslr: no"), None);
    }

    #[test]
    fn a_damaged_or_foreign_image_is_refused_in_words() {
        let mut iso = image(b"paging_mode: 4level\n");
        let len = iso.len() as u64;
        // A file extent past the end of the image.
        let at = 22 * SECTOR as usize;
        let entry = record(&ucs2("limine.conf"), 4000, 20, false);
        let offset = record(&[0], 22, 2048, true).len() * 2
            + record(&ucs2("limine-uefi-cd.bin"), 24, 0, false).len();
        iso[at + offset..at + offset + entry.len()].copy_from_slice(&entry);
        assert!(matches!(
            read_joliet_file(&mut Cursor::new(&iso), len, &LIMINE_CONF),
            Err(ReadError::Format(_))
        ));
        let mut foreign = vec![0u8; 20 * SECTOR as usize];
        foreign[16 * SECTOR as usize] = 1;
        assert!(matches!(
            read_joliet_file(
                &mut Cursor::new(&foreign),
                foreign.len() as u64,
                &LIMINE_CONF
            ),
            Err(ReadError::Format(_))
        ));
        assert!(matches!(
            read_joliet_file(&mut Cursor::new(&[0u8; 100]), 100, &LIMINE_CONF),
            Err(ReadError::Format(_))
        ));
    }

    #[test]
    fn inspection_digests_the_whole_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kore.iso");
        std::fs::write(&path, image(b"paging_mode: 4level\n")).unwrap();
        let medium = BootMedium::inspect(&path).unwrap();
        assert_eq!(medium.paging, Some(Paging::FourLevel));
        assert_eq!(medium.sha256.len(), 64);
        assert_eq!(medium.bytes, 26 * SECTOR);
    }
}
