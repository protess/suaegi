//! Legacy base-ref search result derivation — verbatim port of Orca's
//! `src/shared/base-ref-search-result.ts` (@ v1.4.146-rc.0).
//!
//! Mixed-version runtimes only return display refs (e.g. `origin/main`), not
//! separate local-branch names. [`derive_legacy_local_branch_name`] strips a
//! known remote-tracking prefix so callers don't reintroduce
//! `origin/feature/foo` as the local branch name; [`legacy_base_ref_search_result`]
//! packages that into the same [`BaseRefSearchResult`] shape newer runtimes
//! return directly (`types.ts:418-421`).
//!
//! The `startsWith(prefix) && length > prefix.length` guard in the original
//! only changes behavior when `ref_name` is **exactly equal** to the prefix
//! (e.g. `"origin/"`): `str::strip_prefix` alone would return `Some("")` there,
//! but the guard means the loop instead falls through to the next prefix (and
//! eventually returns `ref_name` unchanged) rather than yielding an empty
//! branch name. The upstream oracle never exercises this exact-prefix input,
//! so a bare `strip_prefix` port passes the ported test suite; see the
//! `pin_g6_*` test below for the divergence.

/// Port of the `BaseRefSearchResult` type (`types.ts:418-421`). Both fields
/// are non-optional strings in the original; no `serde` derive is added here
/// (this module never serializes it) — see the `suaegi-workspace-cleanup`
/// precedent for deferring that to an optional feature only once a caller
/// actually crosses a process boundary with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRefSearchResult {
    pub ref_name: String,
    pub local_branch_name: String,
}

/// Prefix candidates, tried in order — first match wins (`:3`). The oracle
/// never asserts this list's contents directly (it's a module-private
/// implementation detail in the TS source), only its effect, so the value is
/// pinned literally below (`pin_g7_*`).
const LEGACY_REMOTE_REF_PREFIXES: [&str; 2] = ["origin/", "upstream/"];

/// Port of `deriveLegacyLocalBranchName` (`base-ref-search-result.ts:5-14`).
///
/// Strips the first matching prefix from [`LEGACY_REMOTE_REF_PREFIXES`], but
/// only when doing so leaves a **non-empty** remainder — `ref_name` equal to
/// a bare prefix (e.g. `"origin/"`) is returned unchanged, not emptied.
pub fn derive_legacy_local_branch_name(ref_name: &str) -> String {
    for prefix in LEGACY_REMOTE_REF_PREFIXES {
        if let Some(rest) = ref_name.strip_prefix(prefix) {
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    ref_name.to_string()
}

/// Port of `legacyBaseRefSearchResult` (`base-ref-search-result.ts:16-21`).
///
/// This function is the wiring the upstream oracle leaves unverified: its
/// only fixture (`test:11-14`) is an identity case where `ref_name` has no
/// known remote prefix, so a port that skipped calling
/// [`derive_legacy_local_branch_name`] entirely (and just copied `ref_name`
/// into both fields) would still pass. See `pin_g8_*` below for the witness
/// that actually exercises the wiring.
pub fn legacy_base_ref_search_result(ref_name: &str) -> BaseRefSearchResult {
    BaseRefSearchResult {
        ref_name: ref_name.to_string(),
        local_branch_name: derive_legacy_local_branch_name(ref_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: base-ref-search-result.test.ts

    #[test]
    fn derives_local_branch_names_for_common_remote_refs_returned_by_older_runtimes() {
        assert_eq!(
            derive_legacy_local_branch_name("origin/feature/something"),
            "feature/something"
        );
        assert_eq!(
            derive_legacy_local_branch_name("upstream/release/1.2"),
            "release/1.2"
        );
    }

    #[test]
    fn keeps_local_branch_refs_unchanged_when_a_remote_prefix_is_not_known() {
        assert_eq!(
            legacy_base_ref_search_result("feature/something"),
            BaseRefSearchResult {
                ref_name: "feature/something".to_string(),
                local_branch_name: "feature/something".to_string(),
            }
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// G6: `ref_name` exactly equal to a prefix is returned unchanged, NOT
    /// emptied — the `length > prefix.length` guard's only observable effect.
    #[test]
    fn pin_g6_exact_prefix_input_is_returned_unchanged_not_emptied() {
        assert_eq!(derive_legacy_local_branch_name("origin/"), "origin/");
        assert_eq!(derive_legacy_local_branch_name("upstream/"), "upstream/");
    }

    /// G7: the prefix list's literal contents and order — the oracle never
    /// asserts either, so this is the only thing pinning them.
    #[test]
    fn pin_g7_prefix_list_literal_and_order() {
        assert_eq!(LEGACY_REMOTE_REF_PREFIXES, ["origin/", "upstream/"]);
    }

    /// G8: `legacy_base_ref_search_result` must actually call
    /// `derive_legacy_local_branch_name` — the only upstream fixture is an
    /// identity case that a skipped-wiring port would also satisfy.
    #[test]
    fn pin_g8_legacy_result_wires_through_the_derive_function() {
        assert_eq!(
            legacy_base_ref_search_result("origin/main"),
            BaseRefSearchResult {
                ref_name: "origin/main".to_string(),
                local_branch_name: "main".to_string(),
            }
        );
    }
}
