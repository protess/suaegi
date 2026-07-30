//! Visual tokens mirrored from Orca's renderer `assets/main.css`.
//!
//! The desktop shell intentionally uses the same quiet light surfaces as the
//! reference app. State is communicated with compact selection surfaces and
//! small status colors instead of large, saturated cards.

use iced::widget::{button, container};
use iced::{Background, Border, Color, Shadow, Theme, Vector};
use std::sync::OnceLock;
use suaegi_core::domain::UiSettings;

pub const BACKGROUND: Color = Color::from_rgb8(0xff, 0xff, 0xff);
pub const SIDEBAR: Color = Color::from_rgb8(0xf5, 0xf5, 0xf5);
pub const SIDEBAR_ACCENT: Color = Color::from_rgb8(0xea, 0xea, 0xea);
pub const MUTED_SURFACE: Color = Color::from_rgb8(0xfa, 0xfa, 0xfa);
pub const BORDER: Color = Color::from_rgb8(0xe5, 0xe5, 0xe5);
pub const TEXT: Color = Color::from_rgb8(0x0a, 0x0a, 0x0a);
pub const MUTED: Color = Color::from_rgb8(0x73, 0x73, 0x73);

const DARK_BACKGROUND: Color = Color::from_rgb8(0x0a, 0x0a, 0x0a);
const DARK_SIDEBAR: Color = Color::from_rgb8(0x17, 0x17, 0x17);
const DARK_ACCENT: Color = Color::from_rgb8(0x35, 0x35, 0x35);
const DARK_MUTED_SURFACE: Color = Color::from_rgb8(0x1e, 0x1e, 0x1e);
const DARK_BORDER: Color = Color::from_rgb8(0x2d, 0x2d, 0x2d);
const DARK_TEXT: Color = Color::from_rgb8(0xfa, 0xfa, 0xfa);

pub fn app_theme(mode: &str) -> Theme {
    let dark = mode_is_dark(mode);
    if dark {
        return Theme::custom(
            "Orca Dark",
            iced::theme::Palette {
                background: DARK_BACKGROUND,
                text: DARK_TEXT,
                primary: Color::from_rgb8(0xe5, 0xe5, 0xe5),
                success: Color::from_rgb8(0x38, 0xb7, 0x6a),
                warning: Color::from_rgb8(0xe4, 0xa1, 0x24),
                danger: Color::from_rgb8(0xe0, 0x5b, 0x50),
            },
        );
    }
    Theme::custom(
        "Orca Light",
        iced::theme::Palette {
            background: BACKGROUND,
            text: TEXT,
            primary: Color::from_rgb8(0x17, 0x17, 0x17),
            success: Color::from_rgb8(0x22, 0xa0, 0x59),
            warning: Color::from_rgb8(0xd8, 0x8c, 0x00),
            danger: Color::from_rgb8(0xc0, 0x39, 0x2b),
        },
    )
}

pub fn mode_is_dark(mode: &str) -> bool {
    mode == "dark" || (mode == "system" && system_prefers_dark())
}

fn system_prefers_dark() -> bool {
    static DARK: OnceLock<bool> = OnceLock::new();
    *DARK.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("defaults")
                .args(["read", "-g", "AppleInterfaceStyle"])
                .output()
                .is_ok_and(|output| {
                    output.status.success()
                        && String::from_utf8_lossy(&output.stdout)
                            .trim()
                            .eq_ignore_ascii_case("dark")
                })
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    })
}

fn is_dark(theme: &Theme) -> bool {
    let background = theme.palette().background;
    background.r + background.g + background.b < 1.5
}

fn surface_tokens(theme: &Theme) -> (Color, Color, Color, Color, Color, Color) {
    if is_dark(theme) {
        (
            DARK_BACKGROUND,
            DARK_SIDEBAR,
            DARK_ACCENT,
            DARK_MUTED_SURFACE,
            DARK_BORDER,
            DARK_TEXT,
        )
    } else {
        (
            BACKGROUND,
            SIDEBAR,
            SIDEBAR_ACCENT,
            MUTED_SURFACE,
            BORDER,
            TEXT,
        )
    }
}

