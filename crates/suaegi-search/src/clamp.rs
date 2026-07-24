//! Line-context clamping — port of `clampLineContext`
//! (`src/shared/text-search.ts:65-103`), reflecting plan correction **C1
//! (byte-safety)**.
//!
//! rg `--json` reports `start`/`end` as **UTF-8 byte offsets** (Codex verified:
//! `é` is 2 bytes → `start:2`). Orca uses these as JS string (UTF-16 code-unit)
//! indices — a latent bug the ASCII-only oracle never exposes. In Rust, slicing
//! a `&str` at a non-char-boundary *panics* (a cardinal sin: a wrong slice
//! crashes the search). So this port:
//!
//! - **Preserves `column = match_start + 1` and `match_length` verbatim** as the
//!   canonical numeric source coordinates. We do NOT convert byte offsets to char
//!   indices — that would change the observable contract (plan C1(a)).
//! - **Snaps the render window to char boundaries** before slicing, always moving
//!   *outward* (start toward 0, end toward `len`) so `&text[start..end]` can never
//!   panic (plan C1(c),(d)).
//! - Computes the `display_*` coordinates on the safe, snapped snippet (C1(e)).
//! - Never panics and never drops a match; malformed offsets (`start > end`,
//!   beyond `len`) are absorbed by `saturating_*` / clamping.
//!
//! # Length measure (documented decision)
//! Orca's `MAX_LINE_CONTENT_LENGTH` budget (500) is measured in JS `String.length`
//! (**UTF-16 code units**). This port measures it in **bytes** (`str::len`).
//! Bytes is the natural Rust choice and matches rg's byte-offset world (`match_start`
//! / `match_length` are byte offsets, so the window arithmetic stays in one unit).
//! For ASCII the two measures are identical. For non-ASCII they diverge: a line of
//! N multibyte scalars reaches the 500-*byte* cap sooner than the 500-*code-unit*
//! cap, so a slightly different set of lines gets windowed. This is the documented,
//! accepted divergence from Orca's defective UTF-16 behavior (plan §3 deferred:
//! "완전한 Unicode 윈도잉 충실도" is explicitly not reproduced).
//!
//! Consequently `display_column` / `display_match_length` are **byte offsets/lengths**
//! into `line_content` (1-based column), so `&line_content.as_bytes()[dc-1 .. dc-1+dml]`
//! recovers the matched text. The truncation marker therefore contributes its UTF-8
//! byte length (3) to `display_column`, not Orca's UTF-16 length (1).

use crate::constants::{MAX_LINE_CONTENT_LENGTH, TRUNCATION_MARKER};

/// Result of [`clamp_line_context`]. `column` / `match_length` are the canonical
/// (byte-derived) source coordinates; `display_*` are the render-safe snippet
/// coordinates, present only when the line was long enough to be windowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clamped {
    pub line_content: String,
    pub column: usize,
    pub match_length: usize,
    pub display_column: Option<usize>,
    pub display_match_length: Option<usize>,
}

