//! 터미널 입력 프로토콜 타입. **iced를 알지 않는다** — 의존 방향이
//! `suaegi-app → suaegi-term` 단방향이라, 인코더가 받는 타입이 여기 있어야
//! 순환이 생기지 않는다. iced 이벤트를 이 타입들로 옮기는 번역기는 앱에 있다.
//!
//! 여기 있는 것은 전부 **값**이다. 락도, 세션도, 그리드도 만지지 않는다.
//! 인코딩과 라우팅 판단(모드가 필요한 모든 것)은 `TerminalGrid`가 term 락을
//! 쥔 채 하고, 이 모듈은 그 입력과 출력의 모양만 정한다.

use alacritty_terminal::index::Side;
use alacritty_terminal::selection::SelectionType;

// ---------------------------------------------------------------------------
// 키
// ---------------------------------------------------------------------------

/// 논리 키. iced의 논리 키는 **문자열**이라 스칼라가 0개이거나 2개 이상일 수
/// 있다 — 그때는 `Unknown`으로 두되 `KeyInput::text`는 보존한다. 조합 문자는
/// 인코딩 우선순위 3번(`text`)으로 흘러가 정상 입력된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermKey {
    /// 유니코드 스칼라가 **정확히 하나**일 때만.
    Char(char),
    Named(NamedKey),
    /// 매핑 없음 — 인코더가 `None`을 돌려준다. 미디어 키 같은 것을 조용히
    /// 다른 키로 오인하지 않기 위해 존재한다.
    Unknown,
}

/// 이 목록이 전부다. 여기 없는 명명 키는 `TermKey::Unknown`이 된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Tab,
    Space,
    Backspace,
    Escape,
    Delete,
    Insert,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

/// `APP_KEYPAD` 분기에 필요하다 — 키패드 키는 같은 논리 키라도 다른 시퀀스를 낸다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLocation {
    Standard,
    Numpad,
    Left,
    Right,
}

/// 수식자. **정확 일치로 비교하지 않는다** — CapsLock 하나가 껴서 키가 조용히
/// 무시되는 것이 `iced_term`의 실제 버그다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

/// **`modified_key`를 나르지 않는다.** `text`가 이미 수식자·IME·데드키 적용
/// 결과를 담고, 제어 조회는 `physical_latin`이 담당한다. 셋 다 나르면 어느
/// 것이 권위인지가 흐려진다 — `iced_term`이 캐시된 수식자와 이벤트 수식자를
/// 섞어 쓰다 만든 것과 같은 종류의 실수다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    pub key: TermKey,
    /// iced의 `key.to_latin(physical_key)` 결과. **제어 조회 전용** —
    /// 비US 레이아웃에서 `Ctrl+[`를 찾기 위한 것이지 문자를 삽입하기 위한
    /// 것이 아니다.
    pub physical_latin: Option<char>,
    pub location: KeyLocation,
    pub mods: Mods,
    /// iced의 `SmolStr`을 소유 `String`으로.
    pub text: Option<String>,
    pub repeat: bool,
}

// ---------------------------------------------------------------------------
// 마우스
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermMouseButton {
    Left,
    Middle,
    Right,
}

/// 히트테스트 결과. **뷰포트 좌표**다(그리드 좌표가 아니다) — 스크롤백 보정은
/// 락 안에서 `display_offset`을 읽어야 옳으므로 그리드가 한다. 위젯이
/// 스냅샷의 offset으로 미리 보정하면 그 사이 스크롤이 일어나 레이스가 된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportHit {
    pub row: usize,
    pub col: usize,
    /// 셀의 어느 쪽에 찍혔는가. 선택 경계 판정에 alacritty가 요구한다.
    pub side: Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickKind {
    Single,
    Double,
    Triple,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(TermMouseButton),
    Release(TermMouseButton),
    Motion,
    /// 부호 있음. 양수 = 위로. 위젯이 픽셀 델타를 누산해 만든 **정수 줄**이다 —
    /// 그것이 로컬 스크롤인지 alt-screen 화살표인지 리포트인지는 그리드가 정한다.
    Wheel {
        lines: i32,
    },
}

