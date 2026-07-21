//! 터미널 커스텀 위젯. `iced_core::Widget`를 직접 구현한다.
//!
//! **위젯은 `TerminalSession`을 절대 만지지 않는다.** 스냅샷을 읽어 그리고,
//! 입력을 `TermCommand`로 번역해 발행한다. 세션을 만지는 것은 앱의 `update`뿐이고,
//! 이 경계가 이 플랜의 테스트 가능성의 근거다.
//!
//! **모듈을 먼저 가르는 이유**는 Task 4(입력)와 Task 5(렌더링)가 병렬로 돌기
//! 위해서다. `Widget` 트레이트 impl은 `mod.rs`에 두되 각 메서드 본문은 해당
//! 모듈의 함수에 위임해, 두 태스크가 같은 줄을 건드리지 않게 한다.
//!
//! | 모듈 | 채우는 태스크 |
//! |------|---------------|
//! | `contract` | Task 0 — 위젯 → 앱 커맨드 |
//! | `state`, 이 파일의 `Widget` impl | Task 3 — 뼈대와 순수 레이아웃 계산 |
//! | `input` | Task 4 — 포커스와 키 입력 |
//! | `render`, `palette` | Task 5 — 팔레트와 드로우 |
//! | `mouse` | Task 6 — iced 이벤트 → `MouseIntent` |

pub mod contract;
pub mod input;
pub mod mouse;
pub mod palette;
pub mod render;
pub mod state;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{operation, tree, Tree};
use iced::advanced::{mouse as iced_mouse, renderer, text, widget, Clipboard, Shell};
use iced::{Element, Event, Font, Length, Pixels, Rectangle, Size, Theme};

use suaegi_term::grid::TerminalSnapshot;

use crate::session_store::SessionId;
use crate::terminal::contract::TermCommand;
use crate::terminal::input::Platform;
use crate::terminal::state::State;

/// 위젯이 발행하는 메시지. 위젯이 자기 `SessionId`를 들고 있으므로 커맨드
/// 자체에는 대상이 없다 — 워크벤치가 `.map()`으로 앱 메시지에 싣는다.
pub type Published = (SessionId, TermCommand);

/// 세션의 터미널 위젯을 가리키는 iced 위젯 주소.
///
/// **`SessionId`에서 결정론적으로 만든다.** 앱은 `operation::focus(id)`로 포커스를
/// 옮기는데, 뷰가 프레임마다 새로 만들어지므로 `Id::unique()`를 쓰면 매 프레임
/// 다른 주소가 되어 포커스 조작이 아무 위젯에도 닿지 않는다. 뷰를 만드는
/// 워크벤치와 포커스를 옮기는 `state`가 **같은 함수**를 불러야 둘이 어긋나지
/// 않는다 — 그래서 문자열을 양쪽에 흩어 쓰지 않고 여기 한 곳에 둔다.
pub fn widget_id_for(id: SessionId) -> widget::Id {
    // `Id::new`는 `&'static str`만 받는다 — 세션마다 다른 문자열이 필요하므로
    // `From<String>`으로 만든다.
    widget::Id::from(format!("suaegi-terminal-{}", id.0))
}

/// 한 세션의 터미널 화면.
///
/// 스냅샷을 **빌린다**. 위젯은 뷰가 만들어질 때마다 새로 생기고 상태는
/// `tree.state`에 남으므로, 여기 있는 필드는 전부 "이번 프레임의 설정"이다.
pub struct Terminal<'a> {
    id: SessionId,
    /// `operation::focus(..)`의 표적. **`SessionId`와 다른 것이다** — 이쪽은
    /// iced의 위젯 트리 주소다. `Option`이 아닌 이유: id 없는 터미널은 앱이
    /// 포커스를 줄 수 없어 영영 입력을 못 받는다. 그런 상태를 만들 수 있게
    /// 두느니 `widget_id_for`로 항상 파생시킨다.
    widget_id: widget::Id,
    snapshot: &'a TerminalSnapshot,
    font: Font,
    text_size: Pixels,
    line_height: text::LineHeight,
    /// 단축키 화음이 플랫폼마다 다르다. **필드로 두는 이유**는 `cfg!`가
    /// `classify_shortcut` 안에 들어가면 한쪽 플랫폼의 테스트가 아예 돌지 않기
    /// 때문이다 — 경계는 `Platform::host()` 한 곳뿐이고 여기는 그걸 주입받는다.
    platform: Platform,
}

