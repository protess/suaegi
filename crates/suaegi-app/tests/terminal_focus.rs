//! Task 4(위젯 쪽) — 포커스 게이팅과 키 배선의 헤드리스 테스트.
//!
//! **별도 파일인 이유**는 `terminal_widget.rs`가 Task 3의 리사이즈 배선을 담고
//! 있고 다른 에이전트가 그 파일을 소유하기 때문이다. 하네스 모듈은 공유한다.
//!
//! 여기서 검증하는 것은 **배선**이다 — 어떤 이벤트가 어떤 커맨드를 낳는가.
//! `KeyInput` 변환 자체와 단축키 분류는 순수 함수라 `terminal/input.rs`의 유닛
//! 테스트가 표로 덮는다.

mod harness;

use harness::{Harness, RecordingClipboard, Step};

use iced::advanced::widget::Tree;
use iced::advanced::{clipboard, widget};
use iced::keyboard::key::{Code, Named as IcedNamed, Physical};
use iced::keyboard::{self, Key, Location, Modifiers};
use iced::{Event, Size};

use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::input_types::TermKey;

use suaegi_app::session_store::{blank_snapshot, SessionId};
use suaegi_app::terminal::contract::TermCommand;
use suaegi_app::terminal::input::Platform;
use suaegi_app::terminal::state::{CellMetrics, State};
use suaegi_app::terminal::{Published, Terminal};

// --------------------------------------------------------------------- 준비

fn snapshot() -> TerminalSnapshot {
    blank_snapshot()
}

const ID: SessionId = SessionId(7);

fn session() -> SessionId {
    ID
}

/// 메트릭을 심어 두면 첫 레이아웃에서 리사이즈가 한 번 나간다. 그 커맨드는
/// 이 파일의 관심사가 아니므로 아래 `keys()`가 걸러낸다.
fn seed_focus(focused: bool) -> impl FnOnce(&mut Tree) {
    move |tree: &mut Tree| {
        let state = tree.state.downcast_mut::<State>();
        state.focused = focused;
        state.metrics = CellMetrics::new(8.0, 16.0);
    }
}

fn key_event(key: Key, mods: Modifiers, text: Option<&str>, repeat: bool) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: Physical::Code(Code::KeyA),
        location: Location::Standard,
        modifiers: mods,
        text: text.map(Into::into),
        repeat,
    })
}

fn typed(c: &str) -> Event {
    key_event(Key::Character(c.into()), Modifiers::empty(), Some(c), false)
}

fn chord(c: &str, mods: Modifiers) -> Event {
    key_event(Key::Character(c.into()), mods, None, false)
}

fn run_with(
    focused: bool,
    platform: Platform,
    clipboard: RecordingClipboard,
    steps: &[Step],
) -> (Vec<Published>, RecordingClipboard) {
    let snap = snapshot();
    let mut h = Harness::new()
        .with_bounds(Size::new(800.0, 400.0))
        .with_clipboard(clipboard);
    let element = Terminal::new(session(), &snap).platform(platform).into();
    let run = h.run_seeded(element, steps, seed_focus(focused));
    (run.into_messages(), h.clipboard)
}

fn run(focused: bool, steps: &[Step]) -> Vec<Published> {
    run_with(focused, Platform::Mac, RecordingClipboard::new(), steps).0
}

/// 리사이즈를 뺀 나머지. 리사이즈는 Task 3의 관심사이고 첫 레이아웃에서 반드시
/// 한 번 나간다.
fn non_resize(messages: &[Published]) -> Vec<&TermCommand> {
    messages
        .iter()
        .map(|(_, cmd)| cmd)
        .filter(|cmd| !matches!(cmd, TermCommand::Resize { .. }))
        .collect()
}

fn mac() -> Modifiers {
    Modifiers::LOGO
}

/// 위젯을 **트리를 쥔 채** 돌린다. 하네스는 트리를 소비하므로 위젯 상태를
/// 들여다봐야 하는 테스트(수식자 추적, `operate`)는 이쪽을 쓴다.
struct Driver<'a> {
    element: iced::Element<'a, Published, iced::Theme, ()>,
    tree: Tree,
    node: iced::advanced::layout::Node,
    clipboard: RecordingClipboard,
}

