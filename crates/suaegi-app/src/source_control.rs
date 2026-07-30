//! Working-tree source-control panel.
//!
//! The git primitives were ported earlier; this module is the deliberately thin
//! iced surface that makes them usable. Destructive discard requires an
//! explicit confirmation, and push never escalates to force automatically.

use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text_input};
use iced::{Alignment, Color, Element, Length, Task};
use suaegi_core::domain::WorktreeId;
use suaegi_git::remote::{PullOutcome, PushOutcome};
use suaegi_git::runner::GitRunner;
use suaegi_git::status::{working_tree_status_detailed, DetailedFileStatus, FileStatus};
use suaegi_git::write_ops::{CommitOutcome, DiscardOutcome};

use crate::forge_ui::{self, CreatePrAffordance};
use crate::i18n::text;
use crate::state::{AppState, Message, OpId};
use crate::{icons, theme};

const WIDTH: f32 = 260.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceControlOperation {
    Stage(String),
    Unstage(String),
    Discard(String),
    Commit(String),
    Fetch,
    Pull,
    Push { branch: String },
}

#[derive(Debug)]
enum PanelPhase {
    Closed,
    Loading {
        worktree: WorktreeId,
        op: OpId,
    },
    Ready {
        worktree: WorktreeId,
        entries: Vec<DetailedFileStatus>,
    },
    Failed {
        worktree: WorktreeId,
        message: String,
    },
}

#[derive(Debug)]
pub struct SourceControlState {
    phase: PanelPhase,
    commit_message: String,
    filter: String,
    operation: Option<(OpId, SourceControlOperation)>,
    generation: Option<OpId>,
    discard_confirmation: Option<String>,
    notice: Option<Result<String, String>>,
    launch_action: Option<(String, String)>,
}

impl Default for SourceControlState {
    fn default() -> Self {
        Self {
            phase: PanelPhase::Closed,
            commit_message: String::new(),
            filter: String::new(),
            operation: None,
            generation: None,
            discard_confirmation: None,
            notice: None,
            launch_action: None,
        }
    }
}

impl SourceControlState {
    pub fn is_open(&self) -> bool {
        !matches!(self.phase, PanelPhase::Closed)
    }

    pub fn worktree(&self) -> Option<&WorktreeId> {
        match &self.phase {
            PanelPhase::Closed => None,
            PanelPhase::Loading { worktree, .. }
            | PanelPhase::Ready { worktree, .. }
            | PanelPhase::Failed { worktree, .. } => Some(worktree),
        }
    }

    pub fn close(&mut self) {
        self.phase = PanelPhase::Closed;
        self.operation = None;
        self.generation = None;
        self.discard_confirmation = None;
        self.notice = None;
        self.launch_action = None;
    }

    pub fn begin_load(&mut self, worktree: WorktreeId, op: OpId) {
        self.phase = PanelPhase::Loading { worktree, op };
        self.discard_confirmation = None;
    }

    pub fn begin_refresh(&mut self, op: OpId) -> bool {
        let Some(worktree) = self.worktree().cloned() else {
            return false;
        };
        self.phase = PanelPhase::Loading { worktree, op };
        self.discard_confirmation = None;
        true
    }

    pub fn accept_status(
        &mut self,
        worktree: &WorktreeId,
        op: OpId,
        result: Result<Vec<DetailedFileStatus>, String>,
    ) -> bool {
        let current = matches!(
            &self.phase,
            PanelPhase::Loading {
                worktree: expected_worktree,
                op: expected_op,
            } if expected_worktree == worktree && *expected_op == op
        );
        if !current {
            return false;
        }
        self.phase = match result {
            Ok(entries) => PanelPhase::Ready {
                worktree: worktree.clone(),
                entries,
            },
            Err(message) => PanelPhase::Failed {
                worktree: worktree.clone(),
                message,
            },
        };
        true
    }

    pub fn set_commit_message(&mut self, message: String) {
        self.commit_message = message;
        self.notice = None;
        self.launch_action = None;
    }

