use std::path::{Path, PathBuf};

use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Alignment, Element, Length};

use suaegi_core::domain::{Repo, RepoId, WorktreeId};
use suaegi_git::worktree::WorktreeEntry;

use crate::persistence_thread::{LoadOrigin, SaveStatus};
use crate::state::{AppState, Message};

/// 사이드바 고정 폭. `pane_grid`는 고정 폭 pane이 없고(비율 분할만) 사이드바가
/// 터미널 격자 한가운데로 드래그될 수 있으므로, 사이드바는 pane이 아니라 상위
/// `row!` 레이아웃에서 이 폭으로 못박은 별도 위젯이다.
pub const WIDTH: f32 = 260.0;

pub fn view(state: &AppState) -> Element<'_, Message> {
    let mut list = column![add_repo_row(state)].spacing(16).padding(12);

    for group in grouped_worktrees(state) {
        list = list.push(repo_group(state, &group));
    }

    if let Some(error) = state.last_error() {
        list = list.push(text(format!("! {error}")).size(12));
    }

    let mut layout = column![scrollable(list).height(Length::Fill)].height(Length::Fill);
    if let Some(status) = status_line(state) {
        layout = layout.push(container(text(status).size(12)).padding(8));
    }

    container(layout)
        .width(Length::Fixed(WIDTH))
        .height(Length::Fill)
        .into()
}

fn add_repo_row(state: &AppState) -> Element<'_, Message> {
    let value = state.repo_path_input();
    row![
        text_input("/path/to/repo", value)
            .on_input(Message::RepoPathInputChanged)
            .on_submit(Message::AddRepoSubmitted)
            .width(Length::Fill),
        button("Add")
            .on_press_maybe((!value.trim().is_empty()).then_some(Message::AddRepoSubmitted)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

struct RepoGroup<'a> {
    repo: &'a Repo,
    worktrees: Vec<&'a WorktreeEntry>,
}

/// 뷰가 그리는 repo → worktree 그룹. `state.repos()`의 등록 순서를 그대로
/// 따르므로 (HashMap 반복 순서가 아니므로) 프레임마다 순서가 흔들리지 않는다.
/// 삭제된 repo를 가리키는 worktree 항목은 이 repo 목록을 기준으로 순회하는
/// 이상 애초에 방문되지 않는다 — 패닉 없이 조용히 빠진다.
fn grouped_worktrees(state: &AppState) -> Vec<RepoGroup<'_>> {
    state
        .repos()
        .iter()
        .map(|repo| RepoGroup {
            repo,
            worktrees: state.worktrees_for(&repo.id).iter().collect(),
        })
        .collect()
}