impl<'a> Driver<'a> {
    fn new(focused: bool) -> Driver<'static> {
        Driver::with_snapshot(Box::leak(Box::new(snapshot())), focused, None)
    }

    fn with_snapshot(
        snap: &'a TerminalSnapshot,
        focused: bool,
        widget_id: Option<widget::Id>,
    ) -> Driver<'a> {
        let mut terminal = Terminal::new(session(), snap).platform(Platform::Mac);
        if let Some(id) = widget_id {
            terminal = terminal.widget_id(id);
        }
        let mut element: iced::Element<'a, Published, iced::Theme, ()> = terminal.into();

        let mut tree = Tree::new(&element);
        {
            let state = tree.state.downcast_mut::<State>();
            state.focused = focused;
            state.metrics = CellMetrics::new(8.0, 16.0);
        }

        let limits = iced::advanced::layout::Limits::new(Size::ZERO, Size::new(800.0, 400.0));
        let node = element.as_widget_mut().layout(&mut tree, &(), &limits);

        Driver {
            element,
            tree,
            node,
            clipboard: RecordingClipboard::new(),
        }
    }

    fn state(&self) -> &State {
        self.tree.state.downcast_ref::<State>()
    }

    fn event(&mut self, event: Event) -> Vec<Published> {
        self.drive(event, false)
    }

    /// 형제 위젯이 **이미 가져간** 이벤트를 흘린다. 하네스로는 만들 수 없는
    /// 상황이다 — 하네스는 이벤트마다 새 `Shell`을 만들어 캡처 플래그가 항상
    /// 꺼진 채로 들어온다.
    fn event_already_captured(&mut self, event: Event) -> Vec<Published> {
        self.drive(event, true)
    }

    fn drive(&mut self, event: Event, pre_captured: bool) -> Vec<Published> {
        let mut messages = Vec::new();
        let mut shell = iced::advanced::Shell::new(&mut messages);
        if pre_captured {
            shell.capture_event();
        }
        self.element.as_widget_mut().update(
            &mut self.tree,
            &event,
            iced::advanced::Layout::new(&self.node),
            iced::advanced::mouse::Cursor::Unavailable,
            &(),
            &mut self.clipboard,
            &mut shell,
            &iced::Rectangle::with_size(Size::new(800.0, 400.0)),
        );
        drop(shell);
        messages
    }

    fn operate(&mut self, op: &mut dyn widget::Operation) {
        self.element.as_widget_mut().operate(
            &mut self.tree,
            iced::advanced::Layout::new(&self.node),
            &(),
            op,
        );
    }
}

// --------------------------------------------------------------- 포커스 게이팅

/// **대조군과 함께 단언한다.** "언포커스면 아무 일도 안 일어난다"만 보면
/// `update`가 통째로 죽어 있어도 통과한다.
#[test]
fn keys_are_gated_on_focus() {
    let unfocused = run(false, &[Step::nowhere(typed("a"))]);
    assert!(
        non_resize(&unfocused).is_empty(),
        "언포커스에서 키가 나갔다: {:?}",
        non_resize(&unfocused)
    );

    let focused = run(true, &[Step::nowhere(typed("a"))]);
    let cmds = non_resize(&focused);
    assert!(
        cmds.iter().any(|c| matches!(c, TermCommand::Key(_))),
        "포커스에서 키가 안 나갔다: {cmds:?}"
    );
}

/// **언포커스여도 수식자는 따라간다.**
///
/// `iced_term`은 언포커스 상태의 키보드 이벤트를 통째로 버려서 수식자 캐시가
/// 상한다 — 포커스를 잃은 사이에 Shift를 떼면 위젯은 영영 Shift가 눌린 줄 안다.
///
/// **하네스를 쓰지 않고 위젯을 직접 돌리는 이유**: `Harness::run_seeded`가 트리를
/// 소비해서 `state.mods`를 볼 수 없다. 게이팅을 통과한 **커맨드**로 간접 확인하면
/// 언포커스 경로는 애초에 커맨드를 내지 않으므로 무엇도 단언할 수 없다.
#[test]
fn modifiers_are_tracked_while_unfocused() {
    let mut w = Driver::new(false);

    // 언포커스 상태에서 Shift가 눌렸다.
    w.event(Event::Keyboard(keyboard::Event::ModifiersChanged(
        Modifiers::SHIFT,
    )));
    assert!(
        w.state().mods.shift,
        "언포커스에서 ModifiersChanged가 버려졌다 — 이것이 iced_term의 버그다"
    );

    // 그리고 떼는 것도 따라가야 한다. 이쪽이 실제로 아픈 방향이다:
    // 놓친 해제는 "영원히 눌림"으로 남는다.
    w.event(Event::Keyboard(keyboard::Event::ModifiersChanged(
        Modifiers::empty(),
    )));
    assert!(
        !w.state().mods.shift,
        "언포커스에서 수식자 해제를 놓치면 영영 눌린 줄 안다"
    );

    // 대조군: 포커스 상태에서도 같은 경로가 돈다(언포커스 전용 분기가 아니다).
    let mut f = Driver::new(true);
    f.event(Event::Keyboard(keyboard::Event::ModifiersChanged(
        Modifiers::CTRL,
    )));
    assert!(f.state().mods.ctrl);
}

