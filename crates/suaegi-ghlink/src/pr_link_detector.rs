//! Chunk-boundary-safe GitHub PR URL scan over PTY output — a verbatim port
//! of Orca's `src/shared/terminal-github-pr-link-detector.ts` (@
//! v1.4.146-rc.0, 174 lines). This is M2 of 2; M1 (`github-links.ts`, this
//! crate's top-level module) supplies [`crate::parse_github_issue_or_pr_link`],
//! the only runtime dependency this module has on M1.
//!
//! Why shared upstream: terminal-side-effect-authority.md (slice 3) makes
//! Orca's main process emit `pr-link` facts from its per-PTY tracker for
//! local/SSH PTYs, while the renderer keeps byte-scanning for remote-runtime
//! PTYs and the kill-switch-off path. Both paths must share the carry/dedupe
//! semantics or links split across chunks would resolve differently per
//! authority mode — see the parity fixture ported below
//! (`oracle_parity_derives_identical_pr_link_facts_from_split_and_repeated_urls`).
//!
//! API shape mirrors [`crate` sibling] `suaegi-term`'s
//! `KittyKeyboardModeTracker`: a struct holding carry state, `observe`, and
//! `reset`. Unlike that tracker, `reset` here clears TWO independent pieces
//! of state (`carry` AND `seen_urls`) — see Q1/Q2 below for why both are
//! load-bearing, not just one of them for "completeness".
//!
//! # Documented decisions/traps (plan `2026-07-27-pr-link-detector-m2.md`, Q1-Q15)
//!
//! - **Q1 — the carry retains the WHOLE URL, not an unconsumed suffix.**
//!   [`get_potential_github_pr_carry`] slices from the last scheme occurrence
//!   to the end of the buffer (`O:65`) — there is no "consumed up to here"
//!   cursor. So the next call's `raw_combined` re-contains an
//!   already-emitted URL verbatim, and [`TerminalGitHubPRLinkDetector::seen_urls`]
//!   is what prevents it from being emitted twice. Trimming the carry to
//!   "only the unconsumed part" would still pass every oracle test while
//!   silently changing this behavior — do not "clean it up".
//! - **Q2 — the oracle's own "does not repeat ... from overlapping carry
//!   text" test (`test:116-121`) does not pin what its name claims.** Its
//!   chunk 1 ends in `\n`, which is JS whitespace inside the scheme tail, so
//!   [`has_terminal_url_whitespace`] clears the carry and chunk 2 starts
//!   empty — meaning EITHER deleting `seen_urls` OR deleting the
//!   whitespace-clearing rule still passes that test (each one masks the
//!   other's absence in that fixture). The genuine cross-call dedupe oracle
//!   lives in a different file entirely — see
//!   `oracle_parity_derives_identical_pr_link_facts_from_split_and_repeated_urls`
//!   below. This module additionally isolates each mechanism with its own
//!   pin: `q2_seen_set_suppresses_reemission_from_retained_carry` (uses a
//!   non-whitespace terminator, `>`, so the carry stays non-empty and ONLY
//!   the seen-set prevents a repeat) and
//!   `q2_whitespace_rule_clears_carry_directly` (calls
//!   [`get_potential_github_pr_carry`] directly, bypassing `seen_urls`
//!   entirely, since any input where the whitespace rule matters is, by
//!   construction, a URL already resolved+deduped within the same call —
//!   see the doc comment on that test for why a seen-set-free unit test is
//!   the only way to isolate this mechanism).
//! - **Q3 — the dedupe key is the raw candidate string, and the set grows
//!   unboundedly.** It is sensitive to scheme, host case, trailing path
//!   segments, and leading zeros, so `https://…/pull/42`, `http://…/pull/42`,
//!   `…/pull/42/files`, and `…/pull/007` are FOUR distinct keys and emit FOUR
//!   links for what a human would call two PRs. Do not "improve" this to a
//!   `(slug, number)` key — a consumer treats a `pr-link` event as "latest
//!   associated update", not a set membership fact. [`TerminalGitHubPRLinkDetector::reset`]
//!   is the only way to clear it, and it must clear both `carry` and
//!   `seen_urls`.
//! - **Q4 — both `\s` tests (`O:73`, `O:97`) are ECMAScript whitespace, not
//!   Rust's** → [`suaegi_misc::is_js_whitespace`], never `char::is_whitespace`.
//!   They disagree in OPPOSITE directions at two code points: U+00A0/U+3000
//!   are ECMAScript whitespace (JS terminates a URL there) but Rust
//!   `char::is_whitespace` also happens to agree on those — the genuinely
//!   divergent points are U+FEFF (ECMAScript whitespace, NOT Unicode
//!   `White_Space`) and U+0085/NEL (Unicode `White_Space`, NOT ECMAScript
//!   whitespace).
//! - **Q5 — the fast-path bail gate (`O:142`) runs on `raw_combined`, BEFORE
//!   ANSI stripping.** A `/pull/` split by an SGR sequence (e.g.
//!   `/pu\x1b[0mll/`) bails out here even though stripping would join the two
//!   halves — the source comment (`O:140-141`) says this is a deliberate
//!   hot-path shortcut for PTY throughput. A port that strips first and then
//!   gates would emit links Orca never does.
//! - **Q6 — the carry (`O:171`) is recomputed from `raw_combined`, NOT the
//!   stripped `combined`.** That is what lets an SGR sequence split across a
//!   chunk boundary (`…/pull/10` + `\x1b` ‖ `[22m\n`) survive: the carry keeps
//!   the raw, unstripped tail (including the dangling `\x1b`), and the next
//!   chunk's `raw_combined` is stripped fresh as a whole.
//! - **Q7 — the three cursor-control guard members are NOT interchangeable.**
//!   Without the guard, `\x08` (BS) is not ECMAScript whitespace, so it would
//!   be silently absorbed into the candidate (fusing digits); `\x0b`/`\x0c`
//!   (VT/FF) ARE ECMAScript whitespace, so they would incorrectly terminate
//!   the URL early (yielding a truncated PR number). The oracle bundles all
//!   three into one `for` loop (`test:62-68`), so a mutant that drops only
//!   one member from the guard class survives it — pinned separately below
//!   (`q7_backspace_control_char_rejected`, `q7_vertical_tab_control_char_rejected`,
//!   `q7_form_feed_control_char_rejected`).
//! - **Q8 — the U+FFFD guard also rejects a genuine U+FFFD.** node-pty's
//!   UTF-8 decoder emits U+FFFD on invalid byte sequences (`O:34`), so a real
//!   U+FFFD can reach this function with no cursor-control byte involved at
//!   all. Zero oracle coverage; pinned in `q8_genuine_replacement_character_is_rejected`.
//! - **Q9 — a candidate reaching the end of the combined buffer is NOT
//!   emitted this call** (`O:159-161`) — it waits for the next chunk to
//!   supply a terminator, even if it would otherwise parse successfully.
//! - **Q10 — both length caps are UTF-16 CODE UNITS**, not bytes and not
//!   `char` counts: carry `MAX_CARRY_LENGTH` = 512 (`O:20`), URL
//!   `MAX_TERMINAL_GITHUB_PR_URL_LENGTH` = 2048 (`O:21`). We use
//!   `.encode_utf16().count()` at both cap sites — never `.len()` (bytes) or
//!   `.chars().count()`. The oracle's own overshoot test uses 10 000 ASCII
//!   characters, which cannot distinguish any of the three metrics (they all
//!   agree on pure ASCII); the pins below use astral (surrogate-pair)
//!   characters specifically so byte length, char count, and UTF-16 length
//!   all disagree, at the exact 511/512/513 and 2047/2048/2049 boundaries.
//! - **Q11 — `ends_with_http_scheme_prefix_fragment` (`O:45-54`) has ZERO
//!   positive oracle coverage.** It is reachable on every oracle call (it's
//!   the fallback of [`get_potential_github_pr_carry`] whenever no full
//!   scheme substring is present) but no existing oracle case ever produces
//!   a non-empty carry through it. Pinned in
//!   `q11_partial_scheme_prefix_carried_across_chunk_boundary` (`…creating h`
//!   ‖ `ttps://…/pull/1\n`) and directly in
//!   `q11_ends_with_http_scheme_prefix_fragment_positive_cases`. The
//!   `http://`-only branch is reachable only when the trailing fragment is
//!   exactly `http:` or `http:/` — any shorter suffix (`h`, `ht`, `htt`,
//!   `http`) is also a prefix of `https://` and matches there first, since
//!   `HTTP_SCHEME_PREFIXES` is checked in order and `"https://"` comes first.
//! - **Q12 — the trailing-punctuation trim (`O:29-31`, called at `O:37`)
//!   happens AFTER the `/pull/` and length filters** (which run inside
//!   [`iterate_terminal_url_candidates`], evaluated before `O:163` ever
//!   calls `parse_terminal_github_pr_url`/trim). An over-length candidate
//!   ending in `))))` is dropped even though the TRIMMED url would fit under
//!   the cap — pinned in `q12_trailing_punctuation_trim_happens_after_length_filter`.
//!   The trim class is exactly `)`, `,`, `.`, `;`, `]`, `}` — no opening
//!   brackets, no `>` or `"` (those are terminators, handled separately, not
//!   trimmed).
//! - **Q13 — every string index in the TS source is a UTF-16 code unit**
//!   (`O:48`, `:49`, `:65`, `:120`). Rust byte indexing on `&str` panics on a
//!   non-boundary, so all scanning here walks `char_indices()`/`chars()`
//!   (safe against multi-byte UTF-8) while the two LENGTH CAPS (Q10) are
//!   computed via `encode_utf16().count()`. Unlike this crate's sibling
//!   terminal byte-scanners (`bell_detector`, `partial_escape_tail` in
//!   `suaegi-term`), the "every byte that matters is ASCII" justification for
//!   pure byte-native scanning does NOT hold here: non-ASCII ECMAScript
//!   whitespace (U+00A0, U+3000, U+FEFF, …) is semantically load-bearing
//!   (Q4), and the UTF-16 caps (Q10) are observably different from a byte or
//!   `char` count. The ANSI SGR-strip and cursor-control-guard passes ARE
//!   safe to reason about byte-wise in principle (their pattern bytes are all
//!   ASCII, so they cannot straddle a multi-byte UTF-8 sequence), but this
//!   port scans them via `chars()` too, for one uniform, easily-audited
//!   scanning discipline across the whole module.
//! - **Q14 — the oracle's `matchAll` spy test (`test:165-178`) has no Rust
//!   analogue** (there is nothing to spy on `String.prototype.matchAll`
//!   with). Its *intent* — a huge chunk of `/pull/`-containing noise plus one
//!   real PR URL yields exactly one result, without pathological
//!   backtracking — is ported as
//!   `oracle_scans_huge_terminal_chunks_containing_pull_markers_without_global_regex_iteration`,
//!   dropping only the spy assertion.
//! - **Q15 — API shape mirrors `KittyKeyboardModeTracker`**: a struct with
//!   carry state, `observe(&mut self, &str) -> Vec<...>`, and
//!   `reset(&mut self)`. `reset` is the Rust spelling of Orca recreating the
//!   detector closure (`terminal-output-side-effects.ts:314`), which is
//!   documented upstream as losing all dedupe memory — mirrored here by
//!   clearing BOTH `carry` and `seen_urls`.
//!
//! No `regex` dependency (plan §1/crate charter): both ANSI patterns
//! (`TERMINAL_SGR_PATTERN`, `TERMINAL_CURSOR_CONTROL_PATTERN`) are trivial
//! character-class scans, and the trailing-punctuation strip is a
//! `trim_end_matches`.

