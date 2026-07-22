//! Plan 7b: PR 패널 — 7a 상태 위에 얹히는 **머지가능성·리뷰·코멘트 읽기 + 확인
//! 게이트가 달린 파괴적 머지**(§4.6). diff 패널(`diff_panel.rs`)과 같은 구조다:
//! `AppState`에 필드 하나(`PrPanelState`)로 들어가고, 검사 가능한 순수 결정은
//! `forge_ui`가 본다(머지 버튼 게이팅·결과 표시·Unavailable≠none). 여기엔 상태
//! 전이·staleness 가드·view가 산다.
//!
//! **머지가 7a가 아니라 7b인 이유가 이 파일의 확인 흐름이다.** Merge 버튼은
//! 머지가능성이 `Mergeable`일 때만 켜지고(그 밖은 이유를 단 비활성), 눌러도 **바로
//! 머지하지 않는다** — 방식(merge/squash/rebase)을 고르는 **확인 단계**를 연 뒤,
//! 명시적 확정에서만 `merge_pr`을 부른다. 원클릭 파괴적 머지는 없다. 그 게이트를
//! 상태로 표현한 것이 `confirm: Option<MergeConfirm>`이고, `confirm_merge`가 그
//! 상태가 있을 때만 방식을 돌려준다(없으면 절대 머지를 발급하지 않는다).

use iced::widget::{button, checkbox, column, container, row, scrollable, text};
use iced::{Color, Element, Length};

use suaegi_core::domain::WorktreeId;
use suaegi_forge::{MergeMethod, MergeabilityState, ReviewState};

use crate::forge_ui::{
    self, CommentsLine, MergeButton, MergeResultDisplay, PrDetails, ReviewsLine,
};
use crate::state::{Message, OpId};

/// 패널 고정 폭. 사이드바·diff 패널과 같은 이유로 `row!` 레벨에서 못 박는다.
pub const WIDTH: f32 = 380.0;

/// 확인 단계에서 미리 고른 기본 방식. **HUMAN-EYES / 제품 선택**: 사용자는 확인
/// 단계에서 세 방식 중 하나를 명시적으로 고를 수 있고, 이건 그저 선택 커서의 시작
/// 위치다. GitHub 웹 기본은 repo 설정에 달렸으므로 여기선 가장 평이한 `Merge`를
/// 둔다(머지 커밋 생성).
const DEFAULT_MERGE_METHOD: MergeMethod = MergeMethod::Merge;

const OK: Color = Color::from_rgb(0.18, 0.63, 0.26);
const WARN: Color = Color::from_rgb(0.85, 0.55, 0.0);
const BAD: Color = Color::from_rgb(0.75, 0.22, 0.17);
const MUTED: Color = Color::from_rgb(0.53, 0.53, 0.53);

/// 세부(머지가능성·리뷰·코멘트) 로딩 상태.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum DetailsLoad {
    #[default]
    Loading,
    Loaded(PrDetails),
}

/// 확인 단계의 편집 상태. **`Some`이면 확인 단계가 열려 있다** — 파괴적 머지는
/// 오직 이 상태가 있을 때만 확정될 수 있다(원클릭 방지의 상태 계약).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConfirm {
    pub method: MergeMethod,
    pub delete_branch: bool,
}

/// PR 패널 전체 상태. **`AppState`에 필드 하나로 들어간다** — `DiffState`와 같은
/// 이유(동시 편집 충돌 면적 최소화).
#[derive(Debug, Default)]
pub struct PrPanelState {
    /// 지금 패널이 보여주는 worktree(`None` = 닫힘).
    worktree: Option<WorktreeId>,
    // 헤더(번호/상태/제목)는 **7a 리뷰에서** 연 시점에 씨딩한다 — 세부 조회로 다시
    // 받지 않는다(brief: 7a 상태-조회를 중복하지 않는다).
    number: Option<u64>,
    title: String,
    review_state: Option<ReviewState>,
    details: DetailsLoad,
    /// 확인 단계(없으면 닫힘). **파괴적 머지의 게이트다.**
    confirm: Option<MergeConfirm>,
    /// `merge_pr`가 진행 중 — 버튼을 잠근다.
    merging: bool,
    /// 마지막 머지 시도의 표시(Merged/Rejected/Unavailable, 셋이 구별된다).
    outcome: Option<MergeResultDisplay>,
    /// 세부 조회의 staleness 가드(수동 새로고침이 on-open 조회를 앞질러도 낡은
    /// 결과가 새것을 덮지 않게 한다 — diff 패널과 같은 규율).
    latest_details_op: Option<OpId>,
    latest_merge_op: Option<OpId>,
}

