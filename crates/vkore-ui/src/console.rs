//! Styled terminal output, always shaped in the registered mono face.
use iced_core::{Color, Length, text::Span};
use iced_widget::{container, rich_text};
use vkored::console::{Color as AnsiColor, Output};

use crate::{UiElement, fonts, theme, typography::Typography, widgets};

const BACKGROUND: Color = Color::from_rgb(0.035, 0.039, 0.05);
const FOREGROUND: Color = Color::from_rgb(0.83, 0.84, 0.87);

fn color(value: AnsiColor, default: Color, bright: bool) -> Color {
    let rgb = match value {
        AnsiColor::Default => return if bright { Color::WHITE } else { default },
        AnsiColor::Rgb(r, g, b) => [r, g, b],
        AnsiColor::Idx(mut index) => {
            if bright && index < 8 {
                index += 8;
            }
            match index {
                0..=15 => [
                    [24, 24, 28],
                    [205, 70, 70],
                    [80, 180, 110],
                    [195, 165, 65],
                    [90, 130, 220],
                    [175, 105, 205],
                    [65, 170, 185],
                    [200, 200, 205],
                    [115, 115, 125],
                    [255, 105, 105],
                    [120, 220, 145],
                    [240, 215, 100],
                    [130, 170, 255],
                    [215, 145, 245],
                    [100, 220, 235],
                    [245, 245, 250],
                ][usize::from(index)],
                16..=231 => {
                    let n = index - 16;
                    let component = |v| if v == 0 { 0 } else { 55 + 40 * v };
                    [component(n / 36), component(n / 6 % 6), component(n % 6)]
                }
                _ => [8 + (index - 232) * 10; 3],
            }
        }
    };
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

pub fn view(output: &Output, ty: Typography) -> UiElement<'_> {
    let spans: Vec<Span<'_, ()>> = output
        .runs
        .iter()
        .map(|run| {
            let style = run.style;
            let mut fg = color(style.foreground, FOREGROUND, style.bold);
            let mut bg = color(style.background, BACKGROUND, false);
            if style.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            if style.dim {
                fg = Color::from_rgb((fg.r + bg.r) / 2., (fg.g + bg.g) / 2., (fg.b + bg.b) / 2.);
            }
            // Parsing removes terminal commands; this second boundary still escapes
            // unsupported glyphs, bidi controls and invisible data before shaping.
            Span::new(fonts::shown(
                fonts::MONO,
                run.text.as_str(),
                fonts::Breaks::Keep,
            ))
            .font(fonts::MONO)
            .color(fg)
            .background(bg)
            .underline(style.underline)
        })
        .collect();
    container(
        widgets::scroll(
            rich_text(spans)
                .font(fonts::MONO)
                .size(ty.size(theme::SIZE_MONO))
                .line_height(iced_core::text::LineHeight::Relative(1.35))
                .color(FOREGROUND),
        )
        .height(240)
        .anchor_bottom(),
    )
    .padding(14)
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(BACKGROUND.into()),
        border: iced_core::Border {
            radius: 8.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}
