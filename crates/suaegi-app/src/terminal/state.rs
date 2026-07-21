//! Task 3 — 위젯 상태와 순수 레이아웃 계산(`CellMetrics`, `grid_size`,
//! `hit_test`). Task 3이 끝나는 시점에 위젯 상태 구조체와 접근자를 **동결**한다:
//! Task 4·5는 그 필드를 읽고 쓸 뿐 정의를 바꾸지 않는다.
//!
//! **왜 순수 함수로 뽑는가.** 헤드리스 하네스의 `()` 렌더러는 텍스트를 측정하지
//! 않고 `Paragraph::compare`가 항상 `Difference::None`이다
//! (`iced_core/src/renderer/null.rs`). 측정에 의존하는 계산을 위젯 안에 남겨두면
//! 그 계산은 **어떤 자동 테스트로도 닿을 수 없다.** 그래서 계산을
//! `CellMetrics`를 인자로 받는 순수 함수로 내보내고 실제 메트릭 값으로 표
//! 테스트한다. 이 분리는 선택이 아니라 테스트 가능성의 전제다.

use std::sync::atomic::{AtomicU64, Ordering};

use alacritty_terminal::index::Side;
use iced::advanced::text;
use iced::advanced::widget::operation;
use iced::alignment;
use iced::{Font, Pixels, Point, Size};

use suaegi_term::grid::GridSize;
use suaegi_term::input_types::{Mods, TermMouseButton, ViewportHit};

use crate::session_store::SessionId;
use crate::terminal::contract::TermCommand;
use crate::terminal::mouse::LastClick;

// ---------------------------------------------------------------------------
// 셀 메트릭
// ---------------------------------------------------------------------------

/// 셀 하나의 크기. **필드가 비공개인 것이 핵심이다** — 생성자가 유효성을
/// 보장하므로 아래 순수 함수들이 "메트릭이 이상한 경우"를 다시 방어하지 않아도
/// 된다. 방어를 두 곳에 두면 어느 쪽이 권위인지 흐려지고, 둘 중 하나는 반드시
/// 도달 불가능한 죽은 코드가 된다.
///
/// **`f32`로 유지한다.** 반올림은 `TermCommand::Resize`를 만들 때 딱 한 번
/// 일어난다 — `iced_term`은 셀 폭을 `u16`으로 잘라 열당 ~1px 드리프트를 쌓는다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    width: f32,
    height: f32,
}

impl CellMetrics {
    /// `width`/`height`가 **유한하고 0보다 클 때만** `Some`.
    ///
    /// 0을 거절하는 이유는 나눗셈이 무한대를 내기 때문만이 아니다 — 폭 0은
    /// 측정이 실패했다는 뜻이고(널 렌더러의 `min_bounds()`가 정확히 그렇다),
    /// 그걸 메트릭으로 받아들이면 실패가 "셀이 아주 작다"로 위장된다.
    pub fn new(width: f32, height: f32) -> Option<Self> {
        if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
            Some(Self { width, height })
        } else {
            None
        }
    }

    pub fn width(self) -> f32 {
        self.width
    }

    pub fn height(self) -> f32 {
        self.height
    }
}

// ---------------------------------------------------------------------------
// 순수 레이아웃 계산
// ---------------------------------------------------------------------------

/// `bounds`에 들어가는 그리드 크기. 결과가 0행/0열이거나 `u16::MAX`를 넘으면
/// `None`.
///
/// **`Option`인 이유**: pane이 0으로 접히는 것은 정상 상태이고, 그때
/// `GridSize { rows: 0, .. }`를 돌려주면 호출자가 "크기가 0인 터미널"을 유효한
/// 값으로 받아들여 PTY에 0을 보낸다(`TerminalSession::resize`는 0을 조용히
/// 무시하므로 오류로도 드러나지 않는다).
///
/// **하나의 범위 검사가 캐스팅 전 방어의 전부다.** `f32 → 정수` 캐스팅은
/// saturating이라 `65536.0 as u16`은 조용히 `65535`가 되고 `-1.0 as usize`는
/// `0`이 된다. `RangeInclusive::contains`는 NaN·음수·무한대에 대해 **전부
/// false**이므로 이 검사 하나가 그 세 경우를 같이 막는다. 별도의 `is_finite`
/// 가드를 앞에 두지 않는 것은 의도적이다 — 그 가드는 결코 도달할 수 없어
/// mutation 검증이 불가능한 죽은 코드가 된다.
pub fn grid_size(bounds: Size, m: CellMetrics) -> Option<GridSize> {
    let cols = (bounds.width / m.width).floor();
    let rows = (bounds.height / m.height).floor();

    const LIMIT: std::ops::RangeInclusive<f32> = 1.0..=u16::MAX as f32;
    if !LIMIT.contains(&cols) || !LIMIT.contains(&rows) {
        return None;
    }

    Some(GridSize {
        rows: rows as usize,
        cols: cols as usize,
    })
}