impl PrPanelState {
    pub fn is_open(&self) -> bool {
        self.worktree.is_some()
    }

    pub fn worktree(&self) -> Option<&WorktreeId> {
        self.worktree.as_ref()
    }

    pub fn number(&self) -> Option<u64> {
        self.number
    }

    /// 패널을 특정 worktree의 PR로 연다. 7a 리뷰에서 헤더를 씨딩하고 세부 조회를
    /// 대기 상태로 둔다. `op`는 세부 조회의 staleness 게이트.
    pub fn open(
        &mut self,
        worktree: WorktreeId,
        number: u64,
        title: String,
        review_state: ReviewState,
        op: OpId,
    ) {
        self.worktree = Some(worktree);
        self.number = Some(number);
        self.title = title;
        self.review_state = Some(review_state);
        self.details = DetailsLoad::Loading;
        self.confirm = None;
        self.merging = false;
        self.outcome = None;
        self.latest_details_op = Some(op);
        // merge op 가드는 지우지 않는다 — 닫은/전환한 뒤 도착하는 늦은 결과가
        // "가드 없음 = 최신"으로 통과하지 못하게(diff 패널 `close`와 같은 이유).
    }

    pub fn close(&mut self) {
        self.worktree = None;
        self.number = None;
        self.title = String::new();
        self.review_state = None;
        self.details = DetailsLoad::Loading;
        self.confirm = None;
        self.merging = false;
        self.outcome = None;
        // op 가드는 남긴다(위 `open` 주석과 같은 이유).
    }

    /// 세부 조회를 새로 발급할 때(수동 새로고침). op를 갱신하고 로딩으로 되돌린다.
    pub fn begin_details(&mut self, op: OpId) {
        self.details = DetailsLoad::Loading;
        self.latest_details_op = Some(op);
    }

    /// 세부 결과를 받아들일지. **다른 worktree거나 오래된 op면 버린다.**
    pub fn accept_details(&self, worktree: &WorktreeId, op: OpId) -> bool {
        self.worktree.as_ref() == Some(worktree) && self.latest_details_op == Some(op)
    }

    pub fn apply_details(&mut self, details: PrDetails) {
        self.details = DetailsLoad::Loaded(details);
    }

    pub fn details(&self) -> Option<&PrDetails> {
        match &self.details {
            DetailsLoad::Loaded(d) => Some(d),
            DetailsLoad::Loading => None,
        }
    }

    pub fn mergeability(&self) -> Option<MergeabilityState> {
        self.details().map(|d| d.mergeability)
    }

    /// 확인 단계를 연다. **오직 머지가능성이 `Mergeable`일 때만** — 그 밖에서는
    /// 아무 일도 없다(비활성 버튼이 어떻게든 눌려도 파괴적 경로가 안 열리는
    /// 마지막 방어선). 반환값은 확인 단계가 열렸는지 여부.
    pub fn request_merge(&mut self) -> bool {
        if self.merging {
            return false;
        }
        if self.mergeability() != Some(MergeabilityState::Mergeable) {
            return false;
        }
        if self.confirm.is_none() {
            self.confirm = Some(MergeConfirm {
                method: DEFAULT_MERGE_METHOD,
                delete_branch: false,
            });
        }
        true
    }

