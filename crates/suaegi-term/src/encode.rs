//! 터미널 입력 인코더. **순수 함수만 있다** — 락도, 세션도, I/O도 없다.
//! 전부 `(입력, TermMode) → 바이트` 모양이다.
//!
//! 모드를 **인자로 받는** 것이 이 모듈의 존재 이유다. 어떤 모드 캐시도
//! correctness에 쓸 수 없다 — `feed()`가 최대 64KiB 청크를 락 쥔 채 처리하므로
//! 청크 중간에 `BRACKETED_PASTE`가 켜져도 미러는 청크가 끝나야 갱신된다.
//! 그 창에서 인코딩하면 **개행이 든 붙여넣기가 그대로 실행된다.** 따라서
//! 호출자(`TerminalGrid`)가 락을 쥔 채 진짜 모드를 읽어 여기 넘긴다.
//!
//! 레거시 xterm 인코딩만 다룬다. kitty 키보드 프로토콜은 꺼져 있다
//! (`Config::kitty_keyboard`가 기본 `false`이고 `TerminalGrid::new`는
//! `scrolling_history`만 덮어쓴다).

use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;

use crate::input_types::{
    ClickKind, KeyInput, KeyLocation, Mods, MouseAction, MouseEncodeError, MouseIntent, MouseRoute,
    NamedKey, TermKey, TermMouseButton,
};

const ESC: u8 = 0x1b;

/// 레거시 마우스 좌표의 전송 값 오프셋. **`33 + 좌표`다** — 관례적인 `32`에
/// 0-based → 1-based 변환이 더해진 값이라 33이 된다.
const LEGACY_WIRE_OFFSET: usize = 33;

/// `UTF8_MOUSE`에서 좌표를 두 바이트로 내보내기 시작하는 경계. **좌표 공간의
/// 값이지 전송 값이 아니다.**
///
/// 좌표 95는 전송 값 `33 + 95 = 128`이 된다 — 2바이트 UTF-8이 시작되는 바로 그
/// 지점이므로 `0xC2 0x80`이 나오고 overlong 시퀀스가 생기지 않는다. 경계를 전송
/// 값 쪽에 두면 이 정합이 깨진다.
const UTF8_TWO_BYTE_COORD_THRESHOLD: usize = 95;

/// 단일 바이트 좌표의 **배타적** 상한. 마지막 좌표 222가 전송 값 255가 된다.
const LEGACY_MAX_COORD: usize = 223;
/// 2바이트 UTF-8 좌표의 **배타적** 상한. 마지막 좌표 2014가 전송 값
/// 2047 = U+07FF가 된다.
const UTF8_MAX_COORD: usize = 2015;

// ---------------------------------------------------------------------------
// 키
// ---------------------------------------------------------------------------

/// 키 입력을 PTY 바이트로. 보낼 것이 없으면 `None`(억제).
///
/// 우선순위는 플랜 0.10의 2~4번이다. 1번(앱 단축키)은 모드와 무관하므로 위젯이
/// 이 함수를 부르기 **전에** 처리한다.
///
/// 1. 키패드(`APP_KEYPAD` + `KeyLocation::Numpad`)
/// 2. 명명 키 / 제어 인코딩
/// 3. `text` — IME·데드키·평범한 타이핑
/// 4. `TermKey::Char` 폴백(`text`가 없을 때)
pub fn encode_key(input: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    // macOS의 Cmd, 그 외의 Super는 **터미널 입력이 아니다.** 위젯의
    // `classify_shortcut`이 Copy/Paste가 아니라고 판정한 나머지 Cmd 조합이
    // 여기까지 흘러오면 `text` 갈래에서 맨 글자가 셸로 나간다 — `Cmd+W`가
    // `w`를 입력하는 식이다.
    if input.mods.logo {
        return None;
    }

    if let Some(bytes) = encode_keypad(input, mode) {
        return Some(bytes);
    }

    if let TermKey::Named(named) = input.key {
        return encode_named(named, input, mode);
    }

    // Ctrl 조합. 매핑이 없으면(`Ctrl+1` 등) 아래 `text`로 흘려보낸다 —
    // 여기서 `None`을 돌려주면 정상 입력이 사라진다.
    if input.mods.ctrl {
        if let Some(byte) = control_byte(input) {
            return Some(alt_prefixed(input.mods.alt, vec![byte]));
        }
    }

    // Alt 조합의 메타 프리픽스는 **수식되지 않은** 문자에 붙인다. macOS에서
    // Option+a는 `text`에 합성된 `å`를 실어 보내는데, 사용자가 뜻한 것은 meta-a다
    // — `text`를 그대로 쓰면 `ESC å`가 나간다. "Option as Meta"를 지원하는
    // 터미널들이 하는 것이 이것이다. 권위는 `physical_latin`이고, 없으면 논리 키다.
    if input.mods.alt {
        let base = input.physical_latin.or(match input.key {
            TermKey::Char(c) => Some(c),
            _ => None,
        });
        if let Some(c) = base {
            let mut buf = [0u8; 4];
            let bytes = c.encode_utf8(&mut buf).as_bytes().to_vec();
            return Some(alt_prefixed(true, bytes));
        }
        // 둘 다 없으면 아래 `text`로 떨어진다 — 합성 문자라도 보내는 편이
        // 아무것도 안 보내는 것보다 낫다.
    }

    if let Some(text) = input.text.as_deref() {
        if !text.is_empty() {
            return Some(alt_prefixed(input.mods.alt, text.as_bytes().to_vec()));
        }
    }

    if let TermKey::Char(c) = input.key {
        let mut buf = [0u8; 4];
        let bytes = c.encode_utf8(&mut buf).as_bytes().to_vec();
        return Some(alt_prefixed(input.mods.alt, bytes));
    }

    // `TermKey::Unknown`이고 `text`도 없다 — 미디어 키 같은 것. 조용히 다른
    // 키로 오인하느니 아무것도 보내지 않는다.
    None
}

/// 수식자 파라미터. **열거하지 않고 계산한다** — `iced_term`은 이걸 ~100줄로
/// 손으로 열거했다. 수식자가 없으면 `1`이고, 그때는 파라미터를 붙이지 않는다.
fn modifier_param(mods: &Mods) -> u8 {
    1 + u8::from(mods.shift) + u8::from(mods.alt) * 2 + u8::from(mods.ctrl) * 4
}

/// Alt는 메타 프리픽스다 — 만들어진 바이트 **앞에** `ESC`를 붙인다.
/// `Alt+Ctrl+letter`가 제어 바이트 앞에 `ESC`가 붙는 것도 이 경로다.
fn alt_prefixed(alt: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        bytes.insert(0, ESC);
    }
    bytes
}

/// Ctrl 조합의 제어 바이트. 논리 문자를 먼저 보고, 매핑이 없으면
/// `physical_latin`으로 떨어진다 — 비US 레이아웃에서 논리 키가 `[`가 아닌데도
/// `Ctrl+[`를 찾아야 하기 때문이다. **문자를 삽입하려는 것이 아니다.**
fn control_byte(input: &KeyInput) -> Option<u8> {
    if let TermKey::Char(c) = input.key {
        if let Some(byte) = control_byte_for(c) {
            return Some(byte);
        }
    }
    input.physical_latin.and_then(control_byte_for)
}

fn control_byte_for(c: char) -> Option<u8> {
    match c {
        // 0x01..=0x1A. `Ctrl+U`가 `0x15`인 것이 여기서 나온다 —
        // `iced_term`은 이걸 `0x51`('Q')로 손으로 적어 kill-line을 깨뜨렸다.
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        _ => None,
    }
}

/// `APP_KEYPAD`가 켜진 상태의 키패드 키. 수식자가 붙으면 표준 경로로 보낸다.
fn encode_keypad(input: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    if input.location != KeyLocation::Numpad || !mode.contains(TermMode::APP_KEYPAD) {
        return None;
    }
    if modifier_param(&input.mods) != 1 {
        return None;
    }

    let final_byte = match input.key {
        TermKey::Named(NamedKey::Enter) => b'M',
        TermKey::Char(c) => match c {
            '0'..='9' => b'p' + (c as u8 - b'0'),
            '.' => b'n',
            ',' => b'l',
            '+' => b'k',
            '-' => b'm',
            '*' => b'j',
            '/' => b'o',
            '=' => b'X',
            _ => return None,
        },
        _ => return None,
    };
    Some(vec![ESC, b'O', final_byte])
}

