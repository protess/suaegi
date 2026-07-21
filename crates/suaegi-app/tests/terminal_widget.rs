//! 터미널 위젯의 **배선**을 헤드리스로 확인한다 — 이벤트가 들어가면 어떤
//! 커맨드가 나오는가.
//!
//! 여기서 검증하지 않는 것과 그 이유는 `tests/harness`의 모듈 문서에 있다.
//! 요약하면 `()` 렌더러가 텍스트를 측정하지 못하므로 **측정 자체와 측정 캐시
//! 무효화는 이 파일의 사정권 밖**이다. 그래서 각 테스트는 위젯 상태에 실제
//! 메트릭을 심어 넣고 그 **다음**을 본다. 측정 없이 도는 계산(`grid_size`,
//! `hit_test`)의 표 테스트는 `src/terminal/state.rs`의 유닛 테스트에 있다.

mod harness;

use iced::advanced::widget::Tree;
use iced::{Element, Size, Theme};

use suaegi_app::session_store::{SessionId, blank_snapshot};
use suaegi_app::terminal::contract::TermCommand;
use suaegi_app::terminal::state::{CellMetrics, State};
use suaegi_app::terminal::{Published, Terminal};

use harness::{Harness, RecordingClipboard, Step, redraw};

const ID: SessionId = SessionId(7);

/// 8x16은 이진수로 정확해서 "딱 떨어지는 크기"가 부동소수 오차로 한 칸
/// 모자라게 나오지 않는다.
fn metrics() -> CellMetrics {
    CellMetrics::new(8.0, 16.0).expect("8x16 must be valid metrics")
}

/// `()` 렌더러는 측정을 못 하므로 `layout`이 메트릭을 채우지 못한다. 측정
/// **결과**를 심어 그 다음 배선을 본다 — 측정은 검증 대상이 아니다.
fn seed_metrics(tree: &mut Tree) {
    tree.state.downcast_mut::<State>().set_metrics(metrics());
}

/// `TermCommand`는 `PartialEq`를 파생할 수 없다 —
/// `alacritty_terminal::grid::Scroll`이 `Debug, Copy, Clone`만 파생한다
/// (`grid/mod.rs:72`). 그래서 커맨드 목록을 통째로 비교하지 않고 필요한 변형만
/// 분해해 꺼낸다. 래퍼 타입을 만들지 않는 것이 규칙이다.
fn resizes(messages: &[Published]) -> Vec<(u16, u16, u64)> {
    messages
        .iter()
        .filter_map(|(id, command)| match command {
            TermCommand::Resize { rows, cols, seq } => {
                assert_eq!(*id, ID, "every command must carry the widget's session id");
                Some((*rows, *cols, *seq))
            }
            _ => None,
        })
        .collect()
}

fn run(bounds: Size, steps: &[Step], seed: bool) -> Vec<Published> {
    let snapshot = blank_snapshot();
    let element: Element<'_, Published, Theme, ()> = Terminal::new(ID, &snapshot).into();

    let mut harness = Harness::new().with_bounds(bounds);
    if seed {
        harness.run_seeded(element, steps, seed_metrics)
    } else {
        harness.run(element, steps)
    }
    .into_messages()
}

// ------------------------------------------------------------------ 대조군 한 쌍

/// 메트릭이 없으면 리사이즈를 발행하지 않는다 — 그리고 **그 짝**으로, 메트릭이
/// 있으면 같은 시퀀스가 정확히 하나를 발행한다. 둘을 같이 단언하지 않으면
/// "아무 일도 안 일어났다"가 배선이 옳아서인지 애초에 아무것도 안 돌아서인지
/// 구별되지 않는다.
#[test]
fn metrics_are_what_makes_a_resize_happen() {
    let bounds = Size::new(800.0, 400.0);
    let steps = [Step::nowhere(redraw())];

    let without = run(bounds, &steps, false);
    assert!(
        resizes(&without).is_empty(),
        "without cell metrics the widget must not guess a grid size; got {:?}",
        resizes(&without)
    );

    let with = run(bounds, &steps, true);
    assert_eq!(
        resizes(&with),
        vec![(25, 100, resizes(&with)[0].2)],
        "control: the identical run WITH metrics must emit exactly one resize"
    );
}

// ------------------------------------------------------------------ 리사이즈 발행

#[test]
fn a_size_change_emits_exactly_one_resize() {
    let emitted = run(Size::new(800.0, 400.0), &[Step::nowhere(redraw())], true);

    let resizes = resizes(&emitted);
    assert_eq!(resizes.len(), 1, "expected one resize, got {resizes:?}");
    assert_eq!(
        (resizes[0].0, resizes[0].1),
        (25, 100),
        "800x400 with 8x16 cells is 100 columns by 25 rows"
    );
    assert!(
        resizes[0].2 > 0,
        "seq must be non-zero so that 0 can mean \"nothing emitted yet\""
    );
}

#[test]
fn re_entering_the_same_size_emits_nothing_more() {
    // 같은 bounds로 프레임을 여러 번 돌린다. 리사이즈 판정이 매 이벤트마다
    // 도므로, 캐시가 없으면 프레임 수만큼 발행된다.
    let emitted = run(
        Size::new(800.0, 400.0),
        &[
            Step::nowhere(redraw()),
            Step::nowhere(redraw()),
            Step::nowhere(redraw()),
        ],
        true,
    );

    let resizes = resizes(&emitted);
    assert_eq!(
        resizes.len(),
        1,
        "three frames at one size must produce one resize, not three; got {resizes:?}"
    );
}

