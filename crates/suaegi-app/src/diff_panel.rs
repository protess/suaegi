//! 오른쪽 diff 패널: worktree의 변경 목록과, 고른 파일 하나의 patch.
//!
//! **`row!`의 단순 분할이지 `pane_grid`가 아니다** — 리사이즈는 범위 밖이라
//! 사이드바와 같은 이유로 고정 폭 위젯이다.
//!
//! 렌더링은 patch **텍스트**에 줄 선두로 색만 입힌다. Orca를 따라 하지 않는다:
//! Orca는 양쪽 blob을 Monaco에 넘기는데 우리에겐 Monaco가 없으므로, 따라 하면
//! 없는 diff 알고리즘을 새로 쓰는 셈이라 **더 비싸다.** `file_diff`가 이미
//! 완성된 patch를 준다.

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Color, Element, Font, Length};

use suaegi_core::domain::WorktreeId;
use suaegi_git::compare::{ChangeStatus, ChangedFile, CompareHandle, CompareOutcome, FileDiff};

use crate::state::{DiffFailure, Message, OpId};

/// 패널 고정 폭. 사이드바와 같은 이유로 `row!` 레벨에서 못 박는다.
pub const WIDTH: f32 = 420.0;

/// 목록 영역의 상태. **`Cancelled`에 대응하는 변형이 없는 것이 의도다** —
/// 취소는 상태 전이가 아니라 "아무 일도 일어나지 않음"이다([`panel_state_for`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PanelState {
    #[default]
    Closed,
    Loading,
    Ready(Vec<ChangedFile>),
    /// 비교는 성공했는데 바뀐 파일이 없다. **`Failed`가 아니다.**
    Empty,
    /// 진짜 오류. 배너를 띄운다.
    Failed(String),
    /// 아래 셋은 오류가 아니라 **보여줄 상태다.** Task 1이 `CompareOutcome`으로
    /// 분류해 둔 것을 여기서 잃어버리면 그 분류가 헛일이 된다.
    NoMergeBase,
    UnbornHead,
    InvalidBase,
    /// 비교 출력 자체가 러너 상한을 넘었다(파일 하나의 patch가 아니라).
    TooLarge {
        limit: usize,
    },
}

/// 패널 하단 patch 영역의 상태.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PatchState {
    /// 아직 파일을 고르지 않았다.
    #[default]
    Idle,
    Loading,
    Loaded(FileDiff),
    Failed(String),
}

/// diff 패널의 전체 상태. **`AppState`에 필드 하나로 들어간다** — 여러 필드로
/// 흩으면 동시에 `state.rs`를 고치는 다른 작업과 충돌 면적이 그만큼 넓어진다.
#[derive(Debug, Default)]
pub struct DiffState {
    panel: PanelState,
    /// 지금 패널이 보여주는 worktree. 사이드바에서 다른 worktree의 토글을
    /// 누르면 여기가 바뀐다.
    worktree: Option<WorktreeId>,
    selected_file: Option<String>,
    patch: PatchState,
    /// 마지막으로 발급한 비교 요청. 이보다 오래된 응답은 버린다 — 느린 비교
    /// 하나가 나중에 도착해 새 결과를 덮으면 사용자는 옛 목록을 본다.
    latest_compare_op: Option<OpId>,
    latest_patch_op: Option<OpId>,
    /// 진행 중인 비교의 취소 손잡이. 패널을 닫으면 당긴다.
    cancel: Option<CompareHandle>,
}

/// `branch_compare` 결과 → 목록 상태.
///
/// **`None`은 "상태를 바꾸지 말라"는 뜻이고, 그 경우는 취소뿐이다.**
/// `Cancelled`가 `Ok`로 온다는 사실이 여기서 제일 중요하다: "`Ready`가 아니면
/// 실패"라고 쓰면 패널을 닫을 때마다 오류 배너가 뜬다 — 플랜이 명시적으로
/// 금지한 바로 그 동작이다. 취소는 사용자가 방금 한 행동이지 사고가 아니다.
pub fn panel_state_for(result: Result<CompareOutcome, DiffFailure>) -> Option<PanelState> {
    match result {
        Ok(CompareOutcome::Cancelled) => None,
        Ok(CompareOutcome::Ready(compare)) if compare.files.is_empty() => Some(PanelState::Empty),
        Ok(CompareOutcome::Ready(compare)) => Some(PanelState::Ready(compare.files)),
        Ok(CompareOutcome::NoMergeBase) => Some(PanelState::NoMergeBase),
        Ok(CompareOutcome::UnbornHead) => Some(PanelState::UnbornHead),
        Ok(CompareOutcome::InvalidBase) => Some(PanelState::InvalidBase),
        Err(DiffFailure::TooLarge { limit }) => Some(PanelState::TooLarge { limit }),
        Err(DiffFailure::Failed(message)) => Some(PanelState::Failed(message)),
    }
}

