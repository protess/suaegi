use iced::widget::{button, column, container, row, Space};
use iced::{Alignment, Element, Length};

use crate::i18n::text;
use crate::state::{AppState, Message};
use crate::theme;

pub fn view(state: &AppState) -> Option<Element<'_, Message>> {
    if !state.onboarding_open() {
        return None;
    }

    let cli_installed = state.cli_installed();
    let steps = column![
        text(if cli_installed {
            "SETUP                                      4 / 6"
        } else {
            "SETUP                                      3 / 6"
        })
        .size(11)
        .color(theme::MUTED),
        step("✓", "Turn on notifications", true),
        step("✓", "Choose your default agent", true),
        step(
            if cli_installed { "✓" } else { "3" },
            "Enable Suaegi CLI",
            cli_installed
        ),
        step("✓", "Connect integrations", true),
        step("5", "Automate workspace setup", false),
        step("6", "Start work in multiple repos", false),
        Space::new().height(Length::Fixed(8.0)),
        text("MILESTONES                              1 / 2")
            .size(11)
            .color(theme::MUTED),
        step("✓", "Multi-task", true),
        step("8", "Use Suaegi's browser", false),
    ]
    .spacing(5);

    let cards = row![
        capability(
            "Agent Orchestration",
            "Let agents coordinate through Suaegi to keep large, multi-step tasks moving to completion.",
        ),
        capability(
            "Agent Browser Use",
            "Give agents direct access to the browser so they can test pages and capture screenshots.",
        ),
        capability(
            "Computer Use",
            "Let agents control the desktop, moving the cursor, clicking, and typing in any app.",
        ),
    ]
    .spacing(8);

    let details = column![
        row![
            text("Enable Suaegi CLI").size(20).width(Length::Fill),
            container(text(if cli_installed { "Installed" } else { "Not done yet" }).size(11))
                .padding([3, 7])
                .style(theme::chip),
        ]
        .align_y(Alignment::Center),
        text("Register the Suaegi shell command and install agent skills for browser, computer, and orchestration workflows.")
            .size(13)
            .color(theme::MUTED),
        cards,
        container(
            row![
                text("▣  Full Disk Access").size(13),
                container(text("RECOMMENDED").size(10))
                    .padding([2, 5])
                    .style(theme::chip),
                Space::new().width(Length::Fill),
                button(text("Open Full Disk Access").size(12))
                    .on_press(Message::OnboardingOpenFullDiskAccess)
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        )
        .padding(9)
        .style(theme::session_card),
        button(text(if cli_installed {
            "✓  CLI installed"
        } else {
            ">_  Install CLI"
        }).size(12))
            .on_press(Message::OnboardingInstallCli)
            .padding([6, 10])
            .style(theme::selected_button),
    ]
    .spacing(12);

    let modal = container(
        column![
            row![
                column![
                    text("Getting started").size(17),
                    text("Finish the core workflows that make Suaegi useful for parallel agent work.")
                        .size(12)
                        .color(theme::MUTED),
                ]
                .spacing(3)
                .width(Length::Fill),
                button("×")
                    .on_press(Message::OnboardingClosed)
                    .padding([3, 7])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Start),
            row![
                container(steps)
                    .width(Length::Fixed(210.0))
                    .height(Length::Fill)
                    .padding([12, 0]),
                container(details)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding([12, 16])
                    .style(theme::app_canvas),
            ]
            .height(Length::Fill),
        ]
        .spacing(8),
    )
    .width(Length::Fixed(800.0))
    .height(Length::Fixed(580.0))
    .padding([12, 18])
    .style(theme::modal);

    Some(modal.into())
}

fn step(marker: &'static str, title: &'static str, done: bool) -> Element<'static, Message> {
    container(
        row![
            text(marker).size(12).color(if done {
                iced::Color::from_rgb8(0x22, 0xa0, 0x59)
            } else {
                theme::MUTED
            }),
            text(title).size(13).width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(theme::session_card)
    .into()
}

fn capability(title: &'static str, description: &'static str) -> Element<'static, Message> {
    container(
        column![
            row![
                text("◉").size(13),
                Space::new().width(Length::Fill),
                text("✓").size(12),
            ],
            text(title).size(13),
            text(description).size(11).color(theme::MUTED),
            text("Click Install CLI & Skills")
                .size(11)
                .color(theme::MUTED),
        ]
        .spacing(6),
    )
    .width(Length::FillPortion(1))
    .height(Length::Fixed(130.0))
    .padding(10)
    .style(theme::session_card)
    .into()
}
