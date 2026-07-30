//! Orchestration task-spec abbreviation (`--brief` mode) — verbatim port of
//! Orca's `src/shared/orchestration-task-summary.ts` (@ v1.4.146-rc.0).
//!
//! Why: `orchestration.taskList --brief` and the RPC `orchestration` method
//! shrink each task's `spec` so a large task list stays readable, without
//! ever hiding a spec change behind pure whitespace cleanup.
//!
//! # N11 — the three-step pipeline
//! ① `\s+` → `' '` collapse, then `.trim()` (`:7`) — applied to **every**
//! task, truncated or not. ② `spec.length > TASK_SPEC_BRIEF_LENGTH`
//! (strict `>`, `:8`). ③ Only when over the cap: slice + `trimEnd` + a
//! single `…` (U+2026, NOT three `.` characters — 1 UTF-16 unit / 3 UTF-8
//! bytes). The element count never changes (`.map`, 1:1).
//!
//! # ⚠ N12 — 160 is a ceiling, not an invariant
//! 159 is reachable two different ways: the surrogate-drop (see N13) and
//! `trimEnd` eating trailing whitespace that the slice cut left behind. A
//! test that asserts every truncated output has length exactly 160 is
//! WRONG — see `pin_trailing_whitespace_at_cut_yields_length_159` below.
//!
//! # ⚠ N13 — the truncation unit is UTF-16 code units, not bytes or `char`s
//! `.slice`/`.length` are UTF-16-code-unit operations. A raw `&s[..159]`
//! byte slice panics whenever a multi-byte character straddles that byte
//! offset (e.g. `'é'.repeat(200)`: unit 159 is byte 318, mid-character).
//! [`utf16_slice_prefix`] (copied locally from `suaegi_forge::repo_icon`,
//! per this repo's per-module-copy charter) snaps down to a whole-character
//! boundary instead.
//!
//! This snap-down is **provably equivalent** to the source's `slice(0,159)` +
//! high-surrogate-drop (`:23-24`) for every input a Rust `&str` can actually
//! hold: if the 159th UTF-16 unit would fall inside an astral character's
//! surrogate pair, [`utf16_slice_prefix`] excludes that whole character (its
//! `next_units > max_units` check fires on the *first* half), which is
//! exactly what the source's slice-then-drop-the-lone-high-surrogate ends up
//! doing. The only case where they could diverge — a lone *low* surrogate at
//! the cut point — cannot arise here: a well-formed `&str` can never contain
//! an unpaired surrogate. So the source's explicit high-surrogate check is a
//! **dead branch in Rust** and is deliberately NOT reimplemented on `char`
//! here — see `pin_surrogate_pair_at_boundary_is_dropped_and_length_is_159`.
//!
//! # N14 — three ECMAScript-whitespace sites, one predicate
//! `\s+`, `.trim()`, and `.trimEnd()` are all ECMAScript whitespace, which
//! diverges from Rust's Unicode `char::is_whitespace` at U+FEFF (JS
//! whitespace, Rust not) and U+0085 (Rust whitespace, JS not) — see
//! [`crate::js_ws`]. Built from [`is_js_whitespace`]/[`js_trim`] plus a
//! module-local `js_trim_end` two-liner (mirroring `suaegi-quickcmd`'s
//! `js_trim_end`; not exported from `js_ws` because only two ported modules
//! so far need the trailing-only variant).
//!
//! # N15 — generic passthrough is the caller's job
//! The source is generic over `T extends { spec: string }` and spreads
//! `...task` to keep every other field untouched. Without `serde_json::Value`
//! (this crate takes no dependencies), that spread isn't representable
//! generically in Rust. [`abbreviate_orchestration_task_spec`] transforms a
//! single `&str`; [`abbreviate_orchestration_tasks`] maps it across a slice
//! via a caller-supplied accessor closure. Re-attaching the transformed spec
//! (and `spec_truncated`) onto the original `T` — the equivalent of the `…`
//! spread — is the caller's responsibility, mirroring how
//! `worktree_submodule_removal` pushes `String(error)` coercion onto its
//! caller.
//!
//! # N16/N17/N18 — the oracle never exercises the boundary
//! The oracle's truncation fixtures are 290, 200, and 10 UTF-16 units —
//! nothing at 159, 160, or 161 — so `>` vs. `>=` and the constant `160` vs.
//! `159`/`161` are all invisible to it (N16). The corpus is effectively all
//! ASCII (one `😀` aside), so byte-vs-unit slicing is also invisible (N17).
//! And the surrogate fixture (`test:29-37`) never asserts a length (N18), so
//! "pad back to 160", "drop 2 units instead of 1", and the correct snap-down
//! are indistinguishable to it. This module pins all three directly.
//!
//! # N19 — consumer subtlety, documented only (not this module's bug)
//! `cli/handlers/orchestration.ts:583-584` guards client-side abbreviation
//! with `.some((task) => task.spec_truncated === undefined)`: if **any** row
//! in a mixed server response lacks `spec_truncated` (older runtime), the
//! whole batch — including rows a newer runtime already truncated
//! server-side — gets re-run through `abbreviateOrchestrationTasks`, which
//! recomputes `spec_truncated` from the (now-already-abbreviated) spec text.
//! An already-short abbreviated spec would then be flagged
//! `spec_truncated: false` even though the original was truncated. This is a
//! caller-side inconsistency in Orca's CLI handler, not a bug in the ported
//! functions here — noted for anyone wiring a Rust caller the same way.

