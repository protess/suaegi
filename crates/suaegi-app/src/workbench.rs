//! `pane_grid` 워크벤치 — 세션마다 캐시된 스냅샷을 터미널 커스텀 위젯
//! (`crate::terminal`)으로 그린다. 세션이 하나도 없으면 pane_grid 자체가 없다 —
//! `pane_grid::State`는 pane 없이 존재할 수 없어서, "아직 세션 없음"과 "세션이
//! 있지만 비어 있음"을 `Option`으로 구분한다.
//!
//! **이 파일의 세 상수는 실측으로 정해진 것이다**(`docs/superpowers/research/`의
//! pane_grid 스파이크): `TitleBar`의 존재, `spacing(4)`, `on_resize(0, ..)`,
//! 그리고 `scrollable`의 **부재**. 각각의 이유가 아래 주석에 붙어 있다 —
//! 넷 다 지우면 조용히 깨지고, 깨진 것이 눈에 잘 안 보이는 종류다.
//!
//! **구독 동일성이 이 파일의 핵심이다.** `Subscription::run`은 `fn` 포인터라
//! 컨텍스트를 캡처할 수 없으므로 `run_with(data, builder)`를 쓴다. `data`의
//! `Hash`가 세션 id **만** 해싱해야 한다 — `Arc<TerminalSession>`을 해싱에
//! 들이면(포인터든, 매 프레임 바뀌는 값이든) 프레임마다 다른 데이터로 보여
//! iced가 구독을 파괴하고 다시 만든다. 스트림이 재시작되는 동안 출력이 오면
//! 그 결과는 새 스트림이 아니라 죽는 스트림에 버려지므로, 터미널이 "가끔
//! 멈칫거리는" 게 아니라 아예 갱신을 멈춘 것처럼 보인다.

use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, Stream};
use iced::widget::{button, container, pane_grid, text};
use iced::{Element, Length, Subscription};

use suaegi_term::session::TerminalSession;

use crate::session_store::SessionId;
use crate::state::{AppState, Message};
use crate::terminal::Terminal;

