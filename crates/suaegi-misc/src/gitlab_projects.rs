//! GitLab "recent projects" list computation — verbatim port of Orca's
//! `src/shared/gitlab-projects.ts` (@ v1.4.146-rc.0).
//!
//! Pure helper for `GitLabProjectSettings['recent']` (see
//! `gitlab-types.ts:235-238`): given the existing recents, the project just
//! opened, and a timestamp, returns the next most-recent-first, deduped,
//! capped list. Kept out of the IPC handler so the recents logic is testable
//! without mocking the Store — the original module never touches a clock, a
//! filesystem, or the network.
//!
//! Contract decisions ported from
//! `docs/superpowers/plans/2026-07-27-gitlab-recents.md` (L-numbers refer to
//! that plan's §1):
//!
//! - **L1** — ⚠⚠ [`compute_next_gitlab_recents`]'s `now` is taken as an
//!   **already-formatted `&str`**, not reimplemented as `toISOString`. The TS
//!   signature is `now: Date = new Date()`, used in exactly one place
//!   (`:24`) as `now.toISOString()` — no comparison, no sorting, no
//!   arithmetic, so the entire observable contribution of `now` is that one
//!   string. Reimplementing `toISOString` faithfully means expanded
//!   `±YYYYYY` years outside 0000-9999, always-3 fractional digits, and a
//!   `RangeError` throw on an invalid `Date` — roughly 40 lines with zero
//!   oracle coverage, and it would break this crate's empty `[dependencies]`
//!   (no `chrono`, no date crate). `&str` pass-through is *more* faithful
//!   than that reimplementation, not less. The one thing lost is the
//!   `RangeError` arm: `new Date()` is never invalid and the sole production
//!   caller never passes `now` explicitly, so that arm is dead in practice.
//!   `now` is **required** here (no default) — with no clock to read, there
//!   is nothing for a `None` to fall back to; the TS default is the caller's
//!   responsibility now. Bonus: the oracle never pins this either way — its
//!   expected values are `fixedNow.toISOString()` (`test:9`,`:21`), a
//!   self-referential check of `toISOString` against itself that would not
//!   notice a self-consistent but wrong formatter.
//! - **L2** — dedupe key is `entry.host === host && entry.path === path`
//!   (`:23`), strict string equality: **case-sensitive**, no trim, no host
//!   lowercasing. `&&` is load-bearing (an `||` port fails the oracle); case
//!   sensitivity is **not** pinned upstream (no mixed-case fixture) but
//!   pinned here. It's `Vec::retain`/`filter`, so **every** matching entry is
//!   dropped, not just the first — also unpinned upstream (no fixture with
//!   two identical `(host, path)` entries) and pinned here.
//! - **L3** — filtering happens **before** prepending (`:23` -> `:24`).
//!   Order is observable: prepend-then-filter-**all** would delete the
//!   just-inserted head too (the new head satisfies its own filter
//!   predicate) and the oracle catches that. But prepend-then-keep-**first**-
//!   **occurrence** is a true equivalent given the 3-field entry type and a
//!   head that's always fresh — not a mutation target, just documented here.
//! - **L4** — ⚠ `.slice(0, max)` is not `Vec::truncate`. ECMAScript slice
//!   computes `end` via `ToIntegerOrInfinity` then
//!   `final = end < 0 ? max(len + end, 0) : min(end, len)`; see
//!   [`js_array_slice_prefix_len`]. The one value class where this diverges
//!   from a naive `truncate(max as usize)` is a negative finite `max` with
//!   `|max| < len`: JS drops only the last element, while `-1.0 as usize`
//!   saturates to `0` in Rust and would empty the whole vector. Neither
//!   production nor the oracle ever pass `max` explicitly, so a naive
//!   `usize` port is 100% green there — the divergence is oracle-silent by
//!   construction, hence `Option<f64>` plus the real helper (same rationale
//!   as `github_project_ref_input`'s K2: the signature is the contract).
//! - **L5** — input is never mutated; the return is a fresh `Vec` (TS
//!   allocates three times: `filter`, the spread array literal, `slice`).
//!   Taking `existing: &[GitLabRecentProject]` makes in-place mutation
//!   structurally impossible in Rust, so this can't regress by construction
//!   — the case is still pinned (with an entry that *would* be removed, not
//!   survive, since the oracle's own "does not mutate" fixture never removes
//!   its own entry) in case a future `&mut` refactor reintroduces the risk.
//!   JS additionally copies surviving entries **by reference**, so aliasing
//!   between the input and output arrays is observable upstream; an owned
//!   `Vec<GitLabRecentProject>` clones instead, so that aliasing has no Rust
//!   equivalent and is not represented.
//! - **L6** — ⚠⚠ surviving entries keep their **original**
//!   `last_opened_at` — only the new head gets `now`. The oracle never pins
//!   this (`test:20` maps `.path` only, `:21` checks `result[0]` only,
//!   `:28-29` use `toMatchObject({host, path})`), so a port that stamps
//!   `now` on every entry passes all 5 oracle cases. Pinned directly here:
//!   [`pin_surviving_entries_keep_their_original_timestamp`].
//! - **L7** — [`GITLAB_RECENTS_MAX`] is symbolic-only upstream (referenced,
//!   never compared to a literal) — pin the literal value, not just the
//!   name. The oracle's own cap fixture builds `lastOpenedAt` as
//!   `` `2026-05-0${i}` ``, which would silently corrupt to `2026-05-010` if
//!   `GITLAB_RECENTS_MAX` became 11 while the test still passed — another
//!   reason to pin the literal directly.
//! - **L8** — [`GitLabRecentProject`] mirrors `GitLabProjectSettings['recent']`
//!   element type (`gitlab-types.ts:235-238`): three non-optional `String`
//!   fields, `PartialEq`. `lastOpenedAt` is typed as a plain string upstream
//!   (not a `Date`), consistent with the `&str` L1 decision. No `serde`
//!   derive — this module neither parses nor serializes (precedent:
//!   `claude_roster`, `project_runtime`). `pinned` (the sibling field on
//!   `GitLabProjectSettings`) is out of scope: this module never reads or
//!   writes it, callers pass it through unchanged.
//! - **L9** — upstream has **zero integration coverage** of the recents
//!   write path: `ipc/gitlab.test.ts:114` mocks `recordGitLabProjectRecent`
//!   with `vi.fn()`. This module only covers the pure computation.