/// 위젯 bounds **기준 상대 좌표**(`Cursor::position_in`의 결과)를 셀로 옮긴다.
/// 그리드 밖이면 `None`.
///
/// **`Option`인 이유**: 셀을 항상 돌려주면 오른쪽/아래 모서리에서 `col == cols`가
/// 나온다. 그리고 음수·NaN은 캐스팅에서 **조용히 0이 되어** 왼쪽 위 셀을
/// 가리킨다 — 창 밖으로 끌고 나간 드래그가 (0,0)을 선택하는 버그다. 위의
/// `grid_size`와 같은 이유로 범위 검사 하나가 세 경우를 다 막는다.
///
/// 돌려주는 좌표는 **뷰포트 좌표**다. `display_offset` 보정은 락 안에서
/// 그리드가 한다 — 위젯이 스냅샷의 offset으로 미리 보정하면 그 사이 스크롤이
/// 일어나 레이스가 된다.
pub fn hit_test(pos: Point, m: CellMetrics, size: GridSize) -> Option<ViewportHit> {
    let col = (pos.x / m.width).floor();
    let row = (pos.y / m.height).floor();

    if !(0.0..size.cols as f32).contains(&col) || !(0.0..size.rows as f32).contains(&row) {
        return None;
    }

    // 셀의 어느 쪽에 찍혔는가. alacritty의 선택 경계 판정이 요구한다 —
    // 오른쪽 절반에서 시작한 선택은 그 셀을 포함하지 않는다.
    let side = if pos.x - col * m.width < m.width / 2.0 {
        Side::Left
    } else {
        Side::Right
    };

    Some(ViewportHit {
        row: row as usize,
        col: col as usize,
        side,
    })
}

// ---------------------------------------------------------------------------
// 측정 — 순수 함수 **밖**이다
// ---------------------------------------------------------------------------

/// 모노스페이스 셀 하나를 잰다. 측정에 실패하면 `None`.
///
/// **줄 높이는 재지 않는다.** `LineHeight::to_absolute(size)`가 cosmic-text에
/// 그대로 들어가는 권위 있는 값이다(`iced_graphics/src/text/paragraph.rs:71-76`).
/// 측정값과 어긋나면 행 위치가 렌더러와 계산에서 갈린다.
///
/// 폭은 `"M"` 하나가 아니라 10개를 재서 나눈다 — 한 글자만 재면 셰이퍼의 반올림
/// 오차가 그대로 셀 폭이 되고, 그 오차가 열마다 누적된다.
///
/// **이 함수는 헤드리스로 검증할 수 없다.** `()` 렌더러의 `min_bounds()`가 항상
/// `Size::ZERO`라 언제나 `None`을 돌려준다.
pub(crate) fn measure_cell<P>(
    font: Font,
    text_size: Pixels,
    line_height: text::LineHeight,
) -> Option<CellMetrics>
where
    P: text::Paragraph<Font = Font>,
{
    const SAMPLE: &str = "MMMMMMMMMM";

    // `Text`에 `Default`가 없다 — 9개 필드 전부 명시한다.
    let paragraph = P::with_text(text::Text {
        content: SAMPLE,
        bounds: Size::INFINITE,
        size: text_size,
        line_height,
        font,
        align_x: text::Alignment::Default,
        align_y: alignment::Vertical::Top,
        // 모노스페이스 셀 폭을 재는 데 `Advanced`가 필요 없다. 그리고
        // `with_text`는 `with_spans`와 달리 이 값을 존중한다.
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    });

    CellMetrics::new(
        paragraph.min_bounds().width / SAMPLE.len() as f32,
        line_height.to_absolute(text_size).0,
    )
}

