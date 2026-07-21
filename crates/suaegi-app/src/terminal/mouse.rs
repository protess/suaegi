//! Task 6 — iced 마우스 이벤트를 `MouseIntent`로 만든다. 라우팅과 선택
//! 변경은 여기 없다 — `TerminalGrid`가 락 안에서 한다.
//!
//! **위젯은 라우팅 결과를 볼 수 없다.** `MouseResult`는 위젯의 `update`가 끝난
//! 뒤에 앱으로 돌아가므로, 위젯이 그걸 보고 상태를 유지할 방법이 없다. 그래서
//! 여기 있는 것은 전부 **원시 사실**이다: 눌린 버튼, 마지막 클릭, 커서 위치,
//! 스크롤 누산기, 수식자.
//!
//! 위젯이 내리는 **유일한** 라우팅 판단은 `force_local`(Shift 오버라이드)이다.
//! 그것만은 모드와 무관하기 때문이다 — 앱이 마우스 모드를 쥐고 있어도 Shift를
//! 누르면 사용자가 선택할 수 있어야 한다.

use std::time::{Duration, Instant};

use iced::advanced::mouse as iced_mouse;
use iced::advanced::Shell;
use iced::{window, Event, Point, Rectangle};

use suaegi_term::grid::GridSize;
use suaegi_term::input_types::{
    ClickKind, Mods, MouseAction, MouseIntent, TermMouseButton, ViewportHit,
};

use crate::session_store::SessionId;
use crate::terminal::contract::TermCommand;
use crate::terminal::state::{hit_test, CellMetrics, State};
use crate::terminal::Published;

// ---------------------------------------------------------------------------
// 클릭 분류 — 우리 소유의 순수 함수
// ---------------------------------------------------------------------------

/// 연속 클릭으로 인정하는 시간 간격. iced와 같은 값이다
/// (`iced_core/src/mouse/click.rs:86`) — 사용자가 앱 안에서 두 가지 다른
/// 더블클릭 감각을 느끼면 안 된다.
pub const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(300);

/// 연속 클릭으로 인정하는 이동 거리. 역시 iced와 같은 값이다(`:85`).
pub const MULTI_CLICK_DISTANCE: f32 = 6.0;

/// 마지막 클릭. **`kind`를 포함한다** — 이것이 없으면 트리플 클릭을 만들 수
/// 없다. 분류는 "직전 클릭의 종류에서 한 단계 올린다"이지 "직전 클릭이
/// 있었는가"가 아니기 때문이다(`ClickKind::next` 참고).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LastClick {
    pub button: TermMouseButton,
    pub at: Instant,
    pub pos: Point,
    pub kind: ClickKind,
}

/// 다음 단계. **트리플 다음은 더블이다**(싱글이 아니다) — 네 번 이상 연타할 때
/// 더블↔트리플을 오가는 것이 iced와 다른 터미널의 공통 동작이다
/// (`iced_core/src/mouse/click.rs:31-36`).
fn next_kind(kind: ClickKind) -> ClickKind {
    match kind {
        ClickKind::Single => ClickKind::Double,
        ClickKind::Double => ClickKind::Triple,
        ClickKind::Triple => ClickKind::Double,
    }
}

/// 클릭 분류. **시각과 버튼을 인자로 받는다.**
///
/// `mouse::Click::new`를 쓰지 않는 이유: 그쪽은 내부에서 `Instant::now()`를
/// 읽으므로 하네스가 시계를 주입할 수 없고, 그러면 임계 직전/직후를 가르는
/// 동작을 **mutation 검증할 방법이 없다.** 시각은 위젯 상태가 들고 다닌다.
///
/// **버튼이 비교에 반드시 들어간다.** 없으면 좌클릭 직후 같은 자리의 빠른
/// 우클릭이 더블클릭으로 분류된다 — iced의 분류기도 버튼 일치를 요구한다
/// (`iced_core/src/mouse/click.rs:50-53`).
pub fn classify_click(
    prev: Option<LastClick>,
    button: TermMouseButton,
    now: Instant,
    pos: Point,
) -> ClickKind {
    let Some(prev) = prev else {
        return ClickKind::Single;
    };
    if prev.button != button {
        return ClickKind::Single;
    }
    // 시계가 뒤로 갔으면(`now < prev.at`) 연속으로 보지 않는다 — `Instant`
    // 뺄셈은 그 경우 패닉하므로 `checked_`가 정확성만이 아니라 안전 문제다.
    let Some(elapsed) = now.checked_duration_since(prev.at) else {
        return ClickKind::Single;
    };
    if elapsed > MULTI_CLICK_INTERVAL || prev.pos.distance(pos) >= MULTI_CLICK_DISTANCE {
        return ClickKind::Single;
    }
    next_kind(prev.kind)
}

// ---------------------------------------------------------------------------
// 좌표
// ---------------------------------------------------------------------------

/// iced 버튼 → 프로토콜 버튼. 터미널이 리포트할 수 있는 것은 셋뿐이다.
///
/// **와일드카드를 쓰지 않는다** — iced가 변형을 늘리면 여기서 컴파일이 깨져야
/// 한다. 조용히 `None`으로 흘리면 새 버튼이 영영 죽은 채로 남는다.
pub fn to_button(button: iced_mouse::Button) -> Option<TermMouseButton> {
    match button {
        iced_mouse::Button::Left => Some(TermMouseButton::Left),
        iced_mouse::Button::Middle => Some(TermMouseButton::Middle),
        iced_mouse::Button::Right => Some(TermMouseButton::Right),
        iced_mouse::Button::Back | iced_mouse::Button::Forward | iced_mouse::Button::Other(_) => {
            None
        }
    }
}

/// 커서 위치를 셀로 옮긴다. 위젯 밖이거나 그리드 밖이면 `None`.
///
/// **`Cursor::Levitating`은 `position()`이 `None`이라 여기서 조용히 실패한다.**
/// 이것은 의도된 동작이다 — `Levitating`은 우리 위에 오버레이(메뉴 등)가 떠
/// 있다는 뜻이고, 그때 터미널이 마우스를 먹으면 오버레이 위에서 드래그가
/// 시작된다. 무시가 맞다. 다만 **조용하다는 것 자체가 함정**이라 여기 적어둔다.
pub fn hit_for(
    cursor: iced_mouse::Cursor,
    bounds: Rectangle,
    metrics: CellMetrics,
    size: GridSize,
) -> Option<ViewportHit> {
    hit_test(cursor.position_in(bounds)?, metrics, size)
}

// ---------------------------------------------------------------------------
// 스크롤 누산
// ---------------------------------------------------------------------------