/// Default max recents kept before older entries fall off. L7: symbolic-only
/// upstream — pin the literal, not just the name.
pub const GITLAB_RECENTS_MAX: f64 = 10.0;

/// One entry in `GitLabProjectSettings['recent']` (`gitlab-types.ts:235-238`).
/// L8: three non-optional `String` fields, no `serde` derive (never
/// parsed/serialized here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabRecentProject {
    pub host: String,
    pub path: String,
    pub last_opened_at: String,
}

/// ECMAScript `Array.prototype.slice(0, end)` prefix length for an array of
/// length `len` (L4). Implements `ToIntegerOrInfinity` followed by
/// `final = end < 0 ? max(len + end, 0) : min(end, len)`, then returns
/// `final` as the number of leading elements to keep (since `slice(0, end)`
/// always starts at index 0). `ToIntegerOrInfinity(NaN)` is `+0`; finite
/// values truncate toward zero; `±Infinity` pass through unchanged.
fn js_array_slice_prefix_len(len: usize, end: f64) -> usize {
    let relative_end = if end.is_nan() {
        0.0
    } else if end.is_infinite() {
        end
    } else {
        end.trunc()
    };
    let len_f = len as f64;
    let final_len = if relative_end < 0.0 {
        (len_f + relative_end).max(0.0)
    } else {
        relative_end.min(len_f)
    };
    // final_len is always within [0, len_f] here, so this cast is exact.
    final_len as usize
}