/// 수식자를 추적하는 것과 **키를 통과시키는 것은 다르다.** 언포커스에서
/// `ModifiersChanged`를 처리하되 커맨드는 내지 않아야 한다.
#[test]
fn tracking_modifiers_does_not_leak_commands_while_unfocused() {
    let mut w = Driver::new(false);
    let msgs = w.event(Event::Keyboard(keyboard::Event::ModifiersChanged(
        Modifiers::SHIFT,
    )));
    assert!(
        non_resize(&msgs).is_empty(),
        "ModifiersChanged가 커맨드를 냈다: {:?}",
        non_resize(&msgs)
    );
}

/// 우리가 처리한 키는 캡처한다 — 그래야 형제 위젯이 같은 키를 두 번 처리하지
/// 않는다. 대조군은 언포커스: 캡처하면 포커스된 pane이 키를 못 받는다.
#[test]
fn handled_keys_are_captured_and_unhandled_ones_are_not() {
    let snap = snapshot();
    let mut h = Harness::new().with_bounds(Size::new(800.0, 400.0));
    let element = Terminal::new(session(), &snap).into();
    let run = h.run_seeded(element, &[Step::nowhere(typed("a"))], seed_focus(true));
    assert!(
        run.frames.last().expect("한 프레임은 있어야 한다").captured,
        "키를 처리했으면 캡처해야 한다"
    );

    let snap2 = snapshot();
    let mut h2 = Harness::new().with_bounds(Size::new(800.0, 400.0));
    let element2 = Terminal::new(session(), &snap2).into();
    let run2 = h2.run_seeded(element2, &[Step::nowhere(typed("a"))], seed_focus(false));
    assert!(
        !run2.frames.last().unwrap().captured,
        "언포커스에서 캡처하면 포커스된 pane이 키를 못 받는다"
    );
}

/// **이미 캡처된 이벤트는 건드리지 않는다.** 캡처는 단락이 아니라 플래그라
/// 런타임이 우리를 계속 호출한다 — 우리가 직접 확인하지 않으면 형제가 처리한
/// 키를 한 번 더 PTY로 보낸다.
#[test]
fn an_already_captured_event_is_ignored() {
    let mut w = Driver::new(true);
    let msgs = w.event_already_captured(typed("a"));
    assert!(
        non_resize(&msgs).is_empty(),
        "이미 캡처된 이벤트를 처리했다: {:?}",
        non_resize(&msgs)
    );

    // 대조군: 캡처만 빼면 같은 이벤트가 커맨드를 낸다.
    let mut w2 = Driver::new(true);
    let msgs2 = w2.event(typed("a"));
    assert!(
        non_resize(&msgs2)
            .iter()
            .any(|c| matches!(c, TermCommand::Key(_))),
        "대조군이 비었다면 위 단언은 공허하다"
    );
}

// ------------------------------------------------------------------ 키 배선

/// 키를 치면 화면이 맨 아래로 돌아온다 — 스크롤백을 보다 타이핑하면 프롬프트가
/// 보여야 한다. `Scroll`이 `Key`보다 **먼저** 나가야 그 뒤의 출력이 끼어들지 않는다.
#[test]
fn typing_scrolls_to_the_bottom_before_the_key() {
    let msgs = run(true, &[Step::nowhere(typed("a"))]);
    let cmds = non_resize(&msgs);

    assert!(
        matches!(cmds.first(), Some(TermCommand::Scroll(_))),
        "첫 커맨드가 Scroll이어야 한다: {cmds:?}"
    );
    assert!(
        matches!(cmds.get(1), Some(TermCommand::Key(_))),
        "그 다음이 Key여야 한다: {cmds:?}"
    );
}

/// 명명 키도 그대로 실려 나간다 — 인코딩은 모드를 아는 `encode_key`가 한다.
#[test]
fn named_keys_are_published_unencoded() {
    let event = key_event(
        Key::Named(IcedNamed::ArrowUp),
        Modifiers::empty(),
        None,
        false,
    );
    let msgs = run(true, &[Step::nowhere(event)]);

    let key = non_resize(&msgs)
        .into_iter()
        .find_map(|c| match c {
            TermCommand::Key(input) => Some(input.clone()),
            _ => None,
        })
        .expect("Key 커맨드가 있어야 한다");

    assert!(matches!(key.key, TermKey::Named(_)), "{:?}", key.key);
}