pub fn app_canvas(theme: &Theme) -> container::Style {
    let (background, _, _, _, _, text) = surface_tokens(theme);
    container::Style::default()
        .background(background)
        .color(text)
}

pub fn app_canvas_translucent(theme: &Theme) -> container::Style {
    let (mut background, _, _, _, _, text) = surface_tokens(theme);
    background.a = 0.76;
    container::Style::default()
        .background(background)
        .color(text)
}

pub fn sidebar(theme: &Theme) -> container::Style {
    let (_, sidebar, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(sidebar)
        .color(text)
        .border(Border {
            color: border,
            width: 0.0,
            radius: 0.0.into(),
        })
}

pub fn sidebar_solid(theme: &Theme) -> container::Style {
    let (background, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(background)
        .color(text)
        .border(Border {
            color: border,
            width: 0.0,
            radius: 0.0.into(),
        })
}

pub fn sidebar_tinted(theme: &Theme) -> container::Style {
    let dark = is_dark(theme);
    let (_, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(if dark {
            Color::from_rgb8(0x16, 0x1a, 0x20)
        } else {
            Color::from_rgb8(0xf2, 0xf5, 0xf8)
        })
        .color(text)
        .border(Border {
            color: border,
            width: 0.0,
            radius: 0.0.into(),
        })
}

fn mix_color(foreground: Color, background: Color, foreground_weight: f32) -> Color {
    let weight = foreground_weight.clamp(0.0, 1.0);
    Color {
        r: foreground.r * weight + background.r * (1.0 - weight),
        g: foreground.g * weight + background.g * (1.0 - weight),
        b: foreground.b * weight + background.b * (1.0 - weight),
        a: foreground.a * weight + background.a * (1.0 - weight),
    }
}

pub(crate) fn parse_hex(value: &str) -> Option<Color> {
    let normalized = crate::terminal::palette::normalize_hex_color(value)?;
    let value = normalized.trim_start_matches('#');
    Some(Color::from_rgb8(
        u8::from_str_radix(&value[0..2], 16).ok()?,
        u8::from_str_radix(&value[2..4], 16).ok()?,
        u8::from_str_radix(&value[4..6], 16).ok()?,
    ))
}

fn configured_sidebar_surface(settings: &UiSettings, theme: &Theme) -> (Color, Color, Color) {
    let (background, sidebar, _, _, _, text) = surface_tokens(theme);
    match settings.left_sidebar_appearance.as_str() {
        "match-terminal" => {
            let terminal_theme = if !is_dark(theme) && settings.terminal_use_separate_light_theme {
                &settings.terminal_theme_light
            } else {
                &settings.terminal_theme_dark
            };
            let palette = crate::terminal::palette::shared_for(terminal_theme);
            let mut terminal_background = palette.background();
            terminal_background.a =
                f32::from(settings.terminal_background_opacity_percent.min(100)) / 100.0;
            let foreground = palette.foreground();
            (
                terminal_background,
                foreground,
                mix_color(foreground, terminal_background, 0.07),
            )
        }
        "tinted" => {
            let tint = parse_hex(&settings.left_sidebar_tint_color)
                .unwrap_or_else(|| Color::from_rgb8(0x18, 0x18, 0x1b));
            let strength = f32::from(settings.left_sidebar_tint_opacity_percent.min(35)) / 100.0;
            let tinted = mix_color(tint, background, strength);
            (tinted, text, mix_color(text, tinted, 0.07))
        }
        _ => (sidebar, text, mix_color(text, sidebar, 0.07)),
    }
}

pub fn configured_sidebar(settings: &UiSettings) -> impl Fn(&Theme) -> container::Style + use<> {
    let settings = settings.clone();
    move |theme| {
        let (background, text, _) = configured_sidebar_surface(&settings, theme);
        container::Style::default()
            .background(background)
            .color(text)
    }
}

pub fn configured_sidebar_top_bar(
    settings: &UiSettings,
) -> impl Fn(&Theme) -> container::Style + use<> {
    let settings = settings.clone();
    move |theme| {
        let (background, text, border) = configured_sidebar_surface(&settings, theme);
        container::Style::default()
            .background(background)
            .color(text)
            .border(Border {
                color: border,
                width: 1.0,
                radius: 0.0.into(),
            })
    }
}

pub fn top_bar(theme: &Theme) -> container::Style {
    let (_, _, _, muted_surface, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(muted_surface)
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 0.0.into(),
        })
}

pub fn attention_top_bar(theme: &Theme) -> container::Style {
    let mut style = top_bar(theme);
    style.border.color = theme.palette().warning;
    style.border.width = 2.0;
    style
}

pub fn sidebar_top_bar(theme: &Theme) -> container::Style {
    let (_, sidebar, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(sidebar)
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 0.0.into(),
        })
}

pub fn editor_surface(theme: &Theme) -> container::Style {
    let (background, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(background)
        .color(text)
        .border(Border {
            color: border,
            width: 0.0,
            radius: 0.0.into(),
        })
}

pub fn context_panel(theme: &Theme) -> container::Style {
    let (_, _, _, muted_surface, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(muted_surface)
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 0.0.into(),
        })
}

pub fn floating_workspace_panel(theme: &Theme) -> container::Style {
    let (background, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(background)
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 9.0.into(),
        })
        .shadow(Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.2),
            offset: Vector::new(0.0, 10.0),
            blur_radius: 28.0,
        })
}

