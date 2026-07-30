//! ⚠ P1 — security. Hook-command source policy normalization — verbatim port
//! of Orca's `src/shared/hook-command-source-policy.ts` (@ v1.4.146-rc.0).
//! The `import type { HookCommandSourcePolicy } from './types'` becomes a
//! module-local `enum` per [`suaegi-misc-placement-rule`] — no shared types
//! module, no cross-module import.
//!
//! **Dropping the `'local-only'` arm ships green and flips a user's explicit
//! opt-out into running a repo's committed script.** No oracle fixture ever
//! passes the string `'local-only'` in — the only `'local-only'` result
//! comes from the `undefined` + `has_local_script` default branch
//! (`resolve(undefined, { hasLocalScript: true }) -> 'local-only'`). So a
//! port that drops `'local-only'` from the explicit-choice membership chain
//! still passes every oracle case, while `resolve(Some("local-only"), _)`
//! silently falls through past the removed arm to `'shared-only'` —
//! converting "don't run this repo's committed script" into running it
//! (checked-out repo's `orca.yaml` `scripts.setup` / `defaultTabs[].command`
//! execute arbitrarily, per `main/hooks.ts:210-218,305-311`). The pins below
//! (`resolve(Some("local-only"), false/true) -> LocalOnly`,
//! `normalize(Some("local-only")) -> LocalOnly`) exist specifically to catch
//! that regression.
//!
//! **`undefined` and `null` are not interchangeable** — verified directly in
//! the source: `resolve`'s default-branch guard is `policy === undefined &&
//! hasLocalScript`, a strict-equality check against `undefined` specifically,
//! not a nullish/falsy check. `resolve(null, hasLocalScript=true)` is
//! `'shared-only'`; `resolve(undefined, true)` is `'local-only'`. A naive
//! `Option<&str>` port folds both into `None` and gets `null` wrong — this is
//! reachable, since `new-workspace.ts:166` types its input as
//! `commandSourcePolicy?: unknown` and round-trips persisted JSON (where an
//! absent key deserializes as `undefined` but an explicit JSON `null` is
//! `null`, a real and distinct wire value). Modeled as a **three-state**
//! `Option<Option<&str>>`: `None` = `undefined`, `Some(None)` = `null` or any
//! non-string, `Some(Some(s))` = a string.
//!
//! No fixture ever exercises trimming or case-folding, and this module is
//! the one place in the crate where over-normalizing is dangerous rather
//! than merely wrong: `' run-both '` or `'Run-Both'` accepted as `'run-both'`
//! would start running a repo's committed script under a laxer match than
//! upstream ever performs. Membership is exact-string only.

/// The closed set of hook-command source policies. Membership is exact — no
/// trim/case-fold belongs here (see module doc: loosening this one is a
/// security regression, not just an oracle miss).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookCommandSourcePolicy {
    SharedOnly,
    LocalOnly,
    RunBoth,
}

/// `policy === 'local-only' || policy === 'run-both' || policy === 'shared-only'
/// ? policy : 'shared-only'`. Unlike [`resolve_hook_command_source_policy`],
/// `normalize` has no `undefined`-specific branch, so the `Option<Option<&str>>`
/// three-state input collapses identically for `None` and `Some(None)` here —
/// both fall to `SharedOnly`. The `'shared-only'` arm is genuinely dead (its
/// value equals the fallback, so removing it changes no input's behavior);
/// kept verbatim per F12 so a later simplify pass doesn't delete it and this
/// module's contract silently drifts from the two-function pair in the source.
pub fn normalize_hook_command_source_policy(
    policy: Option<Option<&str>>,
) -> HookCommandSourcePolicy {
    match policy {
        Some(Some("local-only")) => HookCommandSourcePolicy::LocalOnly,
        Some(Some("run-both")) => HookCommandSourcePolicy::RunBoth,
        Some(Some("shared-only")) => HookCommandSourcePolicy::SharedOnly,
        _ => HookCommandSourcePolicy::SharedOnly,
    }
}

