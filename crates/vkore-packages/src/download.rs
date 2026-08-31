use crate::PackageError;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;

pub(crate) fn resumable_download(
    url: &str,
    partial: &Path,
    expected_bytes: u64,
) -> Result<(), PackageError> {
    if let Some(parent) = partial.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = partial.metadata().map_or(0, |metadata| metadata.len());
    if existing > expected_bytes {
        return Err(PackageError::DownloadTooLarge);
    }
    let mut request = ureq::get(url);
    if existing > 0 {
        request = request.set("Range", &format!("bytes={existing}-"));
    }
    let response = request
        .call()
        .map_err(|error| PackageError::Download(error.to_string()))?;
    let append = existing > 0 && response.status() == 206;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(partial)?;
    let mut reader = response.into_reader();
    let mut total = if append { existing } else { 0 };
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| PackageError::Download(error.to_string()))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > expected_bytes {
            return Err(PackageError::DownloadTooLarge);
        }
        file.write_all(&buffer[..read])?;
    }
    file.sync_all()?;
    if total != expected_bytes {
        return Err(PackageError::DownloadIncomplete {
            expected: expected_bytes,
            actual: total,
        });
    }
    Ok(())
}
