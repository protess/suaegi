//! 헤드리스 위젯 하네스 — 창도 GPU도 OS 입력도 없이 위젯 트리에 합성 이벤트를
//! 흘린다.
//!
//! `iced_core`가 feature gate 없이 `impl Renderer for ()`를 제공하므로
//! (`iced_core/src/renderer/null.rs:10`) `Tree::new` + `Widget::layout` +
//! `Shell::new`만으로 `Widget::update`를 직접 구동할 수 있다. 합성 **OS 클릭**이
//! 아니라 런타임이 위젯에 넘기는 것과 **같은 값**을 넣는 것이므로 금지 규칙에
//! 걸리지 않는다.
//!
//! # 이 하네스로 검증할 수 없는 것 — 먼저 읽을 것
//!
//! `()` 렌더러는 **텍스트를 측정하지 않는다**. `iced_core/src/renderer/null.rs`의
//! `impl text::Paragraph for ()`가 전부 상수를 돌려준다:
//!
//! | 메서드 | 항상 |
//! |--------|------|
//! | `min_bounds()` / `bounds()` | `Size::ZERO` |
//! | `compare(..)` | `Difference::None` |
//! | `grapheme_position(..)` / `hit_test(..)` | `None` |
//! | `with_text(..)` / `with_spans(..)` | `()` (아무것도 하지 않는다) |
//!
//! 따라서:
//!
//! - **셀 메트릭 측정을 여기서 테스트할 수 없다.** 측정은 항상 폭 0을 주고,
//!   `CellMetrics::new`가 그걸 거절한다.
//! - **측정 캐시 무효화를 여기서 테스트할 수 없다.** `compare`가 언제나
//!   `Difference::None`이라 "폰트가 바뀌었다"를 이 렌더러로는 만들 수 없다.
//! - **픽셀·글리프·셰이핑 결과를 여기서 볼 수 없다.** `draw`는 아무 데도
//!   기록되지 않는다.
//!
//! → **측정에 의존하는 계산은 순수 함수로 뽑아 실제 메트릭 값으로 표 테스트하고,
//! 이 하네스는 이벤트/메시지 배선에만 쓴다.** 실제 메트릭이 있어야 도는 배선
//! 테스트는 [`Harness::run_seeded`]로 위젯 상태에 메트릭을 심어 넣는다 —
//! 측정 자체가 아니라 그 **다음**을 보는 것이 목적이기 때문이다.

// 통합 테스트 바이너리마다 이 모듈을 따로 컴파일하므로, 어느 바이너리에서든
// 안 쓰이는 부분이 반드시 생긴다.
#![allow(dead_code)]

use std::cell::RefCell;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::Tree;
use iced::advanced::{clipboard, mouse, Clipboard, Shell};
use iced::{window, Element, Event, Point, Rectangle, Size, Theme};

// ---------------------------------------------------------------- 클립보드 페이크

/// **읽을 수 있는** 클립보드 페이크. `clipboard::Null`은 `read`가 항상 `None`이라
/// 붙여넣기 경로를 아예 돌릴 수 없다 — 위젯이 클립보드를 읽어
/// `TermCommand::Paste`를 내는지 보려면 내용물이 있는 클립보드가 필요하다.
///
/// 읽기/쓰기를 **기록**하는 이유: 복사가 `Standard`와 `Primary` 중 요청된 곳에만
/// 갔는지는 최종 내용물만 봐서는 알 수 없다(같은 값을 양쪽에 써도 한쪽만 봐선
/// 구별되지 않는다). 호출 자체를 남긴다.
#[derive(Debug, Default)]
pub struct RecordingClipboard {
    standard: Option<String>,
    primary: Option<String>,
    /// `Clipboard::read`가 `&self`라 내부 가변성이 필요하다.
    reads: RefCell<Vec<clipboard::Kind>>,
    writes: RefCell<Vec<(clipboard::Kind, String)>>,
}