/// Explicit choices (`'local-only'`/`'run-both'`/`'shared-only'`) pass through
/// unchanged. Otherwise: `policy === undefined && hasLocalScript` (strict
/// equality against `undefined` specifically, i.e. `policy.is_none()` here,
/// **not** `Some(None)`/null) resolves to `'local-only'`; everything else
/// (including explicit `null`) falls to `'shared-only'`. See module doc for
/// why the `undefined`/`null` distinction and the `'local-only'` arm are both
/// security-load-bearing.
pub fn resolve_hook_command_source_policy(
    policy: Option<Option<&str>>,
    has_local_script: bool,
) -> HookCommandSourcePolicy {
    match policy {
        Some(Some("local-only")) => return HookCommandSourcePolicy::LocalOnly,
        Some(Some("run-both")) => return HookCommandSourcePolicy::RunBoth,
        Some(Some("shared-only")) => return HookCommandSourcePolicy::SharedOnly,
        _ => {}
    }

    if policy.is_none() && has_local_script {
        return HookCommandSourcePolicy::LocalOnly;
    }

    HookCommandSourcePolicy::SharedOnly
}

#[cfg(test)]
mod tests {
    use super::HookCommandSourcePolicy::{LocalOnly, RunBoth, SharedOnly};
    use super::*;

    // Oracle: hook-command-source-policy.test.ts

    #[test]
    fn normalizes_unknown_persisted_policies_to_shared_only() {
        assert_eq!(
            normalize_hook_command_source_policy(Some(Some("shared-first"))),
            SharedOnly
        );
    }

    #[test]
    fn uses_local_commands_by_default_when_a_local_script_is_configured() {
        assert_eq!(resolve_hook_command_source_policy(None, true), LocalOnly);
    }

    #[test]
    fn uses_shared_commands_by_default_when_no_local_script_is_configured() {
        assert_eq!(resolve_hook_command_source_policy(None, false), SharedOnly);
    }

    #[test]
    fn preserves_explicit_command_source_choices() {
        assert_eq!(
            resolve_hook_command_source_policy(Some(Some("shared-only")), true),
            SharedOnly
        );
        assert_eq!(
            resolve_hook_command_source_policy(Some(Some("run-both")), true),
            RunBoth
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// F9 — the highest-leverage pins in this module: no oracle fixture ever
    /// passes the string `'local-only'` in, so a port that drops it from the
    /// membership chain still passes every case above while silently
    /// converting an explicit opt-out into running the local script anyway.
    #[test]
    fn pin_explicit_local_only_is_preserved() {
        assert_eq!(
            resolve_hook_command_source_policy(Some(Some("local-only")), false),
            LocalOnly
        );
        assert_eq!(
            resolve_hook_command_source_policy(Some(Some("local-only")), true),
            LocalOnly
        );
        assert_eq!(
            normalize_hook_command_source_policy(Some(Some("local-only"))),
            LocalOnly
        );
    }

    /// F10 — `undefined` (`None`) and `null`/non-string (`Some(None)`) are
    /// not interchangeable: only strict `undefined` combines with
    /// `has_local_script` to produce `LocalOnly`.
    #[test]
    fn pin_undefined_and_null_are_not_interchangeable() {
        assert_eq!(
            resolve_hook_command_source_policy(Some(None), true),
            SharedOnly
        );
        assert_eq!(resolve_hook_command_source_policy(None, true), LocalOnly);
    }

    /// F11 — each policy member pinned directly, not just accepted-vs-fallback.
    #[test]
    fn pin_each_member_directly() {
        assert_eq!(
            normalize_hook_command_source_policy(Some(Some("shared-only"))),
            SharedOnly
        );
        assert_eq!(
            normalize_hook_command_source_policy(Some(Some("local-only"))),
            LocalOnly
        );
        assert_eq!(
            normalize_hook_command_source_policy(Some(Some("run-both"))),
            RunBoth
        );
    }

    /// F13 — no trim/case-fold: looser matching here is a security regression
    /// (a laxly-matched policy could start a repo's committed script running
    /// under input the source would have rejected to the safe fallback).
    #[test]
    fn pin_no_trim_or_case_fold() {
        assert_eq!(
            normalize_hook_command_source_policy(Some(Some("Shared-Only"))),
            SharedOnly
        );
        assert_eq!(
            normalize_hook_command_source_policy(Some(Some(" local-only "))),
            SharedOnly
        );
        assert_eq!(
            resolve_hook_command_source_policy(Some(Some(" run-both ")), true),
            SharedOnly
        );
    }
}