/// 스크롤 델타를 **정수 줄**로 바꾸고 나머지를 누산기에 남긴다. 작은 델타가
/// 여러 번 와도 합이 한 줄을 넘는 순간 정확히 한 줄이 나오게 한다.
///
/// **누산기의 단위는 픽셀이 아니라 줄이다.** 위젯 상태에는 `scroll_acc`가
/// **하나뿐인데**(Task 3에서 동결) 휠은 `Lines`로, 트랙패드는 `Pixels`로 온다.
/// 단위를 섞어 누산하면 세션 도중 장치를 바꿨을 때 남은 나머지가 엉뚱한
/// 배율로 해석된다. 픽셀을 들어오는 즉시 줄로 환산해 넣으면 그 문제가 없고,
/// 픽셀만 놓고 보면 `acc_px % h`와 `(acc_px / h) % 1`이 같으므로 결과도 같다.
///
/// **부호**: 반환값은 `MouseAction::Wheel { lines }`의 규약대로 **양수 = 위로**다.
/// iced의 `y`도 양수가 위다(`iced_widget`의 `scrollable`이 `-y`를 아래 방향
/// 오프셋에 더한다 — `scrollable.rs:873, 1799`), 그래서 부호를 뒤집지 않는다.
pub fn accumulate_scroll(acc: &mut f32, delta: iced_mouse::ScrollDelta, cell_height: f32) -> i32 {
    let lines = match delta {
        iced_mouse::ScrollDelta::Lines { y, .. } => y,
        iced_mouse::ScrollDelta::Pixels { y, .. } => y / cell_height,
    };
    // OS가 NaN을 주면 누산기가 영구히 오염돼 **스크롤이 다시는 동작하지 않는다.**
    // 이 한 줄이 그 영구 고장을 막는다.
    if !lines.is_finite() {
        return 0;
    }
    *acc += lines;
    let whole = acc.trunc();
    *acc %= 1.0;
    whole as i32
}

// ---------------------------------------------------------------------------
// intent 만들기 — held 전이 표
// ---------------------------------------------------------------------------

/// Shift 오버라이드. 위젯이 내리는 **유일한** 라우팅 판단이다 — 모드와
/// 무관하기 때문이다.
fn force_local(mods: Mods) -> bool {
    mods.shift
}

/// Press. **`held`를 intent를 만들기 전에 갱신한다** — `route_mouse`가
/// `Press(b)`에 `held == Some(b)`를 요구하고, 어긋나면 정상 입력에서
/// `MouseEncodeError`가 튄다.
/// **이미 버튼이 눌려 있으면 둘째 press는 아예 발행하지 않는다**(리뷰에서 발견).
///
/// `held`만 유지하고 인텐트는 내보내는 절충은 비대칭을 자리만 옮긴다 — 그러면
/// 이번엔 둘째 버튼의 release가 `held`와 어긋나 버려지고, 마우스 리포팅 앱은
/// 짝 없는 press를 받아 **드래그 상태에 갇힌다.** 터미널은 코드 클릭으로 할 일이
/// 없으므로 제스처의 주인은 첫 버튼 하나로 못박고, 둘째 버튼은 press도 release도
/// 내보내지 않는다 — 짝이 맞는다.
///
/// **버튼별로 따로 래치하는 설계를 고르지 않은 진짜 이유는 프로토콜 타입이다.**
/// `MouseIntent.held`가 `Option<TermMouseButton>`이다(Task 0에서 못박은 타입).
/// 버튼별 래치로 가려면 이걸 집합으로 넓혀야 하고, 그러면 `route_mouse`의 드래그
/// 분기, `encode_mouse_report`의 모션 버튼 코드, 인코더 표 테스트가 전부 딸려
/// 온다. 그렇게까지 해도 **xterm 와이어 포맷이 두 버튼을 동시에 표현하지 못한다**
/// — 레거시 릴리스는 어느 버튼이든 코드 3으로 보고한다. 즉 집합으로 넓히면
/// 타입이 와이어가 나를 수 없는 것을 나르는 척하게 된다. 여기서 하나로 줄이는
/// 편이 타입과 와이어를 일치시킨다.
///
/// → 이 함수를 "버튼마다 하나씩"으로 되돌리려면 위 세 곳과 와이어 포맷을 먼저
/// 해결해야 한다. 그 전에는 개선이 아니라 거짓말이 된다.
pub fn press_intent(
    state: &mut State,
    button: TermMouseButton,
    hit: ViewportHit,
    click: ClickKind,
) -> Option<MouseIntent> {
    if state.held.is_some() {
        return None;
    }
    state.held = Some(button);
    Some(MouseIntent {
        action: MouseAction::Press(button),
        hit,
        held: state.held,
        mods: state.mods,
        click,
        force_local: force_local(state.mods),
    })
}

/// Release. **intent를 만든 뒤에 `held`를 지운다** — 놓인 버튼이 intent에
/// 실려야 한다.
///
/// 눌린 적 없는 버튼의 release는 **intent를 만들지 않는다**(창 밖에서 눌렸다
/// 들어온 경우 등). 그대로 흘려보내면 그리드의 래치가 남의 제스처를 끊는다.
pub fn release_intent(
    state: &mut State,
    button: TermMouseButton,
    hit: ViewportHit,
    click: ClickKind,
) -> Option<MouseIntent> {
    if state.held != Some(button) {
        return None;
    }
    let intent = MouseIntent {
        action: MouseAction::Release(button),
        hit,
        held: Some(button),
        mods: state.mods,
        click,
        force_local: force_local(state.mods),
    };
    state.held = None;
    Some(intent)
}

/// Motion. `held`를 **그대로** 싣는다(`None`일 수 있다). 상태를 갱신하지 않는다.
pub fn motion_intent(state: &State, hit: ViewportHit, click: ClickKind) -> MouseIntent {
    MouseIntent {
        action: MouseAction::Motion,
        hit,
        held: state.held,
        mods: state.mods,
        click,
        force_local: force_local(state.mods),
    }
}

/// Wheel. `held`를 그대로 싣고 상태를 갱신하지 않는다.
///
/// **휠은 그리드의 래치에 참여하지 않는다** — 드래그 중이라도 매번 라이브
/// 모드로 독립 판정된다. 그 판단은 그리드가 하지만, 위젯도 여기서 `held`를
/// 건드리지 않는 것으로 같은 규칙을 지킨다.
pub fn wheel_intent(state: &State, hit: ViewportHit, lines: i32, click: ClickKind) -> MouseIntent {
    MouseIntent {
        action: MouseAction::Wheel { lines },
        hit,
        held: state.held,
        mods: state.mods,
        click,
        force_local: force_local(state.mods),
    }
}

// ---------------------------------------------------------------------------
// Widget::update의 마우스 몫
// ---------------------------------------------------------------------------

