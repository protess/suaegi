use iced::widget::{button, column, container, row, text, Space, Svg};
use iced::{Alignment, Color, Element, Length};

use crate::icons::{self, Icon};
use crate::state::{
    AppState, Message, MobileConnectionMode, MobileIosChannel, MobilePlatform, MobileStage,
};
use crate::theme;

const PHONE_TEXT: Color = Color::from_rgb8(0xf5, 0xf5, 0xf5);
const PHONE_MUTED: Color = Color::from_rgb8(0x8d, 0x8d, 0x93);
const GREEN: Color = Color::from_rgb8(0x24, 0xc8, 0x63);
const BLUE: Color = Color::from_rgb8(0x4f, 0x7f, 0xff);
const ERROR: Color = Color::from_rgb8(0xc4, 0x35, 0x31);

pub fn view(state: &AppState) -> Element<'_, Message> {
    let header = row![
        button(
            text(if state.ui_settings().show_mobile_sidebar {
                "Hide from sidebar"
            } else {
                "Show in sidebar"
            })
            .size(11)
        )
        .on_press(Message::UiSettingToggled(
            crate::state::UiSetting::ShowMobileSidebar
        ))
        .padding([5, 8])
        .style(theme::primary_dark_button),
        Space::new().width(Length::Fill),
        button(text("×").size(17))
            .on_press(Message::MobileClosed)
            .padding([2, 7])
            .style(theme::ghost_button),
    ]
    .align_y(Alignment::Center);

    let copy = match state.mobile_stage() {
        MobileStage::Intro => intro(),
        MobileStage::Install => install_step(state),
        MobileStage::Pair => pair_step(state),
    };

    let hero = row![
        container(copy).padding(iced::Padding {
            top: 0.0,
            right: 0.0,
            bottom: 50.0,
            left: 0.0,
        }),
        phone_preview(state)
    ]
    .spacing(114)
    .align_y(Alignment::Center);

    column![
        container(header)
            .padding([6, 10])
            .height(Length::Fixed(42.0)),
        container(hero)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 10.0,
                left: 0.0,
            }),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn intro<'a>() -> Element<'a, Message> {
    column![
        text("SUAEGI MOBILE").size(12).color(theme::MUTED),
        text("Your workspaces,\nin your pocket.")
            .size(43)
            .line_height(iced::widget::text::LineHeight::Relative(1.05)),
        text(
            "Control Suaegi from your phone. Check on agents, review changes,\n\
             and kick off tasks while you're away from your desk."
        )
        .size(14)
        .line_height(iced::widget::text::LineHeight::Relative(1.55))
        .color(theme::MUTED),
        row![
            text("Available on").size(11).color(theme::MUTED),
            container(text("●  iOS").size(11))
                .padding([3, 8])
                .style(theme::chip),
            container(text("▲  Android").size(11))
                .padding([3, 8])
                .style(theme::chip),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
        button(
            row![
                text("Get started").size(12),
                icons::view(Icon::ArrowRight, 11.0, Color::WHITE)
            ]
            .spacing(7)
            .align_y(Alignment::Center)
        )
        .on_press(Message::MobileGetStarted)
        .padding([7, 17])
        .style(theme::primary_dark_button),
    ]
    .spacing(15)
    .width(Length::Fixed(380.0))
    .into()
}

fn install_step(state: &AppState) -> Element<'_, Message> {
    let platform = state.mobile_platform();
    let channel = state.mobile_ios_channel();
    let open_label = match platform {
        MobilePlatform::Ios if channel == MobileIosChannel::Preview => "Open TestFlight",
        MobilePlatform::Ios => "Open App Store",
        MobilePlatform::Android => "Download APK",
    };
    let channel_controls: Element<'_, Message> = if platform == MobilePlatform::Ios {
        row![
            selection_button(
                "Preview",
                channel == MobileIosChannel::Preview,
                Message::MobileIosChannelSelected(MobileIosChannel::Preview)
            ),
            selection_button(
                "Stable",
                channel == MobileIosChannel::Stable,
                Message::MobileIosChannelSelected(MobileIosChannel::Stable)
            ),
            text(if channel == MobileIosChannel::Preview {
                "Newest features, updated daily."
            } else {
                "The public release, updated weekly."
            })
            .size(11)
            .color(theme::MUTED),
        ]
        .spacing(5)
        .align_y(Alignment::Center)
        .into()
    } else {
        Space::new().height(Length::Fixed(24.0)).into()
    };

    container(
        column![
            row![
                container(text("1").size(12))
                    .padding([3, 7])
                    .style(theme::chip),
                text("STEP 1 OF 2").size(12).color(theme::MUTED),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
            text("Get the app.").size(34),
            text("Scan the QR with your phone or open the install link to grab Orca Mobile.")
                .size(13)
                .line_height(iced::widget::text::LineHeight::Relative(1.45))
                .color(theme::MUTED),
            row![
                selection_button(
                    "●  iOS",
                    platform == MobilePlatform::Ios,
                    Message::MobilePlatformSelected(MobilePlatform::Ios)
                ),
                selection_button(
                    "▲  Android",
                    platform == MobilePlatform::Android,
                    Message::MobilePlatformSelected(MobilePlatform::Android)
                ),
            ]
            .spacing(4),
            channel_controls,
            row![
                column![
                    button(text(open_label).size(12))
                        .on_press(Message::MobileOpenInstallLink)
                        .padding([7, 13])
                        .style(theme::ghost_button),
                    button(text("⧉  Copy install link").size(11))
                        .on_press(Message::MobileCopyInstallLink)
                        .padding([5, 8])
                        .style(theme::ghost_button),
                ]
                .spacing(4),
                install_qr(state.mobile_install_url()),
            ]
            .spacing(36)
            .align_y(Alignment::Center),
            row![
                button(row![
                    icons::view(Icon::ArrowLeft, 10.0, theme::MUTED),
                    text("Back").size(12)
                ])
                .on_press(Message::MobileBack)
                .padding([6, 8])
                .style(theme::ghost_button),
                Space::new().width(Length::Fill),
                button(row![
                    text("Continue").size(12),
                    icons::view(Icon::ArrowRight, 10.0, Color::WHITE)
                ])
                .on_press(Message::MobileContinue)
                .padding([7, 14])
                .style(theme::primary_dark_button),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(13)
        .width(Length::Fixed(430.0)),
    )
    .padding(17)
    .style(theme::modal)
    .into()
}

fn pair_step(state: &AppState) -> Element<'_, Message> {
    let mode = state.mobile_connection_mode();
    let mode_description = match mode {
        MobileConnectionMode::Anywhere => {
            "Anywhere pairing securely relays encrypted traffic when your phone is away."
        }
        MobileConnectionMode::LocalNetwork => {
            "Local network pairing keeps traffic between devices on the same network."
        }
    };
    let pairing_status: Element<'_, Message> = match state.mobile_pairing_error() {
        Some(error) => text(error).size(11).color(ERROR).into(),
        None => text("No pairing code has been generated.")
            .size(11)
            .color(theme::MUTED)
            .into(),
    };
    container(
        column![
            row![
                container(text("2").size(12))
                    .padding([3, 7])
                    .style(theme::chip),
                text("STEP 2 OF 2").size(12).color(theme::MUTED),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
            text("Pair this Mac.").size(34),
            text("Open Orca Mobile, tap Pair Desktop, and scan the code.")
                .size(13)
                .color(theme::MUTED),
            row![
                selection_button(
                    "Anywhere",
                    mode == MobileConnectionMode::Anywhere,
                    Message::MobileConnectionModeSelected(MobileConnectionMode::Anywhere)
                ),
                selection_button(
                    "Local network",
                    mode == MobileConnectionMode::LocalNetwork,
                    Message::MobileConnectionModeSelected(MobileConnectionMode::LocalNetwork)
                ),
            ]
            .spacing(4),
            container(
                column![
                    text("Mobile relay is in beta").size(12),
                    text(mode_description).size(11).color(theme::MUTED),
                ]
                .spacing(4),
            )
            .padding(9)
            .width(Length::Fill)
            .style(theme::session_card),
            row![
                qr_placeholder("PAIR"),
                column![
                    text("Generating a pairing code requires the Suaegi mobile relay.")
                        .size(12)
                        .color(theme::MUTED),
                    button(text("Generate code").size(12))
                        .on_press(Message::MobileGeneratePairingRequested)
                        .padding([7, 12])
                        .style(theme::ghost_button),
                    pairing_status,
                    text("Network  Automatic").size(11).color(theme::MUTED),
                ]
                .spacing(8),
            ]
            .spacing(20)
            .align_y(Alignment::Center),
            button(row![
                icons::view(Icon::ArrowLeft, 10.0, theme::MUTED),
                text("Back").size(12)
            ])
            .on_press(Message::MobileBack)
            .padding([6, 8])
            .style(theme::ghost_button),
        ]
        .spacing(13)
        .width(Length::Fixed(430.0)),
    )
    .padding(17)
    .style(theme::modal)
    .into()
}

fn selection_button<'a>(
    label: &'static str,
    selected: bool,
    message: Message,
) -> iced::widget::Button<'a, Message> {
    button(text(label).size(12))
        .on_press(message)
        .padding([5, 10])
        .style(if selected {
            theme::selected_button
        } else {
            theme::ghost_button
        })
}

