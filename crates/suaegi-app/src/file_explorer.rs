//! Plan 9 M7 — selected-worktree file explorer.
//!
//! The filesystem and git-status backends were already ported in M1–M6. This
//! module is the thin application layer that makes them usable: directories are
//! loaded one level at a time, ignored entries stay visible with a marker like
//! Orca, and git status is rendered without turning a transient git failure
//! into an empty result.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use iced::widget::{button, column, container, row, scrollable, text_input, Space};
use iced::{Alignment, Color, Element, Length, Task};
use suaegi_core::domain::WorktreeId;
use suaegi_git::fs::{list_dir, DirEntry};
use suaegi_git::runner::GitRunner;
use suaegi_git::status::{check_ignored, working_tree_status, FileStatus, HARDCODED_HIDES};

use crate::i18n::text;
use crate::state::{AppState, Message, OpId};
use crate::{icons, theme};

pub const WIDTH: f32 = 260.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub ignored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryState {
    Loading,
    Ready(Vec<ExplorerEntry>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerRow {
    pub name: String,
    pub path: String,
    pub depth: usize,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub expanded: bool,
    pub loading: bool,
    pub status: Option<&'static str>,
    pub ignored: bool,
}

#[derive(Debug, Default)]
pub struct FileExplorerState {
    worktree: Option<WorktreeId>,
    directories: HashMap<String, DirectoryState>,
    expanded: HashSet<String>,
    latest_directory_ops: HashMap<String, OpId>,
    latest_status_op: Option<OpId>,
    statuses: HashMap<String, FileStatus>,
    status_error: Option<String>,
    filter_query: String,
}

impl FileExplorerState {
    pub fn is_open(&self) -> bool {
        self.worktree.is_some()
    }

    pub fn worktree(&self) -> Option<&WorktreeId> {
        self.worktree.as_ref()
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    pub fn set_filter_query(&mut self, query: String) {
        self.filter_query = query;
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        self.expanded.insert(String::new());
    }

    pub fn expanded_directories(&self) -> Vec<String> {
        let mut directories = self.expanded.iter().cloned().collect::<Vec<_>>();
        directories.sort();
        directories
    }

    pub fn begin_directory_refresh(&mut self, path: &str, op: OpId) {
        self.latest_directory_ops.insert(path.to_string(), op);
    }

    pub fn begin_status_refresh(&mut self, op: OpId) {
        self.latest_status_op = Some(op);
    }

    pub fn begin(&mut self, worktree: WorktreeId, directory_op: OpId, status_op: OpId) {
        *self = Self::default();
        self.worktree = Some(worktree);
        self.expanded.insert(String::new());
        self.directories
            .insert(String::new(), DirectoryState::Loading);
        self.latest_directory_ops
            .insert(String::new(), directory_op);
        self.latest_status_op = Some(status_op);
    }

    pub fn begin_refresh(&mut self, directory_op: OpId, status_op: OpId) {
        let Some(worktree) = self.worktree.clone() else {
            return;
        };
        self.begin(worktree, directory_op, status_op);
    }

    /// Returns true when expanding this directory needs an async load.
    pub fn toggle_directory(&mut self, path: &str, op: OpId) -> bool {
        if self.expanded.remove(path) {
            return false;
        }
        self.expanded.insert(path.to_string());
        if matches!(self.directories.get(path), Some(DirectoryState::Ready(_))) {
            return false;
        }
        self.directories
            .insert(path.to_string(), DirectoryState::Loading);
        self.latest_directory_ops.insert(path.to_string(), op);
        true
    }

    pub fn accept_directory(
        &mut self,
        worktree: &WorktreeId,
        path: &str,
        op: OpId,
        result: Result<Vec<ExplorerEntry>, String>,
    ) -> bool {
        if self.worktree.as_ref() != Some(worktree)
            || self.latest_directory_ops.get(path) != Some(&op)
        {
            return false;
        }
        let state = match result {
            Ok(entries) => DirectoryState::Ready(entries),
            Err(error) => DirectoryState::Failed(error),
        };
        self.directories.insert(path.to_string(), state);
        true
    }

    pub fn accept_status(
        &mut self,
        worktree: &WorktreeId,
        op: OpId,
        result: Result<HashMap<String, FileStatus>, String>,
    ) -> bool {
        if self.worktree.as_ref() != Some(worktree) || self.latest_status_op != Some(op) {
            return false;
        }
        match result {
            Ok(statuses) => {
                self.statuses = statuses;
                self.status_error = None;
            }
            Err(error) => {
                // Keep the previous authoritative snapshot. A failed refresh is not
                // evidence that the worktree suddenly became clean.
                self.status_error = Some(error);
            }
        }
        true
    }

    pub fn rows(&self) -> Vec<ExplorerRow> {
        let mut rows = Vec::new();
        self.append_rows("", 0, &mut rows);
        rows
    }

    pub fn status_error(&self) -> Option<&str> {
        self.status_error.as_deref()
    }

    fn append_rows(&self, directory: &str, depth: usize, out: &mut Vec<ExplorerRow>) {
        let Some(DirectoryState::Ready(entries)) = self.directories.get(directory) else {
            return;
        };
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            let loading = entry.is_dir
                && matches!(
                    self.directories.get(&entry.path),
                    Some(DirectoryState::Loading)
                );
            out.push(ExplorerRow {
                name: entry.name.clone(),
                path: entry.path.clone(),
                depth,
                is_dir: entry.is_dir,
                is_symlink: entry.is_symlink,
                expanded,
                loading,
                ignored: entry.ignored,
                status: if entry.ignored {
                    Some("⊘")
                } else {
                    status_label(&entry.path, entry.is_dir, &self.statuses)
                },
            });
            if expanded {
                self.append_rows(&entry.path, depth + 1, out);
            }
        }
    }

    fn root_state(&self) -> Option<&DirectoryState> {
        self.directories.get("")
    }
}

fn join_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn is_hidden_name(name: &str) -> bool {
    HARDCODED_HIDES.contains(&name)
}

fn entries_with_paths(parent: &str, entries: Vec<DirEntry>) -> Vec<ExplorerEntry> {
    entries
        .into_iter()
        .filter(|entry| !is_hidden_name(&entry.name))
        .map(|entry| ExplorerEntry {
            path: join_rel(parent, &entry.name),
            name: entry.name,
            is_dir: entry.is_dir,
            is_symlink: entry.is_symlink,
            ignored: false,
        })
        .collect()
}

pub async fn load_directory_now(
    worktree: PathBuf,
    directory: String,
) -> Result<Vec<ExplorerEntry>, String> {
    let worktree_for_list = worktree.clone();
    let directory_for_list = directory.clone();
    let entries = tokio::task::spawn_blocking(move || {
        list_dir(&worktree_for_list, &directory_for_list).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("file listing worker failed: {error}"))??;

    let mut entries = entries_with_paths(&directory, entries);
    let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    let ignored = check_ignored(&GitRunner::new(), &worktree, &paths)
        .await
        .map_err(|error| error.to_string())?;

    for entry in &mut entries {
        entry.ignored = ignored.contains(&entry.path);
    }
    Ok(entries)
}

pub async fn load_status_now(worktree: PathBuf) -> Result<HashMap<String, FileStatus>, String> {
    working_tree_status(&GitRunner::new(), &worktree)
        .await
        .map_err(|error| error.to_string())
}

pub fn load_directory(
    worktree_id: WorktreeId,
    worktree_path: PathBuf,
    directory: String,
    op: OpId,
) -> Task<Message> {
    let directory_for_message = directory.clone();
    Task::perform(
        load_directory_now(worktree_path, directory),
        move |result| Message::FileExplorerDirectoryLoaded {
            worktree: worktree_id,
            path: directory_for_message,
            op,
            result,
        },
    )
}

pub fn load_status(worktree_id: WorktreeId, worktree_path: PathBuf, op: OpId) -> Task<Message> {
    Task::perform(load_status_now(worktree_path), move |result| {
        Message::FileExplorerStatusLoaded {
            worktree: worktree_id,
            op,
            result,
        }
    })
}

fn status_rank(status: &FileStatus) -> u8 {
    match status {
        FileStatus::Conflicted(_) => 6,
        FileStatus::Deleted => 5,
        FileStatus::Renamed { .. } | FileStatus::Copied { .. } => 4,
        FileStatus::Added => 3,
        FileStatus::Modified => 2,
        FileStatus::Untracked => 1,
        FileStatus::Other(_) => 0,
    }
}

fn status_text(status: &FileStatus) -> &'static str {
    match status {
        FileStatus::Conflicted(_) => "!",
        FileStatus::Deleted => "D",
        FileStatus::Renamed { .. } => "R",
        FileStatus::Copied { .. } => "C",
        FileStatus::Added => "A",
        FileStatus::Modified => "M",
        FileStatus::Untracked => "?",
        FileStatus::Other(_) => "•",
    }
}

fn status_label(
    path: &str,
    is_dir: bool,
    statuses: &HashMap<String, FileStatus>,
) -> Option<&'static str> {
    if let Some(status) = statuses.get(path) {
        return Some(status_text(status));
    }
    if !is_dir {
        return None;
    }
    let prefix = format!("{path}/");
    statuses
        .iter()
        .filter(|(candidate, _)| candidate.starts_with(&prefix))
        .max_by_key(|(_, status)| status_rank(status))
        .map(|(_, status)| status_text(status))
}

pub fn view(state: &AppState) -> Option<Element<'_, Message>> {
    let explorer = state.file_explorer();
    let worktree = explorer.worktree()?.clone();
    let title = state
        .repo_name_for_worktree(&worktree)
        .map(str::to_string)
        .unwrap_or_else(|| {
            Path::new(&worktree.0)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Files")
                .to_string()
        });

    let refresh_worktree = worktree.clone();
    let header = row![
        text(title).size(15).width(Length::Fill),
        button(icons::view(icons::Icon::ChevronUp, 12.0, theme::MUTED))
            .on_press(Message::FileExplorerCollapseAll)
            .padding([2, 5])
            .style(crate::theme::ghost_button),
        button(icons::view(icons::Icon::Refresh, 12.0, theme::MUTED))
            .on_press(Message::FileExplorerRefreshRequested {
                worktree: refresh_worktree,
            })
            .padding([2, 5])
            .style(crate::theme::ghost_button),
        button(icons::view(icons::Icon::Ellipsis, 12.0, theme::MUTED))
            .padding([2, 5])
            .style(crate::theme::ghost_button),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let query = explorer.filter_query();
    let search = text_input("Find files", query)
        .on_input(Message::FileExplorerFilterChanged)
        .size(14)
        .padding([5, 7])
        .width(Length::Fill);
    let view_switch = row![
        button(text("Names").size(13))
            .padding([3, 9])
            .style(crate::theme::selected_button),
        button(text("Contents").size(13))
            .on_press(Message::ContentSearchRequested {
                worktree: worktree.clone(),
            })
            .padding([3, 9])
            .style(crate::theme::ghost_button),
        Space::new().width(Length::Fill),
    ]
    .spacing(2);

    let mut body = column![header, search, view_switch]
        .spacing(3)
        .padding([4, 8]);
    match explorer.root_state() {
        Some(DirectoryState::Loading) => {
            body = body.push(text("Loading files…").size(14));
        }
        Some(DirectoryState::Failed(error)) => {
            body = body.push(
                text(format!("Could not load files: {error}"))
                    .size(13)
                    .color(Color::from_rgb(0.75, 0.22, 0.17)),
            );
        }
        _ => {
            let mut rows = column![].spacing(1);
            let normalized_query = query.trim().to_lowercase();
            for item in explorer.rows().into_iter().filter(|item| {
                (state.ui_settings().show_git_ignored_files || !item.ignored)
                    && (normalized_query.is_empty()
                        || item.name.to_lowercase().contains(&normalized_query))
            }) {
                let indent = "  ".repeat(item.depth);
                let marker = if item.is_symlink {
                    "↗"
                } else if item.is_dir && item.loading {
                    "…"
                } else if item.is_dir && item.expanded {
                    "▾"
                } else if item.is_dir {
                    "▸"
                } else {
                    " "
                };
                let status = item.status.unwrap_or(" ");
                let label = format!("{indent}{marker} {}  {status}", item.name);
                if item.is_dir {
                    rows = rows.push(
                        button(text(label).size(14))
                            .on_press(Message::FileExplorerDirectoryToggled { path: item.path })
                            .width(Length::Fill)
                            .style(crate::theme::ghost_button),
                    );
                } else {
                    let file_worktree = worktree.clone();
                    rows = rows.push(
                        button(text(label).size(14))
                            .on_press(Message::EditorFileRequested {
                                worktree: file_worktree,
                                path: item.path,
                            })
                            .width(Length::Fill)
                            .style(crate::theme::ghost_button),
                    );
                }
            }
            body = body.push(scrollable(rows).height(Length::Fill));
        }
    }
    if let Some(error) = explorer.status_error() {
        body = body.push(
            text(format!("Status unavailable: {error}"))
                .size(12)
                .color(Color::from_rgb(0.85, 0.55, 0.0)),
        );
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

    fn entry(name: &str, path: &str, is_dir: bool) -> ExplorerEntry {
        ExplorerEntry {
            name: name.to_string(),
            path: path.to_string(),
            is_dir,
            is_symlink: false,
            ignored: false,
        }
    }

    #[test]
    fn stale_directory_results_cannot_replace_the_current_worktree() {
        let a = WorktreeId("/tmp/a".into());
        let b = WorktreeId("/tmp/b".into());
        let mut state = FileExplorerState::default();
        state.begin(a.clone(), OpId(1), OpId(2));
        state.begin(b.clone(), OpId(3), OpId(4));

        assert!(!state.accept_directory(&a, "", OpId(1), Ok(vec![entry("old", "old", false)])));
        assert!(state.rows().is_empty());

        assert!(state.accept_directory(&b, "", OpId(3), Ok(vec![entry("new", "new", false)])));
        assert_eq!(state.rows()[0].name, "new");
    }

    #[test]
    fn collapsing_keeps_the_cached_children_and_reopening_needs_no_load() {
        let worktree = WorktreeId("/tmp/w".into());
        let mut state = FileExplorerState::default();
        state.begin(worktree.clone(), OpId(1), OpId(2));
        state.accept_directory(&worktree, "", OpId(1), Ok(vec![entry("src", "src", true)]));
        assert!(state.toggle_directory("src", OpId(3)));
        state.accept_directory(
            &worktree,
            "src",
            OpId(3),
            Ok(vec![entry("lib.rs", "src/lib.rs", false)]),
        );
        assert_eq!(state.rows().len(), 2);

        assert!(!state.toggle_directory("src", OpId(4)));
        assert_eq!(state.rows().len(), 1);
        assert!(!state.toggle_directory("src", OpId(5)));
        assert_eq!(state.rows().len(), 2);
    }

    #[test]
    fn watcher_refreshes_every_expanded_directory_without_collapsing_the_tree() {
        let worktree = WorktreeId("/tmp/w".into());
        let mut state = FileExplorerState::default();
        state.begin(worktree.clone(), OpId(1), OpId(2));
        state.accept_directory(&worktree, "", OpId(1), Ok(vec![entry("src", "src", true)]));
        assert!(state.toggle_directory("src", OpId(3)));
        state.accept_directory(
            &worktree,
            "src",
            OpId(3),
            Ok(vec![entry("old.rs", "src/old.rs", false)]),
        );

        assert_eq!(state.expanded_directories(), vec!["", "src"]);
        state.begin_directory_refresh("", OpId(4));
        state.begin_directory_refresh("src", OpId(5));
        assert!(state.accept_directory(
            &worktree,
            "",
            OpId(4),
            Ok(vec![entry("src", "src", true)])
        ));
        assert!(state.accept_directory(
            &worktree,
            "src",
            OpId(5),
            Ok(vec![entry("new.rs", "src/new.rs", false)])
        ));

        assert_eq!(state.expanded_directories(), vec!["", "src"]);
        assert_eq!(
            state
                .rows()
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src", "src/new.rs"]
        );
    }

    #[test]
    fn a_failed_status_refresh_keeps_the_last_authoritative_snapshot() {
        let worktree = WorktreeId("/tmp/w".into());
        let mut state = FileExplorerState::default();
        state.begin(worktree.clone(), OpId(1), OpId(2));
        state.accept_directory(&worktree, "", OpId(1), Ok(vec![entry("src", "src", true)]));
        state.accept_status(
            &worktree,
            OpId(2),
            Ok(HashMap::from([(
                "src/lib.rs".to_string(),
                FileStatus::Modified,
            )])),
        );
        assert_eq!(state.rows()[0].status, Some("M"));

        state.latest_status_op = Some(OpId(3));
        assert!(state.accept_status(&worktree, OpId(3), Err("git timed out".into())));
        assert_eq!(state.rows()[0].status, Some("M"));
        assert_eq!(state.status_error(), Some("git timed out"));
    }

    #[test]
    fn hardcoded_names_are_hidden_before_git_ignore_filtering() {
        let entries = entries_with_paths(
            "",
            vec![
                DirEntry {
                    name: ".git".into(),
                    is_dir: true,
                    is_symlink: false,
                },
                DirEntry {
                    name: "node_modules".into(),
                    is_dir: true,
                    is_symlink: false,
                },
                DirEntry {
                    name: "src".into(),
                    is_dir: true,
                    is_symlink: false,
                },
            ],
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src"]
        );
    }

    #[tokio::test]
    async fn real_directory_load_marks_git_ignored_and_hides_only_hardcoded_entries() {
        let worktree = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(worktree.path())
            .status()
            .unwrap();
        std::fs::write(worktree.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(worktree.path().join("ignored.txt"), "no").unwrap();
        std::fs::write(worktree.path().join("visible.txt"), "yes").unwrap();
        std::fs::create_dir(worktree.path().join("node_modules")).unwrap();
        std::fs::create_dir(worktree.path().join("src")).unwrap();

        let entries = load_directory_now(worktree.path().to_path_buf(), String::new())
            .await
            .unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();

        assert!(names.contains(&"src"));
        assert!(names.contains(&"visible.txt"));
        assert!(names.contains(&"ignored.txt"));
        assert!(!names.contains(&"node_modules"));
        assert!(!names.contains(&".git"));
        assert!(entries
            .iter()
            .find(|entry| entry.name == "ignored.txt")
            .is_some_and(|entry| entry.ignored));
    }
}
