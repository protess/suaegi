//! Quick Open palette over the already-ported Orca lister and fuzzy ranker.

use std::path::PathBuf;

use crate::state::{AppState, Message, OpId};
use iced::event;
use iced::keyboard::key::Named;
use iced::keyboard::{self, Key};
use iced::widget::{button, column, container, row, scrollable, text_input, Space};
use iced::{Alignment, Color, Element, Length, Subscription, Task};
use suaegi_core::domain::WorktreeId;
use suaegi_fuzzy::{rank, RESULT_LIMIT};

use crate::i18n::text;

const WIDTH: f32 = 548.0;

#[derive(Debug)]
enum Palette {
    Closed,
    Workspaces {
        items: Vec<WorkspaceItem>,
        query: String,
        matches: Vec<WorkspaceItem>,
        selected: usize,
    },
    Loading {
        worktree: WorktreeId,
        op: OpId,
    },
    Ready {
        worktree: WorktreeId,
        files: Vec<String>,
        query: String,
        matches: Vec<String>,
        selected: usize,
    },
    Failed {
        worktree: WorktreeId,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct WorkspaceItem {
    pub id: WorktreeId,
    pub title: String,
    pub subtitle: String,
}

#[derive(Debug)]
pub struct QuickOpenState {
    palette: Palette,
}

impl Default for QuickOpenState {
    fn default() -> Self {
        Self {
            palette: Palette::Closed,
        }
    }
}

impl QuickOpenState {
    pub fn is_open(&self) -> bool {
        !matches!(self.palette, Palette::Closed)
    }

    pub fn worktree(&self) -> Option<&WorktreeId> {
        match &self.palette {
            Palette::Closed | Palette::Workspaces { .. } => None,
            Palette::Loading { worktree, .. }
            | Palette::Ready { worktree, .. }
            | Palette::Failed { worktree, .. } => Some(worktree),
        }
    }

    pub fn close(&mut self) {
        self.palette = Palette::Closed;
    }

    pub fn begin(&mut self, worktree: WorktreeId, op: OpId) {
        self.palette = Palette::Loading { worktree, op };
    }

    pub fn begin_workspaces(&mut self, items: Vec<WorkspaceItem>) {
        self.palette = Palette::Workspaces {
            matches: items.clone(),
            items,
            query: String::new(),
            selected: 0,
        };
    }

    pub fn accept(
        &mut self,
        worktree: &WorktreeId,
        op: OpId,
        result: Result<Vec<String>, String>,
    ) -> bool {
        let current = matches!(
            &self.palette,
            Palette::Loading {
                worktree: expected_worktree,
                op: expected_op,
            } if expected_worktree == worktree && *expected_op == op
        );
        if !current {
            return false;
        }

        self.palette = match result {
            Ok(files) => {
                let matches = ranked_paths("", &files);
                Palette::Ready {
                    worktree: worktree.clone(),
                    files,
                    query: String::new(),
                    matches,
                    selected: 0,
                }
            }
            Err(message) => Palette::Failed {
                worktree: worktree.clone(),
                message,
            },
        };
        true
    }

    pub fn set_query(&mut self, query: String) {
        if let Palette::Workspaces {
            items,
            query: current,
            matches,
            selected,
        } = &mut self.palette
        {
            let needle = query.trim().to_lowercase();
            *matches = items
                .iter()
                .filter(|item| {
                    needle.is_empty()
                        || item.title.to_lowercase().contains(&needle)
                        || item.subtitle.to_lowercase().contains(&needle)
                })
                .take(RESULT_LIMIT)
                .cloned()
                .collect();
            *current = query;
            *selected = 0;
            return;
        }
        let Palette::Ready {
            files,
            query: current,
            matches,
            selected,
            ..
        } = &mut self.palette
        else {
            return;
        };
        *matches = ranked_paths(&query, files);
        *current = query;
        *selected = 0;
    }

    pub fn move_selection(&mut self, offset: isize) {
        let (length, selected) = match &mut self.palette {
            Palette::Ready {
                matches, selected, ..
            } => (matches.len(), selected),
            Palette::Workspaces {
                matches, selected, ..
            } => (matches.len(), selected),
            _ => return,
        };
        if length == 0 {
            *selected = 0;
            return;
        }
        let last = length - 1;
        *selected = selected.saturating_add_signed(offset).min(last);
    }

    pub fn selected_file(&self) -> Option<(WorktreeId, String)> {
        let Palette::Ready {
            worktree,
            matches,
            selected,
            ..
        } = &self.palette
        else {
            return None;
        };
        matches
            .get(*selected)
            .map(|path| (worktree.clone(), path.clone()))
    }

    pub fn selected_workspace(&self) -> Option<WorktreeId> {
        let Palette::Workspaces {
            matches, selected, ..
        } = &self.palette
        else {
            return None;
        };
        matches.get(*selected).map(|item| item.id.clone())
    }

    #[cfg(test)]
    fn snapshot(&self) -> Option<(&str, Vec<&str>, usize)> {
        let Palette::Ready {
            query,
            matches,
            selected,
            ..
        } = &self.palette
        else {
            return None;
        };
        Some((
            query,
            matches.iter().map(String::as_str).collect(),
            *selected,
        ))
    }
}

fn ranked_paths(query: &str, files: &[String]) -> Vec<String> {
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    rank(query, &refs, RESULT_LIMIT)
        .into_iter()
        .map(|matched| matched.path)
        .collect()
}

pub fn input_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from("suaegi-quick-open-input")
}

pub fn list_files(
    worktree: WorktreeId,
    path: PathBuf,
    excludes: Vec<PathBuf>,
    op: OpId,
) -> Task<Message> {
    Task::future(async move {
        let exclude_refs: Vec<&std::path::Path> = excludes.iter().map(PathBuf::as_path).collect();
        let result = suaegi_git::quick_open::list_quick_open_files(&path, &exclude_refs)
            .await
            .map_err(|error| error.to_string());
        Message::QuickOpenLoaded {
            worktree,
            op,
            result,
        }
    })
}

fn event_message(
    event: iced::Event,
    _status: event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, repeat, .. }) = event else {
        return None;
    };
    if repeat {
        return None;
    }

    match key {
        Key::Named(Named::Escape) => Some(Message::QuickOpenClosed),
        Key::Named(Named::ArrowUp) => Some(Message::QuickOpenSelectionMoved(-1)),
        Key::Named(Named::ArrowDown) => Some(Message::QuickOpenSelectionMoved(1)),
        _ => None,
    }
}