    pub fn commit_message(&self) -> &str {
        &self.commit_message
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn request_discard(&mut self, path: String) {
        if self.operation.is_none() {
            self.discard_confirmation = Some(path);
        }
    }

    pub fn cancel_discard(&mut self) {
        self.discard_confirmation = None;
    }

    pub fn take_confirmed_discard(&mut self) -> Option<String> {
        self.discard_confirmation.take()
    }

    pub fn begin_operation(&mut self, op: OpId, operation: SourceControlOperation) -> bool {
        if self.busy() {
            return false;
        }
        self.notice = None;
        self.launch_action = None;
        self.operation = Some((op, operation));
        true
    }

    pub fn accept_operation(
        &mut self,
        worktree: &WorktreeId,
        op: OpId,
        operation: &SourceControlOperation,
        result: Result<String, String>,
    ) -> bool {
        if self.worktree() != Some(worktree)
            || !matches!(
                &self.operation,
                Some((expected_op, expected_operation))
                    if *expected_op == op && expected_operation == operation
            )
        {
            return false;
        }
        if result.is_ok() && matches!(operation, SourceControlOperation::Commit(_)) {
            self.commit_message.clear();
        }
        self.launch_action = result.as_ref().err().and_then(|error| {
            let action = match operation {
                SourceControlOperation::Commit(_) => "fixCommitFailure",
                SourceControlOperation::Push { .. } => "fixPushFailure",
                _ => return None,
            };
            Some((action.to_string(), error.clone()))
        });
        self.operation = None;
        self.notice = Some(result);
        true
    }

    fn busy(&self) -> bool {
        self.operation.is_some() || self.generation.is_some()
    }

    pub fn begin_generation(&mut self, op: OpId) -> bool {
        if self.busy() {
            return false;
        }
        self.notice = None;
        self.launch_action = None;
        self.generation = Some(op);
        true
    }

    pub fn finish_generation(&mut self, op: OpId, result: Result<String, String>) -> bool {
        if self.generation != Some(op) {
            return false;
        }
        self.generation = None;
        match result {
            Ok(message) => {
                self.commit_message = message;
                self.notice = Some(Ok("Commit message generated".to_string()));
            }
            Err(error) => self.notice = Some(Err(error)),
        }
        true
    }

    pub fn generating(&self) -> bool {
        self.generation.is_some()
    }

    pub fn launch_action(&self) -> Option<(&str, &str)> {
        self.launch_action
            .as_ref()
            .map(|(action, detail)| (action.as_str(), detail.as_str()))
    }

    pub fn has_conflicts(&self) -> bool {
        matches!(
            &self.phase,
            PanelPhase::Ready { entries, .. }
                if entries
                    .iter()
                    .any(|entry| matches!(entry.status, FileStatus::Conflicted(_)))
        )
    }
}

pub fn load_status(worktree: WorktreeId, path: PathBuf, op: OpId) -> Task<Message> {
    Task::perform(
        async move {
            working_tree_status_detailed(&GitRunner::new(), &path)
                .await
                .map_err(|error| error.to_string())
        },
        move |result| Message::SourceControlStatusLoaded {
            worktree: worktree.clone(),
            op,
            result,
        },
    )
}

pub async fn status_now(path: PathBuf) -> Result<Vec<DetailedFileStatus>, String> {
    working_tree_status_detailed(&GitRunner::new(), &path)
        .await
        .map_err(|error| error.to_string())
}

pub fn run_operation(
    worktree: WorktreeId,
    path: PathBuf,
    op: OpId,
    operation: SourceControlOperation,
) -> Task<Message> {
    let operation_for_task = operation.clone();
    Task::perform(
        async move {
            let runner = GitRunner::new();
            match &operation_for_task {
                SourceControlOperation::Stage(file) => {
                    suaegi_git::write_ops::stage(&runner, &path, file)
                        .await
                        .map(|_| format!("Staged {file}"))
                        .map_err(|error| error.to_string())
                }
                SourceControlOperation::Unstage(file) => {
                    suaegi_git::write_ops::unstage(&runner, &path, file)
                        .await
                        .map(|_| format!("Unstaged {file}"))
                        .map_err(|error| error.to_string())
                }
                SourceControlOperation::Discard(file) => {
                    suaegi_git::write_ops::discard(&runner, &path, file)
                        .await
                        .map(|outcome| match outcome {
                            DiscardOutcome::RestoredTracked => {
                                format!("Restored {file} from HEAD")
                            }
                            DiscardOutcome::RemovedUntracked => {
                                format!("Removed untracked {file}")
                            }
                            DiscardOutcome::NothingToDiscard => {
                                format!("Nothing to discard for {file}")
                            }
                        })
                        .map_err(|error| error.to_string())
                }
                SourceControlOperation::Commit(message) => {
                    match suaegi_git::write_ops::commit_changes(&runner, &path, message).await {
                        Ok(CommitOutcome::Committed) => Ok("Commit created".to_string()),
                        Ok(CommitOutcome::Failed { message }) => Err(message),
                        Err(error) => Err(error.to_string()),
                    }
                }
                SourceControlOperation::Fetch => suaegi_git::remote::fetch(&runner, &path)
                    .await
                    .map(|_| "Fetch completed".to_string())
                    .map_err(|error| error.to_string()),
                SourceControlOperation::Pull => {
                    match suaegi_git::remote::pull(&runner, &path).await {
                        Ok(PullOutcome::Ok) => Ok("Pulled fast-forward changes".to_string()),
                        Ok(PullOutcome::UpToDate) => Ok("Already up to date".to_string()),
                        Ok(PullOutcome::NotFastForward) => {
                            Err("Pull stopped: local and remote branches diverged".to_string())
                        }
                        Err(error) => Err(error.to_string()),
                    }
                }
                SourceControlOperation::Push { branch } => {
                    match suaegi_git::remote::push(&runner, &path, branch, true).await {
                        Ok(PushOutcome::Ok) => Ok("Push completed".to_string()),
                        Ok(PushOutcome::UpToDate) => Ok("Everything up to date".to_string()),
                        Ok(PushOutcome::NonFastForwardRejected) => Err(
                            "Push rejected: remote has newer commits; pull or rebase first"
                                .to_string(),
                        ),
                        Ok(PushOutcome::AuthFailed) => {
                            Err("Push authentication failed".to_string())
                        }
                        Ok(PushOutcome::NetworkFailed) => {
                            Err("Push failed because the network is unavailable".to_string())
                        }
                        Ok(PushOutcome::Other) => Err("Push failed".to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                }
            }
        },
        move |result| Message::SourceControlOperationFinished {
            worktree: worktree.clone(),
            op,
            operation: operation.clone(),
            result,
        },
    )
}

fn status_label(status: &FileStatus) -> &'static str {
    match status {
        FileStatus::Modified => "M",
        FileStatus::Added => "A",
        FileStatus::Deleted => "D",
        FileStatus::Renamed { .. } => "R",
        FileStatus::Copied { .. } => "C",
        FileStatus::Untracked => "?",
        FileStatus::Conflicted(_) => "!",
        FileStatus::Other(_) => "·",
    }
}

fn file_row<'a>(
    entry: &'a DetailedFileStatus,
    busy: bool,
    display_path: String,
    show_staged: bool,
    show_unstaged: bool,
) -> Element<'a, Message> {
    let mut actions = row![].spacing(4).align_y(Alignment::Center);
    if show_unstaged {
        actions = actions.push(
            button(text("Stage").size(12))
                .on_press_maybe(
                    (!busy).then(|| Message::SourceControlStageRequested(entry.path.clone())),
                )
                .style(crate::theme::ghost_button),
        );
        actions = actions.push(
            button(text("Discard").size(12))
                .on_press_maybe(
                    (!busy).then(|| Message::SourceControlDiscardRequested(entry.path.clone())),
                )
                .style(crate::theme::danger_ghost_button),
        );
    }
    if show_staged {
        actions = actions.push(
            button(text("Unstage").size(12))
                .on_press_maybe(
                    (!busy).then(|| Message::SourceControlUnstageRequested(entry.path.clone())),
                )
                .style(crate::theme::ghost_button),
        );
    }
    row![
        text(status_label(&entry.status))
            .size(13)
            .color(Color::from_rgb(0.72, 0.48, 0.16)),
        text(display_path).size(13).width(Length::Fill),
        actions,
    ]
    .spacing(7)
    .align_y(Alignment::Center)
    .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeGroup {
    Changes,
    Staged,
    Untracked,
}

impl ChangeGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::Staged => "Staged Changes",
            Self::Untracked => "Untracked Changes",
        }
    }

    fn contains(self, entry: &DetailedFileStatus) -> bool {
        match self {
            Self::Changes => entry.unstaged && entry.status != FileStatus::Untracked,
            Self::Staged => entry.staged,
            Self::Untracked => entry.unstaged && entry.status == FileStatus::Untracked,
        }
    }
}

