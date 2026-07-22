use std::path::PathBuf;

use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_input,
};
use iced::{Alignment, Color, Element, Length};

use suaegi_core::domain::{Repo, RepoId};
use suaegi_forge::{ChecksSummary, ReviewState};
use suaegi_git::worktree::WorktreeEntry;
use suaegi_term::presence::AgentPresence;

use crate::agent_status::contract::BadgeState;
use crate::forge_ui::{self, CreatePrAffordance, PrIndicator};
use crate::persistence_thread::{LoadOrigin, SaveStatus};
use crate::state::{worktree_id_for, AppState, CreatePrDraft, Message};

// PR 표시자 색. 배지와 같은 팔레트 계열이되(사이드바 톤 통일) 상태를 색으로 구별한다.
const PR_NEUTRAL: Color = Color::from_rgb(0.53, 0.53, 0.53);
const PR_OPEN: Color = Color::from_rgb(0.18, 0.63, 0.26);
const PR_MERGED: Color = Color::from_rgb(0.52, 0.34, 0.72);
const PR_CLOSED: Color = Color::from_rgb(0.75, 0.22, 0.17);
const PR_UNKNOWN: Color = Color::from_rgb(0.85, 0.55, 0.0);

/// 사이드바 고정 폭. `pane_grid`는 고정 폭 pane이 없고(비율 분할만) 사이드바가
/// 터미널 격자 한가운데로 드래그될 수 있으므로, 사이드바는 pane이 아니라 상위
/// `row!` 레이아웃에서 이 폭으로 못박은 별도 위젯이다.
pub const WIDTH: f32 = 260.0;

pub fn view(state: &AppState) -> Element<'_, Message> {
    let mut list = column![add_repo_row(state)].spacing(16).padding(12);

    // Create-PR 다이얼로그(열려 있을 때만). 픽셀·상호작용은 사람 눈으로 본다 —
    // 로직(자격 게이팅, 상태 매핑)은 `forge_ui`가 검사한다.
    if let Some(dialog) = state.create_pr_dialog() {
        list = list.push(create_pr_form(dialog));
    }

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
    let name_row = row![
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

    // 에이전트 피커. 옵션은 로그인 셸(기본) + **설치된** 에이전트만 — 목록에
    // 있으면 곧 설치돼 있다는 뜻이라 고른 게 exec 실패로 이어지지 않는다. 기본
    // 선택이 "Login shell"이라 피커를 무시하면 오늘의 동작 그대로다.
    let repo_id_for_agent = repo_id.clone();
    let agent_picker = pick_list(
        state.agent_picker_choices(),
        Some(state.worktree_agent_selection(&repo_id)),
        move |choice| Message::WorktreeAgentSelected {
            repo_id: repo_id_for_agent.clone(),
            choice,
        },
    )
    .width(Length::Fill)
    .text_size(12);

    // 선택적 초기 프롬프트. 비워두면 주입 없음(기본). 채우면 새 worktree의 첫
    // 세션에 한 번 실린다 — argv/flag 에이전트는 스폰 인자로, stdin-after-start
    // 에이전트는 composer 준비 후 PTY로. **영속화하지 않는다**(일회성 launch 인자).
    let repo_id_for_prompt = repo_id.clone();
    let repo_id_for_prompt_submit = repo_id.clone();
    let prompt_input = text_input(
        "initial prompt (optional)",
        state.worktree_prompt_draft(&repo_id),
    )
    .on_input(move |value| Message::WorktreePromptInputChanged {
        repo_id: repo_id_for_prompt.clone(),
        value,
    })
    .on_submit(Message::CreateWorktreeSubmitted {
        repo_id: repo_id_for_prompt_submit.clone(),
    })
    .width(Length::Fill)
    .size(12);
    let create_row = column![agent_picker, prompt_input, name_row].spacing(6);

    let mut rows = column![header, create_row].spacing(6);
    for entry in &group.worktrees {
        let worktree_id = worktree_id_for(&entry.path);
        let is_selected = state.selected_worktree() == Some(&worktree_id);
        let presence = state.worktree_presence(&worktree_id);
        let badge = state.worktree_badge(&worktree_id);
        rows = rows.push(worktree_row(repo_id.clone(), entry, is_selected, badge, presence));
        // Plan 7a-1: PR 상태 표시자 + 새로고침 + Create-PR 어포던스. 캐시에서
        // 파생하되 매핑은 `forge_ui`가 검사한다(Unavailable≠NoPr, Offer는 자격 있을 때만).
        let status = state.github_status_for(&worktree_id);
        if let Some(pr_row) = pr_status_row(
            worktree_id,
            forge_ui::indicator_for(status),
            forge_ui::create_pr_affordance(status),
        ) {
            rows = rows.push(pr_row);
        }
    }

    container(rows).width(Length::Fill).into()
}