    pub fn confirm(&self) -> Option<&MergeConfirm> {
        self.confirm.as_ref()
    }

    pub fn set_method(&mut self, method: MergeMethod) {
        if let Some(c) = &mut self.confirm {
            c.method = method;
        }
    }

    pub fn set_delete_branch(&mut self, delete: bool) {
        if let Some(c) = &mut self.confirm {
            c.delete_branch = delete;
        }
    }

    pub fn cancel_merge(&mut self) {
        self.confirm = None;
    }

    /// **파괴적 머지를 확정한다.** 확인 단계(`confirm`)가 열려 있을 때만 방식을
    /// 돌려준다 — `None`이면 아무도 확인하지 않았다는 뜻이고, 그때는 머지를 절대
    /// 발급하지 않는다(**원클릭 파괴 방지의 유일한 게이트**). 확정 시 `merging`을
    /// 세우고 확인 단계를 닫는다. 이미 진행 중이면 중복 발급하지 않는다.
    pub fn confirm_merge(&mut self, op: OpId) -> Option<MergeConfirm> {
        if self.merging {
            return None;
        }
        let confirm = self.confirm.take()?;
        self.merging = true;
        self.outcome = None;
        self.latest_merge_op = Some(op);
        Some(confirm)
    }

    /// 머지 결과를 받아들일지. 세부와 같은 staleness 규율.
    pub fn accept_merge(&self, worktree: &WorktreeId, op: OpId) -> bool {
        self.worktree.as_ref() == Some(worktree) && self.latest_merge_op == Some(op)
    }

    pub fn apply_merge(&mut self, display: MergeResultDisplay) {
        self.merging = false;
        self.outcome = Some(display);
    }

    pub fn is_merging(&self) -> bool {
        self.merging
    }

    pub fn outcome(&self) -> Option<&MergeResultDisplay> {
        self.outcome.as_ref()
    }

    /// 헤더 한 줄: "<state> · <title>". 상태 텍스트/색은 사람 눈이지만 문구 조립은
    /// 여기서 한다(제목이 사라지지 않게).
    pub fn header_line(&self) -> String {
        let state = match self.review_state {
            Some(ReviewState::Open) => "open",
            Some(ReviewState::Merged) => "merged",
            Some(ReviewState::Closed) => "closed",
            Some(ReviewState::Draft) => "draft",
            None => "?",
        };
        format!("{state} · {}", self.title)
    }
}

// ---- view (픽셀·상호작용은 사람 눈; 로직은 `forge_ui`가 검사) ----

pub fn view(state: &PrPanelState) -> Option<Element<'_, Message>> {
    if !state.is_open() {
        return None;
    }
    let number = state.number()?;

    let header = row![
        text(format!("PR #{number}")).size(15).width(Length::Fill),
        button(text("refresh").size(11)).on_press(Message::PrPanelRefreshRequested),
        button(text("close").size(11)).on_press(Message::PrPanelClosed),
    ]
    .spacing(6);

    let mut body = column![header, text(state.header_line()).size(12)]
        .spacing(8)
        .padding(12);

    // 세부: 머지가능성은 아래 머지 영역이 그리고, 여기선 리뷰·코멘트 요약을 그린다.
    // **Unavailable은 색으로도 구별**한다 — "없음"과 다른 값이다.
    match state.details() {
        None => {
            body = body.push(text("Loading PR details…").size(12).color(MUTED));
        }
        Some(details) => {
            body = body.push(reviews_widget(&forge_ui::reviews_line(&details.reviews)));
            body = body.push(comments_widget(&forge_ui::comments_line(&details.comments)));
        }
    }

    // 마지막 머지 결과(있으면). 세 갈래를 색으로 구별한다.
    if let Some(outcome) = state.outcome() {
        body = body.push(outcome_widget(outcome));
    }

    // 머지 영역: 확인 단계 ↔ Merge 버튼.
    body = body.push(merge_area(state));

    Some(
        container(scrollable(body))
            .width(Length::Fixed(WIDTH))
            .height(Length::Fill)
            .into(),
    )
}