pub fn floating_workspace_trigger(theme: &Theme) -> container::Style {
    let (_, _, accent, _, _, text) = surface_tokens(theme);
    container::Style::default()
        .background(accent)
        .color(text)
        .border(Border {
            color: mix_color(text, accent, 0.12),
            width: 1.0,
            radius: 8.0.into(),
        })
        .shadow(Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.22),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 12.0,
        })
}

pub fn floating_workspace_attention(theme: &Theme) -> container::Style {
    let (_, _, accent, _, _, _) = surface_tokens(theme);
    container::Style::default()
        .background(Color::from_rgb8(0xf5, 0x9e, 0x0b))
        .border(Border {
            color: accent,
            width: 2.0,
            radius: 4.0.into(),
        })
}

pub fn card(theme: &Theme) -> container::Style {
    let (background, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(background)
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 8.0.into(),
        })
}

pub fn active_card(theme: &Theme) -> container::Style {
    let (_, _, accent, _, _, text) = surface_tokens(theme);
    container::Style::default()
        .background(accent)
        .color(text)
        .border(Border {
            color: Color::from_rgba8(0, 0, 0, 0.02),
            width: 1.0,
            radius: 8.0.into(),
        })
        .shadow(Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.04),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        })
}

pub fn chip(theme: &Theme) -> container::Style {
    let dark = is_dark(theme);
    container::Style::default()
        .background(if dark {
            Color::from_rgba8(255, 255, 255, 0.07)
        } else {
            Color::from_rgba8(0, 0, 0, 0.035)
        })
        .color(if dark {
            Color::from_rgb8(0xb8, 0xb8, 0xb8)
        } else {
            MUTED
        })
        .border(Border {
            color: if dark {
                Color::from_rgba8(255, 255, 255, 0.12)
            } else {
                Color::from_rgba8(0, 0, 0, 0.12)
            },
            width: 1.0,
            radius: 4.0.into(),
        })
}

pub fn session_card(theme: &Theme) -> container::Style {
    let (background, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(background)
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
        })
}

pub fn modal(theme: &Theme) -> container::Style {
    let (background, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(background)
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 10.0.into(),
        })
        .shadow(Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.24),
            offset: Vector::new(0.0, 20.0),
            blur_radius: 60.0,
        })
}

pub fn mobile_phone(_theme: &Theme) -> container::Style {
    container::Style::default()
        .background(Color::from_rgb8(0x0f, 0x0f, 0x10))
        .color(Color::WHITE)
        .border(Border {
            color: Color::from_rgb8(0x02, 0x02, 0x02),
            width: 3.0,
            radius: 36.0.into(),
        })
        .shadow(Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.26),
            offset: Vector::new(0.0, 18.0),
            blur_radius: 34.0,
        })
}