pub fn subscription() -> Subscription<Message> {
    iced::event::listen_with(event_message)
}

pub fn view(state: &AppState) -> Option<Element<'_, Message>> {
    let palette = &state.quick_open().palette;
    let body: Element<'_, Message> = match palette {
        Palette::Closed => return None,
        Palette::Workspaces {
            query,
            matches,
            selected,
            ..
        } => {
            let mut results = column![].spacing(2);
            for (index, item) in matches.iter().enumerate() {
                let id = item.id.clone();
                results = results.push(
                    button(
                        column![
                            text(&item.title).size(14),
                            text(&item.subtitle).size(12).color(crate::theme::MUTED),
                        ]
                        .spacing(2),
                    )
                    .on_press(Message::WorkspaceSearchSelected(id))
                    .width(Length::Fill)
                    .padding([6, 8])
                    .style(if index == *selected {
                        crate::theme::selected_button
                    } else {
                        crate::theme::ghost_button
                    }),
                );
            }
            let mut open_tabs = column![].spacing(2);
            for (worktree, title) in state.open_tab_items() {
                open_tabs = open_tabs.push(
                    button(
                        column![
                            text(title).size(14),
                            text("Terminal tab").size(12).color(crate::theme::MUTED),
                        ]
                        .spacing(2),
                    )
                    .on_press(Message::WorkspaceSearchSelected(worktree))
                    .width(Length::Fill)
                    .padding([6, 8])
                    .style(crate::theme::ghost_button),
                );
            }
            column![
                palette_input("Search worktrees, settings, tabs, and actions…", query),
                text("RECENT WORKTREES").size(11).color(crate::theme::MUTED),
                results,
                text("OPEN TABS").size(11).color(crate::theme::MUTED),
                scrollable(open_tabs).height(Length::Fill),
                row![
                    text("↵  Open").size(12).color(crate::theme::MUTED),
                    text("esc  Close").size(12).color(crate::theme::MUTED),
                    Space::new().width(Length::Fill),
                    text("↑↓  Move").size(12).color(crate::theme::MUTED),
                ],
            ]
            .spacing(7)
            .padding(8)
            .into()
        }
        Palette::Loading { .. } => column![
            palette_input("Search files", ""),
            text("Indexing files…").size(14),
        ]
        .spacing(8)
        .padding(8)
        .into(),
        Palette::Failed { message, .. } => column![
            palette_input("Search files", ""),
            text(message)
                .size(13)
                .color(Color::from_rgb(0.75, 0.22, 0.17)),
        ]
        .spacing(8)
        .padding(8)
        .into(),
        Palette::Ready {
            query,
            matches,
            selected,
            ..
        } => {
            let mut results = column![].spacing(2);
            for (index, path) in matches.iter().enumerate() {
                let marker = if index == *selected { "› " } else { "  " };
                let path_for_open = path.clone();
                results = results.push(
                    button(text(format!("{marker}{path}")).size(14))
                        .on_press(Message::QuickOpenPathSelected(path_for_open))
                        .width(Length::Fill)
                        .style(if index == *selected {
                            crate::theme::selected_button
                        } else {
                            crate::theme::ghost_button
                        }),
                );
            }
            column![
                palette_input("Search files", query),
                text("RECENT FILES").size(11).color(crate::theme::MUTED),
                scrollable(results).height(Length::Fill),
                row![
                    text("↵  Open").size(12).color(crate::theme::MUTED),
                    text("esc  Close").size(12).color(crate::theme::MUTED),
                    Space::new().width(Length::Fill),
                    text("↑↓  Move").size(12).color(crate::theme::MUTED),
                ],
            ]
            .spacing(7)
            .padding(8)
            .into()
        }
    };

    Some(
        container(body)
            .width(Length::Fixed(WIDTH))
            .height(Length::Fixed(356.0))
            .style(crate::theme::modal)
            .into(),
    )
}

