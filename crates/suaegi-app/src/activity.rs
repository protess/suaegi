use iced::widget::{button, column, container, row, scrollable, Space};
use iced::{Alignment, Element, Length};

use crate::i18n::text;
use crate::state::{AppState, Message};
use crate::theme;

pub fn view(state: &AppState) -> Element<'_, Message> {
    let header = row![
        column![
            text("Activity").size(18),
            text("Agent progress, completions, and blocking events")
                .size(11)
                .color(theme::MUTED),
        ]
        .spacing(2),
        Space::new().width(Length::Fill),
        button(text("Clear").size(11))
            .on_press_maybe(
                (!state.activity_events().is_empty()).then_some(Message::ActivityCleared)
            )
            .padding([5, 8])
            .style(theme::ghost_button),
        button(text("Close").size(11))
            .on_press(Message::ActivityClosed)
            .padding([5, 8])
            .style(theme::ghost_button),
    ]
    .spacing(7)
    .align_y(Alignment::Center);

    let mut events = column![].spacing(6);
    if state.activity_events().is_empty() {
        events = events.push(
            container(
                column![
                    text("No agent activity yet").size(14),
                    text("New work, permission requests, and completions will appear here.")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(5)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .padding(36),
        );
    } else {
        for event in state.activity_events() {
            events = events.push(
                container(
                    row![
                        column![
                            text(&event.status).size(12),
                            text(&event.detail).size(11).color(theme::MUTED),
                        ]
                        .spacing(3)
                        .width(Length::Fill),
                        text(&event.worktree).size(11).color(theme::MUTED),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding([10, 12])
                .width(Length::Fill)
                .style(theme::context_panel),
            );
        }
    }

    container(
        column![header, scrollable(events).height(Length::Fill)]
            .spacing(12)
            .padding([14, 18]),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::editor_surface)
    .into()
}
