//! Agent-event notification id construction — verbatim port of Orca's
//! `src/shared/agent-notification-id.ts` (@ v1.4.146-rc.0).
//!
//! Why: `use-notification-dispatch.ts` and `store/slices/ui.ts` need a stable
//! id per agent-turn notification so OS notification centers can dedupe /
//! replace rather than stack duplicates across re-renders.
//!
//! # ⚠⚠ N6 — `encodeURIComponent` is load-bearing, not decoration
//! Real inputs are NOT id-safe on their own: a real `paneKey` is
//! `tabId:leafUuid` and a real `worktreeId` is `repoId::path` — **both
//! contain `:`**, the exact character this module joins fields with. Percent-
//! encoding `:` → `%3A` (and `%` → `%25`, guarding the escape character
//! itself) makes the 4-segment `agent:<worktree>:<pane>:<ts>` split
//! unambiguous and injective. Skipping the encoding and doing
//! `format!("agent:{w}:{p}:{t}")` directly is a genuine collision bug: with
//! `worktree_id = "a"`, `pane_key = "b:c"` the joined string is
//! `"agent:a:b:c:<ts>"`, byte-for-byte identical to
//! `worktree_id = "a:b"`, `pane_key = "c"`. The upstream oracle never
//! constructs a colon-bearing fixture, so it cannot catch this — see the
//! `pin_colon_collision_requires_percent_encoding` test below, which
//! constructs the collision directly.
//!
//! This is NOT a case like `ephemeral_setup_terminal_worktree_id`'s
//! "preserve upstream collision risk" — there is no equivalent risk to
//! preserve here; the encoding genuinely removes an ambiguity that would
//! otherwise exist.
//!
//! The unreserved set (`A-Za-z0-9 - _ . ! ~ * ' ( )`) is identical to
//! `suaegi-forge::repo_icon`'s private `encode_uri_component` — duplicated
//! rather than shared cross-crate (this repo's per-module-copy charter, see
//! plan §0), not re-derived.
//!
//! # ⚠⚠ N7 — the oracle never asserts an id literal
//! Of the three ported modules, this one is the **least constrained** by its
//! oracle: every fixture only checks determinism (`f(x) === f(x)`, vacuous —
//! every pure function satisfies it, see N10 below), that the id changes
//! when `stateStartedAt` changes, and that missing fields yield `None`. None
//! of that pins the actual string shape. Every one of these WRONG
//! implementations passes the upstream suite unmodified: no percent-encoding
//! at all; `'|'`-separated fields in a different order; returning only
//! `Some(state_started_at.to_string())`; wrong rounding (e.g. no `trunc`);
//! or accepting `NaN` and wrapping it in `Some`. This module pins literal id
//! strings directly (including the colon-collision pair) so a regression in
//! the actual format is caught here even though the oracle can't see it.
//!
//! # N8 — numeric model and truthiness
//! `stateStartedAt` is modeled as `Option<f64>`, not `Option<i64>`: JS's
//! `String(Math.trunc(x))` on a huge magnitude uses exponential notation
//! (e.g. `1e+21`), which an integer type can't reproduce, so the type is kept
//! wide even though real callers pass millisecond epoch timestamps.
//! `stateStartedAt === 0` is accepted (JS's `typeof stateStartedAt ===
//! 'number'` check runs before any truthiness test) and becomes `...:0` in
//! the id — it is NOT filtered out. `worktreeId`/`paneKey`, by contrast, ARE
//! rejected when empty: the source's `!worktreeId || !paneKey` is a
//! truthiness check, not a type check, so `""` fails it exactly like
//! `undefined`/`null` would. Modeled here as `.filter(|s| !s.is_empty())`
//! rather than an `is_some()` check alone.
//!
//! # N9 — lone surrogates are structurally unreachable in Rust
//! `encodeURIComponent` throws `URIError` when given a lone (unpaired)
//! UTF-16 surrogate. A Rust `&str` is guaranteed well-formed UTF-8 and can
//! never contain an unpaired surrogate, so this failure mode is
//! structurally unreachable here — documented, not ported as a `Result`.
//!
//! # N10 — the determinism oracle case is vacuous
//! `agent-notification-id.test.ts:5-13` asserts `f(args) === f(args)` for a
//! fixed `args`. Every pure, side-effect-free function trivially satisfies
//! this; it is not counted as coverage for this port.

