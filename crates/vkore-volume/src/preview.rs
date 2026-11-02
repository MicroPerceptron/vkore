//! What a file on a volume looks like: an image (with the capture record
//! Kore's screenshots carry), text, or a binary summary, from its leading
//! bytes alone.
//!
//! A preview decodes no pixel. A PNG's size and its text chunks come before
//! its image data, and that is where Kore's capture writes its record, so the
//! head of a file is all a preview reads: [`PREVIEW_HEAD_BYTES`] at most, with
//! the PNG decoder's own allocations bounded too. A screenshots folder of any
//! size lists in what its heads cost, and a hostile or damaged file costs a
//! bounded refusal.

use std::io::Cursor;

use serde::Serialize;

/// The most leading bytes a preview reads from a volume.
pub const PREVIEW_HEAD_BYTES: usize = 256 << 10;
/// The most the PNG decoder may allocate for the chunks ahead of the image
/// data (text, a colour profile).
const MAX_DECODER_BYTES: usize = 1 << 20;
/// The most text a preview carries.
pub const MAX_TEXT_BYTES: usize = PREVIEW_HEAD_BYTES;
/// The most leading bytes a binary summary carries.
pub const BINARY_HEAD_BYTES: usize = 256;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A file's preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preview {
    /// A PNG: its size, and the text chunks ahead of its image data.
    Image {
        width: u32,
        height: u32,
        /// The image's text chunks, in file order.
        text: Vec<(String, String)>,
        /// What Kore's capture recorded, when the image is a Kore screenshot.
        capture: Option<Box<Capture>>,
    },
    /// UTF-8 text, cut at [`MAX_TEXT_BYTES`] on a character boundary.
    Text {
        text: String,
        lines: usize,
        truncated: bool,
    },
    /// Anything else: its length and leading bytes.
    Binary { len: u64, head: Vec<u8> },
}

/// What Kore's capture app stamps on a screenshot (`user/apps/capture`):
/// PNG's own keywords, then Kore's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Capture {
    /// `Software`: `kore <build>`.
    pub software: Option<String>,
    /// `Creation Time`, when the machine's wall clock was set.
    pub created: Option<String>,
    /// `kore:kernel-build`: the exact build that drew the frame.
    pub kernel_build: Option<String>,
    /// `kore:uptime-ms`.
    pub uptime_ms: Option<String>,
    /// `kore:screen`.
    pub screen: Option<String>,
    /// `kore:mode`: screen, region or window.
    pub mode: Option<String>,
    /// `kore:app`, for a window capture.
    pub app: Option<String>,
    /// `kore:rect`: `x,y,width,height` on its screen.
    pub rect: Option<String>,
    /// `kore:density-percent`.
    pub density_percent: Option<String>,
}

impl Capture {
    /// The keywords the record reads, in the order of its fields. A capture
    /// may carry more `kore:` keywords than these; they stay in the image's
    /// text.
    pub const KEYWORDS: [&'static str; 9] = [
        "Software",
        "Creation Time",
        "kore:kernel-build",
        "kore:uptime-ms",
        "kore:screen",
        "kore:mode",
        "kore:app",
        "kore:rect",
        "kore:density-percent",
    ];

    /// The capture record in `text`, if it carries Kore's stamp.
    pub fn from_text(text: &[(String, String)]) -> Option<Self> {
        let get = |keyword: &str| {
            text.iter()
                .find(|(key, _)| key == keyword)
                .map(|(_, value)| value.clone())
        };
        let [
            software,
            created,
            kernel_build,
            uptime_ms,
            screen,
            mode,
            app,
            rect,
            density,
        ] = Self::KEYWORDS.map(get);
        let capture = Self {
            software,
            created,
            kernel_build,
            uptime_ms,
            screen,
            mode,
            app,
            rect,
            density_percent: density,
        };
        (capture.kernel_build.is_some()
            || capture
                .software
                .as_deref()
                .is_some_and(|software| software.starts_with("kore ")))
        .then_some(capture)
    }
}

/// Whether `bytes` begin as a PNG.
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_SIGNATURE)
}

/// The preview of a file of `len` bytes whose leading bytes are `bytes`
/// (all of it when `len` fits in what was read).
pub fn preview(bytes: &[u8], len: u64) -> Preview {
    let whole = bytes.len() as u64 == len;
    if is_png(bytes)
        && let Ok(image) = png_image(bytes)
    {
        return image;
    }
    if let Some(text) = text(bytes, whole) {
        return text;
    }
    Preview::Binary {
        len,
        head: bytes[..bytes.len().min(BINARY_HEAD_BYTES)].to_vec(),
    }
}

/// A PNG's size and the text chunks ahead of its image data, from its head:
/// the decoder stops where the pixels begin.
fn png_image(bytes: &[u8]) -> Result<Preview, String> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: MAX_DECODER_BYTES,
        },
    );
    decoder.set_ignore_text_chunk(false);
    let reader = decoder.read_info().map_err(|error| error.to_string())?;
    let info = reader.info();
    let text = text_chunks(info);
    let capture = Capture::from_text(&text).map(Box::new);
    Ok(Preview::Image {
        width: info.width,
        height: info.height,
        text,
        capture,
    })
}

fn text_chunks(info: &png::Info<'_>) -> Vec<(String, String)> {
    let mut text = Vec::new();
    for chunk in &info.uncompressed_latin1_text {
        text.push((chunk.keyword.clone(), chunk.text.clone()));
    }
    for chunk in &info.compressed_latin1_text {
        if let Ok(value) = chunk.get_text() {
            text.push((chunk.keyword.clone(), value));
        }
    }
    for chunk in &info.utf8_text {
        if let Ok(value) = chunk.get_text() {
            text.push((chunk.keyword.clone(), value));
        }
    }
    text
}

