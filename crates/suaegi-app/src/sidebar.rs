use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Color, Element, Length};

use suaegi_core::domain::{Repo, RepoId};
use suaegi_git::worktree::WorktreeEntry;
use suaegi_term::presence::AgentPresence;

use crate::persistence_thread::{LoadOrigin, SaveStatus};
use crate::state::{worktree_id_for, AppState, Message};

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
        let worktree_id = worktree_id_for(&entry.path);
        let is_selected = state.selected_worktree() == Some(&worktree_id);
        let presence = state.worktree_presence(&worktree_id);
        rows = rows.push(worktree_row(repo_id.clone(), entry, is_selected, presence));
    }

    container(rows).width(Length::Fill).into()
}

/// 존재 배지: 세션이 없거나 아직 판정 전이면(`Unknown`) 아무 표시도 없다 —
/// "모른다"를 굳이 시끄럽게 알릴 필요는 없다. `Agent`는 채워진 점,
/// `Exited`/`NoAgent`(에이전트가 foreground를 내줬거나 셸로 돌아간 경우)는
/// 옅게 구분한다. `working|waiting|done` 3색 상태는 Plan 5(hook 서버)의
/// 몫이다 — 여기서는 "에이전트가 떠 있는지"만 안다.
/// `Element`는 직접 검사할 수 없으므로 매핑 자체를 순수 함수로 뽑아 테스트한다.
fn badge_glyph(presence: AgentPresence) -> (&'static str, Color) {
    match presence {
        AgentPresence::Agent(_) => ("●", Color::from_rgb8(0x2e, 0xa0, 0x43)),
        AgentPresence::Exited { .. } => ("×", Color::from_rgb8(0xc0, 0x39, 0x2b)),
        AgentPresence::NoAgent => ("○", Color::from_rgb8(0x88, 0x88, 0x88)),
        AgentPresence::Unknown => ("", Color::TRANSPARENT),
    }
}

fn presence_badge(presence: AgentPresence) -> Element<'static, Message> {
    let (label, color) = badge_glyph(presence);
    container(text(label).size(10).color(color))
        .width(Length::Fixed(10.0))
        .height(Length::Fixed(10.0))
        .into()
}

fn worktree_row(
    repo_id: RepoId,
    entry: &WorktreeEntry,
    is_selected: bool,
    presence: AgentPresence,
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
        presence_badge(presence),
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
    use crate::persistence_thread::{LoadDiagnostics, SaveReport};
    use crate::state::OpId;
    use suaegi_core::domain::PersistedState;

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

    /// Task 8: `PersistenceHandle::spawn`이 만드는 `LoadDiagnostics`가 실제로
    /// `AppState::from_load`(부팅이 쓰는 바로 그 함수)를 거쳐 상태 표시줄까지
    /// 흘러가는지. 위 테스트는 손으로 만든 `AppState::fresh()`류 헬퍼로
    /// `status_line`의 순수 매핑만 검증하지만, `from_load`가 `load.origin`을
    /// `state.load_origin`에 대입하는 걸 빠뜨리는 mutation은 그걸로는 못
    /// 잡는다 — 이 테스트가 그 배선 자체를 태운다. **`Fresh`는 절대 경고를
    /// 내면 안 된다**: 신규 설치가 데이터 손실처럼 보이면 안 되기 때문이다.
    #[test]
    fn load_diagnostics_reach_the_status_line_through_the_real_boot_wiring_for_all_four_origins() {
        let cases = [
            (LoadOrigin::Fresh, false),
            (LoadOrigin::Loaded, false),
            (LoadOrigin::Recovered { slot: 2 }, true),
            (LoadOrigin::RecoveryFailed, true),
        ];
        for (origin, expects_warning) in cases {
            let load = LoadDiagnostics {
                state: PersistedState::default(),
                origin,
                save_blocked: false,
            };
            let state = AppState::from_load(load);
            assert_eq!(
                status_line(&state).is_some(),
                expects_warning,
                "origin {origin:?} must {} a status-line warning",
                if expects_warning {
                    "produce"
                } else {
                    "not produce"
                }
            );
        }
    }

    #[test]
    fn a_failed_save_is_visible_in_the_status_line() {
        assert!(status_line(&AppState::with_save_error("disk full"))
            .unwrap()
            .contains("disk full"));
    }

    /// 위 테스트는 손으로 만든 `with_save_error` 헬퍼로 `status_line`의 순수
    /// 매핑만 본다. 이 테스트는 실제 `Message::Saved` 디스패치(`AppState::boot`가
    /// `results` 스트림을 연결하면 실제로 도착하는 바로 그 메시지)를 태워
    /// `last_save_status`에 반영되는 배선 자체를 검증한다.
    #[test]
    fn a_failed_save_status_reaches_the_status_line_through_real_dispatch() {
        let mut state = AppState::fresh();
        let _ = state.update(Message::Saved(SaveReport {
            seq: 1,
            status: SaveStatus::Failed("disk full".to_string()),
        }));
        assert!(status_line(&state)
            .expect("a failed save must surface a warning")
            .contains("disk full"));
    }

    #[test]
    fn presence_glyphs_distinguish_agent_from_no_agent_and_unknown() {
        use suaegi_term::agent::AgentKind;

        let (agent_glyph, _) = badge_glyph(AgentPresence::Agent(AgentKind::Claude));
        let (no_agent_glyph, _) = badge_glyph(AgentPresence::NoAgent);
        let (unknown_glyph, _) = badge_glyph(AgentPresence::Unknown);
        let (exited_glyph, _) = badge_glyph(AgentPresence::Exited { code: 0 });

        assert!(!agent_glyph.is_empty());
        assert_ne!(agent_glyph, no_agent_glyph);
        assert_ne!(agent_glyph, unknown_glyph);
        assert_ne!(no_agent_glyph, exited_glyph);
        // "모른다"는 조용히 아무것도 안 보여준다 — 시끄러운 badge는 아직
        // 판정 전인 worktree 전부를 에러처럼 보이게 만든다.
        assert!(unknown_glyph.is_empty());
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
