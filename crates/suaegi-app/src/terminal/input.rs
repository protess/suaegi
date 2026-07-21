//! Task 4 — 포커스 게이팅, iced 이벤트 → `KeyInput` 번역, 단축키 분류.
//! `encode_key`는 여기 없다 — 모드가 필요하므로 `suaegi-term::encode`에 있다.
//!
//! 이 파일에 있는 것은 **순수 함수 둘**이다. `suaegi-term`은 iced를 알지
//! 않으므로(의존 방향이 `suaegi-app → suaegi-term` 단방향) iced 타입을
//! 프로토콜 타입으로 옮기는 일은 앱 몫이고, 여기가 그 자리다.

use alacritty_terminal::grid::Scroll;
use iced::advanced::{clipboard, Clipboard, Shell};
use iced::keyboard::key::{Named as IcedNamed, Physical};
use iced::keyboard::{self, Key, Location, Modifiers};
use iced::Event;

use suaegi_term::input_types::{CopyTargets, KeyInput, KeyLocation, Mods, NamedKey, TermKey};

use crate::session_store::SessionId;
use crate::terminal::contract::TermCommand;
use crate::terminal::state::State;
use crate::terminal::Published;

/// iced의 키 이벤트를 프로토콜 타입으로 옮긴다.
///
/// **`modified_key`를 받지 않는다.** `text`가 이미 수식자·IME·데드키 적용
/// 결과를 담고, 제어·단축키 조회는 `physical_latin`이 담당한다. 셋을 다
/// 나르면 어느 것이 권위인지가 흐려진다 — `iced_term`이 캐시된 수식자와
/// 이벤트 수식자를 섞어 쓰다 만든 것과 같은 종류의 실수다.
///
/// `text`는 `Option<&str>`로 받는다. 호출부는 이벤트의 `Option<SmolStr>`을
/// `.as_deref()`로 넘기면 된다.
pub fn to_key_input(
    key: &Key,
    physical_key: Physical,
    location: Location,
    modifiers: Modifiers,
    text: Option<&str>,
    repeat: bool,
) -> KeyInput {
    KeyInput {
        key: to_term_key(key),
        // **제어·단축키 조회 전용이다.** 비US 레이아웃에서 `Ctrl+[`나 `Cmd+C`를
        // 물리 키로 찾기 위한 것이지 문자를 삽입하기 위한 것이 아니다.
        physical_latin: key.to_latin(physical_key),
        location: to_key_location(location),
        mods: to_mods(modifiers),
        text: text.map(str::to_string),
        repeat,
    }
}

/// 논리 키 → `TermKey`.
///
/// **다중 스칼라 규칙**: iced의 논리 키는 문자열이라 스칼라가 0개이거나 2개
/// 이상일 수 있다. 그때는 `Unknown`으로 두되 `KeyInput::text`는 보존한다 —
/// 조합 문자는 인코딩 우선순위 3번(`text`)으로 흘러가 정상 입력된다.
fn to_term_key(key: &Key) -> TermKey {
    match key {
        Key::Named(named) => match to_named_key(*named) {
            Some(n) => TermKey::Named(n),
            // 목록에 없는 명명 키(미디어 키 등)를 조용히 다른 키로 오인하지
            // 않는다. 인코더가 `None`을 돌려준다.
            None => TermKey::Unknown,
        },
        Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => TermKey::Char(c),
                _ => TermKey::Unknown,
            }
        }
        Key::Unidentified => TermKey::Unknown,
    }
}

