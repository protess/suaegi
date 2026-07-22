//! OSC 터미널-타이틀 상태 감지기. **비-Claude 에이전트의 유일한 상태 신호다** —
//! Claude만 훅 서버로 정밀 신호를 얻고(`contract::HookState`), 나머지는 pane이
//! 내보내는 터미널 타이틀에서 상태를 추론한다.
//!
//! Orca의 단일 함수 `detectAgentStatusFromTitle`
//! (`src/shared/agent-title-status.ts:137-203`)을 규칙 그대로 이식했다. **이 함수는
//! 자신이 어떤 에이전트인지 모른다** — 전부 타이틀 텍스트에서 추론한다. 그래서 앱
//! 층이 pane별로 "이 세션이 OSC 상태를 쓰는가"를 [`StatusSource`]로 가르고, 이
//! 함수는 순수 문자열→상태 매핑만 한다.
//!
//! [`StatusSource`]: suaegi_term::agent::StatusSource
//!
//! ## 닫힌 이름 집합 게이팅 (Orca S1의 핵심)
//!
//! idle/permission/working **키워드** 판정은 타이틀이 16개 에이전트 이름 중 하나를
//! **온전한 토큰으로** 담을 때만 발동한다(`AGENT_NAMES` 13종 + droid/hermes/agy 3종).
//! 그 밖의 에이전트(goose·crush·kiro 등)는 타이틀에 braille 스피너가 뜨면 working은
//! 잡아도(스피너는 이름과 무관한 범용 규칙) **idle은 타이틀만으로 절대 못 낸다** —
//! 이름 게이트에 걸려 키워드 검사 이전에 `None`을 돌려주기 때문이다. 이것은 버그가
//! 아니라 **의도된 동작**이고, 테스트가 그대로 고정한다.
//!
//! Gemini 글리프(✦◇✋⏲), Claude의 ✳ 프리픽스, braille 스피너, Pi/OMP는 이름
//! 게이트보다 **먼저** 걸리는 범용 신호다.

use suaegi_term::agent::StatusSource;
use suaegi_term::grid::TitleChange;

use super::contract::HookState;

/// Orca `AgentStatus`. 이 crate의 배지 통화는 [`HookState`]지만, 픽스처가 Orca의
/// working/permission/idle 어휘로 적혀 있어 내부 판정은 이 enum으로 하고 마지막에
/// [`HookState`]로 매핑한다 — 그래야 테스트가 Orca 기대값과 1:1로 읽힌다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitleStatus {
    Working,
    Permission,
    Idle,
}

// Orca `agent-title-core.ts`의 글리프 상수.
const CLAUDE_IDLE: char = '\u{2733}'; // ✳
const GEMINI_WORKING: char = '\u{2726}'; // ✦
const GEMINI_SILENT_WORKING: char = '\u{23f2}'; // ⏲
const GEMINI_IDLE: char = '\u{25c7}'; // ◇
const GEMINI_PERMISSION: char = '\u{270b}'; // ✋

/// OSC 타이틀 감지에만 쓰는 **닫힌** 이름 집합(Orca `agent-name-token-match.ts:16-30`).
/// launchable 에이전트 전체보다 의도적으로 좁다 — "amp" 같은 짧은 이름은 "timestamp
/// ready" 같은 평범한 셸 타이틀을 에이전트 활동으로 오인시킨다.
const AGENT_NAMES: &[&str] = &[
    "claude",
    "openclaude",
    "codex",
    "copilot",
    "cursor",
    "gemini",
    "antigravity",
    "opencode",
    "mimo",
    "openclaw",
    "aider",
    "grok",
    "devin",
];

/// 이름 뒤에 붙을 수 있는 Windows 런처 확장자(Orca `WINDOWS_EXECUTABLE_SUFFIX_RE`).
const WINDOWS_EXE_SUFFIXES: &[&str] = &["exe", "cmd", "bat", "ps1"];