// ---------------------------------------------------------------------------
// 리사이즈 시퀀스
// ---------------------------------------------------------------------------

/// 리사이즈 커맨드의 합치기 시퀀스.
///
/// **위젯 상태가 아니라 프로세스 전역이다.** Task 3이 동결한 상태 목록에
/// 시퀀스가 없고, 넣어서도 안 된다: `Tree::diff`는 태그가 어긋나면 서브트리를
/// 통째로 재생성하며 위젯 상태를 **조용히 리셋한다**
/// (`iced_core/src/widget/tree.rs:57-68`). 카운터가 그때 0으로 돌아가면 앱의
/// "세션당 최신 seq만 실행" 가드가 이후의 **모든** 리사이즈를 낡은 것으로 보고
/// 버리고, PTY가 영원히 낡은 크기에 남는다. 전역 단조 증가는 그 실패 모드를
/// 구조적으로 없앤다 — 세션마다 값이 섞이는 것은 무해하다(가드는 세션 안에서만
/// 비교한다).
///
/// 0에서 시작하지 않는다 — 0을 "아직 아무것도 발행하지 않았다"로 쓸 수 있게
/// 남겨둔다.
static RESIZE_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_resize_seq() -> u64 {
    RESIZE_SEQ.fetch_add(1, Ordering::Relaxed) + 1
}

// ---------------------------------------------------------------------------
// 위젯 상태 — 여기서 동결한다
// ---------------------------------------------------------------------------

/// 위젯이 `tree.state`에 들고 다니는 것 전부. **Task 3에서 동결한다** — Task 4·5·6은
/// 이 필드들을 읽고 쓸 뿐 정의를 바꾸지 않는다.
///
/// **선택 상태기계가 없다.** 위젯은 라우팅 결과(선택이냐 리포트냐)를 볼 수 없다 —
/// `MouseResult`는 위젯의 `update`가 끝난 **뒤에** 앱에 돌아가므로, 위젯이 그걸
/// 보고 선택 상태를 유지할 방법이 자체가 없다. 그래서 위젯은 **원시 사실만** 든다:
/// 눌린 버튼, 마지막 클릭 시각·위치, 커서 위치, 스크롤 누산기, 수식자.
///
/// **리사이즈 시퀀스도 없다** — 위의 `RESIZE_SEQ` 문서 참고.
#[derive(Debug, Default)]
pub struct State {
    /// 렌더링·게이팅용이며 **권위가 아니다.** 포커스 전환의 권위는 앱에 있다
    /// (`Focusable::focus/unfocus`는 `Shell`을 받지 못해 바이트를 낼 수 없다).
    pub focused: bool,
    /// 마지막으로 **성공한** 측정. 측정이 실패해도 지우지 않는다 — 아래
    /// `Widget::layout` 참고.
    pub metrics: Option<CellMetrics>,
    /// 마지막으로 본 위젯 bounds. `last_emitted`와 **둘 다** 필요하다.
    pub last_bounds: Option<Size>,
    /// 실제로 발행한 그리드 크기.
    pub last_emitted: Option<GridSize>,
    pub held: Option<TermMouseButton>,
    /// 우리 소유의 클릭 분류기가 쓴다(Task 6). `mouse::Click::new`는 내부에서
    /// `Instant::now()`를 읽어 시계를 주입할 수 없다 → mutation 검증이 불가능하다.
    ///
    /// **`ClickKind`를 함께 들고 다닌다.** 원래는 `(버튼, 시각, 위치)`였는데
    /// 그걸로는 트리플 클릭을 만들 수 없다 — 분류는 "직전 클릭의 **종류**에서
    /// 한 단계 올린다"이지 "직전 클릭이 있었는가"가 아니기 때문이다. 종류를
    /// 안 들고 있으면 두 번째 클릭도 세 번째 클릭도 `Double`이 되어
    /// `ClickKind::Triple`이 영영 도달 불가능해지고, 줄 단위 선택
    /// (`SelectionType::Lines`)이 조용히 사라진다. iced도 같은 벽에 부딪혀
    /// `Click`에 `kind`를 넣어 둔다(`iced_core/src/mouse/click.rs:9-14`,
    /// `previous.kind.next()`는 `:53`).
    pub last_click: Option<LastClick>,
    pub cursor_pos: Option<Point>,
    /// 픽셀 스크롤(트랙패드)의 나머지 누산기. 셀 높이로 나눈 나머지를 보존해야
    /// 작은 델타가 여러 번 와도 정확히 한 줄이 된다.
    pub scroll_acc: f32,
    pub mods: Mods,
}

