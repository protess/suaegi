//! Stream parsers + accumulator — port of the rg-JSON and git-grep line
//! ingestion, `push_match`, and `finalize` (`src/shared/text-search.ts:106-134`,
//! `:225-282`, `:366-447`, `:451-457`). This is the byte-offset-hazardous
//! surface (plan §2 M3); every slice is char-boundary-safe via [`crate::clamp`].
//!
//! # Deviations from Orca (documented)
//! - **`push_match` signature.** Orca passes both `fileResult` and `acc`. In Rust
//!   the `fileResult` lives *inside* `acc.file_map`, so a simultaneous
//!   `&mut SearchFileResult` + `&mut SearchAccumulator` is a borrow conflict.
//!   Instead [`push_match`] takes the accumulator plus the `(abs, rel)` key and
//!   does get-or-create + push + count-bump + truncation itself. Semantics are
//!   identical; the truncated invariant (**C5**) stays inside `push_match`.
//! - **early-stop sets `truncated` (C5).** Orca's `ingest*` entry guard
//!   (`totalMatches >= maxResults → stop`) does NOT set `truncated`
//!   (`:232-234`, `:373-375`). Per plan C5 we set it here too, so the accumulator
//!   invariant `total_matches >= max_results ⇒ truncated` holds unconditionally
//!   (it is already true in the normal flow, since only `push_match` reaches the
//!   cap — this makes it robust even if a driver pre-seeds the total).
//! - **empty-submatch fallback is byte-safe (C6).** `end = 1` literal (Orca
//!   `:265-268`) is unsafe on a non-ASCII first scalar; we use the first UTF-8
//!   scalar's byte length instead. See [`ingest_rg_json_line`].
//! - **`finalize` iterates `file_order`, not the `HashMap`.** Rust's `HashMap` is
//!   unordered; `file_order` preserves Orca's insertion-ordered `Map` iteration.

use serde::Deserialize;

use crate::clamp::{clamp_line_context, Clamped};
use crate::match_count::normalize_search_result;
use crate::path::{join_search_root, relative_to_search_root};
use crate::path::normalize_relative_path;
use crate::types::{SearchAccumulator, SearchFileResult, SearchMatch, SearchResult};

/// The stream-parser verdict: keep feeding lines, or the cap was hit and the
/// caller should kill the child. Orca's `'continue' | 'stop'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingest {
    Continue,
    Stop,
}

// ─── accumulator helpers ─────────────────────────────────────────────

/// Get-or-create the file result for `abs`, tracking first-insertion order in
/// `file_order` exactly once (Orca's lazy `fileMap.get`/`set`, `:271-275` /
/// `:407-414`).
fn ensure_file(acc: &mut SearchAccumulator, abs: &str, rel: &str) {
    if !acc.file_map.contains_key(abs) {
        acc.file_order.push(abs.to_string());
        acc.file_map.insert(
            abs.to_string(),
            SearchFileResult {
                file_path: abs.to_string(),
                relative_path: rel.to_string(),
                matches: Vec::new(),
                match_count: Some(0),
            },
        );
    }
}

/// Push one match, bump the per-file count (`acceptMatch`, `:28-30`) and the
/// running total, and enforce the **truncated invariant (C5)**: on reaching the
/// cap, set `truncated = true` and return [`Ingest::Stop`] in the same call —
/// never a silent truncation (`:126-133`).
fn push_match(
    acc: &mut SearchAccumulator,
    abs: &str,
    rel: &str,
    clamped: Clamped,
    line_number: usize,
    max_results: usize,
) -> Ingest {
    ensure_file(acc, abs, rel);
    // `ensure_file` guarantees presence.
    let file_result = acc
        .file_map
        .get_mut(abs)
        .expect("ensure_file inserted the entry");
    file_result.matches.push(SearchMatch {
        line: line_number,
        column: clamped.column,
        match_length: clamped.match_length,
        line_content: clamped.line_content,
        display_column: clamped.display_column,
        display_match_length: clamped.display_match_length,
    });
    // acceptMatch: matchCount = (matchCount ?? 0) + 1.
    file_result.match_count = Some(file_result.match_count.unwrap_or(0) + 1);
    // The `file_result` borrow ends here (NLL); now touch accumulator totals.
    acc.total_matches += 1;
    if acc.total_matches >= max_results {
        acc.truncated = true;
        Ingest::Stop
    } else {
        Ingest::Continue
    }
}

