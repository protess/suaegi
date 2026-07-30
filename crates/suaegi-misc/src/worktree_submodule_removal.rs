//! Submodule worktree-removal refusal detection — verbatim port of Orca's
//! `src/shared/worktree-submodule-removal.ts` (@ v1.4.146-rc.0).
//!
//! Why: `git worktree remove` (non-force) categorically refuses any worktree
//! containing an initialised submodule, even when parent and submodule are
//! fully clean (`validate_no_submodules`, Git >= 2.17). Callers re-prove
//! cleanliness and retry with `--force`. Both the local runner and the relay
//! pin English git output (`UNTRANSLATED_GIT_OUTPUT_ENV`), so text matching
//! is stable.
//!
//! # ASCII-only case folding (H1)
//! The source regex is `/working trees containing submodules cannot be moved
//! or removed/i` — unanchored substring, no `u` flag, one literal phrase. Both
//! oracle fixtures are all-lowercase, so the `/i` flag is never actually
//! exercised there. It matters anyway: JS `/i` without `/u` folds **ASCII
//! only**, while Rust's `str::to_lowercase` folds full Unicode. The phrase
//! contains a `k` (in "working"), so an input with U+212A KELVIN SIGN in
//! place of that `k` is JS `false` but would be `to_lowercase` `true` — a
//! false positive that would make a caller retry `git worktree remove
//! --force` on a worktree refused for an unrelated reason. Use
//! `to_ascii_lowercase`, never `to_lowercase`.
//!
//! # `unknown` input model (H2)
//! JS's `String(error)` (`:12`) is **not** ported: `String(null) →
//! "null"`, `String(1e21) → "1e+21"` (ECMAScript float formatting),
//! `String(fn)` → source text, `String(Symbol('x')) → "Symbol(x)"` — none of
//! this has a faithful Rust analog. [`GitErrorLike::Primitive`] is supplied by
//! the caller instead; recovering it from an arbitrary error value is the
//! caller's responsibility (mirrors `remote_runtime_error`'s treatment of
//! `toRemoteRuntimeClientErrorLike`). Also note: in JS an array is
//! `typeof === 'object'`, so it would fall into the object branch (and yield
//! `""`, having none of the three named fields) — there is no array variant
//! here because [`GitErrorFields`] only carries the three consumed fields.
//!
//! # Field order and join (H3)
//! Fields are read in **exactly** `["message", "stderr", "stdout"]` order and
//! joined with `'\n'` (`:4`, `:10`). The final match is an unanchored
//! substring, so the public predicate alone can't distinguish this join from
//! `join("")`, `join(" ")`, or "return the first matching field" — all of
//! those would also pass the oracle. [`get_error_text`] is `pub` specifically
//! so the order and separator can be pinned directly against it.
//!
//! # Truthiness filter (H4)
//! `typeof value === 'string' && value` (`:6`) drops an empty string via
//! truthiness, not just a type check — modeled as `.filter(|v|
//! !v.is_empty())`.
//!
//! # Fields NOT read (H6)
//! Only `message`, `stderr`, `stdout` are consumed. `stack`, `cause`, `code`,
//! and `output` are never read.

/// The three fields read off an object-like error, in the field-check order
/// `["message", "stderr", "stdout"]` (H3, H6). Mirrors the TS `unknown`
/// indexing `(error as Record<string, unknown>)[field]`, restricted up front
/// to the three fields actually consumed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GitErrorFields<'a> {
    pub message: Option<&'a str>,
    pub stderr: Option<&'a str>,
    pub stdout: Option<&'a str>,
}