fn repo_group<'a>(state: &'a AppState, group: &RepoGroup<'a>) -> Element<'a, Message> {
    let repo_id = group.repo.id.clone();
    let draft = state.worktree_name_draft(&repo_id);

    let header = text(group.repo.display_name.clone()).size(15);

    let repo_id_for_input = repo_id.clone();
    let repo_id_for_submit = repo_id.clone();
    let repo_id_for_button = repo_id.clone();
    let create_row = row![
        text_input("new-worktree-name", draft)
            .on_input(move |value| Message::WorktreeNameInputChanged {
                repo_id: repo_id_for_input.clone(),
                value,
            })
            .on_submit(Message::CreateWorktreeSubmitted {
                repo_id: repo_id_for_submit.clone()
            })
            .width(Length::Fill),
        button("+ worktree").on_press_maybe((!draft.trim().is_empty()).then(|| {
            Message::CreateWorktreeSubmitted {
                repo_id: repo_id_for_button.clone(),
            }
        })),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let mut rows = column![header, create_row].spacing(6);
    for entry in &group.worktrees {
        let is_selected = state.selected_worktree() == Some(&worktree_id_for(&entry.path));
        rows = rows.push(worktree_row(repo_id.clone(), entry, is_selected));
    }

    container(rows).width(Length::Fill).into()
}

/// git이 돌려주는 `WorktreeEntry`에는 안정적인 id가 없다. `RepoId`가 정규화된
/// 절대 경로 문자열이듯, worktree 경로도 이미 canonical absolute path다
/// (`add_worktree`가 canonicalize한 parent 아래 만든다) — 같은 규칙을 따른다.
fn worktree_id_for(path: &Path) -> WorktreeId {
    WorktreeId(path.to_string_lossy().into_owned())
}

/// 존재 배지 자리는 비워둔 채 폭만 잡는다 — Task 7이 실제 `AgentPresence`로 채운다.
fn worktree_row(
    repo_id: RepoId,
    entry: &WorktreeEntry,
    is_selected: bool,
) -> Element<'static, Message> {
    let worktree_id = worktree_id_for(&entry.path);
    let label = entry
        .branch
        .clone()
        .unwrap_or_else(|| "(detached)".to_string());
    let marker = if is_selected { "> " } else { "  " };

    let remove_id = worktree_id.clone();
    let remove_path: PathBuf = entry.path.clone();
    let remove_branch = entry.branch.clone();

    row![
        space()
            .width(Length::Fixed(10.0))
            .height(Length::Fixed(10.0)),
        button(text(format!("{marker}{label}")))
            .on_press(Message::WorktreeSelected(worktree_id))
            .width(Length::Fill),
        button("remove").on_press(Message::RemoveWorktreeRequested {
            repo_id,
            worktree_id: remove_id,
            worktree_path: remove_path,
            branch: remove_branch,
        }),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// `LoadOrigin::Fresh`(신규 설치)와 `Loaded`(정상 로드)는 경고가 없다.
/// `Recovered`/`RecoveryFailed`는 알린다. 저장 실패(`SaveStatus::Failed`)는
/// 항상 최우선으로 드러나야 하고, 정상적인 디바운스 대체(`Superseded`)는
/// 절대 에러처럼 보이면 안 된다 — 안 그러면 사용자가 상태 표시줄 자체를
/// 무시하는 법을 배운다.
fn status_line(state: &AppState) -> Option<String> {
    if let Some(SaveStatus::Failed(message)) = state.last_save_status() {
        return Some(format!("Save failed: {message}"));
    }
    match state.load_origin() {
        LoadOrigin::Fresh | LoadOrigin::Loaded => None,
        LoadOrigin::Recovered { slot } => Some(format!(
            "Recovered from backup #{slot} — a recent save may be missing."
        )),
        LoadOrigin::RecoveryFailed => {
            Some("Could not read saved data — starting from an empty state.".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence_thread::SaveReport;
    use crate::state::OpId;

    fn repo(name: &str) -> Repo {
        Repo {
            id: RepoId(format!("/tmp/{name}")),
            path: PathBuf::from(format!("/tmp/{name}")),
            display_name: name.to_string(),
            worktree_base_ref: None,
        }
    }

    fn entry(name: &str) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(format!("/tmp/wt/{name}")),
            branch: Some(name.to_string()),
            head: None,
            is_main: false,
        }
    }

    #[test]
    fn worktree_rows_group_under_their_repo_in_a_stable_order() {
        let mut state = AppState::default();
        let repo_b = repo("b-repo");
        let repo_a = repo("a-repo");
        // 등록 순서를 일부러 알파벳 역순으로 해서, "정렬됐다"가 아니라
        // "등록 순서를 보존한다"는 걸 검증한다.
        state.upsert_repo(repo_b.clone());
        state.upsert_repo(repo_a.clone());

        state.note_list_issued(repo_a.id.clone(), OpId(1));
        state.apply_worktree_listing(repo_a.id.clone(), OpId(1), vec![entry("a1"), entry("a2")]);
        state.note_list_issued(repo_b.id.clone(), OpId(1));
        state.apply_worktree_listing(repo_b.id.clone(), OpId(1), vec![entry("b1")]);

        let groups = grouped_worktrees(&state);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0].repo.id, repo_b.id,
            "registration order must win, not alphabetical"
        );
        assert_eq!(groups[1].repo.id, repo_a.id);
        assert_eq!(
            groups[1]
                .worktrees
                .iter()
                .map(|w| w.branch.clone())
                .collect::<Vec<_>>(),
            vec![Some("a1".to_string()), Some("a2".to_string())],
        );

        // 순서는 호출마다 안정적이어야 한다 (HashMap 반복 순서에 기대면 흔들린다).
        let groups_again = grouped_worktrees(&state);
        let ids: Vec<_> = groups.iter().map(|g| g.repo.id.clone()).collect();
        let ids_again: Vec<_> = groups_again.iter().map(|g| g.repo.id.clone()).collect();
        assert_eq!(ids, ids_again);
    }

    #[test]
    fn a_worktree_whose_repo_is_gone_is_skipped_without_panicking() {
        let mut state = AppState::default();
        let gone = RepoId("/tmp/deleted-repo".into());
        // repo는 등록돼 있지 않다 — 영속화된 worktree가 삭제된 repo를 가리키는
        // 상황을 흉내낸다.
        state.note_list_issued(gone.clone(), OpId(1));
        state.apply_worktree_listing(gone, OpId(1), vec![entry("orphan")]);

        let groups = grouped_worktrees(&state);
        assert!(
            groups.is_empty(),
            "an orphaned worktree entry must not surface a group"
        );
    }

    #[test]
    fn status_line_text_distinguishes_fresh_install_from_recovery_failure() {
        assert!(status_line(&AppState::fresh()).is_none());
        assert!(status_line(&AppState::recovery_failed()).is_some());
        assert!(status_line(&AppState::recovered(0)).is_some());
    }

    #[test]
    fn a_failed_save_is_visible_in_the_status_line() {
        assert!(status_line(&AppState::with_save_error("disk full"))
            .unwrap()
            .contains("disk full"));
    }

    #[test]
    fn a_superseded_save_does_not_look_like_an_error() {
        // Superseded는 정상적인 debounce 대체다 — 에러처럼 보이면 사용자가
        // 상태 표시줄을 무시하는 법을 배운다.
        let mut state = AppState::fresh();
        let _ = state.update(Message::Saved(SaveReport {
            seq: 1,
            status: SaveStatus::Superseded { by: 2 },
        }));
        assert!(status_line(&state).is_none());
    }
}