impl<'a> Terminal<'a> {
    pub fn new(id: SessionId, snapshot: &'a TerminalSnapshot) -> Self {
        Self {
            id,
            widget_id: widget_id_for(id),
            snapshot,
            font: Font::MONOSPACE,
            text_size: Pixels(13.0),
            line_height: text::LineHeight::default(),
            platform: Platform::host(),
        }
    }

    /// 파생된 id를 덮어쓴다. 기본값(`widget_id_for(session_id)`)으로 충분하므로
    /// 테스트가 별개의 위젯 둘을 구분할 때만 쓴다.
    pub fn widget_id(mut self, id: widget::Id) -> Self {
        self.widget_id = id;
        self
    }

    /// 테스트가 양쪽 플랫폼을 강제하기 위한 것. 프로덕션 기본값은
    /// `Platform::host()`다.
    pub fn platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    pub fn font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }

    pub fn text_size(mut self, text_size: impl Into<Pixels>) -> Self {
        self.text_size = text_size.into();
        self
    }

    pub fn line_height(mut self, line_height: impl Into<text::LineHeight>) -> Self {
        self.line_height = line_height.into();
        self
    }

    /// 이번 프레임에 그릴 스냅샷. Task 5의 드로우가 쓴다.
    pub fn snapshot(&self) -> &'a TerminalSnapshot {
        self.snapshot
    }
}

impl<Renderer> iced::advanced::Widget<Published, Theme, Renderer> for Terminal<'_>
where
    Renderer: text::Renderer<Font = Font>,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    /// 터미널은 주어진 자리를 전부 쓴다 — 그리드 크기가 자리에서 나오지, 자리가
    /// 그리드에서 나오지 않는다.
    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();

        // **측정 실패는 캐시를 지우지 않는다.** 지우면 마지막으로 알던 셀 크기가
        // 사라져 리사이즈가 멈춘다 — 화면은 그대로인데 PTY만 낡는 쪽이 더 나쁘다.
        // (헤드리스 하네스의 `()` 렌더러는 **항상** 여기서 실패한다. 그래서
        // 배선 테스트는 상태에 메트릭을 심어 넣는다.)
        if let Some(metrics) =
            state::measure_cell::<Renderer::Paragraph>(self.font, self.text_size, self.line_height)
        {
            state.set_metrics(metrics);
        }

        layout::Node::new(limits.max())
    }

    /// 포커스 조작이 위젯 상태에 닿는 통로. `Focusable` impl은 `state.rs`에 있고
    /// **권위가 아니다** — 포커스 전환과 `FOCUS_IN_OUT` 바이트는 앱이 소유한다
    /// (`focus`/`unfocus`가 `Shell`을 받지 못해 여기서 바이트를 낼 수 없다).
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn operation::Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(Some(&self.widget_id), layout.bounds(), state);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: iced_mouse::Cursor,
        _renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Published>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();

        // 리사이즈는 **레이아웃 사실**이다. 포커스에도 커서 위치에도 걸리지
        // 않으므로 Task 4의 게이팅보다 앞에 둔다.
        state::emit_resize(state, self.id, layout.bounds().size(), shell);

        input::update(state, self.id, event, clipboard, shell, self.platform);

        // 그리드 크기는 **스냅샷의 것**을 넘긴다 — 사용자가 보고 클릭하는 것이
        // 스냅샷이므로 히트테스트도 같은 좌표계여야 한다.
        mouse::update(
            state,
            self.id,
            event,
            layout.bounds(),
            cursor,
            self.snapshot.size,
            shell,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: iced_mouse::Cursor,
        viewport: &Rectangle,
    ) {
        render::draw(
            self,
            tree.state.downcast_ref::<State>(),
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    /// 커서가 위에 있을 때만 텍스트 커서를 요구한다. 무조건 돌려주면 부모가
    /// 걸러주지 않는 경우 창 전체에서 텍스트 커서가 된다.
    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: iced_mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> iced_mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            iced_mouse::Interaction::Text
        } else {
            iced_mouse::Interaction::None
        }
    }
}

impl<'a, Renderer> From<Terminal<'a>> for Element<'a, Published, Theme, Renderer>
where
    Renderer: text::Renderer<Font = Font> + 'a,
{
    fn from(terminal: Terminal<'a>) -> Self {
        Element::new(terminal)
    }
}