pub fn mobile_card(_theme: &Theme) -> container::Style {
    container::Style::default()
        .background(Color::from_rgb8(0x1a, 0x1a, 0x1c))
        .color(Color::WHITE)
        .border(Border {
            color: Color::from_rgb8(0x29, 0x29, 0x2c),
            width: 1.0,
            radius: 10.0.into(),
        })
}

pub fn board_todo(theme: &Theme) -> container::Style {
    board_column(theme, Color::from_rgb8(0xfb, 0xfb, 0xfb))
}

pub fn board_progress(theme: &Theme) -> container::Style {
    board_column(theme, Color::from_rgb8(0xfb, 0xfa, 0xf4))
}

pub fn board_review(theme: &Theme) -> container::Style {
    board_column(theme, Color::from_rgb8(0xf5, 0xfa, 0xf7))
}

pub fn board_done(theme: &Theme) -> container::Style {
    board_column(theme, Color::from_rgb8(0xfb, 0xf7, 0xf6))
}

fn board_column(theme: &Theme, light_background: Color) -> container::Style {
    let (background, _, _, _, border, text) = surface_tokens(theme);
    container::Style::default()
        .background(if is_dark(theme) {
            background
        } else {
            light_background
        })
        .color(text)
        .border(Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
        })
}

pub fn primary_dark_button(_theme: &Theme, status: button::Status) -> button::Style {
    let background = if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        Color::from_rgb8(0x2d, 0x2d, 0x2d)
    } else {
        Color::from_rgb8(0x18, 0x18, 0x18)
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: Color::WHITE,
        border: Border {
            radius: 99.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style::default().background(Color::from_rgba8(0, 0, 0, 0.36))
}

pub fn ghost_button(theme: &Theme, status: button::Status) -> button::Style {
    let (_, _, accent, _, _, text) = surface_tokens(theme);
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(Background::Color(accent)),
        button::Status::Active | button::Status::Disabled => None,
    };
    button::Style {
        background,
        text_color: if matches!(status, button::Status::Disabled) {
            MUTED
        } else {
            text
        },
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub fn selected_button(theme: &Theme, _status: button::Status) -> button::Style {
    let (_, _, accent, _, _, text) = surface_tokens(theme);
    button::Style {
        background: Some(Background::Color(accent)),
        text_color: text,
        border: Border {
            color: Color::from_rgba8(0, 0, 0, 0.025),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..button::Style::default()
    }
}

pub fn search_button(theme: &Theme, status: button::Status) -> button::Style {
    let dark = is_dark(theme);
    let background = if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        if dark {
            Color::from_rgba8(255, 255, 255, 0.12)
        } else {
            Color::from_rgba8(0, 0, 0, 0.08)
        }
    } else if dark {
        Color::from_rgba8(255, 255, 255, 0.07)
    } else {
        Color::from_rgba8(0, 0, 0, 0.045)
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: MUTED,
        border: Border {
            color: Color::from_rgba8(0, 0, 0, 0.10),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..button::Style::default()
    }
}

pub fn template_card(theme: &Theme, status: button::Status) -> button::Style {
    let (background_surface, _, accent, _, border, text) = surface_tokens(theme);
    let background = if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        accent
    } else {
        background_surface
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: text,
        border: Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..button::Style::default()
    }
}

pub fn danger_ghost_button(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = ghost_button(theme, status);
    style.text_color = if matches!(status, button::Status::Disabled) {
        MUTED
    } else {
        Color::from_rgb8(0xc0, 0x39, 0x2b)
    };
    style
}

pub fn traffic_button(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let fill = if matches!(status, button::Status::Pressed) {
            Color::from_rgb(color.r * 0.85, color.g * 0.85, color.b * 0.85)
        } else {
            color
        };
        button::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: Color::from_rgba8(0, 0, 0, 0.12),
                width: 0.5,
                radius: 99.0.into(),
            },
            ..button::Style::default()
        }
    }
}
