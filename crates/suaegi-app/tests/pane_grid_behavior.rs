//! `pane_grid`의 마우스 이벤트 디스패치 동작을 **헤드리스로** 확인한다.
//!
//! iced_core는 `impl Renderer for ()`(renderer/null.rs)를 제공하므로 실제
//! 윈도우/GPU 없이 위젯 트리를 레이아웃하고 `Widget::update`를 직접 합성
//! 이벤트로 구동할 수 있다. 여기서 검증하는 건 우리 코드가 아니라
//! **iced_widget 0.14.2의 동작**이다 — workbench.rs가 그 동작에 의존하므로,
//! iced 업그레이드가 전제를 깨면 이 테스트가 먼저 깨진다.
//!
//! 구동 장치는 `tests/harness`에 있다 — 원래 이 파일 안에 있던 것을 위젯
//! 테스트와 공유하려고 뽑아냈다. 하네스의 한계(측정 불가)는 그 모듈 문서에
//! 적혀 있고, 이 파일의 주장들은 측정에 의존하지 않는다.

mod harness;

use std::cell::RefCell;
use std::rc::Rc;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Clipboard, Shell, mouse, renderer};
use iced::widget::{button, pane_grid, scrollable, text};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme};

use harness::{Harness, Step};

// ---------------------------------------------------------------- 테스트 배선

#[derive(Debug, Clone)]
#[allow(dead_code)] // 필드는 메시지 종류 구분에만 쓰고 값은 읽지 않는다
enum Message {
    PaneClicked(pane_grid::Pane),
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeEvent),
    PaneCloseRequested(pane_grid::Pane),
}

/// 본문 자리에 놓는 프로브. 받은 이벤트를 전부 공유 로그에 적는다.
/// 조건 없이 적는 이유: "자식이 이벤트를 봤는가"가 질문이기 때문이다.
/// 실제 위젯은 자기 bounds를 스스로 검사하지만, 그건 자식의 선택이지
/// pane_grid가 걸러준 결과가 아니다.
struct Probe {
    name: &'static str,
    log: Rc<RefCell<Vec<String>>>,
    height: f32,
    /// true면 프로브가 받은 이벤트를 `shell.capture_event()`로 소비한다.
    /// C2("pane_grid는 캡처를 확인하지 않는다") 검증용.
    capture: bool,
}

impl Probe {
    fn new(name: &'static str, log: &Rc<RefCell<Vec<String>>>, height: f32, capture: bool) -> Self {
        Self {
            name,
            log: Rc::clone(log),
            height,
            capture,
        }
    }
}

impl<Renderer> Widget<Message, Theme, Renderer> for Probe
where
    Renderer: renderer::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(
            Length::Fill,
            Length::Fixed(self.height),
            Size::new(0.0, self.height),
        ))
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let label = match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) => "ButtonPressed",
            Event::Mouse(mouse::Event::ButtonReleased(_)) => "ButtonReleased",
            Event::Mouse(mouse::Event::CursorMoved { .. }) => "CursorMoved",
            Event::Mouse(mouse::Event::WheelScrolled { .. }) => "WheelScrolled",
            _ => return,
        };
        let over = cursor
            .position()
            .is_some_and(|p| layout.bounds().contains(p));
        // `captured`가 이 로그의 핵심이다. pane_grid는 그리드 bounds 위의
        // press에서 **무조건** `shell.capture_event()`를 부른다. 따라서 자식이
        // press를 보는 시점에 captured=false라면, 그건 자식이 pane_grid의 자기
        // 로직보다 **먼저** 돌았다는 직접 증거다(C1).
        self.log.borrow_mut().push(format!(
            "{}:{label}:over={over}:captured={}",
            self.name,
            shell.is_event_captured()
        ));
        if self.capture {
            shell.capture_event();
        }
    }
}

impl<'a, Renderer> From<Probe> for Element<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer + 'a,
{
    fn from(probe: Probe) -> Self {
        Element::new(probe)
    }
}

/// 본문을 어떻게 감쌀지 — C6(스크롤 트랜잭션) 확인용으로만 갈라진다.
#[derive(Clone, Copy, PartialEq)]
enum Body {
    Bare,
    Scrollable,
}

