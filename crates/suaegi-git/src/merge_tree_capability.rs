//! Port of Orca `shared/git-merge-tree-capability.ts` (@ v1.4.150-rc.0).
//!
//! Detects whether a git error indicates an OLD git that does not support the
//! `merge-tree --write-tree` / `--merge-base` options — by matching the ERROR
//! TEXT (message/stderr/stdout), not by parsing git versions.

use regex::Regex;
use std::sync::LazyLock;

/// A git command error, mirroring the object fields Orca's `getGitErrorText`
/// reads. A plain string message (or a JS `Error.message`) maps to `message`.
#[derive(Clone, Copy, Debug, Default)]
pub struct GitCommandError<'a> {
    pub message: Option<&'a str>,
    pub stderr: Option<&'a str>,
    pub stdout: Option<&'a str>,
}

impl<'a> GitCommandError<'a> {
    /// Convenience: an error carrying only a message.
    pub fn message(message: &'a str) -> Self {
        Self {
            message: Some(message),
            ..Self::default()
        }
    }
    /// Convenience: an error carrying only stderr.
    pub fn stderr(stderr: &'a str) -> Self {
        Self {
            stderr: Some(stderr),
            ..Self::default()
        }
    }
    /// Convenience: an error carrying only stdout.
    pub fn stdout(stdout: &'a str) -> Self {
        Self {
            stdout: Some(stdout),
            ..Self::default()
        }
    }
}

/// Join the present message/stderr/stdout fields with `\n`, in that order.
fn git_error_text(error: &GitCommandError) -> String {
    [error.message, error.stderr, error.stdout]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}

// `/i`, Unicode `\s`; `.test()` is unanchored (== `is_match`). Both backtick and
// apostrophe are accepted independently on each side.
static WRITE_TREE_OPTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:unknown|invalid|unrecognized) option(?::|\s+)[`']?(?:--?)?write-tree[`']?(?:\s|$)",
    )
    .unwrap()
});
static WRITE_TREE_UNKNOWN_REV_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)unknown rev [`']?--write-tree[`']?(?:\s|$)").unwrap());
static WRITE_TREE_USAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)usage:\s*git merge-tree\s+<base-tree>\s+<branch1>\s+<branch2>").unwrap()
});
static MERGE_BASE_OPTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:unknown|invalid|unrecognized) option(?::|\s+)[`']?(?:--?)?merge-base[`']?(?:\s|$)",
    )
    .unwrap()
});

/// True when the error indicates `git merge-tree --write-tree` is unsupported.
pub fn is_unsupported_merge_tree_write_tree_error(error: &GitCommandError) -> bool {
    let output = git_error_text(error);
    WRITE_TREE_OPTION_RE.is_match(&output)
        || WRITE_TREE_UNKNOWN_REV_RE.is_match(&output)
        || WRITE_TREE_USAGE_RE.is_match(&output)
}

/// True when the error indicates `git merge-tree --merge-base` is unsupported.
pub fn is_unsupported_merge_tree_merge_base_error(error: &GitCommandError) -> bool {
    MERGE_BASE_OPTION_RE.is_match(&git_error_text(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- git-merge-tree-capability.test.ts oracle ---

    #[test]
    fn detects_unsupported_write_tree_in_all_three_forms() {
        // unknown rev form (stderr).
        assert!(is_unsupported_merge_tree_write_tree_error(
            &GitCommandError::stderr("fatal: unknown rev --write-tree")
        ));
        // usage form (stdout).
        assert!(is_unsupported_merge_tree_write_tree_error(
            &GitCommandError::stdout("usage: git merge-tree <base-tree> <branch1> <branch2>")
        ));
        // unknown option form (message, apostrophe-quoted).
        assert!(is_unsupported_merge_tree_write_tree_error(
            &GitCommandError::message("error: unknown option 'write-tree'")
        ));
    }

    #[test]
    fn does_not_flag_an_ordinary_merge_failure() {
        assert!(!is_unsupported_merge_tree_write_tree_error(
            &GitCommandError::stderr("fatal: refusing to merge unrelated histories")
        ));
    }

    #[test]
    fn detects_unsupported_merge_base_and_ignores_ordinary_failure() {
        // Backtick-quoted option.
        assert!(is_unsupported_merge_tree_merge_base_error(
            &GitCommandError::stderr("error: unknown option `merge-base'")
        ));
        assert!(!is_unsupported_merge_tree_merge_base_error(
            &GitCommandError::stderr("fatal: merge-base failed")
        ));
    }

    // --- extra pins ---

    #[test]
    fn reads_all_three_fields_joined() {
        // The matching text is only in stdout; message/stderr are unrelated.
        let err = GitCommandError {
            message: Some("boom"),
            stderr: Some("noise"),
            stdout: Some("error: unrecognized option --write-tree"),
        };
        assert!(is_unsupported_merge_tree_write_tree_error(&err));
    }

    #[test]
    fn write_tree_matcher_does_not_match_merge_base_and_vice_versa() {
        assert!(!is_unsupported_merge_tree_write_tree_error(
            &GitCommandError::stderr("error: unknown option `merge-base'")
        ));
        assert!(!is_unsupported_merge_tree_merge_base_error(
            &GitCommandError::stderr("error: unknown option 'write-tree'")
        ));
    }
}