impl State {
    /// bounds가 이번에 어떤 리사이즈를 발행해야 하는지 정하고 캐시를 갱신한다.
    /// 발행할 것이 없으면 `None`.
    ///
    /// **캐시가 둘인 이유가 이 함수의 전부다.** `last_emitted`(발행한 그리드
    /// 크기)만 두면 이렇게 깨진다:
    ///
    /// 1. pane이 100×25로 열린다 → `Resize{100,25}` 발행, `last_emitted = (100,25)`
    /// 2. 사용자가 분할선을 끝까지 밀어 pane이 0으로 접힌다 → `grid_size`가
    ///    `None` → 발행 없음, `last_emitted`는 **(100,25)로 남는다**
    /// 3. 원래 크기로 되돌린다 → `grid_size`가 다시 `(100,25)` → `last_emitted`와
    ///    **같으므로 발행하지 않는다**
    ///
    /// 그동안 PTY는 2단계에서 무슨 크기였든 그대로다. 화면은 25행인데 셸은
    /// 다른 크기를 믿는다. → **`grid_size`가 `None`이면 `last_emitted`를
    /// 무효화한다.** `last_bounds`는 그 앞의 값싼 조기 반환을 맡는다(리사이즈
    /// 판정이 매 이벤트마다 돌기 때문이다).
    pub(crate) fn resize_to_emit(
        &mut self,
        bounds: Size,
        metrics: CellMetrics,
    ) -> Option<GridSize> {
        if self.last_bounds == Some(bounds) {
            return None;
        }
        self.last_bounds = Some(bounds);

        match grid_size(bounds, metrics) {
            // 접혔다. 다음에 유효한 크기가 오면 그것이 무엇이든 반드시 발행된다.
            None => {
                self.last_emitted = None;
                None
            }
            Some(size) if self.last_emitted == Some(size) => None,
            Some(size) => {
                self.last_emitted = Some(size);
                Some(size)
            }
        }
    }

    /// 측정이 바뀌면 리사이즈 판정을 처음부터 다시 시켜야 한다 — 같은 bounds라도
    /// 셀 크기가 달라지면 그리드 크기가 달라진다.
    ///
    /// **이 경로는 헤드리스로 검증할 수 없다**: `()` 렌더러는 측정을 못 하므로
    /// 메트릭이 바뀌는 상황 자체를 만들 수 없다.
    pub fn set_metrics(&mut self, metrics: CellMetrics) {
        if self.metrics != Some(metrics) {
            self.metrics = Some(metrics);
            self.last_bounds = None;
        }
    }
}

/// **포커스는 렌더링·게이팅용이지 권위가 아니다**(플랜 0.9).
/// `Focusable::focus`/`unfocus`는 `Shell`도 메시지 채널도 받지 않아 바이트를
/// 발행할 수 없다(`iced_core/src/widget/operation/focusable.rs:7-16`). 따라서
/// **여기서 `FOCUS_IN_OUT` 리포트를 낼 방법이 없고, 내려고 해서도 안 된다** —
/// 포커스 전환과 그에 딸린 바이트는 앱이 소유한다. 이 impl이 하는 일은
/// `operation::focus(id)`가 위젯 상태의 플래그를 뒤집게 해주는 것뿐이다.
impl operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
    }
}