/// 에이전트 상태 배지. **`Unknown`은 `Working`과 시각적으로 구별한다** — "모른다"와
/// "바쁘다"는 다른 상태이고, 사용자가 그 둘을 구별할 수 있어야 한다. 같은 글리프를
/// 옅게만 쓰면 색 대비가 약한 화면에서 구별이 사라지므로 **글리프도 색도** 다르다.
///
/// **오류 스타일링만 `AgentPresence`를 직접 읽는다.** `BadgeState`에는 일부러 오류
/// 변형이 없다 — 리듀서 반환에 변형을 더하면 배지 상태와 프로세스 사실이 두 곳에서
/// 관리된다. 리듀서는 "무슨 상태인가"만 답하고, "어떻게 끝났는가"는 여기서 본다.
///
/// `Element`는 직접 검사할 수 없으므로 매핑 자체를 순수 함수로 뽑아 테스트한다.
fn badge_glyph(badge: BadgeState, presence: AgentPresence) -> (&'static str, Color) {
    // 0이 아닌 종료 코드는 상태와 무관하게 오류로 보여야 한다.
    if let AgentPresence::Exited { code } = presence {
        if code != 0 {
            return ("×", Color::from_rgb8(0xc0, 0x39, 0x2b));
        }
    }
    match badge {
        BadgeState::Working => ("●", Color::from_rgb8(0x2e, 0xa0, 0x43)),
        // 사람을 기다린다 — 이 플랜에서 사용자가 가장 알고 싶은 상태다.
        BadgeState::Waiting => ("◆", Color::from_rgb8(0xd8, 0x8c, 0x00)),
        BadgeState::Done => ("○", Color::from_rgb8(0x88, 0x88, 0x88)),
        // 글리프와 색이 **둘 다** Working과 다르다.
        BadgeState::Unknown => ("·", Color::from_rgb8(0xbb, 0xbb, 0xbb)),
    }
}

fn presence_badge(badge: BadgeState, presence: AgentPresence) -> Element<'static, Message> {
    let (label, color) = badge_glyph(badge, presence);
    container(text(label).size(10).color(color))
        .width(Length::Fixed(10.0))
        .height(Length::Fixed(10.0))
        .into()
}

/// git이 non-forced `worktree remove`로 main 체크아웃을 항상 거부하므로
/// 지우는 버튼을 눌러도 안전은 하지만, 애초에 버튼을 안 보여주는 게 낫다 —
/// 눌러도 아무 일도 안 일어나는 죽은 버튼보다 명확하다.
fn worktree_is_removable(entry: &WorktreeEntry) -> bool {
    !entry.is_main
}

fn worktree_row(
    repo_id: RepoId,
    entry: &WorktreeEntry,
    is_selected: bool,
    badge: BadgeState,
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

    let diff_id = worktree_id.clone();
    let mut cells: Vec<Element<'static, Message>> = vec![
        presence_badge(badge, presence),
        button(text(format!("{marker}{label}")))
            .on_press(Message::WorktreeSelected(worktree_id))
            .width(Length::Fill)
            .into(),
        // diff 패널 토글. 같은 worktree를 다시 누르면 닫힌다.
        button(text("diff").size(11))
            .on_press(Message::DiffRequested { worktree: diff_id })
            .into(),
    ];
    if worktree_is_removable(entry) {
        cells.push(
            button("remove")
                .on_press(Message::RemoveWorktreeRequested {
                    repo_id,
                    worktree_id: remove_id,
                    worktree_path: remove_path,
                    branch: remove_branch,
                })
                .into(),
        );
    }

    row(cells).spacing(6).align_y(Alignment::Center).into()
}

