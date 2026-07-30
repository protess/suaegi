//! VERBATIM port of Orca's `src/shared/setup-runner-command.ts` (87 lines)
//! @ v1.4.146-rc.0. Line citations below (`SRC:N`) refer to that source file.
//!
//! Ported: `SRC:3` [`SetupRunnerCommandPlatform`], `SRC:4`
//! [`SetupRunnerCommandShell`], `SRC:6-10` [`SetupRunnerCommandResolution`],
//! `SRC:12-17` [`build_setup_runner_command`], `SRC:19-30`
//! [`get_setup_runner_command_platform_for_path`], `SRC:32-64`
//! [`resolve_setup_runner_command`], `SRC:66-69` [`is_wsl_unc_path`],
//! `SRC:71-75` [`wsl_unc_to_linux_path`], `SRC:77-83` (`quotePosixArg`,
//! private), `SRC:85-87` (`quoteWindowsArg`, private).
//!
//! # K1 — two same-named-but-swapped-order enums, NOT interchangeable
//! `SetupRunnerCommandPlatform = 'windows' | 'posix'` (the caller's *input*
//! hint — "what host is this?") and `SetupRunnerCommandShell = 'posix' |
//! 'windows'` (the *resolved output* — "what shell will actually run this
//! command?") are modeled as two distinct Rust enums, never unified. A
//! `Windows` platform legitimately resolves to a `Posix` shell for a WSL UNC
//! runner path (`SRC:37-44`) — collapsing the two types would make that
//! resolution look like a type error instead of the intended branch.
//!
//! # K14 — reusing `suaegi_path::is_windows_absolute_path_like` here IS the
//! faithful port, unlike the mcp-config M1 precedent that rejected it
//! `SRC:1` imports exactly this function from `cross-platform-path.ts`, and
//! it is already ported verbatim at
//! `suaegi-path::cross_platform_path::is_windows_absolute_path_like`. An
//! earlier milestone (mcp-config M1) rejected reusing this same helper for a
//! different module whose contract needed a UNC *two-component* requirement
//! this predicate does not enforce; here there is no such extra contract —
//! Orca itself calls this exact function, so reuse is the correct choice.
//!
//! # K8 — the WSL-UNC-to-Linux-path match has no `/s` flag
//! `wslUncToLinuxPath`'s regex, `/^\/\/(wsl\.localhost|wsl\$)\/[^/]+(\/.*)?$/i`
//! (`SRC:73`), uses `.` in its trailing `(\/.*)?` group. Without JS's `s`
//! (`dotAll`) flag, `.` excludes the four line-terminator code points (LF,
//! CR, U+2028, U+2029). A "rest" segment (everything after the distro name)
//! that contains any of those four characters can never be matched by
//! `.*`, so the optional group — and, once the tail cannot reach `$`, the
//! *whole* regex — fails to match; `match?.[2] || '/'` then falls back to
//! `'/'`. This is a silent-conversion hazard, not an injection: such a path
//! silently collapses to the filesystem root. Ported verbatim, not
//! corrected — see [`k8_unc_path_containing_a_newline_silently_converts_to_root`]
//! in the test module below.
//!
//! # K9 — the WSL host-alias check folds ASCII only (no `/u` flag)
//! `isWslUncPath`'s `/^\/\/(wsl\.localhost|wsl\$)\//i` (`SRC:68`) and
//! `wslUncToLinuxPath`'s leading alias match are case-insensitive but,
//! lacking `/u`, JS folds only ASCII letters (the oracle exercises this with
//! literal `WSL.LOCALHOST` uppercase, `setup-runner-command.test.ts:11`). A
//! Rust `(?i)` via the `regex` crate would be Unicode-aware and fold wider
//! (e.g. U+017F LATIN SMALL LETTER LONG S folds to `'s'` under full Unicode
//! case folding, and U+212A KELVIN SIGN folds to `'k'`) — so this module
//! hand-rolls the alias comparison as an ASCII-only `eq_ignore_ascii_case`
//! (see [`strip_ascii_ci_prefix`]), which correctly treats those codepoints
//! as ordinary non-matching bytes, matching JS.