/// `KeyReleased`는 아무것도 내지 않는다. 대조군은 같은 키의 `KeyPressed`다.
#[test]
fn key_release_publishes_nothing() {
    let released = Event::Keyboard(keyboard::Event::KeyReleased {
        key: Key::Character("a".into()),
        modified_key: Key::Character("a".into()),
        physical_key: Physical::Code(Code::KeyA),
        location: Location::Standard,
        modifiers: Modifiers::empty(),
    });

    let msgs = run(true, &[Step::nowhere(released)]);
    assert!(
        non_resize(&msgs).is_empty(),
        "릴리스에서 커맨드가 나갔다: {:?}",
        non_resize(&msgs)
    );

    // 대조군.
    let pressed = run(true, &[Step::nowhere(typed("a"))]);
    assert!(!non_resize(&pressed).is_empty());
}

// ---------------------------------------------------------------- 단축키 배선

#[test]
fn copy_shortcut_publishes_copy_and_not_a_key() {
    let msgs = run(true, &[Step::nowhere(chord("c", mac()))]);
    let cmds = non_resize(&msgs);

    assert!(
        cmds.iter()
            .any(|c| matches!(c, TermCommand::CopySelection { .. })),
        "{cmds:?}"
    );
    assert!(
        !cmds.iter().any(|c| matches!(c, TermCommand::Key(_))),
        "단축키가 걸렸으면 Key를 내면 안 된다: {cmds:?}"
    );
}

/// 위젯이 클립보드를 **읽어** 원문 그대로 낸다. 감싸기는 라이브 모드가 필요해
/// `encode_paste`가 한다 — 여기서 감싸면 모드가 꺼져 있을 때 괄호 문자열이
/// 셸에 그대로 찍힌다.
#[test]
fn paste_shortcut_reads_the_clipboard_and_publishes_it_raw() {
    let raw = "echo hi\nrm -rf /\x1b[201~";
    let (msgs, clip) = run_with(
        true,
        Platform::Mac,
        RecordingClipboard::seeded(raw),
        &[Step::nowhere(chord("v", mac()))],
    );

    let pasted = non_resize(&msgs)
        .into_iter()
        .find_map(|c| match c {
            TermCommand::Paste(s) => Some(s.clone()),
            _ => None,
        })
        .expect("Paste 커맨드가 있어야 한다");

    assert_eq!(pasted, raw, "위젯이 클립보드 내용을 가공했다");
    assert_eq!(
        clip.reads(),
        vec![clipboard::Kind::Standard],
        "표준 클립보드를 정확히 한 번 읽어야 한다"
    );
}

/// 클립보드가 비어 있으면 아무것도 내지 않는다 — 빈 `Paste`를 내면 앱이 빈
/// 쓰기를 큐에 넣는다.
#[test]
fn an_empty_clipboard_publishes_no_paste() {
    let (msgs, _) = run_with(
        true,
        Platform::Mac,
        RecordingClipboard::new(),
        &[Step::nowhere(chord("v", mac()))],
    );
    assert!(
        !non_resize(&msgs)
            .iter()
            .any(|c| matches!(c, TermCommand::Paste(_))),
        "빈 클립보드에서 Paste가 나갔다"
    );
}

/// **플랫폼이 인자라서 양쪽을 다 돌 수 있다.** `cfg!`였다면 이 테스트의 절반은
/// 어느 호스트에서도 실행되지 않는다.
#[test]
fn the_same_chord_means_different_things_per_platform() {
    // Cmd+C: Mac에서는 복사, Other에서는 그냥 키다.
    let on_mac = run_with(
        true,
        Platform::Mac,
        RecordingClipboard::new(),
        &[Step::nowhere(chord("c", mac()))],
    )
    .0;
    assert!(non_resize(&on_mac)
        .iter()
        .any(|c| matches!(c, TermCommand::CopySelection { .. })));

    let on_other = run_with(
        true,
        Platform::Other,
        RecordingClipboard::new(),
        &[Step::nowhere(chord("c", mac()))],
    )
    .0;
    let cmds = non_resize(&on_other);
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, TermCommand::CopySelection { .. })),
        "Other에서 Cmd+C가 복사가 됐다: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| matches!(c, TermCommand::Key(_))),
        "Other에서는 Cmd+C가 터미널로 가야 한다: {cmds:?}"
    );
}