/// PR 상태 한 줄: 표시자 + 새로고침(↻) + Create-PR 어포던스. 그릴 게 하나도 없으면
/// (조회 전이거나 GitHub 리포가 아니면) `None`을 돌려 행 자체를 넣지 않는다.
///
/// **표시자 텍스트/색은 사람 눈으로 보는 픽셀이다.** 검사되는 결정은 `forge_ui`의
/// `indicator_for`/`create_pr_affordance`(순수)이고, 여기 `pr_indicator_label`은 그
/// 결과가 시각적으로 구별되는지만 얕게 테스트한다(배지의 `badge_glyph`와 같은 규율).
fn pr_status_row(
    worktree_id: suaegi_core::domain::WorktreeId,
    indicator: PrIndicator,
    affordance: CreatePrAffordance,
) -> Option<Element<'static, Message>> {
    let label = pr_indicator_label(&indicator);
    // 조회 전(라벨 없음) + 어포던스 숨김 = 이 worktree엔 보여줄 GitHub 정보가 없다.
    if label.is_none() && matches!(affordance, CreatePrAffordance::Hidden) {
        return None;
    }

    let mut cells: Vec<Element<'static, Message>> = Vec::new();
    if let Some((txt, color)) = label {
        cells.push(text(txt).size(11).color(color).into());
    }
    // 명시적 수동 새로고침(배경 폴링 없음 — §3.7).
    let refresh_id = worktree_id.clone();
    cells.push(
        button(text("↻").size(11))
            .on_press(Message::GithubRefreshRequested {
                worktree: refresh_id,
            })
            .into(),
    );
    match affordance {
        CreatePrAffordance::Offer => cells.push(
            button(text("Create PR").size(11))
                .on_press(Message::CreatePrOpened {
                    worktree: worktree_id,
                })
                .into(),
        ),
        // 죽은 버튼 대신 이유. 예: NoUpstream → "push the branch first".
        CreatePrAffordance::Blocked(reason) => {
            cells.push(text(reason).size(10).color(PR_NEUTRAL).into())
        }
        CreatePrAffordance::Hidden => {}
    }

    Some(row(cells).spacing(6).align_y(Alignment::Center).into())
}

/// PR 표시자 → (문구, 색). `None` = 아무것도 안 그린다(Hidden). **`Unknown`은
/// `NoPr`와 문구·색이 모두 다르다** — "상태 모름"이 "PR 없음"으로 보이면 안 된다.
fn pr_indicator_label(indicator: &PrIndicator) -> Option<(String, Color)> {
    match indicator {
        PrIndicator::Hidden => None,
        PrIndicator::Checking => Some(("PR …".to_string(), PR_NEUTRAL)),
        PrIndicator::NoPr => Some(("no PR".to_string(), PR_NEUTRAL)),
        PrIndicator::Present {
            number,
            state,
            checks,
        } => {
            let mut label = format!("PR #{number} {}", pr_state_text(*state));
            if let Some(summary) = checks_text(*checks) {
                label.push(' ');
                label.push_str(&summary);
            }
            Some((label, pr_state_color(*state)))
        }
        // 실행 가능한 힌트를 붙인다(NotAuthenticated → "run gh auth login" 등).
        PrIndicator::Unknown(reason) => Some((
            format!("PR ? {}", forge_ui::unavailable_text(reason)),
            PR_UNKNOWN,
        )),
    }
}

fn pr_state_text(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Open => "open",
        ReviewState::Merged => "merged",
        ReviewState::Closed => "closed",
        ReviewState::Draft => "draft",
    }
}

