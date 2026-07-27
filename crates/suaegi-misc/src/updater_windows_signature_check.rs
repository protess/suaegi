//! Windows updater signature-check failure classification — verbatim port of
//! Orca's `src/shared/updater-windows-signature-check.ts` (@ v1.4.146-rc.0).
//!
//! Why: electron-updater verifies Windows installers by spawning PowerShell's
//! `Get-AuthenticodeSignature`. Antivirus/EDR interception, its hardcoded 20s
//! timeout, or a stalled revocation lookup kills that spawn, and the raw
//! child-process failure becomes the update error. Detect that shape so the
//! UI can explain it instead of dumping the command line. A true signature
//! mismatch ("not signed by the application owner") must NOT match — that is
//! a real integrity failure, not environment interference.
//!
//! # ⚠⚠ N1 — precedent INVERSION: `to_lowercase()`, not `to_ascii_lowercase`
//! This module has **no regex** — the source is `String.prototype.toLowerCase()`
//! together with `.includes()`. `.toLowerCase()` is **full-Unicode** Default
//! Case Conversion in JS, and so is Rust's `str::to_lowercase` — the two *agree*.
//! This is the *other* of the [two lowercasing mechanisms][crate::js_ws]:
//! `codex_auth_errors` and `worktree_submodule_removal` prescribe
//! `to_ascii_lowercase` only because *those* sources are non-`u` `/…/i`
//! regexes, where JS folds ASCII only (`/k/i.test('K')` → `false`, yet
//! `'K'.toLowerCase()` → `"k"`). That reasoning does NOT apply here. Use
//! `str::to_lowercase()`. Future ports: check which mechanism (regex vs.
//! `.toLowerCase()`) the *source* actually uses before reaching for
//! `to_ascii_lowercase` by default.
//!
//! Observationally, on these two exact phrases, `to_ascii_lowercase` would
//! currently produce the same answer (documented equivalence, NOT a mutation
//! target): the only ASCII-folding non-ASCII codepoints are U+212A KELVIN
//! SIGN → `k` and U+0130 LATIN CAPITAL LETTER I WITH DOT ABOVE → `i` + a
//! combining dot above (U+0307). Neither phrase contains a `k`
//! (`get-authenticodesignature`, `not signed by the application owner`), and
//! U+0130's extra combining character breaks the match either way. The
//! moment either phrase gains a `k`, the two diverge silently — hence
//! `to_lowercase()`, not the ASCII variant, is what's actually correct here.
//! Also note `to_lowercase()` **can change length** (e.g. `İ` → 2 chars,
//! final sigma) — do not assume in-place/length-preserving lowercasing.
//!
//! # ⚠⚠ N2 — security: the veto is NOT oracle-pinned; do not delete it
//! [`is_windows_signature_check_unavailable_failure`] is a 3-leaf: ① contains
//! `"not signed by the application owner"` → **veto, `false`** (evaluated
//! FIRST) ② contains `"get-authenticodesignature"` → `true` ③ else `false`.
//! [`is_windows_signature_mismatch_failure`] is a single leaf (no veto,
//! independently re-lowercased).
//!
//! The upstream oracle's veto test fixture contains no
//! `get-authenticodesignature` substring, so leaf ② alone already returns
//! `false` for it — **the veto is not actually exercised by the oracle**.
//! Deleting it passes the whole suite. Consequence (consumer-traced,
//! `UpdateCard.tsx:388-423`): a genuine signature MISMATCH misclassified as
//! "check unavailable" surfaces a **Retry Download** default action linking
//! directly to the release for the version that just failed publisher
//! verification — one click from installing a wrong-publisher binary. The
//! reverse misclassification only costs availability. The asymmetry means
//! the veto must be kept maximally aggressive even though nothing upstream
//! forces it. This module pins a message containing BOTH phrases to force
//! the veto to matter (see `pin_veto_survives_when_both_phrases_present`).
//!
//! # N9 — `encodeURIComponent`/surrogate note does not apply here
//! (No `encodeURIComponent` in this module; see `agent_notification_id` for
//! that note.)

/// Port of `isWindowsSignatureCheckUnavailableFailure` (`:8-14`). See the
/// module header for the N1 lowercasing choice and the N2 veto rationale.
pub fn is_windows_signature_check_unavailable_failure(message: &str) -> bool {
    let normalized = message.to_lowercase();
    if normalized.contains("not signed by the application owner") {
        return false;
    }
    normalized.contains("get-authenticodesignature")
}

/// Port of `isWindowsSignatureMismatchFailure` (`:21-23`). Electron-updater
/// throws `ERR_UPDATER_INVALID_SIGNATURE` with this exact phrase when the
/// downloaded installer is validly readable but signed by the wrong
/// publisher — a genuine integrity failure that must drive a security-stop
/// message, never a silent "retry" framing.
pub fn is_windows_signature_mismatch_failure(message: &str) -> bool {
    message.to_lowercase().contains("not signed by the application owner")
}

#[cfg(test)]
mod tests {
    use super::{
        is_windows_signature_check_unavailable_failure, is_windows_signature_mismatch_failure,
    };

    // Oracle: updater-windows-signature-check.test.ts

    #[test]
    fn matches_the_powershell_command_failure_shape() {
        assert!(is_windows_signature_check_unavailable_failure(
            "Command failed: set \"PSModulePath=\" & chcp 65001 >NUL & powershell.exe -NoProfile \
             -NonInteractive -InputFormat None -Command \"Get-AuthenticodeSignature -LiteralPath \
             'C:\\Users\\u\\AppData\\Local\\orca-updater\\pending\\orca-windows-setup.exe' | \
             ConvertTo-Json -Compress\""
        ));
    }