/// workbench.rs와 **동일한** pane_grid 설정을 만들어 이벤트 시퀀스를 흘린다.
///
/// `pane_grid::State`를 여기서 소유하는 이유: `PaneGrid::new`가 그걸 빌리므로
/// Element보다 오래 살아야 한다. 하네스는 트리와 상태를 스텝 사이에 유지하므로
/// press→move→release가 하나의 상태 기계로 이어진다.
fn run(body: Body, capture: bool, events: &[(Event, Point)]) -> (Vec<Message>, Vec<String>) {
    let (mut state, first) = pane_grid::State::new(0usize);
    let (_second, _split) = state
        .split(pane_grid::Axis::Vertical, first, 1usize)
        .expect("split must succeed");

    let log = Rc::new(RefCell::new(Vec::new()));
    let build_log = Rc::clone(&log);

    let grid: Element<'_, Message, Theme, ()> =
        pane_grid::PaneGrid::new(&state, move |pane, index, _is_maximized| {
            let name: &'static str = if *index == 0 { "left" } else { "right" };
            let title_bar = pane_grid::TitleBar::new(text(name).size(13))
                .controls(pane_grid::Controls::new(
                    button(text("x").size(12)).on_press(Message::PaneCloseRequested(pane)),
                ))
                .padding(6);

            // 프로브를 pane보다 훨씬 크게 만들어야 scrollable이 실제로
            // 스크롤 가능해지고, 그래야 트랜잭션 경로를 탄다.
            let probe = Probe::new(name, &build_log, 5_000.0, capture);
            let content: Element<'_, Message, Theme, ()> = match body {
                Body::Bare => probe.into(),
                Body::Scrollable => scrollable(probe)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
            };
            pane_grid::Content::new(content).title_bar(title_bar)
        })
        .spacing(2)
        .on_click(Message::PaneClicked)
        .on_drag(Message::PaneDragged)
        .on_resize(8, Message::PaneResized)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

    let steps: Vec<Step> = events
        .iter()
        .map(|(event, cursor)| Step::at(event.clone(), *cursor))
        .collect();

    let messages = Harness::new()
        .with_bounds(Size::new(800.0, 600.0))
        .run(grid, &steps)
        .into_messages();

    let log = log.borrow().clone();
    (messages, log)
}

fn press() -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
}
fn release() -> Event {
    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
}
fn moved(p: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position: p })
}
fn wheel() -> Event {
    Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
    })
}

fn dragged(messages: &[Message]) -> Vec<&pane_grid::DragEvent> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::PaneDragged(e) => Some(e),
            _ => None,
        })
        .collect()
}

// -------------------------------------------------------------- 관찰 3 (C1)

/// C1: pane_grid는 자식에게 **먼저** 이벤트를 넘긴 뒤 자기 마우스 로직을
/// 돌린다. 분할선 leeway 안이지만 본문 bounds 안이기도 한 지점을 누르면,
/// 자식이 그 press를 보고 **동시에** pane_grid가 리사이즈를 시작한다.
#[test]
fn press_near_divider_reaches_the_body_and_still_starts_a_resize() {
    // 세로 분할선은 x≈400. spacing 2 + leeway 8 = 10 폭 밴드 → 395..405.
    // 오른쪽 pane 본문 안쪽으로 3px 들어간 지점을 고른다.
    let inside_body_but_in_band = Point::new(404.0, 300.0);
    let (messages, log) = run(
        Body::Bare,
        false,
        &[
            (press(), inside_body_but_in_band),
            (moved(Point::new(420.0, 300.0)), Point::new(420.0, 300.0)),
        ],
    );

    assert!(
        log.iter()
            .any(|l| l == "right:ButtonPressed:over=true:captured=false"),
        "the body must receive the press, and `captured=false` proves it ran \
         BEFORE pane_grid's own handler (which captures unconditionally on a \
         press over the grid); log = {log:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::PaneResized(_))),
        "pane_grid must still start a resize from the same press; messages = {messages:?}",
    );
}

// -------------------------------------------------------------------- C2

/// C2: 자식이 `shell.capture_event()`를 불러도 pane_grid는 그걸 확인하지
/// 않고 자기 클릭/리사이즈 로직을 그대로 돌린다.
///
/// 이 테스트는 **자기 대조군을 포함한다**. 캡처하는 실행과 캡처하지 않는
/// 실행을 둘 다 돌려서 pane_grid가 낸 메시지가 같음을 본다 — "캡처가 아무
/// 영향이 없다"가 곧 주장이므로, 한쪽만 돌리면 무엇과 비교해 영향이 없다는
/// 건지 알 수 없다.
#[test]
fn a_child_capturing_the_event_does_not_suppress_pane_grid() {
    let in_band = Point::new(404.0, 300.0);
    let moved_to = Point::new(420.0, 300.0);

    let go = |capture: bool| {
        let (messages, log) = run(
            Body::Bare,
            capture,
            &[(press(), in_band), (moved(moved_to), moved_to)],
        );
        let kinds: Vec<&'static str> = messages
            .iter()
            .map(|m| match m {
                Message::PaneClicked(_) => "Clicked",
                Message::PaneDragged(_) => "Dragged",
                Message::PaneResized(_) => "Resized",
                Message::PaneCloseRequested(_) => "Close",
            })
            .collect();
        (kinds, log)
    };

    let (captured_kinds, captured_log) = go(true);
    let (uncaptured_kinds, _) = go(false);

    assert!(
        captured_log.iter().any(|l| l.contains(":captured=true")),
        "sanity: the probe's capture must really be set; log = {captured_log:?}"
    );
    assert!(
        captured_kinds.contains(&"Resized"),
        "pane_grid must still act despite the child capturing; got {captured_kinds:?}"
    );
    assert_eq!(
        captured_kinds, uncaptured_kinds,
        "pane_grid never checks is_event_captured, so a capturing child must \
         produce exactly the same messages as a non-capturing one"
    );
}