/// 이름 게이트에서 substring이 아니라 **토큰**으로 매칭하는 근거(Orca 주석):
/// 경로/cwd 타이틀 "opencode-blinker"(⊃ opencode)나 "openclaude"(⊃ claude)가
/// 엉뚱한 에이전트로 칠해지는 것을 막는다. 양쪽 경계에서 이 문자들을 거부한다.
fn is_left_reject(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '\\' || c == '-'
}

/// 이름 토큰의 오른쪽 경계 거부 집합(Orca 이름 정규식 `(?![\w./\\-])`).
fn is_name_right_reject(c: char) -> bool {
    is_left_reject(c)
}

/// 강한 키워드의 오른쪽 경계 거부 집합(Orca `(?![\w\-])`). **이름과 달리
/// `.`·`/`·`\`는 허용한다** — "Codex done."·"Aider thinking..."이 idle/working으로
/// 잡히도록.
fn is_keyword_right_reject(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_braille(c: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&c)
}

fn contains_braille(s: &str) -> bool {
    s.chars().any(is_braille)
}

/// `needle`(ASCII, 소문자)가 `chars`(소문자화된 타이틀) 안에 **온전한 토큰**으로
/// 있는가. 왼쪽 경계는 항상 [`is_left_reject`], 오른쪽 경계는 `right_reject`.
/// `allow_exe_suffix`면 토큰 뒤 `.exe`/`.cmd`/`.bat`/`.ps1`를 소비한 뒤 경계를 본다.
fn contains_token(
    chars: &[char],
    needle: &[char],
    allow_exe_suffix: bool,
    right_reject: fn(char) -> bool,
) -> bool {
    let n = needle.len();
    if n == 0 || n > chars.len() {
        return false;
    }
    let mut i = 0;
    while i + n <= chars.len() {
        if chars[i..i + n] == *needle {
            let left_ok = i == 0 || !is_left_reject(chars[i - 1]);
            if left_ok {
                let mut end = i + n;
                if allow_exe_suffix && end < chars.len() && chars[end] == '.' {
                    for suf in WINDOWS_EXE_SUFFIXES {
                        let s: Vec<char> = suf.chars().collect();
                        if end + 1 + s.len() <= chars.len()
                            && chars[end + 1..end + 1 + s.len()] == s[..]
                        {
                            end += 1 + s.len();
                            break;
                        }
                    }
                }
                let right_ok = end >= chars.len() || !right_reject(chars[end]);
                if right_ok {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn lower_chars(title: &str) -> Vec<char> {
    title.chars().map(|c| c.to_ascii_lowercase()).collect()
}

/// 타이틀이 16개 이름 집합 중 하나를 온전한 토큰으로 담는가(이름 게이트).
fn contains_any_agent_name(chars: &[char]) -> bool {
    // AGENT_NAMES는 exe 접미사 허용, droid/hermes/agy는 미허용(Orca 정규식 그대로).
    for name in AGENT_NAMES {
        let needle: Vec<char> = name.chars().collect();
        if contains_token(chars, &needle, true, is_name_right_reject) {
            return true;
        }
    }
    false
}

fn contains_droid(chars: &[char]) -> bool {
    contains_token(
        chars,
        &['d', 'r', 'o', 'i', 'd'],
        false,
        is_name_right_reject,
    )
}

fn contains_hermes(chars: &[char]) -> bool {
    contains_token(
        chars,
        &['h', 'e', 'r', 'm', 'e', 's'],
        false,
        is_name_right_reject,
    )
}

fn contains_agy(chars: &[char]) -> bool {
    contains_token(chars, &['a', 'g', 'y'], false, is_name_right_reject)
}

/// Orca `CLAUDE_MANAGEMENT_TITLE_RE`: `<경로?>claude<.exe?> agents` (전체 일치,
/// 대소문자 무시, 따옴표 감쌈 허용). Claude 자체의 에이전트 관리 화면 타이틀이라
/// 상태 신호가 아니다.
fn is_claude_management_title(title: &str) -> bool {
    let t = title.trim();
    let lower: String = t.chars().map(|c| c.to_ascii_lowercase()).collect();
    let Some(head) = lower.strip_suffix("agents") else {
        return false;
    };
    // "agents" 앞에는 최소 하나의 공백(`\s+`)이 있어야 한다.
    if head.is_empty() || !head.ends_with(char::is_whitespace) {
        return false;
    }
    let mut cmd = head.trim_end();
    // 따옴표로 감싼 커맨드 허용: `"…claude.cmd" agents`.
    if cmd.len() >= 2
        && ((cmd.starts_with('"') && cmd.ends_with('"'))
            || (cmd.starts_with('\'') && cmd.ends_with('\'')))
    {
        cmd = &cmd[1..cmd.len() - 1];
    }
    // `(?:.*[\\/])?` — 마지막 경로 구분자 뒤만 남긴다.
    let base = match cmd.rfind(['/', '\\']) {
        Some(i) => &cmd[i + 1..],
        None => cmd,
    };
    if base == "claude" {
        return true;
    }
    if let Some(rest) = base.strip_prefix("claude.") {
        return WINDOWS_EXE_SUFFIXES.contains(&rest);
    }
    false
}

/// Orca `getPiCompatibleSyntheticAgentStatus`(`pi-compatible-synthetic-title.ts`).
/// 합성 Pi/OMP 라벨: `<braille?> (pi|omp) (<- action required>|<ready|idle|done>)?`.
/// 매칭되면 braille→working, "action required"→permission, 그 외→idle.
fn pi_synthetic_status(title: &str) -> Option<TitleStatus> {
    let t = title.trim();
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    let has_braille = if !chars.is_empty() && is_braille(chars[0]) {
        i = 1;
        // braille 뒤에는 `\s+`가 와야 한다.
        if i >= chars.len() || !chars[i].is_whitespace() {
            return None;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        true
    } else {
        false
    };

    let rest: String = chars[i..].iter().collect::<String>().to_ascii_lowercase();
    let label_len = if rest.starts_with("omp") {
        3
    } else if rest.starts_with("pi") {
        2
    } else {
        return None;
    };

    let after = &rest[label_len..];
    let after_body = after.trim_end(); // 트레일링 `\s*`.
    if after_body.is_empty() {
        // 맨 라벨.
        return Some(if has_braille {
            TitleStatus::Working
        } else {
            TitleStatus::Idle
        });
    }
    // 접미사가 있으면 그 앞에 `\s+`가 있어야 한다.
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    let body = after_body.trim_start();
    let status = if body == "ready" || body == "idle" || body == "done" {
        if has_braille {
            TitleStatus::Working
        } else {
            TitleStatus::Idle
        }
    } else if let Some(x) = body.strip_prefix('-') {
        // `\s+-\s+action required`.
        if x.trim_start() == "action required" {
            if has_braille {
                TitleStatus::Working
            } else {
                TitleStatus::Permission
            }
        } else {
            return None;
        }
    } else {
        return None;
    };
    Some(status)
}

/// Orca `LEGACY_PI_COMPATIBLE_TITLE_RE`: `<braille?> π (\s*[-:] | \s) .*`. 실제 π
/// 글리프(U+03C0)를 쓰는 레거시 Pi 셸 타이틀.
fn is_legacy_pi_compatible(title: &str) -> bool {
    let t = title.trim_start(); // `^\s*`.
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    if !chars.is_empty() && is_braille(chars[0]) {
        i = 1;
        if i >= chars.len() || !chars[i].is_whitespace() {
            return false;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
    }
    if i >= chars.len() || chars[i] != 'π' {
        return false;
    }
    i += 1;
    // `(?:\s*[-:] | \s)` — 첫 대안: 공백* 뒤 `-`/`:`; 둘째 대안: 공백 하나.
    let mut j = i;
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    if j < chars.len() && (chars[j] == '-' || chars[j] == ':') {
        return true;
    }
    i < chars.len() && chars[i].is_whitespace()
}

/// **공개 진입점.** 타이틀에서 배지용 상태를 추론한다. `None` = 상태 신호 없음
/// (배지를 건드리지 않는다). working→`Working`, permission→`Waiting`, idle→`Done`.
pub fn detect_status_from_title(title: &str) -> Option<HookState> {
    detect_title_status(title).map(|s| match s {
        TitleStatus::Working => HookState::Working,
        TitleStatus::Permission => HookState::Waiting,
        TitleStatus::Idle => HookState::Done,
    })
}

/// Orca `detectAgentStatusFromTitle`의 규칙을 그 순서 그대로 옮긴 내부 판정.
fn detect_title_status(title: &str) -> Option<TitleStatus> {
    if title.is_empty() || is_claude_management_title(title) {
        return None;
    }
    if title.trim().eq_ignore_ascii_case("cursor agent") {
        return None;
    }

    // Gemini 글리프 — 이름 게이트보다 앞서는 범용 신호.
    if title.contains(GEMINI_PERMISSION) {
        return Some(TitleStatus::Permission);
    }
    if title.contains(GEMINI_WORKING) || title.contains(GEMINI_SILENT_WORKING) {
        return Some(TitleStatus::Working);
    }
    if title.contains(GEMINI_IDLE) {
        return Some(TitleStatus::Idle);
    }

    // 합성 Pi/OMP 라벨.
    if let Some(status) = pi_synthetic_status(title) {
        return Some(status);
    }

    // Claude의 ✳ 프리픽스.
    let claude_idle_prefix: String = format!("{CLAUDE_IDLE} ");
    if title.starts_with(&claude_idle_prefix) || title.chars().eq(std::iter::once(CLAUDE_IDLE)) {
        return Some(TitleStatus::Idle);
    }

    // 레거시 π 타이틀(스피너 없을 때만 idle).
    if is_legacy_pi_compatible(title) && !contains_braille(title) {
        return Some(TitleStatus::Idle);
    }

    // Braille 스피너 — 이름과 무관한 범용 working 규칙.
    if contains_braille(title) {
        return Some(TitleStatus::Working);
    }

    // ---- 이름 게이트: 여기부터는 16개 이름 중 하나가 타이틀에 있어야 한다. ----
    let chars = lower_chars(title);
    let has_legacy_name = contains_any_agent_name(&chars);
    let has_droid = contains_droid(&chars);
    let has_hermes = contains_hermes(&chars);
    let has_agy = contains_agy(&chars);
    if !has_legacy_name && !has_droid && !has_hermes && !has_agy {
        return None;
    }

    // permission 키워드(substring, 경계 없음 — Orca `containsAny`).
    let lower_title = title.to_ascii_lowercase();
    if lower_title.contains("action required")
        || lower_title.contains("permission")
        || lower_title.contains("waiting")
    {
        return Some(TitleStatus::Permission);
    }
    // 강한 idle/working 키워드(경계 인식).
    if contains_token(
        &chars,
        &['r', 'e', 'a', 'd', 'y'],
        false,
        is_keyword_right_reject,
    ) || contains_token(
        &chars,
        &['i', 'd', 'l', 'e'],
        false,
        is_keyword_right_reject,
    ) || contains_token(
        &chars,
        &['d', 'o', 'n', 'e'],
        false,
        is_keyword_right_reject,
    ) {
        return Some(TitleStatus::Idle);
    }
    if contains_token(
        &chars,
        &['w', 'o', 'r', 'k', 'i', 'n', 'g'],
        false,
        is_keyword_right_reject,
    ) || contains_token(
        &chars,
        &['t', 'h', 'i', 'n', 'k', 'i', 'n', 'g'],
        false,
        is_keyword_right_reject,
    ) || contains_token(
        &chars,
        &['r', 'u', 'n', 'n', 'i', 'n', 'g'],
        false,
        is_keyword_right_reject,
    ) {
        return Some(TitleStatus::Working);
    }
    if title.starts_with(". ") {
        return Some(TitleStatus::Working);
    }
    if title.starts_with("* ") {
        return Some(TitleStatus::Idle);
    }

    // Droid의 네이티브 이름-only 타이틀은 완료로 치지 않는다(훅이 권위).
    if has_droid && !has_legacy_name {
        return None;
    }

    Some(TitleStatus::Idle)
}

/// 이번 폴에서 드레인한 타이틀 변경들로부터 배지에 먹일 상태를 계산한다.
///
/// **Hooks 세션(Claude)은 항상 `None`** — 훅이 권위이고, 타이틀 감지가 훅 슬롯을
/// 덮으면 Claude의 정밀 상태가 깨진다. OscTitle 세션만 타이틀을 신뢰한다.
///
/// OscTitle 세션에서는 **가장 최근** 변경만 본다: 마지막이 `Set`이면 그 타이틀을
/// 감지하고, `Reset`(타이틀이 지워짐)이거나 변경이 없으면 `None`(배지를 건드리지
/// 않는다 — 6b-A는 상태만 다루고, 타이틀 리셋을 "에이전트 종료"로 해석하는 Orca의
/// 트래커는 이식하지 않는다).
pub fn title_status_update(changes: &[TitleChange], source: StatusSource) -> Option<HookState> {
    if source != StatusSource::OscTitle {
        return None;
    }
    match changes.last()? {
        TitleChange::Set(title) => detect_status_from_title(title),
        TitleChange::Reset => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 아래 **양성**(status를 내는) 픽스처는 전부 Orca의 실제 테스트
    // (`src/renderer/src/lib/agent-status.test.ts`)에서 그대로 가져왔다. 임의로
    // 지어낸 양성 타이틀은 "working"이 우연히 들어가 공허하게 통과할 수 있어 쓰지
    // 않는다. Orca는 working/permission/idle을 돌려주고, 우리는 그것을
    // Working/Waiting/Done으로 매핑한다.
    //
    // **예외**: `out_of_name_set_agents_cannot_derive_idle_from_title`의 goose/crush
    // 입력은 Orca 테스트 파일에 없다 — Orca는 out-of-set "에이전트 이름"을 직접
    // 테스트하지 않고, substring 비-매칭(`timestamp ready`·`clamp working`·`android
    // …`, 이건 아래 `cwd_and_path_fragments…`에 실제 Orca 픽스처로 있다)으로만
    // 게이트를 고정한다. goose/crush는 플랜 §3의 out-of-set 로스터에서 가져와, 실제
    // Orca 픽스처("claude ready"→idle)와 **같은 타이틀 모양**으로 구성했다 — 이름만
    // 바꿔 게이트가 결과를 가르는지 본다. 지어낸 타이틀이지만 게이트에 **걸려서
    // None이 되는** 케이스라, "working이 우연히 들어가 통과"하는 공허함과는 반대다.
    const WORKING: Option<HookState> = Some(HookState::Working);
    const WAITING: Option<HookState> = Some(HookState::Waiting); // Orca "permission"
    const DONE: Option<HookState> = Some(HookState::Done); // Orca "idle"
    const NONE: Option<HookState> = None;

    fn detect(title: &str) -> Option<HookState> {
        detect_status_from_title(title)
    }

    #[test]
    fn empty_and_non_agent_titles_are_none() {
        assert_eq!(detect(""), NONE);
        assert_eq!(detect("bash"), NONE);
        assert_eq!(detect("vim myfile.ts"), NONE);
    }

    /// Gemini 글리프는 이름 게이트보다 앞서는 범용 신호다.
    #[test]
    fn gemini_glyphs_map_to_status() {
        assert_eq!(detect("✋ Gemini CLI"), WAITING);
        assert_eq!(detect("✦ Gemini CLI"), WORKING);
        assert_eq!(detect("◇ Gemini CLI"), DONE);
        assert_eq!(detect("⏲  Working… (my-project)"), WORKING);
        // permission이 working보다 우선.
        assert_eq!(detect("✋✦ Gemini CLI"), WAITING);
    }

    /// **범용 braille-스피너 규칙**: 이름·글리프와 무관하게 스피너가 있으면 working.
    /// (c) 항목 — 이름 집합 밖 타이틀("some task", "process", "loading")도 working.
    #[test]
    fn braille_spinner_is_working_regardless_of_name() {
        for t in [
            "⠋ Codex is thinking",
            "⠙ some task",
            "⠹ aider running",
            "⠸ process",
            "⠼ opencode",
            "⠴ loading",
            "⠦ claude",
            "⠧ task",
            // Claude Code 실제 OSC 타이틀(태스크 설명/에이전트 이름).
            "⠐ User acknowledgment and confirmation",
            "⠂ Claude Code",
        ] {
            assert_eq!(
                detect(t),
                WORKING,
                "{t:?}: a braille spinner is the universal working signal, gated on nothing"
            );
        }
    }

    /// (a) 이름 집합 **안의** 에이전트는 대표 타이틀에서 working과 idle을 모두 낸다.
    #[test]
    fn in_name_set_agents_derive_working_and_idle() {
        // idle 키워드.
        assert_eq!(detect("claude ready"), DONE);
        assert_eq!(detect("codex idle"), DONE);
        assert_eq!(detect("aider done"), DONE);
        assert_eq!(detect("Codex done"), DONE);
        assert_eq!(detect("OpenCode ready"), DONE);
        // working 키워드.
        assert_eq!(detect("claude working on task"), WORKING);
        assert_eq!(detect("gemini thinking"), WORKING);
        assert_eq!(detect("opencode running tests"), WORKING);
        // permission.
        assert_eq!(detect("Claude Code - action required"), WAITING);
        assert_eq!(detect("codex - permission needed"), WAITING);
        assert_eq!(detect("gemini waiting for input"), WAITING);
        // 맨 이름은 idle.
        assert_eq!(detect("claude"), DONE);
        assert_eq!(detect("codex"), DONE);
        assert_eq!(detect("aider"), DONE);
        assert_eq!(detect("opencode"), DONE);
    }

    /// **(b) 이름 집합 밖 에이전트의 게이팅 — 이 플랜의 S1 핵심.** goose·crush는
    /// launchable이지만 16개 집합 밖이라, idle처럼 보이는 타이틀도 `None`이다.
    /// 스피너가 있을 때만 working을 낸다. 이것은 **의도된 동작**이다.
    ///
    /// 대조군으로 in-set 에이전트("claude ready"→Done)와 **같은 타이틀 모양**을
    /// 쓴다 — 결과가 갈리는 유일한 원인이 이름 게이트임을 고정한다.
    #[test]
    fn out_of_name_set_agents_cannot_derive_idle_from_title() {
        // idle처럼 보여도 None — 이름 게이트가 키워드 검사 전에 막는다.
        assert_eq!(
            detect("goose ready"),
            NONE,
            "goose is not in the 16-name set, so an idle-looking title yields no status"
        );
        assert_eq!(detect("crush - done"), NONE);
        assert_eq!(detect("goose idle"), NONE);
        assert_eq!(detect("crush waiting for input"), NONE);
        // 하지만 스피너는 범용이라 working은 낸다.
        assert_eq!(
            detect("⠋ goose"),
            WORKING,
            "the braille-spinner rule fires before the name gate, so out-of-set agents still \
             report working"
        );
        assert_eq!(detect("⠹ crush building"), WORKING);
    }

    /// 경계 인식: cwd/경로/하이픈 복합어의 이름 조각을 활동으로 오인하지 않는다.
    #[test]
    fn cwd_and_path_fragments_are_not_agent_activity() {
        assert_eq!(detect("~/codex-scratch"), NONE);
        assert_eq!(detect("~/codex already built"), NONE);
        assert_eq!(detect("opencode-blinker"), NONE);
        assert_eq!(detect("claude-scratch"), NONE);
        // "amp"를 담은 평범한 단어.
        assert_eq!(detect("timestamp ready"), NONE);
        assert_eq!(detect("clamp working"), NONE);
        assert_eq!(detect("example permission needed"), NONE);
        // "android" ⊃ "droid".
        assert_eq!(detect("android"), NONE);
        assert_eq!(detect("android emulator ready"), NONE);
        assert_eq!(detect("android build working"), NONE);
        // hermes 경로 조각.
        assert_ne!(detect("~/hermes/working"), WORKING);
        assert_eq!(detect("C:\\hermes\\ready"), NONE);
    }

    /// 경계의 오른쪽: 키워드는 `.`/`/`/`\`는 허용하되 단어문자·`-`는 거부한다.
    #[test]
    fn keyword_boundaries_reject_word_and_hyphen_but_allow_punctuation() {
        // 하이픈/경로/도트로 이어붙은 키워드는 매칭하지 않는다.
        assert_ne!(detect("~/codex/working"), WORKING);
        assert_ne!(detect("C:\\codex\\working"), WORKING);
        assert_ne!(detect("codex.working"), WORKING);
        // 트레일링 구두점은 허용.
        assert_eq!(detect("Codex done."), DONE);
        assert_eq!(detect("Aider idle!"), DONE);
        assert_eq!(detect("OpenCode ready?"), DONE);
        assert_eq!(detect("Codex working."), WORKING);
        assert_eq!(detect("Aider thinking..."), WORKING);
    }

    /// Claude Code 프리픽스 규칙.
    #[test]
    fn claude_code_prefixes() {
        assert_eq!(detect(". claude"), WORKING);
        assert_eq!(detect("* claude"), DONE);
        assert_eq!(detect("✳ User acknowledgment and confirmation"), DONE);
        assert_eq!(detect("✳ Claude Code"), DONE);
    }

    /// Claude 에이전트 관리 타이틀은 상태가 아니다(따옴표·경로·확장자 변형 포함).
    #[test]
    fn claude_management_title_is_excluded() {
        assert_eq!(detect("claude agents"), NONE);
        assert_eq!(detect("  Claude Agents  "), NONE);
        assert_eq!(detect("claude.exe agents"), NONE);
        assert_eq!(detect("Claude.CMD agents"), NONE);
        assert_eq!(detect("claude.bat agents"), NONE);
        assert_eq!(detect("Claude.PS1 agents"), NONE);
        assert_eq!(
            detect("C:\\Users\\dev\\AppData\\Roaming\\npm\\claude.cmd agents"),
            NONE
        );
        assert_eq!(
            detect("\"C:\\Users\\dev\\AppData\\Roaming\\npm\\claude.cmd\" agents"),
            NONE
        );
        // 하지만 뒤에 다른 말이 붙으면 관리 타이틀이 아니다 → 정상 판정.
        assert_eq!(detect("claude agents working"), WORKING);
    }

    /// OpenClaude는 claude로 흘러가지 않고 독립적으로 분류된다.
    #[test]
    fn openclaude_classifies_independently() {
        assert_eq!(detect("OpenClaude ready"), DONE);
        assert_eq!(detect("OpenClaude running"), WORKING);
        assert_eq!(detect("OpenClaude - action required"), WAITING);
        assert_eq!(detect("⠋ OpenClaude"), WORKING);
    }

    /// Cursor 네이티브 "Cursor Agent" 타이틀은 no-op(idle 아님) — turn 중
    /// 재방출이 스피너를 꺼버리지 않게.
    #[test]
    fn cursor_native_title_is_a_noop() {
        assert_eq!(detect("Cursor Agent"), NONE);
        assert_eq!(detect("cursor agent"), NONE);
        assert_eq!(detect("  Cursor Agent  "), NONE);
        // 합성 장식 타이틀은 정상 분류.
        assert_eq!(detect("⠋ Cursor Agent"), WORKING);
        assert_eq!(detect("Cursor ready"), DONE);
        assert_eq!(detect("Cursor - action required"), WAITING);
    }

    /// Droid/Hermes/Devin 합성 타이틀(droid/hermes는 정규식 3종에 포함).
    #[test]
    fn droid_hermes_devin_synthetic_titles() {
        for (name, _) in [("Droid", ()), ("Hermes", ()), ("Devin", ())] {
            assert_eq!(detect(&format!("⠋ {name}")), WORKING);
            assert_eq!(detect(&format!("{name} ready")), DONE);
            assert_eq!(detect(&format!("{name} - action required")), WAITING);
            assert_eq!(detect(&format!("{name} working")), WORKING);
        }
    }

    /// Factory Droid 네이티브 needs-input 타이틀은 완료로 치지 않는다:
    /// droid는 있지만 legacy 이름이 없으므로 `None`(hasDroid && !hasLegacy → null).
    #[test]
    fn factory_droid_needs_input_is_not_completion() {
        assert_eq!(detect("Factory Droid needs input"), NONE);
        assert_eq!(detect("Factory Droid needs your input"), NONE);
    }

    #[test]
    fn agent_names_are_case_insensitive() {
        assert_eq!(detect("CLAUDE"), DONE);
        assert_eq!(detect("Codex Working"), WORKING);
    }

    /// 레거시 π 타이틀은 idle(스피너 없을 때).
    #[test]
    fn legacy_pi_titles_are_idle() {
        assert_eq!(detect("π - my-project"), DONE);
        assert_eq!(detect("π - session-name - my-project"), DONE);
    }

    /// **HookState 매핑을 직접 고정한다.** 내부 TitleStatus가 옳아도 매핑이
    /// 뒤바뀌면(working↔done 등) 배지가 정반대로 뜬다.
    #[test]
    fn status_maps_to_the_correct_hook_state() {
        assert_eq!(detect("claude working on task"), Some(HookState::Working));
        assert_eq!(
            detect("Claude Code - action required"),
            Some(HookState::Waiting)
        );
        assert_eq!(detect("claude ready"), Some(HookState::Done));
    }

    // ---- 배선 헬퍼: 소스 게이팅 + "가장 최근 변경" 규칙 ----

    /// **OscTitle 세션만** 타이틀에서 상태를 얻는다. Hooks(Claude) 세션은 같은
    /// 타이틀이어도 `None` — 훅 권위를 타이틀이 덮으면 안 된다.
    #[test]
    fn only_osc_title_sessions_derive_status_from_title() {
        let working = [TitleChange::Set("codex working".to_string())];
        assert_eq!(
            title_status_update(&working, StatusSource::OscTitle),
            Some(HookState::Working),
            "an OSC-title session must pick up the title-derived status"
        );
        // Claude가 실제로 내보내는 idle 타이틀이지만 Hooks 세션에서는 무시된다.
        let claude_idle = [TitleChange::Set("✳ Claude Code".to_string())];
        assert_eq!(
            title_status_update(&claude_idle, StatusSource::Hooks),
            None,
            "a Hooks session must never let the title overwrite its authoritative hook status"
        );
    }

    /// 가장 최근 변경만 본다: 트레일링 `Reset`은 상태 없음(배지 유지), 빈 배치도.
    #[test]
    fn only_the_latest_title_change_matters() {
        assert_eq!(title_status_update(&[], StatusSource::OscTitle), None);
        let then_reset = [
            TitleChange::Set("codex working".to_string()),
            TitleChange::Reset,
        ];
        assert_eq!(
            title_status_update(&then_reset, StatusSource::OscTitle),
            None,
            "the title was cleared after the working title, so there is no current status"
        );
        // 반대로 마지막이 Set이면 그 값이 이긴다(앞선 Reset은 무시).
        let then_set = [
            TitleChange::Reset,
            TitleChange::Set("claude ready".to_string()),
        ];
        assert_eq!(
            title_status_update(&then_set, StatusSource::OscTitle),
            Some(HookState::Done)
        );
    }
}
