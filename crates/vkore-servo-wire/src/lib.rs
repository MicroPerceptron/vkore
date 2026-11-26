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

pub const VERSION: u8 = 4;
pub const MAX_TABS: usize = 4;
pub const MAX_ADDRESS: usize = 2048;
/// Characters of a page title the helper sends.
pub const MAX_TITLE: usize = 256;
/// Characters of the hovered link's address the helper sends.
pub const MAX_STATUS: usize = 512;
/// The side of the largest favicon sent, in pixels. A tab shows 16 logical
/// pixels; 32 covers a 2x display without a large message.
pub const MAX_ICON_SIDE: u32 = 32;
pub const MAX_WIDTH: u32 = 8192;
pub const MAX_HEIGHT: u32 = 8192;
pub const MAX_PIXELS: u64 = 16_777_216;
const MAX_HEADER: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    ShowSurface(bool),
    Open(u64),
    Select(u64),
    Close(u64),
    Navigate(u64, String),
    Back(u64),
    Forward(u64),
    Reload(u64),
    Resize(u64, u32, u32, f32),
    Pointer(u64, u32, u32, Pointer),
    Key(u64, String, bool, u8),
    // The helper paints only the selected tab, on demand, at most one frame in flight.
    Paint(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The tab's address, and whether it can go back and forward.
    Location(u64, String, bool, bool),
    Loading(u64, Load),
    /// The page's title, empty when it has none. At most `MAX_TITLE` characters.
    Title(u64, String),
    /// The page's icon, or none.
    Favicon(u64, Option<Icon>),
    /// The address of the link under the pointer, empty when there is none.
    Status(u64, String),
    Cursor(u64, Cursor),
    Error(String),
    // The helper has presented this tab to its native surface.
    Presented(u64),
    DmaFrame(u64, DmaBuffer),
}

/// How far a page load has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Load {
    /// Requested; nothing parsed yet.
    Started,
    /// The head is parsed: the page can start to show.
    Parsed,
    /// Finished, or stopped.
    Done,
}

/// The pointer's shape over a page, as far as the desktop can draw them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cursor {
    #[default]
    Default,
    None,
    Pointer,
    Text,
    Crosshair,
    Wait,
    Progress,
    Help,
    ContextMenu,
    Cell,
    Alias,
    Copy,
    Move,
    NoDrop,
    NotAllowed,
    Grab,
    Grabbing,
    AllScroll,
    ResizeHorizontal,
    ResizeVertical,
    ResizeDiagonalUp,
    ResizeDiagonalDown,
    ResizeColumn,
    ResizeRow,
    ZoomIn,
    ZoomOut,
}

/// A small RGBA image, carried as hex so a message stays text and bounded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub hex: String,
}

impl Icon {
    /// Packs `rgba` (width x height x 4 bytes) for the wire.
    pub fn pack(width: u32, height: u32, rgba: &[u8]) -> io::Result<Self> {
        let icon = Self {
            width,
            height,
            hex: rgba.iter().map(|byte| format!("{byte:02x}")).collect(),
        };
        icon.pixels()?;
        Ok(icon)
    }

    /// The pixels, after checking the size against the limits and the hex
    /// against the size. Nothing is allocated before it passes.
    pub fn pixels(&self) -> io::Result<Vec<u8>> {
        let bad = |why| io::Error::new(io::ErrorKind::InvalidData, why);
        if !(1..=MAX_ICON_SIDE).contains(&self.width) || !(1..=MAX_ICON_SIDE).contains(&self.height)
        {
            return Err(bad("invalid favicon dimensions"));
        }
        let bytes = self.width as usize * self.height as usize * 4;
        if self.hex.len() != bytes * 2 || !self.hex.is_ascii() {
            return Err(bad("favicon data does not match its dimensions"));
        }
        let digit = |c: u8| match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(bad("favicon data is not hexadecimal")),
        };
        self.hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| Ok(digit(pair[0])? << 4 | digit(pair[1])?))
            .collect()
    }
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
    fn an_icon_round_trips_and_is_checked_against_its_size() {
        let rgba: Vec<u8> = (0..16 * 16 * 4).map(|n| (n % 251) as u8).collect();
        let icon = Icon::pack(16, 16, &rgba).unwrap();
        assert_eq!(icon.pixels().unwrap(), rgba);
        let mut stream = Vec::new();
        write(&mut stream, &Event::Favicon(3, Some(icon.clone()))).unwrap();
        let Event::Favicon(3, Some(read_back)) = read::<Event>(&mut stream.as_slice()).unwrap()
        else {
            panic!("a favicon event");
        };
        assert_eq!(read_back, icon);
        // Too large, wrong length, and not hex are all refused.
        assert!(
            Icon::pack(
                MAX_ICON_SIDE + 1,
                1,
                &vec![0; (MAX_ICON_SIDE as usize + 1) * 4]
            )
            .is_err()
        );
        assert!(Icon::pack(2, 2, &[0; 15]).is_err());
        assert!(Icon::pack(0, 4, &[]).is_err());
        let mut bad = icon.clone();
        bad.hex.replace_range(0..1, "g");
        assert!(bad.pixels().is_err());
        bad = icon;
        bad.hex.truncate(bad.hex.len() - 2);
        assert!(bad.pixels().is_err());
    }

    #[test]
    fn the_largest_favicon_fits_in_one_message() {
        let largest = vec![255; (MAX_ICON_SIDE * MAX_ICON_SIDE * 4) as usize];
        let icon = Icon::pack(MAX_ICON_SIDE, MAX_ICON_SIDE, &largest).unwrap();
        let mut stream = Vec::new();
        write(&mut stream, &Event::Favicon(0, Some(icon))).unwrap();
        // Well inside the header limit: a favicon can never be refused for size.
        assert!(stream.len() < MAX_HEADER / 2, "{} bytes", stream.len());
        // So can the longest title and status.
        let title = "é".repeat(MAX_TITLE);
        let status = "é".repeat(MAX_STATUS);
        write(&mut stream, &Event::Title(0, title)).unwrap();
        write(&mut stream, &Event::Status(0, status)).unwrap();
    }

    #[test]
    fn the_new_events_and_commands_survive_the_wire() {
        let mut stream = Vec::new();
        write(&mut stream, &Event::Loading(1, Load::Parsed)).unwrap();
        write(&mut stream, &Event::Cursor(1, Cursor::Pointer)).unwrap();
        write(
            &mut stream,
            &Event::Location(1, "https://example.org/".into(), true, false),
        )
        .unwrap();
        write(&mut stream, &Command::Forward(1)).unwrap();
        write(&mut stream, &Command::Reload(1)).unwrap();
        let mut input = stream.as_slice();
        assert!(matches!(
            read::<Event>(&mut input).unwrap(),
            Event::Loading(1, Load::Parsed)
        ));
        assert!(matches!(
            read::<Event>(&mut input).unwrap(),
            Event::Cursor(1, Cursor::Pointer)
        ));
        assert!(matches!(
            read::<Event>(&mut input).unwrap(),
            Event::Location(1, _, true, false)
        ));
        assert!(matches!(
            read::<Command>(&mut input).unwrap(),
            Command::Forward(1)
        ));
        assert!(matches!(
            read::<Command>(&mut input).unwrap(),
            Command::Reload(1)
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