/// `bytes` as text, if they read as UTF-8 with no NUL and few control
/// characters; a cut at the end of a partial read may split one character.
fn text(bytes: &[u8], whole: bool) -> Option<Preview> {
    let cut = bytes.len().min(MAX_TEXT_BYTES);
    let candidate = &bytes[..cut];
    let decoded = match std::str::from_utf8(candidate) {
        Ok(text) => text,
        // A cut (ours or the read's) may end inside one character.
        Err(error) if (!whole || cut < bytes.len()) && error.error_len().is_none() => {
            std::str::from_utf8(&candidate[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    let controls = decoded
        .chars()
        .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\x0c' | '\x1b'))
        .count();
    if decoded.contains('\0') || controls * 100 > decoded.chars().count().max(1) {
        return None;
    }
    Some(Preview::Text {
        lines: decoded.lines().count(),
        truncated: !whole || cut < bytes.len(),
        text: decoded.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, color: png::ColorType, text: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        for (keyword, value) in text {
            encoder
                .add_text_chunk((*keyword).into(), (*value).into())
                .unwrap();
        }
        let mut writer = encoder.write_header().unwrap();
        let samples = color.samples();
        let data = (0..width * height * samples as u32)
            .map(|at| at as u8)
            .collect::<Vec<_>>();
        writer.write_image_data(&data).unwrap();
        writer.finish().unwrap();
        out
    }

    /// Where the image data begins: what a head must reach.
    fn image_data_at(bytes: &[u8]) -> usize {
        bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("a PNG has image data")
            + 4
    }

    #[test]
    fn a_kore_screenshot_previews_from_its_head_with_its_capture_record() {
        let bytes = png(
            64,
            48,
            png::ColorType::Rgb,
            &[
                ("Software", "kore 304bfef9ca08"),
                ("Creation Time", "2026-10-27T03:12:45Z"),
                ("kore:kernel-build", "304bfef9ca08"),
                ("kore:mode", "screen"),
                ("kore:rect", "0,0,64,48"),
                ("kore:freeze-us", "412"),
            ],
        );
        let len = bytes.len() as u64;
        // The whole file, and a head that stops where the pixels begin, read
        // the same.
        for head in [&bytes[..], &bytes[..image_data_at(&bytes)]] {
            let Preview::Image {
                width,
                height,
                capture,
                text,
            } = preview(head, len)
            else {
                panic!("a PNG previews as an image from {} bytes", head.len());
            };
            assert_eq!((width, height), (64, 48));
            let capture = capture.unwrap();
            assert_eq!(capture.kernel_build.as_deref(), Some("304bfef9ca08"));
            assert_eq!(capture.created.as_deref(), Some("2026-10-27T03:12:45Z"));
            assert_eq!(capture.mode.as_deref(), Some("screen"));
            // A keyword the record does not name stays in the text.
            assert_eq!(text.len(), 6);
            assert!(
                text.iter()
                    .any(|(key, value)| key == "kore:freeze-us" && value == "412")
            );
        }
    }

    #[test]
    fn the_record_reads_every_keyword_it_names() {
        let text = Capture::KEYWORDS
            .iter()
            .map(|keyword| ((*keyword).to_owned(), format!("{keyword}!")))
            .collect::<Vec<_>>();
        let capture = Capture::from_text(&text).unwrap();
        let fields = [
            capture.software,
            capture.created,
            capture.kernel_build,
            capture.uptime_ms,
            capture.screen,
            capture.mode,
            capture.app,
            capture.rect,
            capture.density_percent,
        ];
        for (keyword, field) in Capture::KEYWORDS.iter().zip(fields) {
            assert_eq!(field, Some(format!("{keyword}!")));
        }
    }

    #[test]
    fn other_images_preview_without_a_capture_record() {
        for color in [
            png::ColorType::Rgba,
            png::ColorType::Grayscale,
            png::ColorType::GrayscaleAlpha,
        ] {
            let bytes = png(4, 4, color, &[("Software", "gimp")]);
            let Preview::Image { capture, text, .. } = preview(&bytes, bytes.len() as u64) else {
                panic!("{color:?} previews as an image");
            };
            assert!(capture.is_none());
            assert_eq!(text, [("Software".to_owned(), "gimp".to_owned())]);
        }
    }

    #[test]
    fn a_head_short_of_the_image_data_or_a_damaged_header_is_not_an_image() {
        let bytes = png(8, 8, png::ColorType::Rgb, &[("Software", "kore 1")]);
        let len = bytes.len() as u64;
        assert!(matches!(
            preview(&bytes[..image_data_at(&bytes) - 12], len),
            Preview::Binary { .. }
        ));
        let mut damaged = bytes.clone();
        // The header's width, under its checksum.
        damaged[17] ^= 0xff;
        assert!(matches!(preview(&damaged, len), Preview::Binary { .. }));
    }

    #[test]
    fn text_is_text_and_a_cut_keeps_whole_characters() {
        let log = "boot: ok\nfirst-shell: ready\n";
        assert_eq!(
            preview(log.as_bytes(), log.len() as u64),
            Preview::Text {
                text: log.into(),
                lines: 2,
                truncated: false
            }
        );
        let accented = "é".repeat(10);
        let cut = &accented.as_bytes()[..7];
        let Preview::Text {
            text, truncated, ..
        } = preview(cut, 20)
        else {
            panic!("a cut text is still text");
        };
        assert_eq!(text, "ééé");
        assert!(truncated);
        assert!(matches!(
            preview(&[0, 1, 2, 3, 0xff], 5),
            Preview::Binary { len: 5, .. }
        ));
    }
}