/// **이 목록이 전부다.** 여기 없는 iced 명명 키는 `TermKey::Unknown`이 된다.
fn to_named_key(named: IcedNamed) -> Option<NamedKey> {
    Some(match named {
        IcedNamed::Enter => NamedKey::Enter,
        IcedNamed::Tab => NamedKey::Tab,
        IcedNamed::Space => NamedKey::Space,
        IcedNamed::Backspace => NamedKey::Backspace,
        IcedNamed::Escape => NamedKey::Escape,
        IcedNamed::Delete => NamedKey::Delete,
        IcedNamed::Insert => NamedKey::Insert,
        IcedNamed::ArrowUp => NamedKey::ArrowUp,
        IcedNamed::ArrowDown => NamedKey::ArrowDown,
        IcedNamed::ArrowLeft => NamedKey::ArrowLeft,
        IcedNamed::ArrowRight => NamedKey::ArrowRight,
        IcedNamed::Home => NamedKey::Home,
        IcedNamed::End => NamedKey::End,
        IcedNamed::PageUp => NamedKey::PageUp,
        IcedNamed::PageDown => NamedKey::PageDown,
        IcedNamed::F1 => NamedKey::F1,
        IcedNamed::F2 => NamedKey::F2,
        IcedNamed::F3 => NamedKey::F3,
        IcedNamed::F4 => NamedKey::F4,
        IcedNamed::F5 => NamedKey::F5,
        IcedNamed::F6 => NamedKey::F6,
        IcedNamed::F7 => NamedKey::F7,
        IcedNamed::F8 => NamedKey::F8,
        IcedNamed::F9 => NamedKey::F9,
        IcedNamed::F10 => NamedKey::F10,
        IcedNamed::F11 => NamedKey::F11,
        IcedNamed::F12 => NamedKey::F12,
        _ => return None,
    })
}

/// `APP_KEYPAD` 분기에 필요하다 — 키패드 키는 같은 논리 키라도 다른 시퀀스를 낸다.
/// **와일드카드를 쓰지 않는다**: iced가 변형을 늘리면 컴파일이 깨져야 한다.
fn to_key_location(location: Location) -> KeyLocation {
    match location {
        Location::Standard => KeyLocation::Standard,
        Location::Left => KeyLocation::Left,
        Location::Right => KeyLocation::Right,
        Location::Numpad => KeyLocation::Numpad,
    }
}

/// **`control()`을 쓴다.** `command()`는 macOS에서 Cmd로 갈라지므로, 그걸 쓰면
/// macOS에서 `Ctrl+화살표`가 아무것도 내지 못한다(`iced_term`의 실제 버그다).
fn to_mods(m: Modifiers) -> Mods {
    Mods {
        shift: m.shift(),
        ctrl: m.control(),
        alt: m.alt(),
        logo: m.logo(),
    }
}

// ---------------------------------------------------------------------------
// 단축키 분류
// ---------------------------------------------------------------------------

/// **인자로 받는다 — `cfg!`를 쓰지 않는다.** `cfg!`였다면 한쪽 플랫폼의 표
/// 테스트가 아예 돌지 않아, 그쪽이 깨져도 CI가 초록으로 남는다. 정확 일치
/// 조회가 만든 것과 같은 종류의 결함이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Other,
}

impl Platform {
    /// 실제 호스트. **`cfg!`가 이 저장소에서 등장해도 되는 유일한 자리다** —
    /// 경계를 여기 하나로 가둬 두면 `classify_shortcut`은 순수하게 남아 양쪽
    /// 플랫폼을 인자로 테스트할 수 있다. 분류 함수 안에서 `cfg!`를 부르면
    /// 한쪽 플랫폼의 표 테스트가 아예 돌지 않는다.
    pub fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Copy,
    Paste,
}