fn group_order(value: &str) -> [ChangeGroup; 3] {
    match value {
        "staged-first" => [
            ChangeGroup::Staged,
            ChangeGroup::Changes,
            ChangeGroup::Untracked,
        ],
        "untracked-first" => [
            ChangeGroup::Untracked,
            ChangeGroup::Changes,
            ChangeGroup::Staged,
        ],
        _ => [
            ChangeGroup::Changes,
            ChangeGroup::Staged,
            ChangeGroup::Untracked,
        ],
    }
}

pub fn view(state: &AppState) -> Option<Element<'_, Message>> {
    let source = state.source_control();
    if !source.is_open() {
        return None;
    }
    let worktree = source.worktree()?.clone();
    let (branch, base) = state
        .branch_context_for_worktree(&worktree)
        .unwrap_or_else(|| ("(detached)".to_string(), "main".to_string()));
    let base = if base.contains('/') {
        base
    } else {
        format!("origin/{base}")
    };
    let can_create_pr = matches!(
        forge_ui::create_pr_affordance(state.github_status_for(&worktree)),
        CreatePrAffordance::Offer
    );

    let header = row![
        button(text("Create PR").size(12))
            .on_press_maybe(can_create_pr.then_some(Message::CreatePrOpened {
                worktree: worktree.clone(),
            }),)
            .padding([3, 7])
            .style(crate::theme::selected_button),
        iced::widget::Space::new().width(Length::Fill),
        text_input("Filter", source.filter())
            .on_input(Message::SourceControlFilterChanged)
            .size(12)
            .padding([3, 6])
            .width(Length::Fixed(82.0)),
        button(icons::view(icons::Icon::Refresh, 12.0, theme::MUTED))
            .on_press_maybe((!source.busy()).then_some(Message::SourceControlRefreshRequested),)
            .padding([2, 5])
            .style(crate::theme::ghost_button),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let branch_row = column![
        text(branch).size(13),
        text(format!("→ {base}")).size(12).color(theme::MUTED),
    ]
    .spacing(2);
    let (has_staged, has_unstaged) = match &source.phase {
        PanelPhase::Ready { entries, .. } => (
            entries.iter().any(|entry| entry.staged),
            entries.iter().any(|entry| entry.unstaged),
        ),
        _ => (false, false),
    };
    let commit_ready = !source.busy() && has_staged && !source.commit_message.trim().is_empty();
    let commit_row = row![
        text_input("Message", &source.commit_message)
            .on_input(Message::SourceControlCommitMessageChanged)
            .on_submit(Message::SourceControlCommitRequested)
            .width(Length::Fill),
        button("Commit")
            .on_press_maybe(commit_ready.then_some(Message::SourceControlCommitRequested))
            .style(crate::theme::selected_button),
        button(if source.generating() {
            "Generating…"
        } else {
            "Generate"
        })
        .on_press_maybe(
            (state.ui_settings().source_control_ai.enabled && has_staged && !source.busy())
                .then_some(Message::SourceControlAiCommitMessageRequested),
        )
        .style(crate::theme::ghost_button),
    ]
    .spacing(5);
    let stage_all = button(text("+ Stage All").size(12))
        .on_press_maybe(
            (!source.busy() && has_unstaged)
                .then_some(Message::SourceControlStageRequested(".".to_string())),
        )
        .width(Length::Fill)
        .padding([4, 8])
        .style(crate::theme::ghost_button);

    let mut body = column![header, branch_row, commit_row, stage_all]
        .spacing(7)
        .padding([7, 8]);
    if source.has_conflicts() && state.ui_settings().source_control_ai.enabled {
        body = body.push(
            button(text("Resolve conflicts with agent").size(12))
                .on_press(Message::SourceControlAiLaunchActionRequested(
                    "resolveConflicts".to_string(),
                ))
                .style(crate::theme::selected_button),
        );
    }
    match &source.phase {
        PanelPhase::Closed => return None,
        PanelPhase::Loading { .. } => {
            body = body.push(text("Loading changes…").size(14));
        }
        PanelPhase::Failed { message, .. } => {
            body = body.push(
                text(message)
                    .size(13)
                    .color(Color::from_rgb(0.75, 0.22, 0.17)),
            );
        }
        PanelPhase::Ready { entries, .. } => {
            if entries.is_empty() {
                body = body.push(text("Working tree clean").size(14));
            } else {
                let mut files = column![].spacing(5);
                let filter = source.filter().to_lowercase();
                let tree_mode = state.ui_settings().source_control_view_mode == "tree";
                for group in group_order(&state.ui_settings().source_control_group_order) {
                    let visible = entries
                        .iter()
                        .filter(|entry| {
                            group.contains(entry)
                                && (filter.is_empty()
                                    || entry.path.to_lowercase().contains(&filter))
                        })
                        .collect::<Vec<_>>();
                    if visible.is_empty() {
                        continue;
                    }
                    files = files.push(text(group.label()).size(11).color(theme::MUTED));
                    let mut previous_directory = String::new();
                    for entry in visible {
                        let (directory, label) = entry
                            .path
                            .rsplit_once('/')
                            .map_or(("", entry.path.as_str()), |(directory, name)| {
                                (directory, name)
                            });
                        if tree_mode && directory != previous_directory {
                            previous_directory = directory.to_string();
                            if !directory.is_empty() {
                                files = files.push(
                                    text(format!("▾ {directory}")).size(11).color(theme::MUTED),
                                );
                            }
                        }
                        files = files.push(file_row(
                            entry,
                            source.busy(),
                            if tree_mode {
                                format!("  {label}")
                            } else {
                                entry.path.clone()
                            },
                            group == ChangeGroup::Staged,
                            group != ChangeGroup::Staged,
                        ));
                    }
                }
                body = body.push(scrollable(files).height(Length::FillPortion(2)));
            }
        }
    }

    if let Some(path) = &source.discard_confirmation {
        body = body.push(
            column![
                text(format!("Discard all unstaged changes in {path}?")).size(13),
                row![
                    button("Cancel")
                        .on_press(Message::SourceControlDiscardCancelled)
                        .style(crate::theme::ghost_button),
                    button("Discard")
                        .on_press(Message::SourceControlDiscardConfirmed)
                        .style(crate::theme::danger_ghost_button),
                ]
                .spacing(6),
            ]
            .spacing(6),
        );
    }

    body = body.push(
        row![
            button("Fetch")
                .on_press_maybe((!source.busy()).then_some(Message::SourceControlFetchRequested))
                .style(crate::theme::ghost_button),
            button("Pull")
                .on_press_maybe((!source.busy()).then_some(Message::SourceControlPullRequested))
                .style(crate::theme::ghost_button),
            button("Push")
                .on_press_maybe((!source.busy()).then_some(Message::SourceControlPushRequested))
                .style(crate::theme::ghost_button),
        ]
        .spacing(6),
    );

    if let Some(notice) = &source.notice {
        let (message, color) = match notice {
            Ok(message) => (message.as_str(), Color::from_rgb(0.18, 0.58, 0.31)),
            Err(message) => (message.as_str(), Color::from_rgb(0.75, 0.22, 0.17)),
        };
        body = body.push(text(message).size(13).color(color));
    }
    if let Some((action, _detail)) = source.launch_action() {
        body = body.push(
            button(
                text(if action == "fixCommitFailure" {
                    "Fix commit failure with agent"
                } else {
                    "Fix push failure with agent"
                })
                .size(12),
            )
            .on_press(Message::SourceControlAiLaunchActionRequested(
                action.to_string(),
            ))
            .style(crate::theme::selected_button),
        );
    }
    if source.busy() {
        body = body.push(text("Git operation in progress…").size(13));
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
    use super::{group_order, ChangeGroup, PanelPhase, SourceControlOperation, SourceControlState};
    use crate::state::OpId;
    use suaegi_core::domain::WorktreeId;

    fn wt(value: &str) -> WorktreeId {
        WorktreeId(value.to_string())
    }

    #[test]
    fn stale_status_cannot_replace_a_newer_worktree() {
        let mut state = SourceControlState::default();
        state.begin_load(wt("a"), OpId(1));
        state.begin_load(wt("b"), OpId(2));
        assert!(!state.accept_status(&wt("a"), OpId(1), Ok(Vec::new())));
        assert!(state.accept_status(&wt("b"), OpId(2), Ok(Vec::new())));
        assert!(matches!(state.phase, PanelPhase::Ready { .. }));
    }

    #[test]
    fn operation_result_requires_matching_worktree_op_and_kind() {
        let mut state = SourceControlState::default();
        state.begin_load(wt("a"), OpId(1));
        state.accept_status(&wt("a"), OpId(1), Ok(Vec::new()));
        let operation = SourceControlOperation::Stage("a.txt".to_string());
        assert!(state.begin_operation(OpId(2), operation.clone()));
        assert!(!state.accept_operation(&wt("a"), OpId(3), &operation, Ok("wrong".to_string())));
        assert!(state.accept_operation(&wt("a"), OpId(2), &operation, Ok("done".to_string())));
    }

    #[test]
    fn discard_needs_an_explicit_confirm_step() {
        let mut state = SourceControlState::default();
        state.request_discard("important.txt".to_string());
        assert_eq!(
            state.take_confirmed_discard().as_deref(),
            Some("important.txt")
        );
        assert!(state.take_confirmed_discard().is_none());
    }

    #[test]
    fn source_control_group_order_matches_the_three_orca_policies() {
        assert_eq!(
            group_order("changes-first"),
            [
                ChangeGroup::Changes,
                ChangeGroup::Staged,
                ChangeGroup::Untracked
            ]
        );
        assert_eq!(group_order("staged-first")[0], ChangeGroup::Staged);
        assert_eq!(group_order("untracked-first")[0], ChangeGroup::Untracked);
        assert_eq!(group_order("invalid")[0], ChangeGroup::Changes);
    }
}