use crate::{GitHubItemKind, RepoSlug, parse_github_issue_or_pr_link};
use std::collections::HashSet;
use suaegi_misc::is_js_whitespace;

/// `GITHUB_PR_PATH_MARKER` (`O:14`).
const GITHUB_PR_PATH_MARKER: &str = "/pull/";

/// `TERMINAL_CONTROL_GUARD` (`O:17`) — also the literal replacement character
/// node-pty's UTF-8 decoder emits on invalid bytes (Q8).
const TERMINAL_CONTROL_GUARD: char = '\u{fffd}';

/// `HTTP_SCHEME_PREFIXES` (`O:18`). Order matters: `"https://"` is always
/// tried before `"http://"` wherever these are scanned in order (Q11).
const HTTP_SCHEME_PREFIXES: [&str; 2] = ["https://", "http://"];

/// `MAX_CARRY_LENGTH` (`O:20`) — UTF-16 code units (Q10).
const MAX_CARRY_LENGTH: usize = 512;

/// `MAX_TERMINAL_GITHUB_PR_URL_LENGTH` (`O:21`) — UTF-16 code units (Q10).
const MAX_TERMINAL_GITHUB_PR_URL_LENGTH: usize = 2048;

/// A GitHub PR URL observed in terminal output.
///
/// Mirrors Orca's `TerminalGitHubPRLink` (`O:23-27`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalGitHubPRLink {
    pub url: String,
    pub slug: RepoSlug,
    pub number: u64,
}