/// `mod.rs`가 `input::update` **뒤에** 부른다.
///
/// **bounds 게이팅은 여기가 맡는다.** 키 경로는 포커스로만 거른다(포커스된
/// 터미널은 커서가 어디 있든 타이핑을 받아야 한다). 마우스는 반대로 커서 좌표
/// 자체가 의미이므로 bounds가 곧 대상 판정이다 — `pane_grid`는 커서가 근처에도
/// 없는 pane에까지 이벤트를 뿌린다.
///
/// **포커스로는 거르지 않는다.** 포커스 없는 pane을 클릭하는 것이 바로 포커스를
/// 옮기는 동작이다. 그걸 막으면 마우스로 pane을 고를 수 없다.
///
/// 그리드 크기는 **스냅샷의 것**을 받는다. 사용자가 보고 클릭하는 것이 스냅샷이라
/// 히트테스트도 같은 좌표계여야 한다 — 방금 리사이즈했는데 PTY가 아직 못 따라온
/// 순간에 새 크기로 판정하면 화면에 없는 셀을 집는다.
pub(crate) fn update(
    state: &mut State,
    id: SessionId,
    event: &Event,
    bounds: Rectangle,
    cursor: iced_mouse::Cursor,
    size: GridSize,
    shell: &mut Shell<'_, Published>,
) {
    // **창 포커스를 잃으면 제스처를 닫는다.** `held`를 푸는 경로 중 유일하게
    // 마우스 이벤트에 기대지 않는 것이다 — OS가 릴리스를 아예 주지 않는 경우
    // (드래그 중 포커스를 빼앗김)에 남는 마지막 그물이다.
    if matches!(event, Event::Window(window::Event::Unfocused)) {
        let hit = state.metrics.and_then(|m| {
            hit_for(cursor, bounds, m, size).or_else(|| last_known_hit(state, m, size))
        });
        resolve_held_gesture(state, id, hit, shell);
        return;
    }

    let Event::Mouse(mouse_event) = event else {
        return;
    };
    // 다른 위젯이 이미 가져간 이벤트는 건드리지 않는다. **캡처는 단락이 아니라
    // 플래그이므로** 이 검사가 없으면 같은 이벤트를 두 번 처리할 수 있다.
    //
    // **예외: 이미 우리가 소유한 제스처의 릴리스.** 그것까지 흘려보내면 `held`가
    // 남고, 그 뒤로 이 pane의 **모든 press가 조용히 죽는다**(`press_intent`가
    // 계속 `None`). 남이 캡처한 이벤트를 가로채는 것이 아니라, 이미 시작한 우리
    // 제스처를 닫는 것이므로 예외로 둘 근거가 있다.
    let owns_this_release = state.held.is_some()
        && matches!(*mouse_event, iced_mouse::Event::ButtonReleased(b) if to_button(b) == state.held);
    if shell.is_event_captured() && !owns_this_release {
        return;
    }
    // 아직 측정 전이면 셀을 알 수 없다. 추측한 좌표로 선택을 시작하는 것보다
    // 아무것도 안 하는 편이 낫다.
    let Some(metrics) = state.metrics else {
        return;
    };

    let hit = hit_for(cursor, bounds, metrics, size);
    if let Some(position) = cursor.position_in(bounds) {
        state.cursor_pos = Some(position);
    }

    match *mouse_event {
        iced_mouse::Event::ButtonPressed(button) => {
            let (Some(button), Some(hit)) = (to_button(button), hit) else {
                return;
            };
            // **같은 버튼이 이미 눌려 있다는 것은 릴리스를 잃었다는 증거다** —
            // 버튼을 떼지 않고 두 번 누를 수는 없다. 다른 버튼의 press는 진짜
            // 코드 클릭이라 모호하지만, 이 경우만은 모호하지 않으므로 여기서
            // 제스처를 닫고 새로 시작한다. 이것이 없으면 잃어버린 릴리스 하나가
            // 이 pane의 마우스 입력을 세션 끝까지 죽인다.
            if state.held == Some(button) {
                resolve_held_gesture(state, id, Some(hit), shell);
            }
            let now = Instant::now();
            let pos = cursor_point(state);
            let kind = classify_click(state.last_click, button, now, pos);
            // **`last_click`은 press가 실제로 받아들여진 뒤에만 갱신한다.**
            // 무시된 코드 클릭(다른 버튼이 눌린 채 들어온 press)을 기록해버리면
            // 더블클릭 사슬이 그 버튼으로 끊긴다 — 좌클릭 → 우버튼 스침 → 좌클릭이
            // `Double`이 아니라 `Single`이 된다.
            if let Some(intent) = press_intent(state, button, hit, kind) {
                state.last_click = Some(LastClick {
                    button,
                    at: now,
                    pos,
                    kind,
                });
                publish(shell, id, intent);
            }
        }
        iced_mouse::Event::ButtonReleased(button) => {
            let Some(button) = to_button(button) else {
                return;
            };
            // **릴리스는 위젯 밖에서도 반드시 나가야 한다.** 안 보내면 그리드의
            // 포인터 래치가 풀리지 않아, 버튼을 뗀 뒤에도 마우스를 움직이는
            // 것만으로 선택이 계속 따라온다. bounds 밖이면 마지막으로 알던
            // 셀을 쓴다 — press가 안에서 일어났으므로 반드시 하나 있다.
            //
            // **놓을 자리를 못 찾아도 `held`는 반드시 정리한다.** 예전에는 여기서
            // 그냥 빠져나갔는데, 드래그 도중 pane이 줄어 그 셀이 사라지기만 해도
            // (평범한 리사이즈다) `held`가 남아 이후 모든 press가 죽었다.
            // **발행과 상태 정리는 다른 관심사다** — 전자가 실패해도 후자는 한다.
            let placed = hit.or_else(|| last_known_hit(state, metrics, size));
            let kind = state.last_click.map_or(ClickKind::Single, |c| c.kind);
            match placed {
                Some(hit) => {
                    if let Some(intent) = release_intent(state, button, hit, kind) {
                        publish(shell, id, intent);
                    }
                }
                None if state.held == Some(button) => {
                    // 그리드의 래치는 자기 자가복구(같은 버튼 재-press)에 맡긴다.
                    state.held = None;
                }
                None => {}
            }
        }
        iced_mouse::Event::CursorMoved { .. } => {
            // 드래그 중이 아니면 모션은 보낼 것이 없다 — 마우스 모드 TUI만
            // 순수 모션을 원하고, 그 판단은 그리드가 한다. 여기서는 bounds
            // 안인지만 본다.
            let Some(hit) = hit else {
                return;
            };
            let kind = state.last_click.map_or(ClickKind::Single, |c| c.kind);
            publish(shell, id, motion_intent(state, hit, kind));
        }
        iced_mouse::Event::WheelScrolled { delta } => {
            let Some(hit) = hit else {
                return;
            };
            let lines = accumulate_scroll(&mut state.scroll_acc, delta, metrics.height());
            // 누산 결과가 0줄이면 발행하지 않는다 — 나머지는 누산기에 남아 있다.
            if lines == 0 {
                return;
            }
            publish(
                shell,
                id,
                wheel_intent(state, hit, lines, ClickKind::Single),
            );
        }
        // 커서가 창을 드나드는 것 자체로는 터미널이 할 일이 없다. 버튼이
        // 눌린 채 나갔다면 릴리스가 위에서 처리한다.
        iced_mouse::Event::CursorEntered | iced_mouse::Event::CursorLeft => {}
    }
}

/// 진행 중인 제스처를 강제로 닫는다. `held`를 풀고, 놓을 자리를 알면 **합성
/// 릴리스까지 발행해** 그리드의 포인터 래치도 함께 해소한다.
///
/// 자리를 모르면 `held`만 푼다 — 그리드 쪽은 같은 버튼이 다시 눌릴 때 스스로
/// 복구한다(`resolve_route`). 여기서 좌표를 지어내지 않는 이유는, 지어낸 셀로
/// 릴리스를 보내면 사용자가 만들던 선택의 **끝점이 엉뚱한 곳으로 확정**되기
/// 때문이다. 래치가 잠시 남는 것보다 그쪽이 눈에 더 잘 띄는 오류다.
fn resolve_held_gesture(
    state: &mut State,
    id: SessionId,
    hit: Option<ViewportHit>,
    shell: &mut Shell<'_, Published>,
) {
    let Some(button) = state.held else {
        return;
    };
    let kind = state.last_click.map_or(ClickKind::Single, |c| c.kind);
    match hit {
        Some(hit) => {
            if let Some(intent) = release_intent(state, button, hit, kind) {
                publish(shell, id, intent);
            }
        }
        None => state.held = None,
    }
}