fn qr_placeholder<'a>(label: &'static str) -> Element<'a, Message> {
    container(
        column![
            text("▦ ▦ ▦ ▦ ▦").size(22),
            text("▦ ▪ ▦ ▪ ▦").size(22),
            text("▦ ▦ ▪ ▦ ▦").size(22),
            text("▦ ▪ ▦ ▪ ▦").size(22),
            text(label).size(10).color(theme::MUTED),
        ]
        .spacing(0)
        .align_x(Alignment::Center),
    )
    .width(Length::Fixed(118.0))
    .height(Length::Fixed(118.0))
    .align_x(iced::alignment::Horizontal::Center)
    .align_y(iced::alignment::Vertical::Center)
    .style(theme::session_card)
    .into()
}

fn install_qr(url: &str) -> Element<'static, Message> {
    let Ok(code) = qrcode::QrCode::new(url.as_bytes()) else {
        return qr_placeholder("INSTALL");
    };
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(112, 112)
        .quiet_zone(true)
        .build();
    container(
        Svg::new(iced::widget::svg::Handle::from_memory(svg.into_bytes()))
            .width(Length::Fixed(112.0))
            .height(Length::Fixed(112.0)),
    )
    .width(Length::Fixed(118.0))
    .height(Length::Fixed(118.0))
    .align_x(iced::alignment::Horizontal::Center)
    .align_y(iced::alignment::Vertical::Center)
    .style(theme::session_card)
    .into()
}

