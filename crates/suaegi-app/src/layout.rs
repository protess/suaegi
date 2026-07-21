//! 레이아웃 영속화 — `pane_grid` 트리 ↔ [`PersistedPane`] 변환.
//!
//! **전부 순수 함수다.** `pane_grid::State`는 `Serialize`를 갖지 않고 갖게 만들
//! 수도 없지만(외래 타입) 공개 API로 완전히 왕복한다: 읽기는
//! `State::layout() -> &Node`(필드가 전부 공개), 쓰기는
//! `State::with_configuration(Configuration<T>)`. `Pane`/`Split`의 내부 `usize`는
//! 비공개라 **직렬화하지 않고 트리를 걸으며 우리 값으로 치환한다** — 복원 후
//! pane id는 달라지지만 무관하다. 우리는 `WorktreeId`로 키를 잡는다.
//!
//! 이 파일이 `state.rs`와 분리된 이유는 그것이 배리어 규칙을 **실제 값으로**
//! 검사할 수 있는 유일한 형태이기 때문이다. `AppState`를 통해서만 검사하면
//! 세션을 띄워야 하고, 그러면 "양쪽 자식이 다 실패한 분할" 같은 모양을 만드는
//! 비용이 규칙 자체를 검사하는 비용보다 커진다.

use std::collections::{HashMap, HashSet};

use iced::widget::pane_grid::{self, Configuration, Node};
use suaegi_core::domain::{PersistedAxis, PersistedPane, WorktreeId};

use crate::session_store::SessionId;

/// 복원 시 잎 하나가 받는 **종단** 결과. 잎마다 정확히 하나가 온다.
///
/// `Failed`와 `WorktreeGone`은 트리 구성에서 똑같이 다뤄지지만(둘 다 `None`)
/// 구분해 남긴다 — "세션 스폰이 실패했다"와 "worktree 자체가 사라졌다"는 사용자에게
/// 다른 사실이고, 후자는 재시도해도 소용이 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeafOutcome {
    Started(SessionId),
    Failed,
    WorktreeGone,
}

/// 저장할 분할 비율. **0.5에서 0.005 넘게 벗어날 때만 소수 3자리로** 남기고
/// 나머지는 0.5로 스냅한다.
///
/// float 잡음이 저장을 계속 흔드는 것을 막는다: `Store::save`가 내용 해시로
/// 변경 없음을 스킵하는데, `0.5000001`과 `0.4999998`이 매번 다른 JSON이 되면
/// 그 스킵이 무력해져 사용자가 아무것도 안 해도 디스크 쓰기가 계속 난다.
pub(crate) fn quantize_ratio(ratio: f32) -> f32 {
    // **비유한 값은 여기서 끝낸다 — 이것이 데이터 파일 전체를 날리는 경로다.**
    //
    // `serde_json`은 비유한 float을 `null`로 쓰고, `null`은 `f32`로 **역직렬화되지
    // 않는다.** 그러면 우리가 쓴 파일을 우리가 다시 못 읽고, `parse_trusted`가
    // 문서 전체를 거절하므로 repo·worktree 메타데이터·설정이 **같이** 사라진다.
    // 게다가 저장할 때마다 백업이 회전하므로 몇 시간이면 백업 슬롯 다섯 개가
    // 전부 그 손상된 파일로 덮인다.
    //
    // **손상된 파일이 없어도 도달한다**: iced가
    // `(position / rectangle.height).clamp(0.0, 1.0)`으로 비율을 만드는데
    // (`iced_widget-0.14.2/src/pane_grid.rs:656-670`), 높이 0짜리 분할 영역에
    // 커서가 경계에 있으면 `0.0/0.0` = NaN이고 **Rust의 `f32::clamp`는 NaN을
    // 그대로 돌려준다.** 실측으로 `"ratio":null` → 재파싱 실패까지 확인했다.
    //
    // 이 크레이트는 이미 같은 방어를 한다(`terminal/state.rs`, `terminal/mouse.rs`) —
    // 이 파일만 예외였다.
    if !ratio.is_finite() {
        return 0.5;
    }
    let ratio = ratio.clamp(0.0, 1.0);
    if (ratio - 0.5).abs() <= 0.005 {
        0.5
    } else {
        (ratio * 1000.0).round() / 1000.0
    }
}

/// 복원 시 읽어 들이는 비율의 방어. 저장 경로가 막혀도 **이미 디스크에 있는**
/// 파일이나 손으로 편집한 파일이 비유한 값을 담고 있을 수 있다.
fn safe_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