fn encode_named(named: NamedKey, input: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    let mods = input.mods;
    let param = modifier_param(&mods);

    let bytes = match named {
        NamedKey::Enter => {
            let body: &[u8] = if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
                b"\r\n"
            } else {
                b"\r"
            };
            alt_prefixed(mods.alt, body.to_vec())
        }
        // Shift+Tab은 백탭이다 — 수식자 파라미터 규칙을 타지 않는 별도 시퀀스.
        NamedKey::Tab if mods.shift => vec![ESC, b'[', b'Z'],
        NamedKey::Tab => alt_prefixed(mods.alt, vec![b'\t']),
        NamedKey::Space if mods.ctrl => alt_prefixed(mods.alt, vec![0x00]),
        NamedKey::Space => alt_prefixed(mods.alt, vec![b' ']),
        // Backspace는 `0x08`이 아니라 `0x7F`다.
        NamedKey::Backspace => alt_prefixed(mods.alt, vec![0x7f]),
        NamedKey::Escape => alt_prefixed(mods.alt, vec![ESC]),

        // 커서 키 — `APP_CURSOR`면 SS3, 아니면 CSI. 수식자가 붙으면 양쪽 다 CSI.
        NamedKey::ArrowUp => cursor_key(b'A', param, mode),
        NamedKey::ArrowDown => cursor_key(b'B', param, mode),
        NamedKey::ArrowRight => cursor_key(b'C', param, mode),
        NamedKey::ArrowLeft => cursor_key(b'D', param, mode),
        NamedKey::Home => cursor_key(b'H', param, mode),
        NamedKey::End => cursor_key(b'F', param, mode),

        NamedKey::Insert => tilde_key(2, param),
        NamedKey::Delete => tilde_key(3, param),
        NamedKey::PageUp => tilde_key(5, param),
        NamedKey::PageDown => tilde_key(6, param),

        // F1-F4는 SS3, F5부터는 틸드 시퀀스. 번호가 16·22를 건너뛴다.
        NamedKey::F1 => ss3_or_csi(b'P', param, true),
        NamedKey::F2 => ss3_or_csi(b'Q', param, true),
        NamedKey::F3 => ss3_or_csi(b'R', param, true),
        NamedKey::F4 => ss3_or_csi(b'S', param, true),
        NamedKey::F5 => tilde_key(15, param),
        NamedKey::F6 => tilde_key(17, param),
        NamedKey::F7 => tilde_key(18, param),
        NamedKey::F8 => tilde_key(19, param),
        NamedKey::F9 => tilde_key(20, param),
        NamedKey::F10 => tilde_key(21, param),
        NamedKey::F11 => tilde_key(23, param),
        NamedKey::F12 => tilde_key(24, param),
    };
    Some(bytes)
}

fn cursor_key(final_byte: u8, param: u8, mode: TermMode) -> Vec<u8> {
    ss3_or_csi(final_byte, param, mode.contains(TermMode::APP_CURSOR))
}

/// `ESC O <f>` / `ESC [ <f>`가 갈리는 키들. **수식자가 붙으면 양쪽 다**
/// `ESC [ 1 ; <m> <f>` 형식이 된다 — `APP_CURSOR`는 무수식자에서만 의미가 있다.
fn ss3_or_csi(final_byte: u8, param: u8, ss3: bool) -> Vec<u8> {
    if param == 1 {
        let intro = if ss3 { b'O' } else { b'[' };
        vec![ESC, intro, final_byte]
    } else {
        let mut out = vec![ESC, b'[', b'1', b';'];
        push_number(&mut out, param as usize);
        out.push(final_byte);
        out
    }
}

/// `ESC [ <n> ~` — 수식자가 있으면 `ESC [ <n> ; <m> ~`.
fn tilde_key(number: u8, param: u8) -> Vec<u8> {
    let mut out = vec![ESC, b'['];
    push_number(&mut out, number as usize);
    if param != 1 {
        out.push(b';');
        push_number(&mut out, param as usize);
    }
    out.push(b'~');
    out
}

fn push_number(out: &mut Vec<u8>, n: usize) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut n = n;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

// ---------------------------------------------------------------------------
// 붙여넣기
// ---------------------------------------------------------------------------

/// 붙여넣기 텍스트를 PTY 바이트로.
///
/// `BRACKETED_PASTE`가 켜져 있으면 `ESC[200~ … ESC[201~`로 감싸고
/// **페이로드에서 종료자를 제거한다.** 제거를 빠뜨리면 `\x1b[201~`가 든 텍스트를
/// 붙여넣었을 때 그 뒤가 괄호 밖으로 나가 **셸이 실행한다** — 이 설계 전체가
/// 막으려는 보안 실패다. `iced_term`은 이 모드 자체를 구현하지 않았다.
///
/// 꺼져 있으면 개행을 `\r`로 정규화한다. 그 모드에서는 애플리케이션이 붙여넣은
/// 데이터와 타이핑을 구분할 수 없고, Enter가 실제로 내는 것은 `\r`이다 —
/// `\n`을 그대로 보내면 여러 줄 붙여넣기가 타이핑과 다르게 동작한다.
/// `alacritty_terminal`이 이걸 해주지 않는 것은 그쪽이 파서이기 때문이고,
/// 정규화는 프런트엔드 몫이다. 여기가 그 프런트엔드다.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    if !mode.contains(TermMode::BRACKETED_PASTE) {
        // `\r\n`을 먼저 접어야 CRLF가 `\r\r`이 되지 않는다.
        return text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
    }
    wrap_bracketed_paste(text)
}

