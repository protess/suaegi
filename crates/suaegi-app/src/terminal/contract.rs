//! 위젯 → 앱 계약. **위젯은 `TerminalSession`을 절대 만지지 않는다** —
//! 스냅샷을 읽어 그리고, 입력을 `TermCommand`로 번역해 발행할 뿐이다.
//! 세션을 만지는 것은 앱의 `update`뿐이고, 이 경계가 이 플랜의 테스트
//! 가능성의 근거다.

use alacritty_terminal::grid::Scroll;
use suaegi_term::input_types::{CopyTargets, KeyInput, MouseIntent};

/// 위젯이 발행하는 커맨드. 위젯은 자기 `SessionId`를 들고 있으므로 실제로
/// 나가는 것은 `(SessionId, TermCommand)`다 — 커맨드 자체에 대상이 없다.
///
/// **없는 것들과 그 이유:**
/// - `Select*` 변형이 없다. 선택은 마우스 처리의 **결과**이지 별도 커맨드가
///   아니다 — 라우팅·좌표 변환·선택 변경을 한 락 안에서 끝내야 하는데,
///   앱이 다시 `Select*`를 부르면 두 번째 락에서 `display_offset`이 이미
///   달라져 있을 수 있다.
/// - 선택 epoch가 없다. epoch는 세션이 락 안에서 할당하므로 위젯은 알 수 없다.
/// - `InputDropped`가 없다. 유실은 앱이 커맨드를 **실행한 뒤** 아는 것이라
///   위젯이 발행할 수 없다 — 앱 레벨 메시지/상태 전이다.
///
/// **`PartialEq`가 없다**: `alacritty_terminal::grid::Scroll`이 `Debug, Copy,
/// Clone`만 파생한다(`grid/mod.rs:72`). 헤드리스 테스트가 발행 커맨드를
/// 비교할 때는 `matches!`와 필드 분해를 쓴다. Plan은 이 지점을 다루지 않는다
/// — Task 0 보고서의 미해결 항목 참고.
#[derive(Debug, Clone)]
pub enum TermCommand {
    Key(KeyInput),
    /// 클립보드 **원문 그대로**. bracketed paste 감싸기는 라이브 모드가
    /// 필요하므로 `suaegi-term`의 `encode_paste`가 락 안에서 한다.
    Paste(String),
    /// 라우팅 판단은 세션(그리드)이 한다 — 위젯은 원시 사실만 실어 보낸다.
    Mouse(MouseIntent),
    /// `seq`는 합치기용이다. 리사이즈는 블로킹이라 워커로 가고, 세션당
    /// **최신 `seq`만** 실행한다.
    Resize {
        rows: u16,
        cols: u16,
        seq: u64,
    },
    /// 위젯이 로컬 스크롤로 확정한 경우만. 마우스 리포팅 대상인 휠은
    /// `Mouse`로 나간다.
    Scroll(Scroll),
    CopySelection {
        to: CopyTargets,
    },
}