fn reviews_widget(line: &ReviewsLine) -> Element<'static, Message> {
    match line {
        ReviewsLine::Summary(s) => text(format!("Reviews: {s}")).size(12).into(),
        // 일시 실패 → 경고색. 절대 "없음"으로 보이지 않는다.
        ReviewsLine::Unavailable(s) => text(format!("Reviews: {s}")).size(12).color(WARN).into(),
    }
}

fn comments_widget(line: &CommentsLine) -> Element<'static, Message> {
    match line {
        CommentsLine::Summary(s) => text(format!("Comments: {s}")).size(12).into(),
        CommentsLine::Unavailable(s) => text(format!("Comments: {s}")).size(12).color(WARN).into(),
    }
}

fn outcome_widget(outcome: &MergeResultDisplay) -> Element<'static, Message> {
    let (label, color) = match outcome {
        MergeResultDisplay::Merged => ("Merged.".to_string(), OK),
        // 확정 거부 — 성공으로 안 읽힌다.
        MergeResultDisplay::Rejected(reason) => (format!("Not merged: {reason}"), BAD),
        // 일시 실패 — "거부됨"이 아니라 재시도.
        MergeResultDisplay::Unavailable(reason) => (format!("Merge failed: {reason}"), WARN),
    };
    text(label).size(12).color(color).into()
}

fn merge_area(state: &PrPanelState) -> Element<'_, Message> {
    if state.is_merging() {
        return text("Merging…").size(12).color(MUTED).into();
    }
    match state.confirm() {
        Some(confirm) => confirm_widget(state, confirm),
        None => merge_button_widget(state),
    }
}

/// 확인 단계 밖의 Merge 버튼. **머지가능성이 Mergeable일 때만 눌린다** — 그 밖은
/// on_press 없는(=비활성) 버튼 + 이유(죽은 버튼이 아니다).
fn merge_button_widget(state: &PrPanelState) -> Element<'_, Message> {
    match state.mergeability() {
        None => text("Checking mergeability…").size(12).color(MUTED).into(),
        Some(m) => match forge_ui::merge_button(m) {
            MergeButton::Enabled => button(text("Merge…").size(12))
                .on_press(Message::MergeRequested)
                .into(),
            // on_press 없는 버튼은 iced가 비활성으로 렌더한다 + 이유를 붙인다.
            MergeButton::Disabled(reason) => column![
                button(text("Merge").size(12)),
                text(reason).size(10).color(MUTED),
            ]
            .spacing(4)
            .into(),
        },
    }
}

/// 확인 단계. 파괴 경고 + 방식 선택(merge/squash/rebase) + delete-branch + 확정/취소.
fn confirm_widget<'a>(state: &'a PrPanelState, confirm: &MergeConfirm) -> Element<'a, Message> {
    let number = state.number().unwrap_or(0);
    let warning = text(format!(
        "Merge PR #{number} now? This writes to the base branch and cannot be easily undone."
    ))
    .size(12)
    .color(WARN);

    let methods = row![
        method_button("merge", MergeMethod::Merge, confirm.method),
        method_button("squash", MergeMethod::Squash, confirm.method),
        method_button("rebase", MergeMethod::Rebase, confirm.method),
    ]
    .spacing(6);

    let delete = checkbox(confirm.delete_branch)
        .label("Delete branch after merge")
        .on_toggle(Message::MergeDeleteBranchToggled)
        .text_size(12);

    let actions = row![
        button(text("Confirm merge").size(12)).on_press(Message::MergeConfirmed),
        button(text("Cancel").size(12)).on_press(Message::MergeCancelled),
    ]
    .spacing(6);

    column![
        warning,
        text("Method:").size(11).color(MUTED),
        methods,
        delete,
        actions,
    ]
    .spacing(6)
    .into()
}

