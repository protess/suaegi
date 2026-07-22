use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{viewport_to_point, Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor};

use crate::encode;
use crate::input_types::{
    CopyRequest, CopyTargets, GridMouseResult, KeyInput, MouseAction, MouseEncodeError,
    MouseIntent, MouseRoute, PointerLatch, TermMouseButton,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub rows: usize,
    pub cols: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotCell {
    pub c: char,
    /// 결합 문자(zero-width). `c`만 그리면 결합 문자가 있는 텍스트가 깨진다.
    pub combining: Vec<char>,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    /// `Term::cursor_style().blinking`. `RenderableCursor`에는 shape만 있어서
    /// 따로 읽는다 — 같은 락 안에서.
    pub blinking: bool,
}

/// 뷰포트로 **미리 잘라낸** 선택 영역. `(row, col)` 순서이고 **양 끝을 포함한다**.
///
/// 렌더러가 매 셀마다 교차 판정을 하는 것보다 스냅샷을 만드는 쪽이 락 안에서
/// 한 번 잘라내는 편이 싸고, 렌더러를 단순하게 유지한다. 좌표는 이미
/// `display_offset`이 반영된 **뷰포트 좌표**다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportSelection {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub is_block: bool,
}

/// 락 없이 렌더링할 수 있는 뷰포트 사본. 스크롤백 전체를 복사하지 않는다 —
/// 출력마다 수 MB를 복사하면 이 프로젝트의 존재 이유(성능)와 충돌한다.
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    pub rows: Vec<Vec<SnapshotCell>>,
    pub size: GridSize,
    /// 커서가 표시 중인 뷰포트 안에 있을 때만 Some (스크롤백을 올려보면 None)
    pub cursor: Option<SnapshotCursor>,
    pub display_offset: usize,
    pub history_size: usize,
    /// **렌더링 전용이다.** 입력 경로가 이 값을 읽으면 리뷰에서 반려한다 —
    /// 스냅샷은 비동기라 낡았고, `feed()`가 최대 64KiB 청크를 락 쥔 채
    /// 처리하므로 청크 중간에 켜진 모드는 청크가 끝나야 여기 반영된다.
    /// 인코딩 판단은 전부 `TerminalGrid`가 term 락을 쥔 채 한다.
    pub mode: TermMode,
    /// 뷰포트로 잘라낸 선택 영역. 교차 후 남는 행이 없으면 `None`.
    pub selection: Option<ViewportSelection>,
}