/// Clamp a match's line context to at most `MAX_LINE_CONTENT_LENGTH` bytes of
/// window (plus up to two truncation markers), keeping the match centered.
///
/// `match_start` / `match_length` are byte offsets (as rg reports them). The
/// returned `column` / `match_length` echo those canonical coordinates; the
/// windowed path additionally returns `display_*`.
pub fn clamp_line_context(text: &str, match_start: usize, match_length: usize) -> Clamped {
    // Short line: pass through unchanged, no display coordinates (`:76-78`).
    if text.len() <= MAX_LINE_CONTENT_LENGTH {
        return Clamped {
            line_content: text.to_string(),
            column: match_start + 1,
            match_length,
            display_column: None,
            display_match_length: None,
        };
    }

    // Windowing (`:79-102`). Clamp the match first so a pathological multi-MB
    // regex hit can't defeat the windowing below (`:80`).
    let clamped_match_length = match_length.min(MAX_LINE_CONTENT_LENGTH);
    let remaining = MAX_LINE_CONTENT_LENGTH - clamped_match_length;
    let left_budget = remaining / 2;
    // `max(0, match_start - left_budget)` → saturating_sub.
    let mut window_start = match_start.saturating_sub(left_budget);
    let mut window_end = text.len().min(window_start + MAX_LINE_CONTENT_LENGTH);
    // Pull the left edge back when the right edge hit EOL, to fill the window (`:85`).
    window_start = window_start.max(window_end.saturating_sub(MAX_LINE_CONTENT_LENGTH));

    // C1: snap to char boundaries *before* slicing — outward, so we never split a
    // scalar and never panic. `0` and `text.len()` are always boundaries, so both
    // loops terminate.
    while !text.is_char_boundary(window_start) {
        window_start -= 1;
    }
    while window_end < text.len() && !text.is_char_boundary(window_end) {
        window_end += 1;
    }
    // Defensive: keep the range well-formed even under malformed offsets.
    if window_end < window_start {
        window_end = window_start;
    }

    let mut snippet = text[window_start..window_end].to_string();
    // Display column: 1-based byte offset of the match within the (pre-marker)
    // snippet. `window_start <= match_start` by construction, so no underflow.
    let mut display_column = match_start.saturating_sub(window_start) + 1;
    if window_start > 0 {
        snippet.insert_str(0, TRUNCATION_MARKER);
        // Byte-measured column: the marker adds its UTF-8 byte length (3), not
        // Orca's UTF-16 length (1). See module note.
        display_column += TRUNCATION_MARKER.len();
    }
    if window_end < text.len() {
        snippet.push_str(TRUNCATION_MARKER);
    }

    Clamped {
        line_content: snippet,
        column: match_start + 1,
        match_length,
        display_column: Some(display_column),
        display_match_length: Some(clamped_match_length),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line at or under the cap passes through with no `display_*` fields and
    /// the canonical 1-based `column`. Oracle cases 8/11/12/26 all take this path.
    #[test]
    fn short_line_passthrough_no_display() {
        let c = clamp_line_context("abc", 0, 3);
        assert_eq!(c.line_content, "abc");
        assert_eq!(c.column, 1);
        assert_eq!(c.match_length, 3);
        assert!(c.display_column.is_none());
        assert!(c.display_match_length.is_none());

        // Mid-line match keeps column = start+1.
        let c2 = clamp_line_context("foo and foo again", 8, 3);
        assert_eq!(c2.column, 9);
        assert!(c2.display_column.is_none());
    }

    /// Oracle case 14 (`text-search.test.ts:156`): a huge line is windowed to
    /// ≤ `MAX_LINE_CONTENT_LENGTH + 2` bytes, `column` stays the TRUE source
    /// coordinate (`match_start + 1`), `display_match_length` is 6, and slicing
    /// `line_content` by the display coordinates recovers `NEEDLE`.
    ///
    /// *Mutation:* breaking the window arithmetic (e.g. dropping the `:85`
    /// left-pull, or a wrong `left_budget`) misaligns the display slice → the
    /// `NEEDLE` assertion fails.
    #[test]
    fn oracle_clamps_huge_line_true_vs_display_coords() {
        let huge = format!("{}NEEDLE{}", "x".repeat(200_000), "y".repeat(200_000));
        let match_start = 200_000;
        let c = clamp_line_context(&huge, match_start, 6);

        assert!(c.line_content.len() <= MAX_LINE_CONTENT_LENGTH + 2 * TRUNCATION_MARKER.len());
        // True (canonical) source coordinate — unchanged by windowing.
        assert_eq!(c.column, match_start + 1);
        assert_eq!(c.match_length, 6);
        let dc = c.display_column.expect("windowed → display_column set");
        assert_eq!(c.display_match_length, Some(6));
        // Byte-slice by the display coordinates recovers the match text.
        let bytes = c.line_content.as_bytes();
        assert_eq!(&bytes[dc - 1..dc - 1 + 6], b"NEEDLE");
    }

    /// C1 byte-safety: a >500-byte line whose window edges land *inside*
    /// multibyte scalars must snap to char boundaries and never panic. Uses a
    /// line built entirely of 2-byte `é` so nearly every byte offset is a
    /// non-boundary; the match sits deep in the middle.
    ///
    /// *Mutation (d):* replacing the `is_char_boundary` snap loops with a raw
    /// `&text[window_start..window_end]` slice at the unsnapped offsets PANICS
    /// here (byte offset falls mid-`é`) → this test fails.
    #[test]
    fn c1_multibyte_window_snaps_and_never_panics() {
        // 400 `é` (800 bytes) + "NEEDLE" (ASCII) + 400 `é` (800 bytes).
        let left = "é".repeat(400);
        let right = "é".repeat(400);
        let text = format!("{left}NEEDLE{right}");
        let match_start = left.len(); // 800, a char boundary (start of NEEDLE)
        let c = clamp_line_context(&text, match_start, 6);

        // Snippet is valid UTF-8 (constructing the String already proves no
        // mid-scalar slice happened) and bounded.
        assert!(c.line_content.len() <= MAX_LINE_CONTENT_LENGTH + 2 * TRUNCATION_MARKER.len() + 4);
        assert_eq!(c.column, match_start + 1);
        let dc = c.display_column.expect("windowed");
        let dml = c.display_match_length.unwrap();
        // The display slice still lands on a boundary and recovers NEEDLE.
        assert_eq!(&c.line_content.as_bytes()[dc - 1..dc - 1 + dml], b"NEEDLE");
        // Sanity: line_content round-trips as UTF-8.
        assert!(std::str::from_utf8(c.line_content.as_bytes()).is_ok());
    }

    /// A match that starts inside a multibyte scalar region, forcing an *odd*
    /// (non-boundary) window_start, must still slice safely. Also exercises the
    /// left-edge marker prepend on non-ASCII content.
    #[test]
    fn c1_odd_window_start_snaps_safely() {
        // 700 `é` = 1400 bytes so the line is windowed; put an ASCII match late.
        let text = format!("{}Zz", "é".repeat(700));
        let match_start = 1400; // byte offset of 'Z'
        let c = clamp_line_context(&text, match_start, 1);
        assert_eq!(c.column, 1401);
        // No panic + valid UTF-8.
        assert!(std::str::from_utf8(c.line_content.as_bytes()).is_ok());
        let dc = c.display_column.unwrap();
        assert_eq!(&c.line_content.as_bytes()[dc - 1..dc], b"Z");
    }

    /// Malformed offsets (start beyond len, start > end after subtraction) must be
    /// absorbed without panic. Long line to force the windowing path.
    #[test]
    fn malformed_offsets_never_panic() {
        let text = "a".repeat(1000);
        // start far beyond len; match_length larger than the cap.
        let c = clamp_line_context(&text, 5000, 10_000);
        assert!(std::str::from_utf8(c.line_content.as_bytes()).is_ok());
        assert!(c.line_content.len() <= MAX_LINE_CONTENT_LENGTH + 2 * TRUNCATION_MARKER.len());
    }
}