fn palette_input<'a>(placeholder: &'a str, value: &'a str) -> Element<'a, Message> {
    row![
        crate::icons::view(crate::icons::Icon::Search, 14.0, crate::theme::MUTED),
        text_input(placeholder, value)
            .id(input_id())
            .on_input(Message::QuickOpenQueryChanged)
            .on_submit(Message::QuickOpenSelected)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_listing_cannot_replace_a_newer_worktree() {
        let a = WorktreeId("/tmp/a".into());
        let b = WorktreeId("/tmp/b".into());
        let mut state = QuickOpenState::default();
        state.begin(a.clone(), OpId(1));
        state.begin(b.clone(), OpId(2));

        assert!(!state.accept(&a, OpId(1), Ok(vec!["old.rs".into()])));
        assert_eq!(state.worktree(), Some(&b));
    }

    #[test]
    fn query_reranks_and_resets_selection() {
        let worktree = WorktreeId("/tmp/w".into());
        let mut state = QuickOpenState::default();
        state.begin(worktree.clone(), OpId(1));
        assert!(state.accept(
            &worktree,
            OpId(1),
            Ok(vec![
                "src/main.rs".into(),
                "tests/main.rs".into(),
                "README.md".into()
            ])
        ));
        state.move_selection(1);
        state.set_query("read".into());

        let (query, matches, selected) = state.snapshot().unwrap();
        assert_eq!(query, "read");
        assert_eq!(matches, vec!["README.md"]);
        assert_eq!(selected, 0);
    }

    #[test]
    fn selection_is_bounded_and_returns_the_ranked_path() {
        let worktree = WorktreeId("/tmp/w".into());
        let mut state = QuickOpenState::default();
        state.begin(worktree.clone(), OpId(1));
        state.accept(&worktree, OpId(1), Ok(vec!["a.rs".into(), "b.rs".into()]));

        state.move_selection(-10);
        assert_eq!(state.snapshot().unwrap().2, 0);
        state.move_selection(10);
        assert_eq!(state.snapshot().unwrap().2, 1);
        assert_eq!(state.selected_file(), Some((worktree, "b.rs".to_string())));
    }
}