/// 클릭 분류에 쓰는 위치. **위젯 상대 좌표**다 — 절대 좌표를 쓰면 pane을 옮긴
/// 뒤의 클릭이 거리 임계를 엉뚱하게 넘는다.
fn cursor_point(state: &State) -> Point {
    state.cursor_pos.unwrap_or(Point::ORIGIN)
}

/// 마지막으로 알던 커서 위치의 셀. 릴리스가 bounds 밖에서 일어났을 때 쓴다.
fn last_known_hit(state: &State, metrics: CellMetrics, size: GridSize) -> Option<ViewportHit> {
    hit_test(state.cursor_pos?, metrics, size)
}

fn publish(shell: &mut Shell<'_, Published>, id: SessionId, intent: MouseIntent) {
    shell.publish((id, TermCommand::Mouse(intent)));
    // **캡처를 단락으로 믿지 않는다.** 호출은 하되 위의 게이팅이 그것 없이도
    // 옳아야 한다(플랜 절대 규칙).
    shell.capture_event();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> CellMetrics {
        CellMetrics::new(8.0, 16.0).expect("8x16 must be valid metrics")
    }

    fn grid() -> GridSize {
        GridSize { rows: 10, cols: 20 }
    }

    /// `Instant`는 생성자가 없으므로 기준을 한 번 잡고 오프셋을 더한다.
    ///
    /// **기준을 함수 안에서 매번 `Instant::now()`로 잡으면 안 된다** — 호출마다
    /// 값이 달라져 "300ms 뒤"가 실제로는 "300ms + 그 사이에 흐른 시간"이 된다.
    /// 처음에 그렇게 썼고 임계 테스트가 곧바로 어긋났다.
    struct Clock(Instant);

    impl Clock {
        fn new() -> Self {
            Self(Instant::now())
        }

        fn at(&self, millis: u64) -> Instant {
            self.0 + Duration::from_millis(millis)
        }

        fn last(
            &self,
            button: TermMouseButton,
            millis: u64,
            pos: Point,
            kind: ClickKind,
        ) -> LastClick {
            LastClick {
                button,
                at: self.at(millis),
                pos,
                kind,
            }
        }
    }

    // ---------------------------------------------------------- classify_click

    #[test]
    fn the_first_click_is_always_single() {
        let c = Clock::new();
        assert_eq!(
            classify_click(None, TermMouseButton::Left, c.at(0), Point::new(1.0, 1.0)),
            ClickKind::Single
        );
    }

    #[test]
    fn consecutive_clicks_walk_single_double_triple_then_back_to_double() {
        let c = Clock::new();
        let pos = Point::new(10.0, 10.0);
        let mut kind = ClickKind::Single;
        let mut prev = Some(c.last(TermMouseButton::Left, 0, pos, kind));

        for (i, expected) in [ClickKind::Double, ClickKind::Triple, ClickKind::Double]
            .into_iter()
            .enumerate()
        {
            let now = c.at((i as u64 + 1) * 100);
            kind = classify_click(prev, TermMouseButton::Left, now, pos);
            assert_eq!(kind, expected, "click #{}", i + 2);
            prev = Some(LastClick {
                button: TermMouseButton::Left,
                at: now,
                pos,
                kind,
            });
        }
    }

    /// 임계 **직전과 직후**를 함께 단언한다 — 시각을 인자로 받았기 때문에 이
    /// 경계를 mutation 검증할 수 있다. `Click::new`였다면 불가능했다.
    #[test]
    fn the_interval_boundary_is_inclusive_on_the_near_side() {
        let c = Clock::new();
        let pos = Point::new(4.0, 4.0);
        let prev = Some(c.last(TermMouseButton::Left, 0, pos, ClickKind::Single));

        assert_eq!(
            classify_click(prev, TermMouseButton::Left, c.at(300), pos),
            ClickKind::Double,
            "exactly at the interval must still be consecutive"
        );
        assert_eq!(
            classify_click(prev, TermMouseButton::Left, c.at(301), pos),
            ClickKind::Single,
            "one millisecond past the interval must restart"
        );
    }

    #[test]
    fn the_distance_boundary_is_exclusive_on_the_far_side() {
        let c = Clock::new();
        let origin = Point::new(0.0, 0.0);
        let prev = Some(c.last(TermMouseButton::Left, 0, origin, ClickKind::Single));

        assert_eq!(
            classify_click(prev, TermMouseButton::Left, c.at(10), Point::new(5.99, 0.0)),
            ClickKind::Double,
            "just inside the radius is consecutive"
        );
        assert_eq!(
            classify_click(prev, TermMouseButton::Left, c.at(10), Point::new(6.0, 0.0)),
            ClickKind::Single,
            "exactly at the radius is not"
        );
    }

    /// 버튼을 비교에 넣지 않으면 좌클릭 직후의 빠른 우클릭이 더블클릭이 된다.
    #[test]
    fn a_different_button_restarts_the_count_even_at_the_same_spot_and_instant() {
        let c = Clock::new();
        let pos = Point::new(3.0, 3.0);
        let prev = Some(c.last(TermMouseButton::Left, 0, pos, ClickKind::Single));

        assert_eq!(
            classify_click(prev, TermMouseButton::Right, c.at(10), pos),
            ClickKind::Single,
            "a right click after a left click is not a double click"
        );
        // 대조군: 같은 버튼이면 같은 시각·자리에서 더블이 된다.
        assert_eq!(
            classify_click(prev, TermMouseButton::Left, c.at(10), pos),
            ClickKind::Double
        );
    }

    #[test]
    fn a_clock_that_went_backwards_restarts_the_count_instead_of_panicking() {
        let c = Clock::new();
        let pos = Point::new(2.0, 2.0);
        let prev = Some(c.last(TermMouseButton::Left, 500, pos, ClickKind::Single));
        assert_eq!(
            classify_click(prev, TermMouseButton::Left, c.at(0), pos),
            ClickKind::Single
        );
    }

    // ------------------------------------------------------------- to_button

    #[test]
    fn only_the_three_reportable_buttons_convert() {
        assert_eq!(
            to_button(iced_mouse::Button::Left),
            Some(TermMouseButton::Left)
        );
        assert_eq!(
            to_button(iced_mouse::Button::Middle),
            Some(TermMouseButton::Middle)
        );
        assert_eq!(
            to_button(iced_mouse::Button::Right),
            Some(TermMouseButton::Right)
        );
        assert_eq!(to_button(iced_mouse::Button::Back), None);
        assert_eq!(to_button(iced_mouse::Button::Forward), None);
        assert_eq!(to_button(iced_mouse::Button::Other(9)), None);
    }

    // --------------------------------------------------------------- hit_for

    #[test]
    fn hit_for_maps_a_cursor_inside_the_widget_to_a_cell() {
        let bounds = Rectangle::new(Point::new(100.0, 50.0), iced::Size::new(160.0, 160.0));
        // 위젯 원점에서 (17, 32) 떨어진 점 → 열 2, 행 2. 셀 안에서 17-16=1px,
        // 폭 8의 절반보다 작으므로 왼쪽이다.
        let cursor = iced_mouse::Cursor::Available(Point::new(117.0, 82.0));
        assert_eq!(
            hit_for(cursor, bounds, metrics(), grid()),
            Some(ViewportHit {
                row: 2,
                col: 2,
                side: alacritty_terminal::index::Side::Left,
            })
        );
    }

    /// `Levitating`은 `position()`이 `None`이라 조용히 실패한다. 대조군과 함께
    /// 단언해 "아무 일도 안 일어났다"가 혼자 서지 않게 한다.
    #[test]
    fn a_levitating_cursor_yields_no_hit_while_the_same_point_available_does() {
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(160.0, 160.0));
        let point = Point::new(8.0, 16.0);

        assert_eq!(
            hit_for(
                iced_mouse::Cursor::Levitating(point),
                bounds,
                metrics(),
                grid()
            ),
            None,
            "an overlay is above us — ignoring the cursor is deliberate"
        );
        assert!(
            hit_for(
                iced_mouse::Cursor::Available(point),
                bounds,
                metrics(),
                grid()
            )
            .is_some(),
            "the very same point must hit when the cursor is not levitating"
        );
    }

    #[test]
    fn a_cursor_outside_the_widget_yields_no_hit() {
        let bounds = Rectangle::new(Point::new(100.0, 100.0), iced::Size::new(160.0, 160.0));
        assert_eq!(
            hit_for(
                iced_mouse::Cursor::Available(Point::new(10.0, 10.0)),
                bounds,
                metrics(),
                grid()
            ),
            None
        );
    }

    // ----------------------------------------------------- accumulate_scroll

    #[test]
    fn a_line_delta_becomes_that_many_lines() {
        let mut acc = 0.0;
        let lines = accumulate_scroll(
            &mut acc,
            iced_mouse::ScrollDelta::Lines { x: 0.0, y: 3.0 },
            16.0,
        );
        assert_eq!(lines, 3);
        assert_eq!(acc, 0.0);
    }

    /// **위로 굴리면 양수다.** iced의 `y`도 양수가 위이고
    /// (`scrollable.rs`가 `-y`를 아래 방향 오프셋에 더한다),
    /// `MouseAction::Wheel { lines }`도 양수가 위다. 부호를 뒤집으면 스크롤이
    /// 통째로 반대가 된다.
    #[test]
    fn the_sign_is_preserved_so_that_positive_means_up() {
        let mut acc = 0.0;
        assert_eq!(
            accumulate_scroll(
                &mut acc,
                iced_mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
                16.0
            ),
            1
        );
        acc = 0.0;
        assert_eq!(
            accumulate_scroll(
                &mut acc,
                iced_mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
                16.0
            ),
            -1
        );
    }

    /// 나머지가 보존되지 않으면 트랙패드의 작은 델타가 **영원히 한 줄도**
    /// 만들지 못한다.
    #[test]
    fn small_pixel_deltas_accumulate_into_exactly_one_line() {
        let mut acc = 0.0;
        // 셀 높이 16, 한 번에 6px → 세 번째에 18px이 되어 1줄이 나온다.
        assert_eq!(pixels(&mut acc, 6.0), 0);
        assert_eq!(pixels(&mut acc, 6.0), 0);
        assert_eq!(pixels(&mut acc, 6.0), 1);
        // 남은 2px(=0.125줄)이 보존되어 다음 14px에서 정확히 한 줄이 더 나온다.
        assert_eq!(pixels(&mut acc, 13.0), 0);
        assert_eq!(pixels(&mut acc, 1.0), 1);
    }

    #[test]
    fn a_pixel_delta_smaller_than_a_cell_emits_nothing_but_is_not_lost() {
        let mut acc = 0.0;
        assert_eq!(pixels(&mut acc, 4.0), 0, "quarter of a cell is not a line");
        assert!(acc > 0.0, "but it must be remembered, not discarded");
        // 대조군: 같은 누산기에 나머지를 마저 채우면 한 줄이 나온다.
        assert_eq!(pixels(&mut acc, 12.0), 1);
    }

    #[test]
    fn the_accumulator_survives_a_direction_change_without_drifting() {
        let mut acc = 0.0;
        assert_eq!(pixels(&mut acc, 12.0), 0);
        assert_eq!(pixels(&mut acc, -12.0), 0, "back to where we started");
        assert_eq!(acc, 0.0, "and the accumulator is clean again");
    }

    /// OS가 NaN을 주면 누산기가 영구히 오염돼 스크롤이 다시는 동작하지 않는다.
    #[test]
    fn a_non_finite_delta_is_dropped_without_poisoning_the_accumulator() {
        let mut acc = 0.5;
        assert_eq!(pixels(&mut acc, f32::NAN), 0);
        assert_eq!(acc, 0.5, "the accumulator must be untouched");
        // 대조군: 정상 델타는 계속 동작한다.
        assert_eq!(pixels(&mut acc, 8.0), 1);
    }

    fn pixels(acc: &mut f32, y: f32) -> i32 {
        accumulate_scroll(acc, iced_mouse::ScrollDelta::Pixels { x: 0.0, y }, 16.0)
    }

    // ------------------------------------------------------- held 전이 표

    fn hit() -> ViewportHit {
        ViewportHit {
            row: 1,
            col: 1,
            side: alacritty_terminal::index::Side::Left,
        }
    }

    #[test]
    fn press_sets_held_before_building_the_intent() {
        let mut state = State::default();
        let intent = press_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single)
            .expect("the first press always yields an intent");
        assert_eq!(
            intent.held,
            Some(TermMouseButton::Left),
            "route_mouse rejects Press(b) unless held == Some(b)"
        );
        assert_eq!(state.held, Some(TermMouseButton::Left));
    }

    #[test]
    fn release_carries_the_button_then_clears_held() {
        let mut state = State::default();
        let _ = press_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single);

        let intent = release_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single)
            .expect("the button was held");
        assert_eq!(
            intent.held,
            Some(TermMouseButton::Left),
            "the released button must ride on the intent"
        );
        assert_eq!(state.held, None, "and only then is held cleared");
    }

    /// 창 밖에서 눌렸다 안에서 놓인 경우. intent를 만들면 그리드의 래치가 남의
    /// 제스처를 끊는다.
    #[test]
    fn a_release_for_a_button_that_was_never_pressed_is_dropped() {
        let mut state = State::default();
        assert!(
            release_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single).is_none()
        );
        // 대조군: 눌렸던 버튼이면 만들어진다.
        let _ = press_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single);
        assert!(
            release_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single).is_some()
        );
    }

    /// **무시된 코드 클릭이 더블클릭 사슬을 끊으면 안 된다.**
    ///
    /// 좌클릭 → 우버튼을 스침(무시됨) → 좌클릭. `last_click`을 press 수락 전에
    /// 기록하면 버려진 우클릭이 사슬에 남아 둘째 좌클릭이 `Double`이 아니라
    /// `Single`이 된다. 리뷰가 잡은 1차 수정의 잔여 버그다.
    #[test]
    fn an_ignored_chord_does_not_corrupt_the_double_click_chain() {
        let mut state = wired_state();
        let at = cursor_at(1.0, 1.0);

        let _ = run(&mut state, &press(iced_mouse::Button::Left), at);
        let _ = run(&mut state, &press(iced_mouse::Button::Right), at); // 무시됨
        let _ = run(&mut state, &release(iced_mouse::Button::Left), at);

        let second = run(&mut state, &press(iced_mouse::Button::Left), at);
        let kinds: Vec<ClickKind> = intents(&second).iter().map(|i| i.click).collect();
        assert_eq!(
            kinds,
            vec![ClickKind::Double],
            "the discarded right press must not enter the click chain"
        );
    }

    #[test]
    fn a_release_for_a_different_button_than_the_held_one_is_dropped() {
        let mut state = State::default();
        let _ = press_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single);

        assert!(
            release_intent(&mut state, TermMouseButton::Right, hit(), ClickKind::Single).is_none(),
            "the right button was never pressed"
        );
        assert_eq!(
            state.held,
            Some(TermMouseButton::Left),
            "and the left button must still be held"
        );
    }

    #[test]
    fn motion_and_wheel_carry_held_unchanged_and_do_not_touch_it() {
        let mut state = State::default();
        let _ = press_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single);

        let motion = motion_intent(&state, hit(), ClickKind::Single);
        assert_eq!(motion.held, Some(TermMouseButton::Left));
        assert_eq!(state.held, Some(TermMouseButton::Left));

        let wheel = wheel_intent(&state, hit(), 2, ClickKind::Single);
        assert_eq!(wheel.held, Some(TermMouseButton::Left));
        assert_eq!(state.held, Some(TermMouseButton::Left));
    }

    #[test]
    fn motion_with_no_button_down_carries_none() {
        let state = State::default();
        assert_eq!(motion_intent(&state, hit(), ClickKind::Single).held, None);
    }

    // -------------------------------------------------------- force_local

    #[test]
    fn shift_sets_force_local_on_every_action() {
        let mut state = State {
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            ..State::default()
        };

        assert!(
            press_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single)
                .expect("the first press always yields an intent")
                .force_local
        );
        assert!(motion_intent(&state, hit(), ClickKind::Single).force_local);
        assert!(wheel_intent(&state, hit(), 1, ClickKind::Single).force_local);
        assert!(
            release_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single)
                .expect("held")
                .force_local
        );
    }

    // -------------------------------------------------------- update 배선
    //
    // `Shell`만 있으면 되므로 위젯 트리도 렌더러도 필요 없다. 여기서 보는 것은
    // **어떤 이벤트가 어떤 intent가 되어 나가는가**뿐이다.

    const BOUNDS: Rectangle = Rectangle {
        x: 100.0,
        y: 50.0,
        width: 160.0,
        height: 160.0,
    };

    /// 위젯 안쪽 (col, row) 셀 한가운데를 가리키는 **절대** 좌표.
    fn cursor_at(col: f32, row: f32) -> iced_mouse::Cursor {
        iced_mouse::Cursor::Available(Point::new(
            BOUNDS.x + col * 8.0 + 1.0,
            BOUNDS.y + row * 16.0 + 1.0,
        ))
    }

    fn wired_state() -> State {
        State {
            // 헤드리스에서는 측정이 항상 실패하므로 심어 넣는다.
            metrics: Some(metrics()),
            ..State::default()
        }
    }

    /// 발행된 마우스 intent만 뽑는다. `TermCommand`에 `PartialEq`가 없어
    /// (`Scroll`이 파생하지 않는다) 통째 비교 대신 분해한다.
    fn intents(messages: &[(SessionId, TermCommand)]) -> Vec<MouseIntent> {
        messages
            .iter()
            .filter_map(|(_, cmd)| match cmd {
                TermCommand::Mouse(intent) => Some(*intent),
                _ => None,
            })
            .collect()
    }

    fn run(
        state: &mut State,
        event: &Event,
        cursor: iced_mouse::Cursor,
    ) -> Vec<(SessionId, TermCommand)> {
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        update(
            state,
            SessionId(1),
            event,
            BOUNDS,
            cursor,
            grid(),
            &mut shell,
        );
        messages
    }

    fn press(button: iced_mouse::Button) -> Event {
        Event::Mouse(iced_mouse::Event::ButtonPressed(button))
    }

    fn release(button: iced_mouse::Button) -> Event {
        Event::Mouse(iced_mouse::Event::ButtonReleased(button))
    }

    fn moved() -> Event {
        Event::Mouse(iced_mouse::Event::CursorMoved {
            position: Point::ORIGIN,
        })
    }

    #[test]
    fn a_press_inside_the_widget_publishes_a_press_intent_at_that_cell() {
        let mut state = wired_state();
        let messages = run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(2.0, 3.0),
        );

        let got = intents(&messages);
        assert_eq!(got.len(), 1, "exactly one intent");
        assert!(matches!(
            got[0].action,
            MouseAction::Press(TermMouseButton::Left)
        ));
        assert_eq!(got[0].hit.col, 2);
        assert_eq!(got[0].hit.row, 3);
    }

    /// `pane_grid`는 커서가 근처에도 없는 pane에까지 이벤트를 뿌린다 — bounds
    /// 게이팅이 우리 책임인 이유다.
    #[test]
    fn a_press_outside_the_widget_publishes_nothing() {
        let mut state = wired_state();
        let outside = iced_mouse::Cursor::Available(Point::new(5.0, 5.0));
        let messages = run(&mut state, &press(iced_mouse::Button::Left), outside);
        assert!(intents(&messages).is_empty());
        assert_eq!(state.held, None, "and nothing is latched");

        // 대조군: 같은 이벤트가 안쪽 커서에서는 발행된다.
        let messages = run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(1.0, 1.0),
        );
        assert_eq!(intents(&messages).len(), 1);
    }

    /// **이것이 이 배선에서 가장 중요한 테스트다.** 릴리스를 안 보내면 그리드의
    /// 포인터 래치가 영영 풀리지 않아, 버튼을 뗀 뒤에도 마우스를 움직이는
    /// 것만으로 선택이 계속 따라온다.
    #[test]
    fn a_release_outside_the_widget_still_publishes_so_the_latch_can_clear() {
        let mut state = wired_state();
        let _ = run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(2.0, 2.0),
        );
        assert_eq!(state.held, Some(TermMouseButton::Left));

        let outside = iced_mouse::Cursor::Available(Point::new(-500.0, -500.0));
        let messages = run(&mut state, &release(iced_mouse::Button::Left), outside);

        let got = intents(&messages);
        assert_eq!(got.len(), 1, "the release must go out even from outside");
        assert!(matches!(
            got[0].action,
            MouseAction::Release(TermMouseButton::Left)
        ));
        assert_eq!(
            got[0].hit.col, 2,
            "it falls back to the last cell we actually saw"
        );
        assert_eq!(state.held, None);
    }

    #[test]
    fn a_wheel_inside_the_widget_publishes_lines_and_keeps_the_remainder() {
        let mut state = wired_state();
        // 셀 높이 16 → 8px는 반 줄이라 아직 발행하지 않는다.
        let half = Event::Mouse(iced_mouse::Event::WheelScrolled {
            delta: iced_mouse::ScrollDelta::Pixels { x: 0.0, y: 8.0 },
        });
        assert!(
            intents(&run(&mut state, &half, cursor_at(1.0, 1.0))).is_empty(),
            "half a line is not a line yet"
        );

        // 대조군: 나머지가 남아 있으므로 8px이 더 오면 정확히 한 줄이 나간다.
        let got = intents(&run(&mut state, &half, cursor_at(1.0, 1.0)));
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].action, MouseAction::Wheel { lines: 1 }));
    }

    #[test]
    fn a_captured_event_is_left_alone() {
        let mut state = wired_state();
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        shell.capture_event();
        update(
            &mut state,
            SessionId(1),
            &press(iced_mouse::Button::Left),
            BOUNDS,
            cursor_at(1.0, 1.0),
            grid(),
            &mut shell,
        );
        assert!(intents(&messages).is_empty());
        assert_eq!(state.held, None);

        // 대조군: 캡처되지 않은 같은 이벤트는 발행된다.
        assert_eq!(
            intents(&run(
                &mut state,
                &press(iced_mouse::Button::Left),
                cursor_at(1.0, 1.0)
            ))
            .len(),
            1
        );
    }

    /// 측정 전에는 셀을 모른다. 추측한 좌표로 선택을 시작하느니 아무것도 하지
    /// 않는다.
    #[test]
    fn nothing_is_published_before_the_first_successful_measurement() {
        let mut state = State::default(); // metrics: None
        let messages = run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(1.0, 1.0),
        );
        assert!(intents(&messages).is_empty());

        // 대조군: 메트릭이 생기면 같은 이벤트가 발행된다.
        state.metrics = Some(metrics());
        assert_eq!(
            intents(&run(
                &mut state,
                &press(iced_mouse::Button::Left),
                cursor_at(1.0, 1.0)
            ))
            .len(),
            1
        );
    }

    /// 포커스로 거르면 안 된다 — 포커스 없는 pane을 클릭하는 것이 바로 포커스를
    /// 옮기는 동작이다.
    #[test]
    fn an_unfocused_widget_still_reports_mouse_events() {
        let mut state = wired_state();
        assert!(!state.focused, "the default is unfocused");
        assert_eq!(
            intents(&run(
                &mut state,
                &press(iced_mouse::Button::Left),
                cursor_at(1.0, 1.0)
            ))
            .len(),
            1
        );
    }

    /// 두 버튼을 겹쳐 누르는 것은 평범한 OS 입력이다. 여기서 지켜야 할 것은
    /// **먼저 누른 버튼의 release가 반드시 나간다**는 것 — 안 나가면 마우스
    /// 리포팅 TUI가 press만 받고 영원히 눌린 상태로 남는다.
    ///
    /// 전체 **시퀀스**를 단언한다. 최종 상태만 보면 release가 아예 발행되지
    /// 않아도 `held == None`이라 통과해버린다 — 빠진 것이 이벤트 하나이므로
    /// 끝 상태로는 잡히지 않는다.
    #[test]
    fn chording_a_second_button_never_swallows_the_first_buttons_release() {
        let mut state = wired_state();
        let mut all = Vec::new();
        all.extend(run(&mut state, &press(iced_mouse::Button::Left), cursor_at(1.0, 1.0)));
        all.extend(run(&mut state, &press(iced_mouse::Button::Right), cursor_at(1.0, 1.0)));
        all.extend(run(&mut state, &release(iced_mouse::Button::Left), cursor_at(1.0, 1.0)));
        all.extend(run(&mut state, &release(iced_mouse::Button::Right), cursor_at(1.0, 1.0)));

        let actions: Vec<MouseAction> = intents(&all).iter().map(|i| i.action).collect();
        assert_eq!(
            actions,
            vec![
                MouseAction::Press(TermMouseButton::Left),
                MouseAction::Release(TermMouseButton::Left),
            ],
            "the second press is ignored while a button is held, and the first \
             button's release must still fire"
        );
        assert_eq!(state.held, None, "nothing is left held");
    }

    // ------------------------------------------------- 제스처 유실 복구
    //
    // **불변식: `held`에는 특정 미래 이벤트에 기대지 않는 해소 경로가 있어야
    // 한다.** 없으면 릴리스 하나를 잃는 순간 `press_intent`가 영원히 `None`을
    // 돌려주고, 그 pane의 마우스 입력이 세션 끝까지 조용히 죽는다.

    /// 릴리스가 아예 오지 않아도 **같은 버튼을 다시 누르면 복구된다.**
    /// 버튼을 떼지 않고 두 번 누를 수는 없으므로 둘째 press는 유실의 증거다.
    #[test]
    fn a_lost_release_is_recovered_by_pressing_the_same_button_again() {
        let mut state = wired_state();
        let _ = run(&mut state, &press(iced_mouse::Button::Left), cursor_at(1.0, 1.0));
        // 릴리스가 오지 않는다.

        let got = intents(&run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(2.0, 2.0),
        ));
        let actions: Vec<MouseAction> = got.iter().map(|i| i.action).collect();
        assert_eq!(
            actions,
            vec![
                // 먼저 잃어버린 제스처를 닫고(그리드 래치까지 해소된다),
                MouseAction::Release(TermMouseButton::Left),
                // 그 다음 새 제스처를 연다.
                MouseAction::Press(TermMouseButton::Left),
            ],
            "the stale gesture must be closed before the new one opens"
        );
        assert_eq!(state.held, Some(TermMouseButton::Left));

        // 그리고 계속 동작해야 한다 — 한 번 복구되고 다시 막히면 의미가 없다.
        let _ = run(&mut state, &release(iced_mouse::Button::Left), cursor_at(2.0, 2.0));
        for i in 0..3 {
            let again = intents(&run(
                &mut state,
                &press(iced_mouse::Button::Left),
                cursor_at(1.0, 1.0),
            ));
            assert!(!again.is_empty(), "press #{i} after recovery produced nothing");
            let _ = run(&mut state, &release(iced_mouse::Button::Left), cursor_at(1.0, 1.0));
        }
    }

    /// 릴리스가 다른 위젯에 캡처돼도 **우리가 소유한 제스처는 닫아야 한다.**
    /// `pane_grid`의 `TitleBar`가 있으면 충분히 일어날 수 있다.
    #[test]
    fn a_captured_release_still_resolves_our_own_gesture() {
        let mut state = wired_state();
        let _ = run(&mut state, &press(iced_mouse::Button::Left), cursor_at(1.0, 1.0));

        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        shell.capture_event();
        update(
            &mut state,
            SessionId(1),
            &release(iced_mouse::Button::Left),
            BOUNDS,
            cursor_at(1.0, 1.0),
            grid(),
            &mut shell,
        );

        assert_eq!(state.held, None, "the gesture must be closed");
        assert!(
            matches!(
                intents(&messages).first().map(|i| i.action),
                Some(MouseAction::Release(TermMouseButton::Left))
            ),
            "and the grid's latch must be resolved too"
        );

        // 대조군: 우리가 소유하지 않은 제스처의 캡처된 press는 여전히 무시한다.
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        shell.capture_event();
        update(
            &mut state,
            SessionId(1),
            &press(iced_mouse::Button::Left),
            BOUNDS,
            cursor_at(1.0, 1.0),
            grid(),
            &mut shell,
        );
        assert!(
            intents(&messages).is_empty(),
            "a captured press is still not ours to take"
        );
    }

    /// 드래그 도중 pane이 줄어 눌렀던 셀이 사라져도 `held`는 정리된다.
    /// **리뷰가 짚은 두 경로 밖의 세 번째 경로다** — OS가 이상하게 굴 필요도,
    /// 다른 위젯이 캡처할 필요도 없이 평범한 리사이즈만으로 일어난다.
    #[test]
    fn a_release_that_cannot_be_placed_still_clears_held() {
        let mut state = wired_state();
        let big = GridSize { rows: 10, cols: 20 };
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        update(
            &mut state,
            SessionId(1),
            &press(iced_mouse::Button::Left),
            BOUNDS,
            cursor_at(1.0, 8.0),
            big,
            &mut shell,
        );
        assert_eq!(state.held, Some(TermMouseButton::Left));

        // pane이 2행으로 줄었다 — 8행은 이제 존재하지 않는다.
        let small = GridSize { rows: 2, cols: 20 };
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        update(
            &mut state,
            SessionId(1),
            &release(iced_mouse::Button::Left),
            BOUNDS,
            cursor_at(1.0, 8.0),
            small,
            &mut shell,
        );
        assert_eq!(
            state.held, None,
            "publishing may fail, but the bookkeeping must not"
        );

        // 대조군: 그래서 다음 press가 정상적으로 나간다.
        let after = intents(&run(&mut state, &press(iced_mouse::Button::Left), cursor_at(1.0, 1.0)));
        assert_eq!(after.len(), 1);
    }

    /// 창 포커스를 잃으면 제스처를 닫는다. **마우스 이벤트에 기대지 않는 유일한
    /// 해소 경로**라, OS가 릴리스를 아예 주지 않는 경우의 마지막 그물이다.
    #[test]
    fn losing_window_focus_resolves_a_held_gesture() {
        let mut state = wired_state();
        let _ = run(&mut state, &press(iced_mouse::Button::Left), cursor_at(1.0, 1.0));

        let unfocused = Event::Window(window::Event::Unfocused);
        let got = intents(&run(&mut state, &unfocused, cursor_at(1.0, 1.0)));

        assert!(
            matches!(
                got.first().map(|i| i.action),
                Some(MouseAction::Release(TermMouseButton::Left))
            ),
            "a synthetic release must resolve the grid latch as well"
        );
        assert_eq!(state.held, None);

        // 대조군: 아무것도 눌려 있지 않을 때의 포커스 상실은 조용하다.
        let quiet = intents(&run(&mut state, &unfocused, cursor_at(1.0, 1.0)));
        assert!(quiet.is_empty(), "nothing to resolve, nothing to publish");
    }

    #[test]
    fn a_full_drag_produces_press_motion_release_in_order() {
        let mut state = wired_state();
        let mut all = Vec::new();
        all.extend(run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(0.0, 0.0),
        ));
        all.extend(run(&mut state, &moved(), cursor_at(3.0, 0.0)));
        all.extend(run(
            &mut state,
            &release(iced_mouse::Button::Left),
            cursor_at(3.0, 0.0),
        ));

        let got = intents(&all);
        assert_eq!(got.len(), 3);
        assert!(matches!(
            got[0].action,
            MouseAction::Press(TermMouseButton::Left)
        ));
        assert!(matches!(got[1].action, MouseAction::Motion));
        assert!(matches!(
            got[2].action,
            MouseAction::Release(TermMouseButton::Left)
        ));
        // 그리드의 held 불변식: 모션은 눌린 버튼을 그대로 실어야 한다.
        assert_eq!(got[1].held, Some(TermMouseButton::Left));
    }

    /// 두 번 빠르게 누르면 두 번째 press가 `Double`로 나가야 한다 —
    /// `state.last_click`이 종류를 들고 다니는 것이 여기서 눈에 보인다.
    #[test]
    fn a_second_quick_press_at_the_same_spot_reports_a_double_click() {
        let mut state = wired_state();
        let first = intents(&run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(1.0, 1.0),
        ));
        assert_eq!(first[0].click, ClickKind::Single);

        // 같은 자리에서 곧바로 한 번 더. 릴리스를 사이에 넣어 held를 정리한다.
        let _ = run(
            &mut state,
            &release(iced_mouse::Button::Left),
            cursor_at(1.0, 1.0),
        );
        let second = intents(&run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(1.0, 1.0),
        ));
        assert_eq!(
            second[0].click,
            ClickKind::Double,
            "the previous click's kind must have been remembered"
        );
    }

    /// **세 번째 클릭이 `Triple`이어야 한다.** 두 번까지만 보는 테스트로는
    /// 부족하다 — 첫 클릭의 종류는 어차피 `Single`이라, `last_click`이 종류를
    /// 버리고 항상 `Single`을 적어 넣어도 두 번째는 여전히 `Double`이 나온다.
    /// 세 번째에서야 갈린다. 이 테스트가 `State::last_click`을 `ClickKind`까지
    /// 들고 다니게 넓힌 이유를 붙들고 있는 유일한 지점이다.
    #[test]
    fn a_third_quick_press_reports_a_triple_click() {
        let mut state = wired_state();
        let mut kinds = Vec::new();
        for _ in 0..3 {
            let got = intents(&run(
                &mut state,
                &press(iced_mouse::Button::Left),
                cursor_at(1.0, 1.0),
            ));
            kinds.push(got[0].click);
            let _ = run(
                &mut state,
                &release(iced_mouse::Button::Left),
                cursor_at(1.0, 1.0),
            );
        }
        assert_eq!(
            kinds,
            vec![ClickKind::Single, ClickKind::Double, ClickKind::Triple],
            "triple-click line selection depends on this walk"
        );
    }

    #[test]
    fn motion_outside_the_widget_publishes_nothing() {
        let mut state = wired_state();
        // 먼저 안에서 눌러 last-known 셀을 만들어 둔다 — 그래야 이 테스트가
        // "좌표를 몰라서 안 보냈다"가 아니라 "밖이라서 안 보냈다"를 보게 된다.
        let _ = run(
            &mut state,
            &press(iced_mouse::Button::Left),
            cursor_at(2.0, 2.0),
        );

        let outside = iced_mouse::Cursor::Available(Point::new(-500.0, -500.0));
        assert!(
            intents(&run(&mut state, &moved(), outside)).is_empty(),
            "a drag that leaves the widget stops extending"
        );
        // 대조군: 안쪽 모션은 발행된다.
        assert_eq!(
            intents(&run(&mut state, &moved(), cursor_at(3.0, 2.0))).len(),
            1
        );
    }

    #[test]
    fn a_wheel_outside_the_widget_publishes_nothing() {
        let mut state = wired_state();
        let outside = iced_mouse::Cursor::Available(Point::new(-500.0, -500.0));
        let three_lines = Event::Mouse(iced_mouse::Event::WheelScrolled {
            delta: iced_mouse::ScrollDelta::Lines { x: 0.0, y: 3.0 },
        });
        assert!(
            intents(&run(&mut state, &three_lines, outside)).is_empty(),
            "only the terminal under the cursor scrolls"
        );
        // 대조군: 같은 델타가 안쪽에서는 발행된다.
        assert_eq!(
            intents(&run(&mut state, &three_lines, cursor_at(1.0, 1.0))).len(),
            1
        );
    }

    #[test]
    fn without_shift_force_local_is_off() {
        let mut state = State {
            mods: Mods {
                ctrl: true,
                alt: true,
                logo: true,
                shift: false,
            },
            ..State::default()
        };
        assert!(
            !press_intent(&mut state, TermMouseButton::Left, hit(), ClickKind::Single)
                .expect("the first press always yields an intent")
                .force_local,
            "only shift overrides — ctrl selects block, it does not force local"
        );
    }
}