/// 스트림을 이 간격으로 페이싱한다. `generation()`을 루프에서 그냥 읽으면
/// executor 워커를 점유한 채 busy-spin 하고, `std::thread::sleep`은 async
/// 워커 스레드를 블로킹한다 — 그래서 `tokio::time::sleep`을 쓴다. 60fps
/// 화면 갱신에 충분하면서도 CPU를 태우지 않는 절충값.
///
/// `pub(crate)`인 이유: `session_store.rs`의 `apply_snapshot`이 바쁜
/// 세션에서 재요청을 곧바로 다시 내지 않고 이 값만큼 늦춰, 스냅샷을 요청하는
/// 두 경로(이 구독의 알림, 재요청 루프)가 같은 주기로 안정된다.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// **렌더러에 대해 제네릭인 이유**는 테스트가 이 함수를 *그대로* 구동하기
/// 위해서다. `()` 렌더러(`iced_core/src/renderer/null.rs`)로 같은 트리를 만들면
/// 창도 GPU도 없이 pane_grid에 이벤트를 흘릴 수 있고, 그래야 아래의
/// `spacing`/`on_resize`/`TitleBar`를 **값이 아니라 동작으로** 고정할 수 있다.
/// 뷰를 테스트용으로 따로 만들면 정작 프로덕션이 쓰는 설정은 검사되지 않는다.
pub fn view<R>(state: &AppState) -> Element<'_, Message, iced::Theme, R>
where
    R: iced::advanced::text::Renderer<Font = iced::Font> + 'static,
{
    let Some(panes) = state.panes() else {
        return container(text("Select or create a worktree to start a session"))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
    };

    let grid = pane_grid::PaneGrid::new(panes, |pane, session_id, _is_maximized| {
        let session_id = *session_id;
        // **`TitleBar`는 장식이 아니라 하중을 받는 부재다. 지우지 말 것.**
        // 본문 드래그가 pane을 옮기지 못하게 막는 **유일한** 기제다:
        // `can_be_dragged_at`이 타이틀바가 없으면 무조건 `false`를 돌려주고
        // (`pane_grid/content.rs:413-426`), 드래그는 타이틀바의 pick 영역에서만
        // 시작된다. `shell.capture_event()`로는 막을 수 없다 — `pane_grid`는
        // `is_event_captured`를 **어디서도 확인하지 않으며**, 자식보다 나중에
        // 돈다. 타이틀바 없이 `on_drag`를 걸면 터미널 본문을 긁어 텍스트를
        // 선택하는 동작이 곧바로 pane을 통째로 끌고 다니는 동작이 된다.
        let title = match state.last_input_loss() == Some(session_id) {
            // 입력 유실은 보이는 피드백이 있어야 한다(`WriteOutcome::Dropped` =
            // 사용자가 친 것이 사라졌다). `Suppressed`는 여기 오지 않는다.
            true => format!("{} — input dropped", state.session_title(session_id)),
            false => state.session_title(session_id).to_string(),
        };
        let title_bar = pane_grid::TitleBar::new(text(title).size(13))
            .controls(pane_grid::Controls::new(
                button(text("x").size(12)).on_press(Message::PaneCloseRequested(pane)),
            ))
            .padding(6);

        pane_grid::Content::new(session_body(state, session_id)).title_bar(title_bar)
    })
    // **본문 침범량은 `leeway/2`이고 `spacing`과 무관하다.** 히트밴드는
    // `spacing + leeway` 폭으로 분할선에 중앙정렬되고 본문은 중앙에서
    // `spacing/2`부터 시작하므로, 침범 = `(spacing+leeway)/2 - spacing/2` =
    // `leeway/2`다. `leeway = 0`이면 밴드가 거터와 정확히 일치해 침범이 0이 된다 —
    // 분할선 근처를 눌러도 터미널이 그 press를 선택 시작으로 오해하지 않는다.
    // `spacing`은 **잡기 좋은 폭**을 위해 올린다(4px면 충분하다).
    .spacing(4)
    .on_click(Message::PaneClicked)
    .on_drag(Message::PaneDragged)
    .on_resize(0, Message::PaneResized)
    .width(Length::Fill)
    .height(Length::Fill);

    container(grid)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// **`scrollable`로 감싸지 않는다.** `scrollable`은 첫 스크롤 후 트랜잭션을 열고
/// 그동안 휠 이벤트를 자식에게 **전달하지 않는다**(`scrollable.rs:786-791`) —
/// 터미널이 스크롤백과 마우스 리포팅을 직접 소유하므로 휠은 반드시 위젯까지
/// 닿아야 한다. 터미널은 자기 스크롤백을 스스로 그린다.
fn session_body<R>(state: &AppState, id: SessionId) -> Element<'_, Message, iced::Theme, R>
where
    R: iced::advanced::text::Renderer<Font = iced::Font> + 'static,
{
    // `Terminal::new`가 `widget_id_for(id)`로 위젯 id를 파생시킨다 — 앱의
    // `operation::focus`가 같은 함수를 불러 같은 id에 도달한다.
    Element::from(Terminal::new(id, state.session_store().snapshot(id)))
        .map(|(id, command)| Message::Terminal { id, command })
}

/// `Subscription::run_with`의 `data`. **`session`은 절대 해싱에 참여하지
/// 않는다** — 아래 `Hash` 구현과 하단 테스트가 이걸 지킨다.
#[derive(Clone)]
struct TermFeed {
    id: SessionId,
    session: Arc<TerminalSession>,
}

impl std::hash::Hash for TermFeed {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state); // 오직 id — Arc는 동일성에 참여하지 않는다
    }
}

/// 세션마다 하나씩, `generation()`이 바뀔 때마다 `Message::SessionDirty`를
/// 낸다. 전역 `iced::time::every` 하나로 모든 세션을 훑는 대안은 바쁜
/// 터미널과 유휴 터미널을 같은 주기로 묶으므로 택하지 않았다 — 세션별
/// 구독이라야 각자의 속도로 돈다.
pub fn subscription(state: &AppState) -> Subscription<Message> {
    Subscription::batch(
        state
            .session_store()
            .sessions()
            .map(|(id, session)| Subscription::run_with(TermFeed { id, session }, feed_stream)),
    )
}