fn persisted_axis(axis: pane_grid::Axis) -> PersistedAxis {
    match axis {
        pane_grid::Axis::Horizontal => PersistedAxis::Horizontal,
        pane_grid::Axis::Vertical => PersistedAxis::Vertical,
    }
}

fn grid_axis(axis: PersistedAxis) -> pane_grid::Axis {
    match axis {
        PersistedAxis::Horizontal => pane_grid::Axis::Horizontal,
        PersistedAxis::Vertical => pane_grid::Axis::Vertical,
    }
}

/// 지금 화면의 트리를 저장 가능한 모양으로 옮긴다. 잎의 `SessionId`를
/// `WorktreeId`로 치환한다 — `SessionId`는 실행마다 매기는 카운터라 재시작을
/// 넘지 못한다.
///
/// **`None`이 될 수 있다**: worktree에 묶이지 않은 세션(매핑이 아직/이미 없는
/// 세션)의 잎은 저장할 수 없으므로 [`collapse`]와 같은 규칙으로 접는다. 트리
/// 전체가 그러면 저장할 레이아웃이 없다는 뜻이다.
pub(crate) fn to_persisted(
    node: &Node,
    panes: &pane_grid::State<SessionId>,
    session_worktrees: &HashMap<SessionId, WorktreeId>,
) -> Option<PersistedPane> {
    match node {
        Node::Pane(pane) => {
            let session = panes.get(*pane)?;
            let worktree = session_worktrees.get(session)?;
            Some(PersistedPane::Leaf(worktree.clone()))
        }
        Node::Split {
            axis, ratio, a, b, ..
        } => {
            let a = to_persisted(a, panes, session_worktrees);
            let b = to_persisted(b, panes, session_worktrees);
            collapse(a, b, |a, b| PersistedPane::Split {
                axis: persisted_axis(*axis),
                ratio: quantize_ratio(*ratio),
                a: Box::new(a),
                b: Box::new(b),
            })
        }
    }
}

/// **복원 배리어.** 잎의 종단 결과로 트리를 다시 짓는다.
///
/// **재귀로 정의해야 한다** — "형제가 부모 자리를 차지"라는 말로는 **양쪽이 다
/// 실패한 분할**을 정의하지 못하고, 중첩된 서브트리가 통째로 비는 경우도 같다.
/// 다섯 경우가 전부다([`collapse`]가 그중 넷을 담는다):
///
/// | 노드 | 결과 |
/// |---|---|
/// | 잎, 시작 성공 | `Some(Pane)` |
/// | 잎, `Failed`/`WorktreeGone`/중복 | `None` |
/// | 분할, 양쪽 살아남음 | `Some(Split)` |
/// | 분할, 한쪽만 살아남음 | 그 한쪽 (형제 승격) |
/// | 분할, 양쪽 다 소멸 | `None` (서브트리 전체 소멸) |
///
/// 루트가 `None`이면 빈 워크벤치다. **부분 복원을 허용한다** — 하나 실패했다고
/// 전체를 버리지 않고, 전부 실패해야만 포기한다. 재시도는 하지 않는다.
///
/// `seen`이 **중복 `WorktreeId` 잎**을 접는다. 디스크의 JSON은 손상·수동 편집·
/// 구버전 버그로 같은 id를 여러 잎에 담을 수 있고, 그러면 `PaneKey`가 중복돼 훅
/// 라우팅이 모호해진다. **순회 순서상 첫 등장만 남긴다** — `a`를 `b`보다 먼저
/// 평가하는 것이 그 순서의 정의이므로 아래 `let` 두 줄의 순서가 계약이다.
pub(crate) fn to_configuration(
    node: &PersistedPane,
    outcomes: &HashMap<WorktreeId, LeafOutcome>,
    seen: &mut HashSet<WorktreeId>,
) -> Option<Configuration<SessionId>> {
    match node {
        PersistedPane::Leaf(worktree) => {
            if !seen.insert(worktree.clone()) {
                return None;
            }
            match outcomes.get(worktree) {
                Some(LeafOutcome::Started(id)) => Some(Configuration::Pane(*id)),
                Some(LeafOutcome::Failed | LeafOutcome::WorktreeGone) | None => None,
            }
        }
        PersistedPane::Split { axis, ratio, a, b } => {
            // 순서가 계약이다 — 중복 접기의 "첫 등장"이 이 두 줄로 정의된다.
            let a = to_configuration(a, outcomes, seen);
            let b = to_configuration(b, outcomes, seen);
            collapse(a, b, |a, b| Configuration::Split {
                axis: grid_axis(*axis),
                // 디스크의 값을 그대로 믿지 않는다 — 손상·수동 편집으로 비유한
                // 값이 들어 있으면 pane_grid의 레이아웃 계산이 NaN으로 오염된다.
                ratio: safe_ratio(*ratio),
                a: Box::new(a),
                b: Box::new(b),
            })
        }
    }
}