// ─── rg --json ingestion ─────────────────────────────────────────────
//
// serde structs mirror rg's ACTUAL `--json` `match` event schema (snake_case:
// `line_number`, `submatches`, `path.text`, `lines.text`), NOT Orca's IPC shape.
// `#[serde(default)]` on every struct makes missing fields tolerant (Orca's
// `?? default`); unknown fields (e.g. a submatch's `match` object) are ignored.
// Numeric fields are `f64` so an integer OR a float (the "2.0 vs 2" hazard the
// M1 reviewer flagged) both deserialize — a wrong *type* (string) fails the
// whole line, which is then skipped (Continue), never aborting the stream.

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RgEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    data: Option<RgData>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RgData {
    path: Option<RgText>,
    submatches: Option<Vec<RgSubmatch>>,
    line_number: Option<f64>,
    lines: Option<RgText>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RgText {
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RgSubmatch {
    start: f64,
    end: f64,
}

/// Strip a single trailing `\n` (Orca `.replace(/\n$/, '')`, `:262`).
fn strip_trailing_newline(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

/// Ingest one line of rg `--json` stdout, mutating `acc`. Returns
/// [`Ingest::Stop`] when the total cap is reached (so the caller can kill the
/// child), else [`Ingest::Continue`]. `transform_abs_path` applies the local
/// WSL translation hook; the relay passes `None`.
///
/// Tolerant like Orca: empty line, malformed JSON, non-`match` type, or a
/// non-string path all yield `Continue` without aborting the stream.
pub fn ingest_rg_json_line(
    line: &str,
    root_path: &str,
    acc: &mut SearchAccumulator,
    max_results: usize,
    transform_abs_path: Option<&dyn Fn(&str) -> String>,
) -> Ingest {
    // Entry cap. C5: set truncated so `total >= max ⇒ truncated` always holds.
    if acc.total_matches >= max_results {
        acc.truncated = true;
        return Ingest::Stop;
    }
    if line.is_empty() {
        return Ingest::Continue;
    }
    // Malformed JSON → skip this line, keep the stream alive (`:247-251`).
    let Ok(event) = serde_json::from_str::<RgEvent>(line) else {
        return Ingest::Continue;
    };
    // Only `match` events with data (`:252-254`).
    if event.kind.as_deref() != Some("match") {
        return Ingest::Continue;
    }
    let Some(data) = event.data else {
        return Ingest::Continue;
    };
    // Non-string path → skip (`:256-259`).
    let Some(raw_path) = data.path.and_then(|p| p.text) else {
        return Ingest::Continue;
    };

    let abs_path = match transform_abs_path {
        Some(f) => f(&raw_path),
        None => raw_path,
    };
    let rel_path = normalize_relative_path(&relative_to_search_root(root_path, &abs_path));
    let line_content = strip_trailing_newline(data.lines.and_then(|l| l.text).as_deref().unwrap_or("")).to_string();
    let line_number = data.line_number.map(|n| n as usize).unwrap_or(0);

    let mut submatches = data.submatches.unwrap_or_default();
    if submatches.is_empty() {
        // C6 byte-safe empty-submatch fallback: surface a navigable line-level
        // result instead of a count-0 row. `end` is the first UTF-8 scalar's byte
        // length (Orca's literal `1` would be a mid-scalar, byte-unsafe boundary
        // on a non-ASCII first char) — a deliberate safety correction (plan C6).
        let end = line_content
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(0) as f64;
        submatches = vec![RgSubmatch { start: 0.0, end }];
    }

    for sub in submatches {
        let start = sub.start as usize;
        let end = sub.end as usize;
        // Malformed `start > end` cannot underflow-panic.
        let match_length = end.saturating_sub(start);
        let clamped = clamp_line_context(&line_content, start, match_length);
        if push_match(acc, &abs_path, &rel_path, clamped, line_number, max_results) == Ingest::Stop {
            return Ingest::Stop;
        }
    }
    Ingest::Continue
}

// ─── git grep ingestion ──────────────────────────────────────────────

/// Ingest one line of `git grep --null -n` stdout, mutating `acc`. Parses the
/// modern `filename\0lineno\0content` format, falling back to the legacy
/// `filename\0lineno:content` colon form (kept for older git; plan keeps both).
///
/// With `submatch_regex == None` (regex compile failed) the whole line is
/// highlighted (`:417-421`). Otherwise every occurrence is located; if the regex
/// finds nothing but git reported the line, the whole line is still surfaced —
/// **a git-confirmed hit is never dropped** (`:438-445`).
pub fn ingest_git_grep_line(
    line: &str,
    root_path: &str,
    submatch_regex: Option<&regex::Regex>,
    acc: &mut SearchAccumulator,
    max_results: usize,
) -> Ingest {
    // Entry cap. C5: set truncated so `total >= max ⇒ truncated` always holds.
    if acc.total_matches >= max_results {
        acc.truncated = true;
        return Ingest::Stop;
    }
    if line.is_empty() {
        return Ingest::Continue;
    }

    // filename\0... — at least one NUL required (`:381-384`). NUL/':' are ASCII,
    // so every `find`/`+1` index below is a char boundary (safe on multibyte
    // content).
    let Some(null_idx) = line.find('\0') else {
        return Ingest::Continue;
    };
    let rel_path = normalize_relative_path(&line[..null_idx]);
    let rest = &line[null_idx + 1..];

    let (line_number_text, line_content): (&str, &str) = match rest.find('\0') {
        // Modern: lineno\0content (`:390-392`).
        Some(second) => (&rest[..second], strip_trailing_newline(&rest[second + 1..])),
        // Legacy colon fallback (`:393-400`).
        None => {
            let Some(colon) = rest.find(':') else {
                return Ingest::Continue;
            };
            (&rest[..colon], strip_trailing_newline(&rest[colon + 1..]))
        }
    };

    // `/^\d+$/`: non-empty, all ASCII digits (`:401-403`).
    if line_number_text.is_empty() || !line_number_text.bytes().all(|b| b.is_ascii_digit()) {
        return Ingest::Continue;
    }
    let line_num = line_number_text.parse::<usize>().unwrap_or(0);

    let abs_path = join_search_root(root_path, &rel_path);

    // No JS-side regex: whole-line highlight (`:417-421`).
    let Some(re) = submatch_regex else {
        let clamped = clamp_line_context(line_content, 0, line_content.len());
        return push_match(acc, &abs_path, &rel_path, clamped, line_num, max_results);
    };

    // Locate every occurrence. `regex::find_iter` yields non-overlapping matches
    // and advances past empty matches internally, so it CANNOT infinite-loop on a
    // zero-length pattern — this replaces Orca's manual `lastIndex++` guard
    // (`:433-436`). Match offsets are byte offsets on char boundaries.
    let mut accepted = false;
    for m in re.find_iter(line_content) {
        let clamped = clamp_line_context(line_content, m.start(), m.end() - m.start());
        accepted = true;
        if push_match(acc, &abs_path, &rel_path, clamped, line_num, max_results) == Ingest::Stop {
            return Ingest::Stop;
        }
    }

    // git confirmed the line but the regex found nothing → keep it navigable;
    // don't drop a git-confirmed hit (`:438-445`).
    if !accepted {
        let clamped = clamp_line_context(line_content, 0, line_content.len());
        return push_match(acc, &abs_path, &rel_path, clamped, line_num, max_results);
    }
    Ingest::Continue
}

// ─── finalize ────────────────────────────────────────────────────────

/// Collect the accumulator into a `SearchResult`, iterating `file_order` for
/// deterministic (insertion-ordered) output, filtering empty files, and
/// normalizing per-file counts (`:451-457`).
pub fn finalize(acc: &SearchAccumulator) -> SearchResult {
    let files = acc
        .file_order
        .iter()
        .filter_map(|key| acc.file_map.get(key))
        .filter(|file| !file.matches.is_empty())
        .cloned()
        .collect();
    normalize_search_result(SearchResult {
        files,
        total_matches: acc.total_matches,
        truncated: acc.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::submatch::build_submatch_regex;
    use crate::types::{create_accumulator, SearchOptions};
    use serde_json::json;

    // ── rg helpers ──────────────────────────────────────────────

    /// Build one rg `--json` `match` event line (mirrors the test's `makeMatch`).
    fn rg_match(path: &str, line: usize, subs: &[(usize, usize)], text: &str) -> String {
        let submatches: Vec<_> = subs
            .iter()
            .map(|(s, e)| json!({ "match": { "text": "" }, "start": s, "end": e }))
            .collect();
        json!({
            "type": "match",
            "data": {
                "path": { "text": path },
                "line_number": line,
                "lines": { "text": format!("{text}\n") },
                "submatches": submatches,
            }
        })
        .to_string()
    }

    fn only_file(acc: &SearchAccumulator) -> &SearchFileResult {
        assert_eq!(acc.file_order.len(), 1, "expected exactly one file");
        acc.file_map.get(&acc.file_order[0]).unwrap()
    }

    // ── rg oracle cases ─────────────────────────────────────────

    /// Oracle case 8 (`:91`): a basic match populates the accumulator with the
    /// canonical `{line, column:1, matchLength:3, lineContent:'abc'}`.
    #[test]
    fn rg_oracle_8_populates_accumulator() {
        let mut acc = create_accumulator();
        let v = ingest_rg_json_line(
            &rg_match("/root/src/a.ts", 2, &[(0, 3)], "abc"),
            "/root",
            &mut acc,
            100,
            None,
        );
        assert_eq!(v, Ingest::Continue);
        assert_eq!(acc.total_matches, 1);
        let f = only_file(&acc);
        assert_eq!(f.relative_path, "src/a.ts");
        assert_eq!(f.match_count, Some(1));
        assert_eq!(f.matches[0].line, 2);
        assert_eq!(f.matches[0].column, 1);
        assert_eq!(f.matches[0].match_length, 3);
        assert_eq!(f.matches[0].line_content, "abc");
        assert!(f.matches[0].display_column.is_none());
    }

    /// Oracle case 9 (`:107`): a `begin` (non-`match`) event is ignored.
    #[test]
    fn rg_oracle_9_ignores_non_match() {
        let mut acc = create_accumulator();
        let line = json!({ "type": "begin", "data": {} }).to_string();
        let v = ingest_rg_json_line(&line, "/root", &mut acc, 100, None);
        assert_eq!(v, Ingest::Continue);
        assert_eq!(acc.total_matches, 0);
    }

    /// Oracle case 10 (`:113`): malformed JSON is skipped, stream stays alive.
    #[test]
    fn rg_oracle_10_skips_malformed_json() {
        let mut acc = create_accumulator();
        let v = ingest_rg_json_line("not json", "/root", &mut acc, 100, None);
        assert_eq!(v, Ingest::Continue);
        assert_eq!(acc.total_matches, 0);
    }

    /// Oracle case 11 (`:120`): rg omits submatch ranges on a non-empty line →
    /// navigable fallback `matchLength:1`, `lineContent:'foobar'`.
    ///
    /// *Mutation (b):* dropping the empty-submatch fallback (`if submatches.is_empty()`)
    /// leaves zero matches → this fails.
    #[test]
    fn rg_oracle_11_empty_submatch_fallback_nonempty() {
        let mut acc = create_accumulator();
        let v = ingest_rg_json_line(
            &rg_match("/root/a.ts", 4, &[], "foobar"),
            "/root",
            &mut acc,
            100,
            None,
        );
        assert_eq!(v, Ingest::Continue);
        assert_eq!(acc.total_matches, 1);
        let f = only_file(&acc);
        assert_eq!(f.match_count, Some(1));
        assert_eq!(f.matches.len(), 1);
        assert_eq!(f.matches[0].line, 4);
        assert_eq!(f.matches[0].column, 1);
        assert_eq!(f.matches[0].match_length, 1);
        assert_eq!(f.matches[0].line_content, "foobar");
    }

    /// Oracle case 12 (`:130`): empty line + omitted submatches → `matchLength:0`,
    /// `lineContent:''`. *Mutation (b):* dropping the fallback fails this too.
    #[test]
    fn rg_oracle_12_empty_submatch_fallback_empty_line() {
        let mut acc = create_accumulator();
        ingest_rg_json_line(&rg_match("/root/a.ts", 5, &[], ""), "/root", &mut acc, 100, None);
        let f = only_file(&acc);
        assert_eq!(f.match_count, Some(1));
        assert_eq!(f.matches[0].line, 5);
        assert_eq!(f.matches[0].column, 1);
        assert_eq!(f.matches[0].match_length, 0);
        assert_eq!(f.matches[0].line_content, "");
    }

    /// C6 non-ASCII variant of the empty-submatch fallback: on a line whose first
    /// scalar is `é` (2 bytes), the fallback `end` is the scalar's byte length (2),
    /// NOT Orca's literal `1` (which would be a mid-`é`, byte-unsafe boundary).
    /// Deliberate safety correction — must not panic and must report `matchLength:2`.
    #[test]
    fn rg_c6_empty_submatch_fallback_non_ascii() {
        let mut acc = create_accumulator();
        ingest_rg_json_line(
            &rg_match("/root/a.ts", 1, &[], "ément"),
            "/root",
            &mut acc,
            100,
            None,
        );
        let f = only_file(&acc);
        assert_eq!(f.matches[0].match_length, 2, "first scalar 'é' is 2 bytes");
        assert_eq!(f.matches[0].line_content, "ément");
    }

    /// Oracle case 13 (`:138`): three submatches, `maxResults:2` → `Stop`,
    /// `truncated:true`, `totalMatches:2`, `matchCount:2` — synchronously.
    ///
    /// *Mutation (a):* removing `acc.truncated = true` in `push_match` fails the
    /// `truncated` assertion here (the truncated invariant / cardinal-sin guard).
    #[test]
    fn rg_oracle_13_stops_at_max_results_truncated() {
        let mut acc = create_accumulator();
        let v = ingest_rg_json_line(
            &rg_match("/root/a.ts", 1, &[(0, 1), (2, 3), (4, 5)], "abcdef"),
            "/root",
            &mut acc,
            2,
            None,
        );
        assert_eq!(v, Ingest::Stop);
        assert!(acc.truncated, "truncated invariant: cap reached ⇒ truncated");
        assert_eq!(acc.total_matches, 2);
        assert_eq!(only_file(&acc).match_count, Some(2));
    }

    /// Oracle case 14 (`:156`): a huge line is clamped; `column` stays the TRUE
    /// source coordinate while `display_*` index the snippet, and the display
    /// slice recovers `NEEDLE`. (Integration of `clamp_line_context` through the
    /// rg path.)
    #[test]
    fn rg_oracle_14_clamps_huge_line() {
        let mut acc = create_accumulator();
        let huge = format!("{}NEEDLE{}", "x".repeat(200_000), "y".repeat(200_000));
        let start = 200_000;
        let line = json!({
            "type": "match",
            "data": {
                "path": { "text": "/root/big.js" },
                "line_number": 1,
                "lines": { "text": format!("{huge}\n") },
                "submatches": [{ "start": start, "end": start + 6 }],
            }
        })
        .to_string();
        ingest_rg_json_line(&line, "/root", &mut acc, 100, None);
        let m = &only_file(&acc).matches[0];
        assert!(m.line_content.len() <= crate::MAX_LINE_CONTENT_LENGTH + 2 * "…".len());
        assert_eq!(m.column, start + 1);
        assert_eq!(m.match_length, 6);
        let dc = m.display_column.expect("clamped → display_column");
        assert_eq!(m.display_match_length, Some(6));
        assert_eq!(&m.line_content.as_bytes()[dc - 1..dc - 1 + 6], b"NEEDLE");
    }

    /// Oracle case 15 (`:190`): `transformAbsPath` (WSL) rewrites the absolute
    /// path prefix; `filePath` is the transformed path.
    #[test]
    fn rg_oracle_15_applies_transform_abs_path() {
        let mut acc = create_accumulator();
        let transform = |p: &str| p.replace("/home/u/repo", "\\\\wsl$\\Ubuntu\\home\\u\\repo");
        ingest_rg_json_line(
            &rg_match("/home/u/repo/a.ts", 1, &[(0, 1)], "x"),
            "\\\\wsl$\\Ubuntu\\home\\u\\repo",
            &mut acc,
            100,
            Some(&transform),
        );
        let f = only_file(&acc);
        assert_eq!(f.file_path, "\\\\wsl$\\Ubuntu\\home\\u\\repo/a.ts");
        assert_eq!(f.relative_path, "a.ts");
    }

    /// C5 entry guard: entering already at the cap returns `Stop` AND sets
    /// `truncated` (Orca's entry guard does not; plan C5 correction). *Mutation:*
    /// removing `acc.truncated = true` from the entry guard fails this.
    #[test]
    fn rg_entry_guard_sets_truncated() {
        let mut acc = create_accumulator();
        acc.total_matches = 5;
        let v = ingest_rg_json_line(
            &rg_match("/root/a.ts", 1, &[(0, 1)], "x"),
            "/root",
            &mut acc,
            5,
            None,
        );
        assert_eq!(v, Ingest::Stop);
        assert!(acc.truncated);
        assert_eq!(acc.total_matches, 5, "no match pushed past the cap");
    }

    /// The M1-flagged "2.0 vs 2" hazard: rg numeric fields deserialize whether
    /// emitted as an integer or a float. A float `line_number` must parse (not
    /// skip the line). *Mutation:* typing `line_number` as `i64` makes `2.0` fail
    /// to deserialize → the line is skipped → `total_matches` is 0 → this fails.
    #[test]
    fn rg_tolerates_float_line_number() {
        let mut acc = create_accumulator();
        let line = json!({
            "type": "match",
            "data": {
                "path": { "text": "/root/a.ts" },
                "line_number": 2.0,
                "lines": { "text": "abc\n" },
                "submatches": [{ "start": 0.0, "end": 3.0 }],
            }
        })
        .to_string();
        ingest_rg_json_line(&line, "/root", &mut acc, 100, None);
        assert_eq!(acc.total_matches, 1);
        assert_eq!(only_file(&acc).matches[0].line, 2);
    }

    /// A malformed `start > end` submatch must not underflow-panic; the match is
    /// still surfaced with a saturated (0) length.
    #[test]
    fn rg_malformed_submatch_start_gt_end_no_panic() {
        let mut acc = create_accumulator();
        ingest_rg_json_line(
            &rg_match("/root/a.ts", 1, &[(5, 2)], "abcdef"),
            "/root",
            &mut acc,
            100,
            None,
        );
        assert_eq!(only_file(&acc).matches[0].match_length, 0);
    }

    // ── git grep helpers ────────────────────────────────────────

    fn re_for(query: &str) -> regex::Regex {
        build_submatch_regex(query, &SearchOptions::default()).unwrap()
    }

    // ── git grep oracle cases ───────────────────────────────────

    /// **Recorded fixture** replacing impure oracle case 25 (`:268-306`, which
    /// shelled out to real `git`). Captured from `git version 2.50.1 (Apple
    /// Git-155)` via `git -c submodule.recurse=false grep -n -I --null --no-color
    /// --untracked --no-recurse-submodules -i --fixed-strings -e 'reportError(' --
    /// .` over a repo with `src/a.ts` = the three lines below. The bytes are the
    /// real `filename\0lineno\0content\n` stdout (hex-verified). Feeding each split
    /// line reproduces the oracle: 3 matches at `[[1,1],[2,1],[2,19]]`.
    const GIT_GREP_FIXTURE_2_50_1: &str = concat!(
        "src/a.ts\u{0}1\u{0}reportError(err, { action: 'save' })\n",
        "src/a.ts\u{0}2\u{0}reportError(err); reportError(next)\n",
    );

    #[test]
    fn git_oracle_25_recorded_fixture() {
        let mut acc = create_accumulator();
        let re = re_for("reportError(");
        for line in GIT_GREP_FIXTURE_2_50_1.split('\n') {
            ingest_git_grep_line(line, "/root", Some(&re), &mut acc, 100);
        }
        let result = finalize(&acc);
        assert_eq!(result.total_matches, 3);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].relative_path, "src/a.ts");
        assert_eq!(result.files[0].match_count, Some(3));
        let coords: Vec<(usize, usize)> = result.files[0]
            .matches
            .iter()
            .map(|m| (m.line, m.column))
            .collect();
        assert_eq!(coords, vec![(1, 1), (2, 1), (2, 19)]);
    }

    /// Oracle case 26 (`:308`): modern null-delimited line, all occurrences
    /// located (`foo` at col 1 and col 9).
    #[test]
    fn git_oracle_26_null_multi_position() {
        let mut acc = create_accumulator();
        let re = re_for("foo");
        let v = ingest_git_grep_line("src/a.ts\u{0}5\u{0}foo and foo again\n", "/root", Some(&re), &mut acc, 100);
        assert_eq!(v, Ingest::Continue);
        let f = only_file(&acc);
        assert_eq!(f.match_count, Some(2));
        assert_eq!(f.matches.len(), 2);
        assert_eq!((f.matches[0].line, f.matches[0].column), (5, 1));
        assert_eq!((f.matches[1].line, f.matches[1].column), (5, 9));
    }

    /// Oracle case 27 (`:320`): legacy colon-delimited line still parses.
    #[test]
    fn git_oracle_27_legacy_colon_format() {
        let mut acc = create_accumulator();
        let re = re_for("foo");
        ingest_git_grep_line("src/a.ts\u{0}5:foo", "/root", Some(&re), &mut acc, 100);
        let f = only_file(&acc);
        assert_eq!(f.match_count, Some(1));
        assert_eq!((f.matches[0].line, f.matches[0].column), (5, 1));
    }

    /// Oracle case 28 (`:329`): a colon INSIDE matched content is not treated as
    /// the delimiter (the second NUL wins).
    #[test]
    fn git_oracle_28_content_colon_not_delimiter() {
        let mut acc = create_accumulator();
        let re = re_for("reportError(");
        ingest_git_grep_line(
            "src/a.ts\u{0}10\u{0}reportError(err, { action: 'save' })\n",
            "/root",
            Some(&re),
            &mut acc,
            100,
        );
        let f = only_file(&acc);
        assert_eq!(f.match_count, Some(1));
        assert_eq!(f.matches.len(), 1);
        assert_eq!(
            (f.matches[0].line, f.matches[0].column, f.matches[0].match_length),
            (10, 1, 12)
        );
    }

    /// Oracle case 29 (`:345`): a colon in the FILENAME is handled via the NUL
    /// delimiter.
    #[test]
    fn git_oracle_29_colon_in_filename() {
        let mut acc = create_accumulator();
        let re = re_for("x");
        ingest_git_grep_line("weird:name.ts\u{0}1\u{0}x", "/root", Some(&re), &mut acc, 100);
        assert_eq!(only_file(&acc).relative_path, "weird:name.ts");
    }

    /// Oracle case 30 (`:353`): malformed lines (no NUL, no colon, non-numeric
    /// lineno) are all skipped.
    #[test]
    fn git_oracle_30_skips_malformed() {
        let mut acc = create_accumulator();
        let re = re_for("q");
        ingest_git_grep_line("no-null-byte", "/r", Some(&re), &mut acc, 100);
        ingest_git_grep_line("a.ts\u{0}no-colon", "/r", Some(&re), &mut acc, 100);
        ingest_git_grep_line("a.ts\u{0}NaN:content", "/r", Some(&re), &mut acc, 100);
        assert_eq!(acc.total_matches, 0);
    }

    /// Oracle case 31 (`:362`): a zero-length regex does not loop forever; it
    /// terminates with `0 < total ≤ maxResults`.
    #[test]
    fn git_oracle_31_zero_length_regex_no_loop() {
        let mut acc = create_accumulator();
        let re = regex::Regex::new("").unwrap();
        ingest_git_grep_line("a.ts\u{0}1\u{0}abc", "/r", Some(&re), &mut acc, 5);
        assert!(acc.total_matches > 0);
        assert!(acc.total_matches <= 5);
    }

    /// Oracle case 32 (`:371`): git total cap → `Stop`, `truncated:true`,
    /// `totalMatches:2`. *Mutation (a):* removing `truncated = true` fails this.
    #[test]
    fn git_oracle_32_stops_at_max_results_truncated() {
        let mut acc = create_accumulator();
        let re = re_for("a");
        let v = ingest_git_grep_line("f\u{0}1\u{0}aaaa", "/r", Some(&re), &mut acc, 2);
        assert_eq!(v, Ingest::Stop);
        assert!(acc.truncated);
        assert_eq!(acc.total_matches, 2);
        assert_eq!(only_file(&acc).match_count, Some(2));
    }

    /// Oracle case 33 (`:381`): `submatchRegex == None` → whole-line highlight.
    #[test]
    fn git_oracle_33_null_regex_whole_line() {
        let mut acc = create_accumulator();
        let v = ingest_git_grep_line("a.ts\u{0}3\u{0}hello world", "/r", None, &mut acc, 100);
        assert_eq!(v, Ingest::Continue);
        let f = only_file(&acc);
        assert_eq!(f.match_count, Some(1));
        assert_eq!(f.matches.len(), 1);
        assert_eq!(
            (f.matches[0].line, f.matches[0].column, f.matches[0].match_length),
            (3, 1, "hello world".len())
        );
        assert_eq!(f.matches[0].line_content, "hello world");
    }

    /// Oracle case 34 (`:396`): git confirmed the line but the (valid) regex
    /// matched nothing → whole-line fallback; the git-confirmed hit is NOT dropped.
    ///
    /// *Mutation (c):* removing the `if !accepted { … }` fallback leaves zero
    /// matches → this fails.
    #[test]
    fn git_oracle_34_git_confirmed_no_match_whole_line() {
        let mut acc = create_accumulator();
        let re = regex::Regex::new("nomatch").unwrap();
        let v = ingest_git_grep_line(
            "a.ts\u{0}3\u{0}git reported this line",
            "/r",
            Some(&re),
            &mut acc,
            100,
        );
        assert_eq!(v, Ingest::Continue);
        let f = only_file(&acc);
        assert_eq!(f.match_count, Some(1));
        assert_eq!(f.matches.len(), 1);
        assert_eq!(
            (f.matches[0].line, f.matches[0].column, f.matches[0].match_length),
            (3, 1, "git reported this line".len())
        );
        assert_eq!(f.matches[0].line_content, "git reported this line");
    }

    /// C1 for the git path: a >500-byte multibyte line routed through the
    /// whole-line clamp must snap window edges to char boundaries and never
    /// panic. Uses 400 × `é` (800 bytes) with a `None` regex (whole-line path).
    #[test]
    fn git_c1_long_multibyte_line_never_panics() {
        let mut acc = create_accumulator();
        let content = "é".repeat(400);
        let line = format!("a.ts\u{0}1\u{0}{content}");
        ingest_git_grep_line(&line, "/r", None, &mut acc, 100);
        let m = &only_file(&acc).matches[0];
        assert!(std::str::from_utf8(m.line_content.as_bytes()).is_ok());
        assert!(m.display_column.is_some(), "line >500 bytes → windowed");
    }

    // ── finalize oracle cases ───────────────────────────────────

    /// Insert a fully-formed file into an accumulator (helper for finalize tests),
    /// tracking `file_order`.
    fn seed_file(acc: &mut SearchAccumulator, key: &str, rel: &str, count: Option<i64>, n: usize) {
        acc.file_order.push(key.to_string());
        acc.file_map.insert(
            key.to_string(),
            SearchFileResult {
                file_path: key.to_string(),
                relative_path: rel.to_string(),
                match_count: count,
                matches: (1..=n)
                    .map(|line| SearchMatch {
                        line,
                        column: 1,
                        match_length: 3,
                        line_content: "foo".to_string(),
                        display_column: None,
                        display_match_length: None,
                    })
                    .collect(),
            },
        );
    }

    /// Oracle case 35 (`:415`): standard shape; `truncated` passes through.
    #[test]
    fn finalize_oracle_35_shape() {
        let mut acc = create_accumulator();
        seed_file(&mut acc, "/r/a.ts", "a.ts", Some(1), 1);
        acc.total_matches = 1;
        acc.truncated = true;
        let result = finalize(&acc);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].file_path, "/r/a.ts");
        assert_eq!(result.files[0].match_count, Some(1));
        assert_eq!(result.total_matches, 1);
        assert!(result.truncated);
    }

    /// Oracle case 36 (`:439`): empty-matches file is filtered; only `b.ts`
    /// survives.
    #[test]
    fn finalize_oracle_36_filters_empty_file() {
        let mut acc = create_accumulator();
        seed_file(&mut acc, "/r/a.ts", "a.ts", None, 0);
        seed_file(&mut acc, "/r/b.ts", "b.ts", None, 1);
        acc.total_matches = 1;
        let result = finalize(&acc);
        let rels: Vec<&str> = result.files.iter().map(|f| f.relative_path.as_str()).collect();
        assert_eq!(rels, vec!["b.ts"]);
    }

    /// Oracle case 37 (`:451`): missing / too-low per-file counts are normalized
    /// up to the real match count.
    #[test]
    fn finalize_oracle_37_normalizes_counts() {
        let mut acc = create_accumulator();
        seed_file(&mut acc, "/r/a.ts", "a.ts", None, 2);
        seed_file(&mut acc, "/r/b.ts", "b.ts", Some(0), 1);
        acc.total_matches = 3;
        let result = finalize(&acc);
        let got: Vec<(&str, Option<i64>)> = result
            .files
            .iter()
            .map(|f| (f.relative_path.as_str(), f.match_count))
            .collect();
        assert_eq!(got, vec![("a.ts", Some(2)), ("b.ts", Some(1))]);
    }

    /// Oracle case 38 (`:475`): a file claiming a non-zero `match_count` but with
    /// empty `matches` is filtered out entirely.
    #[test]
    fn finalize_oracle_38_filters_empty_despite_count() {
        let mut acc = create_accumulator();
        seed_file(&mut acc, "/r/a.ts", "a.ts", Some(2), 0);
        assert!(finalize(&acc).files.is_empty());
    }

    /// `finalize` MUST iterate `file_order` (insertion order), not the unordered
    /// `HashMap`. Seven files inserted in a deliberately non-alphabetical,
    /// non-hash order; the output order must match insertion exactly.
    ///
    /// *Mutation (e):* iterating `acc.file_map` (the `HashMap`) instead of
    /// `acc.file_order` yields a non-deterministic (here, wrong) order → this
    /// fails essentially always.
    #[test]
    fn finalize_preserves_file_order_not_hashmap() {
        let mut acc = create_accumulator();
        let order = ["zeta", "alpha", "mike", "bravo", "yankee", "charlie", "delta"];
        for name in order {
            seed_file(&mut acc, &format!("/r/{name}.ts"), &format!("{name}.ts"), None, 1);
        }
        acc.total_matches = order.len();
        let got: Vec<String> = finalize(&acc)
            .files
            .iter()
            .map(|f| f.relative_path.clone())
            .collect();
        let want: Vec<String> = order.iter().map(|n| format!("{n}.ts")).collect();
        assert_eq!(got, want);
    }
}