/// 리사이즈가 필요하면 커맨드를 발행한다. `Widget::update`가 **모든 이벤트마다**
/// 부른다 — 리사이즈는 레이아웃 사실이라 포커스에도 커서 위치에도 걸리지 않는다.
///
/// 메트릭이 아직 없으면(측정 실패) 아무것도 하지 않는다. 그리드 크기를 모르는
/// 채로 추측한 값을 PTY에 보내는 것보다 낫다.
pub(crate) fn emit_resize(
    state: &mut State,
    id: SessionId,
    bounds: Size,
    shell: &mut iced::advanced::Shell<'_, (SessionId, TermCommand)>,
) {
    let Some(metrics) = state.metrics else {
        return;
    };
    let Some(size) = state.resize_to_emit(bounds, metrics) else {
        return;
    };

    // 반올림은 여기 한 번뿐이다. `grid_size`가 이미 `u16` 범위를 보장했다.
    shell.publish((
        id,
        TermCommand::Resize {
            rows: size.rows as u16,
            cols: size.cols as u16,
            seq: next_resize_seq(),
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 실제 값으로 짠 메트릭. 폭 8.0 / 높이 16.0은 **이진수로 정확히
    /// 표현된다** — "딱 떨어지는 크기" 케이스가 부동소수 오차 때문에 한 칸
    /// 모자라게 나오는 것을 피하려고 고른 값이다(예컨대 7.8은 정확하지 않아
    /// `780.0 / 7.8`이 100 아래로 떨어질 수 있다).
    fn m() -> CellMetrics {
        CellMetrics::new(8.0, 16.0).expect("8x16 must be valid metrics")
    }

    // ------------------------------------------------------------ CellMetrics

    #[test]
    fn cell_metrics_accepts_only_finite_positive_pairs() {
        assert!(CellMetrics::new(8.0, 16.0).is_some());
        assert!(CellMetrics::new(0.001, 0.001).is_some());

        for (w, h) in [
            (0.0, 16.0),
            (8.0, 0.0),
            (-8.0, 16.0),
            (8.0, -16.0),
            (f32::NAN, 16.0),
            (8.0, f32::NAN),
            (f32::INFINITY, 16.0),
            (8.0, f32::INFINITY),
            (f32::NEG_INFINITY, 16.0),
        ] {
            assert!(
                CellMetrics::new(w, h).is_none(),
                "CellMetrics::new({w}, {h}) must be rejected"
            );
        }
    }

    // -------------------------------------------------------------- grid_size

    #[test]
    fn grid_size_table() {
        // (bounds, 기대값, 왜 이 케이스인가)
        let cases: &[(Size, Option<GridSize>, &str)] = &[
            (
                Size::new(800.0, 400.0),
                Some(GridSize {
                    rows: 25,
                    cols: 100,
                }),
                "exact fit — 800/8 = 100, 400/16 = 25",
            ),
            (
                Size::new(803.0, 407.0),
                Some(GridSize {
                    rows: 25,
                    cols: 100,
                }),
                "remainder is discarded, not rounded up",
            ),
            (
                Size::new(807.9, 415.9),
                Some(GridSize {
                    rows: 25,
                    cols: 100,
                }),
                "remainder just short of one more cell",
            ),
            (
                Size::new(808.0, 416.0),
                Some(GridSize {
                    rows: 26,
                    cols: 101,
                }),
                "exactly one more cell fits",
            ),
            (
                Size::new(7.9, 400.0),
                None,
                "less than one column — 0 cols must not be a grid",
            ),
            (
                Size::new(800.0, 15.9),
                None,
                "less than one row — 0 rows must not be a grid",
            ),
            (Size::new(0.0, 0.0), None, "collapsed pane"),
            (
                Size::new(-800.0, 400.0),
                None,
                "negative width — an unchecked cast would saturate to 0",
            ),
            (Size::new(800.0, -400.0), None, "negative height"),
            (Size::new(f32::NAN, 400.0), None, "NaN width"),
            (Size::new(800.0, f32::NAN), None, "NaN height"),
            (Size::new(f32::INFINITY, 400.0), None, "infinite width"),
            (Size::new(800.0, f32::INFINITY), None, "infinite height"),
            (
                Size::new(8.0, 16.0),
                Some(GridSize { rows: 1, cols: 1 }),
                "the smallest grid that exists",
            ),
            (
                // 65535 * 8.0 = 524280.0
                Size::new(524_280.0, 400.0),
                Some(GridSize {
                    rows: 25,
                    cols: 65_535,
                }),
                "u16::MAX columns is still valid — the boundary is inclusive",
            ),
            (
                // 65536 * 8.0 = 524288.0
                Size::new(524_288.0, 400.0),
                None,
                "one column past u16::MAX — an unchecked cast to u16 would \
                 silently saturate back to 65535",
            ),
            (
                // 65536 * 16.0 = 1048576.0
                Size::new(800.0, 1_048_576.0),
                None,
                "one row past u16::MAX",
            ),
        ];

        for (bounds, expected, why) in cases {
            assert_eq!(
                grid_size(*bounds, m()),
                *expected,
                "grid_size({bounds:?}) — {why}"
            );
        }
    }

    #[test]
    fn grid_size_respects_the_metrics_it_is_given() {
        // 대조군: 같은 bounds라도 셀이 커지면 그리드가 작아진다. 이게 없으면
        // 위의 표가 메트릭을 실제로 쓰는지(상수를 돌려주는 게 아닌지) 알 수 없다.
        let bounds = Size::new(800.0, 400.0);
        let small = CellMetrics::new(8.0, 16.0).unwrap();
        let large = CellMetrics::new(16.0, 32.0).unwrap();

        assert_eq!(
            grid_size(bounds, small),
            Some(GridSize {
                rows: 25,
                cols: 100
            })
        );
        assert_eq!(
            grid_size(bounds, large),
            Some(GridSize { rows: 12, cols: 50 })
        );
    }

    // --------------------------------------------------------------- hit_test

    fn grid() -> GridSize {
        GridSize {
            rows: 25,
            cols: 100,
        }
    }

    /// `ViewportHit`를 통째로 비교하지 않고 분해해서 보는 이유: 기대값을
    /// 리터럴로 적으면 표가 필드 이름으로 뒤덮여 경계 조건이 안 보인다.
    type Hit = Option<(usize, usize, Side)>;

    #[test]
    fn hit_test_table() {
        let cases: &[(Point, Hit, &str)] = &[
            (
                Point::new(0.0, 0.0),
                Some((0, 0, Side::Left)),
                "origin is the left half of cell (0,0)",
            ),
            (
                Point::new(3.9, 0.0),
                Some((0, 0, Side::Left)),
                "just short of the cell midpoint",
            ),
            (
                Point::new(4.0, 0.0),
                Some((0, 0, Side::Right)),
                "exactly the midpoint belongs to the right half",
            ),
            (
                Point::new(7.9, 15.9),
                Some((0, 0, Side::Right)),
                "last pixel of cell (0,0)",
            ),
            (
                Point::new(8.0, 16.0),
                Some((1, 1, Side::Left)),
                "first pixel of cell (1,1)",
            ),
            (
                Point::new(799.9, 399.9),
                Some((24, 99, Side::Right)),
                "last pixel inside the grid",
            ),
            (
                Point::new(800.0, 300.0),
                None,
                "one pixel past the right edge — col == cols is NOT a cell",
            ),
            (
                Point::new(400.0, 400.0),
                None,
                "one pixel past the bottom edge — row == rows is NOT a cell",
            ),
            (
                Point::new(-0.1, 300.0),
                None,
                "negative x — an unchecked cast would saturate to column 0",
            ),
            (
                Point::new(400.0, -0.1),
                None,
                "negative y — an unchecked cast would saturate to row 0",
            ),
            (
                Point::new(-8000.0, -8000.0),
                None,
                "far outside — a drag dragged out of the window",
            ),
            (Point::new(f32::NAN, 300.0), None, "NaN x"),
            (Point::new(400.0, f32::NAN), None, "NaN y"),
            (Point::new(f32::INFINITY, 300.0), None, "infinite x"),
            (Point::new(400.0, f32::INFINITY), None, "infinite y"),
        ];

        for (pos, expected, why) in cases {
            let got = hit_test(*pos, m(), grid()).map(|h| (h.row, h.col, h.side));
            assert_eq!(got, *expected, "hit_test({pos:?}) — {why}");
        }
    }

    #[test]
    fn hit_test_side_splits_every_cell_at_its_midpoint() {
        // 대조군: 셀 (0,0)뿐 아니라 임의의 셀에서도 절반에서 갈리는지.
        // 첫 셀만 보면 `pos.x < 4.0` 같은 구현도 통과한다.
        let left = hit_test(Point::new(403.9, 0.0), m(), grid()).unwrap();
        let right = hit_test(Point::new(404.0, 0.0), m(), grid()).unwrap();
        assert_eq!((left.col, left.side), (50, Side::Left));
        assert_eq!((right.col, right.side), (50, Side::Right));
    }

    #[test]
    fn hit_test_bounds_follow_the_grid_it_is_given() {
        // 대조군: 그리드가 작으면 같은 좌표가 밖으로 나간다.
        let pos = Point::new(200.0, 200.0);
        assert!(hit_test(pos, m(), grid()).is_some());
        assert!(hit_test(pos, m(), GridSize { rows: 5, cols: 5 }).is_none());
    }

    // ----------------------------------------------------- 리사이즈 캐시 두 개

    #[test]
    fn a_collapse_and_restore_re_emits_the_same_grid_size() {
        let mut state = State::default();
        let big = Size::new(800.0, 400.0);
        let collapsed = Size::new(800.0, 10.0); // 한 행도 안 들어간다

        assert_eq!(
            state.resize_to_emit(big, m()),
            Some(GridSize {
                rows: 25,
                cols: 100
            }),
            "the first valid layout must emit"
        );
        assert_eq!(
            state.resize_to_emit(big, m()),
            None,
            "control: re-entering the same bounds must NOT emit"
        );
        assert_eq!(
            state.resize_to_emit(collapsed, m()),
            None,
            "a collapsed pane has no grid size to emit"
        );
        assert_eq!(
            state.resize_to_emit(big, m()),
            Some(GridSize {
                rows: 25,
                cols: 100
            }),
            "restoring must emit again — with a single cache this returns None \
             and the PTY keeps whatever size it had before the collapse"
        );
    }

    /// `last_emitted`가 하중을 받는 지점: bounds는 바뀌었는데 그리드 크기는
    /// 그대로인 경우. 분할선을 몇 픽셀 끄는 것이 정확히 이 경우이고,
    /// `last_bounds`만으로는 걸러지지 않는다 — 걸러지지 않으면 드래그하는 동안
    /// 프레임마다 리사이즈가 나가 워커가 블로킹 리사이즈로 도배된다.
    #[test]
    fn a_sub_cell_bounds_change_does_not_re_emit() {
        let mut state = State::default();

        assert_eq!(
            state.resize_to_emit(Size::new(800.0, 400.0), m()),
            Some(GridSize {
                rows: 25,
                cols: 100
            })
        );
        assert_eq!(
            state.resize_to_emit(Size::new(803.0, 407.0), m()),
            None,
            "different bounds, same grid — nothing to tell the PTY"
        );
        assert_eq!(
            state.resize_to_emit(Size::new(808.0, 416.0), m()),
            Some(GridSize {
                rows: 26,
                cols: 101
            }),
            "control: once the bounds cross a cell boundary it must emit again"
        );
    }

    /// `last_bounds`가 하중을 받는 유일한 지점. bounds가 그대로여도 셀 크기가
    /// 바뀌면 그리드 크기가 달라지므로, `set_metrics`가 bounds 캐시를
    /// 무효화하지 않으면 조기 반환이 그 변화를 통째로 삼킨다.
    ///
    /// 이 경로를 **헤드리스로는 만들 수 없다** — `()` 렌더러가 측정을 못 해
    /// 메트릭이 바뀌는 상황 자체가 생기지 않는다. 그래서 여기 유닛 테스트로 둔다.
    #[test]
    fn changing_the_cell_metrics_re_emits_at_the_same_bounds() {
        let mut state = State::default();
        let bounds = Size::new(800.0, 400.0);
        let bigger = CellMetrics::new(16.0, 32.0).expect("16x32 must be valid metrics");

        state.set_metrics(m());
        assert_eq!(
            state.resize_to_emit(bounds, m()),
            Some(GridSize {
                rows: 25,
                cols: 100
            })
        );

        state.set_metrics(bigger);
        assert_eq!(
            state.resize_to_emit(bounds, bigger),
            Some(GridSize { rows: 12, cols: 50 }),
            "same bounds but bigger cells is a different grid — the bounds \
             early-out must not swallow it"
        );
    }

    #[test]
    fn re_setting_the_same_metrics_does_not_disturb_the_bounds_cache() {
        // 대조군: 위 테스트가 "메트릭을 건드리면 무조건 다시 발행"으로도
        // 통과하지 않게 한다.
        let mut state = State::default();
        let bounds = Size::new(800.0, 400.0);

        state.set_metrics(m());
        assert!(state.resize_to_emit(bounds, m()).is_some());

        state.set_metrics(m());
        assert_eq!(
            state.resize_to_emit(bounds, m()),
            None,
            "an unchanged measurement must not re-open the resize path"
        );
    }
}