fn pr_state_color(state: ReviewState) -> Color {
    match state {
        ReviewState::Open => PR_OPEN,
        ReviewState::Merged => PR_MERGED,
        ReviewState::Closed => PR_CLOSED,
        ReviewState::Draft => PR_NEUTRAL,
    }
}

/// CI 체크 요약 "✓passing ✗failing •pending". 체크가 하나도 없으면 `None`(조용).
fn checks_text(checks: ChecksSummary) -> Option<String> {
    if checks.passing == 0 && checks.failing == 0 && checks.pending == 0 {
        return None;
    }
    Some(format!(
        "✓{} ✗{} •{}",
        checks.passing, checks.failing, checks.pending
    ))
}

/// Create-PR 다이얼로그 폼. **픽셀·상호작용은 사람 눈**이다 — 자격 게이팅과 성공 시
/// 링크 영속화는 `state`/`forge_ui`에서 검사한다.
fn create_pr_form(dialog: &CreatePrDraft) -> Element<'_, Message> {
    let mut form = column![
        text("Create pull request").size(14),
        text_input("title", &dialog.title)
            .on_input(Message::CreatePrTitleChanged)
            .size(12),
        text_input("base branch", &dialog.base)
            .on_input(Message::CreatePrBaseChanged)
            .size(12),
        text_input("body (blank = repo PR template)", &dialog.body)
            .on_input(Message::CreatePrBodyChanged)
            .size(12),
        checkbox(dialog.draft)
            .label("Draft")
            .on_toggle(Message::CreatePrDraftToggled)
            .text_size(12),
    ]
    .spacing(6);

    if let Some(err) = &dialog.error {
        form = form.push(text(format!("! {err}")).size(11).color(PR_CLOSED));
    }

    // 제출 중이면 버튼을 잠근다(중복 제출 방지).
    let submit_label = if dialog.submitting {
        "Creating…"
    } else {
        "Create"
    };
    let submit = button(text(submit_label).size(12))
        .on_press_maybe((!dialog.submitting).then_some(Message::CreatePrSubmitted));
    let cancel = button(text("Cancel").size(12)).on_press(Message::CreatePrCancelled);
    form = form.push(row![submit, cancel].spacing(6));

    container(form).padding(8).into()
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
        state.apply_authoritative_listing(
            repo_a.id.clone(),
            OpId(1),
            vec![entry("a1"), entry("a2")],
        );
        state.note_list_issued(repo_b.id.clone(), OpId(1));
        state.apply_authoritative_listing(repo_b.id.clone(), OpId(1), vec![entry("b1")]);

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
        state.apply_authoritative_listing(gone, OpId(1), vec![entry("orphan")]);

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

    /// **`Unknown`과 `Working`은 반드시 구별된다.** "모른다"와 "바쁘다"는 다른
    /// 상태이고, 이 구별이 사라지면 훅이 안 붙은 pane(신뢰 대화상자 대기 등)이
    /// 열심히 일하는 것처럼 보인다.
    #[test]
    fn every_badge_state_is_visually_distinct() {
        let agent = AgentPresence::Agent("claude");
        let glyphs: Vec<(&str, Color)> = [
            BadgeState::Working,
            BadgeState::Waiting,
            BadgeState::Done,
            BadgeState::Unknown,
        ]
        .into_iter()
        .map(|b| badge_glyph(b, agent))
        .collect();

        for (i, (glyph, color)) in glyphs.iter().enumerate() {
            assert!(!glyph.is_empty(), "state {i} must render something");
            for (j, (other_glyph, other_color)) in glyphs.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        glyph, other_glyph,
                        "badge states {i} and {j} share a glyph — 'we don't know' must not \
                         look like 'it is busy'"
                    );
                    assert_ne!(
                        (color.r, color.g, color.b),
                        (other_color.r, other_color.g, other_color.b),
                        "badge states {i} and {j} share a colour"
                    );
                }
            }
        }
    }

    /// 오류 스타일링은 **리듀서가 아니라** `AgentPresence::Exited{{code}}`에서 온다.
    /// `BadgeState`에 오류 변형을 더하면 배지 상태와 프로세스 사실이 두 곳에서
    /// 관리된다.
    #[test]
    fn a_nonzero_exit_is_styled_as_an_error_whatever_the_badge_says() {
        let (glyph, color) = badge_glyph(BadgeState::Done, AgentPresence::Exited { code: 1 });
        assert_eq!(glyph, "×");
        assert_eq!((color.r, color.g, color.b), {
            let red = Color::from_rgb8(0xc0, 0x39, 0x2b);
            (red.r, red.g, red.b)
        });

        // 대조군: 정상 종료(0)는 오류로 보이지 않는다 — 그렇지 않으면 성공적으로
        // 끝난 세션이 전부 빨간 ×가 된다.
        let (ok_glyph, _) = badge_glyph(BadgeState::Done, AgentPresence::Exited { code: 0 });
        assert_ne!(
            ok_glyph, "×",
            "exit code 0 is a normal finish, not a failure"
        );
        assert_eq!(ok_glyph, badge_glyph(BadgeState::Done, AgentPresence::NoAgent).0);
    }

    /// 최종 리뷰 항목 3: `list_worktrees`가 첫 엔트리에 `is_main: true`를
    /// 세우는데(`suaegi-git`), 여기서 그걸 읽지 않으면 git이 항상 거부할
    /// main 체크아웃에도 remove 버튼이 뜬다.
    #[test]
    fn the_main_worktree_checkout_is_not_removable() {
        let main = WorktreeEntry {
            is_main: true,
            ..entry("main")
        };
        let secondary = WorktreeEntry {
            is_main: false,
            ..entry("feature")
        };
        assert!(!worktree_is_removable(&main));
        assert!(worktree_is_removable(&secondary));
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

    use suaegi_forge::ForgeUnavailable;

    /// **"상태 모름"과 "PR 없음"은 시각적으로 구별된다** — `indicator_for`(§5 (a))가
    /// 둘을 다른 변형으로 나눠도, 라벨이 같은 문구·색이면 화면에서 구별이 사라진다.
    /// 이 테스트가 뷰 쪽 계약을 잠근다.
    #[test]
    fn unknown_status_never_looks_like_no_pr() {
        let no_pr = pr_indicator_label(&PrIndicator::NoPr).expect("no PR has a label");
        let unknown = pr_indicator_label(&PrIndicator::Unknown(ForgeUnavailable::NotAuthenticated))
            .expect("unknown has a label");
        assert_ne!(no_pr.0, unknown.0, "'no PR' and 'status unknown' must read differently");
        assert_ne!(
            (no_pr.1.r, no_pr.1.g, no_pr.1.b),
            (unknown.1.r, unknown.1.g, unknown.1.b),
            "'no PR' and 'status unknown' must not share a colour"
        );
        // 인증 안 됨은 실행 가능한 힌트를 노출한다.
        assert!(unknown.0.contains("gh auth login"));
    }

    /// Hidden(조회 전/비-GitHub)은 아무 라벨도 없다 — worktree 행에 잡음을 안 남긴다.
    #[test]
    fn a_hidden_indicator_renders_no_label() {
        assert!(pr_indicator_label(&PrIndicator::Hidden).is_none());
    }

    /// Present는 번호·상태·체크 요약을 한 줄에 담는다.
    #[test]
    fn a_present_pr_shows_number_state_and_checks() {
        let label = pr_indicator_label(&PrIndicator::Present {
            number: 12,
            state: ReviewState::Open,
            checks: ChecksSummary {
                passing: 2,
                failing: 1,
                pending: 0,
            },
        })
        .expect("present has a label");
        assert!(label.0.contains("#12"));
        assert!(label.0.contains("open"));
        assert!(label.0.contains("✓2"));
        assert!(label.0.contains("✗1"));
    }

    #[test]
    fn checks_summary_is_silent_when_there_are_no_checks() {
        assert!(checks_text(ChecksSummary::default()).is_none());
    }
}