/// 텍스트를 `ESC[200~ … ESC[201~`로 감싸고 **페이로드에서 종료자를 제거한다** —
/// [`encode_paste`]의 bracketed 가지와 같은 로직을 프롬프트 주입(라이브 모드와
/// 무관하게 항상 감싸야 한다)이 재사용하기 위해 갈라낸 것이다.
///
/// 종료자 제거를 빠뜨리면 `\x1b[201~`가 든 텍스트가 괄호 밖으로 새어 **셸이
/// 실행한다** — bracketed paste 설계 전체가 막으려는 보안 실패다.
///
/// **한 번의 `replace`로는 부족하다.** 단일 패스는 쪼갠 입력이 종료자를
/// **재구성**하게 둔다: `"\x1b[2\x1b[201~01~"`에서 가운데 `\x1b[201~`를 한 번만
/// 지우면 남은 조각이 다시 `"\x1b[201~"`로 붙어 살아난다. 그래서 더 이상 종료자가
/// 없을 때까지 **반복 제거**한다(각 패스가 문자열을 줄이므로 유한하다).
pub fn wrap_bracketed_paste(text: &str) -> Vec<u8> {
    let mut sanitized = text.to_string();
    while sanitized.contains("\x1b[201~") {
        sanitized = sanitized.replace("\x1b[201~", "");
    }
    let mut out = Vec::with_capacity(sanitized.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(sanitized.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

// ---------------------------------------------------------------------------
// 포커스
// ---------------------------------------------------------------------------

/// `FOCUS_IN_OUT`이 켜져 있을 때만 바이트가 나간다. 모드는 **락 안에서 진짜
/// 값을 읽어** 넘겨야 한다(캐시 금지).
pub fn encode_focus(focused: bool, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.contains(TermMode::FOCUS_IN_OUT) {
        return None;
    }
    Some(if focused {
        vec![ESC, b'[', b'I']
    } else {
        vec![ESC, b'[', b'O']
    })
}

// ---------------------------------------------------------------------------
// 마우스 — 라우팅
// ---------------------------------------------------------------------------

/// 마우스 intent를 어디로 보낼지 정한다.
///
/// `Err`는 **억제와 다르다** — 상태기계 불변식이 깨졌다는 뜻이므로 호출자가
/// 로그를 남기고 디버그 빌드에서 단언한다. 조용히 버리면 위젯의 held 전이 버그가
/// 정상 억제로 위장된다.
///
/// **휠은 래치에 참여하지 않는다**(플랜 0.4) — 드래그 중이라도 매번 라이브 모드로
/// 독립 판정한다. 이 함수는 순수하므로 래치를 모른다. 래치 유지는 호출자 몫이다.
pub fn route_mouse(intent: &MouseIntent, mode: TermMode) -> Result<MouseRoute, MouseEncodeError> {
    // held 전이 표 검사. Press/Release는 그 버튼이 `held`에 실려 있어야 한다.
    match intent.action {
        MouseAction::Press(button) | MouseAction::Release(button) => {
            if intent.held != Some(button) {
                return Err(MouseEncodeError::HeldMismatch);
            }
        }
        MouseAction::Motion | MouseAction::Wheel { .. } => {}
    }

    if let MouseAction::Wheel { lines } = intent.action {
        if lines == 0 {
            return Ok(MouseRoute::Ignore);
        }
        // Shift 오버라이드는 모드를 이긴다.
        if intent.force_local {
            return Ok(MouseRoute::LocalScroll);
        }
        if mode.intersects(TermMode::MOUSE_MODE) {
            return Ok(MouseRoute::Report);
        }
        // alt 스크린에서는 스크롤백이 없으므로 휠을 화살표 반복으로 바꾼다.
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            return Ok(MouseRoute::AltScreenArrows);
        }
        return Ok(MouseRoute::LocalScroll);
    }

    // **합성 `MOUSE_MODE` 하나로는 못 정한다.** 어떤 모션을 리포트할지가
    // `MOUSE_DRAG`(버튼 눌린 동안만)와 `MOUSE_MOTION`(전부)에서 갈린다.
    if !intent.force_local {
        let reports = match intent.action {
            MouseAction::Press(_) | MouseAction::Release(_) => {
                mode.intersects(TermMode::MOUSE_MODE)
            }
            MouseAction::Motion => {
                mode.contains(TermMode::MOUSE_MOTION)
                    || (intent.held.is_some() && mode.contains(TermMode::MOUSE_DRAG))
            }
            MouseAction::Wheel { .. } => unreachable!("휠은 위에서 처리했다"),
        };
        if reports {
            return Ok(MouseRoute::Report);
        }
    }

    // 로컬 처리. 선택은 좌클릭 제스처에만 붙는다.
    Ok(match intent.action {
        MouseAction::Press(TermMouseButton::Left) => {
            MouseRoute::LocalSelect(selection_type(intent))
        }
        // 드래그 연장·종료. **불변식**: 선택 종류는 press가 정하고 그리드가
        // 래치로 들고 있으므로, 여기 실린 `Simple`은 래치를 무시하는 호출자에게만
        // 보인다. 래치가 권위라는 사실을 코드로 남겨 두는 자리다.
        MouseAction::Motion | MouseAction::Release(TermMouseButton::Left)
            if intent.held == Some(TermMouseButton::Left) =>
        {
            MouseRoute::LocalSelect(SelectionType::Simple)
        }
        _ => MouseRoute::Ignore,
    })
}

fn selection_type(intent: &MouseIntent) -> SelectionType {
    match intent.click {
        ClickKind::Single if intent.mods.ctrl => SelectionType::Block,
        ClickKind::Single => SelectionType::Simple,
        ClickKind::Double => SelectionType::Semantic,
        ClickKind::Triple => SelectionType::Lines,
    }
}

// ---------------------------------------------------------------------------
// 마우스 — 인코딩
// ---------------------------------------------------------------------------

/// 라우트가 확정된 마우스 intent를 PTY 바이트로. 로컬 라우트는 `None`.
///
/// `mode`가 필요한 이유: `MouseRoute::Report`는 어느 와이어 포맷인지를 담지 않고
/// (`SGR_MOUSE`/`UTF8_MOUSE`), `AltScreenArrows`는 `APP_CURSOR`를 봐야 한다.
pub fn encode_mouse(route: &MouseRoute, intent: &MouseIntent, mode: TermMode) -> Option<Vec<u8>> {
    match route {
        MouseRoute::Report => encode_mouse_report(intent, mode),
        MouseRoute::AltScreenArrows => encode_alt_screen_arrows(intent, mode),
        MouseRoute::LocalSelect(_) | MouseRoute::LocalScroll | MouseRoute::Ignore => None,
    }
}

fn encode_alt_screen_arrows(intent: &MouseIntent, mode: TermMode) -> Option<Vec<u8>> {
    let MouseAction::Wheel { lines } = intent.action else {
        return None;
    };
    if lines == 0 {
        return None;
    }
    // 양수 = 위로.
    let final_byte = if lines > 0 { b'A' } else { b'B' };
    let intro = if mode.contains(TermMode::APP_CURSOR) {
        b'O'
    } else {
        b'['
    };
    let count = lines.unsigned_abs() as usize;
    let mut out = Vec::with_capacity(count * 3);
    for _ in 0..count {
        out.extend_from_slice(&[ESC, intro, final_byte]);
    }
    Some(out)
}

fn button_code(button: TermMouseButton) -> u8 {
    match button {
        TermMouseButton::Left => 0,
        TermMouseButton::Middle => 1,
        TermMouseButton::Right => 2,
    }
}

/// Shift 4, Alt 8, Ctrl 16. 키 수식자 파라미터와 **다른 표**다.
fn modifier_bits(mods: &Mods) -> u8 {
    u8::from(mods.shift) * 4 + u8::from(mods.alt) * 8 + u8::from(mods.ctrl) * 16
}

fn encode_mouse_report(intent: &MouseIntent, mode: TermMode) -> Option<Vec<u8>> {
    // 휠은 노치 하나당 리포트 하나다.
    let repeat = match intent.action {
        MouseAction::Wheel { lines } => {
            if lines == 0 {
                return None;
            }
            lines.unsigned_abs() as usize
        }
        _ => 1,
    };

    let (code, release) = match intent.action {
        MouseAction::Press(button) => (button_code(button), false),
        MouseAction::Release(button) => (button_code(button), true),
        // 모션 비트는 +32. 버튼이 안 눌렸으면 3(=버튼 없음).
        MouseAction::Motion => (intent.held.map_or(3, button_code) + 32, false),
        MouseAction::Wheel { lines } => (if lines > 0 { 64 } else { 65 }, false),
    };
    let code = code + modifier_bits(&intent.mods);

    // **1-based 뷰포트 좌표다.** `iced_term`은 버퍼 좌표를 보내 스크롤백에서
    // 음수 줄을 내보낸다.
    let col = intent.hit.col + 1;
    let row = intent.hit.row + 1;

    let one = if mode.contains(TermMode::SGR_MOUSE) {
        encode_sgr(code, col, row, release)
    } else {
        // 레거시는 0-based 좌표를 받아 안에서 `33 + 좌표`로 만든다 — 상한 판정이
        // 좌표 공간에서 이뤄지므로 그쪽이 경계를 읽기 쉽다.
        encode_legacy(
            code,
            intent.hit.col,
            intent.hit.row,
            release,
            mode.contains(TermMode::UTF8_MOUSE),
        )?
    };

    if repeat == 1 {
        return Some(one);
    }
    let mut out = Vec::with_capacity(one.len() * repeat);
    for _ in 0..repeat {
        out.extend_from_slice(&one);
    }
    Some(out)
}

/// `ESC [ < <b> ; <x> ; <y> M|m`. **좌표 상한이 없다** — 가능하면 이쪽이다.
fn encode_sgr(code: u8, col: usize, row: usize, release: bool) -> Vec<u8> {
    let mut out = vec![ESC, b'[', b'<'];
    push_number(&mut out, code as usize);
    out.push(b';');
    push_number(&mut out, col);
    out.push(b';');
    push_number(&mut out, row);
    out.push(if release { b'm' } else { b'M' });
    out
}

/// `ESC [ M <32+b> <전송값 x> <전송값 y>`. `col`/`row`는 **0-based 좌표**다.
/// 상한을 넘으면 `None` — **오류가 아니라 억제다.** 좌표를 표현할 방법이 없으니
/// 아무것도 보내지 않는 것이 맞다.
fn encode_legacy(code: u8, col: usize, row: usize, release: bool, utf8: bool) -> Option<Vec<u8>> {
    // X10은 릴리스에서 버튼을 구분하지 못한다 — 항상 3이다. SGR만 구분한다.
    let code = if release {
        3 + (code & 0b1111_1100)
    } else {
        code
    };
    let mut out = vec![ESC, b'[', b'M', 32 + code];
    push_legacy_pos(&mut out, col, utf8)?;
    push_legacy_pos(&mut out, row, utf8)?;
    Some(out)
}

/// 0-based 좌표 하나를 레거시 바이트로 민다.
///
/// **전송 값은 `33 + 좌표`다** — `32` 오프셋에 1-based 변환이 더해진 값이다.
/// 상한은 좌표 공간에서 **배타적**이다: 단일 바이트는 `< 223`(전송 값 255까지),
/// UTF-8은 `< 2015`(전송 값 2047 = U+07FF까지).
fn push_legacy_pos(out: &mut Vec<u8>, coord: usize, utf8: bool) -> Option<()> {
    if utf8 {
        if coord >= UTF8_MAX_COORD {
            return None;
        }
        // 경계가 **좌표 공간**에 있다는 것이 요점이다. 좌표 95는 전송 값 128이
        // 되므로 여기서 갈리는 2바이트 시퀀스는 `0xC2 0x80`, 즉 정확한
        // UTF-8이다 — overlong이 나오지 않는다.
        if coord >= UTF8_TWO_BYTE_COORD_THRESHOLD {
            let pos = LEGACY_WIRE_OFFSET + coord;
            out.push(0xC0 + (pos / 64) as u8);
            out.push(0x80 + (pos & 63) as u8);
            return Some(());
        }
    } else if coord >= LEGACY_MAX_COORD {
        return None;
    }
    out.push((LEGACY_WIRE_OFFSET + coord) as u8);
    Some(())
}

// ---------------------------------------------------------------------------
// 테스트
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::Side;

    use crate::input_types::ViewportHit;

    // --- 생성 헬퍼 -------------------------------------------------------

    fn key(k: TermKey) -> KeyInput {
        KeyInput {
            key: k,
            physical_latin: None,
            location: KeyLocation::Standard,
            mods: Mods::default(),
            text: None,
            repeat: false,
        }
    }

    fn named(n: NamedKey) -> KeyInput {
        key(TermKey::Named(n))
    }

    fn mods(shift: bool, ctrl: bool, alt: bool) -> Mods {
        Mods {
            shift,
            ctrl,
            alt,
            logo: false,
        }
    }

    /// 문자 키. iced가 실제로 그러듯 `text`도 같이 채운다 — `text`가 없는
    /// 케이스는 그 케이스를 노리는 테스트에서 따로 만든다.
    fn ch(c: char) -> KeyInput {
        KeyInput {
            text: Some(c.to_string()),
            ..key(TermKey::Char(c))
        }
    }

    fn intent(action: MouseAction, row: usize, col: usize) -> MouseIntent {
        MouseIntent {
            action,
            hit: ViewportHit {
                row,
                col,
                side: Side::Left,
            },
            held: None,
            mods: Mods::default(),
            click: ClickKind::Single,
            force_local: false,
        }
    }

    fn show(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|b| match b {
                0x1b => "<ESC>".to_string(),
                0x20..=0x7e => (*b as char).to_string(),
                _ => format!("<{b:02x}>"),
            })
            .collect()
    }

    fn assert_key(input: &KeyInput, mode: TermMode, expect: Option<&[u8]>) {
        let got = encode_key(input, mode);
        assert_eq!(
            got.as_deref(),
            expect,
            "\n  input : {input:?}\n  mode  : {mode:?}\n  got   : {:?}\n  expect: {:?}",
            got.as_deref().map(show),
            expect.map(show),
        );
    }

    // --- 화살표 / APP_CURSOR ---------------------------------------------

    #[test]
    fn arrows_switch_on_app_cursor() {
        let table: &[(NamedKey, &[u8], &[u8])] = &[
            (NamedKey::ArrowUp, b"\x1b[A", b"\x1bOA"),
            (NamedKey::ArrowDown, b"\x1b[B", b"\x1bOB"),
            (NamedKey::ArrowRight, b"\x1b[C", b"\x1bOC"),
            (NamedKey::ArrowLeft, b"\x1b[D", b"\x1bOD"),
            (NamedKey::Home, b"\x1b[H", b"\x1bOH"),
            (NamedKey::End, b"\x1b[F", b"\x1bOF"),
        ];
        for (k, normal, app) in table {
            assert_key(&named(*k), TermMode::NONE, Some(normal));
            assert_key(&named(*k), TermMode::APP_CURSOR, Some(app));
        }
    }

    /// 수식자가 붙으면 **APP_CURSOR와 무관하게** CSI 형식이다.
    #[test]
    fn modified_arrows_are_csi_in_both_cursor_modes() {
        let mut k = named(NamedKey::ArrowUp);
        k.mods = mods(false, true, false);
        assert_key(&k, TermMode::NONE, Some(b"\x1b[1;5A"));
        assert_key(&k, TermMode::APP_CURSOR, Some(b"\x1b[1;5A"));
    }

    // --- 수식자 파라미터 산술 (8조합 전수) -------------------------------

    #[test]
    fn modifier_parameter_is_arithmetic_over_all_eight_combinations() {
        // 1 + shift*1 + alt*2 + ctrl*4
        let table: &[(bool, bool, bool, &[u8])] = &[
            (false, false, false, b"\x1b[A"),
            (true, false, false, b"\x1b[1;2A"),
            (false, false, true, b"\x1b[1;3A"),
            (true, false, true, b"\x1b[1;4A"),
            (false, true, false, b"\x1b[1;5A"),
            (true, true, false, b"\x1b[1;6A"),
            (false, true, true, b"\x1b[1;7A"),
            (true, true, true, b"\x1b[1;8A"),
        ];
        for (shift, ctrl, alt, expect) in table {
            let mut k = named(NamedKey::ArrowUp);
            k.mods = mods(*shift, *ctrl, *alt);
            assert_key(&k, TermMode::NONE, Some(expect));
        }
    }

    /// **수식자를 정확 일치로 비교하지 않는다.** `iced_term`은 바인딩 테이블을
    /// 정확 일치로 조회해(`bindings.rs:121`) 잉여 수식자 하나에 키가 조용히
    /// 사라진다. `Ctrl+Shift+U`가 그 함정이다 — Shift가 붙어도 제어 바이트가
    /// 나와야 한다.
    #[test]
    fn extra_modifier_does_not_drop_the_key() {
        let mut k = ch('u');
        k.mods = mods(true, true, false);
        assert_key(&k, TermMode::NONE, Some(&[0x15]));

        // 명명 키도 마찬가지다 — 잉여 Shift가 파라미터에 반영될 뿐 사라지지 않는다.
        let mut arrow = named(NamedKey::ArrowUp);
        arrow.mods = mods(true, true, false);
        assert_key(&arrow, TermMode::NONE, Some(b"\x1b[1;6A"));
    }

    /// **Cmd/Super는 터미널 입력이 아니다.** 위젯이 Copy/Paste가 아니라고
    /// 판정한 Cmd 조합이 여기까지 오면 맨 글자가 셸로 새어 나간다.
    #[test]
    fn logo_modifier_is_suppressed() {
        let logo = Mods {
            shift: false,
            ctrl: false,
            alt: false,
            logo: true,
        };

        // Cmd+W가 "w"를 입력하면 안 된다.
        let mut w = ch('w');
        w.mods = logo;
        assert_key(&w, TermMode::NONE, None);

        // 명명 키도, 키패드도 막힌다.
        let mut arrow = named(NamedKey::ArrowUp);
        arrow.mods = logo;
        assert_key(&arrow, TermMode::NONE, None);

        let mut pad = ch('7');
        pad.location = KeyLocation::Numpad;
        pad.mods = logo;
        assert_key(&pad, TermMode::APP_KEYPAD, None);

        // 대조군: logo가 없으면 셋 다 정상 입력된다.
        assert_key(&ch('w'), TermMode::NONE, Some(b"w"));
        assert_key(&named(NamedKey::ArrowUp), TermMode::NONE, Some(b"\x1b[A"));
    }

    // --- Ctrl 제어 바이트 -------------------------------------------------

    #[test]
    fn ctrl_letter_boundaries() {
        let table: &[(char, u8)] = &[('a', 0x01), ('z', 0x1a), ('A', 0x01), ('Z', 0x1a)];
        for (c, byte) in table {
            let mut k = ch(*c);
            k.mods = mods(false, true, false);
            assert_key(&k, TermMode::NONE, Some(&[*byte]));
        }
    }

    /// **회귀**: `Ctrl+U`는 `\x15`다. `iced_term`은 `\x51`('Q')로 손으로 적어
    /// kill-line을 깨뜨렸다(`bindings.rs:231,299`).
    #[test]
    fn ctrl_u_is_0x15_not_0x51() {
        let mut k = ch('u');
        k.mods = mods(false, true, false);
        assert_key(&k, TermMode::NONE, Some(&[0x15]));
    }

    #[test]
    fn ctrl_punctuation_and_space() {
        let table: &[(char, u8)] = &[
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
        ];
        for (c, byte) in table {
            let mut k = ch(*c);
            k.mods = mods(false, true, false);
            assert_key(&k, TermMode::NONE, Some(&[*byte]));
        }
        let mut space = named(NamedKey::Space);
        space.mods = mods(false, true, false);
        assert_key(&space, TermMode::NONE, Some(&[0x00]));
    }

    /// 비US 레이아웃: 논리 키는 `ㅂ`인데 물리 키가 `[`다. `physical_latin`이
    /// **제어 조회 전용**으로 쓰인다 — 문자 삽입에는 쓰이지 않는다.
    #[test]
    fn control_lookup_falls_back_to_physical_latin() {
        let mut k = ch('ㅂ');
        k.physical_latin = Some('[');
        k.mods = mods(false, true, false);
        assert_key(&k, TermMode::NONE, Some(&[0x1b]));

        // Ctrl이 없으면 physical_latin은 무시되고 text가 나간다.
        let mut plain = ch('ㅂ');
        plain.physical_latin = Some('[');
        assert_key(&plain, TermMode::NONE, Some("ㅂ".as_bytes()));
    }

    /// `Alt+Ctrl+letter`: 먼저 Ctrl로 제어 바이트를 만들고 그 앞에 ESC.
    #[test]
    fn alt_ctrl_letter_prefixes_esc_to_the_control_byte() {
        let mut k = ch('c');
        k.mods = mods(false, true, true);
        assert_key(&k, TermMode::NONE, Some(&[0x1b, 0x03]));
    }

    /// 제어 매핑이 없는 Ctrl 조합은 사라지지 않고 `text`로 흘러간다.
    #[test]
    fn ctrl_without_control_mapping_falls_through_to_text() {
        let mut k = ch('1');
        k.mods = mods(false, true, false);
        assert_key(&k, TermMode::NONE, Some(b"1"));
    }

    // --- Enter / LINE_FEED_NEW_LINE --------------------------------------

    #[test]
    fn enter_branches_on_line_feed_new_line() {
        assert_key(&named(NamedKey::Enter), TermMode::NONE, Some(b"\r"));
        assert_key(
            &named(NamedKey::Enter),
            TermMode::LINE_FEED_NEW_LINE,
            Some(b"\r\n"),
        );
    }

    // --- Tab / Backspace / Escape ----------------------------------------

    #[test]
    fn tab_backspace_escape() {
        assert_key(&named(NamedKey::Tab), TermMode::NONE, Some(b"\t"));

        let mut shift_tab = named(NamedKey::Tab);
        shift_tab.mods = mods(true, false, false);
        assert_key(&shift_tab, TermMode::NONE, Some(b"\x1b[Z"));

        assert_key(&named(NamedKey::Backspace), TermMode::NONE, Some(&[0x7f]));

        let mut alt_bs = named(NamedKey::Backspace);
        alt_bs.mods = mods(false, false, true);
        assert_key(&alt_bs, TermMode::NONE, Some(&[0x1b, 0x7f]));

        assert_key(&named(NamedKey::Escape), TermMode::NONE, Some(&[0x1b]));
    }

    // --- 틸드 키 / 펑션 키 ------------------------------------------------

    #[test]
    fn tilde_and_function_keys() {
        let table: &[(NamedKey, &[u8])] = &[
            (NamedKey::Insert, b"\x1b[2~"),
            (NamedKey::Delete, b"\x1b[3~"),
            (NamedKey::PageUp, b"\x1b[5~"),
            (NamedKey::PageDown, b"\x1b[6~"),
            (NamedKey::F1, b"\x1bOP"),
            (NamedKey::F2, b"\x1bOQ"),
            (NamedKey::F3, b"\x1bOR"),
            (NamedKey::F4, b"\x1bOS"),
            (NamedKey::F5, b"\x1b[15~"),
            (NamedKey::F6, b"\x1b[17~"),
            (NamedKey::F7, b"\x1b[18~"),
            (NamedKey::F8, b"\x1b[19~"),
            (NamedKey::F9, b"\x1b[20~"),
            (NamedKey::F10, b"\x1b[21~"),
            (NamedKey::F11, b"\x1b[23~"),
            (NamedKey::F12, b"\x1b[24~"),
        ];
        for (k, expect) in table {
            assert_key(&named(*k), TermMode::NONE, Some(expect));
        }
    }

    #[test]
    fn modified_tilde_and_function_keys() {
        let mut del = named(NamedKey::Delete);
        del.mods = mods(true, false, false);
        assert_key(&del, TermMode::NONE, Some(b"\x1b[3;2~"));

        let mut f1 = named(NamedKey::F1);
        f1.mods = mods(false, true, false);
        assert_key(&f1, TermMode::NONE, Some(b"\x1b[1;5P"));
    }

    // --- APP_KEYPAD -------------------------------------------------------

    #[test]
    fn app_keypad_only_applies_to_numpad_location() {
        let mut k = ch('7');
        k.location = KeyLocation::Numpad;

        // 대조군 셋: 모드 꺼짐 / 위치가 Standard / 수식자가 붙음.
        assert_key(&k, TermMode::NONE, Some(b"7"));
        assert_key(&ch('7'), TermMode::APP_KEYPAD, Some(b"7"));

        assert_key(&k, TermMode::APP_KEYPAD, Some(b"\x1bOw"));

        let mut ctrl = k.clone();
        ctrl.mods = mods(false, true, false);
        assert_key(&ctrl, TermMode::APP_KEYPAD, Some(b"7"));
    }

    #[test]
    fn app_keypad_table() {
        let table: &[(char, &[u8])] = &[
            ('0', b"\x1bOp"),
            ('9', b"\x1bOy"),
            ('.', b"\x1bOn"),
            ('+', b"\x1bOk"),
            ('-', b"\x1bOm"),
            ('*', b"\x1bOj"),
            ('/', b"\x1bOo"),
        ];
        for (c, expect) in table {
            let mut k = ch(*c);
            k.location = KeyLocation::Numpad;
            assert_key(&k, TermMode::APP_KEYPAD, Some(expect));
        }

        let mut enter = named(NamedKey::Enter);
        enter.location = KeyLocation::Numpad;
        assert_key(&enter, TermMode::APP_KEYPAD, Some(b"\x1bOM"));
        // 대조군: 모드가 꺼져 있으면 평범한 Enter다.
        assert_key(&enter, TermMode::NONE, Some(b"\r"));
    }

    // --- Unknown / text ---------------------------------------------------

    #[test]
    fn unknown_key_without_text_is_none() {
        assert_key(&key(TermKey::Unknown), TermMode::NONE, None);
    }

    /// 다중 스칼라(조합 문자)는 `Unknown`이지만 `text`가 보존돼 정상 입력된다.
    #[test]
    fn unknown_key_with_text_still_types() {
        let mut k = key(TermKey::Unknown);
        k.text = Some("é".to_string());
        assert_key(&k, TermMode::NONE, Some("é".as_bytes()));
    }

    /// **IME 조합이 확정한 여러 음절 문자열이 통째로 나간다.** 한글 IME의
    /// `Commit`은 한 글자가 아니라 완성된 문자열("안녕")을 준다 — 위젯이 그걸
    /// `text`가 채워진 `Unknown` 키로 실어 이 경로를 탄다. 단일 문자만 검증하면
    /// "첫 글자만 보낸다" 같은 버그를 놓친다(각 음절은 UTF-8 3바이트라 첫
    /// 글자만 보내면 6바이트가 아니라 3바이트가 나간다).
    #[test]
    fn unknown_key_with_multi_syllable_text_types_the_whole_string() {
        let mut k = key(TermKey::Unknown);
        k.text = Some("안녕".to_string());
        assert_key(&k, TermMode::NONE, Some("안녕".as_bytes()));
        // 회귀 방어를 명시적으로: 6바이트 전부여야 한다(음절당 3).
        assert_eq!("안녕".as_bytes().len(), 6);
    }

    /// `text`가 비어 있으면 논리 문자로 떨어진다.
    #[test]
    fn empty_text_falls_back_to_logical_char() {
        let mut k = key(TermKey::Char('x'));
        k.text = Some(String::new());
        assert_key(&k, TermMode::NONE, Some(b"x"));
    }

    #[test]
    fn alt_char_gets_esc_prefix() {
        let mut k = ch('b');
        k.mods = mods(false, false, true);
        assert_key(&k, TermMode::NONE, Some(b"\x1bb"));
    }

    /// **macOS의 Option+a는 meta-a이지 meta-å가 아니다.** iced는 합성된 `å`를
    /// `text`에 실어 보내지만 사용자가 뜻한 것은 수식되지 않은 `a`다.
    /// "Option as Meta"를 지원하는 터미널들이 하는 것이 이것이다.
    #[test]
    fn alt_prefixes_the_unmodified_latin_char_not_the_composed_text() {
        let mut k = KeyInput {
            text: Some("å".to_string()),
            physical_latin: Some('a'),
            ..key(TermKey::Char('å'))
        };
        k.mods = mods(false, false, true);
        assert_key(&k, TermMode::NONE, Some(b"\x1ba"));

        // 대조군: Alt가 없으면 합성 문자가 그대로 입력돼야 한다 —
        // `physical_latin`은 **제어·메타 조회 전용**이지 문자 삽입용이 아니다.
        let mut plain = k.clone();
        plain.mods = Mods::default();
        assert_key(&plain, TermMode::NONE, Some("å".as_bytes()));
    }

    /// `physical_latin`이 없으면 논리 키의 문자로 떨어진다(리눅스의 Alt+b 등).
    #[test]
    fn alt_falls_back_to_the_logical_char_without_physical_latin() {
        let mut k = ch('b');
        k.physical_latin = None;
        k.mods = mods(false, false, true);
        assert_key(&k, TermMode::NONE, Some(b"\x1bb"));

        // 둘 다 없으면 합성 문자라도 보낸다 — 아무것도 안 보내는 것보다 낫다.
        let mut unknown = key(TermKey::Unknown);
        unknown.text = Some("ß".to_string());
        unknown.mods = mods(false, false, true);
        assert_key(&unknown, TermMode::NONE, Some("\u{1b}ß".as_bytes()));
    }

    // --- 붙여넣기 ---------------------------------------------------------

    /// 브래킷 모드가 꺼져 있으면 개행이 `\r`로 정규화된다 — Enter가 실제로 내는
    /// 것이 `\r`이므로, `\n`을 그대로 보내면 붙여넣기가 타이핑과 다르게 동작한다.
    #[test]
    fn unbracketed_paste_normalizes_newlines_to_cr() {
        assert_eq!(encode_paste("ls -al\n", TermMode::NONE), b"ls -al\r");
        assert_eq!(encode_paste("a\nb\nc", TermMode::NONE), b"a\rb\rc");
        // CRLF는 `\r\r`이 아니라 `\r` 하나가 돼야 한다.
        assert_eq!(encode_paste("a\r\nb", TermMode::NONE), b"a\rb");
        // 이미 `\r`인 것은 그대로.
        assert_eq!(encode_paste("a\rb", TermMode::NONE), b"a\rb");
        // 개행이 없으면 손대지 않는다.
        assert_eq!(encode_paste("ls -al", TermMode::NONE), b"ls -al");
    }

    /// 대조군: 브래킷 모드에서는 정규화하지 않는다 — 애플리케이션이 붙여넣기임을
    /// 알고 있으므로 원문이 권위다.
    #[test]
    fn bracketed_paste_does_not_normalize_newlines() {
        assert_eq!(
            encode_paste("a\nb", TermMode::BRACKETED_PASTE),
            b"\x1b[200~a\nb\x1b[201~"
        );
    }

    #[test]
    fn paste_is_wrapped_with_bracketed_mode() {
        assert_eq!(
            encode_paste("ls -al", TermMode::BRACKETED_PASTE),
            b"\x1b[200~ls -al\x1b[201~"
        );
    }

    /// 프롬프트 주입 경로(`wrap_bracketed_paste`)는 **모드 인자 없이** 항상
    /// 감싸고 종료자를 제거한다 — 게이트가 이미 BRACKETED_PASTE를 확인한 뒤에만
    /// 부르기 때문이다. `encode_paste`의 bracketed 가지와 byte-identical해야 한다.
    #[test]
    fn wrap_bracketed_paste_always_wraps_and_strips_terminator() {
        assert_eq!(wrap_bracketed_paste("fix the bug"), b"\x1b[200~fix the bug\x1b[201~");
        // 주입 텍스트에 든 종료자를 제거하지 않으면 그 뒤가 괄호 밖으로 새어
        // 셸이 실행한다 — 프롬프트도 신뢰할 수 없는 입력이다.
        assert_eq!(
            wrap_bracketed_paste("a\x1b[201~rm -rf /"),
            b"\x1b[200~arm -rf /\x1b[201~"
        );
        // 라이브 모드가 켜진 encode_paste와 정확히 같은 바이트여야 한다.
        assert_eq!(
            wrap_bracketed_paste("hello"),
            encode_paste("hello", TermMode::BRACKETED_PASTE)
        );
    }

    /// **HIGH 보안 회귀**: 단일 패스 strip은 쪼갠 종료자가 **재구성**되게 둔다.
    /// `"\x1b[2\x1b[201~01~"`에서 가운데 종료자를 한 번만 지우면 남은 조각이 다시
    /// `"\x1b[201~"`로 붙어 살아난다 — 그러면 괄호가 조기에 닫히고 이후 바이트가
    /// 라이브 키입력이 되어 셸이 실행한다. **페이로드(여는 `200~`와 마지막 닫는
    /// `201~` 사이)에는 어떤 종료자도 남아선 안 된다.** 이 경로는 모든 터미널
    /// 페이스트(`send_paste`→`encode_paste`)에도 도달하므로 원시 클립보드에도 적용된다.
    #[test]
    fn split_terminator_cannot_be_reconstructed_by_a_single_pass() {
        let out = wrap_bracketed_paste("\x1b[2\x1b[201~01~");
        // 마지막 6바이트가 우리가 붙인 닫는 종료자다. 그 앞(페이로드)에는 종료자가
        // 하나도 없어야 한다.
        assert!(
            out.ends_with(b"\x1b[201~"),
            "must still end with our closing terminator: {}",
            show(&out)
        );
        let payload = &out[b"\x1b[200~".len()..out.len() - b"\x1b[201~".len()];
        let reconstructed = payload
            .windows(6)
            .filter(|w| *w == b"\x1b[201~".as_slice())
            .count();
        assert_eq!(
            reconstructed, 0,
            "a reconstructed terminator survived in the payload — the bracket closes early \
             and the rest executes as keystrokes: {}",
            show(&out)
        );
        // 결과적으로 전체에 종료자는 **딱 하나**(우리가 붙인 것)뿐이다.
        let total = out
            .windows(6)
            .filter(|w| *w == b"\x1b[201~".as_slice())
            .count();
        assert_eq!(total, 1, "exactly one terminator (ours) must remain: {}", show(&out));
    }

    #[test]
    fn multiline_paste_keeps_newlines_inside_the_brackets() {
        assert_eq!(
            encode_paste("a\nb\n", TermMode::BRACKETED_PASTE),
            b"\x1b[200~a\nb\n\x1b[201~"
        );
    }

    /// **보안 회귀**: 페이로드에 든 종료자를 제거하지 않으면 그 뒤가 괄호 밖으로
    /// 나가 셸이 실행한다.
    #[test]
    fn paste_strips_injected_terminator() {
        let attack = "safe\x1b[201~rm -rf /\n";
        let out = encode_paste(attack, TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~saferm -rf /\n\x1b[201~");

        // 종료자가 정확히 한 번만 등장한다 — 괄호가 조기에 닫히지 않았다.
        let count = out
            .windows(6)
            .filter(|w| *w == b"\x1b[201~".as_slice())
            .count();
        assert_eq!(count, 1, "종료자가 {count}번 나왔다: {}", show(&out));
    }

    #[test]
    fn paste_strips_every_injected_terminator() {
        let out = encode_paste("a\x1b[201~b\x1b[201~c", TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~abc\x1b[201~");
    }

    /// 대조군: 시작자(`200~`)는 페이로드 안에 있어도 무해하므로 건드리지 않는다.
    #[test]
    fn paste_does_not_strip_the_introducer() {
        let out = encode_paste("a\x1b[200~b", TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~a\x1b[200~b\x1b[201~");
    }

    // --- 포커스 -----------------------------------------------------------

    #[test]
    fn focus_reports_only_when_mode_is_set() {
        assert_eq!(encode_focus(true, TermMode::NONE), None);
        assert_eq!(encode_focus(false, TermMode::NONE), None);
        assert_eq!(
            encode_focus(true, TermMode::FOCUS_IN_OUT).as_deref(),
            Some(b"\x1b[I".as_slice())
        );
        assert_eq!(
            encode_focus(false, TermMode::FOCUS_IN_OUT).as_deref(),
            Some(b"\x1b[O".as_slice())
        );
    }

    // --- 라우팅 -----------------------------------------------------------

    #[test]
    fn press_release_require_matching_held() {
        let mut i = intent(MouseAction::Press(TermMouseButton::Left), 0, 0);
        i.held = Some(TermMouseButton::Left);
        assert!(route_mouse(&i, TermMode::NONE).is_ok());

        i.held = Some(TermMouseButton::Right);
        assert_eq!(
            route_mouse(&i, TermMode::NONE),
            Err(MouseEncodeError::HeldMismatch)
        );

        i.held = None;
        assert_eq!(
            route_mouse(&i, TermMode::NONE),
            Err(MouseEncodeError::HeldMismatch)
        );

        // Release도 놓인 버튼이 실려 있어야 한다.
        let mut r = intent(MouseAction::Release(TermMouseButton::Left), 0, 0);
        r.held = None;
        assert_eq!(
            route_mouse(&r, TermMode::NONE),
            Err(MouseEncodeError::HeldMismatch)
        );
        r.held = Some(TermMouseButton::Left);
        assert!(route_mouse(&r, TermMode::NONE).is_ok());
    }

    /// **합성 `MOUSE_MODE` 하나로는 어떤 모션을 리포트할지 못 정한다.**
    /// `MOUSE_DRAG`는 버튼이 눌린 동안만, `MOUSE_MOTION`은 항상.
    #[test]
    fn motion_routing_distinguishes_drag_from_motion() {
        let free = intent(MouseAction::Motion, 3, 4);
        let mut dragging = free;
        dragging.held = Some(TermMouseButton::Left);

        // MOUSE_REPORT_CLICK만: 모션은 어느 쪽도 리포트하지 않는다.
        let m = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(route_mouse(&free, m), Ok(MouseRoute::Ignore));
        assert_eq!(
            route_mouse(&dragging, m),
            Ok(MouseRoute::LocalSelect(SelectionType::Simple))
        );

        // MOUSE_DRAG: 눌린 동안만.
        let m = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG;
        assert_eq!(route_mouse(&free, m), Ok(MouseRoute::Ignore));
        assert_eq!(route_mouse(&dragging, m), Ok(MouseRoute::Report));

        // MOUSE_MOTION: 버튼과 무관하게 전부.
        let m = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION;
        assert_eq!(route_mouse(&free, m), Ok(MouseRoute::Report));
        assert_eq!(route_mouse(&dragging, m), Ok(MouseRoute::Report));
    }

    #[test]
    fn press_reports_under_any_mouse_mode_flag() {
        let mut i = intent(MouseAction::Press(TermMouseButton::Left), 0, 0);
        i.held = Some(TermMouseButton::Left);

        for m in [
            TermMode::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_DRAG,
            TermMode::MOUSE_MOTION,
        ] {
            assert_eq!(route_mouse(&i, m), Ok(MouseRoute::Report), "{m:?}");
        }
        // 대조군: 아무 플래그도 없으면 로컬 선택이다.
        assert_eq!(
            route_mouse(&i, TermMode::NONE),
            Ok(MouseRoute::LocalSelect(SelectionType::Simple))
        );
    }

    /// Shift 오버라이드는 모드를 이긴다 — 앱이 마우스 모드를 쥐고 있어도
    /// 사용자가 선택할 수 있어야 한다.
    #[test]
    fn shift_override_forces_local_in_every_mouse_mode() {
        let mut press = intent(MouseAction::Press(TermMouseButton::Left), 0, 0);
        press.held = Some(TermMouseButton::Left);
        press.force_local = true;
        assert_eq!(
            route_mouse(&press, TermMode::MOUSE_MODE),
            Ok(MouseRoute::LocalSelect(SelectionType::Simple))
        );

        let mut wheel = intent(MouseAction::Wheel { lines: 3 }, 0, 0);
        wheel.force_local = true;
        assert_eq!(
            route_mouse(&wheel, TermMode::MOUSE_MODE),
            Ok(MouseRoute::LocalScroll)
        );
        assert_eq!(
            route_mouse(&wheel, TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL),
            Ok(MouseRoute::LocalScroll)
        );
    }

    #[test]
    fn wheel_routing_table() {
        let w = intent(MouseAction::Wheel { lines: 1 }, 0, 0);

        assert_eq!(route_mouse(&w, TermMode::NONE), Ok(MouseRoute::LocalScroll));
        assert_eq!(
            route_mouse(&w, TermMode::MOUSE_REPORT_CLICK),
            Ok(MouseRoute::Report)
        );
        assert_eq!(
            route_mouse(&w, TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL),
            Ok(MouseRoute::AltScreenArrows)
        );
        // 리포트가 alt-screen 화살표를 이긴다.
        assert_eq!(
            route_mouse(
                &w,
                TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL | TermMode::MOUSE_REPORT_CLICK
            ),
            Ok(MouseRoute::Report)
        );
        // ALT_SCREEN만으로는 부족하다 — ALTERNATE_SCROLL이 같이 켜져야 한다.
        assert_eq!(
            route_mouse(&w, TermMode::ALT_SCREEN),
            Ok(MouseRoute::LocalScroll)
        );
        // 0줄은 아무것도 아니다.
        let zero = intent(MouseAction::Wheel { lines: 0 }, 0, 0);
        assert_eq!(route_mouse(&zero, TermMode::NONE), Ok(MouseRoute::Ignore));
    }

    /// **휠은 래치에 참여하지 않는다** — 드래그 중(`held`가 살아 있어도)에도
    /// 매번 라이브 모드로 독립 판정한다.
    #[test]
    fn wheel_ignores_held_button() {
        let mut w = intent(MouseAction::Wheel { lines: -2 }, 0, 0);
        w.held = Some(TermMouseButton::Left);
        assert_eq!(
            route_mouse(&w, TermMode::MOUSE_REPORT_CLICK),
            Ok(MouseRoute::Report)
        );
        assert_eq!(route_mouse(&w, TermMode::NONE), Ok(MouseRoute::LocalScroll));
    }

    #[test]
    fn click_kind_selects_selection_type() {
        let table: &[(ClickKind, bool, SelectionType)] = &[
            (ClickKind::Single, false, SelectionType::Simple),
            (ClickKind::Single, true, SelectionType::Block),
            (ClickKind::Double, false, SelectionType::Semantic),
            (ClickKind::Triple, false, SelectionType::Lines),
        ];
        for (click, ctrl, expect) in table {
            let mut i = intent(MouseAction::Press(TermMouseButton::Left), 0, 0);
            i.held = Some(TermMouseButton::Left);
            i.click = *click;
            i.mods = mods(false, *ctrl, false);
            assert_eq!(
                route_mouse(&i, TermMode::NONE),
                Ok(MouseRoute::LocalSelect(*expect)),
                "{click:?} ctrl={ctrl}"
            );
        }
    }

    /// 로컬 모드에서 좌클릭이 아닌 버튼은 선택을 만들지 않는다.
    #[test]
    fn non_left_press_is_ignored_locally() {
        for b in [TermMouseButton::Middle, TermMouseButton::Right] {
            let mut i = intent(MouseAction::Press(b), 0, 0);
            i.held = Some(b);
            assert_eq!(route_mouse(&i, TermMode::NONE), Ok(MouseRoute::Ignore));
        }
    }

    // --- 마우스 인코딩 ----------------------------------------------------

    fn report(i: &MouseIntent, mode: TermMode) -> Option<Vec<u8>> {
        encode_mouse(&MouseRoute::Report, i, mode)
    }

    #[test]
    fn local_routes_produce_no_bytes() {
        let i = intent(MouseAction::Motion, 0, 0);
        for r in [
            MouseRoute::LocalSelect(SelectionType::Simple),
            MouseRoute::LocalScroll,
            MouseRoute::Ignore,
        ] {
            assert_eq!(encode_mouse(&r, &i, TermMode::SGR_MOUSE), None, "{r:?}");
        }
    }

    /// **좌표는 1-based 뷰포트 좌표다.** `iced_term`은 버퍼 좌표를 보내
    /// 스크롤백에서 음수 줄을 내보낸다.
    #[test]
    fn sgr_coordinates_are_one_based() {
        let mut i = intent(MouseAction::Press(TermMouseButton::Left), 0, 0);
        i.held = Some(TermMouseButton::Left);
        assert_eq!(
            report(&i, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<0;1;1M".as_slice())
        );

        let mut j = intent(MouseAction::Press(TermMouseButton::Left), 23, 79);
        j.held = Some(TermMouseButton::Left);
        assert_eq!(
            report(&j, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<0;80;24M".as_slice())
        );
    }

    #[test]
    fn sgr_button_and_release_final_byte() {
        let table: &[(TermMouseButton, &[u8], &[u8])] = &[
            (TermMouseButton::Left, b"\x1b[<0;1;1M", b"\x1b[<0;1;1m"),
            (TermMouseButton::Middle, b"\x1b[<1;1;1M", b"\x1b[<1;1;1m"),
            (TermMouseButton::Right, b"\x1b[<2;1;1M", b"\x1b[<2;1;1m"),
        ];
        for (b, press, release) in table {
            let mut p = intent(MouseAction::Press(*b), 0, 0);
            p.held = Some(*b);
            assert_eq!(report(&p, TermMode::SGR_MOUSE).as_deref(), Some(*press));

            let mut r = intent(MouseAction::Release(*b), 0, 0);
            r.held = Some(*b);
            assert_eq!(report(&r, TermMode::SGR_MOUSE).as_deref(), Some(*release));
        }
    }

    /// Shift 4 / Alt 8 / Ctrl 16 — 키 수식자 파라미터와 다른 표다.
    #[test]
    fn mouse_modifier_bits_over_all_eight_combinations() {
        let table: &[(bool, bool, bool, &[u8])] = &[
            (false, false, false, b"\x1b[<0;1;1M"),
            (true, false, false, b"\x1b[<4;1;1M"),
            (false, false, true, b"\x1b[<8;1;1M"),
            (true, false, true, b"\x1b[<12;1;1M"),
            (false, true, false, b"\x1b[<16;1;1M"),
            (true, true, false, b"\x1b[<20;1;1M"),
            (false, true, true, b"\x1b[<24;1;1M"),
            (true, true, true, b"\x1b[<28;1;1M"),
        ];
        for (shift, ctrl, alt, expect) in table {
            let mut i = intent(MouseAction::Press(TermMouseButton::Left), 0, 0);
            i.held = Some(TermMouseButton::Left);
            i.mods = mods(*shift, *ctrl, *alt);
            assert_eq!(
                report(&i, TermMode::SGR_MOUSE).as_deref(),
                Some(*expect),
                "shift={shift} ctrl={ctrl} alt={alt}"
            );
        }
    }

    #[test]
    fn motion_sets_the_motion_bit_and_uses_held_button() {
        let free = intent(MouseAction::Motion, 0, 0);
        // 버튼 없음 = 3, +32 → 35.
        assert_eq!(
            report(&free, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<35;1;1M".as_slice())
        );

        let mut held = free;
        held.held = Some(TermMouseButton::Right);
        assert_eq!(
            report(&held, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<34;1;1M".as_slice())
        );
    }

    #[test]
    fn wheel_reports_one_per_notch() {
        let up = intent(MouseAction::Wheel { lines: 1 }, 0, 0);
        assert_eq!(
            report(&up, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<64;1;1M".as_slice())
        );

        let down = intent(MouseAction::Wheel { lines: -1 }, 0, 0);
        assert_eq!(
            report(&down, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<65;1;1M".as_slice())
        );

        let three = intent(MouseAction::Wheel { lines: 3 }, 0, 0);
        assert_eq!(
            report(&three, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<64;1;1M\x1b[<64;1;1M\x1b[<64;1;1M".as_slice())
        );

        let zero = intent(MouseAction::Wheel { lines: 0 }, 0, 0);
        assert_eq!(report(&zero, TermMode::SGR_MOUSE), None);
    }

    #[test]
    fn alt_screen_arrows_follow_app_cursor_and_repeat() {
        let up = intent(MouseAction::Wheel { lines: 2 }, 0, 0);
        assert_eq!(
            encode_mouse(&MouseRoute::AltScreenArrows, &up, TermMode::NONE).as_deref(),
            Some(b"\x1b[A\x1b[A".as_slice())
        );
        assert_eq!(
            encode_mouse(&MouseRoute::AltScreenArrows, &up, TermMode::APP_CURSOR).as_deref(),
            Some(b"\x1bOA\x1bOA".as_slice())
        );

        let down = intent(MouseAction::Wheel { lines: -1 }, 0, 0);
        assert_eq!(
            encode_mouse(&MouseRoute::AltScreenArrows, &down, TermMode::NONE).as_deref(),
            Some(b"\x1b[B".as_slice())
        );

        // 휠이 아닌 액션에는 화살표가 없다.
        let motion = intent(MouseAction::Motion, 0, 0);
        assert_eq!(
            encode_mouse(&MouseRoute::AltScreenArrows, &motion, TermMode::NONE),
            None
        );
    }

    // --- 레거시 / UTF8 오버플로 -------------------------------------------

    #[test]
    fn legacy_x10_bytes() {
        let mut i = intent(MouseAction::Press(TermMouseButton::Left), 0, 0);
        i.held = Some(TermMouseButton::Left);
        // ESC [ M <32+0> <32+1> <32+1>
        assert_eq!(
            report(&i, TermMode::NONE).as_deref(),
            Some(&[0x1b, b'[', b'M', 32, 33, 33][..])
        );
    }

    /// X10은 릴리스에서 버튼을 구분하지 못한다 — 항상 3이다. 수식자 비트는 남는다.
    #[test]
    fn legacy_release_collapses_button_to_three_but_keeps_modifiers() {
        let mut r = intent(MouseAction::Release(TermMouseButton::Right), 0, 0);
        r.held = Some(TermMouseButton::Right);
        assert_eq!(
            report(&r, TermMode::NONE).as_deref(),
            Some(&[0x1b, b'[', b'M', 32 + 3, 33, 33][..])
        );

        r.mods = mods(false, true, false); // ctrl = 16
        assert_eq!(
            report(&r, TermMode::NONE).as_deref(),
            Some(&[0x1b, b'[', b'M', 32 + 19, 33, 33][..])
        );

        // 대조군: SGR은 릴리스에서도 버튼을 유지하고 종결자로 구분한다.
        assert_eq!(
            report(&r, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<18;1;1m".as_slice())
        );
    }

    /// X10 좌표 상한은 223(전송 값 255)이다. 넘으면 **오류가 아니라 억제**다.
    #[test]
    fn legacy_x10_overflow_boundary() {
        // 좌표 222 → 전송 값 33 + 222 = 255. 마지막으로 가능한 값.
        let ok = intent(MouseAction::Motion, 0, 222);
        assert_eq!(
            report(&ok, TermMode::NONE).as_deref(),
            Some(&[0x1b, b'[', b'M', 32 + 35, 255, 33][..])
        );

        // 한 칸 더 가면 아무것도 보내지 않는다.
        let over = intent(MouseAction::Motion, 0, 223);
        assert_eq!(report(&over, TermMode::NONE), None);

        // 행도 같은 규칙을 탄다.
        let row_over = intent(MouseAction::Motion, 223, 0);
        assert_eq!(report(&row_over, TermMode::NONE), None);
    }

    /// `UTF8_MOUSE`는 2바이트로 넓힌다. **경계는 좌표 공간의 95**이고, 그 좌표의
    /// 전송 값이 정확히 128이라 2바이트 UTF-8이 시작되는 지점과 맞아떨어진다 —
    /// overlong 시퀀스가 나오지 않는다. 상한은 좌표 2015(배타적).
    #[test]
    fn utf8_mouse_two_byte_boundary() {
        let mode = TermMode::UTF8_MOUSE;

        // 좌표 94 → 전송 값 127 → 아직 1바이트.
        let below = intent(MouseAction::Motion, 0, 94);
        assert_eq!(
            report(&below, mode).as_deref(),
            Some(&[0x1b, b'[', b'M', 32 + 35, 127, 33][..])
        );

        // 좌표 95 → 전송 값 128 → `0xC2 0x80`. 정확한 UTF-8이다.
        let at = intent(MouseAction::Motion, 0, 95);
        assert_eq!(
            report(&at, mode).as_deref(),
            Some(&[0x1b, b'[', b'M', 32 + 35, 0xC2, 0x80, 33][..])
        );

        // 마지막으로 가능한 좌표 2014 → 전송 값 2047 → 0xDF 0xBF (U+07FF).
        let max = intent(MouseAction::Motion, 0, 2014);
        assert_eq!(
            report(&max, mode).as_deref(),
            Some(&[0x1b, b'[', b'M', 32 + 35, 0xDF, 0xBF, 33][..])
        );

        // 상한은 배타적이다 — 2015부터 억제.
        let over = intent(MouseAction::Motion, 0, 2015);
        assert_eq!(report(&over, mode), None);

        // 행도 같은 규칙을 탄다.
        assert_eq!(report(&intent(MouseAction::Motion, 2015, 0), mode), None);

        // 대조군: 같은 좌표라도 SGR에는 상한이 없다.
        assert_eq!(
            report(&over, TermMode::SGR_MOUSE).as_deref(),
            Some(b"\x1b[<35;2016;1M".as_slice())
        );
    }

    /// 2바이트 시퀀스가 전부 **정상 UTF-8**인지 확인한다. 경계를 전송 값 쪽에
    /// 두면 좌표 62..=94 구간이 overlong(`0xC1 ..`)이 되는데, 그 리딩 바이트는
    /// UTF-8에 존재할 수 없다.
    #[test]
    fn utf8_mouse_never_emits_overlong_sequences() {
        for coord in 0..300usize {
            let bytes = match report(&intent(MouseAction::Motion, 0, coord), TermMode::UTF8_MOUSE) {
                Some(b) => b,
                None => continue,
            };
            // 헤더 4바이트(ESC [ M <button>) 뒤부터가 좌표다.
            let tail = &bytes[4..];
            assert!(
                tail[0] != 0xC0 && tail[0] != 0xC1,
                "좌표 {coord}이 overlong 리딩 바이트를 냈다: {}",
                show(&bytes)
            );
            // 실제로 디코딩되는지까지 확인한다.
            let coord_bytes = &tail[..tail.len() - 1];
            assert!(
                std::str::from_utf8(coord_bytes).is_ok(),
                "좌표 {coord}이 UTF-8로 디코딩되지 않는다: {}",
                show(&bytes)
            );
        }
    }

    /// UTF8_MOUSE가 없으면 같은 좌표가 1바이트 규칙을 탄다 — 두 모드가 실제로
    /// 갈리는지 확인한다.
    #[test]
    fn utf8_mouse_flag_actually_switches_encoding() {
        let i = intent(MouseAction::Motion, 0, 99);
        let legacy = report(&i, TermMode::NONE).expect("좌표 100은 X10 범위 안이다");
        let utf8 = report(&i, TermMode::UTF8_MOUSE).expect("UTF8도 표현할 수 있다");
        assert_ne!(
            legacy,
            utf8,
            "두 모드가 같은 바이트를 냈다: {}",
            show(&utf8)
        );
        assert_eq!(legacy.len(), 6);
        assert_eq!(utf8.len(), 7);
    }
}