/// Compute the next `recent` list when a project at (host, path) is opened.
/// Most-recent-first ordering, dedupes by `host` + `path` (L2), caps at
/// `max` entries ([`GITLAB_RECENTS_MAX`] when `None`, L4/L7). Returns a
/// fresh `Vec` — caller is responsible for persisting it.
///
/// `now` (L1) is the caller-formatted ISO-8601 timestamp stamped on the new
/// head entry only — surviving entries keep their original `last_opened_at`
/// (L6). Required, no default: this module never reads a clock.
pub fn compute_next_gitlab_recents(
    existing: &[GitLabRecentProject],
    host: &str,
    path: &str,
    now: &str,
    max: Option<f64>,
) -> Vec<GitLabRecentProject> {
    let max = max.unwrap_or(GITLAB_RECENTS_MAX);

    // Why: filter before prepend so re-opening an already-recent project
    // moves it to the front rather than producing a duplicate (L3).
    let filtered = existing
        .iter()
        .filter(|entry| !(entry.host == host && entry.path == path));

    let mut result = Vec::with_capacity(existing.len() + 1);
    result.push(GitLabRecentProject {
        host: host.to_string(),
        path: path.to_string(),
        last_opened_at: now.to_string(),
    });
    result.extend(filtered.cloned());

    let keep = js_array_slice_prefix_len(result.len(), max);
    result.truncate(keep);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(host: &str, path: &str, last_opened_at: &str) -> GitLabRecentProject {
        GitLabRecentProject {
            host: host.to_string(),
            path: path.to_string(),
            last_opened_at: last_opened_at.to_string(),
        }
    }

    const FIXED_NOW: &str = "2026-05-08T10:00:00.000Z";

    // Oracle: gitlab-projects.test.ts (5/5).

    #[test]
    fn prepends_a_fresh_entry_to_an_empty_list() {
        let result = compute_next_gitlab_recents(&[], "gitlab.com", "g/p", FIXED_NOW, None);
        assert_eq!(result, vec![entry("gitlab.com", "g/p", FIXED_NOW)]);
    }

    #[test]
    fn moves_an_existing_entry_to_the_front_dedupes_by_host_and_path() {
        let existing = vec![
            entry("gitlab.com", "a/b", "2026-05-07"),
            entry("gitlab.com", "g/p", "2026-05-06"),
            entry("gitlab.com", "c/d", "2026-05-05"),
        ];
        let result = compute_next_gitlab_recents(&existing, "gitlab.com", "g/p", FIXED_NOW, None);
        let paths: Vec<&str> = result.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["g/p", "a/b", "c/d"]);
        assert_eq!(result[0].last_opened_at, FIXED_NOW);
    }

    #[test]
    fn treats_different_hosts_at_the_same_path_as_distinct_entries() {
        let existing = vec![entry("gitlab.example.com", "g/p", "2026-05-07")];
        let result = compute_next_gitlab_recents(&existing, "gitlab.com", "g/p", FIXED_NOW, None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].host, "gitlab.com");
        assert_eq!(result[0].path, "g/p");
        assert_eq!(result[1].host, "gitlab.example.com");
        assert_eq!(result[1].path, "g/p");
    }

    #[test]
    fn caps_the_list_at_gitlab_recents_max_entries() {
        let existing: Vec<GitLabRecentProject> = (0..GITLAB_RECENTS_MAX as usize)
            .map(|i| entry("gitlab.com", &format!("g/p{i}"), &format!("2026-05-0{i}")))
            .collect();
        let result = compute_next_gitlab_recents(&existing, "gitlab.com", "g/new", FIXED_NOW, None);
        assert_eq!(result.len(), GITLAB_RECENTS_MAX as usize);
        assert_eq!(result[0].path, "g/new");
        // Why: oldest entry (the one that was at the tail before the
        // prepend) must be the one that fell off.
        let dropped_path = format!("g/p{}", GITLAB_RECENTS_MAX as usize - 1);
        assert!(!result.iter().any(|r| r.path == dropped_path));
    }

    #[test]
    fn does_not_mutate_the_input_slice() {
        let existing = vec![entry("gitlab.com", "a/b", "2026-05-07")];
        let snapshot = existing.clone();
        compute_next_gitlab_recents(&existing, "gitlab.com", "g/p", FIXED_NOW, None);
        assert_eq!(existing, snapshot);
    }

    // Mandatory extra pins (oracle-silent — plan §2):

    /// L6: the single most valuable pin in this module — surviving entries
    /// keep their **original** `last_opened_at`; only the new head gets
    /// `now`. The oracle never asserts this and a stamp-every-entry
    /// implementation would otherwise pass all 5 oracle cases.
    #[test]
    fn pin_surviving_entries_keep_their_original_timestamp() {
        let existing = vec![
            entry("gitlab.com", "a/b", "2026-05-07"),
            entry("gitlab.com", "c/d", "2026-05-05"),
        ];
        let result = compute_next_gitlab_recents(&existing, "gitlab.com", "g/p", FIXED_NOW, None);
        assert_eq!(result[0].last_opened_at, FIXED_NOW);
        assert_eq!(result[1].last_opened_at, "2026-05-07");
        assert_eq!(result[2].last_opened_at, "2026-05-05");
    }

    /// L7: the cap really is the literal 10, not just a named symbol.
    #[test]
    fn pin_gitlab_recents_max_is_the_literal_value() {
        assert_eq!(GITLAB_RECENTS_MAX, 10.0);
    }

    /// L4: every `max` arm across the JS numeric domain that
    /// `ToIntegerOrInfinity` + the slice `final` formula defines, especially
    /// the negative-finite divergence from `Vec::truncate` (`-1` drops only
    /// the last element, never saturates to empty).
    #[test]
    fn pin_max_covers_every_js_slice_numeric_branch() {
        let existing = vec![
            entry("gitlab.com", "a", "1"),
            entry("gitlab.com", "b", "2"),
            entry("gitlab.com", "c", "3"),
        ];
        // None: falls back to GITLAB_RECENTS_MAX (10), well above len+1 (4).
        let result = compute_next_gitlab_recents(&existing, "gitlab.com", "new", FIXED_NOW, None);
        assert_eq!(result.len(), 4);

        // 0: slice(0, 0) => empty.
        let result =
            compute_next_gitlab_recents(&existing, "gitlab.com", "new", FIXED_NOW, Some(0.0));
        assert_eq!(result.len(), 0);

        // -1: drops only the last element (len 4 -> 3), NOT truncate's
        // saturating-to-0 behavior.
        let result =
            compute_next_gitlab_recents(&existing, "gitlab.com", "new", FIXED_NOW, Some(-1.0));
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].path, "new");
        assert_eq!(result[1].path, "a");
        assert_eq!(result[2].path, "b");

        // NaN: ToIntegerOrInfinity(NaN) = 0 -> empty.
        let result =
            compute_next_gitlab_recents(&existing, "gitlab.com", "new", FIXED_NOW, Some(f64::NAN));
        assert_eq!(result.len(), 0);

        // 2.9: truncates toward zero -> 2.
        let result =
            compute_next_gitlab_recents(&existing, "gitlab.com", "new", FIXED_NOW, Some(2.9));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "new");
        assert_eq!(result[1].path, "a");

        // +Infinity: min(+inf, len) = len -> everything kept.
        let result = compute_next_gitlab_recents(
            &existing,
            "gitlab.com",
            "new",
            FIXED_NOW,
            Some(f64::INFINITY),
        );
        assert_eq!(result.len(), 4);
    }

    /// L2: dedupe is case-sensitive — `GitLab.com` and `gitlab.com` are
    /// distinct hosts, not merged.
    #[test]
    fn pin_dedupe_is_case_sensitive() {
        let existing = vec![entry("GitLab.com", "g/p", "2026-05-07")];
        let result = compute_next_gitlab_recents(&existing, "gitlab.com", "g/p", FIXED_NOW, None);
        assert_eq!(result.len(), 2);
    }

    /// L2: two pre-existing entries with the identical `(host, path)` are
    /// both removed by the dedupe filter, not just the first match.
    #[test]
    fn pin_dedupe_removes_all_matching_entries_not_just_the_first() {
        let existing = vec![
            entry("gitlab.com", "g/p", "2026-05-07"),
            entry("gitlab.com", "other", "2026-05-06"),
            entry("gitlab.com", "g/p", "2026-05-05"),
        ];
        let result = compute_next_gitlab_recents(&existing, "gitlab.com", "g/p", FIXED_NOW, None);
        let paths: Vec<&str> = result.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["g/p", "other"]);
    }

    /// L5: the "does not mutate" guarantee holds even when the opened
    /// project is itself removed by the dedupe filter — the oracle's own
    /// fixture never exercises this (its one entry is never a match).
    #[test]
    fn pin_does_not_mutate_the_input_when_the_opened_entry_is_itself_removed() {
        let existing = vec![entry("gitlab.com", "g/p", "2026-05-07")];
        let snapshot = existing.clone();
        compute_next_gitlab_recents(&existing, "gitlab.com", "g/p", FIXED_NOW, None);
        assert_eq!(existing, snapshot);
    }

    /// L4: `existing.len() > max` — the case is already over cap before the
    /// new head is even added, so more than one entry must fall off.
    #[test]
    fn pin_existing_longer_than_max_drops_multiple_entries() {
        let existing = vec![
            entry("gitlab.com", "a", "1"),
            entry("gitlab.com", "b", "2"),
            entry("gitlab.com", "c", "3"),
        ];
        let result =
            compute_next_gitlab_recents(&existing, "gitlab.com", "new", FIXED_NOW, Some(1.0));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "new");
    }

    /// Empty host/path are ordinary strings to this module — no special
    /// casing, still dedupe and prepend correctly.
    #[test]
    fn pin_empty_host_and_path_are_ordinary_strings() {
        let existing = vec![entry("", "", "2026-05-07")];
        let result = compute_next_gitlab_recents(&existing, "", "", FIXED_NOW, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].last_opened_at, FIXED_NOW);
    }

    /// L1: `now` is passed through verbatim — any string comes back
    /// unchanged, since this module never parses or reformats it.
    #[test]
    fn pin_now_is_passed_through_verbatim() {
        let result =
            compute_next_gitlab_recents(&[], "gitlab.com", "g/p", "not-a-real-timestamp", None);
        assert_eq!(result[0].last_opened_at, "not-a-real-timestamp");
    }
}
