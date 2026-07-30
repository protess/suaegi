//! Port of Orca `shared/git-status-limit.ts` (@ v1.4.150-rc.0).
//!
//! Caps the number of git-status entries so a repo with an enormous un-ignored
//! folder cannot freeze the source-control view. `limit == 0` means unlimited.
//! A "hit the limit" signal is sticky across passes.

/// Default status entry cap (native, WSL, SSH, and renderer agree on this).
pub const DEFAULT_GIT_STATUS_LIMIT: i64 = 1_000;

/// Resolve a persisted status-limit setting. `value` is modeled as `Option<f64>`
/// (JS `unknown`): only a finite non-negative integer is accepted; anything else
/// (None / non-integer / NaN / ∞ / negative) → default.
pub fn resolve_git_status_limit(value: Option<f64>) -> i64 {
    match value {
        Some(v) if v.is_finite() && v.fract() == 0.0 && v >= 0.0 => v as i64,
        _ => DEFAULT_GIT_STATUS_LIMIT,
    }
}

/// The sticky "previous pass" state fed into [`cap_git_status_entries`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapPrevious {
    pub did_hit_limit: bool,
    pub status_length: Option<i64>,
}

/// Result of capping. `did_hit_limit`/`status_length` distinguish the "clean"
/// case (`did_hit_limit == false`, `status_length == None`) from the "hit" case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapResult<T> {
    pub entries: Vec<T>,
    pub did_hit_limit: bool,
    pub status_length: Option<i64>,
}

/// Cap `entries` to `limit` (0 = unlimited). Once the limit has ever been hit
/// (this pass or a previous one), the `did_hit_limit`/`status_length` fields are
/// reported and carried forward (`status_length` is the max observed length).
pub fn cap_git_status_entries<T>(
    entries: Vec<T>,
    limit: i64,
    previous: CapPrevious,
) -> CapResult<T> {
    let len = entries.len() as i64;
    let exceeded_limit = limit > 0 && len > limit;
    if !exceeded_limit && !previous.did_hit_limit {
        return CapResult {
            entries,
            did_hit_limit: false,
            status_length: None,
        };
    }
    let out_entries = if exceeded_limit {
        entries.into_iter().take(limit as usize).collect()
    } else {
        entries
    };
    CapResult {
        entries: out_entries,
        did_hit_limit: true,
        status_length: Some(previous.status_length.unwrap_or(0).max(len)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- resolveGitStatusLimit oracle ---

    #[test]
    fn resolves_integer_ge_zero_else_default() {
        assert_eq!(resolve_git_status_limit(Some(0.0)), 0);
        assert_eq!(resolve_git_status_limit(Some(25.0)), 25);
        assert_eq!(
            resolve_git_status_limit(Some(1.5)),
            DEFAULT_GIT_STATUS_LIMIT
        ); // non-integer
        assert_eq!(
            resolve_git_status_limit(Some(f64::NAN)),
            DEFAULT_GIT_STATUS_LIMIT
        );
        assert_eq!(
            resolve_git_status_limit(Some(-1.0)),
            DEFAULT_GIT_STATUS_LIMIT
        );
        assert_eq!(resolve_git_status_limit(None), DEFAULT_GIT_STATUS_LIMIT); // non-number
    }

    // --- capGitStatusEntries oracle ---

    #[test]
    fn caps_and_reports_observed_length() {
        let r = cap_git_status_entries(vec!["a", "b", "c"], 2, CapPrevious::default());
        assert_eq!(
            r,
            CapResult {
                entries: vec!["a", "b"],
                did_hit_limit: true,
                status_length: Some(3),
            }
        );
    }

    #[test]
    fn preserves_previous_hit_signal_and_max_length() {
        let r = cap_git_status_entries(
            vec!["a"],
            2,
            CapPrevious {
                did_hit_limit: true,
                status_length: Some(3),
            },
        );
        assert_eq!(
            r,
            CapResult {
                entries: vec!["a"],
                did_hit_limit: true,
                status_length: Some(3), // max(3, 1)
            }
        );
    }

    #[test]
    fn zero_limit_is_unlimited() {
        let r = cap_git_status_entries(vec!["a", "b"], 0, CapPrevious::default());
        assert_eq!(
            r,
            CapResult {
                entries: vec!["a", "b"],
                did_hit_limit: false,
                status_length: None,
            }
        );
    }

    // --- extra pins ---

    #[test]
    fn exactly_at_limit_is_not_exceeded() {
        // len == limit is NOT "> limit" → clean.
        let r = cap_git_status_entries(vec!["a", "b"], 2, CapPrevious::default());
        assert!(!r.did_hit_limit);
        assert_eq!(r.status_length, None);
        assert_eq!(r.entries, vec!["a", "b"]);
    }

    #[test]
    fn sticky_previous_uses_max_of_current_length() {
        // Previous statusLength smaller than current → current wins.
        let r = cap_git_status_entries(
            vec!["a", "b", "c", "d"],
            0, // unlimited, so not exceeded this pass
            CapPrevious {
                did_hit_limit: true,
                status_length: Some(2),
            },
        );
        assert!(r.did_hit_limit);
        assert_eq!(r.status_length, Some(4)); // max(2, 4)
        assert_eq!(r.entries.len(), 4); // unlimited → not truncated
    }
}
