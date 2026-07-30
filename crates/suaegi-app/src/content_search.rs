//! Thin content-search panel over `suaegi-search`'s streamed rg/git-grep backend.

use std::path::PathBuf;

use iced::widget::{button, checkbox, column, container, row, scrollable, text_input};
use iced::{Alignment, Color, Element, Length, Task};
use suaegi_core::domain::WorktreeId;
use suaegi_search::{SearchOptions, SearchResult};

use crate::i18n::text;
use crate::state::{AppState, Message, OpId};

const WIDTH: f32 = 260.0;
const DISPLAY_LIMIT: usize = 200;

#[derive(Debug)]
enum SearchPhase {
    Idle,
    Searching { op: OpId },
    Results(SearchResult),
    Failed(String),
}

#[derive(Debug)]
pub struct ContentSearchState {
    worktree: Option<WorktreeId>,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
    phase: SearchPhase,
}

impl Default for ContentSearchState {
    fn default() -> Self {
        Self {
            worktree: None,
            query: String::new(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            phase: SearchPhase::Idle,
        }
    }
}

impl ContentSearchState {
    pub fn is_open(&self) -> bool {
        self.worktree.is_some()
    }

    pub fn worktree(&self) -> Option<&WorktreeId> {
        self.worktree.as_ref()
    }

    pub fn open(&mut self, worktree: WorktreeId) {
        self.worktree = Some(worktree);
        self.query.clear();
        self.phase = SearchPhase::Idle;
    }

    pub fn close(&mut self) {
        self.worktree = None;
        self.phase = SearchPhase::Idle;
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.phase = SearchPhase::Idle;
    }

    pub fn set_case_sensitive(&mut self, value: bool) {
        self.case_sensitive = value;
        self.phase = SearchPhase::Idle;
    }

    pub fn set_whole_word(&mut self, value: bool) {
        self.whole_word = value;
        self.phase = SearchPhase::Idle;
    }

    pub fn set_use_regex(&mut self, value: bool) {
        self.use_regex = value;
        self.phase = SearchPhase::Idle;
    }

    pub fn begin_search(&mut self, root_path: PathBuf, op: OpId) -> Option<SearchRequest> {
        let worktree = self.worktree.clone()?;
        if self.query.is_empty() {
            return None;
        }
        self.phase = SearchPhase::Searching { op };
        Some(SearchRequest {
            worktree,
            op,
            options: SearchOptions {
                query: self.query.clone(),
                root_path: root_path.to_string_lossy().into_owned(),
                case_sensitive: Some(self.case_sensitive),
                whole_word: Some(self.whole_word),
                use_regex: Some(self.use_regex),
                include_pattern: None,
                exclude_pattern: None,
                max_results: None,
            },
        })
    }

    pub fn accept(
        &mut self,
        worktree: &WorktreeId,
        op: OpId,
        result: Result<SearchResult, String>,
    ) -> bool {
        if self.worktree.as_ref() != Some(worktree)
            || !matches!(self.phase, SearchPhase::Searching { op: expected } if expected == op)
        {
            return false;
        }
        self.phase = match result {
            Ok(result) => SearchPhase::Results(result),
            Err(error) => SearchPhase::Failed(error),
        };
        true
    }

    #[cfg(test)]
    fn result(&self) -> Option<&SearchResult> {
        match &self.phase {
            SearchPhase::Results(result) => Some(result),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub worktree: WorktreeId,
    pub op: OpId,
    pub options: SearchOptions,
}

pub fn run(request: SearchRequest) -> Task<Message> {
    Task::future(async move {
        let result = suaegi_search::run_search(&request.options)
            .await
            .map_err(|error| error.to_string());
        Message::ContentSearchFinished {
            worktree: request.worktree,
            op: request.op,
            result,
        }
    })
}

pub fn input_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from("suaegi-content-search-input")
}

pub fn view(state: &AppState) -> Option<Element<'_, Message>> {
    let search = state.content_search();
    search.worktree.as_ref()?;