use suaegi_path::is_windows_absolute_path_like;

/// `SRC:3` — the caller-supplied hint for "what host is this runner script
/// on?". K1: a **different type** from [`SetupRunnerCommandShell`], despite
/// sharing variant names in the opposite declared order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRunnerCommandPlatform {
    Windows,
    Posix,
}

/// `SRC:4` — the *resolved* shell that will actually execute the command.
/// K1: NOT the same type as [`SetupRunnerCommandPlatform`] — a `Windows`
/// platform can resolve to a `Posix` shell (WSL UNC path, `SRC:37-44`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRunnerCommandShell {
    Posix,
    Windows,
}

/// `SetupRunnerCommandResolution` (`SRC:6-10`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRunnerCommandResolution {
    pub command: String,
    pub runner_script_path_for_shell: String,
    pub shell: SetupRunnerCommandShell,
}

/// `buildSetupRunnerCommand` (`SRC:12-17`).
pub fn build_setup_runner_command(
    runner_script_path: &str,
    platform: SetupRunnerCommandPlatform,
) -> String {
    resolve_setup_runner_command(runner_script_path, platform).command
}

/// `getSetupRunnerCommandPlatformForPath` (`SRC:19-30`). K14: reuses
/// [`is_windows_absolute_path_like`] verbatim (see module docs).
pub fn get_setup_runner_command_platform_for_path(
    runner_script_path: &str,
    fallback_platform: SetupRunnerCommandPlatform,
) -> SetupRunnerCommandPlatform {
    if is_windows_absolute_path_like(runner_script_path) {
        return SetupRunnerCommandPlatform::Windows;
    }
    if runner_script_path.starts_with('/') {
        return SetupRunnerCommandPlatform::Posix;
    }
    fallback_platform
}

/// `resolveSetupRunnerCommand` (`SRC:32-64`).
pub fn resolve_setup_runner_command(
    runner_script_path: &str,
    platform: SetupRunnerCommandPlatform,
) -> SetupRunnerCommandResolution {
    if platform == SetupRunnerCommandPlatform::Windows {
        if is_wsl_unc_path(runner_script_path) {
            let linux_path = wsl_unc_to_linux_path(runner_script_path);
            return SetupRunnerCommandResolution {
                command: format!("bash {}", quote_posix_arg(&linux_path)),
                runner_script_path_for_shell: linux_path,
                shell: SetupRunnerCommandShell::Posix,
            };
        }
        // K14: reuse again — a POSIX-looking path that is NOT also a
        // Windows-absolute-path-like string (so plain `/mnt/...`, remote
        // POSIX paths, etc.) stays on bash even from a Windows client.
        if runner_script_path.starts_with('/') && !is_windows_absolute_path_like(runner_script_path)
        {
            return SetupRunnerCommandResolution {
                command: format!("bash {}", quote_posix_arg(runner_script_path)),
                runner_script_path_for_shell: runner_script_path.to_string(),
                shell: SetupRunnerCommandShell::Posix,
            };
        }
        return SetupRunnerCommandResolution {
            command: format!("cmd.exe /c {}", quote_windows_arg(runner_script_path)),
            runner_script_path_for_shell: runner_script_path.to_string(),
            shell: SetupRunnerCommandShell::Windows,
        };
    }

    SetupRunnerCommandResolution {
        command: format!("bash {}", quote_posix_arg(runner_script_path)),
        runner_script_path_for_shell: runner_script_path.to_string(),
        shell: SetupRunnerCommandShell::Posix,
    }
}

/// `isWslUncPath` (`SRC:66-69`). K9: ASCII-only case fold, see module docs.
pub fn is_wsl_unc_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    match normalized.strip_prefix("//") {
        Some(after) => {
            strip_ascii_ci_prefix(after, "wsl.localhost/").is_some()
                || strip_ascii_ci_prefix(after, "wsl$/").is_some()
        }
        None => false,
    }
}

/// `wslUncToLinuxPath` (`SRC:71-75`). K8/K9: see module docs.
pub fn wsl_unc_to_linux_path(windows_path: &str) -> String {
    let normalized = windows_path.replace('\\', "/");
    wsl_unc_captured_rest(&normalized).unwrap_or_else(|| "/".to_string())
}