fn feed_stream(feed: &TermFeed) -> impl Stream<Item = Message> {
    let id = feed.id;
    let session = Arc::clone(&feed.session);
    // 씨드를 `session.generation()`으로 읽으면(그 값을 읽는 시점 자체가
    // `TerminalSession::start`의 블로킹 스폰과 이 구독의 첫 poll 사이 어딘가라
    // 레이스다) 그 사이 이미 나온 출력이 씨드 값에 흡수돼 사라진다 — 셸이
    // 프롬프트를 찍고 조용히 기다리기만 하면(또는 명령이 그 창 안에서 바로
    // 종료하면) 그 이후로는 `generation()`이 다시 안 바뀌므로 이 pane은
    // 영원히 빈 채로 남는다. `blank_snapshot()`의 generation과 같은 `0`으로
    // 고정해서 씨딩하면, 첫 poll 시점까지 실제로 있었던 모든 출력(언제
    // 일어났든)이 항상 `current != 0`으로 잡힌다 — `generation`은 단조 증가고
    // 실제 출력 없이는 결코 0에서 움직이지 않으므로 오탐은 없다.
    stream::unfold(0u64, move |last_seen| {
        let session = Arc::clone(&session);
        async move {
            loop {
                tokio::time::sleep(POLL_INTERVAL).await;
                let current = session.generation();
                if current != last_seen {
                    return Some((
                        Message::SessionDirty {
                            id,
                            generation: current,
                        },
                        current,
                    ));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    use crate::session_store::SessionStore;

    // ---- pane_grid 설정 상수: 실제로 이벤트를 흘려 확인한다 ----
    //
    // 이 세 상수(`spacing(4)`, `on_resize(0, ..)`, `TitleBar`의 존재)는 전부
    // "지우거나 되돌려도 컴파일은 되고, 깨진 것이 눈에 잘 안 보이는" 종류다.
    // 그래서 값이 아니라 **동작**을 고정한다. PTY는 하나도 띄우지 않는다 —
    // `snapshot(id)`가 모르는 세션에 빈 스냅샷을 주므로 뷰는 세션 없이 만들어진다.

    use crate::terminal::state::{CellMetrics, State as TerminalState};
    use iced::advanced::layout::{self, Layout};
    use iced::advanced::widget::tree as tree_tag;
    use iced::advanced::widget::Tree;
    use iced::advanced::{mouse, Shell};
    use iced::{Event, Point, Rectangle, Size, Theme};

    const BOUNDS: Size = Size::new(800.0, 600.0);

    fn two_pane_state() -> AppState {
        let (mut panes, first) = pane_grid::State::new(SessionId(1));
        panes
            .split(pane_grid::Axis::Vertical, first, SessionId(2))
            .expect("split must succeed");
        AppState::with_panes_for_test(panes)
    }

    /// `workbench::view`가 만든 **진짜** 트리에 이벤트를 흘리고 발행된 메시지를
    /// 모은다.
    fn drive(state: &AppState, events: &[(Event, Point)]) -> Vec<Message> {
        let mut view: iced::Element<'_, Message, Theme, ()> = super::view(state);
        let mut tree = Tree::new(&view);
        let limits = layout::Limits::new(Size::ZERO, BOUNDS);
        let node = view.as_widget_mut().layout(&mut tree, &(), &limits);
        let layout = Layout::new(&node);
        let viewport = Rectangle::with_size(BOUNDS);
        let mut clipboard = iced::advanced::clipboard::Null;

        let mut messages = Vec::new();
        for (event, cursor) in events {
            let mut shell = Shell::new(&mut messages);
            view.as_widget_mut().update(
                &mut tree,
                event,
                layout,
                mouse::Cursor::Available(*cursor),
                &(),
                &mut clipboard,
                &mut shell,
                &viewport,
            );
        }
        messages
    }

    fn press_at(x: f32) -> Vec<(Event, Point)> {
        let p = Point::new(x, 300.0);
        let moved = Point::new(x + 20.0, 300.0);
        vec![
            (
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                p,
            ),
            (
                Event::Mouse(mouse::Event::CursorMoved { position: moved }),
                moved,
            ),
        ]
    }

    fn resized(messages: &[Message]) -> usize {
        messages
            .iter()
            .filter(|m| matches!(m, Message::PaneResized(_)))
            .count()
    }

    /// **`on_resize(0, ..)`가 하는 일 전부.** 본문 침범량은 `leeway/2`이므로
    /// `leeway = 0`이면 리사이즈 밴드가 거터와 정확히 일치하고, 본문 안쪽 press는
    /// 절대 분할 리사이즈를 시작하지 않는다.
    ///
    /// 되돌리면(예: `on_resize(8, ..)`) 분할선 근처에서 터미널 텍스트를 긁는
    /// 동작이 **동시에** 분할선을 끄는 동작이 된다 — 눈으로는 "선택이 이상하게
    /// 튄다"로만 보여 원인을 찾기 어렵다.
    #[test]
    fn a_press_just_inside_the_body_does_not_start_a_split_resize() {
        let state = two_pane_state();

        // 세로 분할선은 x≈400, spacing 4 ⇒ 거터는 398..402. 본문은 402부터.
        let inside_body = drive(&state, &press_at(403.0));
        assert_eq!(
            resized(&inside_body),
            0,
            "a press inside the terminal body must never resize the split; \
             messages = {inside_body:?}"
        );

        // **대조군**: 거터 위의 press는 여전히 리사이즈를 시작해야 한다. 이게
        // 없으면 위의 0이 "leeway가 0이라서"가 아니라 "리사이즈가 아예 배선되지
        // 않아서"로도 설명된다.
        let on_gutter = drive(&state, &press_at(400.0));
        assert!(
            resized(&on_gutter) > 0,
            "control: the divider itself must still be draggable; \
             messages = {on_gutter:?}"
        );
    }

    /// **`TitleBar`가 하중을 받는 부재라는 것.** 본문에서 시작한 드래그는 pane을
    /// 집지 않는다 — 타이틀바가 없으면 `can_be_dragged_at`이 무조건 false를
    /// 돌려주는 것이 유일한 방어선이고, `capture_event()`로는 막을 수 없다.
    #[test]
    fn a_drag_from_the_body_never_picks_up_the_pane() {
        let state = two_pane_state();
        let body = drive(&state, &press_at(200.0));

        let picked = body
            .iter()
            .filter(|m| matches!(m, Message::PaneDragged(pane_grid::DragEvent::Picked { .. })))
            .count();
        assert_eq!(
            picked, 0,
            "dragging inside the terminal must select text, not carry the pane \
             away; messages = {body:?}"
        );
        // 대조군: 타이틀바 여백에서 시작한 드래그는 집어야 한다.
        let title_gap = drive(&state, &press_at_y(200.0, 15.0));
        let picked_from_title = title_gap
            .iter()
            .filter(|m| matches!(m, Message::PaneDragged(pane_grid::DragEvent::Picked { .. })))
            .count();
        assert!(
            picked_from_title > 0,
            "control: the title bar must still pick the pane; messages = {title_gap:?}"
        );
    }

    fn press_at_y(x: f32, y: f32) -> Vec<(Event, Point)> {
        let p = Point::new(x, y);
        let moved = Point::new(x + 100.0, y + 200.0);
        vec![
            (
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                p,
            ),
            (
                Event::Mouse(mouse::Event::CursorMoved { position: moved }),
                moved,
            ),
        ]
    }

    /// **터미널 위젯이 실제로 마운트돼 있다는 것.** `session_body`가 `text()`로
    /// 되돌아가면(Plan 3의 읽기 전용 렌더링) 컴파일은 되고 화면에도 뭔가는
    /// 보이지만 키 입력·마우스·리사이즈가 전부 죽는다 — 위젯 상태 노드의 존재로
    /// 고정한다.
    #[test]
    fn the_pane_body_is_the_terminal_widget() {
        let state = two_pane_state();
        let view: iced::Element<'_, Message, Theme, ()> = super::view(&state);
        let tree = Tree::new(&view);

        assert_eq!(
            count_terminals(&tree),
            2,
            "each of the two panes must host a terminal widget — a plain `text()` \
             body compiles and renders but has no input, mouse, or resize"
        );
    }

    fn count_terminals(tree: &Tree) -> usize {
        let here = usize::from(tree.tag == tree_tag::Tag::of::<TerminalState>());
        here + tree.children.iter().map(count_terminals).sum::<usize>()
    }

    /// `()` 렌더러는 텍스트를 측정하지 못하므로 위젯의 `metrics`가 영영 `None`이고,
    /// 그러면 `mouse::update`가 셀을 모른 채 이벤트를 버린다(그 게이팅은 옳다 —
    /// 좌표를 모르는 마우스 리포트는 보낼 수 없다). 측정 **결과**를 심어 그
    /// 다음을 본다. Task 3 하네스의 `run_seeded`와 같은 취지다.
    fn seed_terminal_metrics(tree: &mut Tree) {
        if tree.tag == tree_tag::Tag::of::<TerminalState>() {
            tree.state
                .downcast_mut::<TerminalState>()
                .set_metrics(CellMetrics::new(8.0, 16.0).expect("8x16 is valid"));
        }
        for child in &mut tree.children {
            seed_terminal_metrics(child);
        }
    }

    /// **휠은 매번 터미널까지 닿아야 한다** — 터미널이 스크롤백과 마우스 리포팅을
    /// 직접 소유하므로, 휠을 삼키거나 포커스로 게이팅하는 층이 중간에 생기면
    /// 스크롤이 조용히 죽는다. 터미널이 실제로 `Mouse` 커맨드를 내는 것으로 본다.
    ///
    /// **이 테스트는 `scrollable`의 부재를 고정하지 못한다 — 시도했고 실패했다.**
    /// mutation(래퍼를 도로 씌우기)이 **살아남았다**. 이유가 중요하다:
    /// `scrollable`의 삼킴은 첫 **실제 스크롤** 뒤에 열리는 트랜잭션에서 일어나는데
    /// (`scrollable.rs:786-791`), 우리 터미널은 `Length::Fill`이라 뷰포트와 크기가
    /// 정확히 같아 **넘칠 것이 없고 따라서 스크롤도 일어나지 않는다.** 즉 우리
    /// 구성에서는 래퍼가 있어도 휠이 그대로 통과한다. `scrollable` 제거는 여전히
    /// 옳지만(불필요한 층이고, 콘텐츠가 넘치는 순간 삼키기 시작한다), 근거로
    /// 인용된 그 실패 모드는 이 구성에서 재현되지 않는다. 삼킴 기제 자체는
    /// `tests/pane_grid_behavior.rs`가 5000px 콘텐츠로 확인한다.
    ///
    /// 이 테스트만 진짜 세션을 하나 띄운다: 휠이 커맨드가 되려면 셀 좌표가 필요하고
    /// (`MouseIntent`가 `hit`을 나른다) 그러려면 스냅샷에 0이 아닌 그리드 크기가
    /// 있어야 하는데, 그건 슬롯을 통해서만 들어간다.
    #[test]
    fn every_wheel_event_reaches_the_terminal() {
        let (state, _id) = one_session_state_with_a_sized_grid();

        let mut view: iced::Element<'_, Message, Theme, ()> = super::view(&state);
        let mut tree = Tree::new(&view);
        seed_terminal_metrics(&mut tree);

        let limits = layout::Limits::new(Size::ZERO, BOUNDS);
        let node = view.as_widget_mut().layout(&mut tree, &(), &limits);
        let layout = Layout::new(&node);
        let viewport = Rectangle::with_size(BOUNDS);
        let mut clipboard = iced::advanced::clipboard::Null;
        let over_body = Point::new(200.0, 300.0);

        let mut messages = Vec::new();
        for _ in 0..3 {
            let mut shell = Shell::new(&mut messages);
            view.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
                }),
                layout,
                mouse::Cursor::Available(over_body),
                &(),
                &mut clipboard,
                &mut shell,
                &viewport,
            );
        }

        let wheels = messages
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    Message::Terminal {
                        command: crate::terminal::contract::TermCommand::Mouse(_),
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            wheels, 3,
            "every wheel event must reach the terminal; messages = {messages:?}"
        );
    }

    /// 슬롯이 있어야 스냅샷에 그리드 크기가 들어간다 — `snapshot(id)`는 모르는
    /// 세션에 0×0짜리 빈 스냅샷을 준다.
    fn one_session_state_with_a_sized_grid() -> (AppState, SessionId) {
        use suaegi_core::domain::WorktreeId;

        let mut state = AppState::default();
        let id = state.session_store_mut().next_id();
        state
            .session_store_mut()
            .accept_started(
                id,
                WorktreeId("/tmp/wheel".into()),
                SessionStore::spawn_throwaway_for_test(),
                true,
            )
            .expect("the throwaway session must be accepted");

        let mut snapshot = crate::session_store::blank_snapshot();
        snapshot.size = suaegi_term::grid::GridSize {
            rows: 25,
            cols: 100,
        };
        state.session_store_mut().apply_snapshot(id, 1, snapshot);

        let (panes, _first) = pane_grid::State::new(id);
        // **세션을 채워둔 바로 그 상태에 얹는다.** `with_panes_for_test`는 새
        // 상태를 만들어 돌려주므로 여기서 쓰면 방금 넣은 슬롯이 사라진다.
        state.set_panes_for_test(panes);
        (state, id)
    }

    fn start_throwaway_session() -> Arc<TerminalSession> {
        Arc::new(SessionStore::spawn_throwaway_for_test())
    }

    /// 뭔가를 한 번 찍고("hello") 그다음엔 아무 출력 없이 그대로 대기하는
    /// 세션 — 프롬프트를 찍고 조용히 기다리는 실제 셸을 흉내낸다.
    fn start_session_that_prints_then_goes_quiet() -> TerminalSession {
        use suaegi_term::pty::PtySpawn;
        use suaegi_term::session::SessionSpec;

        #[cfg(unix)]
        let (program, args) = (
            "sh".to_string(),
            vec!["-c".to_string(), "printf 'hello\\n'; sleep 5".to_string()],
        );
        #[cfg(windows)]
        let (program, args) = (
            "cmd".to_string(),
            vec![
                "/C".to_string(),
                "echo hello && ping -n 6 127.0.0.1 > nul".to_string(),
            ],
        );
        TerminalSession::start(SessionSpec {
            pty: PtySpawn {
                program,
                args,
                cwd: None,
                env: Vec::new(),
                rows: 24,
                cols: 80,
            },
            scrollback: 200,
        })
        .expect("test session must start")
    }

    /// 해시 입력 바이트를 그대로 기록한다. "우연히 같은 u64"가 아니라
    /// "무엇을 해싱했는지"를 직접 본다.
    #[derive(Default)]
    struct RecordingHasher(Vec<u8>);
    impl Hasher for RecordingHasher {
        fn write(&mut self, bytes: &[u8]) {
            self.0.extend_from_slice(bytes);
        }
        fn finish(&self) -> u64 {
            0
        }
    }
    fn recorded<T: Hash>(v: &T) -> Vec<u8> {
        let mut h = RecordingHasher::default();
        v.hash(&mut h);
        h.0
    }

    #[test]
    fn feed_identity_is_exactly_the_session_id_and_nothing_else() {
        // 서로 다른 세션 객체를 같은 id로 감쌌을 때 같아야 한다.
        // 같은 Arc의 클론 둘로 비교하면 포인터를 해싱해도 통과해버린다.
        let a = TermFeed {
            id: SessionId(7),
            session: start_throwaway_session(),
        };
        let b = TermFeed {
            id: SessionId(7),
            session: start_throwaway_session(),
        };
        assert_eq!(recorded(&a), recorded(&b));
        assert_eq!(recorded(&a), recorded(&7u64), "only the id may be hashed");
    }

    #[test]
    fn different_sessions_have_different_identity() {
        let a = TermFeed {
            id: SessionId(7),
            session: start_throwaway_session(),
        };
        let b = TermFeed {
            id: SessionId(8),
            session: start_throwaway_session(),
        };
        assert_ne!(recorded(&a), recorded(&b));
    }

    /// 최종 리뷰 항목 1: 구독이 붙기 **전에** 이미 도착한 출력이 첫 poll에서
    /// 잡히는지. 씨드를 스폰 시점의 `session.generation()`으로 읽으면(고쳐지기
    /// 전 동작) 이 출력이 씨드 값에 흡수돼 `feed_stream`이 다시는
    /// `SessionDirty`를 내지 않는다 — 프롬프트를 찍고 조용히 기다리는 셸의
    /// pane이 영원히 빈 채로 남는 버그였다.
    #[tokio::test]
    async fn output_that_arrives_before_the_subscription_starts_still_reaches_the_cache() {
        use futures::StreamExt;

        let session = Arc::new(start_session_that_prints_then_goes_quiet());

        // 구독을 붙이기 전에, 출력이 실제로 도착할 때까지(generation이
        // 움직일 때까지) 기다린다 — "구독 시작 전에 출력이 이미 왔다"는
        // 전제 자체를 확실히 한다.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while session.generation() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the session never produced its initial output"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let feed = TermFeed {
            id: SessionId(1),
            session,
        };
        let stream = feed_stream(&feed);
        tokio::pin!(stream);

        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect(
                "feed_stream must report output that already arrived before subscription, \
                 not hang forever waiting for a *new* change",
            )
            .expect("the stream must not end");

        assert!(matches!(
            msg,
            Message::SessionDirty {
                id: SessionId(1),
                ..
            }
        ));
    }
}