    let query = text_input("Search file contents", &search.query)
        .id(input_id())
        .on_input(Message::ContentSearchQueryChanged)
        .on_submit(Message::ContentSearchSubmitted)
        .width(Length::Fill);
    let toggles = row![
        checkbox(search.case_sensitive)
            .label("Case")
            .on_toggle(Message::ContentSearchCaseToggled),
        checkbox(search.whole_word)
            .label("Word")
            .on_toggle(Message::ContentSearchWordToggled),
        checkbox(search.use_regex)
            .label("Regex")
            .on_toggle(Message::ContentSearchRegexToggled),
    ]
    .spacing(12);
    let header = row![
        text("Search").size(17).width(Length::Fill),
        button("×")
            .on_press(Message::ContentSearchClosed)
            .style(crate::theme::ghost_button),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut body = column![header, query, toggles].spacing(8).padding(10);
    match &search.phase {
        SearchPhase::Idle => {}
        SearchPhase::Searching { .. } => {
            body = body.push(text("Searching…").size(14));
        }
        SearchPhase::Failed(error) => {
            body = body.push(
                text(error)
                    .size(13)
                    .color(Color::from_rgb(0.75, 0.22, 0.17)),
            );
        }
        SearchPhase::Results(result) => {
            let suffix = if result.truncated { " (truncated)" } else { "" };
            body = body.push(
                text(format!(
                    "{} match(es) in {} file(s){suffix}",
                    result.total_matches,
                    result.files.len()
                ))
                .size(12),
            );
            let mut rows = column![].spacing(2);
            let mut displayed = 0usize;
            'files: for file in &result.files {
                for matched in &file.matches {
                    if displayed == DISPLAY_LIMIT {
                        break 'files;
                    }
                    displayed += 1;
                    let path = file.relative_path.clone();
                    let label = format!(
                        "{}:{}:{}  {}",
                        file.relative_path,
                        matched.line,
                        matched.column,
                        matched.line_content.trim()
                    );
                    rows = rows.push(
                        button(text(label).size(13))
                            .on_press(Message::ContentSearchPathSelected(path))
                            .width(Length::Fill)
                            .style(crate::theme::ghost_button),
                    );
                }
            }
            if result.total_matches > displayed {
                rows = rows.push(
                    text(format!(
                        "Showing the first {displayed} matches. Refine the query to see more."
                    ))
                    .size(12),
                );
            }
            body = body.push(scrollable(rows).height(Length::Fill));
        }
    }

    Some(
        container(body)
            .width(Length::Fixed(WIDTH))
            .height(Length::Fill)
            .style(crate::theme::context_panel)
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_result() -> SearchResult {
        SearchResult {
            files: Vec::new(),
            total_matches: 0,
            truncated: false,
        }
    }

    #[test]
    fn changing_query_invalidates_an_in_flight_result() {
        let worktree = WorktreeId("/tmp/w".into());
        let mut state = ContentSearchState::default();
        state.open(worktree.clone());
        state.set_query("old".into());
        state
            .begin_search(PathBuf::from("/tmp/w"), OpId(1))
            .unwrap();
        state.set_query("new".into());

        assert!(!state.accept(&worktree, OpId(1), Ok(empty_result())));
        assert!(state.result().is_none());
    }

    #[test]
    fn stale_worktree_result_is_rejected() {
        let a = WorktreeId("/tmp/a".into());
        let b = WorktreeId("/tmp/b".into());
        let mut state = ContentSearchState::default();
        state.open(a.clone());
        state.set_query("needle".into());
        state
            .begin_search(PathBuf::from("/tmp/a"), OpId(1))
            .unwrap();
        state.open(b.clone());

        assert!(!state.accept(&a, OpId(1), Ok(empty_result())));
        assert_eq!(state.worktree(), Some(&b));
    }
}