/// `file_diff` 결과 → patch 영역 상태. 이쪽은 취소가 없다(파일 patch는 git 호출
/// 하나라 취소 확인 지점이 없다) — 그래서 `Option`이 아니다.
pub fn patch_state_for(result: Result<FileDiff, String>) -> PatchState {
    match result {
        Ok(diff) => PatchState::Loaded(diff),
        Err(message) => PatchState::Failed(message),
    }
}

/// patch 한 줄의 종류. 색만 이걸로 정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Added,
    Removed,
    Hunk,
    /// `diff --git`, `index`, `+++`, `---`, `new file mode` 같은 머리말.
    Meta,
    Context,
}

/// **`+++`/`---`를 `+`/`-`보다 먼저 본다.** unified diff의 파일 머리말은
/// `+++ b/path`, `--- a/path`라서 선두 한 글자만 보면 추가/삭제 줄로 오인하고,
/// 그러면 모든 patch가 맨 위 두 줄을 초록/빨강으로 칠한 채 시작한다.
pub fn line_kind(line: &str) -> LineKind {
    if line.starts_with("+++") || line.starts_with("---") {
        return LineKind::Meta;
    }
    match line.as_bytes().first() {
        Some(b'+') => LineKind::Added,
        Some(b'-') => LineKind::Removed,
        Some(b'@') => LineKind::Hunk,
        _ if line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("similarity index")
            || line.starts_with("rename ")
            || line.starts_with("copy ")
            || line.starts_with("Binary files") =>
        {
            LineKind::Meta
        }
        _ => LineKind::Context,
    }
}

fn line_color(kind: LineKind) -> Color {
    match kind {
        LineKind::Added => Color::from_rgb8(0x2e, 0xa0, 0x43),
        LineKind::Removed => Color::from_rgb8(0xc0, 0x39, 0x2b),
        LineKind::Hunk => Color::from_rgb8(0x62, 0x6c, 0xd9),
        LineKind::Meta => Color::from_rgb8(0x88, 0x88, 0x88),
        LineKind::Context => Color::from_rgb8(0x33, 0x33, 0x33),
    }
}

/// 목록 행의 접두 글자. `Copied`가 `Renamed`와 **구별되어야 한다** —
/// Task 1이 파서를 고쳐 둘을 가른 이유가 사라진다.
pub fn status_glyph(status: &ChangeStatus) -> &'static str {
    match status {
        ChangeStatus::Added => "A",
        ChangeStatus::Modified => "M",
        ChangeStatus::Deleted => "D",
        ChangeStatus::Renamed { .. } => "R",
        ChangeStatus::Copied { .. } => "C",
        ChangeStatus::Other(_) => "?",
    }
}

/// 목록이 비었을 때/실패했을 때 사용자에게 보일 한 줄.
///
/// 순수 함수로 뽑은 이유는 **`Element`를 들여다볼 수 없기 때문이다** — 문구
/// 자체가 이 패널의 유일한 산출물인 상태들(`NoMergeBase` 등)에서는 이 매핑이
/// 곧 기능이다.
pub fn status_message(state: &PanelState) -> Option<String> {
    match state {
        PanelState::Closed | PanelState::Ready(_) => None,
        PanelState::Loading => Some("Comparing…".to_string()),
        PanelState::Empty => Some("No changes against the base branch.".to_string()),
        PanelState::NoMergeBase => {
            Some("No common ancestor with the base branch — nothing to compare.".to_string())
        }
        PanelState::UnbornHead => Some("This worktree has no commits yet.".to_string()),
        PanelState::InvalidBase => Some("The base branch could not be resolved.".to_string()),
        PanelState::TooLarge { limit } => Some(format!(
            "Too many changes to list (over {} MB of output).",
            limit / (1024 * 1024)
        )),
        PanelState::Failed(message) => Some(format!("Could not compare: {message}")),
    }
}

impl DiffState {
    pub fn is_open(&self) -> bool {
        self.panel != PanelState::Closed
    }

    pub fn panel(&self) -> &PanelState {
        &self.panel
    }

    pub fn patch(&self) -> &PatchState {
        &self.patch
    }

    pub fn worktree(&self) -> Option<&WorktreeId> {
        self.worktree.as_ref()
    }

    pub fn selected_file(&self) -> Option<&str> {
        self.selected_file.as_deref()
    }

    /// 비교를 시작한다. 이전 비교가 진행 중이면 **먼저 취소한다** — 안 그러면
    /// 옛 비교가 끝까지 돌며 최대 210초 동안 git을 붙든다.
    pub fn begin_compare(&mut self, worktree: WorktreeId, op: OpId) -> CompareHandle {
        self.cancel_in_flight();
        self.panel = PanelState::Loading;
        self.worktree = Some(worktree);
        self.selected_file = None;
        self.patch = PatchState::Idle;
        self.latest_compare_op = Some(op);
        let handle = CompareHandle::new();
        self.cancel = Some(handle.clone());
        handle
    }

