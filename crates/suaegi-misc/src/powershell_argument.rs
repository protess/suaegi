//! PowerShell argument quoting — verbatim port of Orca's
//! `src/shared/powershell-native-argument.ts` (@ v1.4.150-rc.0).
//!
//! Why: Windows PowerShell 5.1 drops unescaped embedded quotes when it
//! constructs argv for native executables such as `wsl.exe`. Kept
//! dependency-free (no `regex` crate) per the crate charter — see D8: the
//! `/(\\*)"/g` → `$1$1\"` substitution is hand-scanned instead.

/// Quote a literal for PowerShell *source* parsing: every `'` doubles to
/// `''`, then the whole value is wrapped in `'…'`.
pub fn quote_powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Quote a value as a single native-argv argument, pre-escaping embedded `"`
/// so PowerShell 5.1 does not silently drop them when marshaling argv for a
/// native (non-PowerShell) executable.
///
/// D8: the original regex `/(\\*)"/g` replaces a run of `n` backslashes
/// immediately preceding a `"` with `2n` backslashes followed by a literal
/// `\"`. We hand-scan for the same effect: walk the string copying
/// characters through; on hitting a `"`, emit one *additional* backslash per
/// backslash already copied from the immediately-preceding run (doubling
/// it), then emit `\"`. A trailing backslash run *not* followed by `"` is
/// left unchanged, matching the regex (which only matches runs anchored
/// immediately before a quote).
pub fn quote_powershell_native_argument(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    let mut preceding_backslash_run = 0usize;

    for ch in value.chars() {
        match ch {
            '\\' => {
                preceding_backslash_run += 1;
                escaped.push(ch);
            }
            '"' => {
                // Double the preceding backslash run (it is already copied
                // once above; add the same count again), then emit `\"`.
                for _ in 0..preceding_backslash_run {
                    escaped.push('\\');
                }
                escaped.push('\\');
                escaped.push('"');
                preceding_backslash_run = 0;
            }
            _ => {
                preceding_backslash_run = 0;
                escaped.push(ch);
            }
        }
    }

    quote_powershell_literal(&escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: powershell-native-argument.test.ts

    #[test]
    fn escapes_literals_for_powershell_source_parsing() {
        assert_eq!(
            quote_powershell_literal("WSL 'Preview'"),
            "'WSL ''Preview'''"
        );
    }

    #[test]
    fn pre_escapes_embedded_quotes_for_windows_native_argv_parsing() {
        assert_eq!(
            quote_powershell_native_argument(r#"eval "decoded""#),
            r#"'eval \"decoded\"'"#
        );
        assert_eq!(
            quote_powershell_native_argument(r#"before\"after"#),
            r#"'before\\\"after'"#
        );
    }

    // Extra pins (oracle-silent), per plan D8:

    /// Mixed `'` and `"` in the same value: the literal-quote doubling and
    /// the native-quote backslash-escaping must both apply, in that order
    /// (escape first, then literal-wrap).
    #[test]
    fn pin_mixed_single_and_double_quotes() {
        assert_eq!(
            quote_powershell_native_argument(r#"it's "quoted""#),
            r#"'it''s \"quoted\"'"#
        );
    }

    /// A backslash run of length >= 2 immediately before a `"` must be
    /// doubled to length >= 4, not just incremented by one.
    #[test]
    fn pin_multi_backslash_run_before_quote_is_doubled() {
        assert_eq!(
            quote_powershell_native_argument(r#"path\\\"quoted"#),
            r#"'path\\\\\\\"quoted'"#
        );
    }

    /// A trailing backslash run with no following `"` is left unchanged
    /// (the regex only matches runs anchored immediately before a quote).
    #[test]
    fn pin_trailing_backslash_run_without_quote_is_unchanged() {
        assert_eq!(
            quote_powershell_native_argument(r"trailing\\\"),
            r"'trailing\\\'"
        );
    }

    /// Empty string round-trips to an empty quoted literal.
    #[test]
    fn pin_empty_string() {
        assert_eq!(quote_powershell_native_argument(""), "''");
        assert_eq!(quote_powershell_literal(""), "''");
    }
}