/// UTF-16 code unit count of `s` (Q10). Never substitute `.len()` (bytes) or
/// `.chars().count()` here — both disagree from UTF-16 on non-BMP/multi-byte
/// input, and the two length caps are pinned exactly at this boundary.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `trimTerminalUrl` (`O:29-31`). The trim class is exactly `)`, `,`, `.`,
/// `;`, `]`, `}` (`TRAILING_TERMINAL_PUNCTUATION_RE`, `O:19`) — no opening
/// brackets, no `>` or `"` (Q12).
fn trim_terminal_url(candidate: &str) -> &str {
    candidate.trim_end_matches([')', ',', '.', ';', ']', '}'])
}

/// `parseTerminalGitHubPRUrl` (`O:33-43`). Rejects any candidate still
/// containing a raw ESC or the U+FFFD guard character (Q8) BEFORE trimming
/// or parsing.
fn parse_terminal_github_pr_url(candidate: &str) -> Option<TerminalGitHubPRLink> {
    if candidate.contains('\u{1b}') || candidate.contains(TERMINAL_CONTROL_GUARD) {
        return None;
    }
    let url = trim_terminal_url(candidate);
    let parsed = parse_github_issue_or_pr_link(url)?;
    if parsed.kind != GitHubItemKind::Pr {
        return None;
    }
    Some(TerminalGitHubPRLink {
        url: url.to_string(),
        slug: parsed.slug,
        number: parsed.number,
    })
}

/// `endsWithHttpSchemePrefixFragment` (`O:45-54`). Tries `"https://"` first,
/// then `"http://"`; within each prefix, tries the LONGEST fragment first
/// (Q11). Zero positive oracle coverage — see the module doc and the `q11_*`
/// pins.
fn ends_with_http_scheme_prefix_fragment(value: &str) -> String {
    for prefix in HTTP_SCHEME_PREFIXES {
        // `prefix` is ASCII-only, so its UTF-16 length, char count, and byte
        // length all coincide — plain byte slicing of `prefix` is safe here.
        let max_len = prefix.len() - 1;
        for length in (1..=max_len).rev() {
            let fragment = &prefix[..length];
            if value.ends_with(fragment) {
                return fragment.to_string();
            }
        }
    }
    String::new()
}

/// `hasTerminalUrlWhitespace` (`O:71-78`), inlined as a scan over `tail`
/// (the JS version takes `start`/`end` indices into a larger string; here the
/// caller already slices the exact tail to scan). ECMAScript whitespace
/// (Q4), not Rust's `char::is_whitespace`.
fn has_terminal_url_whitespace(tail: &str) -> bool {
    tail.chars().any(is_js_whitespace)
}

/// `getPotentialGitHubPRCarry` (`O:56-69`). ⚠ Retains the WHOLE URL from the
/// last scheme occurrence to the end of `value` (Q1) — there is no
/// "unconsumed suffix" cursor. Do not "clean up" this to a shorter suffix;
/// see the module doc's Q1/Q2 discussion for why that would be an
/// undetected-by-the-oracle behavior change.
fn get_potential_github_pr_carry(value: &str) -> String {
    // `value.lastIndexOf(prefix)` for each prefix, then `Math.max(...)`
    // (`O:57`). Byte-offset comparison agrees with UTF-16-index comparison
    // here because both are monotonic in string position, and each prefix's
    // occurrence start is always a valid `&str` char boundary (the prefixes
    // are ASCII literals).
    let scheme_index = HTTP_SCHEME_PREFIXES.iter().filter_map(|p| value.rfind(p)).max();

    if let Some(idx) = scheme_index {
        let tail = &value[idx..];
        if utf16_len(tail) > MAX_CARRY_LENGTH {
            return String::new();
        }
        if has_terminal_url_whitespace(tail) {
            return String::new();
        }
        return tail.to_string();
    }

    ends_with_http_scheme_prefix_fragment(value)
}