fn phone_preview(state: &AppState) -> Element<'_, Message> {
    if state.mobile_stage() != MobileStage::Intro {
        return phone_worktree_preview(state);
    }

    let (repo, branch) = state
        .selected_worktree()
        .map(|worktree| {
            let repo = state
                .repo_name_for_worktree(worktree)
                .unwrap_or("suaegi")
                .to_string();
            let branch = state
                .branch_context_for_worktree(worktree)
                .map(|(branch, _)| branch)
                .unwrap_or_else(|| "main".to_string());
            (repo, branch)
        })
        .unwrap_or_else(|| ("suaegi".to_string(), "main".to_string()));

    let top = row![
        row![
            text("⌁").size(17).color(PHONE_TEXT),
            text("Suaegi").size(14).color(PHONE_TEXT),
        ]
        .spacing(5)
        .align_y(Alignment::Center)
        .width(Length::Fill),
        icons::view(Icon::Settings, 13.0, PHONE_MUTED),
    ]
    .align_y(Alignment::Center);

    let stats = row![
        stat("1,284", "Agents spawned"),
        stat("142h", "Agent time"),
        stat("96", "PRs created"),
    ]
    .spacing(5);

    let desktops = column![
        section_label("DESKTOPS"),
        phone_card(
            row![
                phone_icon("▣"),
                column![
                    text("MacBook Pro").size(12).color(PHONE_TEXT),
                    row![
                        text("●").size(10).color(GREEN),
                        text("Connected · 4 worktrees · 1 active")
                            .size(9)
                            .color(PHONE_MUTED)
                    ]
                    .spacing(4)
                ]
                .spacing(2)
                .width(Length::Fill),
                text("›").size(15).color(PHONE_MUTED),
            ]
            .spacing(7)
            .align_y(Alignment::Center)
        ),
        phone_card(
            row![
                phone_icon("▣"),
                column![
                    text("M1 Mini · home").size(12).color(PHONE_MUTED),
                    row![
                        text("●").size(10).color(PHONE_MUTED),
                        text("Disconnected").size(9).color(PHONE_MUTED)
                    ]
                    .spacing(4)
                ]
                .spacing(2)
                .width(Length::Fill),
                text("›").size(15).color(PHONE_MUTED),
            ]
            .spacing(7)
            .align_y(Alignment::Center)
        ),
    ]
    .spacing(8);

    let resume = column![
        section_label("RESUME"),
        phone_card(
            row![
                phone_icon(">_"),
                column![
                    text(branch.clone()).size(12).color(PHONE_TEXT),
                    row![
                        text("●").size(10).color(BLUE),
                        text(format!("{repo} · {branch}"))
                            .size(9)
                            .color(PHONE_MUTED)
                    ]
                    .spacing(4)
                ]
                .spacing(2)
                .width(Length::Fill),
                text("›").size(15).color(PHONE_MUTED),
            ]
            .spacing(7)
            .align_y(Alignment::Center)
        ),
    ]
    .spacing(8);

    let tasks = column![
        section_label("TASKS"),
        phone_card(
            row![
                phone_icon("☷"),
                column![
                    text("Tasks").size(12).color(PHONE_TEXT),
                    text("GitHub · Linear").size(9).color(PHONE_MUTED)
                ]
                .spacing(2)
                .width(Length::Fill),
                text("◉  ◈").size(12).color(PHONE_MUTED),
                text("›").size(15).color(PHONE_MUTED),
            ]
            .spacing(7)
            .align_y(Alignment::Center)
        ),
    ]
    .spacing(8);

    let quick_actions = column![
        section_label("QUICK ACTIONS"),
        row![
            phone_card(
                row![text("▦").size(12), text("Pair Desktop").size(10)]
                    .spacing(6)
                    .align_y(Alignment::Center)
            ),
            phone_card(
                row![text("+").size(13), text("New Workspace").size(10)]
                    .spacing(6)
                    .align_y(Alignment::Center)
            ),
        ]
        .spacing(6),
    ]
    .spacing(8);

    let usage = column![
        section_label("ACCOUNT USAGE"),
        phone_card(
            column![
                usage_row("✺", "claude@stably.ai", "5h  ━━━━━      7d  ━━━"),
                usage_row("◉", "codex@stably.ai", "5h  ━━━━━━━    7d  ━━━━"),
            ]
            .spacing(12)
        ),
    ]
    .spacing(8);

    container(
        column![
            top,
            text("Welcome back").size(17).color(PHONE_TEXT),
            stats,
            desktops,
            resume,
            tasks,
            quick_actions,
            usage,
        ]
        .spacing(15),
    )
    .width(Length::Fixed(314.0))
    .height(Length::Fixed(675.0))
    .padding([24, 17])
    .style(theme::mobile_phone)
    .into()
}

