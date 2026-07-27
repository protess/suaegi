//! `git worktree add` base-ref resolution — verbatim port of Orca's
//! `src/shared/worktree-base-ref.ts` (@ v1.4.146-rc.0).
//!
//! `git worktree add` receives a revision, so a short name like `main` can
//! collide with a tag. This picks the namespace implied by Orca's base
//! picker — remote display names like `origin/main` first, otherwise local
//! branches — probing candidates in order and returning the first one that
//! exists.
//!
//! **S1 (injected callback, extending the `clipboard_text.rs` pattern to a
//! callback with both an argument and a return value):** the original
//! `refExists: (qualifiedRef: string) => Promise<boolean>` is async; this
//! crate has no runtime and adds no dependency, so it becomes a synchronous
//! `ref_exists: &mut dyn FnMut(&str) -> bool` injected by the caller.
//! `&mut dyn FnMut` (not `Fn`) is required so a test spy can own a `Vec<String>`
//! recording call order — the only way to pin the short-circuiting (G10) and
//! candidate-order (G11) behavior below.
//!
//! **Deliberate divergence:** the TS version propagates a rejected
//! `refExists` promise (there is no `try/catch` around the `await`). The
//! `bool`-returning callback form here drops that contract — a probe that
//! "fails" can only report `false`, not a distinct error. This is intentional:
//! all six production call sites already collapse a thrown/rejected probe to
//! `false` before this function would see it, so a `Result`-based signature
//! would have zero real users and would infect every caller with a type
//! parameter for no observable gain.
//!
//! **Short-circuiting and candidate order are both effectively unpinned by
//! the upstream oracle** — a port that probes every candidate before picking
//! the first success, or that probes candidates in the wrong order, still
//! passes every fixture in `worktree-base-ref.test.ts` (see the two
//! oracle-silent pins below for the only inputs that actually separate this
//! implementation's behavior).

/// Port of `resolveWorktreeAddBaseRef` (`worktree-base-ref.ts:3-25`).
///
/// Three branches:
/// 1. `base_ref` already starts with `refs/` (a fully-qualified ref, or a
///    hosted-provider review ref like `refs/pull/123/head`): returned as-is,
///    **without calling `ref_exists` at all**.
/// 2. `base_ref` contains `/`: probes `refs/remotes/<base_ref>` then
///    `refs/heads/<base_ref>`, in that order, returning the first that exists.
/// 3. `base_ref` has no `/`: probes only `refs/heads/<base_ref>`.
///
/// If no candidate exists, `base_ref` is returned unchanged.
pub fn resolve_worktree_add_base_ref(
    base_ref: &str,
    ref_exists: &mut dyn FnMut(&str) -> bool,
) -> String {
    if base_ref.starts_with("refs/") {
        return base_ref.to_string();
    }

    let candidates: Vec<String> = if base_ref.contains('/') {
        vec![
            format!("refs/remotes/{base_ref}"),
            format!("refs/heads/{base_ref}"),
        ]
    } else {
        vec![format!("refs/heads/{base_ref}")]
    };

    for candidate in candidates {
        if ref_exists(&candidate) {
            return candidate;
        }
    }

    base_ref.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: worktree-base-ref.test.ts

    #[test]
    fn leaves_fully_qualified_refs_unchanged() {
        let mut calls: Vec<String> = Vec::new();
        let mut ref_exists = |r: &str| {
            calls.push(r.to_string());
            false
        };

        assert_eq!(
            resolve_worktree_add_base_ref("refs/heads/main", &mut ref_exists),
            "refs/heads/main"
        );
        assert!(calls.is_empty());
    }

    /// `:15-26` in the oracle is byte-identical in code path to `:5-13`
    /// (both hit the same `refs/` early return) — ported for provider-compat
    /// documentation intent, but not counted as extra branch coverage.
    #[test]
    fn leaves_provider_review_refs_unchanged() {
        let mut calls: Vec<String> = Vec::new();
        let mut ref_exists = |r: &str| {
            calls.push(r.to_string());
            false
        };

        assert_eq!(
            resolve_worktree_add_base_ref("refs/pull/123/head", &mut ref_exists),
            "refs/pull/123/head"
        );
        assert_eq!(
            resolve_worktree_add_base_ref("refs/merge-requests/456/head", &mut ref_exists),
            "refs/merge-requests/456/head"
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn qualifies_a_bare_local_branch_name() {
        let mut ref_exists = |r: &str| r == "refs/heads/main";

        assert_eq!(
            resolve_worktree_add_base_ref("main", &mut ref_exists),
            "refs/heads/main"
        );
    }

    #[test]
    fn prefers_a_remote_tracking_ref_for_remote_display_names() {
        let mut calls: Vec<String> = Vec::new();
        let mut ref_exists = |r: &str| {
            calls.push(r.to_string());
            r == "refs/remotes/origin/main"
        };

        assert_eq!(
            resolve_worktree_add_base_ref("origin/main", &mut ref_exists),
            "refs/remotes/origin/main"
        );

        // G10: the first candidate already exists, so the loop must
        // short-circuit — a probe-everything port would call this twice.
        assert_eq!(calls.len(), 1);
        assert_eq!(calls, vec!["refs/remotes/origin/main"]);
    }

    #[test]
    fn qualifies_a_slash_containing_local_branch_when_no_matching_remote_ref_exists() {
        let mut calls: Vec<String> = Vec::new();
        let mut ref_exists = |r: &str| {
            calls.push(r.to_string());
            r == "refs/heads/release/main"
        };

        assert_eq!(
            resolve_worktree_add_base_ref("release/main", &mut ref_exists),
            "refs/heads/release/main"
        );

        // G11: remotes-before-heads is only pinned by an exact call array on
        // a fixture where the first (remotes) candidate fails.
        assert_eq!(
            calls,
            vec!["refs/remotes/release/main", "refs/heads/release/main"]
        );
    }

    #[test]
    fn keeps_unresolvable_revisions_untouched() {
        let mut ref_exists = |_: &str| false;

        assert_eq!(
            resolve_worktree_add_base_ref("abc1234", &mut ref_exists),
            "abc1234"
        );
    }

    // Mandatory extra pin (oracle-silent):

    /// G12: the no-slash branch probes exactly ONE candidate
    /// (`refs/heads/<base_ref>`), never `refs/remotes/<base_ref>` — the
    /// oracle's no-slash fixtures (`main`, `abc1234`) never assert the call
    /// array, only the return value, so a two-candidate port for this branch
    /// would still pass every oracle case.
    #[test]
    fn pin_g12_bare_name_without_slash_probes_only_refs_heads() {
        let mut calls: Vec<String> = Vec::new();
        let mut ref_exists = |r: &str| {
            calls.push(r.to_string());
            false
        };

        assert_eq!(
            resolve_worktree_add_base_ref("main", &mut ref_exists),
            "main"
        );
        assert_eq!(calls, vec!["refs/heads/main"]);
    }
}
