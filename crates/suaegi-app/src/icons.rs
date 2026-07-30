//! Lucide 0.577 symbolic icons used by Orca's renderer.
//! The path data is distributed under the Lucide ISC license.

use iced::widget::{svg, Svg};
use iced::{Color, Length};

#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Files,
    Bot,
    GitBranch,
    ListChecks,
    PanelRight,
    PanelLeft,
    Search,
    ClipboardList,
    CalendarClock,
    Smartphone,
    Settings,
    CircleHelp,
    ArrowLeft,
    ArrowRight,
    Refresh,
    MemoryStick,
    Terminal,
    PanelsTopLeft,
    Globe,
    FileText,
    Maximize,
    Restore,
    Minimize,
    Plus,
    Plug,
    Link,
    ChevronUp,
    Ellipsis,
}

macro_rules! lucide {
    ($body:literal) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
            $body,
            "</svg>"
        )
        .as_bytes()
    };
}

fn data(icon: Icon) -> &'static [u8] {
    match icon {
        Icon::Files => lucide!(
            r#"<path d="M15 2h-4a2 2 0 0 0-2 2v11a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V8"/><path d="M16.706 2.706A2.4 2.4 0 0 0 15 2v5a1 1 0 0 0 1 1h5a2.4 2.4 0 0 0-.706-1.706z"/><path d="M5 7a2 2 0 0 0-2 2v11a2 2 0 0 0 2 2h8a2 2 0 0 0 1.732-1"/>"#
        ),
        Icon::Bot => lucide!(
            r#"<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>"#
        ),
        Icon::GitBranch => lucide!(
            r#"<path d="M15 6a9 9 0 0 0-9 9V3"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/>"#
        ),
        Icon::ListChecks => lucide!(
            r#"<path d="M13 5h8"/><path d="M13 12h8"/><path d="M13 19h8"/><path d="m3 17 2 2 4-4"/><path d="m3 7 2 2 4-4"/>"#
        ),
        Icon::PanelRight => {
            lucide!(r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M15 3v18"/>"#)
        }
        Icon::PanelLeft => {
            lucide!(r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>"#)
        }
        Icon::Search => {
            lucide!(r#"<path d="m21 21-4.34-4.34"/><circle cx="11" cy="11" r="8"/>"#)
        }
        Icon::ClipboardList => lucide!(
            r#"<rect width="8" height="4" x="8" y="2" rx="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><path d="M12 11h4"/><path d="M12 16h4"/><path d="M8 11h.01"/><path d="M8 16h.01"/>"#
        ),
        Icon::CalendarClock => lucide!(
            r#"<path d="M16 14v2.2l1.6 1"/><path d="M16 2v4"/><path d="M21 7.5V6a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h3.5"/><path d="M3 10h5"/><path d="M8 2v4"/><circle cx="16" cy="16" r="6"/>"#
        ),
        Icon::Smartphone => {
            lucide!(r#"<rect width="14" height="20" x="5" y="2" rx="2"/><path d="M12 18h.01"/>"#)
        }
        Icon::Settings => lucide!(
            r#"<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>"#
        ),
        Icon::CircleHelp => lucide!(
            r#"<circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>"#
        ),
        Icon::ArrowLeft => {
            lucide!(r#"<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>"#)
        }
        Icon::ArrowRight => {
            lucide!(r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#)
        }
        Icon::Refresh => lucide!(
            r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#
        ),
        Icon::MemoryStick => lucide!(
            r#"<path d="M6 19v-3"/><path d="M10 19v-3"/><path d="M14 19v-3"/><path d="M18 19v-3"/><path d="M8 11V9"/><path d="M16 11V9"/><path d="M12 11V9"/><path d="M2 15h20"/><path d="M2 7h20"/><rect x="4" y="3" width="16" height="16" rx="2"/>"#
        ),
        Icon::Terminal => lucide!(r#"<path d="m4 17 6-6-6-6"/><path d="M12 19h8"/>"#),
        Icon::PanelsTopLeft => lucide!(
            r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/><path d="M9 21V9"/>"#
        ),
        Icon::Globe => lucide!(
            r#"<circle cx="12" cy="12" r="10"/><path d="M2 12h20"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>"#
        ),
        Icon::FileText => lucide!(
            r#"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><path d="M16 13H8"/><path d="M16 17H8"/><path d="M10 9H8"/>"#
        ),
        Icon::Maximize => lucide!(
            r#"<path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M16 3h3a2 2 0 0 1 2 2v3"/><path d="M8 21H5a2 2 0 0 1-2-2v-3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/>"#
        ),
        Icon::Restore => lucide!(
            r#"<path d="m14 10 7-7"/><path d="M20 10h-6V4"/><path d="m3 21 7-7"/><path d="M4 14h6v6"/>"#
        ),
        Icon::Minimize => lucide!(r#"<path d="M5 12h14"/>"#),
        Icon::Plus => lucide!(r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#),
        Icon::Plug => lucide!(
            r#"<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a6 6 0 0 1-12 0V8Z"/>"#
        ),
        Icon::Link => lucide!(
            r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"#
        ),
        Icon::ChevronUp => lucide!(r#"<path d="m18 15-6-6-6 6"/>"#),
        Icon::Ellipsis => lucide!(
            r#"<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>"#
        ),
    }
}

pub fn view(icon: Icon, size: f32, color: Color) -> Svg<'static> {
    Svg::new(svg::Handle::from_memory(data(icon)))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_theme, _status| svg::Style { color: Some(color) })
}