    /// 패널을 닫는다. 진행 중인 비교를 취소하지만 **오류로 만들지 않는다** —
    /// 뒤늦게 도착하는 `Cancelled`는 [`panel_state_for`]가 `None`으로 흘린다.
    pub fn close(&mut self) {
        self.cancel_in_flight();
        self.panel = PanelState::Closed;
        self.worktree = None;
        self.selected_file = None;
        self.patch = PatchState::Idle;
        // op 가드는 지우지 않는다. 지우면 방금 취소한 요청의 늦은 응답이
        // "가드가 없다 = 최신이다"로 통과해 닫힌 패널을 다시 연다.
    }

    fn cancel_in_flight(&mut self) {
        if let Some(handle) = self.cancel.take() {
            handle.stop_after_current_call();
        }
    }

    /// 비교 결과를 받아들일지. **오래된 `op`는 버린다.**
    pub fn accept_compare(&mut self, worktree: &WorktreeId, op: OpId) -> bool {
        if self.worktree.as_ref() != Some(worktree) {
            return false;
        }
        self.latest_compare_op == Some(op)
    }

    pub fn apply_compare(&mut self, state: PanelState) {
        self.panel = state;
        self.cancel = None;
    }

    pub fn begin_patch(&mut self, path: String, op: OpId) {
        self.selected_file = Some(path);
        self.patch = PatchState::Loading;
        self.latest_patch_op = Some(op);
    }

    /// patch 결과를 받아들일지.
    ///
    /// **경로는 보지 않는다 — `OpId`가 이미 경로를 함의한다.** `begin_patch`가
    /// `selected_file`과 `latest_patch_op`를 **같이** 세우므로, op가 최신이면
    /// 그 op를 발급한 요청의 경로가 곧 `selected_file`이다. 경로를 또 비교하는
    /// 코드를 뒀다가 mutation으로 걸렀다: 지워도 아무 테스트가 죽지 않았고,
    /// op가 맞는데 경로가 틀린 입력은 만들 수가 없었다(= 도달 불가).
    ///
    /// **worktree는 다르다. 그건 진짜로 필요하다**: `begin_compare`는
    /// `latest_patch_op`을 건드리지 않으므로, 파일을 고른 뒤 패널을 다른
    /// worktree로 옮기면 앞 worktree의 patch가 최신 op를 그대로 달고 도착해
    /// **엉뚱한 worktree의 patch가 화면에 뜬다.**
    pub fn accept_patch(&mut self, worktree: &WorktreeId, op: OpId) -> bool {
        if self.worktree.as_ref() != Some(worktree) {
            return false;
        }
        self.latest_patch_op == Some(op)
    }

    pub fn apply_patch(&mut self, state: PatchState) {
        self.patch = state;
    }

    /// 목록에서 고른 파일의 `ChangeStatus`. `file_diff`가 이걸 요구한다 —
    /// 스니핑할 리비전이 상태에 달렸고 `Other(c)`는 git을 부르지 않고 끝내야
    /// 하기 때문이다.
    pub fn status_of(&self, path: &str) -> Option<ChangeStatus> {
        let PanelState::Ready(files) = &self.panel else {
            return None;
        };
        files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.status.clone())
    }
}

// ---- view ----

pub fn view(state: &DiffState) -> Option<Element<'_, Message>> {
    if !state.is_open() {
        return None;
    }
    let worktree = state.worktree()?.clone();

    let header = row![
        text("Changes").size(15).width(Length::Fill),
        button(text("close").size(12)).on_press(Message::DiffCancelled { worktree }),
    ]
    .spacing(6);

    let mut body = column![header].spacing(8).padding(12);

    if let Some(message) = status_message(state.panel()) {
        body = body.push(text(message).size(12));
    }
    if let PanelState::Ready(files) = state.panel() {
        body = body.push(file_list(state, files));
    }
    body = body.push(patch_view(state.patch()));

    Some(
        container(body)
            .width(Length::Fixed(WIDTH))
            .height(Length::Fill)
            .into(),
    )
}

fn file_list<'a>(state: &'a DiffState, files: &'a [ChangedFile]) -> Element<'a, Message> {
    let worktree = state.worktree().cloned();
    let mut list = column![].spacing(2);
    for file in files {
        let selected = state.selected_file() == Some(file.path.as_str());
        let marker = if selected { ">" } else { " " };
        let label = format!(
            "{marker} {} {}{}",
            status_glyph(&file.status),
            file.path,
            counts_suffix(file)
        );
        // **요청 메시지는 `OpId`를 나르지 않는다.** 뷰는 `&self`라 발급할 수
        // 없다 — `next_op()`은 `&mut self`다. 발급은 `update`가 하고, `OpId`는
        // 응답(`FileDiffLoaded`)에 실려 돌아온다. 저장소의 기존 쌍
        // (`CreateWorktreeSubmitted` → `WorktreeCreated`)과 같은 모양이다.
        let press = worktree.clone().map(|worktree| Message::FileDiffRequested {
            worktree,
            path: file.path.clone(),
        });
        list = list.push(
            button(text(label).size(12))
                .on_press_maybe(press)
                .width(Length::Fill),
        );
    }
    scrollable(list).height(Length::FillPortion(2)).into()
}