impl RecordingClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 두 클립보드에 같은 내용을 채운다.
    pub fn seeded(contents: &str) -> Self {
        Self {
            standard: Some(contents.to_owned()),
            primary: Some(contents.to_owned()),
            ..Self::default()
        }
    }

    pub fn set_standard(&mut self, contents: Option<String>) {
        self.standard = contents;
    }

    pub fn set_primary(&mut self, contents: Option<String>) {
        self.primary = contents;
    }

    pub fn standard(&self) -> Option<&str> {
        self.standard.as_deref()
    }

    pub fn primary(&self) -> Option<&str> {
        self.primary.as_deref()
    }

    /// 지금까지 일어난 읽기의 종류를 순서대로.
    pub fn reads(&self) -> Vec<clipboard::Kind> {
        self.reads.borrow().clone()
    }

    /// 지금까지 일어난 쓰기를 순서대로.
    pub fn writes(&self) -> Vec<(clipboard::Kind, String)> {
        self.writes.borrow().clone()
    }
}

impl Clipboard for RecordingClipboard {
    fn read(&self, kind: clipboard::Kind) -> Option<String> {
        self.reads.borrow_mut().push(kind);
        match kind {
            clipboard::Kind::Standard => self.standard.clone(),
            clipboard::Kind::Primary => self.primary.clone(),
        }
    }

    fn write(&mut self, kind: clipboard::Kind, contents: String) {
        self.writes.borrow_mut().push((kind, contents.clone()));
        match kind {
            clipboard::Kind::Standard => self.standard = Some(contents),
            clipboard::Kind::Primary => self.primary = Some(contents),
        }
    }
}

// ------------------------------------------------------------------------ 스텝

/// 하나의 위젯 트리에 순서대로 적용하는 단계. **트리와 위젯 상태는 단계 사이에
/// 유지된다** — press→move→release 같은 상태 기계와, "접혔다가 돌아온" 리사이즈
/// 시나리오를 재현하려면 그래야 한다.
pub enum Step {
    /// 이 크기로 **다시 레이아웃한다.** 상태는 유지된다. 런타임이 매 프레임
    /// `build`(레이아웃) → `update`(이벤트) 순으로 도는 것과 같은 순서다.
    Bounds(Size),
    Event(Event, mouse::Cursor),
}

impl Step {
    /// 커서가 주어진 점에 있는 상태로 이벤트를 흘린다.
    pub fn at(event: Event, cursor: Point) -> Self {
        Step::Event(event, mouse::Cursor::Available(cursor))
    }

    /// 커서 위치를 알 수 없는 상태(창 밖 등)로 이벤트를 흘린다.
    pub fn nowhere(event: Event) -> Self {
        Step::Event(event, mouse::Cursor::Unavailable)
    }
}

// ------------------------------------------------------------------------ 관찰

/// 이벤트 하나가 만든 것 전부. `Shell`은 이벤트마다 새로 만들어지므로
/// 필드들은 **그 이벤트만의** 결과다.
#[derive(Debug)]
pub struct Frame<Message> {
    pub messages: Vec<Message>,
    pub captured: bool,
    pub redraw: window::RedrawRequest,
    pub layout_invalid: bool,
    pub widgets_invalid: bool,
}

#[derive(Debug)]
pub struct Run<Message> {
    pub frames: Vec<Frame<Message>>,
}

impl<Message> Run<Message> {
    /// 모든 프레임의 메시지를 발행 순서대로.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.frames.iter().flat_map(|f| f.messages.iter())
    }

    pub fn into_messages(self) -> Vec<Message> {
        self.frames.into_iter().flat_map(|f| f.messages).collect()
    }

    pub fn message_count(&self) -> usize {
        self.frames.iter().map(|f| f.messages.len()).sum()
    }
}

// ---------------------------------------------------------------------- 하네스

pub struct Harness {
    bounds: Size,
    pub clipboard: RecordingClipboard,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    pub fn new() -> Self {
        Self {
            bounds: Size::new(800.0, 600.0),
            clipboard: RecordingClipboard::new(),
        }
    }