/// 오토리피트가 클립보드 동작을 재발동시키지 않는다. 대조군은 같은 화음의
/// 비반복 이벤트다.
#[test]
fn repeated_shortcuts_do_not_refire() {
    let repeated = key_event(Key::Character("v".into()), mac(), None, true);
    let (msgs, clip) = run_with(
        true,
        Platform::Mac,
        RecordingClipboard::seeded("x"),
        &[Step::nowhere(repeated)],
    );
    assert!(
        !non_resize(&msgs)
            .iter()
            .any(|c| matches!(c, TermCommand::Paste(_))),
        "오토리피트가 붙여넣기를 재발동했다"
    );
    assert!(
        clip.reads().is_empty(),
        "오토리피트가 클립보드를 읽었다: {:?}",
        clip.reads()
    );

    // 대조군.
    let (msgs2, clip2) = run_with(
        true,
        Platform::Mac,
        RecordingClipboard::seeded("x"),
        &[Step::nowhere(chord("v", mac()))],
    );
    assert!(non_resize(&msgs2)
        .iter()
        .any(|c| matches!(c, TermCommand::Paste(_))));
    assert_eq!(clip2.reads().len(), 1);
}

// ------------------------------------------------------------------- operate

/// `operation::focus(id)`가 위젯 상태에 닿는다. **`Focusable`은 권위가 아니지만**
/// (바이트는 앱이 낸다) 이 통로가 없으면 앱이 포커스를 옮길 방법이 아예 없다.
#[test]
fn the_focus_operation_reaches_the_widget_state() {
    let snap = snapshot();
    let id = widget::Id::new("term-under-test");
    let mut w = Driver::with_snapshot(&snap, false, Some(id.clone()));

    assert!(!w.state().focused, "기본값은 언포커스여야 한다");

    w.operate(&mut widget::operation::focusable::focus::<()>(id.clone()));
    assert!(w.state().focused, "focus(id)가 상태에 닿지 않았다");

    w.operate(&mut widget::operation::focusable::unfocus::<()>());
    assert!(!w.state().focused, "unfocus가 상태에 닿지 않았다");
}

/// **`focus(다른_id)`는 이 위젯을 명시적으로 언포커스한다.**
///
/// iced의 `focus` 오퍼레이션이 일치하지 않는 모든 focusable에 `unfocus()`를
/// 부른다(`iced_core/src/widget/operation/focusable.rs:42-48`) — "포커스는 하나뿐"을
/// 오퍼레이션이 스스로 강제하는 것이다. 앱이 이전 터미널을 손으로 언포커스할
/// 필요가 없다는 뜻이라 배선에 직접 영향이 있다.
///
/// **그렇다고 `FOCUS_IN_OUT` 바이트까지 알아서 나가지는 않는다**(플랜 0.9) —
/// 상태 플래그만 뒤집힌다. 이전 세션에 focus-out, 새 세션에 focus-in을 그 순서로
/// 보내는 것은 여전히 앱 몫이다.
#[test]
fn focusing_another_widget_unfocuses_this_one() {
    let snap = snapshot();
    let id = widget::Id::new("mine");
    let mut w = Driver::with_snapshot(&snap, true, Some(id));

    assert!(w.state().focused, "출발점은 포커스 상태다");

    w.operate(&mut widget::operation::focusable::focus::<()>(
        widget::Id::new("someone-else"),
    ));
    assert!(
        !w.state().focused,
        "다른 위젯이 포커스를 가져가면 이쪽은 언포커스여야 한다"
    );
}

/// **앱과 위젯이 같은 id를 각자 계산한다.** 앱은 `widget_id_for(session_id)`로
/// 포커스를 옮기고 위젯은 같은 함수로 자기 주소를 만든다 — 둘이 어긋나면
/// 포커스 조작이 아무 위젯에도 닿지 않아 터미널이 영영 입력을 못 받는다.
#[test]
fn the_app_can_focus_a_terminal_by_its_session_id_alone() {
    let snap = snapshot();
    // `widget_id`를 **덮어쓰지 않는다** — 기본 파생값이 표적이 되어야 한다.
    let mut w = Driver::with_snapshot(&snap, false, None);

    w.operate(&mut widget::operation::focusable::focus::<()>(
        suaegi_app::terminal::widget_id_for(ID),
    ));
    assert!(
        w.state().focused,
        "앱이 SessionId만으로 계산한 id가 위젯에 닿지 않았다"
    );
}