/// 분할 노드의 네 경우를 한 곳에 둔다. **저장 경로와 복원 경로가 같은 규칙을
/// 쓴다는 것**이 이 함수의 존재 이유다 — 따로 쓰면 한쪽만 고쳐져 왕복이 깨진다.
fn collapse<T>(a: Option<T>, b: Option<T>, split: impl FnOnce(T, T) -> T) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(split(a, b)),
        // 형제 승격: 살아남은 쪽이 분할 자리를 그대로 물려받는다.
        (Some(x), None) | (None, Some(x)) => Some(x),
        // 서브트리 전체 소멸.
        (None, None) => None,
    }
}

/// 저장된 트리에서 잎 하나를 지운다. [`collapse`]와 **같은 규칙**으로 접히므로
/// 형제 승격과 빈 서브트리 소멸이 복원 경로와 일치한다.
///
/// **쓰이는 곳이 하나뿐이라는 것이 중요하다**: 권위 있는 목록이 worktree의
/// 소멸을 확인했을 때. 세션 시작 실패로는 부르지 않는다 — 그것은 일시적
/// 실패이지 사라졌다는 증거가 아니다.
pub(crate) fn without_leaf(node: &PersistedPane, gone: &WorktreeId) -> Option<PersistedPane> {
    match node {
        PersistedPane::Leaf(worktree) => (worktree != gone).then(|| node.clone()),
        PersistedPane::Split { axis, ratio, a, b } => {
            let a = without_leaf(a, gone);
            let b = without_leaf(b, gone);
            collapse(a, b, |a, b| PersistedPane::Split {
                axis: *axis,
                ratio: *ratio,
                a: Box::new(a),
                b: Box::new(b),
            })
        }
    }
}

/// 순회 순서(왼쪽/위 먼저)로 잎의 `WorktreeId`를 모으되 **중복은 첫 등장만**
/// 남긴다. 복원이 세션을 띄우기 전에 부르는 함수다 — 같은 worktree로 세션을
/// 두 번 띄우지 않기 위해서고, 여기의 순서가 [`to_configuration`]의 `seen`이
/// 접는 순서와 반드시 같아야 한다.
pub(crate) fn leaves_in_order(node: &PersistedPane) -> Vec<WorktreeId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    collect_leaves(node, &mut seen, &mut out);
    out
}