fn counts_suffix(file: &ChangedFile) -> String {
    match (file.additions, file.deletions) {
        (Some(a), Some(d)) => format!("  +{a} -{d}"),
        // `-`(바이너리)는 숫자가 없다. 0으로 꾸며내지 않는다.
        _ => String::new(),
    }
}

fn patch_view(state: &PatchState) -> Element<'_, Message> {
    let body: Element<'_, Message> = match state {
        PatchState::Idle => text("Select a file to see its patch.").size(12).into(),
        PatchState::Loading => text("Loading patch…").size(12).into(),
        PatchState::Failed(message) => text(format!("Could not load patch: {message}"))
            .size(12)
            .into(),
        PatchState::Loaded(FileDiff::Binary) => text("Binary file — no text diff.").size(12).into(),
        PatchState::Loaded(FileDiff::TooLarge { limit }) => text(format!(
            "Patch is too large to display (over {} MB).",
            limit / (1024 * 1024)
        ))
        .size(12)
        .into(),
        // **추측하지 않는다** — 타입 변경·미병합 등은 무엇을 보여줘야 하는지 모른다.
        PatchState::Loaded(FileDiff::NonRenderable(code)) => {
            text(format!("No preview for change type '{code}'."))
                .size(12)
                .into()
        }
        PatchState::Loaded(FileDiff::Patch(patch)) => patch_lines(patch),
    };
    scrollable(body).height(Length::FillPortion(3)).into()
}