/// Hand-rolled `encodeURIComponent`, copied locally per this repo's
/// per-module-copy charter (plan §0) from `suaegi_forge::repo_icon`'s
/// private helper of the same name/behavior — not shared cross-crate. The
/// unreserved set is `A-Za-z0-9 - _ . ! ~ * ' ( )`; everything else is
/// percent-encoded byte-by-byte over the UTF-8 encoding, matching how
/// `encodeURIComponent` encodes a UTF-8 byte sequence (N6).
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Port of `buildAgentNotificationId`. Returns `None` unless `worktree_id`
/// and `pane_key` are both present and non-empty (N8 truthiness) and
/// `state_started_at` is a finite number (N8: `Option<f64>`, not `i64`;
/// `Some(0.0)` is accepted). On success, joins `"agent"`, the
/// percent-encoded `worktree_id` and `pane_key` (N6), and
/// `Math.trunc(state_started_at)` rendered as a plain integer string, with
/// `:` separators.
pub fn build_agent_notification_id(
    worktree_id: Option<&str>,
    pane_key: Option<&str>,
    state_started_at: Option<f64>,
) -> Option<String> {
    let worktree_id = worktree_id.filter(|s| !s.is_empty())?;
    let pane_key = pane_key.filter(|s| !s.is_empty())?;
    let state_started_at = state_started_at.filter(|v| v.is_finite())?;

    // Why: `Display` on an `f64` prints an integral value without a
    // fractional part (e.g. `42.0` -> "42"), matching `String(Math.trunc(x))`
    // for every realistic epoch-millisecond timestamp. NOT cast to `i64`:
    // that would silently saturate (Rust's `as i64` clamps rather than
    // panicking) for magnitudes beyond `i64::MAX`, whereas staying in the
    // `f64` domain keeps printing full digits. Neither this nor JS's
    // scientific-notation `ToString` (e.g. `1e+21`) are reproduced for
    // truly extreme magnitudes — real callers never approach that range.
    Some(format!(
        "agent:{}:{}:{}",
        encode_uri_component(worktree_id),
        encode_uri_component(pane_key),
        state_started_at.trunc()
    ))
}

#[cfg(test)]
mod tests {
    use super::build_agent_notification_id;

    // Oracle: agent-notification-id.test.ts

    /// N10: vacuous — every pure function satisfies `f(x) === f(x)`. Kept
    /// only to mirror the oracle 1:1; not counted as real coverage.
    #[test]
    fn builds_a_stable_id_for_the_same_agent_event_metadata() {
        let worktree_id = Some("repo::/Users/me/orca/workspaces/feature");
        let pane_key = Some("tab-1:11111111-1111-4111-8111-111111111111");
        let state_started_at = Some(1_780_000_000_123.0);

        assert_eq!(
            build_agent_notification_id(worktree_id, pane_key, state_started_at),
            build_agent_notification_id(worktree_id, pane_key, state_started_at)
        );
    }

    #[test]
    fn changes_when_the_agent_state_start_time_changes() {
        let worktree_id = Some("repo::/Users/me/orca/workspaces/feature");
        let pane_key = Some("tab-1:11111111-1111-4111-8111-111111111111");

        assert_ne!(
            build_agent_notification_id(worktree_id, pane_key, Some(1_780_000_000_123.0)),
            build_agent_notification_id(worktree_id, pane_key, Some(1_780_000_000_456.0))
        );
    }

