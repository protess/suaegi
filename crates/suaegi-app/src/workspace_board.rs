use iced::widget::{button, column, container, row, scrollable, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::i18n::text;
use crate::state::{worktree_id_for, AppState, BoardStatus, Message};
use crate::theme;

pub fn view(state: &AppState) -> Element<'_, Message> {
    let mut todo_cards = column![].spacing(5);
    let mut progress_cards = column![].spacing(5);
    let mut review_cards = column![].spacing(5);
    let mut done_cards = column![].spacing(5);
    let mut counts = [0usize; 4];
    let mut board_entries = Vec::new();
    for repo in state.repos() {
        for entry in state.worktrees_for(&repo.id) {
            board_entries.push((repo, entry));
        }
    }
    board_entries.sort_by_key(|(_, entry)| entry.is_main);
    let pinned_count = board_entries
        .iter()
        .filter(|(_, entry)| state.worktree_is_pinned(&worktree_id_for(&entry.path)))
        .count();
    for (repo, entry) in board_entries {
        let id = worktree_id_for(&entry.path);
        let name = entry.branch.clone().unwrap_or_else(|| {
            entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
                .to_string()
        });
        let selected = state.selected_worktree() == Some(&id);
        let mut card_content = column![
            row![
                text("●").size(10).color(theme::MUTED),
                text(name.clone()).size(12).width(Length::Fill),
                if selected {
                    container(text("primary").size(9))
                        .padding([1, 4])
                        .style(theme::chip)
                } else {
                    container(text("")).padding(0)
                }
            ]
            .align_y(Alignment::Center),
            row![
                text("▪").size(10).color(theme::MUTED),
                text(&repo.display_name).size(10).color(theme::MUTED),
                text(name).size(10).color(theme::MUTED),
            ]
            .spacing(5),
        ]
        .spacing(3);
        if let Some(comment) = state.worktree_comment(&id) {
            card_content =
                card_content.push(crate::i18n::text(comment).size(10).color(theme::MUTED));
        }
        let card = button(card_content)
            .on_press(Message::WorktreeSelected(id.clone()))
            .width(Length::Fill)
            .padding([6, 7])
            .style(if selected {
                theme::selected_button
            } else {
                theme::ghost_button
            });
        match state.board_status(&id) {
            BoardStatus::Todo => {
                counts[0] += 1;
                todo_cards = todo_cards.push(card);
            }
            BoardStatus::InProgress => {
                counts[1] += 1;
                progress_cards = progress_cards.push(card);
            }
            BoardStatus::InReview => {
                counts[2] += 1;
                review_cards = review_cards.push(card);
            }
            BoardStatus::Done => {
                counts[3] += 1;
                done_cards = done_cards.push(card);
            }
        }
    }

    let header = row![
        text("Workspace board").size(13).width(Length::Fill),
        button(text("×").size(15))
            .on_press(Message::WorkspaceBoardToggled)
            .padding([2, 6])
            .style(theme::ghost_button),
    ]
    .align_y(Alignment::Center);

    let pinned = container(
        row![
            text("⚑").size(11).color(theme::MUTED),
            text("Pinned").size(11).color(theme::MUTED),
            text(pinned_count).size(10).color(theme::MUTED),
            text("Drop here to pin without changing status.")
                .size(10)
                .color(theme::MUTED),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .height(Length::Fixed(27.0))
    .padding([5, 9])
    .width(Length::Fill)
    .style(theme::card);

    let columns = row![
        board_column(
            "Todo",
            counts[0],
            (counts[0] > 0).then_some(todo_cards),
            theme::board_todo
        ),
        board_column(
            "In progress",
            counts[1],
            (counts[1] > 0).then_some(progress_cards),
            theme::board_progress
        ),
        board_column(
            "In review",
            counts[2],
            (counts[2] > 0).then_some(review_cards),
            theme::board_review
        ),
        board_column(
            "Done",
            counts[3],
            (counts[3] > 0).then_some(done_cards),
            theme::board_done
        ),
    ]
    .spacing(8)
    .height(Length::Fill);

    let board = column![
        Space::new().height(Length::Fixed(21.0)),
        container(header)
            .height(Length::Fixed(35.0))
            .padding([6, 3]),
        pinned,
        columns,
        Space::new().height(Length::Fixed(6.0)),
    ]
    .spacing(6)
    .width(Length::Fixed(946.0))
    .height(Length::Fill);

    container(board)
        .padding([0, 9])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::app_canvas)
        .into()
}

fn board_column<'a>(
    title: &'static str,
    count: usize,
    cards: Option<iced::widget::Column<'a, Message>>,
    style: fn(&Theme) -> iced::widget::container::Style,
) -> Element<'a, Message> {
    let contents: Element<'a, Message> = if let Some(cards) = cards {
        scrollable(cards.padding([4, 6]))
            .height(Length::Fill)
            .into()
    } else {
        container(text("Empty").size(11).color(theme::MUTED))
            .center_x(Length::Fill)
            .height(Length::Fill)
            .padding([24, 0])
            .into()
    };

    container(
        column![
            row![
                text("○").size(12).color(theme::MUTED),
                text(title).size(12).width(Length::Fill),
                text(count).size(10).color(theme::MUTED),
                text("+").size(14).color(theme::MUTED),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
            contents,
        ]
        .spacing(3),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding([6, 7])
    .style(style)
    .into()
}