/// 앱 단축키 분류. 인코딩 우선순위 1번이다 — 걸리면 `TermCommand::Key`를
/// 발행하지 않는다.
///
/// **분류와 인코딩을 나누는 이유**: 섞으면 `Ctrl+C`가 복사인지 ETX인지가
/// 함수 안에 숨는다. 여기는 모드를 모르고, 모드를 아는 인코더는 단축키를
/// 모른다.
///
/// 화음은 플랫폼마다 하나씩이다: macOS `Cmd+C`/`Cmd+V`, 그 외
/// `Ctrl+Shift+C`/`Ctrl+Shift+V`. 화음에 없는 수식자가 끼면 단축키가 아니다
/// — `Cmd+Alt+C`는 복사가 아니라 다른 화음이고, 여기서 삼켜 버리면 터미널로
/// 가야 할 입력이 사라진다.
pub fn classify_shortcut(input: &KeyInput, platform: Platform) -> Option<Shortcut> {
    // **오토리피트는 건너뛴다.** 키를 누르고 있는 동안 복사·붙여넣기가 반복
    // 발동하면 안 된다.
    if input.repeat {
        return None;
    }

    let m = input.mods;
    let chord = match platform {
        Platform::Mac => m.logo && !m.ctrl && !m.shift && !m.alt,
        Platform::Other => m.ctrl && m.shift && !m.logo && !m.alt,
    };
    if !chord {
        return None;
    }

    // **단축키 조회도 `physical_latin`이 담당한다** — 비US 레이아웃에서도
    // `Cmd+C`가 물리 C 키에 붙어 있어야 한다. 논리 키는 폴백이다.
    let letter = input.physical_latin.or(match input.key {
        TermKey::Char(c) => Some(c),
        _ => None,
    })?;

    match letter.to_ascii_lowercase() {
        'c' => Some(Shortcut::Copy),
        'v' => Some(Shortcut::Paste),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 키 이벤트 처리 — 게이팅이 전부다
// ---------------------------------------------------------------------------

/// `Widget::update`의 키보드 몫. `mod.rs`가 리사이즈 처리 **뒤에** 부른다.
///
/// **게이팅이 우리 책임인 이유**: `Widget::update`는 포커스와 무관하게 모든 키
/// 이벤트를 받고, `pane_grid`는 커서가 근처에도 없는 pane에까지 이벤트를 뿌린다.
/// 아무도 대신 걸러 주지 않는다.
///
/// **bounds로는 키를 거르지 않는다.** 포커스된 터미널은 커서가 다른 데 있어도
/// 타이핑을 받아야 한다 — 키에 bounds 게이팅을 걸면 마우스를 옆 pane으로 옮기는
/// 순간 입력이 죽는다. bounds 필터링은 커서 좌표가 의미를 갖는 **마우스 경로**
/// (Task 6)의 몫이다.
pub(crate) fn update(
    state: &mut State,
    id: SessionId,
    event: &Event,
    clipboard: &mut dyn Clipboard,
    shell: &mut Shell<'_, Published>,
    platform: Platform,
) {
    // **수식자는 언포커스여도 따라간다.** `iced_term`은 언포커스 상태의 키보드
    // 이벤트를 통째로 버려서 수식자 캐시가 상한다(`view.rs:334` vs `:348`) —
    // 포커스를 잃은 사이에 Shift를 떼면 위젯은 영영 Shift가 눌린 줄 안다.
    // 이 갈래가 포커스 검사보다 **앞에** 있어야 하는 이유다.
    if let Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
        state.mods = to_mods(*modifiers);
        return;
    }

    // 다른 위젯이 이미 가져간 이벤트는 건드리지 않는다. **캡처는 단락이 아니라
    // 플래그이므로** 이 검사가 없으면 같은 키를 두 번 처리할 수 있다.
    if shell.is_event_captured() {
        return;
    }

    if !state.focused {
        return;
    }

    let Event::Keyboard(keyboard::Event::KeyPressed {
        key,
        physical_key,
        location,
        modifiers,
        text,
        repeat,
        ..
    }) = event
    else {
        return;
    };

    let input = to_key_input(
        key,
        *physical_key,
        *location,
        *modifiers,
        text.as_deref(),
        *repeat,
    );

    // 우선순위 1번: 앱 단축키. 모드와 무관하므로 여기서 끝내고 `Key`를 내지 않는다.
    if let Some(shortcut) = classify_shortcut(&input, platform) {
        match shortcut {
            Shortcut::Copy => {
                shell.publish((
                    id,
                    TermCommand::CopySelection {
                        to: CopyTargets::EXPLICIT,
                    },
                ));
            }
            Shortcut::Paste => {
                // **원문 그대로 낸다.** bracketed paste 감싸기는 라이브 모드가
                // 필요하므로 `encode_paste`가 락 안에서 한다 — 여기서 감싸면
                // 모드가 꺼져 있을 때 괄호 문자열이 셸에 그대로 찍힌다.
                if let Some(contents) = clipboard.read(clipboard::Kind::Standard) {
                    shell.publish((id, TermCommand::Scroll(Scroll::Bottom)));
                    shell.publish((id, TermCommand::Paste(contents)));
                }
            }
        }
        shell.capture_event();
        return;
    }

    // 인코딩은 모드를 알아야 하므로 여기서 하지 않는다 — `KeyInput`을 그대로
    // 실어 보내고 `encode_key`가 그리드 락 안에서 한다.
    //
    // 타이핑하면 화면이 맨 아래로 돌아온다. 스크롤백을 보다가 키를 누르면
    // 프롬프트가 보여야 한다. **`Scroll`을 먼저 내는 이유**는 앱이 순서대로
    // 처리하기 때문이다 — 키를 먼저 내면 그 출력이 스크롤 앞에 끼어든다.
    shell.publish((id, TermCommand::Scroll(Scroll::Bottom)));
    shell.publish((id, TermCommand::Key(input)));
    shell.capture_event();
}

// ---------------------------------------------------------------------------
// 테스트
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Code;

    /// 우리 `NamedKey` 전 변형. **와일드카드 없는 `match`와 짝지어 두었으므로**
    /// 변형이 늘면 아래 `iced_named_for`가 컴파일되지 않는다 — 매핑을 빠뜨린 채
    /// 조용히 `Unknown`이 되는 일을 컴파일러가 막는다.
    const ALL_NAMED: [NamedKey; 27] = [
        NamedKey::Enter,
        NamedKey::Tab,
        NamedKey::Space,
        NamedKey::Backspace,
        NamedKey::Escape,
        NamedKey::Delete,
        NamedKey::Insert,
        NamedKey::ArrowUp,
        NamedKey::ArrowDown,
        NamedKey::ArrowLeft,
        NamedKey::ArrowRight,
        NamedKey::Home,
        NamedKey::End,
        NamedKey::PageUp,
        NamedKey::PageDown,
        NamedKey::F1,
        NamedKey::F2,
        NamedKey::F3,
        NamedKey::F4,
        NamedKey::F5,
        NamedKey::F6,
        NamedKey::F7,
        NamedKey::F8,
        NamedKey::F9,
        NamedKey::F10,
        NamedKey::F11,
        NamedKey::F12,
    ];

    /// **와일드카드 금지.** `NamedKey`에 변형이 추가되면 여기서 컴파일이 깨진다.
    fn iced_named_for(k: NamedKey) -> IcedNamed {
        match k {
            NamedKey::Enter => IcedNamed::Enter,
            NamedKey::Tab => IcedNamed::Tab,
            NamedKey::Space => IcedNamed::Space,
            NamedKey::Backspace => IcedNamed::Backspace,
            NamedKey::Escape => IcedNamed::Escape,
            NamedKey::Delete => IcedNamed::Delete,
            NamedKey::Insert => IcedNamed::Insert,
            NamedKey::ArrowUp => IcedNamed::ArrowUp,
            NamedKey::ArrowDown => IcedNamed::ArrowDown,
            NamedKey::ArrowLeft => IcedNamed::ArrowLeft,
            NamedKey::ArrowRight => IcedNamed::ArrowRight,
            NamedKey::Home => IcedNamed::Home,
            NamedKey::End => IcedNamed::End,
            NamedKey::PageUp => IcedNamed::PageUp,
            NamedKey::PageDown => IcedNamed::PageDown,
            NamedKey::F1 => IcedNamed::F1,
            NamedKey::F2 => IcedNamed::F2,
            NamedKey::F3 => IcedNamed::F3,
            NamedKey::F4 => IcedNamed::F4,
            NamedKey::F5 => IcedNamed::F5,
            NamedKey::F6 => IcedNamed::F6,
            NamedKey::F7 => IcedNamed::F7,
            NamedKey::F8 => IcedNamed::F8,
            NamedKey::F9 => IcedNamed::F9,
            NamedKey::F10 => IcedNamed::F10,
            NamedKey::F11 => IcedNamed::F11,
            NamedKey::F12 => IcedNamed::F12,
        }
    }

    fn convert(key: Key) -> KeyInput {
        to_key_input(
            &key,
            Physical::Code(Code::Escape),
            Location::Standard,
            Modifiers::empty(),
            None,
            false,
        )
    }

    // --- 명명 키 매핑 ----------------------------------------------------

    /// 전수 매핑. 빠진 arm은 조용히 `Unknown`이 되어 **키가 사라지므로**
    /// 하나하나 확인한다.
    #[test]
    fn every_named_key_maps() {
        for k in ALL_NAMED {
            let got = convert(Key::Named(iced_named_for(k)));
            assert_eq!(
                got.key,
                TermKey::Named(k),
                "{k:?}가 {:?}로 매핑됐다",
                got.key
            );
        }
    }

    /// 목록에 없는 명명 키는 `Unknown`이다 — 조용히 다른 키로 오인하지 않는다.
    #[test]
    fn unsupported_named_key_becomes_unknown() {
        for named in [
            IcedNamed::F13,
            IcedNamed::MediaPlay,
            IcedNamed::CapsLock,
            IcedNamed::Shift,
        ] {
            assert_eq!(
                convert(Key::Named(named)).key,
                TermKey::Unknown,
                "{named:?}"
            );
        }
    }

    // --- 문자 키 / 다중 스칼라 규칙 ---------------------------------------

    #[test]
    fn single_scalar_character_becomes_char() {
        assert_eq!(convert(Key::Character("c".into())).key, TermKey::Char('c'));
        // 비ASCII도 스칼라가 하나면 `Char`다.
        assert_eq!(
            convert(Key::Character("ㅂ".into())).key,
            TermKey::Char('ㅂ')
        );
        assert_eq!(convert(Key::Character("é".into())).key, TermKey::Char('é'));
    }

    /// **스칼라가 2개 이상이면 `Unknown`이지만 `text`는 보존한다** — 조합
    /// 문자가 인코딩 우선순위 3번(`text`)으로 흘러가 정상 입력되어야 한다.
    #[test]
    fn multi_scalar_character_is_unknown_but_text_survives() {
        let got = to_key_input(
            &Key::Character("ab".into()),
            Physical::Code(Code::KeyA),
            Location::Standard,
            Modifiers::empty(),
            Some("ab"),
            false,
        );
        assert_eq!(got.key, TermKey::Unknown);
        assert_eq!(
            got.text.as_deref(),
            Some("ab"),
            "text가 사라지면 입력이 죽는다"
        );
    }

    /// 스칼라가 0개인 경우도 `Unknown`이다.
    #[test]
    fn empty_character_is_unknown() {
        let got = to_key_input(
            &Key::Character("".into()),
            Physical::Code(Code::KeyA),
            Location::Standard,
            Modifiers::empty(),
            Some(""),
            false,
        );
        assert_eq!(got.key, TermKey::Unknown);
        assert_eq!(got.physical_latin, None);
    }

    #[test]
    fn unidentified_is_unknown() {
        assert_eq!(convert(Key::Unidentified).key, TermKey::Unknown);
    }

    // --- to_latin 폴백 ----------------------------------------------------

    /// **비US 레이아웃 폴백.** 키릴 `с`는 논리적으로 라틴이 아니지만 물리 키가
    /// `KeyC`이므로 `Ctrl+C`/`Cmd+C`가 동작해야 한다.
    #[test]
    fn to_latin_falls_back_through_the_physical_key() {
        let got = to_key_input(
            &Key::Character("с".into()), // U+0441 키릴 es
            Physical::Code(Code::KeyC),
            Location::Standard,
            Modifiers::empty(),
            Some("с"),
            false,
        );
        assert_eq!(got.physical_latin, Some('c'));
        // 논리 키는 여전히 키릴이다 — physical_latin은 **조회 전용**이지
        // 문자를 갈아치우지 않는다.
        assert_eq!(got.key, TermKey::Char('с'));
        assert_eq!(got.text.as_deref(), Some("с"));
    }

    /// 물리 키가 라틴 문자로 번역되지 않으면 `None`이다.
    #[test]
    fn to_latin_is_none_when_untranslatable() {
        // 명명 키는 애초에 문자가 아니다.
        let named = convert(Key::Named(IcedNamed::ArrowLeft));
        assert_eq!(named.physical_latin, None);

        // 키릴인데 물리 키가 문자 키가 아니면 폴백할 곳이 없다.
        let got = to_key_input(
            &Key::Character("с".into()),
            Physical::Code(Code::Escape),
            Location::Standard,
            Modifiers::empty(),
            None,
            false,
        );
        assert_eq!(got.physical_latin, None);
    }

    // --- 수식자 / 위치 / text / repeat -------------------------------------

    /// 4비트 전수. **`control()`을 쓰는지**가 핵심이다 — `command()`였다면
    /// macOS에서 ctrl과 logo가 뒤바뀐다.
    #[test]
    fn modifier_bits_translate_over_all_sixteen_combinations() {
        for bits in 0..16u8 {
            let (shift, ctrl, alt, logo) =
                (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let mut m = Modifiers::empty();
            m.set(Modifiers::SHIFT, shift);
            m.set(Modifiers::CTRL, ctrl);
            m.set(Modifiers::ALT, alt);
            m.set(Modifiers::LOGO, logo);

            let got = to_key_input(
                &Key::Character("a".into()),
                Physical::Code(Code::KeyA),
                Location::Standard,
                m,
                None,
                false,
            );
            assert_eq!(
                got.mods,
                Mods {
                    shift,
                    ctrl,
                    alt,
                    logo
                },
                "bits={bits:04b}"
            );
        }
    }

    #[test]
    fn location_maps_including_numpad() {
        let table = [
            (Location::Standard, KeyLocation::Standard),
            (Location::Left, KeyLocation::Left),
            (Location::Right, KeyLocation::Right),
            (Location::Numpad, KeyLocation::Numpad),
        ];
        for (iced_loc, expect) in table {
            let got = to_key_input(
                &Key::Character("7".into()),
                Physical::Code(Code::Numpad7),
                iced_loc,
                Modifiers::empty(),
                Some("7"),
                false,
            );
            assert_eq!(got.location, expect, "{iced_loc:?}");
        }
    }

    #[test]
    fn text_and_repeat_pass_through() {
        let with_text = to_key_input(
            &Key::Character("q".into()),
            Physical::Code(Code::KeyQ),
            Location::Standard,
            Modifiers::empty(),
            Some("q"),
            true,
        );
        assert_eq!(with_text.text.as_deref(), Some("q"));
        assert!(with_text.repeat);

        let without = convert(Key::Character("q".into()));
        assert_eq!(without.text, None);
        assert!(!without.repeat);
    }

    // --- 단축키 분류 ------------------------------------------------------

    fn chord(mods: Mods, letter: char, repeat: bool) -> KeyInput {
        KeyInput {
            key: TermKey::Char(letter),
            physical_latin: Some(letter),
            location: KeyLocation::Standard,
            mods,
            text: None,
            repeat,
        }
    }

    const MAC_CHORD: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
        logo: true,
    };
    const OTHER_CHORD: Mods = Mods {
        shift: true,
        ctrl: true,
        alt: false,
        logo: false,
    };

    #[test]
    fn mac_chords_classify() {
        assert_eq!(
            classify_shortcut(&chord(MAC_CHORD, 'c', false), Platform::Mac),
            Some(Shortcut::Copy)
        );
        assert_eq!(
            classify_shortcut(&chord(MAC_CHORD, 'v', false), Platform::Mac),
            Some(Shortcut::Paste)
        );
        // 다른 글자는 단축키가 아니다.
        assert_eq!(
            classify_shortcut(&chord(MAC_CHORD, 'x', false), Platform::Mac),
            None
        );
    }

    #[test]
    fn other_chords_classify() {
        assert_eq!(
            classify_shortcut(&chord(OTHER_CHORD, 'c', false), Platform::Other),
            Some(Shortcut::Copy)
        );
        assert_eq!(
            classify_shortcut(&chord(OTHER_CHORD, 'v', false), Platform::Other),
            Some(Shortcut::Paste)
        );
        assert_eq!(
            classify_shortcut(&chord(OTHER_CHORD, 'x', false), Platform::Other),
            None
        );
    }

    /// **양쪽 플랫폼을 인자로 도는 이유가 이것이다.** `cfg!`였다면 이 교차
    /// 검사가 한쪽에서만 돌아 반대쪽 회귀를 놓친다.
    #[test]
    fn chords_do_not_leak_across_platforms() {
        // Cmd+C는 Other에서 단축키가 아니다 — 터미널로 가야 한다.
        assert_eq!(
            classify_shortcut(&chord(MAC_CHORD, 'c', false), Platform::Other),
            None
        );
        // Ctrl+Shift+C는 Mac에서 단축키가 아니다.
        assert_eq!(
            classify_shortcut(&chord(OTHER_CHORD, 'c', false), Platform::Mac),
            None
        );
    }

    /// 화음에 없는 수식자가 끼면 단축키가 아니다. 여기서 삼키면 터미널로 가야
    /// 할 입력이 사라진다.
    #[test]
    fn extra_modifiers_reject_the_chord() {
        let mac_alt = Mods {
            alt: true,
            ..MAC_CHORD
        };
        assert_eq!(
            classify_shortcut(&chord(mac_alt, 'c', false), Platform::Mac),
            None
        );

        let mac_shift = Mods {
            shift: true,
            ..MAC_CHORD
        };
        assert_eq!(
            classify_shortcut(&chord(mac_shift, 'c', false), Platform::Mac),
            None
        );

        let other_alt = Mods {
            alt: true,
            ..OTHER_CHORD
        };
        assert_eq!(
            classify_shortcut(&chord(other_alt, 'c', false), Platform::Other),
            None
        );

        // 대조군: Ctrl만으로는(Shift 없이) Other 화음이 아니다 — Ctrl+C는 ETX다.
        let ctrl_only = Mods {
            shift: false,
            ..OTHER_CHORD
        };
        assert_eq!(
            classify_shortcut(&chord(ctrl_only, 'c', false), Platform::Other),
            None
        );
    }

    /// **오토리피트는 클립보드 동작을 재발동시키지 않는다.** 대조군과 함께
    /// 단언한다 — `repeat=false`에서 걸리는 것을 같이 보여야 의미가 있다.
    #[test]
    fn repeat_suppresses_shortcuts() {
        for (platform, mods) in [(Platform::Mac, MAC_CHORD), (Platform::Other, OTHER_CHORD)] {
            assert_eq!(
                classify_shortcut(&chord(mods, 'c', true), platform),
                None,
                "{platform:?} repeat"
            );
            assert_eq!(
                classify_shortcut(&chord(mods, 'c', false), platform),
                Some(Shortcut::Copy),
                "{platform:?} 대조군"
            );
        }
    }

    /// 단축키 조회도 `physical_latin`을 쓴다 — 키릴 레이아웃에서도 `Cmd+C`가
    /// 물리 C 키에 붙어 있어야 한다.
    #[test]
    fn shortcut_lookup_uses_physical_latin() {
        let cyrillic = KeyInput {
            key: TermKey::Char('с'),
            physical_latin: Some('c'),
            location: KeyLocation::Standard,
            mods: MAC_CHORD,
            text: None,
            repeat: false,
        };
        assert_eq!(
            classify_shortcut(&cyrillic, Platform::Mac),
            Some(Shortcut::Copy)
        );

        // physical_latin이 없으면 논리 키로 폴백한다.
        let fallback = KeyInput {
            physical_latin: None,
            key: TermKey::Char('v'),
            ..cyrillic.clone()
        };
        assert_eq!(
            classify_shortcut(&fallback, Platform::Mac),
            Some(Shortcut::Paste)
        );

        // 둘 다 없으면 단축키가 아니다.
        let neither = KeyInput {
            physical_latin: None,
            key: TermKey::Unknown,
            ..cyrillic
        };
        assert_eq!(classify_shortcut(&neither, Platform::Mac), None);
    }

    /// Shift가 만든 대문자도 같은 단축키다(Other 화음은 Shift를 포함하므로
    /// 논리 키가 `C`로 올라온다).
    #[test]
    fn shortcut_lookup_is_case_insensitive() {
        let upper = KeyInput {
            key: TermKey::Char('C'),
            physical_latin: None,
            location: KeyLocation::Standard,
            mods: OTHER_CHORD,
            text: None,
            repeat: false,
        };
        assert_eq!(
            classify_shortcut(&upper, Platform::Other),
            Some(Shortcut::Copy)
        );
    }
}