    #[test]
    fn returns_none_when_required_fields_are_missing() {
        let pane_key = Some("tab-1:11111111-1111-4111-8111-111111111111");
        let worktree_id = Some("repo::/Users/me/orca/workspaces/feature");

        assert_eq!(build_agent_notification_id(None, pane_key, Some(1_780_000_000_123.0)), None);
        assert_eq!(build_agent_notification_id(worktree_id, None, Some(1_780_000_000_123.0)), None);
        assert_eq!(build_agent_notification_id(worktree_id, pane_key, None), None);
    }

    // Mandatory extra pins (oracle-silent — plan §5, N6/N7):

    /// N7: pin the literal id shape directly — the oracle never asserts a
    /// literal string, so a regression in the format is otherwise invisible.
    #[test]
    fn pin_literal_id_shape() {
        assert_eq!(
            build_agent_notification_id(Some("repo"), Some("tab-1:leaf-1"), Some(42.0)),
            Some("agent:repo:tab-1%3Aleaf-1:42".to_string())
        );
    }

    /// N6/N7 crux pin: without percent-encoding, `worktree_id = "a"` +
    /// `pane_key = "b:c"` and `worktree_id = "a:b"` + `pane_key = "c"` would
    /// join to the identical string `"agent:a:b:c:<ts>"` — a real collision.
    /// Percent-encoding the `:` in each field makes the two ids distinct.
    #[test]
    fn pin_colon_collision_requires_percent_encoding() {
        let id_1 = build_agent_notification_id(Some("a"), Some("b:c"), Some(7.0));
        let id_2 = build_agent_notification_id(Some("a:b"), Some("c"), Some(7.0));

        assert_ne!(id_1, id_2);
        assert_eq!(id_1, Some("agent:a:b%3Ac:7".to_string()));
        assert_eq!(id_2, Some("agent:a%3Ab:c:7".to_string()));
    }

    /// N8: `stateStartedAt === 0` is accepted (typeof check runs before
    /// truthiness) — must NOT be treated as "missing".
    #[test]
    fn pin_zero_state_started_at_is_accepted() {
        assert_eq!(
            build_agent_notification_id(Some("repo"), Some("pane"), Some(0.0)),
            Some("agent:repo:pane:0".to_string())
        );
    }

    /// N8: empty-string `worktree_id`/`pane_key` are rejected by
    /// truthiness, exactly like `None` — not merely a type/`Option` check.
    #[test]
    fn pin_empty_string_fields_are_rejected() {
        assert_eq!(build_agent_notification_id(Some(""), Some("pane"), Some(1.0)), None);
        assert_eq!(build_agent_notification_id(Some("repo"), Some(""), Some(1.0)), None);
    }

    /// N8 crux pin: a magnitude beyond `i64::MAX` distinguishes staying in
    /// the `f64` domain (full digit expansion) from an internal `as i64`
    /// cast (silently saturates to `i64::MAX` in Rust, producing a wrong,
    /// constant id for every sufficiently large timestamp).
    #[test]
    fn pin_huge_magnitude_does_not_saturate_via_i64_cast() {
        assert_eq!(
            build_agent_notification_id(Some("repo"), Some("pane"), Some(1e20)),
            Some("agent:repo:pane:100000000000000000000".to_string())
        );
    }

    /// N8: `Math.trunc` drops the fractional part (never rounds), and a
    /// large-magnitude value truncates as an ordinary integer, not `NaN`
    /// (`Number.isFinite` guard rejects non-finite values before this
    /// point).
    #[test]
    fn pin_fractional_and_large_values_are_trunc_and_finite_checked() {
        assert_eq!(
            build_agent_notification_id(Some("repo"), Some("pane"), Some(1_780_000_000_999.9)),
            Some("agent:repo:pane:1780000000999".to_string())
        );
        assert_eq!(
            build_agent_notification_id(Some("repo"), Some("pane"), Some(f64::NAN)),
            None
        );
        assert_eq!(
            build_agent_notification_id(Some("repo"), Some("pane"), Some(f64::INFINITY)),
            None
        );
    }
}