    #[test]
    fn matches_the_stderr_failure_shape() {
        assert!(is_windows_signature_check_unavailable_failure(
            "Cannot execute Get-AuthenticodeSignature, stderr: Access is denied. \
             Failing signature validation due to unknown stderr."
        ));
    }

    #[test]
    fn does_not_match_a_genuine_signature_mismatch() {
        assert!(!is_windows_signature_check_unavailable_failure(
            "New version 1.4.144 is not signed by the application owner: \
             publisherNames: SignPath Foundation, raw info: {\"Status\": 0}"
        ));
    }

    #[test]
    fn does_not_match_unrelated_update_errors() {
        assert!(!is_windows_signature_check_unavailable_failure(
            "net::ERR_HTTP2_PROTOCOL_ERROR"
        ));
        assert!(!is_windows_signature_check_unavailable_failure(
            "Cannot find channel \"latest.yml\" (404)"
        ));
    }

    #[test]
    fn matches_the_wrong_publisher_integrity_failure() {
        assert!(is_windows_signature_mismatch_failure(
            "New version 1.4.144 is not signed by the application owner: \
             publisherNames: SignPath Foundation, raw info: {\"Status\": 0}"
        ));
    }

    #[test]
    fn is_mutually_exclusive_with_the_check_unavailable_classifier() {
        let mismatch =
            "New version 1.4.144 is not signed by the application owner: publisherNames: X";
        assert!(is_windows_signature_mismatch_failure(mismatch));
        assert!(!is_windows_signature_check_unavailable_failure(mismatch));

        let blocked = "Command failed: \u{2026} Get-AuthenticodeSignature -LiteralPath \u{2026}";
        assert!(is_windows_signature_check_unavailable_failure(blocked));
        assert!(!is_windows_signature_mismatch_failure(blocked));
    }

    #[test]
    fn does_not_match_unrelated_errors() {
        assert!(!is_windows_signature_mismatch_failure("net::ERR_HTTP2_PROTOCOL_ERROR"));
    }

    // Mandatory extra pins (oracle-silent — plan §5):

    /// N2 crux pin: a message containing BOTH phrases is the only fixture
    /// that forces the veto to matter — the oracle's own veto fixture never
    /// contains `get-authenticodesignature`, so leaf ② alone already returns
    /// `false` there and a deleted veto passes the whole upstream suite.
    #[test]
    fn pin_veto_survives_when_both_phrases_present() {
        let message =
            "Get-AuthenticodeSignature failed: not signed by the application owner";
        assert!(!is_windows_signature_check_unavailable_failure(message));
        assert!(is_windows_signature_mismatch_failure(message));
    }

    /// N4: an uppercase mismatch phrase must still match — kills a mutant
    /// that drops the lowercasing entirely (plain `contains`).
    #[test]
    fn pin_uppercase_mismatch_phrase_matches() {
        let message = "NOT SIGNED BY THE APPLICATION OWNER";
        assert!(is_windows_signature_mismatch_failure(message));
        assert!(!is_windows_signature_check_unavailable_failure(message));
    }

    /// N4: an uppercase check-unavailable phrase must still match.
    #[test]
    fn pin_uppercase_check_unavailable_phrase_matches() {
        assert!(is_windows_signature_check_unavailable_failure(
            "GET-AUTHENTICODESIGNATURE timed out"
        ));
    }

    /// N5: empty string and an unrelated message both classify false on
    /// both predicates — no anchoring, no trim, no ANSI stripping needed.
    #[test]
    fn pin_empty_and_unrelated_message_is_false_on_both() {
        assert!(!is_windows_signature_check_unavailable_failure(""));
        assert!(!is_windows_signature_mismatch_failure(""));
        assert!(!is_windows_signature_check_unavailable_failure("totally unrelated"));
        assert!(!is_windows_signature_mismatch_failure("totally unrelated"));
    }

    /// N1 regression guard: neither of the two literal phrases contains a
    /// `k`, so a stray U+212A KELVIN SIGN elsewhere in the message cannot
    /// create a spurious match (it folds to `k` under `to_lowercase`, which
    /// the phrases don't need) — this freezes that current behavior so a
    /// future phrase edit that introduces a `k` gets a sentinel. It is NOT a
    /// mutation target for `to_ascii_lowercase` (documented equivalence).
    #[test]
    fn pin_stray_kelvin_sign_does_not_create_a_spurious_match() {
        // Precondition: to_lowercase folds U+212A to ASCII 'k', proving the
        // mechanism this module intentionally relies on (N1) — yet neither
        // phrase contains a 'k', so this fold is inert here.
        assert_eq!('\u{212A}'.to_lowercase().collect::<String>(), "k");

        let message = "\u{212A} not signed by the application owner";
        assert!(is_windows_signature_mismatch_failure(message));
        assert!(!is_windows_signature_check_unavailable_failure(message));
    }

    /// N1 regression guard: replacing an `i` inside either phrase with
    /// U+0130 (which folds to `i` + a combining dot above, U+0307) breaks
    /// the match under `to_lowercase` — the combining character prevents the
    /// literal ASCII substring from reappearing. `to_ascii_lowercase` would
    /// also fail to match (U+0130 isn't ASCII), so this is the documented
    /// equivalence, not a divergence; pinned so a future phrase change is
    /// forced to re-examine this note.
    #[test]
    fn pin_dotted_capital_i_breaks_the_match_on_both_mechanisms() {
        let dotted_i_in_check = "get-authenticodes\u{0130}gnature";
        assert!(!is_windows_signature_check_unavailable_failure(dotted_i_in_check));

        let dotted_i_in_mismatch = "not s\u{0130}gned by the application owner";
        assert!(!is_windows_signature_mismatch_failure(dotted_i_in_mismatch));
    }
}