/// `isTerminalUrlTerminator` (`O:96-98`). The `"`/`'`/`<`/`>` arms plus
/// ECMAScript whitespace (Q4).
fn is_terminal_url_terminator(ch: char) -> bool {
    ch == '"' || ch == '\'' || ch == '<' || ch == '>' || is_js_whitespace(ch)
}

/// `findNextHttpSchemeIndex` (`O:85-94`). Returns the EARLIEST occurrence of
/// either scheme at or after `start` (a byte offset that must be a valid
/// char boundary) — not just the earliest occurrence of whichever prefix is
/// tried first.
fn find_next_http_scheme_index(value: &str, start: usize) -> Option<usize> {
    let mut next: Option<usize> = None;
    for prefix in HTTP_SCHEME_PREFIXES {
        if let Some(rel) = value[start..].find(prefix) {
            let idx = start + rel;
            next = Some(match next {
                None => idx,
                Some(n) => n.min(idx),
            });
        }
    }
    next
}

/// `findTerminalUrlCandidateEnd` (`O:100-108`). Scans forward from `start`
/// (a byte offset) for a terminator char, capped at
/// `MAX_TERMINAL_GITHUB_PR_URL_LENGTH + 1` UTF-16 code units (Q10, Q13) —
/// tracked here via `char::len_utf16()` rather than a UTF-16 index, since we
/// walk `char_indices()` for panic-safety (Q13). Returns the byte offset of
/// the first terminator found, or the byte offset where the cap was reached,
/// or `value.len()` if neither happens before the string ends.
fn find_terminal_url_candidate_end(value: &str, start: usize) -> usize {
    let mut consumed_units: usize = 0;
    for (byte_offset, ch) in value[start..].char_indices() {
        let abs_offset = start + byte_offset;
        if consumed_units > MAX_TERMINAL_GITHUB_PR_URL_LENGTH {
            return abs_offset;
        }
        if is_terminal_url_terminator(ch) {
            return abs_offset;
        }
        consumed_units += ch.len_utf16();
    }
    value.len()
}

/// `TerminalUrlCandidate` (`O:80-83`).
struct TerminalUrlCandidate {
    raw_url: String,
    /// Byte offset into the scanned `value`, one past the candidate's last
    /// character (i.e. where the terminator — or end of buffer — begins).
    end_index: usize,
}

/// `iterateTerminalUrlCandidates` (`O:110-131`), materialized into a `Vec`
/// instead of a generator (Rust has no first-class generators; the JS
/// generator is fully drained by its only caller anyway, so this is
/// behaviorally identical).
fn iterate_terminal_url_candidates(value: &str) -> Vec<TerminalUrlCandidate> {
    let mut candidates = Vec::new();
    let mut search_start = 0usize;

    while search_start < value.len() {
        let Some(candidate_start) = find_next_http_scheme_index(value, search_start) else {
            return candidates;
        };

        let candidate_end = find_terminal_url_candidate_end(value, candidate_start);
        let raw_url = &value[candidate_start..candidate_end];
        // `Math.max(candidateEnd, candidateStart + 1)` (`O:121`). The scheme
        // prefixes both start with the ASCII byte `h`, so `candidate_start +
        // 1` is always a valid char boundary one UTF-16 unit (and one byte)
        // past `candidate_start` — no separate UTF-16-aware step needed.
        search_start = candidate_end.max(candidate_start + 1);

        if utf16_len(raw_url) > MAX_TERMINAL_GITHUB_PR_URL_LENGTH || !raw_url.contains(GITHUB_PR_PATH_MARKER) {
            continue;
        }

        candidates.push(TerminalUrlCandidate {
            raw_url: raw_url.to_string(),
            end_index: candidate_end,
        });
    }

    candidates
}

/// `TERMINAL_SGR_PATTERN` (`O:15`): `\x1b\[[0-?]*[ -/]*m`, hand-rolled as a
/// char scan (no `regex`, per crate charter). `[0-?]` is the CSI parameter
/// byte range 0x30-0x3F; `[ -/]` is the CSI intermediate byte range
/// 0x20-0x2F; both ranges and the terminator `m` are pure ASCII, so this
/// greedy, unambiguous match is safe to reason about per-byte in principle,
/// but is implemented via `chars()` for one uniform scanning discipline
/// (Q13).
fn match_sgr_sequence(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start) != Some(&'\u{1b}') {
        return None;
    }
    if chars.get(start + 1) != Some(&'[') {
        return None;
    }
    let mut j = start + 2;
    while chars.get(j).is_some_and(|&c| ('0'..='?').contains(&c)) {
        j += 1;
    }
    while chars.get(j).is_some_and(|&c| (' '..='/').contains(&c)) {
        j += 1;
    }
    if chars.get(j) == Some(&'m') { Some(j + 1) } else { None }
}

/// `.replace(TERMINAL_SGR_PATTERN, '')` (`O:149`).
fn strip_terminal_sgr_sequences(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(match_end) = match_sgr_sequence(&chars, i) {
            i = match_end;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `.replace(TERMINAL_CURSOR_CONTROL_PATTERN, TERMINAL_CONTROL_GUARD)`
/// (`O:150`). `TERMINAL_CURSOR_CONTROL_PATTERN` (`O:16`) is `[\x08\x0b\x0c]`
/// — BS/VT/FF. These three are NOT interchangeable in effect; see Q7 in the
/// module doc.
fn replace_cursor_controls_with_guard(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\u{08}' | '\u{0b}' | '\u{0c}') {
            out.push(TERMINAL_CONTROL_GUARD);
        } else {
            out.push(ch);
        }
    }
    out
}