    /// 최초 레이아웃 크기. `Step::Bounds`로 도중에 바꿀 수 있다.
    pub fn with_bounds(mut self, bounds: Size) -> Self {
        self.bounds = bounds;
        self
    }

    pub fn with_clipboard(mut self, clipboard: RecordingClipboard) -> Self {
        self.clipboard = clipboard;
        self
    }

    pub fn run<Message>(
        &mut self,
        element: Element<'_, Message, Theme, ()>,
        steps: &[Step],
    ) -> Run<Message> {
        self.run_seeded(element, steps, |_| {})
    }

    /// `Tree::new` 직후, 첫 레이아웃 **전에** `seed`로 위젯 상태를 손본다.
    ///
    /// 존재 이유는 하나뿐이다: `()` 렌더러가 텍스트를 측정하지 못하므로
    /// (모듈 문서 참고) 실제 셀 메트릭이 있어야 도는 배선을 그냥은 돌릴 수
    /// 없다. 측정 **결과**를 심어 그 다음 단계를 본다 — 측정 자체는 이 하네스의
    /// 검증 대상이 아니다.
    pub fn run_seeded<Message>(
        &mut self,
        element: Element<'_, Message, Theme, ()>,
        steps: &[Step],
        seed: impl FnOnce(&mut Tree),
    ) -> Run<Message> {
        let mut element = element;
        let mut tree = Tree::new(&element);
        seed(&mut tree);

        let mut bounds = self.bounds;
        let mut node = layout_at(&mut element, &mut tree, bounds);
        let mut frames = Vec::new();

        for step in steps {
            match step {
                Step::Bounds(size) => {
                    bounds = *size;
                    node = layout_at(&mut element, &mut tree, bounds);
                }
                Step::Event(event, cursor) => {
                    let mut messages = Vec::new();
                    let mut shell = Shell::new(&mut messages);
                    element.as_widget_mut().update(
                        &mut tree,
                        event,
                        Layout::new(&node),
                        *cursor,
                        &(),
                        &mut self.clipboard,
                        &mut shell,
                        &Rectangle::with_size(bounds),
                    );
                    let captured = shell.is_event_captured();
                    let redraw = shell.redraw_request();
                    let layout_invalid = shell.is_layout_invalid();
                    let widgets_invalid = shell.are_widgets_invalid();
                    drop(shell);
                    frames.push(Frame {
                        messages,
                        captured,
                        redraw,
                        layout_invalid,
                        widgets_invalid,
                    });
                }
            }
        }

        Run { frames }
    }
}

fn layout_at<Message>(
    element: &mut Element<'_, Message, Theme, ()>,
    tree: &mut Tree,
    bounds: Size,
) -> layout::Node {
    let limits = layout::Limits::new(Size::ZERO, bounds);
    element.as_widget_mut().layout(tree, &(), &limits)
}

// ------------------------------------------------------------------ 이벤트 생성자

pub fn press(button: mouse::Button) -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(button))
}

pub fn release(button: mouse::Button) -> Event {
    Event::Mouse(mouse::Event::ButtonReleased(button))
}

pub fn left_press() -> Event {
    press(mouse::Button::Left)
}

pub fn left_release() -> Event {
    release(mouse::Button::Left)
}

pub fn moved(position: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position })
}

pub fn wheel_lines(y: f32) -> Event {
    Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Lines { x: 0.0, y },
    })
}

pub fn wheel_pixels(y: f32) -> Event {
    Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Pixels { x: 0.0, y },
    })
}

/// 런타임이 매 프레임 위젯에 흘리는 이벤트. 리사이즈 감지처럼 "이벤트가 없어도
/// 프레임마다 돌아야 하는" 로직을 헤드리스로 구동하는 데 쓴다
/// (`iced_widget/src/text_input.rs:1330`이 커서 깜빡임에 같은 것을 쓴다).
pub fn redraw() -> Event {
    // `iced_core::time::Instant`는 native에서 `std::time::Instant`의 재수출이다
    // (`web_time`이 native를 std로 넘긴다).
    Event::Window(window::Event::RedrawRequested(std::time::Instant::now()))
}