use crate::js_ws::{is_js_whitespace, js_trim};

/// Port of `TASK_SPEC_BRIEF_LENGTH` (`:1`). A ceiling, not an invariant —
/// see N12.
pub const TASK_SPEC_BRIEF_LENGTH: usize = 160;

/// One task's abbreviated spec plus whether it was actually truncated (N11:
/// whitespace normalization alone never sets this true).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbbreviatedSpec {
    pub spec: String,
    pub spec_truncated: bool,
}

/// UTF-16 code-unit length of `s` (JS `.length` semantics), not byte length
/// and not `char` count.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Largest prefix of `s` whose UTF-16-code-unit count is `<= max_units`,
/// snapping down to a whole-character boundary rather than splitting an
/// astral character's surrogate pair (N13). Copied locally (per this repo's
/// per-module-copy charter, plan §0) from the identical technique in
/// `suaegi_forge::repo_icon`'s private `utf16_slice_prefix`.
fn utf16_slice_prefix(s: &str, max_units: usize) -> &str {
    let mut units = 0usize;
    for (byte_offset, ch) in s.char_indices() {
        let next_units = units + ch.len_utf16();
        if next_units > max_units {
            return &s[..byte_offset];
        }
        units = next_units;
    }
    s
}

/// Local ECMAScript trailing-only trim (N14) — the truncation site uses
/// `.trimEnd()` (`:24`), not `.trim()`. Mirrors `suaegi-quickcmd`'s
/// `js_trim_end` two-liner rather than extending `js_ws` for a single caller.
fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(|ch: char| is_js_whitespace(ch))
}