/// Hand-rolled capture of group 2 of
/// `^\/\/(wsl\.localhost|wsl\$)\/[^/]+(\/.*)?$/i`. Returns `None` whenever
/// JS's `match?.[2]` would be `undefined` — both when the whole regex fails
/// to match, AND when the optional group is legitimately absent (rest empty)
/// — the caller folds both to `'/'` (`SRC:74`, `match?.[2] || '/'`).
fn wsl_unc_captured_rest(normalized: &str) -> Option<String> {
    let after = normalized.strip_prefix("//")?;
    let after_alias = strip_ascii_ci_prefix(after, "wsl.localhost/")
        .or_else(|| strip_ascii_ci_prefix(after, "wsl$/"))?;
    // `[^/]+` — one or more non-slash chars (the distro name).
    let distro_end = after_alias.find('/').unwrap_or(after_alias.len());
    if distro_end == 0 {
        return None;
    }
    let rest = &after_alias[distro_end..];
    if rest.is_empty() {
        // No `/rest` at all: `(\/.*)?` legitimately matches nothing, so the
        // captured group is `undefined` in JS, not an empty string.
        return None;
    }
    // K8: `.` (no `/s` flag) excludes LF, CR, U+2028, U+2029.
    if rest.chars().all(is_js_regex_dot_char) {
        Some(rest.to_string())
    } else {
        None
    }
}

/// Whether `c` is matched by a JS `.` without the `/s` (dotAll) flag: every
/// character except the four line terminators LF, CR, U+2028, U+2029 (K8).
fn is_js_regex_dot_char(c: char) -> bool {
    !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// ASCII-only case-insensitive prefix match (K9): `prefix` must be ASCII
/// (both call sites pass ASCII literals), so slicing at `prefix.len()` bytes
/// is always a valid `char` boundary regardless of what follows.
fn strip_ascii_ci_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() >= prefix.len()
        && value.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        Some(&value[prefix.len()..])
    } else {
        None
    }
}

/// `quotePosixArg` (`SRC:77-83`). K2: bare if `value` matches
/// `^[A-Za-z0-9_./:-]+$`, else single-quote-wrap with `'` -> `'\''`.
/// **Duplicated verbatim in `setup_agent_sequencing.rs`** — the TS source
/// itself repeats this exact function body in both files (`SRC:77-83`,
/// `SAS:237-242`); this crate matches that duplication per the repo's
/// per-module-duplication charter (`suaegi-quickcmd`'s `utf16_slice_prefix`
/// precedent), rather than sharing one copy via `pub(crate)`.
fn quote_posix_arg(value: &str) -> String {
    if is_bare_safe_posix_arg(value) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `/^[A-Za-z0-9_./:-]+$/` — requires at least one char (K2: an empty string
/// is NOT bare; it gets wrapped as `''`).
fn is_bare_safe_posix_arg(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b':' | b'-'))
}