#[test]
fn a_different_size_emits_a_new_resize() {
    // 대조군: 위 테스트의 "한 번만"이 캐시 때문인지 아니면 애초에 한 번만
    // 발행하고 마는 건지 가른다.
    let emitted = run(
        Size::new(800.0, 400.0),
        &[
            Step::nowhere(redraw()),
            Step::Bounds(Size::new(400.0, 400.0)),
            Step::nowhere(redraw()),
        ],
        true,
    );

    let resizes = resizes(&emitted);
    assert_eq!(resizes.len(), 2, "expected two resizes, got {resizes:?}");
    assert_eq!((resizes[0].0, resizes[0].1), (25, 100));
    assert_eq!((resizes[1].0, resizes[1].1), (25, 50));
    assert!(
        resizes[1].2 > resizes[0].2,
        "seq must increase so the app's \"latest seq wins\" guard keeps the \
         newer resize; got {resizes:?}"
    );
}

/// **두 캐시가 없으면 깨지는 시나리오.** pane을 0으로 접었다가 원래 크기로
/// 되돌리면 리사이즈가 다시 나가야 한다.
///
/// `last_emitted`(발행한 그리드 크기) 하나만 두면: 접힐 때 `grid_size`가
/// `None`이라 발행하지 않고 `last_emitted`는 옛 값으로 남는다 → 되돌아왔을 때
/// "같은 크기"로 보여 발행하지 않는다 → PTY가 낡은 크기에 남는다. 화면은
/// 멀쩡한데 셸만 틀린 크기를 믿는 상태라 눈으로는 잘 안 보인다.
#[test]
fn collapsing_to_zero_and_restoring_re_emits_the_resize() {
    let big = Size::new(800.0, 400.0);
    // 한 행도 들어가지 않는 높이 — pane을 끝까지 민 상태다.
    let collapsed = Size::new(800.0, 10.0);

    let emitted = run(
        big,
        &[
            Step::nowhere(redraw()),
            Step::Bounds(collapsed),
            Step::nowhere(redraw()),
            Step::Bounds(big),
            Step::nowhere(redraw()),
        ],
        true,
    );

    let resizes = resizes(&emitted);
    assert_eq!(
        resizes.len(),
        2,
        "the collapse itself must emit nothing, but the restore must emit \
         again; got {resizes:?}"
    );
    assert_eq!((resizes[0].0, resizes[0].1), (25, 100), "the first layout");
    assert_eq!(
        (resizes[1].0, resizes[1].1),
        (25, 100),
        "the restore emits the SAME grid size — that is precisely why a single \
         cache cannot detect it"
    );
    assert!(
        resizes[1].2 > resizes[0].2,
        "the re-emitted resize must be newer; got {resizes:?}"
    );
}

/// 접힘 자체는 아무것도 발행하지 않는다. 위 테스트가 총 개수로 이미 덮지만,
/// **0행짜리 리사이즈가 PTY로 나가지 않는다**는 것이 따로 확인할 값이 있는
/// 성질이다 — `TerminalSession::resize`가 0을 조용히 무시하므로 실수가 오류로
/// 드러나지 않는다.
#[test]
fn a_collapsed_pane_never_emits_a_zero_sized_grid() {
    let emitted = run(
        Size::new(800.0, 400.0),
        &[
            Step::nowhere(redraw()),
            Step::Bounds(Size::new(800.0, 10.0)),
            Step::nowhere(redraw()),
            Step::Bounds(Size::new(0.0, 0.0)),
            Step::nowhere(redraw()),
        ],
        true,
    );

    let resizes = resizes(&emitted);
    assert!(
        resizes.iter().all(|(rows, cols, _)| *rows > 0 && *cols > 0),
        "no resize may carry a zero dimension; got {resizes:?}"
    );
    assert_eq!(
        resizes.len(),
        1,
        "only the first (valid) layout emits; got {resizes:?}"
    );
}

// ---------------------------------------------------------------- 클립보드 페이크

/// Task 4의 붙여넣기와 Task 6의 복사가 이 페이크에 전부 걸린다. 아직 그 경로를
/// 쓰는 위젯 코드가 없으므로, **위젯이 보는 그대로**(`&mut dyn Clipboard`)
/// 여기서 직접 확인해둔다 — 안 그러면 Task 4가 망가진 페이크 위에서 시작한다.
#[test]
fn the_clipboard_fake_is_readable_and_records_both_kinds() {
    use iced::advanced::clipboard::{Clipboard, Kind};

    let mut fake = RecordingClipboard::seeded("hello");
    let clipboard: &mut dyn Clipboard = &mut fake;

    assert_eq!(
        clipboard.read(Kind::Standard).as_deref(),
        Some("hello"),
        "clipboard::Null returns None here, which is why this fake exists"
    );
    clipboard.write(Kind::Primary, "world".to_owned());
    assert_eq!(clipboard.read(Kind::Primary).as_deref(), Some("world"));
    assert_eq!(
        clipboard.read(Kind::Standard).as_deref(),
        Some("hello"),
        "the two clipboards must stay independent — writing Primary only must \
         be distinguishable from writing both"
    );

    assert_eq!(
        fake.writes(),
        vec![(Kind::Primary, "world".to_owned())],
        "the write log must show which kinds were actually targeted"
    );
    assert_eq!(
        fake.reads(),
        vec![Kind::Standard, Kind::Primary, Kind::Standard],
        "reads are recorded in order"
    );
}