fn phone_worktree_preview(state: &AppState) -> Element<'_, Message> {
    let mut worktrees = column![].spacing(0);
    let mut count = 0usize;
    for repo in state.repos() {
        for entry in state.worktrees_for(&repo.id) {
            let name = entry
                .branch
                .as_deref()
                .or_else(|| entry.path.file_name().and_then(|value| value.to_str()))
                .unwrap_or("workspace");
            let selected =
                state.selected_worktree() == Some(&crate::state::worktree_id_for(&entry.path));
            worktrees = worktrees.push(
                column![
                    row![
                        text(if selected { "●" } else { "○" })
                            .size(10)
                            .color(if selected { GREEN } else { PHONE_MUTED }),
                        text(name).size(12).color(PHONE_TEXT).width(Length::Fill),
                        text(if selected { "1" } else { "" })
                            .size(10)
                            .color(PHONE_MUTED),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    text(format!("{} · {}", repo.display_name, name))
                        .size(9)
                        .color(PHONE_MUTED),
                ]
                .spacing(3)
                .padding([10, 3]),
            );
            count += 1;
        }
    }
    if count == 0 {
        worktrees = worktrees.push(text("No worktrees").size(12).color(PHONE_MUTED));
    }

    container(
        column![
            row![
                text("‹").size(22).color(PHONE_TEXT),
                text("●").size(10).color(GREEN),
                text("MacBook Pro")
                    .size(14)
                    .color(PHONE_TEXT)
                    .width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                container(text("☷  Filter").size(10).color(PHONE_MUTED))
                    .padding([6, 8])
                    .style(theme::mobile_card),
                text("≡  Recent").size(10).color(PHONE_MUTED),
                text("◇  Repo").size(10).color(PHONE_MUTED),
                Space::new().width(Length::Fill),
                text("+").size(15).color(PHONE_MUTED),
                text("⌕").size(14).color(PHONE_MUTED),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text("⚑  PINNED").size(9).color(PHONE_MUTED),
            phone_card(
                row![
                    text("◔").size(12).color(PHONE_MUTED),
                    text("agent-test")
                        .size(12)
                        .color(PHONE_TEXT)
                        .width(Length::Fill),
                    text("2").size(10).color(PHONE_MUTED),
                ]
                .spacing(7)
                .align_y(Alignment::Center)
            ),
            text(format!("⌄  ACTIVE  {count}"))
                .size(9)
                .color(PHONE_MUTED),
            worktrees,
        ]
        .spacing(12),
    )
    .width(Length::Fixed(314.0))
    .height(Length::Fixed(675.0))
    .padding([24, 17])
    .style(theme::mobile_phone)
    .into()
}

fn stat(value: &'static str, label: &'static str) -> Element<'static, Message> {
    container(
        column![
            text(value).size(14).color(PHONE_TEXT),
            text(label).size(8).color(PHONE_MUTED),
        ]
        .spacing(2),
    )
    .width(Length::Fill)
    .padding([11, 7])
    .style(theme::mobile_card)
    .into()
}

fn phone_icon(label: &'static str) -> Element<'static, Message> {
    container(text(label).size(11).color(PHONE_MUTED))
        .center_x(Length::Fixed(29.0))
        .center_y(Length::Fixed(29.0))
        .style(theme::mobile_card)
        .into()
}

fn section_label(label: &'static str) -> Element<'static, Message> {
    text(label).size(8).color(PHONE_MUTED).into()
}

fn phone_card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .padding([10, 8])
        .style(theme::mobile_card)
        .into()
}

fn usage_row<'a>(icon: &'a str, account: &'a str, bars: &'a str) -> Element<'a, Message> {
    row![
        text(icon).size(13).color(PHONE_TEXT),
        column![
            text(account).size(11).color(PHONE_TEXT),
            text(bars).size(9).color(PHONE_MUTED)
        ]
        .spacing(2),
    ]
    .spacing(7)
    .align_y(Alignment::Center)
    .into()
}