fn patch_lines(patch: &str) -> Element<'_, Message> {
    let mut lines = column![].spacing(0);
    for line in patch.lines() {
        lines = lines.push(
            text(line.to_string())
                .size(11)
                .font(Font::MONOSPACE)
                .color(line_color(line_kind(line))),
        );
    }
    lines.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_git::compare::BranchCompare;

    fn compare(files: Vec<ChangedFile>) -> CompareOutcome {
        CompareOutcome::Ready(BranchCompare {
            merge_base: "abc123".to_string(),
            ahead_count: 1,
            files,
        })
    }

    fn changed(path: &str, status: ChangeStatus) -> ChangedFile {
        ChangedFile {
            path: path.to_string(),
            status,
            additions: Some(1),
            deletions: Some(0),
        }
    }

    /// **이 태스크에서 제일 중요한 단언이다.** 취소는 사용자가 패널을 닫은
    /// 결과이고, 그때 오류 배너가 뜨면 정상 조작이 사고처럼 보인다.
    /// 그리고 `Cancelled`는 `Err`가 아니라 `Ok`로 온다 — "`Ready`가 아니면
    /// 실패"라고 쓰기 쉬운 자리라서 명시적으로 고정한다.
    #[test]
    fn cancellation_changes_nothing_and_is_never_an_error() {
        assert_eq!(panel_state_for(Ok(CompareOutcome::Cancelled)), None);
    }

    /// 취소가 아닌 결과는 전부 상태를 만든다 — 그리고 **어떤 상태인지까지**
    /// 고정한다.
    ///
    /// 처음엔 `.is_some()`만 봤다. 그건 "`Ok`인지만 보고 안을 열지 않는" 모양이라
    /// **변형끼리 뒤바뀌어도 통과한다** — 리뷰에서 `TooLarge` → `Failed` 치환이
    /// 살아남아 드러났다. 표로 정확한 매핑을 못 박으면 그 부류가 통째로 죽는다.
    #[test]
    fn every_non_cancelled_outcome_maps_to_its_exact_state() {
        let file = changed("a.txt", ChangeStatus::Added);
        let cases = vec![
            (
                Ok(compare(vec![file.clone()])),
                PanelState::Ready(vec![file]),
            ),
            (Ok(compare(vec![])), PanelState::Empty),
            (Ok(CompareOutcome::NoMergeBase), PanelState::NoMergeBase),
            (Ok(CompareOutcome::UnbornHead), PanelState::UnbornHead),
            (Ok(CompareOutcome::InvalidBase), PanelState::InvalidBase),
            (
                Err(DiffFailure::Failed("boom".into())),
                PanelState::Failed("boom".into()),
            ),
            (
                Err(DiffFailure::TooLarge {
                    limit: 6 * 1024 * 1024,
                }),
                PanelState::TooLarge {
                    limit: 6 * 1024 * 1024,
                },
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                panel_state_for(input.clone()),
                Some(expected),
                "wrong panel state for {input:?}"
            );
        }
    }

    /// `TooLarge`의 문구는 **바이트를 MB로 바꿔서** 보여줘야 한다. 변환을
    /// 빠뜨리면 화면에 "over 6291456 MB"가 뜬다. `git_tasks`가 6MB 초과 비교마다
    /// 이 상태를 내므로 도달 가능하다.
    #[test]
    fn the_too_large_message_reports_megabytes_not_bytes() {
        let message = status_message(&PanelState::TooLarge {
            limit: 6 * 1024 * 1024,
        })
        .unwrap();
        assert!(
            message.contains("6 MB"),
            "the limit must be shown in MB: {message}"
        );
        assert!(
            !message.contains("6291456"),
            "raw bytes leaked into the message: {message}"
        );
    }

    /// 성공했지만 변경이 없는 것은 **실패가 아니다.** 둘을 뭉개면 "작업이 아직
    /// 아무것도 안 바꿨다"가 오류로 보인다.
    #[test]
    fn an_empty_successful_compare_is_empty_not_failed() {
        assert_eq!(
            panel_state_for(Ok(compare(vec![]))),
            Some(PanelState::Empty)
        );
    }

    /// Task 1이 셋으로 분류해 둔 것을 패널이 하나로 뭉개면 그 분류가 헛일이다.
    /// 셋의 **문구가 서로 달라야** 사용자가 무엇을 고쳐야 하는지 안다.
    #[test]
    fn the_three_classified_outcomes_stay_distinguishable_to_the_user() {
        let no_base = panel_state_for(Ok(CompareOutcome::NoMergeBase)).unwrap();
        let unborn = panel_state_for(Ok(CompareOutcome::UnbornHead)).unwrap();
        let invalid = panel_state_for(Ok(CompareOutcome::InvalidBase)).unwrap();
        assert_ne!(no_base, unborn);
        assert_ne!(unborn, invalid);
        assert_ne!(no_base, invalid);

        let messages = [&no_base, &unborn, &invalid].map(|s| status_message(s).unwrap());
        assert_ne!(messages[0], messages[1]);
        assert_ne!(messages[1], messages[2]);
        assert_ne!(messages[0], messages[2]);
        // 그리고 **오류 문구를 쓰지 않는다** — 설정 문제지 사고가 아니다.
        for message in &messages {
            assert!(
                !message.contains("Could not compare"),
                "a classified outcome must not read as a failure: {message}"
            );
        }
    }

    /// `+++`/`---`가 `+`/`-`보다 먼저 판정돼야 한다. 아니면 모든 patch가 머리말
    /// 두 줄을 초록/빨강으로 칠한 채 시작한다.
    #[test]
    fn file_headers_are_meta_not_additions_or_removals() {
        assert_eq!(line_kind("+++ b/src/main.rs"), LineKind::Meta);
        assert_eq!(line_kind("--- a/src/main.rs"), LineKind::Meta);
        // 대조군: 진짜 추가/삭제 줄은 그대로 판정된다.
        assert_eq!(line_kind("+let x = 1;"), LineKind::Added);
        assert_eq!(line_kind("-let x = 0;"), LineKind::Removed);
        assert_eq!(line_kind("@@ -1,3 +1,4 @@"), LineKind::Hunk);
        assert_eq!(line_kind(" unchanged"), LineKind::Context);
        assert_eq!(line_kind("diff --git a/x b/x"), LineKind::Meta);
    }

    /// 빈 줄은 unified diff에서 **문맥 줄**이다(선두 공백이 잘린 형태로도 온다).
    /// 인덱싱으로 첫 글자를 꺼내면 여기서 패닉한다.
    #[test]
    fn an_empty_line_is_context_and_does_not_panic() {
        assert_eq!(line_kind(""), LineKind::Context);
    }

    /// Task 1이 `C`를 `R`과 가르려고 파서를 고쳤다. 화면에서 같은 글자로
    /// 보이면 그 작업이 사용자에게 도달하지 않는다.
    #[test]
    fn copied_is_visually_distinct_from_renamed() {
        let renamed = status_glyph(&ChangeStatus::Renamed { from: "a".into() });
        let copied = status_glyph(&ChangeStatus::Copied { from: "a".into() });
        assert_ne!(renamed, copied);
    }

    /// 바이너리 파일은 numstat이 `-`를 주므로 카운트가 `None`이다.
    /// **0으로 꾸며내면** 사용자는 "변경 없음"으로 읽는다.
    #[test]
    fn binary_counts_are_omitted_rather_than_shown_as_zero() {
        let binary = ChangedFile {
            path: "logo.png".into(),
            status: ChangeStatus::Modified,
            additions: None,
            deletions: None,
        };
        assert_eq!(counts_suffix(&binary), "");
        let text_file = changed("a.txt", ChangeStatus::Modified);
        assert!(counts_suffix(&text_file).contains("+1"));
    }

    // ---- staleness ----

    fn worktree(name: &str) -> WorktreeId {
        WorktreeId(name.to_string())
    }

    #[test]
    fn a_stale_compare_result_does_not_overwrite_a_newer_one() {
        let mut state = DiffState::default();
        let wt = worktree("/wt/a");
        state.begin_compare(wt.clone(), OpId(1));
        state.begin_compare(wt.clone(), OpId(2));

        assert!(
            !state.accept_compare(&wt, OpId(1)),
            "the older request must be dropped"
        );
        // 대조군: 최신 요청은 받아들인다.
        assert!(state.accept_compare(&wt, OpId(2)));
    }

    /// 다른 worktree의 결과는 `OpId`가 최신이어도 받으면 안 된다 — 패널은
    /// 한 번에 하나의 worktree를 보여준다.
    #[test]
    fn a_result_for_another_worktree_is_dropped() {
        let mut state = DiffState::default();
        state.begin_compare(worktree("/wt/a"), OpId(1));
        assert!(!state.accept_compare(&worktree("/wt/b"), OpId(1)));
        assert!(state.accept_compare(&worktree("/wt/a"), OpId(1)));
    }

    /// 같은 패널에서 파일을 바꿔 고르면 앞 파일의 patch는 버려야 한다.
    /// 거르는 것은 **`OpId`다** — `begin_patch`가 경로와 op를 같이 세우므로
    /// op 하나로 충분하다.
    #[test]
    fn a_patch_for_a_previously_selected_file_is_dropped() {
        let mut state = DiffState::default();
        let wt = worktree("/wt/a");
        state.begin_compare(wt.clone(), OpId(1));
        state.begin_patch("a.txt".into(), OpId(2));
        state.begin_patch("b.txt".into(), OpId(3));

        assert!(!state.accept_patch(&wt, OpId(2)));
        assert!(state.accept_patch(&wt, OpId(3)));
    }

    /// **패널을 다른 worktree로 옮긴 뒤 앞 worktree의 patch가 도착하는 경우.**
    /// `begin_compare`는 `latest_patch_op`을 초기화하지 않으므로 op는 여전히
    /// 최신이다 — worktree를 보지 않으면 엉뚱한 worktree의 patch가 뜬다.
    /// 프로덕션에서 도달 가능하다: 파일을 고른 직후 사이드바에서 다른
    /// worktree의 diff를 열면 된다.
    #[test]
    fn a_patch_from_the_previous_worktree_is_dropped_after_switching() {
        let mut state = DiffState::default();
        let first = worktree("/wt/a");
        state.begin_compare(first.clone(), OpId(1));
        state.begin_patch("a.txt".into(), OpId(2));
        // 사용자가 다른 worktree의 diff를 연다.
        state.begin_compare(worktree("/wt/b"), OpId(3));

        assert!(
            !state.accept_patch(&first, OpId(2)),
            "a patch from the worktree we navigated away from must not be shown"
        );
    }

    /// 닫기가 취소를 부르는지. `CompareHandle`은 `Arc<AtomicBool>`이라
    /// 복제본으로 관찰할 수 있다.
    #[test]
    fn closing_the_panel_cancels_the_in_flight_compare() {
        let mut state = DiffState::default();
        let handle = state.begin_compare(worktree("/wt/a"), OpId(1));
        assert!(!handle.is_stopped(), "control: not cancelled before close");

        state.close();
        assert!(
            handle.is_stopped(),
            "closing must cancel the running compare"
        );
        assert_eq!(*state.panel(), PanelState::Closed);
    }

    /// 새 비교를 시작하는 것도 앞 비교를 취소해야 한다. 안 하면 옛 비교가
    /// 최대 210초 동안 git을 붙들고 돈다.
    #[test]
    fn starting_a_new_compare_cancels_the_previous_one() {
        let mut state = DiffState::default();
        let first = state.begin_compare(worktree("/wt/a"), OpId(1));
        let second = state.begin_compare(worktree("/wt/b"), OpId(2));
        assert!(first.is_stopped());
        assert!(!second.is_stopped(), "control: the new one still runs");
    }

    /// 닫은 뒤 도착하는 늦은 응답이 패널을 **다시 열면 안 된다.**
    #[test]
    fn a_late_result_cannot_reopen_a_closed_panel() {
        let mut state = DiffState::default();
        let wt = worktree("/wt/a");
        state.begin_compare(wt.clone(), OpId(1));
        state.close();
        assert!(
            !state.accept_compare(&wt, OpId(1)),
            "a result arriving after close must be dropped"
        );
        assert_eq!(*state.panel(), PanelState::Closed);
    }

    /// `file_diff`는 `ChangeStatus`를 요구한다. 목록에서 그걸 꺼내오지 못하면
    /// 패치를 요청할 수 없다.
    #[test]
    fn the_status_of_a_listed_file_is_recoverable_for_the_patch_request() {
        let mut state = DiffState::default();
        state.begin_compare(worktree("/wt/a"), OpId(1));
        state.apply_compare(PanelState::Ready(vec![changed(
            "a.txt",
            ChangeStatus::Deleted,
        )]));
        assert_eq!(state.status_of("a.txt"), Some(ChangeStatus::Deleted));
        assert_eq!(state.status_of("missing.txt"), None);
    }

    // ---- 실제 `AppState::update` 디스패치를 태우는 배선 테스트 ----
    //
    // 위 테스트들은 순수 함수와 `DiffState`의 불변식만 본다. `update`가 그
    // 함수를 **부르는 것을 빠뜨리는** mutation은 그걸로 못 잡는다 — 여기서
    // 실제 메시지를 흘려 배선 자체를 태운다.

    mod wiring {
        use super::*;
        use crate::state::{AppState, Message};
        use std::path::PathBuf;
        use suaegi_core::domain::{Repo, RepoId};
        use suaegi_git::worktree::WorktreeEntry;

        /// repo 하나 + worktree 하나가 등록된 상태. `request_diff`가
        /// `find_worktree`/`repo_by_id`를 타므로 둘 다 필요하다.
        fn state_with_worktree() -> (AppState, WorktreeId) {
            let mut state = AppState::fresh();
            let repo = Repo {
                id: RepoId("/tmp/repo".into()),
                path: PathBuf::from("/tmp/repo"),
                display_name: "repo".into(),
                worktree_base_ref: Some("main".into()),
            };
            state.upsert_repo(repo.clone());
            let entry = WorktreeEntry {
                path: PathBuf::from("/tmp/wt/feat"),
                branch: Some("feat".into()),
                head: None,
                is_main: false,
            };
            state.note_list_issued(repo.id.clone(), OpId(1));
            state.apply_authoritative_listing(repo.id, OpId(1), vec![entry.clone()]);
            let id = crate::state::worktree_id_for(&entry.path);
            (state, id)
        }

        /// **패널을 닫는 것은 오류가 아니다.** 배너도 `last_error`도 남지 않아야
        /// 한다 — 사용자가 방금 한 조작이 사고처럼 보이면 안 된다.
        #[test]
        fn cancelling_closes_the_panel_without_recording_an_error() {
            let (mut state, wt) = state_with_worktree();
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            assert!(state.diff().is_open(), "control: the panel opened");

            let _ = state.update(Message::DiffCancelled {
                worktree: wt.clone(),
            });
            assert!(!state.diff().is_open());
            assert!(
                state.last_error().is_none(),
                "closing the panel must not surface an error"
            );
        }

        /// 그리고 **뒤늦게 도착하는 `Cancelled`도** 오류가 아니다. 이게
        /// 실제로 벌어지는 순서다: 닫기 → 취소 → 잠시 뒤 `Cancelled` 도착.
        #[test]
        fn a_late_cancelled_result_neither_reopens_the_panel_nor_errors() {
            let (mut state, wt) = state_with_worktree();
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            let _ = state.update(Message::DiffCancelled {
                worktree: wt.clone(),
            });
            let _ = state.update(Message::DiffLoaded {
                worktree: wt.clone(),
                op: OpId(1),
                result: Ok(CompareOutcome::Cancelled),
            });
            assert!(!state.diff().is_open(), "a late Cancelled must not reopen");
            assert!(state.last_error().is_none());
        }

        /// **취소 결과가 staleness 가드를 통과했을 때** 무슨 일이 나는지.
        ///
        /// 위 `a_late_cancelled_result_...`는 사실 `accept_compare`가 먼저
        /// 걸러서 통과한다 — `panel_state_for`의 `Cancelled` 갈래를 **한 번도
        /// 타지 않는다.** (mutation으로 확인: `Cancelled => Failed`로 바꿔도 그
        /// 테스트는 통과한다.) 그래서 가드를 통과하는 경우를 따로 만든다:
        /// 패널이 열린 채, 같은 worktree, 같은 `op`로 취소가 도착한다.
        ///
        /// 지금 이 경로로 들어오는 프로덕션 흐름은 없다(취소는 늘 `close`나
        /// 새 `begin_compare`를 동반하고 둘 다 가드를 어긋나게 한다). 하지만
        /// 가드가 유일한 방어라면 가드가 틀리는 날 배너가 뜬다 — 이 갈래가
        /// 마지막 방어선이고, 그래서 값이 있다.
        #[test]
        fn an_accepted_cancelled_result_leaves_the_panel_alone() {
            let (mut state, wt) = state_with_worktree();
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            assert_eq!(*state.diff().panel(), PanelState::Loading);

            // 가드를 통과하는 op/worktree 그대로 취소가 도착한다.
            let _ = state.update(Message::DiffLoaded {
                worktree: wt,
                op: OpId(1),
                result: Ok(CompareOutcome::Cancelled),
            });
            assert_eq!(
                *state.diff().panel(),
                PanelState::Loading,
                "an accepted Cancelled must change nothing — least of all to an error"
            );
            assert!(state.last_error().is_none());
        }

        /// 토글: 같은 worktree의 버튼을 다시 누르면 닫힌다.
        #[test]
        fn the_toggle_closes_the_panel_for_the_same_worktree() {
            let (mut state, wt) = state_with_worktree();
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            assert!(state.diff().is_open());
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            assert!(!state.diff().is_open(), "the second press must close it");
        }

        /// 느린 비교가 나중에 도착해 새 결과를 덮으면 안 된다 —
        /// **실제 디스패치를 통과시켜** `accept_compare` 호출 자체를 태운다.
        #[test]
        fn a_stale_compare_result_is_ignored_by_the_real_dispatch() {
            let (mut state, wt) = state_with_worktree();
            // **토글이므로 여는 누름만 op를 발급한다**: 열기(op 1) → 닫기(발급
            // 없음) → 열기(op 2). 세 번 눌렀다고 op가 3이 되지 않는다.
            for _ in 0..3 {
                let _ = state.update(Message::DiffRequested {
                    worktree: wt.clone(),
                });
            }
            assert!(
                state.diff().is_open(),
                "control: the third press reopened it"
            );

            // 최신 결과(op 2)를 먼저 반영한다.
            let fresh = compare(vec![changed("new.txt", ChangeStatus::Added)]);
            let _ = state.update(Message::DiffLoaded {
                worktree: wt.clone(),
                op: OpId(2),
                result: Ok(fresh),
            });
            assert!(matches!(state.diff().panel(), PanelState::Ready(files)
                if files.iter().any(|f| f.path == "new.txt")));

            // 그리고 **오래된** 결과가 뒤늦게 도착한다.
            let stale = compare(vec![changed("old.txt", ChangeStatus::Added)]);
            let _ = state.update(Message::DiffLoaded {
                worktree: wt.clone(),
                op: OpId(1),
                result: Ok(stale),
            });
            assert!(
                matches!(state.diff().panel(), PanelState::Ready(files)
                    if files.iter().any(|f| f.path == "new.txt")),
                "the stale result overwrote the newer one: {:?}",
                state.diff().panel()
            );
        }

        /// 대조군 겸 배선 확인: 진짜 오류는 실제로 패널에 도달한다.
        /// 위 테스트들이 "무조건 무시"로 퇴화하지 않았음을 고정한다.
        #[test]
        fn a_real_failure_reaches_the_panel_through_the_real_dispatch() {
            let (mut state, wt) = state_with_worktree();
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            let _ = state.update(Message::DiffLoaded {
                worktree: wt.clone(),
                op: OpId(1),
                result: Err(DiffFailure::Failed("git exploded".into())),
            });
            assert!(
                matches!(state.diff().panel(), PanelState::Failed(m) if m.contains("git exploded")),
                "{:?}",
                state.diff().panel()
            );
        }

        /// **patch 결과가 실제로 화면 상태에 도달하는지.** mutation으로 구멍을
        /// 찾았다: `apply_patch` 호출을 통째로 지워도 아무 테스트가 죽지 않았다 —
        /// staleness와 "무시" 쪽만 덮고 **성공 경로를 아무도 안 봤다.**
        #[test]
        fn a_loaded_patch_reaches_the_panel_through_the_real_dispatch() {
            let (mut state, wt) = state_with_worktree();
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            // 파일 하나가 목록에 있어야 patch를 요청할 수 있다.
            let _ = state.update(Message::DiffLoaded {
                worktree: wt.clone(),
                op: OpId(1),
                result: Ok(compare(vec![changed("a.txt", ChangeStatus::Modified)])),
            });
            let _ = state.update(Message::FileDiffRequested {
                worktree: wt.clone(),
                path: "a.txt".into(),
            });
            assert_eq!(
                *state.diff().patch(),
                PatchState::Loading,
                "control: requesting a patch must show it is loading"
            );

            let _ = state.update(Message::FileDiffLoaded {
                worktree: wt,
                path: "a.txt".into(),
                op: OpId(2),
                result: Ok(FileDiff::Patch("@@ -1 +1 @@\n-a\n+b\n".into())),
            });
            assert!(
                matches!(state.diff().patch(), PatchState::Loaded(FileDiff::Patch(p)) if p.contains("+b")),
                "the loaded patch never reached the panel: {:?}",
                state.diff().patch()
            );
        }

        /// 그리고 바이너리·과대·렌더 불가도 **patch 영역에 그대로 도달해야 한다.**
        /// 셋을 빈 patch로 뭉개면 화면이 "변경 없음"으로 보인다.
        #[test]
        fn binary_and_non_renderable_results_reach_the_panel_distinctly() {
            for expected in [
                FileDiff::Binary,
                FileDiff::TooLarge {
                    limit: 6 * 1024 * 1024,
                },
                FileDiff::NonRenderable('T'),
            ] {
                let (mut state, wt) = state_with_worktree();
                let _ = state.update(Message::DiffRequested {
                    worktree: wt.clone(),
                });
                let _ = state.update(Message::DiffLoaded {
                    worktree: wt.clone(),
                    op: OpId(1),
                    result: Ok(compare(vec![changed("a.txt", ChangeStatus::Modified)])),
                });
                let _ = state.update(Message::FileDiffRequested {
                    worktree: wt.clone(),
                    path: "a.txt".into(),
                });
                let _ = state.update(Message::FileDiffLoaded {
                    worktree: wt,
                    path: "a.txt".into(),
                    op: OpId(2),
                    result: Ok(expected.clone()),
                });
                assert_eq!(*state.diff().patch(), PatchState::Loaded(expected));
            }
        }

        /// 목록에 없는 파일의 patch 요청은 조용히 무시한다 — `status_of`가
        /// `None`이면 `file_diff`에 넘길 `ChangeStatus`가 없다.
        #[test]
        fn a_patch_request_for_an_unlisted_file_is_ignored() {
            let (mut state, wt) = state_with_worktree();
            let _ = state.update(Message::DiffRequested {
                worktree: wt.clone(),
            });
            let _ = state.update(Message::FileDiffRequested {
                worktree: wt,
                path: "not-in-the-list.txt".into(),
            });
            assert_eq!(*state.diff().patch(), PatchState::Idle);
            assert!(state.last_error().is_none());
        }
    }
}