/// `quoteWindowsArg` (`SRC:85-87`). K3: `"` -> `""` is the
/// *CommandLineToArgvW* convention, NOT a cmd.exe escape — quoting parity
/// with the surrounding cmd.exe command line can break for inputs containing
/// `"`. Ported verbatim, not fixed; see
/// [`k3_quote_bearing_path_through_windows_resolver_breaks_quote_parity`]
/// below for the exact pinned hazard. Duplicated in
/// `setup_agent_sequencing.rs` for the same reason as [`quote_posix_arg`].
fn quote_windows_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Oracle: `setup-runner-command.test.ts` (7 `it`s / 8 assertions)
    // -----------------------------------------------------------------

    #[test]
    fn oracle_uses_bash_for_wsl_unc_runner_scripts_regardless_of_host_casing() {
        assert_eq!(
            build_setup_runner_command(
                "\\\\WSL.LOCALHOST\\Ubuntu\\home\\jin\\repo\\.git\\worktrees\\feature\\orca\\setup-runner.sh",
                SetupRunnerCommandPlatform::Windows
            ),
            "bash /home/jin/repo/.git/worktrees/feature/orca/setup-runner.sh"
        );
    }

    #[test]
    fn oracle_uses_bash_with_linux_paths_for_forward_slash_wsl_unc_runner_scripts() {
        assert_eq!(
            build_setup_runner_command(
                "//wsl.localhost/Ubuntu/home/jin/repo/.git/worktrees/feature/orca/setup-runner.sh",
                SetupRunnerCommandPlatform::Windows
            ),
            "bash /home/jin/repo/.git/worktrees/feature/orca/setup-runner.sh"
        );
    }

    #[test]
    fn oracle_keeps_generic_forward_slash_unc_runner_scripts_on_cmd_exe() {
        assert_eq!(
            build_setup_runner_command(
                "//server/share/repo/.git/orca/setup-runner.cmd",
                SetupRunnerCommandPlatform::Windows
            ),
            "cmd.exe /c \"//server/share/repo/.git/orca/setup-runner.cmd\""
        );
    }

    #[test]
    fn oracle_prefers_posix_for_absolute_posix_runner_paths_even_from_windows_clients() {
        assert_eq!(
            get_setup_runner_command_platform_for_path(
                "/remote/repo/.git/orca/setup-runner.sh",
                SetupRunnerCommandPlatform::Windows
            ),
            SetupRunnerCommandPlatform::Posix
        );
    }

    #[test]
    fn oracle_prefers_windows_for_native_windows_runner_paths_even_from_posix_clients() {
        assert_eq!(
            get_setup_runner_command_platform_for_path(
                "C:\\repo\\.git\\orca\\setup-runner.cmd",
                SetupRunnerCommandPlatform::Posix
            ),
            SetupRunnerCommandPlatform::Windows
        );
    }

    #[test]
    fn oracle_keeps_wsl_unc_paths_on_the_windows_resolver_so_they_can_be_converted() {
        assert_eq!(
            get_setup_runner_command_platform_for_path(
                "\\\\wsl.localhost\\Ubuntu\\home\\jin\\repo\\.git\\orca\\setup-runner.sh",
                SetupRunnerCommandPlatform::Posix
            ),
            SetupRunnerCommandPlatform::Windows
        );
    }

    #[test]
    fn oracle_keeps_forward_slash_unc_paths_on_the_windows_resolver() {
        assert_eq!(
            get_setup_runner_command_platform_for_path(
                "//wsl.localhost/Ubuntu/home/jin/repo/.git/orca/setup-runner.sh",
                SetupRunnerCommandPlatform::Posix
            ),
            SetupRunnerCommandPlatform::Windows
        );
        assert_eq!(
            get_setup_runner_command_platform_for_path(
                "//server/share/repo/.git/orca/setup-runner.cmd",
                SetupRunnerCommandPlatform::Posix
            ),
            SetupRunnerCommandPlatform::Windows
        );
    }

    // -----------------------------------------------------------------
    // Extra oracle pin 1: `main/providers/windows-shell-args.test.ts:384-388`
    // (issue #7236 regression guard). Only the `resolveSetupRunnerCommand`
    // half is in this crate's scope — the `resolveWindowsShellLaunchArgs`
    // half belongs to an unported module, so only the `command` shape and
    // its quote-parity assertion are pinned here.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_pin_issue_7236_wraps_the_setup_runner_in_balanced_double_quotes() {
        let runner_path = "C:/Users/alice/repo/.git/orca/setup-runner.cmd";
        let resolution =
            resolve_setup_runner_command(runner_path, SetupRunnerCommandPlatform::Windows);
        assert_eq!(
            resolution.command,
            "cmd.exe /c \"C:/Users/alice/repo/.git/orca/setup-runner.cmd\""
        );
        assert_eq!(resolution.command.matches('"').count() % 2, 0);
    }

    // -----------------------------------------------------------------
    // Extra oracle pin 2: `renderer/src/lib/setup-runner.test.ts:10-53`.
    // That module wraps this crate's logic with `navigator.userAgent`
    // platform sniffing (not ported — no `navigator` in Rust): it derives a
    // FALLBACK platform from the user agent, feeds the runner path through
    // `getSetupRunnerCommandPlatformForPath(path, fallback)` to get the
    // ACTUAL platform (the path itself can override the fallback — see
    // `oracle_prefers_windows_for_native_windows_runner_paths_even_from_posix_clients`
    // above), and only then calls `resolveSetupRunnerCommand`. Reproduced
    // here as an explicit two-step call per test rather than a single
    // `platform` argument, since collapsing the two steps changes the
    // outcome whenever the fallback and the path-implied platform disagree
    // (see the last case below).
    // -----------------------------------------------------------------

    /// Simulates the renderer wrapper's `buildSetupRunnerCommand(path)`:
    /// `fallback_platform` stands in for its `navigator.userAgent` sniff.
    fn renderer_build_setup_runner_command(
        runner_script_path: &str,
        fallback_platform: SetupRunnerCommandPlatform,
    ) -> String {
        let platform =
            get_setup_runner_command_platform_for_path(runner_script_path, fallback_platform);
        build_setup_runner_command(runner_script_path, platform)
    }

    #[test]
    fn oracle_pin_renderer_uses_bash_with_a_linux_path_for_wsl_unc_runner_scripts_on_windows() {
        assert_eq!(
            renderer_build_setup_runner_command(
                "\\\\wsl.localhost\\Ubuntu\\home\\jin\\repo\\.git\\worktrees\\feature\\orca\\setup-runner.sh",
                SetupRunnerCommandPlatform::Windows
            ),
            "bash /home/jin/repo/.git/worktrees/feature/orca/setup-runner.sh"
        );
    }

    #[test]
    fn oracle_pin_renderer_uses_cmd_exe_for_native_windows_runner_scripts() {
        assert_eq!(
            renderer_build_setup_runner_command(
                "C:\\repo\\.git\\orca\\setup-runner.cmd",
                SetupRunnerCommandPlatform::Windows
            ),
            "cmd.exe /c \"C:\\repo\\.git\\orca\\setup-runner.cmd\""
        );
    }

    #[test]
    fn oracle_pin_renderer_uses_bash_for_posix_runner_paths_on_windows_clients() {
        assert_eq!(
            renderer_build_setup_runner_command(
                "/home/dev/repo/.git/orca/setup-runner.sh",
                SetupRunnerCommandPlatform::Windows
            ),
            "bash /home/dev/repo/.git/orca/setup-runner.sh"
        );
    }

    #[test]
    fn oracle_pin_renderer_uses_cmd_exe_for_native_windows_runner_scripts_on_non_windows_clients() {
        // The renderer test stubs `navigator.userAgent` to a Mac string
        // here — a `Posix` fallback — but the PATH itself is
        // Windows-absolute-like, so `getSetupRunnerCommandPlatformForPath`
        // overrides the fallback to `Windows` before resolving. Calling
        // `resolve_setup_runner_command` directly with the raw `Posix`
        // fallback (skipping that override step) would be UNFAITHFUL here:
        // the `Posix` branch always emits `bash`, regardless of the path.
        assert_eq!(
            renderer_build_setup_runner_command(
                "C:\\repo\\.git\\orca\\setup-runner.cmd",
                SetupRunnerCommandPlatform::Posix
            ),
            "cmd.exe /c \"C:\\repo\\.git\\orca\\setup-runner.cmd\""
        );
    }

    // -----------------------------------------------------------------
    // K1: windows platform + WSL UNC path -> posix shell.
    // -----------------------------------------------------------------

    #[test]
    fn k1_windows_platform_with_wsl_unc_path_resolves_to_posix_shell() {
        let resolution = resolve_setup_runner_command(
            "\\\\wsl.localhost\\Ubuntu\\home\\jin\\repo\\setup-runner.sh",
            SetupRunnerCommandPlatform::Windows,
        );
        assert_eq!(resolution.shell, SetupRunnerCommandShell::Posix);
        assert_eq!(
            resolution.runner_script_path_for_shell,
            "/home/jin/repo/setup-runner.sh"
        );
    }

    // -----------------------------------------------------------------
    // K2: bare/quoted boundary + the `'` -> `'\''` transform.
    // -----------------------------------------------------------------

    #[test]
    fn k2_bare_safe_characters_stay_unquoted() {
        // `-`, `:`, `/`, `.` (plus alnum/`_`) all stay bare.
        assert_eq!(quote_posix_arg("a-b:c/d.e_1"), "a-b:c/d.e_1");
        assert_eq!(quote_posix_arg("-:/."), "-:/.");
    }

    #[test]
    fn k2_space_forces_quoting() {
        assert_eq!(quote_posix_arg("a b"), "'a b'");
    }

    #[test]
    fn k2_single_quote_transform_is_quote_backslash_quote_quote() {
        assert_eq!(quote_posix_arg("it's"), "'it'\\''s'");
    }

    #[test]
    fn k2_empty_string_is_not_bare() {
        assert_eq!(quote_posix_arg(""), "''");
    }

    // -----------------------------------------------------------------
    // K3: a `"`-bearing path through the windows resolver.
    // -----------------------------------------------------------------

    #[test]
    fn k3_quote_bearing_path_through_windows_resolver_breaks_quote_parity() {
        // Deliberately preserved upstream hazard — `quoteWindowsArg`'s `"` ->
        // `""` is the CommandLineToArgvW convention, not a cmd.exe escape.
        let path = "C:\\a\"&calc&\".cmd";
        let resolution = resolve_setup_runner_command(path, SetupRunnerCommandPlatform::Windows);
        assert_eq!(resolution.command, "cmd.exe /c \"C:\\a\"\"&calc&\"\".cmd\"");
    }

    // -----------------------------------------------------------------
    // K8: a UNC path containing a newline -> converts to `/`.
    // -----------------------------------------------------------------

    #[test]
    fn k8_unc_path_containing_a_newline_silently_converts_to_root() {
        let path = "//wsl.localhost/Ubuntu/home/jin\nrepo/setup-runner.sh";
        assert_eq!(wsl_unc_to_linux_path(path), "/");
    }

    #[test]
    fn k8_unc_path_without_a_break_character_converts_normally() {
        let path = "//wsl.localhost/Ubuntu/home/jin/repo/setup-runner.sh";
        assert_eq!(
            wsl_unc_to_linux_path(path),
            "/home/jin/repo/setup-runner.sh"
        );
    }

    #[test]
    fn k8_unc_path_with_no_rest_segment_falls_back_to_root() {
        assert_eq!(wsl_unc_to_linux_path("//wsl.localhost/Ubuntu"), "/");
    }

    // -----------------------------------------------------------------
    // K9: U+212A / U+017F do NOT match the WSL host check.
    // -----------------------------------------------------------------

    #[test]
    fn k9_confusable_unicode_letters_do_not_fold_to_ascii_in_the_wsl_host_check() {
        // U+017F (LATIN SMALL LETTER LONG S) folds to 's' under full Unicode
        // case folding (what a Unicode-aware `(?i)` engine would use) — our
        // ASCII-only compare correctly rejects it, matching JS's `/i`
        // (no `/u`) behavior.
        assert!(!is_wsl_unc_path("//w\u{017F}l.localhost/Ubuntu/x"));
        assert_eq!(
            wsl_unc_to_linux_path("//w\u{017F}l.localhost/Ubuntu/x"),
            "/"
        );
        // U+212A (KELVIN SIGN) folds to 'k' under full Unicode case folding.
        // "wsl"/"wsl$" contain no 'k', so this isn't a near-miss the way
        // U+017F is — it's included as the paired canonical ASCII-vs-Unicode
        // -fold example (see the `js-lowercase-two-mechanisms` project note)
        // and must still not match.
        assert!(!is_wsl_unc_path("//w\u{212A}l.localhost/Ubuntu/x"));
    }

    // -----------------------------------------------------------------
    // Other exact-string pins for mutation-killability.
    // -----------------------------------------------------------------

    #[test]
    fn quote_windows_arg_doubles_embedded_quotes() {
        assert_eq!(quote_windows_arg("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn is_wsl_unc_path_accepts_the_dollar_alias_case_insensitively() {
        assert!(is_wsl_unc_path("\\\\WSL$\\Ubuntu\\home"));
        assert!(!is_wsl_unc_path("//not-wsl/Ubuntu/home"));
    }
}
