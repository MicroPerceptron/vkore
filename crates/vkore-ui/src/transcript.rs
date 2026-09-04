//! Conversation-specific layout and lightweight Markdown presentation.
//!
//! Assistant content stays in read-only text editors so it remains selectable.
//! This module only identifies the block structure and inline ranges the UI
//! needs to style; it does not replace the stored response text.

use crate::{fonts, theme};
use iced_core::{Font, Theme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssistantBlock {
    Heading { level: usize, text: String },
    Paragraph(String),
    List(String),
    Quote(String),
    Code { language: String, text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineMarkdown {
    Code,
    Link,
}

#[derive(Debug, Default)]
pub(crate) struct InlineMarkdownHighlighter {
    line: usize,
}

impl iced_core::text::Highlighter for InlineMarkdownHighlighter {
    type Settings = ();
    type Highlight = InlineMarkdown;
    type Iterator<'a> = std::vec::IntoIter<(std::ops::Range<usize>, InlineMarkdown)>;

    fn new(_settings: &Self::Settings) -> Self {
        Self::default()
    }

    fn update(&mut self, _new_settings: &Self::Settings) {}

    fn change_line(&mut self, line: usize) {
        self.line = line;
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        self.line += 1;
        let mut highlights = Vec::new();
        let mut cursor = 0;
        while let Some(start) = line[cursor..].find('`').map(|offset| cursor + offset) {
            let content = start + 1;
            let Some(end) = line[content..].find('`').map(|offset| content + offset + 1) else {
                break;
            };
            highlights.push((start..end, InlineMarkdown::Code));
            cursor = end;
        }
        cursor = 0;
        while let Some(start) = line[cursor..].find('[').map(|offset| cursor + offset) {
            let Some(middle) = line[start..].find("](").map(|offset| start + offset) else {
                break;
            };
            let Some(end) = line[middle + 2..]
                .find(')')
                .map(|offset| middle + 2 + offset + 1)
            else {
                break;
            };
            highlights.push((start..end, InlineMarkdown::Link));
            cursor = end;
        }
        highlights.sort_by_key(|(range, _)| range.start);
        highlights.into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

pub(crate) fn inline_markdown_format(
    highlight: &InlineMarkdown,
    _theme: &Theme,
) -> iced_core::text::highlighter::Format<Font> {
    match highlight {
        InlineMarkdown::Code => iced_core::text::highlighter::Format {
            color: Some(theme::rgb(theme::TEXT_SECONDARY)),
            font: Some(fonts::MONO),
        },
        InlineMarkdown::Link => iced_core::text::highlighter::Format {
            color: Some(theme::rgb(theme::MAGENTA)),
            font: None,
        },
    }
}

impl AssistantBlock {
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Heading { text, .. }
            | Self::Paragraph(text)
            | Self::List(text)
            | Self::Quote(text)
            | Self::Code { text, .. } => text,
        }
    }
}

fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    ["- ", "* ", "+ "]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
        || trimmed.split_once(". ").is_some_and(|(number, _)| {
            !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
        })
}

/// Split a response into typographic blocks. Fenced code is the only block
/// that receives a card in the transcript view.
pub(crate) fn assistant_blocks(source: &str) -> Vec<AssistantBlock> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some(fence) = line.trim_start().strip_prefix("```") {
            let language = fence.trim().to_owned();
            index += 1;
            let start = index;
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                index += 1;
            }
            blocks.push(AssistantBlock::Code {
                language,
                text: lines[start..index].join("\n"),
            });
            if index < lines.len() {
                index += 1;
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            if (1..=6).contains(&level) && trimmed.as_bytes().get(level) == Some(&b' ') {
                blocks.push(AssistantBlock::Heading {
                    level,
                    text: trimmed[level..].trim_start().to_owned(),
                });
                index += 1;
                continue;
            }
        }
        if is_list_item(line) {
            let start = index;
            index += 1;
            while index < lines.len()
                && !lines[index].trim().is_empty()
                && (is_list_item(lines[index])
                    || lines[index].starts_with(' ')
                    || lines[index].starts_with('\t'))
            {
                index += 1;
            }
            blocks.push(AssistantBlock::List(lines[start..index].join("\n")));
            continue;
        }
        if trimmed.starts_with("> ") {
            let start = index;
            index += 1;
            while index < lines.len() && lines[index].trim_start().starts_with("> ") {
                index += 1;
            }
            blocks.push(AssistantBlock::Quote(
                lines[start..index]
                    .iter()
                    .map(|line| line.trim_start().trim_start_matches("> "))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
            continue;
        }

        let start = index;
        index += 1;
        while index < lines.len() {
            let next = lines[index];
            let trimmed = next.trim_start();
            if next.trim().is_empty()
                || trimmed.starts_with("```")
                || trimmed.starts_with('#')
                || trimmed.starts_with("> ")
                || is_list_item(next)
            {
                break;
            }
            index += 1;
        }
        blocks.push(AssistantBlock::Paragraph(lines[start..index].join("\n")));
    }
    blocks
}

pub(crate) fn user_message_width(text: &str, transcript_width: f32) -> f32 {
    const MIN_MESSAGE_WIDTH: f32 = 120.0;
    const MAX_TURN_WIDTH: f32 = 820.0;
    const USER_WIDTH_FRACTION: f32 = 0.76;
    const APPROXIMATE_GLYPH_WIDTH: f32 = 7.0;

    let available =
        (transcript_width.min(MAX_TURN_WIDTH) * USER_WIDTH_FRACTION).max(MIN_MESSAGE_WIDTH);
    let longest_line = text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0) as f32;
    (longest_line * APPROXIMATE_GLYPH_WIDTH).clamp(MIN_MESSAGE_WIDTH, available)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_split_into_typographic_blocks() {
        let blocks = assistant_blocks(
            "# Result\n\nThe [report](https://example.test) uses `inline code`.\n\n- one\n- two\n\n> a note\n\n```rust\nfn main() {}\n```",
        );
        assert_eq!(
            blocks,
            vec![
                AssistantBlock::Heading {
                    level: 1,
                    text: "Result".into(),
                },
                AssistantBlock::Paragraph(
                    "The [report](https://example.test) uses `inline code`.".into()
                ),
                AssistantBlock::List("- one\n- two".into()),
                AssistantBlock::Quote("a note".into()),
                AssistantBlock::Code {
                    language: "rust".into(),
                    text: "fn main() {}".into(),
                },
            ]
        );
    }

    #[test]
    fn inline_code_and_links_receive_distinct_typography() {
        let mut highlighter = InlineMarkdownHighlighter::default();
        let highlights = iced_core::text::Highlighter::highlight_line(
            &mut highlighter,
            "Use `cargo test` and [the report](https://example.test).",
        )
        .collect::<Vec<_>>();
        assert_eq!(highlights.len(), 2);
        assert_eq!(highlights[0].1, InlineMarkdown::Code);
        assert_eq!(highlights[1].1, InlineMarkdown::Link);
        assert_eq!(
            inline_markdown_format(&InlineMarkdown::Code, &Theme::Dark).font,
            Some(fonts::MONO)
        );
        assert_eq!(
            inline_markdown_format(&InlineMarkdown::Link, &Theme::Dark).color,
            Some(theme::rgb(theme::MAGENTA))
        );
    }

    #[test]
    fn user_message_width_is_content_sized_and_responsive() {
        assert_eq!(user_message_width("Hi", 900.0), 120.0);
        assert!((user_message_width(&"x".repeat(200), 900.0) - 623.2).abs() < 0.01);
        assert!((user_message_width(&"x".repeat(200), 300.0) - 228.0).abs() < 0.01);
        assert!(user_message_width("a moderately sized request", 900.0) < 560.0);
    }
}