/// Caller-supplied model of the TS `unknown` error input. `ObjectLike` covers
/// the `typeof error === 'object' && error !== null` branch (`:2`);
/// `Primitive` covers everything else, which JS stringifies via `String(error)`
/// (H2, out of scope here — the caller must produce the string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitErrorLike<'a> {
    ObjectLike(GitErrorFields<'a>),
    Primitive(&'a str),
}

/// Port of `getErrorText`. For `ObjectLike`, joins the non-empty string
/// fields — in `message`, `stderr`, `stdout` order (H3) — with `'\n'`,
/// dropping empty strings by truthiness (H4). For `Primitive`, returns the
/// caller-supplied string verbatim (H2: no `String(error)` coercion is
/// performed here).
pub fn get_error_text(error: &GitErrorLike<'_>) -> String {
    match error {
        GitErrorLike::ObjectLike(fields) => [fields.message, fields.stderr, fields.stdout]
            .into_iter()
            .flatten()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        GitErrorLike::Primitive(s) => (*s).to_string(),
    }
}

/// Port of `isSubmoduleWorktreeRemovalRefusal`: true when [`get_error_text`]
/// contains, as an ASCII-case-insensitive unanchored substring, "working
/// trees containing submodules cannot be moved or removed" (H1).
pub fn is_submodule_worktree_removal_refusal(error: &GitErrorLike<'_>) -> bool {
    get_error_text(error)
        .to_ascii_lowercase()
        .contains("working trees containing submodules cannot be moved or removed")
}

#[cfg(test)]
mod tests {
    use super::{
        get_error_text, is_submodule_worktree_removal_refusal, GitErrorFields, GitErrorLike,
    };

    // Oracle: worktree-submodule-removal.test.ts

    #[test]
    fn matches_the_english_git_fatal_on_stderr() {
        let error = GitErrorLike::ObjectLike(GitErrorFields {
            message: Some("git worktree remove failed"),
            stderr: Some("fatal: working trees containing submodules cannot be moved or removed\n"),
            stdout: None,
        });
        assert!(is_submodule_worktree_removal_refusal(&error));
    }

    #[test]
    fn matches_when_the_refusal_is_only_in_the_error_message() {
        let error = GitErrorLike::ObjectLike(GitErrorFields {
            message: Some("fatal: working trees containing submodules cannot be moved or removed"),
            stderr: None,
            stdout: None,
        });
        assert!(is_submodule_worktree_removal_refusal(&error));
    }

    #[test]
    fn does_not_match_dirty_worktree_or_lock_refusals() {
        let dirty = GitErrorLike::ObjectLike(GitErrorFields {
            message: Some("git worktree remove failed"),
            stderr: Some("fatal: contains modified or untracked files, use --force to delete it"),
            stdout: None,
        });
        assert!(!is_submodule_worktree_removal_refusal(&dirty));

        let locked = GitErrorLike::ObjectLike(GitErrorFields {
            message: Some("git worktree remove failed"),
            stderr: Some("fatal: cannot remove a locked working tree"),
            stdout: None,
        });
        assert!(!is_submodule_worktree_removal_refusal(&locked));
    }

    // Mandatory extra pins (oracle-silent):

    /// H1 crux pin #1: an uppercase phrase must still match — kills a mutant
    /// that drops the ASCII case fold entirely (plain `contains`).
    #[test]
    fn pin_uppercase_phrase_matches() {
        let error = GitErrorLike::ObjectLike(GitErrorFields {
            message: None,
            stderr: Some("FATAL: WORKING TREES CONTAINING SUBMODULES CANNOT BE MOVED OR REMOVED"),
            stdout: None,
        });
        assert!(is_submodule_worktree_removal_refusal(&error));
    }

    /// H1 crux pin #2: U+212A KELVIN SIGN in place of the `k` in "working"
    /// must NOT fold to ASCII `k` — kills a mutant that uses full Unicode
    /// `str::to_lowercase` instead of `to_ascii_lowercase`.
    #[test]
    fn pin_kelvin_sign_does_not_fold_to_ascii_k() {
        // Precondition: real Rust `to_lowercase` WOULD fold U+212A to 'k',
        // proving the divergence this pin guards against.
        assert_eq!('\u{212A}'.to_lowercase().collect::<String>(), "k");
        let error = GitErrorLike::ObjectLike(GitErrorFields {
            message: None,
            stderr: Some(
                "fatal: wor\u{212A}ing trees containing submodules cannot be moved or removed",
            ),
            stdout: None,
        });
        assert!(!is_submodule_worktree_removal_refusal(&error));
    }

    /// H3: field order is exactly message, stderr, stdout, joined with '\n' —
    /// pinned directly against the public `get_error_text` since the refusal
    /// predicate can't distinguish it from other joins/orders.
    #[test]
    fn pin_get_error_text_field_order_and_separator() {
        let error = GitErrorLike::ObjectLike(GitErrorFields {
            message: Some("msg"),
            stderr: Some("err"),
            stdout: Some("out"),
        });
        assert_eq!(get_error_text(&error), "msg\nerr\nout");
    }

    /// H4: an empty `message` is dropped by truthiness, not joined as a
    /// leading empty line.
    #[test]
    fn pin_empty_message_is_excluded() {
        let error = GitErrorLike::ObjectLike(GitErrorFields {
            message: Some(""),
            stderr: Some("err"),
            stdout: None,
        });
        assert_eq!(get_error_text(&error), "err");
    }

    /// H5: `stdout` is read and can match on its own — the oracle has no
    /// `stdout` fixture at all.
    #[test]
    fn pin_stdout_only_match() {
        let error = GitErrorLike::ObjectLike(GitErrorFields {
            message: None,
            stderr: None,
            stdout: Some("working trees containing submodules cannot be moved or removed"),
        });
        assert!(is_submodule_worktree_removal_refusal(&error));
    }

    /// H2: the `Primitive` branch returns the caller-supplied string verbatim
    /// (no `String(error)` coercion), and a field-less `ObjectLike` yields
    /// `""`.
    #[test]
    fn pin_primitive_branch_and_empty_object_like() {
        let primitive = GitErrorLike::Primitive("plain string error");
        assert_eq!(get_error_text(&primitive), "plain string error");
        assert!(!is_submodule_worktree_removal_refusal(&primitive));

        let empty_object = GitErrorLike::ObjectLike(GitErrorFields::default());
        assert_eq!(get_error_text(&empty_object), "");
    }
}