// -------------------------------------------------------------- 관찰 1 (C3)

/// C3: pane 드래그는 타이틀바의 pick 영역에서만 시작된다. 본문 한가운데를
/// 눌러 끌면 `PaneClicked`는 나오지만 `DragEvent::Picked`는 나오지 않는다.
#[test]
fn drag_from_the_body_clicks_but_never_picks_the_pane() {
    let start = Point::new(200.0, 300.0); // 왼쪽 pane 본문 한가운데
    let end = Point::new(300.0, 300.0); // 100px 끌기
    let (messages, log) = run(
        Body::Bare,
        false,
        &[(press(), start), (moved(end), end), (release(), end)],
    );

    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::PaneClicked(_))),
        "a press in the body must publish PaneClicked; messages = {messages:?}",
    );
    assert!(
        dragged(&messages).is_empty(),
        "a press in the body must NOT start a pane drag; drag events = {:?}",
        dragged(&messages)
    );

    assert!(
        log.iter()
            .any(|l| l.starts_with("left:ButtonPressed:over=true")),
        "the body still sees the press; log = {log:?}"
    );
    assert!(
        log.iter().any(|l| l.starts_with("left:CursorMoved")),
        "the body still sees the drag moves; log = {log:?}"
    );
}

// ---------------------------------------------------------- 관찰 2 (C3 + C4)

/// C3의 반대편 + C4: 타이틀바의 빈 여백에서 시작한 드래그는 `Picked`를 내고,
/// 그동안 그 pane의 본문 `update`는 아예 건너뛴다.
#[test]
fn drag_from_the_title_bar_gap_picks_the_pane_and_silences_its_body() {
    // 타이틀바: padding 6, 안에 title text와 controls 버튼. 제목 글자와
    // 컨트롤 사이의 빈 여백을 노린다. 왼쪽 pane 폭 ≈399, 제목은 왼쪽,
    // 닫기 버튼은 오른쪽 끝 → 중간쯤은 pick 영역이다.
    let start = Point::new(200.0, 15.0);
    let end = Point::new(300.0, 300.0);
    let (messages, log) = run(
        Body::Bare,
        false,
        &[(press(), start), (moved(end), end), (release(), end)],
    );

    let drags = dragged(&messages);
    assert!(
        drags
            .iter()
            .any(|e| matches!(e, pane_grid::DragEvent::Picked { .. })),
        "a press in the title bar gap must pick the pane; messages = {messages:?}",
    );

    // C4: picked 이후에 온 이벤트(move, release)를 본문이 보면 안 된다.
    let left_after_pick: Vec<_> = log
        .iter()
        .filter(|l| l.starts_with("left:CursorMoved") || l.starts_with("left:ButtonReleased"))
        .collect();
    assert!(
        left_after_pick.is_empty(),
        "while picked, the pane body's update must be skipped; leaked = {left_after_pick:?}"
    );
    // 대조군: 잡히지 않은 오른쪽 pane은 그 이벤트들을 계속 받는다. 이게
    // 없으면 "왼쪽이 조용한 건 그냥 이벤트가 안 왔기 때문"과 구분이 안 된다.
    assert!(
        log.iter().any(|l| l.starts_with("right:CursorMoved")),
        "the unpicked pane must still receive events (control); log = {log:?}"
    );
}

// -------------------------------------------------------------- 관찰 4 (C6)

/// C6: pane_grid는 휠을 무시하지만, `scrollable`은 첫 스크롤 후 트랜잭션을
/// 열고 그동안 콘텐츠로 휠을 **전달하지 않는다**.
#[test]
fn scrollable_swallows_the_second_wheel_event_during_its_transaction() {
    let over_body = Point::new(200.0, 300.0);
    let (_messages, log) = run(
        Body::Scrollable,
        false,
        &[(wheel(), over_body), (wheel(), over_body)],
    );

    let left_wheels = log
        .iter()
        .filter(|l| l.starts_with("left:WheelScrolled"))
        .count();
    assert_eq!(
        left_wheels, 1,
        "the first wheel reaches the content, the second is swallowed by the \
         scrollable's transaction; log = {log:?}"
    );
}

/// C6의 전반부를 따로: scrollable이 없으면 pane_grid는 휠을 그냥 통과시킨다
/// (자기 로직은 없고, 자식에게는 매번 전달된다).
#[test]
fn without_a_scrollable_every_wheel_event_reaches_the_body() {
    let over_body = Point::new(200.0, 300.0);
    let (_messages, log) = run(
        Body::Bare,
        false,
        &[(wheel(), over_body), (wheel(), over_body)],
    );

    let left_wheels = log
        .iter()
        .filter(|l| l.starts_with("left:WheelScrolled"))
        .count();
    assert_eq!(
        left_wheels, 2,
        "pane_grid itself never swallows wheel events; log = {log:?}"
    );
}
