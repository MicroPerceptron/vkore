//! Private, bounded pipe protocol between the desktop and its native Servo helper.
use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

#[cfg(feature = "presentation")]
pub mod presentation;

pub const VERSION: u8 = 3;
pub const MAX_TABS: usize = 4;
pub const MAX_ADDRESS: usize = 2048;
pub const MAX_WIDTH: u32 = 8192;
pub const MAX_HEIGHT: u32 = 8192;
pub const MAX_PIXELS: u64 = 16_777_216;
const MAX_HEADER: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    ShowSurface(bool),
    Open(u64),
    Select(u64),
    Close(u64),
    Navigate(u64, String),
    Back(u64),
    Resize(u64, u32, u32, f32),
    Pointer(u64, u32, u32, Pointer),
    Key(u64, String, bool, u8),
    // The helper paints only the selected tab, on demand, at most one frame in flight.
    Paint(u64),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Pointer {
    Move,
    Down,
    Up,
    Wheel(i32),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Event {
    Ready(u8, String, Surface),
    Changed(u64),
    Location(u64, String, bool),
    Loading(u64, bool),
    Error(String),
    // The helper has presented this tab to its native surface.
    Presented(u64),
    DmaFrame(u64, DmaBuffer),
}

/// A desktop compositor resource, never an address in the other process.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Surface {
    MacIoSurface,
    X11(u32),
    Wayland,
    Win32(isize),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DmaBuffer {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub modifier: u64,
    pub stride: u32,
    pub offset: u32,
}

pub fn check_size(width: u32, height: u32) -> io::Result<()> {
    if !(1..=MAX_WIDTH).contains(&width)
        || !(1..=MAX_HEIGHT).contains(&height)
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid browser frame dimensions",
        ));
    }
    Ok(())
}

pub fn write<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > MAX_HEADER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "browser message too large",
        ));
    }
    writer.write_all(&(data.len() as u32).to_le_bytes())?;
    writer.write_all(&data)
}

pub fn read<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let size = u32::from_le_bytes(prefix) as usize;
    if size > MAX_HEADER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "browser message too large",
        ));
    }
    let mut data = vec![0; size];
    reader.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_acknowledges_only_the_painted_tab() {
        let mut stream = Vec::new();
        write(&mut stream, &Event::Presented(7)).unwrap();
        write(&mut stream, &Event::Changed(8)).unwrap();
        let mut input = stream.as_slice();
        assert!(matches!(
            read::<Event>(&mut input).unwrap(),
            Event::Presented(7)
        ));
        assert!(matches!(
            read::<Event>(&mut input).unwrap(),
            Event::Changed(8)
        ));
        assert!(input.is_empty());
    }

    #[test]
    fn rejects_large_headers_and_invalid_frames_before_allocation() {
        assert!(read::<Event>(&mut &u32::MAX.to_le_bytes()[..]).is_err());
        assert!(check_size(MAX_WIDTH, u32::MAX).is_err());
        assert!(check_size(0, 1).is_err());
        assert!(check_size(MAX_WIDTH, MAX_HEIGHT).is_err());
        assert!(check_size(960, 540).is_ok());
    }
}