/// 위젯이 아는 **원시 사실**만 담는다. 라우팅 판단(선택이냐 리포트냐)은 이
/// 값을 받은 그리드가 락 안에서 한다 — 위젯은 라우팅 결과를 볼 수 없다
/// (`MouseResult`는 위젯의 `update`가 끝난 뒤 앱에 돌아간다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseIntent {
    pub action: MouseAction,
    pub hit: ViewportHit,
    /// 현재 눌려 있는 버튼. 전이 규칙은 Press 전/Release 후로 정해져 있다.
    pub held: Option<TermMouseButton>,
    pub mods: Mods,
    pub click: ClickKind,
    /// Shift 오버라이드. **모드와 무관하므로 위젯이 판단한다** — 앱이 마우스
    /// 모드를 쥐고 있어도 Shift를 누르면 사용자가 선택할 수 있어야 한다.
    pub force_local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseRoute {
    /// 마우스 리포팅 시퀀스를 PTY로 보낸다.
    Report,
    LocalSelect(SelectionType),
    LocalScroll,
    /// `ALT_SCREEN` + `ALTERNATE_SCROLL`: 휠이 스크롤 대신 화살표 반복이 된다.
    AltScreenArrows,
    Ignore,
}

/// press에서 래치하고 release에서 해제한다. 드래그 도중 모드가 바뀌어도 한
/// 제스처가 반으로 갈리지 않게 하는 장치다. **휠은 여기 참여하지 않는다** —
/// 드래그 중 휠을 굴리는 TUI가 래치 때문에 리포트를 못 받으면 안 되므로
/// 휠은 매번 라이브 모드로 독립 판정한다.
///
/// 그리드 내부 상태이므로 크레이트 밖으로 내보내지 않는다.
// Task 1이 `GridState`에 넣어 실제로 구성하기 전까지는 소비자가 없다.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PointerLatch {
    pub(crate) button: TermMouseButton,
    pub(crate) route: MouseRoute,
}

/// 상태기계 불변식 위반. **억제(`Ok`인데 보낼 바이트가 없음)와 다르게
/// 취급한다** — 로그를 남기고 디버그 빌드에서 단언한다. 조용히 버리면
/// 상태기계 버그가 정상 억제로 위장된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MouseEncodeError {
    /// intent의 `held`가 액션과 모순된다(예: `Press(Left)`인데 `held`가
    /// `Some(Right)`). 위젯의 held 전이 표가 깨졌다는 뜻이다.
    #[error("mouse intent held state contradicts its action")]
    HeldMismatch,
}

// ---------------------------------------------------------------------------
// 복사 / 쓰기 결과
// ---------------------------------------------------------------------------

/// 어느 클립보드에 쓸 것인가. **`suaegi-term`이 소유한다** — `CopyRequest`가
/// 이걸 담고 `CopyRequest`는 term 타입이므로, app에 두면 역방향 의존이 생긴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyTargets {
    pub standard: bool,
    pub primary: bool,
}

impl CopyTargets {
    /// 명시적 복사(단축키)는 양쪽에.
    pub const EXPLICIT: Self = Self {
        standard: true,
        primary: true,
    };
    /// 드래그 완료는 primary에만 — X11/Wayland의 중클릭 붙여넣기 관례다.
    pub const DRAG_COMPLETE: Self = Self {
        standard: false,
        primary: true,
    };
}

/// 선택 추출 요청. **epoch는 그리드가 락 안에서 실어준다** — 위젯도 앱도
/// 할당하지 않는다. 워커가 `extract_selection(epoch)`을 부르면 그리드가 락을
/// 잡은 뒤 비교해, 불일치면 아무것도 하지 않고 `None`을 돌려준다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyRequest {
    pub epoch: u64,
    pub to: CopyTargets,
}

/// `bool`이면 "모드상 보낼 것이 없음"과 "큐가 차서 유실"이 같은 `false`로
/// 뭉개져 유실 피드백 규칙을 어긴다. 셋을 구분한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Queued,
    /// 큐(상한 256)에 못 넣었다 = **입력 유실**. 앱이 피드백을 내야 한다.
    Dropped,
    /// 모드상 보낼 바이트가 없다. 유실이 아니므로 피드백을 내지 않는다.
    Suppressed,
}

// ---------------------------------------------------------------------------
// 마우스 처리 결과 — grid → session → app
// ---------------------------------------------------------------------------

/// 그리드 → 세션 (크레이트 내부). **바이트는 아직 큐에 들어가지 않았다** —
/// 큐잉은 세션이 한다. 큐를 그리드에 넣으면 락 중첩이 생긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridMouseResult {
    pub bytes: Option<Vec<u8>>,
    pub redraw: bool,
    pub copy: Option<CopyRequest>,
}

/// 세션 → 앱 (공개). **`bytes`를 그대로 노출하지 않는다** — 공개 타입이
/// 바이트를 들고 있으면 큐에 실제로 들어갔는지를 앱이 알 수 없고, 그러면
/// 마우스 입력 유실이 "보이는 피드백" 규칙을 빠져나간다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseResult {
    pub write: WriteOutcome,
    pub redraw: bool,
    pub copy: Option<CopyRequest>,
}
