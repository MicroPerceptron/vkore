//! A collapsible navigation rail with an always-reachable account summary.
use crate::{
    Message, Page, UiElement, account, fonts,
    kit::{self, Status},
    motion::{self, Beat},
    splash, theme,
    typography::Typography,
    visuals::{self, Icon},
    widgets,
};
use iced_core::{
    Length, alignment,
    text::{Ellipsis, Wrapping},
};
use iced_widget::{button, column, container, row, text, tooltip};

const VERSION: &str = concat!(
    "v",
    env!("CARGO_PKG_VERSION"),
    "-",
    env!("VKORE_BUILD_REVISION")
);

pub fn view<'a>(
    current: Page,
    running: usize,
    status: &'a str,
    collapsed: bool,
    account: account::Preview,
    ty: Typography,
) -> UiElement<'a> {
    let tip = |content: UiElement<'a>, label: UiElement<'a>| -> UiElement<'a> {
        kit::tip(content, label, tooltip::Position::Right)
    };
    let toggle = button(visuals::icon(
        if collapsed {
            Icon::Expand
        } else {
            Icon::Collapse
        },
        theme::TEXT_MUTED,
        20.,
    ))
    .width(36)
    .height(36)
    .padding(8)
    .style(widgets::button_style(widgets::Button::Ghost))
    .on_press(Message::ToggleNavigation);
    let toggle = tip(
        container(toggle).id("navigation-toggle").into(),
        ty.body(
            if collapsed {
                "Expand navigation"
            } else {
                "Collapse navigation"
            },
            12.,
            theme::TEXT,
        )
        .into(),
    );
    let header: UiElement<'a> = if collapsed {
        column![container(splash::mark(32.)).center_x(44), toggle]
            .spacing(12)
            .align_x(alignment::Horizontal::Center)
            .into()
    } else {
        column![
            row![
                splash::mark(32.),
                text("vKore")
                    .font(fonts::DISPLAY)
                    .size(23.)
                    .style(theme::text(theme::TEXT)),
                iced_widget::space::horizontal(),
                toggle
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
            ty.body("Your Kore workspace", 11., theme::TEXT_MUTED),
        ]
        .spacing(6)
        .into()
    };
    let nav = |label, page, icon| -> UiElement<'a> {
        let active = current == page;
        let glyph = visuals::icon(
            icon,
            if active {
                theme::MAGENTA
            } else {
                theme::TEXT_MUTED
            },
            20.,
        );
        let content: UiElement<'a> = if collapsed {
            container(glyph)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        } else {
            row![glyph, ty.nav(label)]
                .spacing(12)
                .align_y(alignment::Vertical::Center)
                .into()
        };
        let kind = widgets::Button::Navigation(active);
        let control = kit::lift(
            kind,
            true,
            button(content)
                .height(44)
                .padding([0, 12])
                .width(Length::Fill)
                .style(widgets::button_style_fading(kind))
                .on_press(Message::Show(page)),
        );
        if collapsed {
            tip(control, ty.body(label, 12., theme::TEXT).into())
        } else {
            control
        }
    };
    let mut rail = column![header, iced_widget::space::vertical().height(20)].spacing(6);
    if !collapsed {
        rail = rail.push(ty.label("Workspace", theme::TEXT_MUTED));
    }
    rail = rail
        .push(nav("Machines", Page::Machines, Icon::Machine))
        .push(nav("Volumes", Page::Volumes, Icon::Volume))
        .push(nav("Plugins", Page::Plugins, Icon::Plugin))
        .push(iced_widget::space::vertical())
        .push(nav("Settings", Page::Settings, Icon::Settings));
    // Machines running are alive, so the mark pulses; none running is an
    // outline that holds still.
    let fleet = Status {
        beat: if running == 0 {
            Beat::Hollow
        } else {
            Beat::Pulse
        },
        tone: if running == 0 {
            theme::TEXT_MUTED
        } else {
            theme::SUCCESS
        },
        label: if running == 0 { "idle" } else { "running" },
        steps: None,
    };
    let running_label = if running == 0 {
        "No machines running".to_owned()
    } else {
        format!("{running} running")
    };
    if collapsed {
        rail = rail.push(tip(
            container(
                column![
                    motion::beacon(fleet.beat, fleet.tone, ty.size(14.)),
                    ty.body(format!("{running}"), 11., fleet.tone),
                ]
                .spacing(theme::SPACE_1)
                .align_x(alignment::Horizontal::Center),
            )
            .center_x(Length::Fill)
            .padding([8, 0])
            .into(),
            ty.body(running_label, 12., theme::TEXT).into(),
        ));
    } else {
        rail = rail.push(
            container(kit::status_as(
                ty,
                fleet,
                if running == 0 {
                    "idle".to_owned()
                } else {
                    format!("{running} running")
                },
            ))
            .padding([10, 0]),
        );
    }
    let signed_in = account.email.is_some();
    let label = if account.pending {
        "Signing in…"
    } else if signed_in {
        "Signed in"
    } else {
        "Local workspace"
    };
    let detail = account.email.unwrap_or_else(|| {
        if account.pending {
            "Continue in your browser"
        } else {
            "Sign in for downloads"
        }
        .to_owned()
    });
    let icon = visuals::icon(
        Icon::Account,
        if signed_in {
            theme::SUCCESS
        } else {
            theme::TEXT_SECONDARY
        },
        20.,
    );
    let content: UiElement<'a> = if collapsed {
        icon
    } else {
        row![
            icon,
            column![
                ty.body(label, 12., theme::TEXT),
                ty.data(detail.clone(), theme::TEXT_MUTED)
                    .size(ty.size(11.))
                    .width(Length::Fill)
                    .wrapping(Wrapping::None)
                    .ellipsis(Ellipsis::End)
            ]
            .spacing(3)
            .width(Length::Fill)
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Center)
        .into()
    };
    let account_button = container(
        button(content)
            .padding(if collapsed { 12 } else { 10 })
            .width(Length::Fill)
            .style(widgets::button_style(widgets::Button::Choice(
                current == Page::Account,
            )))
            .on_press(Message::Show(Page::Account)),
    )
    .id("navigation-account")
    .into();
    rail = rail.push(tip(
        account_button,
        column![
            ty.body("Account", 12., theme::TEXT),
            ty.data(detail, theme::TEXT_SECONDARY)
        ]
        .spacing(4)
        .into(),
    ));
    if !status.is_empty() {
        let alert = visuals::icon(Icon::Alert, theme::ERROR, ty.size(18.));
        rail = rail.push(if collapsed {
            tip(
                container(alert).center_x(Length::Fill).into(),
                ty.data(status, theme::ERROR).into(),
            )
        } else {
            row![alert, ty.data(status, theme::ERROR).width(Length::Fill)]
                .spacing(theme::SPACE_2)
                .into()
        });
    }
    let version = ty.body(
        if collapsed {
            concat!("v", env!("CARGO_PKG_VERSION"))
        } else {
            VERSION
        },
        if collapsed { 9. } else { 11. },
        theme::TEXT_MUTED,
    );
    rail = rail.push(tip(
        container(version)
            .padding([8, 0])
            .center_x(Length::Fill)
            .into(),
        ty.body(VERSION, 12., theme::TEXT).into(),
    ));
    container(rail)
        .id("navigation")
        .padding(if collapsed { 14 } else { 18 })
        .width(if collapsed { 72 } else { 224 })
        .height(Length::Fill)
        .style(|appearance| theme::panel(appearance, theme::PANEL, None, 0.))
        .into()
}