/// `spec.replace(/\s+/g, ' ')` (`:7`): every maximal run of ECMAScript
/// whitespace becomes a single ASCII space, applied unconditionally (N11
/// step ①), before the length check.
fn collapse_js_whitespace_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for ch in s.chars() {
        if is_js_whitespace(ch) {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    out
}

/// Port of the per-task transform inside `abbreviateOrchestrationTasks`
/// (`:6-16`), applied to a single spec string. See the module header (N11)
/// for the three-step pipeline.
pub fn abbreviate_orchestration_task_spec(spec: &str) -> AbbreviatedSpec {
    let collapsed = collapse_js_whitespace_runs(spec);
    let trimmed = js_trim(&collapsed);
    let truncated = utf16_len(trimmed) > TASK_SPEC_BRIEF_LENGTH;
    if !truncated {
        return AbbreviatedSpec {
            spec: trimmed.to_string(),
            spec_truncated: false,
        };
    }

    let sliced = utf16_slice_prefix(trimmed, TASK_SPEC_BRIEF_LENGTH - 1);
    let sliced = js_trim_end(sliced);
    AbbreviatedSpec {
        spec: format!("{sliced}\u{2026}"),
        spec_truncated: true,
    }
}

/// Port of `abbreviateOrchestrationTasks` (`:3-17`), reshaped per N15: `T` is
/// left generic and unconstrained, with `get_spec` standing in for the
/// source's `task.spec` access. Returns one [`AbbreviatedSpec`] per input
/// task, in order (`.map`, 1:1) — re-attaching onto the caller's own `T` is
/// the caller's job.
pub fn abbreviate_orchestration_tasks<T>(
    tasks: &[T],
    get_spec: impl Fn(&T) -> &str,
) -> Vec<AbbreviatedSpec> {
    tasks
        .iter()
        .map(|task| abbreviate_orchestration_task_spec(get_spec(task)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        abbreviate_orchestration_task_spec, abbreviate_orchestration_tasks, AbbreviatedSpec,
        TASK_SPEC_BRIEF_LENGTH,
    };

    struct Task {
        id: &'static str,
        spec: String,
    }

    // Oracle: orchestration-task-summary.test.ts

    #[test]
    fn collapses_whitespace_and_caps_long_task_specs() {
        let tasks = [Task {
            id: "task_1",
            spec: format!("First line\n\n{}", "detail ".repeat(40)),
        }];
        let results = abbreviate_orchestration_tasks(&tasks, |t| t.spec.as_str());
        let task = &results[0];

        assert_eq!(tasks[0].id, "task_1");
        assert!(!task.spec.contains('\n'));
        assert_eq!(task.spec.chars().count(), 160);
        assert!(task.spec.ends_with('\u{2026}'));
        assert!(task.spec_truncated);
    }

    #[test]
    fn preserves_a_short_one_line_spec() {
        let tasks = [Task {
            id: "x",
            spec: "Short task".to_string(),
        }];
        let results = abbreviate_orchestration_tasks(&tasks, |t| t.spec.as_str());

        assert_eq!(
            results[0],
            AbbreviatedSpec {
                spec: "Short task".to_string(),
                spec_truncated: false
            }
        );
    }

    #[test]
    fn does_not_report_whitespace_normalization_as_truncation() {
        let tasks = [Task {
            id: "x",
            spec: "Short\n\n  task".to_string(),
        }];
        let results = abbreviate_orchestration_tasks(&tasks, |t| t.spec.as_str());

        assert_eq!(
            results[0],
            AbbreviatedSpec {
                spec: "Short task".to_string(),
                spec_truncated: false
            }
        );
    }

    /// 158 chars + an astral emoji spanning UTF-16 units 158-159: a naive
    /// `slice(0, 159)` (or a byte slice) would cut the pair and leave a
    /// lone high surrogate; a Rust `&str` cannot even represent that, so
    /// "well-formed" is checked instead via successful construction plus the
    /// length pin below (N18).
    #[test]
    fn does_not_split_a_surrogate_pair_at_the_truncation_boundary() {
        let spec = format!("{}\u{1F600}{}", "a".repeat(158), "b".repeat(40));
        let result = abbreviate_orchestration_task_spec(&spec);

        assert!(result.spec_truncated);
        assert!(result.spec.ends_with('\u{2026}'));
    }

    // Mandatory extra pins (oracle-silent — plan §5):

    /// N16: the constant is exactly 160, not some nearby value.
    #[test]
    fn pin_brief_length_constant_is_160() {
        assert_eq!(TASK_SPEC_BRIEF_LENGTH, 160);
    }

    /// N16 boundary pin: exactly 159 units is NOT truncated (`>`, not `>=`).
    #[test]
    fn pin_boundary_159_units_not_truncated() {
        let spec = "a".repeat(159);
        let result = abbreviate_orchestration_task_spec(&spec);
        assert_eq!(
            result,
            AbbreviatedSpec {
                spec: spec.clone(),
                spec_truncated: false
            }
        );
    }

    /// N16 boundary pin: exactly 160 units is NOT truncated (strict `>`).
    #[test]
    fn pin_boundary_160_units_not_truncated() {
        let spec = "a".repeat(160);
        let result = abbreviate_orchestration_task_spec(&spec);
        assert_eq!(
            result,
            AbbreviatedSpec {
                spec: spec.clone(),
                spec_truncated: false
            }
        );
    }

    /// N16 boundary pin: exactly 161 units IS truncated, and the truncated
    /// body is exactly the first 159 units plus the ellipsis (160 total,
    /// since nothing at the cut point is whitespace or a surrogate).
    #[test]
    fn pin_boundary_161_units_truncated() {
        let spec = "a".repeat(161);
        let result = abbreviate_orchestration_task_spec(&spec);
        assert!(result.spec_truncated);
        assert_eq!(result.spec, format!("{}\u{2026}", "a".repeat(159)));
        assert_eq!(result.spec.chars().count(), 160);
    }

    /// N17: `'é'.repeat(200)` — 200 UTF-16 units, but 400 UTF-8 bytes with
    /// every character 2 bytes wide. A byte-slicing port panics here (byte
    /// offset 159 falls mid-character); a correct UTF-16-unit-aware
    /// snap-down does not.
    #[test]
    fn pin_multibyte_bmp_repeat_does_not_panic_and_slices_by_unit() {
        let spec = "\u{00E9}".repeat(200);
        let result = abbreviate_orchestration_task_spec(&spec);
        assert!(result.spec_truncated);
        assert_eq!(result.spec, format!("{}\u{2026}", "\u{00E9}".repeat(159)));
    }

    /// N13/N18 crux pin: the surrogate-pair fixture's truncated result has
    /// length exactly 159 (158 `a`s + ellipsis) — the emoji is dropped
    /// *whole*, not padded back to 160 and not merely 1 unit short. This
    /// distinguishes the correct UTF-16 snap-down from `chars().take(159)`
    /// (which would count the emoji as a single `char` and keep the whole
    /// surrogate pair, yielding length 161) and from any variant that pads
    /// the drop back to the 160 ceiling.
    #[test]
    fn pin_surrogate_pair_at_boundary_is_dropped_and_length_is_159() {
        let spec = format!("{}\u{1F600}{}", "a".repeat(158), "b".repeat(40));
        let result = abbreviate_orchestration_task_spec(&spec);
        assert_eq!(result.spec, format!("{}\u{2026}", "a".repeat(158)));
        assert_eq!(result.spec.chars().count(), 159);
    }

    /// N12 crux pin: a whitespace character exactly at the cut point is
    /// removed by `trimEnd`, so the truncated output lands at 159 units,
    /// not 160 — proving 160 is a ceiling, not an invariant, through a
    /// *different* mechanism than the surrogate drop above.
    #[test]
    fn pin_trailing_whitespace_at_cut_yields_length_159() {
        let spec = format!("{} {}", "a".repeat(158), "b".repeat(50));
        let result = abbreviate_orchestration_task_spec(&spec);
        assert!(result.spec_truncated);
        assert_eq!(result.spec, format!("{}\u{2026}", "a".repeat(158)));
        assert_eq!(result.spec.chars().count(), 159);
    }

    /// N11: whitespace normalization runs even on a task that ends up
    /// untruncated — not merely applied as part of truncation handling.
    #[test]
    fn pin_untruncated_task_is_still_whitespace_normalized() {
        let result = abbreviate_orchestration_task_spec("a\n\n\tb   c");
        assert_eq!(
            result,
            AbbreviatedSpec {
                spec: "a b c".to_string(),
                spec_truncated: false
            }
        );
    }

    /// N14: U+FEFF (BOM/ZWNBSP) is ECMAScript whitespace, so `\s+` collapses
    /// it — matching JS, diverging from Rust's Unicode-whitespace-based
    /// collapse which would NOT treat it as whitespace.
    #[test]
    fn pin_feff_collapses_as_whitespace() {
        let result = abbreviate_orchestration_task_spec("a\u{FEFF}b");
        assert_eq!(
            result,
            AbbreviatedSpec {
                spec: "a b".to_string(),
                spec_truncated: false
            }
        );
    }

    /// N14: U+0085 (NEL) is NOT ECMAScript whitespace, so it must survive
    /// `\s+` collapse untouched — matching JS, diverging from Rust's
    /// `char::is_whitespace` which WOULD treat it as whitespace and collapse
    /// it away.
    #[test]
    fn pin_nel_is_preserved_not_collapsed() {
        let result = abbreviate_orchestration_task_spec("a\u{0085}b");
        assert_eq!(
            result,
            AbbreviatedSpec {
                spec: "a\u{0085}b".to_string(),
                spec_truncated: false
            }
        );
    }

    /// N14 crux pin for `js_trim_end` specifically (distinct from the
    /// `js_trim` pin below): a U+0085 (NEL) landing exactly at the
    /// truncation cut point must survive `.trimEnd()` — it is not
    /// ECMAScript whitespace. Rust's `str::trim_end` WOULD strip it (NEL is
    /// Unicode `White_Space`), shortening the output by one unit (159
    /// instead of 160) and dropping the NEL from the text.
    #[test]
    fn pin_nel_at_truncation_cut_survives_trim_end() {
        let spec = format!("{}\u{0085}{}", "a".repeat(158), "b".repeat(50));
        let result = abbreviate_orchestration_task_spec(&spec);

        assert!(result.spec_truncated);
        assert_eq!(result.spec, format!("{}\u{0085}\u{2026}", "a".repeat(158)));
        assert_eq!(result.spec.chars().count(), 160);
    }

    /// N14 crux pin, distinct from the two above: a leading U+0085 (NEL)
    /// survives the FINAL `.trim()` call, not just the `\s+` collapse. NEL
    /// never reaches the collapse step's whitespace branch (it isn't
    /// ECMAScript whitespace), so it flows into `js_trim` unchanged; JS's
    /// `.trim()` doesn't strip it either. This is the only pin that can
    /// distinguish `js_trim` from Rust's `str::trim` at the outer trim call
    /// (the FEFF pin above can't: collapse already rewrites FEFF to a plain
    /// ASCII space before the outer trim ever sees it, so both trims agree
    /// there).
    #[test]
    fn pin_leading_nel_survives_the_outer_trim_call() {
        let result = abbreviate_orchestration_task_spec("\u{0085}task");
        assert_eq!(
            result,
            AbbreviatedSpec {
                spec: "\u{0085}task".to_string(),
                spec_truncated: false
            }
        );
    }
}