fn collect_leaves(node: &PersistedPane, seen: &mut HashSet<WorktreeId>, out: &mut Vec<WorktreeId>) {
    match node {
        PersistedPane::Leaf(worktree) => {
            if seen.insert(worktree.clone()) {
                out.push(worktree.clone());
            }
        }
        PersistedPane::Split { a, b, .. } => {
            collect_leaves(a, seen, out);
            collect_leaves(b, seen, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(name: &str) -> WorktreeId {
        WorktreeId(name.to_string())
    }

    fn leaf(name: &str) -> PersistedPane {
        PersistedPane::Leaf(wt(name))
    }

    fn split(axis: PersistedAxis, ratio: f32, a: PersistedPane, b: PersistedPane) -> PersistedPane {
        PersistedPane::Split {
            axis,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    fn started(names: &[&str]) -> HashMap<WorktreeId, LeafOutcome> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| (wt(n), LeafOutcome::Started(SessionId(i as u64 + 1))))
            .collect()
    }

    fn build(
        tree: &PersistedPane,
        outcomes: &HashMap<WorktreeId, LeafOutcome>,
    ) -> Option<Configuration<SessionId>> {
        to_configuration(tree, outcomes, &mut HashSet::new())
    }

    /// `Configuration`은 `PartialEq`를 갖지 않는다(외래 타입). 모양을 비교
    /// 가능한 값으로 찍어낸다 — 이렇게 해야 "어떤 트리가 나왔는지"를 눈으로
    /// 읽히는 문자열로 단언할 수 있다.
    fn shape(config: &Configuration<SessionId>) -> String {
        match config {
            Configuration::Pane(id) => format!("{}", id.0),
            Configuration::Split { axis, ratio, a, b } => format!(
                "({}{:.3} {} {})",
                match axis {
                    pane_grid::Axis::Horizontal => "H",
                    pane_grid::Axis::Vertical => "V",
                },
                ratio,
                shape(a),
                shape(b)
            ),
        }
    }

    fn shape_of(config: &Option<Configuration<SessionId>>) -> String {
        config
            .as_ref()
            .map(shape)
            .unwrap_or_else(|| "-".to_string())
    }

    // ---- 배리어: 실패한 잎이 트리를 어떻게 접는가 ----

    /// **대조군**: 전부 성공하면 트리가 원형 그대로여야 한다. 이게 없으면 아래
    /// 접힘 단언들이 "이 함수가 늘 접는다"로도 설명된다.
    #[test]
    fn a_fully_successful_restore_keeps_the_tree_intact() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("a"),
            split(PersistedAxis::Horizontal, 0.75, leaf("b"), leaf("c")),
        );
        assert_eq!(
            shape_of(&build(&tree, &started(&["a", "b", "c"]))),
            "(V0.250 1 (H0.750 2 3))",
            "control: nothing failed, so nothing may collapse"
        );
    }

    #[test]
    fn a_single_failed_leaf_is_replaced_by_its_sibling() {
        let tree = split(PersistedAxis::Vertical, 0.25, leaf("a"), leaf("b"));
        let mut outcomes = started(&["b"]);
        outcomes.insert(wt("a"), LeafOutcome::Failed);

        assert_eq!(
            shape_of(&build(&tree, &outcomes)),
            "1",
            "the surviving sibling takes the split's place — it must not stay wrapped \
             in a split with a hole in it"
        );
    }

    /// 플랜이 명시적으로 요구한 첫 번째 구멍: "형제가 부모 자리를 차지"라는
    /// 표현으로는 **직계 자식 둘 다 실패**를 정의할 수 없다.
    #[test]
    fn a_split_whose_direct_children_both_fail_disappears_entirely() {
        let tree = split(PersistedAxis::Vertical, 0.25, leaf("a"), leaf("b"));
        let outcomes = HashMap::from([
            (wt("a"), LeafOutcome::Failed),
            (wt("b"), LeafOutcome::WorktreeGone),
        ]);

        assert_eq!(
            shape_of(&build(&tree, &outcomes)),
            "-",
            "with no survivor there is no sibling to promote — the node itself must go"
        );
    }

    /// 두 번째 구멍: **중첩 서브트리가 통째로 비는 경우.** 서브트리가 `None`으로
    /// 접힌 뒤 그 결과가 부모의 형제 승격에 다시 참여해야 한다 — 재귀가 아니면
    /// 여기서 빈 분할이 살아남는다.
    #[test]
    fn a_nested_subtree_that_empties_promotes_its_uncle() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("keep"),
            split(PersistedAxis::Horizontal, 0.75, leaf("x"), leaf("y")),
        );
        let mut outcomes = started(&["keep"]);
        outcomes.insert(wt("x"), LeafOutcome::Failed);
        outcomes.insert(wt("y"), LeafOutcome::Failed);

        assert_eq!(
            shape_of(&build(&tree, &outcomes)),
            "1",
            "the whole right subtree vanished, so the left leaf must become the root — \
             not the root of a split with an empty side"
        );
    }

    #[test]
    fn a_tree_in_which_everything_fails_leaves_no_layout_at_all() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("a"),
            split(PersistedAxis::Horizontal, 0.75, leaf("b"), leaf("c")),
        );
        let outcomes = HashMap::from([
            (wt("a"), LeafOutcome::Failed),
            (wt("b"), LeafOutcome::WorktreeGone),
            (wt("c"), LeafOutcome::Failed),
        ]);
        assert_eq!(shape_of(&build(&tree, &outcomes)), "-");
    }

    /// 결과가 아예 보고되지 않은 잎(`None`)도 실패와 같이 다룬다 — 배리어는
    /// "시작을 확인한 것만" 살린다.
    #[test]
    fn a_leaf_with_no_reported_outcome_counts_as_failed() {
        let tree = split(PersistedAxis::Vertical, 0.25, leaf("a"), leaf("b"));
        assert_eq!(
            shape_of(&build(&tree, &started(&["b"]))),
            "1",
            "'a' was never reported at all and must not become a pane"
        );
    }

    // ---- 중복 잎 접기 ----

    /// 디스크의 JSON은 손상·수동 편집으로 같은 `WorktreeId`를 여러 잎에 담을 수
    /// 있다. 두 잎이 같은 `SessionId`를 가리키면 `PaneKey`가 중복돼 훅 라우팅이
    /// 모호해진다 — **순회 순서상 첫 등장만 남긴다.**
    #[test]
    fn a_duplicated_worktree_leaf_survives_only_at_its_first_occurrence() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("dup"),
            split(PersistedAxis::Horizontal, 0.75, leaf("dup"), leaf("other")),
        );
        assert_eq!(
            shape_of(&build(&tree, &started(&["dup", "other"]))),
            "(V0.250 1 2)",
            "the second 'dup' leaf must fold away, leaving the first occurrence and \
             'other' — two panes for one session make PaneKey ambiguous"
        );
    }

    /// 중복이 **첫 등장 쪽에** 남는지를 직접 고정한다. 위 테스트의 모양만으로는
    /// "둘 중 하나가 남았다"까지만 말할 수 있다.
    #[test]
    fn folding_a_duplicate_keeps_the_first_occurrence_not_the_last() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            split(
                PersistedAxis::Horizontal,
                0.75,
                leaf("dup"),
                leaf("first-only"),
            ),
            leaf("dup"),
        );
        // 'dup'는 왼쪽 서브트리에서 먼저 나온다. 오른쪽 잎이 접히면 왼쪽
        // 서브트리가 승격돼 루트가 된다.
        assert_eq!(
            shape_of(&build(&tree, &started(&["dup", "first-only"]))),
            "(H0.750 1 2)",
            "traversal order defines 'first' — the right-hand duplicate is the one \
             that folds, and the left subtree is promoted in its place"
        );
    }

    #[test]
    fn leaves_are_collected_in_traversal_order_with_duplicates_removed() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            split(PersistedAxis::Horizontal, 0.75, leaf("a"), leaf("b")),
            split(PersistedAxis::Horizontal, 0.75, leaf("a"), leaf("c")),
        );
        assert_eq!(
            leaves_in_order(&tree),
            vec![wt("a"), wt("b"), wt("c")],
            "one start per worktree, in the order the tree names them — and this order \
             must match the one to_configuration folds duplicates by"
        );
    }

    // ---- 저장 방향(트리 → PersistedPane) ----

    /// **왕복의 나머지 절반.** 위 테스트들은 전부 복원 방향만 본다 — 저장이
    /// 축을 잘못 적어도 하나도 죽지 않는다(mutation으로 확인: `axis`를
    /// `Horizontal`로 고정한 뮤턴트가 살아남았다).
    ///
    /// **세로 분할은 도달 가능한 입력이다**: `open_pane_for_session`은 가로로만
    /// 쪼개지만, 디스크에서 복원된 트리는 세로 분할을 담을 수 있고 그다음 저장이
    /// 그 트리를 다시 걷는다. 축이 뭉개지면 앱을 켤 때마다 레이아웃이 조금씩
    /// 돌아간다.
    #[test]
    fn a_live_grid_walks_back_out_to_the_tree_it_was_built_from() {
        let original = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("a"),
            split(PersistedAxis::Horizontal, 0.75, leaf("b"), leaf("c")),
        );
        let outcomes = started(&["a", "b", "c"]);
        let config = build(&original, &outcomes).expect("every leaf started");
        let panes = pane_grid::State::with_configuration(config);

        let session_worktrees: HashMap<SessionId, WorktreeId> = outcomes
            .iter()
            .filter_map(|(worktree, outcome)| match outcome {
                LeafOutcome::Started(id) => Some((*id, worktree.clone())),
                _ => None,
            })
            .collect();

        assert_eq!(
            to_persisted(panes.layout(), &panes, &session_worktrees),
            Some(original),
            "a tree restored into a live pane_grid must serialize back to exactly \
             itself — axis, ratio and structure alike"
        );
    }

    /// worktree에 묶이지 않은 세션의 잎은 저장할 수 없다 — 복원과 **같은 규칙**으로
    /// 접힌다. (세션이 닫히는 중이면 매핑이 먼저 사라진다.)
    #[test]
    fn a_pane_whose_session_has_no_worktree_folds_out_of_the_saved_tree() {
        let tree = split(PersistedAxis::Vertical, 0.25, leaf("a"), leaf("b"));
        let outcomes = started(&["a", "b"]);
        let config = build(&tree, &outcomes).expect("both started");
        let panes = pane_grid::State::with_configuration(config);

        // 'a'의 매핑만 남긴다 — 'b'는 worktree를 잃었다.
        let session_worktrees: HashMap<SessionId, WorktreeId> = outcomes
            .iter()
            .filter_map(|(worktree, outcome)| match (worktree, outcome) {
                (w, LeafOutcome::Started(id)) if w == &wt("a") => Some((*id, w.clone())),
                _ => None,
            })
            .collect();

        assert_eq!(
            to_persisted(panes.layout(), &panes, &session_worktrees),
            Some(leaf("a")),
            "the unmappable leaf folds and its sibling is promoted — saving a split with \
             a hole in it would restore as a phantom pane"
        );
    }

    // ---- ratio 양자화 ----

    /// **비유한 비율은 데이터 파일 전체를 파괴한다.** `serde_json`은 그것을
    /// `null`로 쓰고, `null`은 `f32`로 역직렬화되지 않는다 — 우리가 쓴 파일을
    /// 우리가 못 읽고, 문서 전체가 거절되므로 repo·설정까지 같이 사라진다.
    ///
    /// **손상된 파일 없이도 도달한다**: iced가 높이 0인 분할에서
    /// `(0.0/0.0).clamp(0.0,1.0)`을 만들고 `f32::clamp`는 NaN을 통과시킨다.
    #[test]
    fn a_non_finite_ratio_never_reaches_the_serializer() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0 / 0.0] {
            let q = quantize_ratio(bad);
            assert!(
                q.is_finite(),
                "quantize_ratio({bad}) produced {q}, which serde_json writes as null and \
                 then refuses to read back — that loses the entire data file, not just \
                 the layout"
            );
            assert!((0.0..=1.0).contains(&q), "and it must be a legal ratio");
        }
    }

    /// 파괴 사슬 전체를 **실제 직렬화로** 고정한다. 위 단언은 `is_finite`만 보므로
    /// serde의 동작이 바뀌면 조용히 무의미해질 수 있다.
    #[test]
    fn a_tree_built_from_a_nan_resize_still_round_trips_through_json() {
        let tree = PersistedPane::Split {
            axis: PersistedAxis::Vertical,
            ratio: quantize_ratio(0.0 / 0.0),
            a: Box::new(leaf("a")),
            b: Box::new(leaf("b")),
        };
        let json = serde_json::to_string(&tree).expect("serializes");
        assert!(
            !json.contains("null"),
            "a null ratio is what makes the file unreadable; got {json}"
        );
        assert_eq!(
            serde_json::from_str::<PersistedPane>(&json).expect("must reparse"),
            tree,
            "the file we write must be a file we can read back"
        );
    }

    /// 디스크에 **이미** 비유한 값이 있는 경우(손으로 편집했거나 이 수정 이전
    /// 빌드가 썼거나). 복원이 그걸 그대로 pane_grid에 넣으면 레이아웃 계산이
    /// NaN으로 오염된다.
    #[test]
    fn a_non_finite_ratio_read_from_disk_is_repaired_on_restore() {
        let tree = PersistedPane::Split {
            axis: PersistedAxis::Vertical,
            ratio: f32::NAN,
            a: Box::new(leaf("a")),
            b: Box::new(leaf("b")),
        };
        let config = build(&tree, &started(&["a", "b"])).expect("both started");
        match config {
            Configuration::Split { ratio, .. } => assert!(
                ratio.is_finite(),
                "a corrupt ratio on disk must be repaired, not fed into pane_grid"
            ),
            Configuration::Pane(_) => panic!("expected a split"),
        }
    }

    #[test]
    fn ratios_near_the_middle_snap_to_exactly_one_half() {
        for noisy in [0.5_f32, 0.4999998, 0.5000001, 0.4951, 0.5049] {
            assert_eq!(
                quantize_ratio(noisy),
                0.5,
                "{noisy} is within 0.005 of centre and must serialize identically every \
                 time — otherwise float noise defeats the unchanged-content save skip"
            );
        }
    }

    #[test]
    fn a_deliberately_uneven_ratio_is_kept_to_three_decimals() {
        // 대조군: 양자화가 "전부 0.5로 만든다"가 아니라는 것. 사용자가 실제로
        // 끌어다 놓은 위치는 반드시 살아남아야 한다.
        assert_eq!(quantize_ratio(0.25), 0.25);
        assert_eq!(quantize_ratio(0.7512345), 0.751);
        assert_eq!(quantize_ratio(0.3333333), 0.333);
        // 경계 바로 바깥: 0.005를 **넘어야** 유지된다.
        assert_eq!(quantize_ratio(0.4939), 0.494);
    }
}
