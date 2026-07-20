const MAX_LEN: usize = 60;
const MAX_SUFFIX: u32 = 100;

// 대소문자 무관 비교. CON.txt류 확장자 케이스는 sanitize가 '.'을 제거하므로 불필요.
// 위첨자 ¹²³ 변형(COM¹ 등)도 Windows 예약 — is_alphanumeric을 통과하므로 명시 필요.
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "com¹", "com²", "com³", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8",
    "lpt9", "lpt¹", "lpt²", "lpt³",
];

/// 유니코드 문자/숫자만 유지하고 나머지는 `-`로. 출력이 `[alnum|-]`로만 구성되므로
/// git ref로도 디렉토리명으로도 항상 유효하다 (Orca worktree-logic 차용).
/// Windows 예약 장치명은 디렉토리 생성이 불가능하므로 접미사로 회피한다.
pub fn sanitize_worktree_name(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true; // 선행 대시 방지
    for ch in input.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed: String = out
        .trim_matches(|c| c == '-' || c == '.')
        .chars()
        .take(MAX_LEN)
        .collect();
    let trimmed = trimmed.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        return "workspace".to_string();
    }
    if WINDOWS_RESERVED.contains(&trimmed.to_ascii_lowercase().as_str()) {
        return format!("{trimmed}-ws");
    }
    trimmed
}

pub fn candidate_names(base: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(base.to_string()).chain((2..=MAX_SUFFIX).map(move |n| {
        let suffix = format!("-{n}");
        // suffix 포함 총 길이 MAX_LEN 유지 — base를 잘라 맞춘다
        let take = MAX_LEN.saturating_sub(suffix.chars().count());
        let head: String = base.chars().take(take).collect();
        let head = head.trim_end_matches('-');
        format!("{head}{suffix}")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unicode_letters_and_digits() {
        assert_eq!(sanitize_worktree_name("버그수정 v2"), "버그수정-v2");
    }

    #[test]
    fn collapses_and_trims_dashes() {
        assert_eq!(sanitize_worktree_name("--fix!!bug--"), "fix-bug");
    }

    #[test]
    fn rejects_dot_prefix_and_double_dots() {
        assert_eq!(sanitize_worktree_name("..hidden"), "hidden");
        assert_eq!(sanitize_worktree_name("a..b"), "a-b");
    }

    #[test]
    fn empty_input_falls_back() {
        assert_eq!(sanitize_worktree_name("!!!"), "workspace");
    }

    #[test]
    fn truncates_to_60_chars() {
        let long = "a".repeat(100);
        assert_eq!(sanitize_worktree_name(&long).chars().count(), 60);
    }

    #[test]
    fn windows_reserved_names_get_suffix() {
        assert_eq!(sanitize_worktree_name("con"), "con-ws");
        assert_eq!(sanitize_worktree_name("CON"), "CON-ws");
        assert_eq!(sanitize_worktree_name("lpt9"), "lpt9-ws");
        // 예약어가 아닌 유사 이름은 그대로
        assert_eq!(sanitize_worktree_name("console"), "console");
    }

    #[test]
    fn windows_superscript_reserved_names_get_suffix() {
        // COM¹/LPT³ 등 위첨자 변형도 Windows 예약 장치명이다
        assert_eq!(sanitize_worktree_name("com¹"), "com¹-ws");
        assert_eq!(sanitize_worktree_name("LPT³"), "LPT³-ws");
    }

    #[test]
    fn output_charset_is_always_ref_safe() {
        // 브랜치명/디렉토리명 안전성의 근거: 문자·숫자·단일 대시 외 아무것도 남지 않는다
        for input in [
            "a b",
            "x/../y",
            "--",
            "évoluer!",
            "한글 이름",
            "a..b",
            ".git",
            "-x",
            "nul",
        ] {
            let out = sanitize_worktree_name(input);
            assert!(!out.is_empty());
            assert!(
                out.chars().all(|c| c.is_alphanumeric() || c == '-'),
                "{input} -> {out}"
            );
            assert!(!out.starts_with('-') && !out.ends_with('-'));
            assert!(!out.contains("--"));
        }
    }

    #[test]
    fn candidates_start_with_base_then_numbered() {
        let mut it = candidate_names("fix");
        assert_eq!(it.next().unwrap(), "fix");
        assert_eq!(it.next().unwrap(), "fix-2");
        assert_eq!(candidate_names("fix").last().unwrap(), "fix-100");
    }

    #[test]
    fn suffixed_candidates_stay_within_max_len() {
        let base: String = "a".repeat(60);
        for name in candidate_names(&base) {
            assert!(name.chars().count() <= 60, "{name}");
        }
    }
}