impl TerminalSnapshot {
    pub fn row_text(&self, row: usize) -> String {
        match self.rows.get(row) {
            Some(cells) => cells
                .iter()
                .map(|c| c.c)
                .collect::<String>()
                .trim_end()
                .to_string(),
            None => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TitleChange {
    Set(String),
    Reset,
}

/// `title_changes` 상한. `pty_writes`와 달리 리더가 매 피드마다 자동으로 비우지
/// 않는다 — 오직 UI가 `take_title_changes`를 불러야 비워진다. UI가 폴링을
/// 멈추거나 죽은 채로 세션이 계속 살아 있으면, 타이틀 이스케이프를 반복하는
/// 자식(예: 프롬프트마다 타이틀을 바꾸는 셸)이 이 벡터를 무한정 키운다. 256개면
/// 정상 동작(보통 프레임마다 비워 한 자릿수만 쌓임)보다 훨씬 넉넉하면서도
/// 상한을 유지한다. 초과분은 **가장 오래된 것부터** 버린다 — UI가 결국 다시
/// 폴링하기 시작하면 보게 될 값은 최신 타이틀이므로, 보존할 가치가 있는 건
/// 오래된 기록이 아니라 최근 변경들이다.
pub const TITLE_CHANGES_CAPACITY: usize = 256;

/// 터미널의 부수 효과를 모으는 공유 상태. `send_event`가 `&self`라서 내부는 Mutex.
#[derive(Debug, Default)]
struct GridEventState {
    pty_writes: Mutex<Vec<String>>,
    title_changes: Mutex<VecDeque<TitleChange>>,
}

/// 로컬 뉴타입. `impl EventListener for Arc<..>`는 외래 트레이트 + 외래 타입이라
/// 고아 규칙에 걸린다 — 우리 타입에 구현하고 Clone으로 상태를 공유한다.
/// Term은 프록시 접근자를 제공하지 않으므로 TerminalGrid도 같은 클론을 보관한다.
#[derive(Debug, Clone, Default)]
pub struct GridEventProxy(Arc<GridEventState>);

impl EventListener for GridEventProxy {
    fn send_event(&self, event: Event) {
        match event {
            // 장치 질의 응답 등 — PTY로 되돌려 쓰지 않으면 질의한 프로그램이 멈춘다
            Event::PtyWrite(text) => {
                self.0
                    .pty_writes
                    .lock()
                    .expect("pty write mutex")
                    .push(text);
            }
            // 빈 타이틀은 리셋이다 — Set("")로 두면 UI가 이전 타이틀을 지울지
            // 빈 문자열을 표시할지 구분할 수 없다
            Event::Title(title) => {
                let change = if title.is_empty() {
                    TitleChange::Reset
                } else {
                    TitleChange::Set(title)
                };
                self.push_title_change(change);
            }
            Event::ResetTitle => {
                self.push_title_change(TitleChange::Reset);
            }
            _ => {}
        }
    }
}

impl GridEventProxy {
    /// `TITLE_CHANGES_CAPACITY`를 넘으면 가장 오래된 항목부터 버린다 —
    /// 근거는 상수 선언부의 주석 참고.
    fn push_title_change(&self, change: TitleChange) {
        let mut changes = self.0.title_changes.lock().expect("title mutex");
        changes.push_back(change);
        if changes.len() > TITLE_CHANGES_CAPACITY {
            changes.pop_front();
        }
    }
}

/// `FairMutex`의 페이로드. `Term`만 감싸면 포인터 라우팅 상태와 선택 버전을
/// `&self` 메서드에서 바꿀 수 없다. 두 번째 뮤텍스를 쓰면 "라우팅·좌표 변환·
/// 변경을 **한 락 안에서**"라는 불변식이 깨지고 락 순서를 증명해야 한다 —
/// 페이로드를 바꾸는 쪽이 불변식과 일치한다.
struct GridState {
    term: Term<GridEventProxy>,
    /// press에서 래치하고 release에서 해제한다. 드래그 도중 모드가 바뀌어도 한
    /// 제스처가 반으로 갈리지 않게 한다.
    // 이 필드를 실제로 쓰는 것은 `handle_mouse`(마우스 태스크)다.
    #[allow(dead_code)]
    pointer: Option<PointerLatch>,
    /// 선택 **버전**이다(제스처 ID가 아니다) — alacritty 자신이 `feed`·스크롤·
    /// 리사이즈 중에 선택을 회전·클리핑·삭제한다. 비동기 추출이 그 사이 달라진
    /// 선택을 복사하는 것을 이 값으로 막는다.
    selection_epoch: u64,
    last_seen_selection: Option<SelectionRange>,
}

/// 락을 쥔 채 현재 선택 범위를 읽는다. `Selection::to_range`가 `&Term`을
/// 요구하므로 락 밖에서는 부를 수 없다.
fn selection_range(term: &Term<GridEventProxy>) -> Option<SelectionRange> {
    term.selection.as_ref().and_then(|s| s.to_range(term))
}

/// 범위가 달라졌으면 선택 버전을 올린다. **마우스 갱신과 구조적 변화**
/// (회전·클리핑·삭제)에 쓴다. `feed` 뒤에는 이걸 쓰면 안 된다 —
/// [`bump_after_feed`]의 주석 참고.
fn bump_if_selection_changed(state: &mut GridState) {
    let current = selection_range(&state.term);
    if current != state.last_seen_selection {
        state.selection_epoch += 1;
        state.last_seen_selection = current;
    }
}

/// `feed` 전용. **범위 비교만으로는 부족하다** — 범위가 그대로여도 그 안의 셀
/// 내용을 출력이 덮어쓸 수 있고, 그러면 나중에 추출한 텍스트가 사용자가 선택할
/// 때 보던 것과 달라진다. 따라서 선택이 존재하기만 하면 범위 변화와 무관하게
/// **무조건** 올린다(보수적). 선택이 없다가 없는 채로 끝난 경우에만 올리지
/// 않는다 — 그때는 무효화할 대상 자체가 없다.
fn bump_after_feed(state: &mut GridState) {
    let current = selection_range(&state.term);
    if current.is_some() || state.last_seen_selection.is_some() {
        state.selection_epoch += 1;
    }
    state.last_seen_selection = current;
}

/// 이번 intent가 실제로 실행할 라우트를 정하고 래치를 갱신한다.
///
/// **위젯과 그리드는 이벤트 스트림으로만 이어져 있다.** 위젯 상태는 `Tree::diff`가
/// 서브트리를 재생성할 때 조용히 리셋되고(`terminal/state.rs`의 `RESIZE_SEQ` 주석),
/// 그리드의 래치는 세션에 그대로 남는다. 둘을 맞춰주는 시퀀스 번호도 핸드셰이크도
/// 없으므로 **양쪽은 독립적으로 어긋날 수 있다.**
///
/// 그래서 규칙 하나로 못박는다: **제스처 수명주기의 권위는 위젯이고, 래치는 그
/// 파생 상태다.**
///
/// | intent | 래치 |
/// |--------|------|
/// | `Press` — 제스처의 시작 | **언제나** 새로 잡는다 |
/// | `Release` — 제스처의 끝 | **언제나** 푼다 |
/// | `Motion`인데 `held`가 없음 | 어긋난 것이다 → 푼다 |
/// | `Motion`인데 `held`가 있음 | 래치된 라우트를 따른다(제스처가 갈리지 않게) |
/// | `Wheel` | 읽지도 쓰지도 않는다 — 매번 라이브 판정(플랜 0.4) |
///
/// **세 경우 모두 프로덕션에서 모호하지 않다.** `route_mouse`는 `Press(b)`에
/// `held == Some(b)`를 요구하고 `press_intent`는 이미 눌린 것이 있으면 `None`을
/// 돌려준다 → **위젯은 코드 클릭 press를 만들어낼 수 없다.** 따라서 다른 버튼의
/// press가 살아 있는 래치 위로 들어왔다면 그것은 코드 클릭이 아니라 **위젯이
/// 기억을 잃었다는 증거**이고, 그때 낡은 래치를 지키는 것은 증거에 대한 잘못된
/// 응답이다. (코드 클릭 억제는 위젯 쪽 `press_intent`가 맡는다 — 거기서는 코드
/// 클릭이 실재하므로 두 층의 정책이 반대인 것이 맞다.)
fn resolve_route(state: &mut GridState, intent: &MouseIntent, live: MouseRoute) -> MouseRoute {
    match intent.action {
        // 휠은 래치에 참여하지 않는다 — 드래그 중 휠을 굴리는 TUI가 래치 때문에
        // 리포트를 못 받으면 안 된다.
        MouseAction::Wheel { .. } => live,
        // 제스처의 시작. 언제나 라이브 모드로 다시 판정해 새로 잡는다.
        MouseAction::Press(button) => {
            state.pointer = Some(PointerLatch {
                button,
                route: live,
            });
            live
        }
        MouseAction::Motion => match state.pointer {
            // 정상 드래그 — 래치된 라우트를 따라 제스처가 반으로 갈리지 않게 한다.
            Some(latch) if intent.held.is_some() => latch.route,
            // 버튼이 눌리지 않았는데 래치가 있다 = 위젯이 기억을 잃었다.
            // 마우스는 쉬지 않고 움직이므로 이 갈래가 가장 빨리 복구시킨다.
            Some(_) => {
                state.pointer = None;
                live
            }
            None => live,
        },
        // 제스처의 끝. 래치는 언제나 푼다 — 버튼이 어긋나면 어긋난 쪽이 낡은
        // 것이므로, 붙들고 있으면 `request_copy`가 계속 `None`을 돌려준다.
        MouseAction::Release(button) => match state.pointer {
            Some(latch) => {
                state.pointer = None;
                if latch.button == button {
                    latch.route
                } else {
                    live
                }
            }
            None => live,
        },
    }
}

/// 로컬 선택 제스처를 그리드에 반영한다. 반환값은 "다시 그려야 하는가".
///
/// 좌표 변환이 **여기서** 일어나는 것이 핵심이다 — `display_offset`을 읽는 것과
/// 그 좌표로 선택을 바꾸는 것 사이에 락을 놓지 않는다.
fn apply_local_selection(state: &mut GridState, intent: &MouseIntent, ty: SelectionType) -> bool {
    let display_offset = state.term.grid().display_offset();
    let point = viewport_to_point(
        display_offset,
        Point::new(intent.hit.row, Column(intent.hit.col)),
    );

    match intent.action {
        MouseAction::Press(TermMouseButton::Left) => {
            state.term.selection = Some(Selection::new(ty, point, intent.hit.side));
            true
        }
        MouseAction::Motion | MouseAction::Release(TermMouseButton::Left) => {
            match state.term.selection.as_mut() {
                Some(selection) => {
                    selection.update(point, intent.hit.side);
                    true
                }
                // press를 못 본 채로 온 move/release — 선택을 새로 만들지 않는다.
                None => false,
            }
        }
        _ => false,
    }
}

/// 로컬 **선택** 제스처가 진행 중인가. 리포트로 래치된 드래그는 선택을 건드리지
/// 않으므로 여기 해당하지 않는다 — 그때까지 복사를 막으면 마우스 모드 TUI 위에서
/// 우클릭을 붙들고 있는 동안 단축키 복사가 통째로 죽는다.
fn local_selection_in_progress(state: &GridState) -> bool {
    matches!(
        state.pointer,
        Some(PointerLatch {
            route: MouseRoute::LocalSelect(_),
            ..
        })
    )
}

/// 그리드 좌표의 선택 범위를 뷰포트로 잘라낸다. **양 끝을 포함**하며 선형과
/// 블록의 규칙이 다르다:
///
/// - **선형**: 위로 넘치면 시작을 `(0, 0)`, 아래로 넘치면 끝을
///   `(rows-1, cols-1)`로 민다. 잘리지 않은 끝의 열은 **보존한다**.
/// - **블록**: **행만 자르고 열 범위는 보존한다** — 직사각형이라는 것이 블록의
///   정의이므로 열을 경계로 밀면 모양이 망가진다. 열은 클램프만 한다.
/// - **`None`**: 교차 후 남는 행이 없을 때(끝이 뷰포트 위, 또는 시작이 아래).
fn clip_selection(
    range: SelectionRange,
    display_offset: usize,
    rows_len: usize,
    cols_len: usize,
) -> Option<ViewportSelection> {
    if rows_len == 0 || cols_len == 0 {
        return None;
    }
    let last_row = rows_len as i32 - 1;
    let last_col = cols_len - 1;

    // 행·커서에 이미 하는 것과 같은 보정 — 스크롤백 줄은 음수 Line으로 나온다
    let start_row = range.start.line.0 + display_offset as i32;
    let end_row = range.end.line.0 + display_offset as i32;
    if end_row < 0 || start_row > last_row {
        return None;
    }

    let start_col = range.start.column.0.min(last_col);
    let end_col = range.end.column.0.min(last_col);

    let (start, end) = if range.is_block {
        (
            (start_row.max(0) as usize, start_col),
            (end_row.min(last_row) as usize, end_col),
        )
    } else {
        let start = if start_row < 0 {
            (0, 0)
        } else {
            (start_row as usize, start_col)
        };
        let end = if end_row > last_row {
            (rows_len - 1, last_col)
        } else {
            (end_row as usize, end_col)
        };
        (start, end)
    };

    Some(ViewportSelection {
        start,
        end,
        is_block: range.is_block,
    })
}

pub struct TerminalGrid {
    state: FairMutex<GridState>,
    parser: Mutex<Processor>,
    proxy: GridEventProxy,
}

impl TerminalGrid {
    pub fn new(size: GridSize, scrollback: usize) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let proxy = GridEventProxy::default();
        // 스크롤백은 생성 시 고정된다 — 바꾸려면 Term을 새로 만들어야 한다
        let term = Term::new(config, &size, proxy.clone());
        // 갓 만든 Term의 실제 값으로 초기화한다(보통 None). 상수 None으로 두면
        // 언젠가 선택을 들고 태어나는 생성 경로가 생겼을 때 첫 연산이 헛되이
        // 버전을 올린다.
        let last_seen_selection = selection_range(&term);
        Self {
            state: FairMutex::new(GridState {
                term,
                pointer: None,
                selection_epoch: 0,
                last_seen_selection,
            }),
            parser: Mutex::new(Processor::new()),
            proxy,
        }
    }

    /// PTY에서 읽은 바이트를 그리드에 반영한다. 부분 UTF-8은 파서가 유지하므로
    /// 청크 경계를 호출자가 맞출 필요는 없다.
    pub fn feed(&self, bytes: &[u8]) {
        let mut state = self.state.lock();
        let mut parser = self.parser.lock().expect("parser mutex");
        parser.advance(&mut state.term, bytes);
        drop(parser);
        bump_after_feed(&mut state);
    }

    pub fn resize(&self, size: GridSize) {
        let mut state = self.state.lock();
        state.term.resize(size);
        // 리사이즈는 선택을 클리핑하거나 지운다
        bump_if_selection_changed(&mut state);
    }

    /// 선택 버전. 추출 요청에 실려 나가 stale 추출을 막는다.
    #[doc(hidden)]
    pub fn selection_epoch(&self) -> u64 {
        self.state.lock().selection_epoch
    }

    // -----------------------------------------------------------------------
    // intent 메서드 — 락을 안에서 잡고 놓고, **바이트를 돌려준다**
    //
    // 어느 것도 쓰기 큐를 만지지 않는다. 큐를 여기 넣으면 term 락을 쥔 채 큐
    // 락을 잡게 되어 락 중첩이 생긴다. 큐잉은 `TerminalSession`이 한다.
    //
    // 모드를 **락 안에서 진짜 값으로** 읽는 것이 이 메서드들의 존재 이유다 —
    // 어떤 모드 캐시도 correctness에 쓸 수 없다(플랜 0.3).
    // -----------------------------------------------------------------------

    pub fn encode_key_locked(&self, input: &KeyInput) -> Option<Vec<u8>> {
        let state = self.state.lock();
        encode::encode_key(input, *state.term.mode())
    }

    pub fn encode_paste_locked(&self, text: &str) -> Vec<u8> {
        let state = self.state.lock();
        encode::encode_paste(text, *state.term.mode())
    }

    /// 지금 `BRACKETED_PASTE` 모드인가. 프롬프트 주입 게이트(app)가 composer가
    /// 준비됐는지 판단하는 전제로 쓴다 — 값싼 락 한 번(스냅샷 190KB를 뜨지 않는다).
    pub fn bracketed_paste_enabled(&self) -> bool {
        let state = self.state.lock();
        state.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn encode_focus_locked(&self, focused: bool) -> Option<Vec<u8>> {
        let state = self.state.lock();
        encode::encode_focus(focused, *state.term.mode())
    }

    /// 마우스 intent를 처리한다. **라우팅·좌표 변환·선택 변경·인코딩을 전부 이
    /// 한 번의 락 안에서** 끝낸다.
    ///
    /// 나누면 안 되는 이유: 라우팅만 락 안에서 하고 결과를 앱에 돌려줘 앱이 다시
    /// 선택 변경을 부르면, **두 번째 락에서 `display_offset`이 이미 달라져 있을
    /// 수 있다.** 그러면 사용자가 가리킨 셀이 아닌 곳이 선택된다. 이 레이스는
    /// 교차검증에서 두 번 재발했다.
    pub fn handle_mouse(
        &self,
        intent: &MouseIntent,
    ) -> Result<GridMouseResult, MouseEncodeError> {
        let mut state = self.state.lock();
        let mode = *state.term.mode();

        // 라이브 라우팅은 **언제나** 부른다 — 래치를 쓸 때도 마찬가지다. 이
        // 호출이 held 전이 표의 불변식 검사를 겸하므로, 건너뛰면 위젯의 held
        // 버그가 조용히 통과한다.
        let live = encode::route_mouse(intent, mode)?;
        let route = resolve_route(&mut state, intent, live);

        let mut redraw = false;
        let mut copy = None;
        let mut bytes = None;

        match route {
            MouseRoute::Report | MouseRoute::AltScreenArrows => {
                bytes = encode::encode_mouse(&route, intent, mode);
            }
            MouseRoute::LocalScroll => {
                if let MouseAction::Wheel { lines } = intent.action {
                    state.term.scroll_display(Scroll::Delta(lines));
                    redraw = true;
                }
            }
            MouseRoute::LocalSelect(ty) => {
                redraw = apply_local_selection(&mut state, intent, ty);
                // 드래그 완료 시점에만 복사한다. 만드는 중에 복사하면 워커가
                // 추출하기 전에 다음 move가 범위를 바꾼다.
                if matches!(intent.action, MouseAction::Release(_))
                    && state.term.selection.is_some()
                {
                    copy = Some(CopyTargets::DRAG_COMPLETE);
                }
            }
            MouseRoute::Ignore => {}
        }

        // 선택을 바꿀 수 있는 경로가 전부 위를 지났다 — 여기서 한 번만 올린다.
        bump_if_selection_changed(&mut state);

        Ok(GridMouseResult {
            bytes,
            redraw,
            // epoch는 **bump 뒤에** 읽는다. 먼저 읽으면 방금 만든 선택이 옛
            // 버전으로 나가 추출이 항상 불일치로 거절된다.
            copy: copy.map(|to| CopyRequest {
                epoch: state.selection_epoch,
                to,
            }),
        })
    }

    /// 선택 텍스트를 뜬다. **읽기 전용이다** — 선택을 지우지 않는다.
    ///
    /// `epoch`가 지금 버전과 다르면 **아무것도 하지 않고 `None`**. 락을 잡은 뒤
    /// 비교하므로 read-then-use 창이 없다.
    pub fn extract_selection(&self, epoch: u64) -> Option<String> {
        let state = self.state.lock();
        if state.selection_epoch != epoch {
            return None;
        }
        state.term.selection_to_string()
    }

    /// 명시적 복사(단축키)의 요청을 만든다. 현재 epoch를 **락 안에서** 실어
    /// 준다 — 따로 노출해 앱이 읽게 하면 read-then-use 레이스가 생긴다.
    ///
    /// 선택이 없으면 `None`. **선택을 만드는 중(로컬 포인터 래치가 살아 있음)
    /// 이어도 `None`** — 아직 만드는 중인 선택을 복사하면, 워커가 추출하기 전에
    /// 다음 move가 범위를 바꿔 **단축키를 누른 시점보다 나중의 범위**가 복사된다.
    pub fn request_copy(&self, to: CopyTargets) -> Option<CopyRequest> {
        let state = self.state.lock();
        if local_selection_in_progress(&state) {
            return None;
        }
        state.term.selection.as_ref()?;
        Some(CopyRequest {
            epoch: state.selection_epoch,
            to,
        })
    }

    /// 표시 중인 뷰포트의 스냅샷. 모든 값을 **같은 락 안에서** 읽어 size와 rows가
    /// 어긋나(리사이즈 경합) 렌더링 중 인덱스를 초과하는 일이 없게 한다.
    ///
    /// 그리드를 `Line(0..screen_lines)`로 직접 인덱싱하지 않고 `display_iter`를
    /// 쓰는 이유: 사용자가 스크롤백을 올려본 상태(display_offset > 0)에서 전자는
    /// 항상 최신 화면을 복사해 화면에 보이는 것과 다른 내용을 렌더링하게 된다.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let state = self.state.lock();
        let term = &state.term;
        let rows_len = term.grid().screen_lines();
        let cols_len = term.grid().columns();
        let history_size = term.grid().history_size();
        // `RenderableCursor`는 shape만 나른다 — blinking은 여기서 따로, 같은 락 안에서.
        let cursor_blinking = term.cursor_style().blinking;

        let content = term.renderable_content();
        let display_offset = content.display_offset;
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        let mode = content.mode;
        let selection = content
            .selection
            .and_then(|range| clip_selection(range, display_offset, rows_len, cols_len));

        let mut rows: Vec<Vec<SnapshotCell>> = vec![Vec::with_capacity(cols_len); rows_len];
        // display_iter가 내는 point.line은 **그리드 좌표**다: 스크롤백을 올려보면
        // 히스토리 줄이 음수 Line으로 나온다. 음수를 버리면 화면이 빈 채로 그려지므로
        // display_offset을 더해 0..rows_len의 뷰포트 좌표로 옮긴다.
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + display_offset as i32;
            if row < 0 || row as usize >= rows_len {
                continue;
            }
            let cell = indexed.cell;
            rows[row as usize].push(SnapshotCell {
                c: cell.c,
                // zerowidth()는 Cell의 메서드다 (CellExtra가 아니라)
                combining: cell.zerowidth().unwrap_or_default().to_vec(),
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags,
            });
        }
        // 행 길이를 cols_len으로 맞춰 렌더러가 균일하게 인덱싱할 수 있게 한다
        let blank = SnapshotCell {
            c: ' ',
            combining: Vec::new(),
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        };
        for row in rows.iter_mut() {
            row.resize(cols_len, blank.clone());
        }

        let cursor = {
            // 커서 좌표도 같은 그리드 좌표계다 — 동일하게 뷰포트 좌표로 옮긴다
            let r = cursor_point.line.0 + display_offset as i32;
            let c = cursor_point.column.0;
            if r >= 0 && (r as usize) < rows_len && c < cols_len {
                Some(SnapshotCursor {
                    row: r as usize,
                    col: c,
                    shape: cursor_shape,
                    blinking: cursor_blinking,
                })
            } else {
                // 스크롤백을 올려보는 중이면 커서가 화면 밖일 수 있다
                None
            }
        };

        TerminalSnapshot {
            rows,
            size: GridSize {
                rows: rows_len,
                cols: cols_len,
            },
            cursor,
            display_offset,
            history_size,
            mode,
            selection,
        }
    }

    /// 스크롤백 이동. 스냅샷이 표시 좌표계를 쓰므로 즉시 반영된다.
    /// `Scroll` 전체를 받는다 — shift+PgUp/PgDn과 "키를 누르면 맨 아래로"가
    /// `Delta`로는 표현되지 않는다.
    pub fn scroll_display(&self, scroll: Scroll) {
        let mut state = self.state.lock();
        state.term.scroll_display(scroll);
        // 스크롤은 선택을 회전시키거나 뷰포트 밖으로 밀어낸다
        bump_if_selection_changed(&mut state);
    }

    /// 터미널이 생성한 PTY 응답을 비우고 반환한다. 호출자는 반드시 PTY로 써야 한다.
    pub fn take_pty_writes(&self) -> Vec<String> {
        let mut writes = self.proxy.0.pty_writes.lock().expect("pty write mutex");
        std::mem::take(&mut *writes)
    }

    pub fn take_title_changes(&self) -> Vec<TitleChange> {
        let mut changes = self.proxy.0.title_changes.lock().expect("title mutex");
        std::mem::take(&mut *changes).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Line, Side};
    use crate::input_types::{ClickKind, KeyLocation, Mods, NamedKey, TermKey, ViewportHit};

    fn range(start: (i32, usize), end: (i32, usize), is_block: bool) -> SelectionRange {
        SelectionRange {
            start: Point::new(Line(start.0), Column(start.1)),
            end: Point::new(Line(end.0), Column(end.1)),
            is_block,
        }
    }

    /// 4x10 뷰포트 + **진짜 스크롤백**. `display_offset` 보정을 검증하려면
    /// 히스토리가 실제로 있어야 한다 — 오프셋 0에서만 맞고 N에서 틀린 코드가
    /// 정확히 이 플랜이 잡으려는 버그다.
    ///
    /// 줄마다 **다른 글자**를 쓴다. 전부 같은 모양이면("line0", "line1", …)
    /// 좌표를 한 줄 잘못 잡아도 뽑아낸 텍스트가 똑같아 테스트가 통과한다.
    /// 줄 i는 `(b'a'+i)`를 네 번 반복한 뒤 i를 붙인 것이다: `aaaa0`, `bbbb1`, ….
    fn grid_with_scrollback() -> TerminalGrid {
        let grid = TerminalGrid::new(GridSize { rows: 4, cols: 10 }, 100);
        for i in 0..8u8 {
            let letter = (b'a' + i) as char;
            let line: String = letter.to_string().repeat(4);
            grid.feed(format!("{line}{i}\r\n").as_bytes());
        }
        grid
    }

    fn intent(
        action: MouseAction,
        (row, col): (usize, usize),
        side: Side,
        held: Option<TermMouseButton>,
    ) -> MouseIntent {
        MouseIntent {
            action,
            hit: ViewportHit { row, col, side },
            held,
            mods: Mods::default(),
            click: ClickKind::Single,
            force_local: false,
        }
    }

    /// 좌클릭 드래그 한 벌. 시작은 셀 왼쪽, 끝은 셀 오른쪽에 찍어 양 끝 셀이
    /// 모두 포함되게 한다(`range_simple`이 끝 side를 보고 마지막 셀을 뺀다).
    fn drag(grid: &TerminalGrid, from: (usize, usize), to: (usize, usize)) {
        let left = Some(TermMouseButton::Left);
        grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            from,
            Side::Left,
            left,
        ))
        .expect("press routes");
        grid.handle_mouse(&intent(MouseAction::Motion, to, Side::Right, left))
            .expect("motion routes");
        grid.handle_mouse(&intent(
            MouseAction::Release(TermMouseButton::Left),
            to,
            Side::Right,
            left,
        ))
        .expect("release routes");
    }

    /// 마우스 경로가 할 일을 흉내낸다 — 선택을 세우고 같은 자리에서 버전을 올린다.
    fn set_selection(grid: &TerminalGrid, ty: SelectionType, start: Point, end: Point) {
        let mut state = grid.state.lock();
        let mut selection = Selection::new(ty, start, Side::Left);
        selection.update(end, Side::Right);
        state.term.selection = Some(selection);
        bump_if_selection_changed(&mut state);
    }

    // -----------------------------------------------------------------------
    // clip_selection — 네 방향 교차 + 블록
    // -----------------------------------------------------------------------

    #[test]
    fn clipping_keeps_a_selection_that_lies_entirely_inside_the_viewport() {
        let clipped = clip_selection(range((1, 2), (2, 7), false), 0, 4, 10);
        assert_eq!(
            clipped,
            Some(ViewportSelection {
                start: (1, 2),
                end: (2, 7),
                is_block: false,
            })
        );
    }

    #[test]
    fn clipping_a_linear_selection_that_starts_above_the_viewport_snaps_the_start_to_the_origin() {
        // 위로만 넘친다: 시작은 (0, 0)으로, **잘리지 않은 끝의 열은 보존**된다.
        let clipped = clip_selection(range((-3, 4), (1, 6), false), 0, 4, 10);
        assert_eq!(
            clipped,
            Some(ViewportSelection {
                start: (0, 0),
                end: (1, 6),
                is_block: false,
            })
        );
    }

    #[test]
    fn clipping_a_linear_selection_that_ends_below_the_viewport_snaps_the_end_to_the_last_cell() {
        // 아래로만 넘친다: 끝은 (rows-1, cols-1)으로, 시작의 열은 보존된다.
        let clipped = clip_selection(range((1, 3), (9, 2), false), 0, 4, 10);
        assert_eq!(
            clipped,
            Some(ViewportSelection {
                start: (1, 3),
                end: (3, 9),
                is_block: false,
            })
        );
    }

    #[test]
    fn clipping_a_linear_selection_that_spans_the_whole_viewport_snaps_both_ends() {
        let clipped = clip_selection(range((-5, 4), (9, 2), false), 0, 4, 10);
        assert_eq!(
            clipped,
            Some(ViewportSelection {
                start: (0, 0),
                end: (3, 9),
                is_block: false,
            })
        );
    }

    #[test]
    fn a_selection_entirely_outside_the_viewport_clips_to_none() {
        // 끝이 뷰포트 위 — 남는 행이 없다
        assert_eq!(clip_selection(range((-9, 1), (-5, 8), false), 0, 4, 10), None);
        // 시작이 뷰포트 아래 — 역시 없다
        assert_eq!(clip_selection(range((7, 1), (9, 8), false), 0, 4, 10), None);
        // 대조군: 한 행이라도 걸치면 Some이어야 한다
        assert!(clip_selection(range((-9, 1), (0, 8), false), 0, 4, 10).is_some());
    }

    #[test]
    fn clipping_a_block_selection_cuts_rows_but_preserves_the_column_range() {
        // 블록은 직사각형이라는 것이 정의다 — 열을 경계로 밀면 모양이 망가진다.
        let clipped = clip_selection(range((-2, 3), (9, 6), true), 0, 4, 10);
        assert_eq!(
            clipped,
            Some(ViewportSelection {
                start: (0, 3),
                end: (3, 6),
                is_block: true,
            })
        );
        // 대조군: 같은 범위를 선형으로 자르면 열이 모서리로 밀린다
        assert_eq!(
            clip_selection(range((-2, 3), (9, 6), false), 0, 4, 10),
            Some(ViewportSelection {
                start: (0, 0),
                end: (3, 9),
                is_block: false,
            })
        );
    }

    #[test]
    fn clipping_applies_the_display_offset_before_deciding_what_is_visible() {
        // 오프셋 2면 그리드 Line(-2)가 뷰포트 0행이다. 보정을 빼면 이 선택은
        // 통째로 뷰포트 위로 판정돼 None이 된다.
        let clipped = clip_selection(range((-2, 1), (-1, 4), false), 2, 4, 10);
        assert_eq!(
            clipped,
            Some(ViewportSelection {
                start: (0, 1),
                end: (1, 4),
                is_block: false,
            })
        );
    }

    // -----------------------------------------------------------------------
    // 스냅샷 왕복
    // -----------------------------------------------------------------------

    #[test]
    fn the_snapshot_carries_the_selection_in_viewport_coordinates() {
        let grid = grid_with_scrollback();
        set_selection(
            &grid,
            SelectionType::Simple,
            Point::new(Line(0), Column(1)),
            Point::new(Line(1), Column(3)),
        );

        let snapshot = grid.snapshot();
        assert_eq!(snapshot.display_offset, 0);
        assert_eq!(
            snapshot.selection,
            Some(ViewportSelection {
                start: (0, 1),
                end: (1, 3),
                is_block: false,
            })
        );
    }

    #[test]
    fn the_snapshot_selection_is_corrected_for_a_nonzero_display_offset() {
        let grid = grid_with_scrollback();
        grid.scroll_display(Scroll::Delta(2));

        let before = grid.snapshot();
        // 스크롤백이 진짜로 있고 실제로 올라갔다는 증거 — 이게 없으면 아래
        // 단언이 오프셋 0짜리 우연으로도 통과할 수 있다.
        assert!(before.history_size >= 2, "history: {}", before.history_size);
        assert_eq!(before.display_offset, 2);
        assert_eq!(before.row_text(1), "eeee4");

        // 그리드 Line(-1) = 뷰포트 1행("line4"), Line(0) = 뷰포트 2행("line5").
        set_selection(
            &grid,
            SelectionType::Simple,
            Point::new(Line(-1), Column(2)),
            Point::new(Line(0), Column(5)),
        );

        let snapshot = grid.snapshot();
        assert_eq!(
            snapshot.selection,
            Some(ViewportSelection {
                start: (1, 2),
                end: (2, 5),
                is_block: false,
            })
        );
    }

    #[test]
    fn the_snapshot_carries_the_terminal_mode_and_the_cursor_blink_flag() {
        let grid = TerminalGrid::new(GridSize { rows: 4, cols: 10 }, 100);
        let before = grid.snapshot();
        assert!(!before.mode.contains(TermMode::BRACKETED_PASTE));
        assert_eq!(before.cursor.map(|c| c.blinking), Some(false));

        grid.feed(b"\x1b[?2004h"); // bracketed paste on
        grid.feed(b"\x1b[1 q"); // blinking block cursor

        let after = grid.snapshot();
        assert!(after.mode.contains(TermMode::BRACKETED_PASTE));
        assert_eq!(after.cursor.map(|c| c.blinking), Some(true));
    }

    // -----------------------------------------------------------------------
    // 선택 버전(epoch)
    // -----------------------------------------------------------------------

    #[test]
    fn feeding_output_bumps_the_selection_epoch_even_when_the_range_is_unchanged() {
        let grid = grid_with_scrollback();
        set_selection(
            &grid,
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Point::new(Line(0), Column(4)),
        );

        let (before_epoch, before_range) = {
            let state = grid.state.lock();
            (state.selection_epoch, state.last_seen_selection)
        };
        assert!(before_range.is_some(), "the test needs a live selection");

        // 커서를 홈으로 보내고 선택 **안쪽** 셀을 덮어쓴다. 개행이 없으므로
        // 스크롤이 일어나지 않고 선택 범위는 그대로다.
        grid.feed(b"\x1b[1;1HZZZZZ");

        let (after_epoch, after_range) = {
            let state = grid.state.lock();
            (state.selection_epoch, state.last_seen_selection)
        };

        // 이 단언이 이 테스트의 핵심이다 — 범위가 정말로 그대로여야
        // "범위 비교만으로는 부족하다"를 검증한 것이 된다.
        assert_eq!(
            after_range, before_range,
            "the range must be unchanged for this test to mean anything"
        );
        assert_eq!(grid.snapshot().row_text(0), "ZZZZZ", "output must have landed");
        assert!(
            after_epoch > before_epoch,
            "output overwrote cells inside the selection, so the epoch must advance: \
             {before_epoch} -> {after_epoch}"
        );
    }

    #[test]
    fn feeding_output_with_no_selection_leaves_the_epoch_alone() {
        // 위 테스트의 대조군. 이게 없으면 "무조건 올린다"가 "언제나 올린다"와
        // 구분되지 않는다.
        let grid = grid_with_scrollback();
        let before = grid.state.lock().selection_epoch;

        grid.feed(b"\x1b[1;1HZZZZZ");

        let after = grid.state.lock().selection_epoch;
        assert_eq!(
            after, before,
            "there is nothing to invalidate when no selection exists"
        );
        assert_eq!(grid.snapshot().row_text(0), "ZZZZZ", "output must have landed");
    }

    #[test]
    fn dropping_a_selection_bumps_the_epoch_once_and_then_stops() {
        let grid = grid_with_scrollback();
        set_selection(
            &grid,
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Point::new(Line(0), Column(4)),
        );
        grid.state.lock().term.selection = None;

        let before = grid.state.lock().selection_epoch;
        grid.feed(b"\x1b[1;1HAAAAA");
        let after_drop = grid.state.lock().selection_epoch;
        assert!(
            after_drop > before,
            "the selection disappeared — that is a range change and must bump"
        );

        grid.feed(b"\x1b[1;1HBBBBB");
        assert_eq!(
            grid.state.lock().selection_epoch,
            after_drop,
            "with the selection already gone there is nothing left to invalidate"
        );
    }

    // -----------------------------------------------------------------------
    // intent 메서드 — 락 안에서 진짜 모드를 읽는가
    // -----------------------------------------------------------------------

    #[test]
    fn encoding_reads_the_live_mode_rather_than_any_cache() {
        let grid = TerminalGrid::new(GridSize { rows: 4, cols: 10 }, 100);
        let up = KeyInput {
            key: TermKey::Named(NamedKey::ArrowUp),
            physical_latin: None,
            location: KeyLocation::Standard,
            mods: Mods::default(),
            text: None,
            repeat: false,
        };

        // 대조군: APP_CURSOR가 꺼져 있으면 CSI 형식이다.
        assert_eq!(grid.encode_key_locked(&up), Some(b"\x1b[A".to_vec()));
        assert_eq!(grid.encode_paste_locked("hi"), b"hi".to_vec());
        assert_eq!(grid.encode_focus_locked(true), None);

        grid.feed(b"\x1b[?1h"); // APP_CURSOR
        grid.feed(b"\x1b[?2004h"); // BRACKETED_PASTE
        grid.feed(b"\x1b[?1004h"); // FOCUS_IN_OUT

        assert_eq!(grid.encode_key_locked(&up), Some(b"\x1bOA".to_vec()));
        assert_eq!(
            grid.encode_paste_locked("hi"),
            b"\x1b[200~hi\x1b[201~".to_vec()
        );
        assert_eq!(grid.encode_focus_locked(true), Some(b"\x1b[I".to_vec()));
    }

    // -----------------------------------------------------------------------
    // handle_mouse — 좌표 변환, 래치, 선택
    // -----------------------------------------------------------------------

    #[test]
    fn a_left_drag_builds_a_selection_that_extracts_to_the_dragged_text() {
        let grid = grid_with_scrollback();
        // 뷰포트 0행은 "ffff5"다(히스토리 5줄 뒤).
        assert_eq!(grid.snapshot().row_text(0), "ffff5");

        drag(&grid, (0, 0), (0, 3));

        let epoch = grid.selection_epoch();
        assert_eq!(grid.extract_selection(epoch), Some("ffff".to_string()));
    }

    /// 좌표 변환이 **락 안에서** `display_offset`을 읽는다는 것을 텍스트로
    /// 확인한다. 보정을 빼면 같은 클릭이 뷰포트 0행("dddd3")이 아니라 현재
    /// 화면 0행("ffff5")을 집는다 — 줄마다 글자가 다른 이유가 이것이다.
    #[test]
    fn mouse_coordinates_are_converted_against_the_live_display_offset() {
        let grid = grid_with_scrollback();
        grid.scroll_display(Scroll::Delta(2));
        assert_eq!(grid.snapshot().row_text(0), "dddd3");

        drag(&grid, (0, 0), (0, 3));

        let epoch = grid.selection_epoch();
        assert_eq!(grid.extract_selection(epoch), Some("dddd".to_string()));
    }

    #[test]
    fn a_press_latches_its_route_so_a_mid_drag_mode_change_cannot_split_the_gesture() {
        let grid = grid_with_scrollback();
        let left = Some(TermMouseButton::Left);

        // 로컬(선택)로 시작한다 — 마우스 모드가 꺼져 있다.
        let press = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Left),
                (0, 0),
                Side::Left,
                left,
            ))
            .expect("press routes");
        assert_eq!(press.bytes, None, "local select must not write to the pty");

        // 드래그 도중 TUI가 마우스 리포팅을 켠다. **`?1002h`(MOUSE_DRAG)까지
        // 켜야 한다** — `?1000h`만으로는 모션의 라이브 라우트가 여전히 로컬이라,
        // 래치를 없애도 이 테스트가 통과해버린다(실제로 겪었다).
        grid.feed(b"\x1b[?1000h\x1b[?1002h");

        let motion = grid
            .handle_mouse(&intent(MouseAction::Motion, (0, 3), Side::Right, left))
            .expect("motion routes");
        assert_eq!(
            motion.bytes, None,
            "the latched local route must survive the mode change"
        );
        let epoch = grid.selection_epoch();
        assert_eq!(
            grid.extract_selection(epoch),
            Some("ffff".to_string()),
            "the gesture kept building a local selection"
        );

        let release = grid
            .handle_mouse(&intent(
                MouseAction::Release(TermMouseButton::Left),
                (0, 3),
                Side::Right,
                left,
            ))
            .expect("release routes");
        assert_eq!(release.bytes, None, "release stays on the latched route");

        // 대조군: 래치가 풀린 뒤 새 press는 이제 리포트로 간다.
        let after = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Left),
                (1, 1),
                Side::Left,
                left,
            ))
            .expect("press routes");
        assert!(
            after.bytes.is_some(),
            "a fresh gesture must see the live mouse mode"
        );
    }

    /// 휠은 래치에 참여하지 않는다 — 선택을 만드는 도중이라도 매번 라이브 모드로
    /// 독립 판정한다. 드래그 중 휠을 굴리는 TUI가 리포트를 못 받으면 안 된다.
    #[test]
    fn the_wheel_ignores_the_pointer_latch_and_re_evaluates_the_live_mode() {
        let grid = grid_with_scrollback();
        let left = Some(TermMouseButton::Left);

        grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            (0, 0),
            Side::Left,
            left,
        ))
        .expect("press routes");
        grid.feed(b"\x1b[?1000h\x1b[?1006h"); // 마우스 리포팅 + SGR

        let wheel = grid
            .handle_mouse(&intent(
                MouseAction::Wheel { lines: 1 },
                (0, 0),
                Side::Left,
                left,
            ))
            .expect("wheel routes");
        assert!(
            wheel.bytes.is_some(),
            "the wheel must report even though a local drag is latched"
        );

        // 대조군: 같은 순간의 모션은 여전히 래치된 로컬 라우트다.
        let motion = grid
            .handle_mouse(&intent(MouseAction::Motion, (0, 3), Side::Right, left))
            .expect("motion routes");
        assert_eq!(motion.bytes, None, "the drag itself is still local");
    }

    #[test]
    fn a_local_wheel_scrolls_the_display_and_asks_for_a_redraw() {
        let grid = grid_with_scrollback();
        assert_eq!(grid.snapshot().display_offset, 0);

        let result = grid
            .handle_mouse(&intent(
                MouseAction::Wheel { lines: 2 },
                (0, 0),
                Side::Left,
                None,
            ))
            .expect("wheel routes");

        assert!(result.redraw, "scrolling changes what is on screen");
        assert_eq!(result.bytes, None, "a local scroll writes nothing to the pty");
        assert_eq!(grid.snapshot().display_offset, 2);
    }

    #[test]
    fn a_contradictory_held_state_is_an_error_not_a_silent_suppression() {
        let grid = grid_with_scrollback();
        // Press(Left)인데 held가 Right — 위젯의 held 전이 표가 깨졌다는 뜻이다.
        let err = grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            (0, 0),
            Side::Left,
            Some(TermMouseButton::Right),
        ));
        assert_eq!(err, Err(MouseEncodeError::HeldMismatch));

        // 대조군: 같은 press가 held만 맞으면 Ok다. 억제(Ok + bytes None)와
        // 오류가 서로 다른 결과라는 것이 이 짝의 요점이다.
        let ok = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Left),
                (0, 0),
                Side::Left,
                Some(TermMouseButton::Left),
            ))
            .expect("a consistent held state routes");
        assert_eq!(ok.bytes, None);
    }

    // -----------------------------------------------------------------------
    // 복사 / 추출
    // -----------------------------------------------------------------------

    #[test]
    fn a_completed_drag_asks_to_copy_to_the_primary_selection_only() {
        let grid = grid_with_scrollback();
        let left = Some(TermMouseButton::Left);

        let press = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Left),
                (0, 0),
                Side::Left,
                left,
            ))
            .expect("press routes");
        assert_eq!(press.copy, None, "a drag in progress must not copy");

        let release = grid
            .handle_mouse(&intent(
                MouseAction::Release(TermMouseButton::Left),
                (0, 3),
                Side::Right,
                left,
            ))
            .expect("release routes");
        let request = release.copy.expect("a completed drag copies");
        assert_eq!(request.to, CopyTargets::DRAG_COMPLETE);
        // 실려 나간 epoch로 실제로 뽑을 수 있어야 한다 — bump 뒤에 읽지 않으면
        // 여기서 None이 나온다.
        assert_eq!(
            grid.extract_selection(request.epoch),
            Some("ffff".to_string())
        );
    }

    #[test]
    fn extraction_refuses_a_stale_epoch_and_leaves_the_selection_alive() {
        let grid = grid_with_scrollback();
        drag(&grid, (0, 0), (0, 3));
        let epoch = grid.selection_epoch();

        assert_eq!(
            grid.extract_selection(epoch + 1),
            None,
            "a mismatched epoch must extract nothing"
        );
        // 대조군: 맞는 epoch는 기대한 텍스트를 준다.
        assert_eq!(grid.extract_selection(epoch), Some("ffff".to_string()));
        // 그리고 추출은 읽기 전용이다 — 두 번 뽑아도 같은 값이 나온다.
        assert_eq!(grid.extract_selection(epoch), Some("ffff".to_string()));
    }

    #[test]
    fn output_inside_a_selection_invalidates_a_copy_request_made_before_it() {
        let grid = grid_with_scrollback();
        drag(&grid, (0, 0), (0, 3));
        let request = grid
            .request_copy(CopyTargets::EXPLICIT)
            .expect("a finished selection can be copied");

        // 셸이 선택 안쪽 셀을 덮어쓴다. 범위는 그대로지만 내용이 달라졌다.
        grid.feed(b"\x1b[1;1HZZZZ");

        assert_eq!(
            grid.extract_selection(request.epoch),
            None,
            "the text under the old epoch is no longer what the user selected"
        );
        // 대조군: 새로 요청하면 바뀐 내용을 정상적으로 뽑는다.
        let fresh = grid
            .request_copy(CopyTargets::EXPLICIT)
            .expect("the selection still exists");
        assert_eq!(grid.extract_selection(fresh.epoch), Some("ZZZZ".to_string()));
    }

    #[test]
    fn an_explicit_copy_is_refused_while_a_local_selection_is_still_being_built() {
        let grid = grid_with_scrollback();
        let left = Some(TermMouseButton::Left);

        grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            (0, 0),
            Side::Left,
            left,
        ))
        .expect("press routes");
        grid.handle_mouse(&intent(MouseAction::Motion, (0, 3), Side::Right, left))
            .expect("motion routes");

        assert_eq!(
            grid.request_copy(CopyTargets::EXPLICIT),
            None,
            "copying mid-drag would capture a later range than the shortcut press"
        );

        // 대조군: 버튼을 놓으면 곧바로 복사할 수 있다.
        grid.handle_mouse(&intent(
            MouseAction::Release(TermMouseButton::Left),
            (0, 3),
            Side::Right,
            left,
        ))
        .expect("release routes");
        assert!(
            grid.request_copy(CopyTargets::EXPLICIT).is_some(),
            "the gesture is finished, so the range is settled"
        );
    }

    /// 코드 클릭이 끝나면 **다음 press가 라이브 모드로 다시 판정돼야 한다.**
    ///
    /// 관찰 방법이 이 테스트의 핵심이다. "래치가 남았는가"를 `request_copy`로
    /// 물었던 이전 판은 이 버그를 못 잡았다 — 둘째 press가 `Ignore`로 라우팅돼
    /// `local_selection_in_progress`(=`LocalSelect`만 매치)에 걸리지 않았고,
    /// **이름이 걸린 마지막 단언이 아니라 대조군에서 죽었다.**
    ///
    /// 선택 결과로도 못 잡는다 — 남은 래치가 우연히 같은 라우트를 들고 있으면
    /// 재사용해도 결과가 같기 때문이다. **모드를 바꿔야** 낡은 래치를 재사용한
    /// 것과 라이브로 다시 판정한 것이 갈린다.
    #[test]
    fn a_finished_chord_lets_the_next_press_route_against_the_live_mode() {
        let grid = grid_with_scrollback();
        let left = Some(TermMouseButton::Left);
        let right = Some(TermMouseButton::Right);

        // 로컬 선택 모드에서 코드 클릭을 하고 첫 버튼을 뗀다.
        for (action, held) in [
            (MouseAction::Press(TermMouseButton::Left), left),
            (MouseAction::Press(TermMouseButton::Right), right),
            (MouseAction::Release(TermMouseButton::Left), left),
        ] {
            grid.handle_mouse(&intent(action, (0, 0), Side::Left, held))
                .expect("every step of the chord routes");
        }

        // 이제 TUI가 마우스 리포팅을 켠다.
        grid.feed(b"\x1b[?1000h\x1b[?1006h");

        let next = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Left),
                (1, 1),
                Side::Left,
                left,
            ))
            .expect("press routes");
        assert!(
            next.bytes.is_some(),
            "the chord left a stale latch: this press was routed by the old \
             gesture instead of the live mouse mode"
        );
    }

    /// **위젯이 기억을 잃어도 press 하나로 복구된다.**
    ///
    /// `Tree::diff`가 서브트리를 재생성하면 위젯의 `held`는 조용히 `None`이 되고
    /// 그리드의 래치는 남는다. 그 뒤 위젯은 어느 버튼의 press든 받아들이므로,
    /// 그리드에는 **살아 있는 래치와 다른 버튼의 press**가 도착한다.
    ///
    /// 이것이 코드 클릭일 수는 없다 — `route_mouse`가 `Press(b)`에
    /// `held == Some(b)`를 요구하고 `press_intent`는 이미 눌린 것이 있으면 `None`을
    /// 돌려주므로 **위젯은 코드 클릭 press 자체를 만들지 못한다.** 따라서 이
    /// 입력은 어긋남의 증거이고, 낡은 래치를 따르면 새 제스처가 통째로 잘못
    /// 라우팅된다.
    #[test]
    fn a_press_re_latches_even_when_it_names_a_different_button() {
        let grid = grid_with_scrollback();

        // 로컬 모드에서 좌버튼 제스처가 시작돼 래치가 잡힌다.
        grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            (0, 0),
            Side::Left,
            Some(TermMouseButton::Left),
        ))
        .expect("press routes");

        // 그 사이 TUI가 마우스 리포팅을 켠다. 그리고 위젯이 상태를 잃어
        // 우버튼 press가 그대로 들어온다.
        grid.feed(b"\x1b[?1000h\x1b[?1006h");
        let desynced = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Right),
                (1, 1),
                Side::Left,
                Some(TermMouseButton::Right),
            ))
            .expect("press routes");

        assert!(
            desynced.bytes.is_some(),
            "the stale latch must not route a brand-new gesture — a press is \
             always the authoritative start of one"
        );
    }

    /// **release는 버튼이 어긋나도 래치를 푼다.** 안 풀면
    /// `local_selection_in_progress`가 계속 참이라 단축키 복사가 죽는다 —
    /// 리뷰가 실제로 관측한 증상이다.
    #[test]
    fn a_release_clears_the_latch_even_when_it_names_a_different_button() {
        let grid = grid_with_scrollback();
        grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            (0, 0),
            Side::Left,
            Some(TermMouseButton::Left),
        ))
        .expect("press routes");

        // 대조군: 제스처가 살아 있는 동안에는 복사가 막힌다.
        assert_eq!(
            grid.request_copy(CopyTargets::EXPLICIT),
            None,
            "a live local gesture blocks copying"
        );

        // 위젯이 기억을 잃은 뒤 다른 버튼의 release가 도착한다.
        grid.handle_mouse(&intent(
            MouseAction::Release(TermMouseButton::Right),
            (0, 1),
            Side::Left,
            Some(TermMouseButton::Right),
        ))
        .expect("release routes");

        assert!(
            grid.request_copy(CopyTargets::EXPLICIT).is_some(),
            "a mismatched release means our latch is the stale one — holding it \
             keeps shortcut copy dead"
        );
    }

    /// 버튼이 눌리지 않은 모션인데 래치가 살아 있으면 어긋난 것이다. 마우스는
    /// 쉬지 않고 움직이므로 **이 갈래가 가장 빨리 복구시킨다.**
    #[test]
    fn a_motion_with_no_button_held_clears_a_stale_latch() {
        let grid = grid_with_scrollback();
        grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            (0, 0),
            Side::Left,
            Some(TermMouseButton::Left),
        ))
        .expect("press routes");

        // 대조군: 드래그 중(버튼이 눌린) 모션은 래치를 유지한다.
        grid.handle_mouse(&intent(
            MouseAction::Motion,
            (0, 2),
            Side::Right,
            Some(TermMouseButton::Left),
        ))
        .expect("motion routes");
        assert_eq!(
            grid.request_copy(CopyTargets::EXPLICIT),
            None,
            "a real drag must keep the latch"
        );

        // 버튼이 눌리지 않은 모션 = 위젯은 제스처가 없다고 믿는다.
        grid.handle_mouse(&intent(MouseAction::Motion, (0, 3), Side::Right, None))
            .expect("motion routes");
        assert!(
            grid.request_copy(CopyTargets::EXPLICIT).is_some(),
            "hovering with no button down proves the latch is stale"
        );
    }

    /// 릴리스를 잃어버려도 **같은 버튼을 다시 누르면 라이브로 재판정된다.**
    /// 버튼을 떼지 않고 두 번 누를 수는 없으므로 둘째 press는 유실의 증거다.
    ///
    /// 모드를 바꾸는 이유는 위와 같다 — 낡은 래치가 같은 라우트를 들고 있으면
    /// 재사용해도 선택 결과가 같아서 구분되지 않는다.
    #[test]
    fn a_lost_release_lets_the_same_button_re_press_route_against_the_live_mode() {
        let grid = grid_with_scrollback();
        let left = Some(TermMouseButton::Left);

        // press만 하고 release가 오지 않는다.
        grid.handle_mouse(&intent(
            MouseAction::Press(TermMouseButton::Left),
            (0, 0),
            Side::Left,
            left,
        ))
        .expect("press routes");

        grid.feed(b"\x1b[?1000h\x1b[?1006h");

        let again = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Left),
                (1, 1),
                Side::Left,
                left,
            ))
            .expect("press routes");
        assert!(
            again.bytes.is_some(),
            "a lost release left the latch stuck on the old route, so this \
             press never reached the reporting TUI"
        );

        // 대조군: 라우팅이 되살아났을 뿐 아니라 로컬 선택도 여전히 정상이다.
        let grid2 = grid_with_scrollback();
        grid2
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Left),
                (0, 0),
                Side::Left,
                left,
            ))
            .expect("press routes");
        drag(&grid2, (0, 0), (0, 3));
        let epoch = grid2.selection_epoch();
        assert_eq!(grid2.extract_selection(epoch), Some("ffff".to_string()));
    }

    /// 리포트로 래치된 드래그는 선택을 건드리지 않는다 — 그동안 단축키 복사를
    /// 막으면 마우스 모드 TUI 위에서 버튼을 붙들고 있는 내내 복사가 죽는다.
    #[test]
    fn a_reporting_drag_does_not_block_an_explicit_copy() {
        let grid = grid_with_scrollback();
        drag(&grid, (0, 0), (0, 3)); // 먼저 복사할 선택을 만들어 둔다
        grid.feed(b"\x1b[?1000h");

        let right = Some(TermMouseButton::Right);
        let press = grid
            .handle_mouse(&intent(
                MouseAction::Press(TermMouseButton::Right),
                (1, 1),
                Side::Left,
                right,
            ))
            .expect("press routes");
        assert!(press.bytes.is_some(), "this press must be a report");

        assert!(
            grid.request_copy(CopyTargets::EXPLICIT).is_some(),
            "a reporting drag cannot change the selection, so it must not block copying"
        );
    }

    #[test]
    fn request_copy_returns_none_when_there_is_no_selection() {
        let grid = grid_with_scrollback();
        assert_eq!(grid.request_copy(CopyTargets::EXPLICIT), None);
        // 대조군: 선택을 만들면 Some이다.
        drag(&grid, (0, 0), (0, 3));
        assert!(grid.request_copy(CopyTargets::EXPLICIT).is_some());
    }

    /// 선택 범위는 **그리드 좌표계**라 화면을 굴려도 그대로다 — 복사할 내용이
    /// 달라지지 않으므로 버전을 올리면 안 된다. 반면 **뷰포트로 잘라낸** 결과는
    /// 달라진다. 두 값이 서로 다르게 움직인다는 것이 여기서 고정할 사실이고,
    /// "아무 일도 안 일어났다"를 홀로 단언하지 않기 위한 대조군이기도 하다.
    #[test]
    fn scrolling_the_display_reclips_the_selection_without_bumping_the_epoch() {
        let grid = grid_with_scrollback();
        set_selection(
            &grid,
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Point::new(Line(1), Column(4)),
        );
        let before = grid.state.lock().selection_epoch;
        assert_eq!(
            grid.snapshot().selection,
            Some(ViewportSelection {
                start: (0, 0),
                end: (1, 4),
                is_block: false,
            })
        );

        grid.scroll_display(Scroll::Delta(2));

        assert_eq!(
            grid.state.lock().selection_epoch,
            before,
            "the grid-coordinate range did not move, so the copied text cannot differ"
        );
        // 대조군 1: 잘라낸 결과는 실제로 두 행 아래로 밀렸다.
        assert_eq!(
            grid.snapshot().selection,
            Some(ViewportSelection {
                start: (2, 0),
                end: (3, 4),
                is_block: false,
            })
        );
        // 대조군 2: 같은 선택이 살아 있는 상태에서 출력이 들어오면 올라간다.
        grid.feed(b"\x1b[1;1HZZZZZ");
        assert!(
            grid.state.lock().selection_epoch > before,
            "output inside a live selection must still bump"
        );
    }
}