/// 방식 버튼 하나. 선택된 것은 `[squash]`처럼 표시해 눈에 구별되게 한다.
fn method_button(
    label: &'static str,
    method: MergeMethod,
    selected: MergeMethod,
) -> Element<'static, Message> {
    let text_label = if method == selected {
        format!("[{label}]")
    } else {
        label.to_string()
    };
    button(text(text_label).size(11))
        .on_press(Message::MergeMethodSelected(method))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_forge::{CommentLookup, MergeRejection, ReviewThreadLookup};

    fn wt() -> WorktreeId {
        WorktreeId("/wt/a".to_string())
    }

    fn details(mergeability: MergeabilityState) -> PrDetails {
        PrDetails {
            mergeability,
            reviews: ReviewThreadLookup::Found(vec![]),
            comments: CommentLookup::Found(vec![]),
        }
    }

    /// 헬퍼: 열린 뒤 Mergeable 세부가 도착한 패널.
    fn opened_mergeable() -> PrPanelState {
        let mut p = PrPanelState::default();
        p.open(wt(), 7, "t".to_string(), ReviewState::Open, OpId(1));
        p.apply_details(details(MergeabilityState::Mergeable));
        p
    }

    /// **이 태스크의 심장: 파괴적 머지는 확인 없이 절대 발급되지 않는다.**
    /// 확인 단계(`confirm`)를 열지 않은 채 `confirm_merge`를 부르면 `None`이 나오고
    /// `merging`은 서지 않는다 — `update`가 `None`을 받으면 `merge_pr` 태스크를
    /// 만들지 않는다. `confirm_merge`가 `self.confirm`을 무시하고 무조건
    /// `Some`을 돌려주는 뮤턴트(원클릭 머지)는 이 단언을 깨야 한다.
    #[test]
    fn a_merge_is_never_confirmed_without_an_open_confirm_step() {
        let mut p = opened_mergeable();
        assert!(p.confirm().is_none(), "control: no confirm step yet");
        assert_eq!(
            p.confirm_merge(OpId(2)),
            None,
            "confirming with no open confirm step must not yield a merge"
        );
        assert!(
            !p.is_merging(),
            "no merge may be dispatched without an explicit confirm"
        );
    }

    /// 전체 확인 흐름: Merge 요청 → 확인 단계 열림(아직 머지 아님) → 확정 →
    /// 딱 한 번 방식이 돌아온다. `request_merge`는 파괴를 일으키지 않는다.
    #[test]
    fn the_full_confirm_flow_dispatches_the_merge_exactly_once() {
        let mut p = opened_mergeable();

        assert!(p.request_merge(), "Mergeable → confirm step opens");
        assert!(p.confirm().is_some(), "the confirm step is now open");
        assert!(
            !p.is_merging(),
            "opening the confirm step must NOT be a merge — that is the whole point of 7b"
        );

        // 사용자가 방식을 고른다.
        p.set_method(MergeMethod::Squash);
        assert_eq!(p.confirm().unwrap().method, MergeMethod::Squash);

        // 확정 — 딱 한 번 방식이 돌아온다.
        let confirm = p.confirm_merge(OpId(2)).expect("confirm yields the method");
        assert_eq!(confirm.method, MergeMethod::Squash);
        assert!(p.is_merging(), "confirming sets merge in flight");

        // 두 번째 확정은 중복 발급하지 않는다(진행 중).
        assert_eq!(p.confirm_merge(OpId(3)), None);
    }

    /// **Merge 버튼은 Mergeable일 때만 확인 단계를 연다.** Blocked/Conflicting/
    /// Unknown에서 `request_merge`는 아무 일도 하지 않는다 — 비활성 버튼이 어떻게든
    /// 눌려도 파괴 경로가 안 열린다.
    #[test]
    fn request_merge_only_opens_the_confirm_step_when_mergeable() {
        for state in [
            MergeabilityState::Blocked,
            MergeabilityState::Conflicting,
            MergeabilityState::Unknown,
        ] {
            let mut p = PrPanelState::default();
            p.open(wt(), 7, "t".to_string(), ReviewState::Open, OpId(1));
            p.apply_details(details(state));
            assert!(
                !p.request_merge(),
                "{state:?} must not open the confirm step"
            );
            assert!(p.confirm().is_none());
            // 그리고 확정도 불가능하다(확인 단계가 없으니까).
            assert_eq!(p.confirm_merge(OpId(2)), None);
            assert!(!p.is_merging());
        }
    }

    /// 세부가 아직 로딩 중이면(머지가능성 미상) 요청은 열리지 않는다 — Unknown과
    /// 같은 취급(절대 Mergeable로 낙관하지 않는다).
    #[test]
    fn request_merge_does_nothing_while_details_are_loading() {
        let mut p = PrPanelState::default();
        p.open(wt(), 7, "t".to_string(), ReviewState::Open, OpId(1));
        assert_eq!(p.mergeability(), None);
        assert!(!p.request_merge());
        assert!(p.confirm().is_none());
    }

    /// 취소는 확인 단계를 닫고 머지를 발급하지 않는다.
    #[test]
    fn cancelling_the_confirm_step_dispatches_no_merge() {
        let mut p = opened_mergeable();
        p.request_merge();
        p.cancel_merge();
        assert!(p.confirm().is_none());
        assert_eq!(p.confirm_merge(OpId(2)), None);
        assert!(!p.is_merging());
    }

    /// 머지 결과 세 갈래가 패널 상태에 구별되어 남는다(Rejected/Unavailable이
    /// Merged로 안 뭉개진다).
    #[test]
    fn the_three_merge_outcomes_land_distinctly() {
        for (display, is_merged) in [
            (MergeResultDisplay::Merged, true),
            (
                MergeResultDisplay::Rejected(forge_ui::rejection_text(MergeRejection::Conflict)),
                false,
            ),
            (MergeResultDisplay::Unavailable("net — retry".into()), false),
        ] {
            let mut p = opened_mergeable();
            p.request_merge();
            let _ = p.confirm_merge(OpId(2));
            p.apply_merge(display.clone());
            assert!(!p.is_merging(), "applying a result clears in-flight");
            assert_eq!(p.outcome(), Some(&display));
            assert_eq!(
                matches!(p.outcome(), Some(MergeResultDisplay::Merged)),
                is_merged
            );
        }
    }

    // ---- staleness ----

    /// 다른 worktree거나 오래된 op의 세부 결과는 버린다.
    #[test]
    fn a_stale_or_foreign_details_result_is_rejected() {
        let mut p = PrPanelState::default();
        p.open(wt(), 7, "t".into(), ReviewState::Open, OpId(1));
        p.begin_details(OpId(2));

        assert!(!p.accept_details(&wt(), OpId(1)), "older op is dropped");
        assert!(
            !p.accept_details(&WorktreeId("/wt/other".into()), OpId(2)),
            "a result for another worktree is dropped"
        );
        assert!(p.accept_details(&wt(), OpId(2)), "control: the latest is accepted");
    }

    /// 패널을 닫은 뒤(또는 다른 worktree로 옮긴 뒤) 도착하는 머지 결과는 버린다 —
    /// `close`가 op 가드를 남기지만 worktree가 없어져 매칭이 어긋난다.
    #[test]
    fn a_merge_result_after_close_is_dropped() {
        let mut p = opened_mergeable();
        p.request_merge();
        let _ = p.confirm_merge(OpId(2));
        p.close();
        assert!(
            !p.accept_merge(&wt(), OpId(2)),
            "a merge result arriving after close must not be applied"
        );
    }

    /// 헤더 라인이 상태와 제목을 담는다(제목이 사라지지 않는다).
    #[test]
    fn the_header_line_carries_state_and_title() {
        let mut p = PrPanelState::default();
        p.open(wt(), 7, "Fix the thing".into(), ReviewState::Open, OpId(1));
        let line = p.header_line();
        assert!(line.contains("open"));
        assert!(line.contains("Fix the thing"));
    }
}