/// Chunk-boundary-safe scanner for GitHub PR URLs in raw PTY output.
///
/// Port of Orca's `createTerminalGitHubPRLinkDetector` (`O:133-173`). See the
/// module doc for the full Q1-Q15 decision log. API shape mirrors
/// `suaegi_term::KittyKeyboardModeTracker` (Q15).
#[derive(Debug, Default)]
pub struct TerminalGitHubPRLinkDetector {
    carry: String,
    seen_urls: HashSet<String>,
}

impl TerminalGitHubPRLinkDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan one PTY chunk, returning any newly-observed (not yet emitted) PR
    /// links.
    ///
    /// `O:137-173`.
    pub fn observe(&mut self, data: &str) -> Vec<TerminalGitHubPRLink> {
        // `carry ? carry + data : data` (`O:138`) — plain concatenation is
        // behaviorally identical when `carry` is empty (`"" + data == data`),
        // so there is no need to special-case it.
        let raw_combined = format!("{}{data}", self.carry);

        // Q5: this gate runs on `raw_combined`, BEFORE ANSI stripping —
        // deliberately, per the source's own "hot path" comment (`O:140-141`).
        if !raw_combined.contains(GITHUB_PR_PATH_MARKER) {
            self.carry = get_potential_github_pr_carry(&raw_combined);
            return Vec::new();
        }

        // `O:148-150`: strip SGR sequences, then guard cursor controls.
        let combined = replace_cursor_controls_with_guard(&strip_terminal_sgr_sequences(&raw_combined));

        let mut links = Vec::new();
        for candidate in iterate_terminal_url_candidates(&combined) {
            // Q9: a candidate reaching the end of the buffer waits for the
            // next chunk instead of being emitted now.
            if candidate.end_index == combined.len() {
                continue;
            }

            let Some(parsed) = parse_terminal_github_pr_url(&candidate.raw_url) else {
                continue;
            };
            if self.seen_urls.contains(&parsed.url) {
                continue;
            }
            self.seen_urls.insert(parsed.url.clone());
            links.push(parsed);
        }

        // Q6: recomputed from `raw_combined`, NOT `combined` — this is what
        // lets an ANSI sequence split across a chunk boundary survive.
        self.carry = get_potential_github_pr_carry(&raw_combined);
        links
    }

    /// Clears BOTH `carry` and `seen_urls` (Q1/Q3/Q15) — the Rust spelling of
    /// Orca recreating the detector closure, which is documented upstream as
    /// losing all dedupe memory.
    pub fn reset(&mut self) {
        self.carry.clear();
        self.seen_urls.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(owner: &str, repo: &str) -> RepoSlug {
        RepoSlug {
            owner: owner.to_string(),
            repo: repo.to_string(),
        }
    }

    fn pr(url: &str, owner: &str, repo: &str, number: u64) -> TerminalGitHubPRLink {
        TerminalGitHubPRLink {
            url: url.to_string(),
            slug: slug(owner, repo),
            number,
        }
    }

    // ==== Oracle: terminal-github-pr-link-detector.test.ts, ported verbatim (17 cases) ====

    #[test]
    fn oracle_extracts_github_pull_request_urls_from_terminal_output() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(
            d.observe("Created https://github.com/acme/orca/pull/42\r\n"),
            vec![pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42)]
        );
    }

    #[test]
    fn oracle_detects_issue_8126_claude_code_pr_links_with_attached_ansi_reset() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let issue8126_url = "https://github.com/owner/repo/pull/10";
        assert_eq!(
            d.observe(&format!("{issue8126_url}\x1b[22m\n")),
            vec![pr(issue8126_url, "owner", "repo", 10)]
        );
    }

    #[test]
    fn oracle_strips_an_ansi_reset_split_across_pty_chunks() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let issue8126_url = "https://github.com/owner/repo/pull/10";
        assert_eq!(d.observe(&format!("{issue8126_url}\x1b")), Vec::new());
        assert_eq!(d.observe("[22m\n"), vec![pr(issue8126_url, "owner", "repo", 10)]);
    }

    #[test]
    fn oracle_rejects_pr_urls_corrupted_by_cursor_movement() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/owne\x1b[1Cr/repo/pull/10\n"), Vec::new());
    }

    #[test]
    fn oracle_rejects_pr_urls_fused_across_terminal_rows() {
        for cursor_move in ["\x1b[1A", "\x1b[1B"] {
            let mut d = TerminalGitHubPRLinkDetector::default();
            assert_eq!(
                d.observe(&format!("https://github.com/owner/repo/pull/{cursor_move}10\n")),
                Vec::new()
            );
        }
    }

    #[test]
    fn oracle_does_not_fuse_screen_editing_controls_into_pr_urls() {
        for screen_edit in ["\x08", "\x0b", "\x0c", "\x1bD", "\x1b[2J", "\x1b[2K", "\x1b[1S"] {
            let mut d = TerminalGitHubPRLinkDetector::default();
            assert_eq!(
                d.observe(&format!("https://github.com/owner/repo/pull/1{screen_edit}0\n")),
                Vec::new()
            );
        }
    }

    #[test]
    fn oracle_deduplicates_styled_and_plain_instances() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let issue8126_url = "https://github.com/owner/repo/pull/10";
        assert_eq!(
            d.observe(&format!("{issue8126_url}\x1b[22m\n{issue8126_url}\n")),
            vec![pr(issue8126_url, "owner", "repo", 10)]
        );
    }

    #[test]
    fn oracle_waits_for_a_boundary_when_the_url_is_split_across_pty_chunks() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/acme/orca/pull/4"), Vec::new());
        assert_eq!(
            d.observe("2\r\n"),
            vec![pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42)]
        );
    }

    #[test]
    fn oracle_detects_a_url_split_inside_the_github_prefix() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("created https://gith"), Vec::new());
        assert_eq!(
            d.observe("ub.com/acme/orca/pull/42\n"),
            vec![pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42)]
        );
    }

    #[test]
    fn oracle_trims_terminal_punctuation_around_printed_urls() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let result = d.observe("Opened (https://github.com/acme/orca/pull/42).\n");
        assert_eq!(result[0].url, "https://github.com/acme/orca/pull/42");
    }

    #[test]
    fn oracle_does_not_repeat_the_same_pr_url_from_overlapping_carry_text() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42\n").len(), 1);
        assert_eq!(d.observe("more output\n"), Vec::new());
    }

    #[test]
    fn oracle_ignores_non_pr_github_shaped_links() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/acme/orca/issues/42\n"), Vec::new());
    }

    #[test]
    fn oracle_extracts_github_enterprise_pull_request_urls_from_terminal_output() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(
            d.observe("Created https://github.my-company.net/MyOrg/my_repo/pull/395\r\n"),
            vec![pr(
                "https://github.my-company.net/MyOrg/my_repo/pull/395",
                "MyOrg",
                "my_repo",
                395
            )]
        );
    }

    #[test]
    fn oracle_extracts_http_github_enterprise_pull_request_urls_from_terminal_output() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(
            d.observe("Created http://github.internal/MyOrg/my_repo/pull/395\r\n"),
            vec![pr("http://github.internal/MyOrg/my_repo/pull/395", "MyOrg", "my_repo", 395)]
        );
    }

    #[test]
    fn oracle_extracts_github_enterprise_pull_request_urls_with_a_custom_port() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(
            d.observe("Created https://github.internal:8443/MyOrg/my_repo/pull/397\r\n"),
            vec![pr(
                "https://github.internal:8443/MyOrg/my_repo/pull/397",
                "MyOrg",
                "my_repo",
                397
            )]
        );
    }

    /// Q14: the oracle's original spies on `String.prototype.matchAll` (no
    /// Rust analogue); only the behavioral intent is ported — a huge
    /// `/pull/`-containing noise chunk plus one real PR URL yields exactly
    /// one result.
    #[test]
    fn oracle_scans_huge_terminal_chunks_containing_pull_markers_without_global_regex_iteration() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let noise = format!("{}\n", "/pull/not-a-url ".repeat(20_000));
        assert_eq!(
            d.observe(&format!("{noise}Created https://github.com/acme/orca/pull/42\r\n")),
            vec![pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42)]
        );
    }

    #[test]
    fn oracle_drops_overlong_incomplete_url_carry_instead_of_retaining_pasted_megabytes() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(
            d.observe(&format!("https://github.com/acme/orca/pull/{}", "4".repeat(10_000))),
            Vec::new()
        );
        assert_eq!(d.observe("2\r\n"), Vec::new());
    }

    // ==== Parity fixture: terminal-title-tracker-parity.test.ts:265-274 ====
    // The ONLY genuine cross-call dedupe oracle (see Q2 module doc).

    #[test]
    fn oracle_parity_derives_identical_pr_link_facts_from_split_and_repeated_urls() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let mut events = Vec::new();
        events.extend(d.observe("Created https://github.com/acme/orca/pull/4"));
        events.extend(d.observe("2\r\nAlso https://github.com/acme/orca/pull/43 merged\r\n"));
        events.extend(d.observe("again https://github.com/acme/orca/pull/42\r\n"));

        assert_eq!(
            events,
            vec![
                pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42),
                pr("https://github.com/acme/orca/pull/43", "acme", "orca", 43),
            ]
        );
    }

    // ==== Additional pins (oracle-silent branches, plan §2) ====

    /// Q2, seen-set isolation: `>` is NOT ECMAScript whitespace, so the
    /// carry after the first call retains the whole URL verbatim (Q1) and
    /// the second call's combined buffer re-contains the identical URL text.
    /// The whitespace-clearing rule is entirely uninvolved here (`>` is
    /// never whitespace, correct or mutated) — only `seen_urls` prevents the
    /// second call from re-emitting.
    #[test]
    fn q2_seen_set_suppresses_reemission_from_retained_carry() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let first = d.observe("https://github.com/acme/orca/pull/42>");
        assert_eq!(first, vec![pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42)]);

        let second = d.observe("\n");
        assert_eq!(second, Vec::new());
    }

    /// Q2, whitespace-rule isolation: calls `get_potential_github_pr_carry`
    /// directly, bypassing `seen_urls` entirely. This is necessary because,
    /// at the public `observe()` level, ANY input where the whitespace rule
    /// clears the carry is (by construction) a URL that was ALREADY fully
    /// resolved and emitted within the same call — so a mutant that deletes
    /// the whitespace check would only ever cause the SAME already-seen URL
    /// to be rescanned next call, which `seen_urls` would suppress anyway
    /// (masking the mutant, exactly as the oracle's own `test:116-121`
    /// does). Testing the pure carry function directly is the only way to
    /// isolate this mechanism.
    #[test]
    fn q2_whitespace_rule_clears_carry_directly() {
        assert_eq!(get_potential_github_pr_carry("https://github.com/acme/orca/pull/42\n"), "");
        assert_eq!(
            get_potential_github_pr_carry("https://github.com/acme/orca/pull/42>"),
            "https://github.com/acme/orca/pull/42>"
        );
    }

    /// Q11: positive coverage for the partial-scheme carry helper — zero
    /// oracle coverage upstream despite being called on every observe().
    #[test]
    fn q11_partial_scheme_prefix_carried_across_chunk_boundary() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("creating h"), Vec::new());
        assert_eq!(
            d.observe("ttps://github.com/acme/orca/pull/1\n"),
            vec![pr("https://github.com/acme/orca/pull/1", "acme", "orca", 1)]
        );
    }

    /// Q11: direct pins on `ends_with_http_scheme_prefix_fragment`, including
    /// the note that `http:`/`http:/` are the only suffixes that reach the
    /// `"http://"`-specific branch — any shorter suffix is also a prefix of
    /// `"https://"` and matches there first.
    #[test]
    fn q11_ends_with_http_scheme_prefix_fragment_positive_cases() {
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating h"), "h");
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating ht"), "ht");
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating htt"), "htt");
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating http"), "http");
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating https"), "https");
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating https:"), "https:");
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating https:/"), "https:/");
        // Only reachable via the "http://"-specific branch (Q11 note).
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating http:"), "http:");
        assert_eq!(ends_with_http_scheme_prefix_fragment("creating http:/"), "http:/");
        assert_eq!(ends_with_http_scheme_prefix_fragment("no scheme here"), "");
    }

    /// Q7: BS, VT, and FF pinned SEPARATELY (the oracle bundles all three
    /// into one loop, so a mutant dropping a single member from the guard
    /// class survives it).
    #[test]
    fn q7_backspace_control_char_rejected() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/owner/repo/pull/1\x080\n"), Vec::new());
    }

    #[test]
    fn q7_vertical_tab_control_char_rejected() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/owner/repo/pull/1\x0b0\n"), Vec::new());
    }

    #[test]
    fn q7_form_feed_control_char_rejected() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/owner/repo/pull/1\x0c0\n"), Vec::new());
    }

    /// Q8: a genuine U+FFFD (not produced by the cursor-control guard) must
    /// also be rejected — untested upstream, but reachable via node-pty's
    /// UTF-8 decoder on invalid byte sequences.
    #[test]
    fn q8_genuine_replacement_character_is_rejected() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/owner/repo/pull/1\u{fffd}0\n"), Vec::new());
    }

    /// Q4: non-ASCII ECMAScript whitespace terminators (NBSP, ideographic
    /// space, BOM) each correctly terminate a PR URL scan.
    #[test]
    fn q4_non_ascii_ecmascript_whitespace_terminates_pr_url() {
        for ws in ['\u{a0}', '\u{3000}', '\u{feff}'] {
            let mut d = TerminalGitHubPRLinkDetector::default();
            let input = format!("https://github.com/acme/orca/pull/42{ws}x\n");
            assert_eq!(
                d.observe(&input),
                vec![pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42)],
                "ws={ws:?}"
            );
        }
    }

    /// Q4: U+0085 (NEL) is Unicode `White_Space` but NOT ECMAScript
    /// whitespace, so it must NOT terminate the scan — unlike the three
    /// characters above. Embedding it mid-candidate does not split the URL
    /// there; scanning continues to the real terminator (`\n`), and the
    /// `url` crate percent-encodes the embedded C1 control in the path
    /// (same WHATWG behavior documented for P1/P2 in `lib.rs`), which breaks
    /// the digit match entirely — so the candidate parses as NO link, rather
    /// than yielding a truncated PR 1. (Empirically confirmed: this is the
    /// actual output, not a guess — see the crux comment below for what a
    /// terminator-mutant would produce instead.)
    #[test]
    fn q4_nel_is_not_an_ecmascript_whitespace_terminator() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let input = "https://github.com/owner/repo/pull/1\u{85}0\n";
        // Crux: if NEL were (incorrectly) treated as whitespace, this would
        // terminate right after "1" and yield `vec![pr(".../pull/1", ..., 1)]`
        // instead of the empty result pinned here.
        assert_eq!(d.observe(input), Vec::new());
    }

    /// Q10: the carry cap boundary (511/512/513) is measured in UTF-16 code
    /// units. Constructed with astral (surrogate-pair, 2 UTF-16 units per
    /// char) filler so byte length, `char` count, and UTF-16 length all
    /// disagree at every boundary — a mutant using `.len()` or
    /// `.chars().count()` instead of `.encode_utf16().count()` fails at
    /// least one of the three assertions below.
    #[test]
    fn q10_carry_cap_boundary_is_utf16_code_units() {
        fn build_tail(total_units_after_scheme: usize) -> String {
            let astral_count = total_units_after_scheme / 2;
            let remainder = total_units_after_scheme % 2;
            let mut s = String::from("https://");
            s.push_str(&"\u{1f600}".repeat(astral_count));
            if remainder == 1 {
                s.push('4');
            }
            s
        }

        let under = build_tail(MAX_CARRY_LENGTH - 1 - 8);
        let at = build_tail(MAX_CARRY_LENGTH - 8);
        let over = build_tail(MAX_CARRY_LENGTH + 1 - 8);

        assert_eq!(utf16_len(&under), MAX_CARRY_LENGTH - 1);
        assert_eq!(utf16_len(&at), MAX_CARRY_LENGTH);
        assert_eq!(utf16_len(&over), MAX_CARRY_LENGTH + 1);

        assert_eq!(get_potential_github_pr_carry(&under), under);
        assert_eq!(get_potential_github_pr_carry(&at), at);
        assert_eq!(get_potential_github_pr_carry(&over), "");
    }

    /// Q10: the URL length cap boundary (2047/2048/2049), same astral-filler
    /// technique, exercised through `iterate_terminal_url_candidates`
    /// directly (independent of full GitHub-link parseability).
    #[test]
    fn q10_url_length_cap_boundary_is_utf16_code_units() {
        fn build_units(units: usize) -> String {
            let astral_count = units / 2;
            let remainder = units % 2;
            let mut s = String::new();
            s.push_str(&"\u{1f600}".repeat(astral_count));
            if remainder == 1 {
                s.push('4');
            }
            s
        }

        let scheme = "https://";
        let marker_and_number = "/pull/1";
        let overhead = utf16_len(scheme) + utf16_len(marker_and_number);

        for (target, should_pass) in [
            (MAX_TERMINAL_GITHUB_PR_URL_LENGTH - 1, true),
            (MAX_TERMINAL_GITHUB_PR_URL_LENGTH, true),
            (MAX_TERMINAL_GITHUB_PR_URL_LENGTH + 1, false),
        ] {
            let filler = build_units(target - overhead);
            let candidate_url = format!("{scheme}{filler}{marker_and_number}");
            assert_eq!(utf16_len(&candidate_url), target, "target={target}");

            let value = format!("{candidate_url}\n");
            let candidates = iterate_terminal_url_candidates(&value);
            if should_pass {
                assert_eq!(candidates.len(), 1, "target={target}");
                assert_eq!(candidates[0].raw_url, candidate_url, "target={target}");
            } else {
                assert!(candidates.is_empty(), "target={target}");
            }
        }
    }

    /// Q3: each of the four dedupe-key sensitivities pinned individually —
    /// scheme, host case, trailing path, and leading zeros each produce a
    /// DISTINCT key, so both instances are emitted (not deduped).
    #[test]
    fn q3_dedupe_key_is_scheme_sensitive() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42\n").len(), 1);
        assert_eq!(d.observe("http://github.com/acme/orca/pull/42\n").len(), 1);
    }

    #[test]
    fn q3_dedupe_key_is_host_case_sensitive() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://GitHub.com/acme/orca/pull/42\n").len(), 1);
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42\n").len(), 1);
    }

    #[test]
    fn q3_dedupe_key_is_sensitive_to_trailing_path_segments() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42\n").len(), 1);
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42/files\n").len(), 1);
    }

    #[test]
    fn q3_dedupe_key_is_sensitive_to_leading_zeros() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let r1 = d.observe("https://github.com/acme/orca/pull/007\n");
        let r2 = d.observe("https://github.com/acme/orca/pull/7\n");
        assert_eq!(r1.len(), 1);
        assert_eq!(r2.len(), 1);
        assert_eq!(r1[0].number, 7);
        assert_eq!(r2[0].number, 7);
    }

    /// `isTerminalUrlTerminator`'s `"`/`'`/`<`/`>` arms, each pinned.
    #[test]
    fn terminator_quote_and_angle_bracket_arms_each_terminate() {
        for term in ['"', '\'', '<', '>'] {
            let mut d = TerminalGitHubPRLinkDetector::default();
            let input = format!("https://github.com/acme/orca/pull/42{term}trailing\n");
            assert_eq!(
                d.observe(&input),
                vec![pr("https://github.com/acme/orca/pull/42", "acme", "orca", 42)],
                "term={term:?}"
            );
        }
    }

    /// Q12: the length filter runs BEFORE the trailing-punctuation trim. An
    /// over-length (2049-unit) candidate ending in `))))` is dropped even
    /// though the trimmed URL (2045 units) would fit comfortably under the
    /// 2048 cap.
    #[test]
    fn q12_trailing_punctuation_trim_happens_after_length_filter() {
        let base = "https://github.com/acme/orca/pull/1";
        let base_len = utf16_len(base);
        let padding_len = MAX_TERMINAL_GITHUB_PR_URL_LENGTH + 1 - base_len - 4;
        let padding = "x".repeat(padding_len);
        let candidate = format!("{base}{padding}))))");
        assert_eq!(utf16_len(&candidate), MAX_TERMINAL_GITHUB_PR_URL_LENGTH + 1);

        let trimmed = trim_terminal_url(&candidate);
        assert!(utf16_len(trimmed) <= MAX_TERMINAL_GITHUB_PR_URL_LENGTH);

        let value = format!("{candidate}\n");
        assert!(iterate_terminal_url_candidates(&value).is_empty());
    }

    /// The remaining trim-class members (`,`, `;`, `]`, `}`) not covered by
    /// the oracle's own `)`/`.` test.
    #[test]
    fn trailing_punctuation_trim_class_comma_semicolon_bracket_brace() {
        for term in [',', ';', ']', '}'] {
            let mut d = TerminalGitHubPRLinkDetector::default();
            let input = format!("Opened (https://github.com/acme/orca/pull/42{term}\n");
            let result = d.observe(&input);
            assert_eq!(result[0].url, "https://github.com/acme/orca/pull/42", "term={term:?}");
        }
    }

    /// A `/pull/`-containing URL that structurally parses as an ISSUE (the
    /// route segment is `issues`, and `/pull/5` is just a trailing path
    /// segment) must NOT be treated as a PR link.
    #[test]
    fn issues_path_containing_pull_marker_parses_as_issue_not_pr() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/acme/orca/issues/42/pull/5\n"), Vec::new());
    }

    /// `findNextHttpSchemeIndex` must pick whichever scheme occurs FIRST in
    /// the buffer, not always prefer `https://` structurally.
    #[test]
    fn find_next_http_scheme_index_picks_earliest_of_both_schemes_in_one_chunk() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        let input =
            "See http://github.internal/acme/orca/pull/1\nand https://github.com/acme/orca/pull/2\n";
        let result = d.observe(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].url, "http://github.internal/acme/orca/pull/1");
        assert_eq!(result[1].url, "https://github.com/acme/orca/pull/2");
    }

    /// `reset()` clears both `carry` and `seen_urls` — a URL emitted before
    /// `reset()` is emitted again afterward (dedupe memory is gone).
    #[test]
    fn reset_clears_both_carry_and_seen_urls() {
        let mut d = TerminalGitHubPRLinkDetector::default();
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42\n").len(), 1);
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42\n"), Vec::new());
        d.reset();
        assert_eq!(d.observe("https://github.com/acme/orca/pull/42\n").len(), 1);
    }
}
