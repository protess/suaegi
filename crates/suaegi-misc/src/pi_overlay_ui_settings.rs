//! Pi overlay UI settings merge — verbatim port of Orca's
//! `src/shared/pi-overlay-ui-settings.ts` (@ v1.4.146-rc.0).
//!
//! ⚠ **Zero production callers (P11).** A repo-wide grep in Orca finds only
//! this module's own Vitest file referencing it — no caller wires it into the
//! real Pi overlay path. In fact the live overlay path has *moved away* from
//! this policy: `relay/plugin-overlay.test.ts:146-161` and
//! `main/pi/titlebar-extension-service.test.ts:82-117` both assert that
//! `settings.json` is forwarded to the overlay **unmodified**. This is ported
//! for clone-completeness (source exists, has an oracle) and is deliberately
//! wired to nothing — do not invent a call site for it.
//!
//! # Untyped input model — third local copy (P1/§1)
//! `JsValue`/`JsRecord` already exist twice in this workspace
//! (`suaegi-filedrop::{JsValue,JsRecord}`, `suaegi-quickcmd::{JsValue,JsRecord}`).
//! This is a **third, private, trimmed copy** — per-module duplication is
//! this repo's charter, not an oversight; do not import either sibling copy
//! or promote this one to a shared location. `serde_json::Value` is
//! deliberately not used: its `Value::Object` is a `BTreeMap` that re-sorts
//! keys, and enabling `preserve_order` propagates workspace-wide through
//! cargo feature unification — both are unacceptable for a module whose
//! entire point (P3) is that key order is an observable, pinned behavior.
//!
//! # Key order is a real choice here (P3)
//! JS `[[Set]]` leaves an *existing* own key in its slot and *appends* a
//! *new* one at the end. The source (`:12-14`) always assigns
//! `terminal` before `hideThinkingBlock`, so for any non-record (or empty)
//! input the output key order is exactly `["terminal", "hideThinkingBlock"]`
//! — not the reverse, even though the oracle's own fixture text lists
//! `hideThinkingBlock` first for readability. And in the oracle's first
//! fixture (`defaultProvider, hideThinkingBlock, packages, terminal`),
//! `hideThinkingBlock` is an *existing* key, so it is overwritten **in
//! place** at slot 2 — it does not move to the end. [`JsRecord::with`]
//! implements exactly this overwrite-in-place-else-append rule. The oracle
//! uses `toEqual` (order-blind) so none of this is oracle-visible; it is
//! pinned directly by the tests below.
//!
//! # Shallow, non-mutating, always-new (P4/P9)
//! The oracle never re-reads its input after calling the function, so
//! mutate-vs-copy and deep-vs-shallow are both underdetermined by it. This
//! port takes `&JsValue` and returns an owned [`JsRecord`]: every level is
//! `.clone()`d out of the input, so mutating the input through this call is
//! not just untested but **not expressible** — there is no `&mut` path back
//! into the caller's value. Anything nested two or more levels under
//! `terminal` is carried through unchanged (no recursive merge; only
//! `terminal.clearOnShrink` at depth 1 is ever touched).
//!
//! # `isPlainRecord` collapses to an `Object` match (P2)
//! Source (`:4-6`): `typeof value === 'object' && value !== null &&
//! !Array.isArray(value)`. That also accepts `Date`/`Map`/class instances in
//! JS, but [`JsValue`] has no such variants — there is nothing left for the
//! check to do except distinguish [`JsValue::Object`] from everything else,
//! so [`is_plain_record`] is `matches!(value, JsValue::Object(_))`. This is a
//! modeling decision, not a divergence. (`typeof fn === 'function'` already
//! excludes functions in the source too, so folding that case away loses
//! nothing.)
//!
//! # Two separate constants (P7)
//! [`PI_OVERLAY_HIDE_THINKING_BLOCK`] and [`PI_OVERLAY_CLEAR_ON_SHRINK`] are
//! both `true` today, so no value-based test can tell them apart if their
//! *assignments* were swapped (`merged.hideThinkingBlock =
//! PI_OVERLAY_CLEAR_ON_SHRINK` etc.) — with both constants equal to `true`
//! the output is byte-identical either way. They are kept as two distinct
//! named constants anyway, matching the source's two distinct product
//! decisions (one is a Pi-overlay chat-UI safety default, the other is a
//! terminal-resize default); collapsing them into one shared constant would
//! make that no longer visible in the code, even though today's test suite
//! cannot detect the merge. See the mutation table in the port's PR/report
//! for why this specific swap is an accepted, documented equivalent mutant.
//!
//! # Modeling limits — documented, not implemented (P10)
//! Two more JS object behaviors have no analogue here and are not
//! reachable from real (JSON-sourced) `settings.json` input, so they are
//! called out rather than modeled:
//! 1. JS engines iterate integer-like own keys first, in ascending numeric
//!    order, before string keys in insertion order (e.g. `{"7":a,"0":b}`
//!    iterates `0` then `7`). [`JsRecord`]'s `Vec` never reorders like this.
//! 2. Object spread (`{...x}`) also copies `Symbol` keys and invokes getters
//!    on access. [`JsValue`] has no `Symbol` variant and no accessor
//!    protocol — there is no own-key source that could trigger either.

/// A JS-like untyped value tree — mirrors TS `unknown`, the input type of
/// [`merge_pi_overlay_ui_settings`]. Third local copy of this apparatus; see
/// the module docs for why it isn't shared with `suaegi-filedrop` /
/// `suaegi-quickcmd`'s copies and isn't `serde_json::Value`.
#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Null,
    Undefined,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<JsValue>),
    Object(JsRecord),
}

impl JsValue {
    /// Convenience constructor for a string value.
    pub fn str(s: impl Into<String>) -> JsValue {
        JsValue::Str(s.into())
    }

    /// Convenience constructor for a number value.
    pub fn number(n: f64) -> JsValue {
        JsValue::Number(n)
    }

    /// Convenience constructor for an array value.
    pub fn array<I>(items: I) -> JsValue
    where
        I: IntoIterator<Item = JsValue>,
    {
        JsValue::Array(items.into_iter().collect())
    }

    /// Convenience constructor for an object value from `(key, value)` pairs.
    pub fn object<I>(pairs: I) -> JsValue
    where
        I: IntoIterator<Item = (&'static str, JsValue)>,
    {
        JsValue::Object(JsRecord::from_pairs(pairs))
    }
}

/// An untyped object record: an ordered list of own `(key, value)` pairs —
/// see the module docs (P3) for why key order is load-bearing here.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JsRecord(Vec<(String, JsValue)>);

impl JsRecord {
    pub fn new() -> Self {
        JsRecord(Vec::new())
    }

    pub fn from_pairs<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (&'static str, JsValue)>,
    {
        JsRecord(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Builder-style single-key `[[Set]]` (`O:12-14`'s assignment
    /// semantics): a key already present is overwritten **in place** (its
    /// slot does not move); a new key is **appended** at the end.
    pub fn with(mut self, key: &str, value: JsValue) -> Self {
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            self.0.push((key.to_string(), value));
        }
        self
    }

    fn get(&self, key: &str) -> Option<&JsValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// `PI_OVERLAY_HIDE_THINKING_BLOCK` (`O:1`) — forced onto the top-level
/// `hideThinkingBlock` key. See module docs (P7) for why this stays a
/// separate constant from [`PI_OVERLAY_CLEAR_ON_SHRINK`] even though both
/// are `true` today.
pub const PI_OVERLAY_HIDE_THINKING_BLOCK: bool = true;

/// `PI_OVERLAY_CLEAR_ON_SHRINK` (`O:2`) — forced onto `terminal.clearOnShrink`.
pub const PI_OVERLAY_CLEAR_ON_SHRINK: bool = true;

/// `isPlainRecord` (`O:4-6`). See module docs (P2) for why this collapses to
/// a single variant match.
fn is_plain_record(value: &JsValue) -> bool {
    matches!(value, JsValue::Object(_))
}

/// Clones `value`'s fields into a fresh [`JsRecord`] if it classifies as a
/// plain record (`isPlainRecord(x) ? {...x} : {}`, `O:9`/`O:10`), else
/// returns an empty one. The `unreachable!` encodes the invariant that
/// [`is_plain_record`] and the `Object` match agree; if a future edit ever
/// loosens [`is_plain_record`] to accept another variant (e.g. `Array`,
/// reintroducing the exact `Array.isArray` bug the source guards against,
/// P6), this panics instead of silently fabricating record fields for a
/// value that carries none.
fn as_record_or_empty(value: &JsValue) -> JsRecord {
    if is_plain_record(value) {
        match value {
            JsValue::Object(record) => record.clone(),
            _ => unreachable!("is_plain_record classified a non-Object JsValue as a record"),
        }
    } else {
        JsRecord::new()
    }
}

/// `mergePiOverlayUiSettings` (`O:8-17`): starts from a shallow copy of
/// `settings` if it's a plain record (else `{}`), forces
/// `terminal.clearOnShrink` and top-level `hideThinkingBlock` to their Orca
/// safety defaults (unconditionally — P8, even if the input already set
/// them, even to the same value), and returns the result as a new top-level
/// record (P9). See module docs for the key-order (P3), shallow/non-mutating
/// (P4), and non-record-`terminal`-discarded (P5) behaviors this encodes.
pub fn merge_pi_overlay_ui_settings(settings: &JsValue) -> JsRecord {
    let mut merged = as_record_or_empty(settings);
    let terminal_input = merged.get("terminal").cloned();
    let mut terminal = terminal_input.as_ref().map(as_record_or_empty).unwrap_or_default();

    terminal = terminal.with("clearOnShrink", JsValue::Bool(PI_OVERLAY_CLEAR_ON_SHRINK));
    merged = merged.with("terminal", JsValue::Object(terminal));
    merged = merged.with("hideThinkingBlock", JsValue::Bool(PI_OVERLAY_HIDE_THINKING_BLOCK));

    merged
}

#[cfg(test)]
mod tests {
    use super::{
        is_plain_record, merge_pi_overlay_ui_settings, JsRecord, JsValue,
        PI_OVERLAY_CLEAR_ON_SHRINK, PI_OVERLAY_HIDE_THINKING_BLOCK,
    };

    // Oracle: pi-overlay-ui-settings.test.ts

    #[test]
    fn oracle_preserves_user_settings_while_forcing_safety_settings() {
        // :5-25
        let settings = JsValue::object([
            ("defaultProvider", JsValue::str("amazon-bedrock")),
            ("hideThinkingBlock", JsValue::Bool(false)),
            ("packages", JsValue::array([JsValue::str("npm:pi-web-access")])),
            (
                "terminal",
                JsValue::object([
                    ("showImages", JsValue::Bool(false)),
                    ("clearOnShrink", JsValue::Bool(false)),
                ]),
            ),
        ]);

        let expected = JsRecord::from_pairs([
            ("defaultProvider", JsValue::str("amazon-bedrock")),
            ("hideThinkingBlock", JsValue::Bool(true)),
            ("packages", JsValue::array([JsValue::str("npm:pi-web-access")])),
            (
                "terminal",
                JsValue::object([
                    ("showImages", JsValue::Bool(false)),
                    ("clearOnShrink", JsValue::Bool(true)),
                ]),
            ),
        ]);

        assert_eq!(merge_pi_overlay_ui_settings(&settings), expected);
    }

    #[test]
    fn oracle_creates_a_valid_settings_object_from_malformed_shapes() {
        // :28-31 — null input. Actual `[[Set]]` order is `terminal` (assigned
        // first, `O:13`) then `hideThinkingBlock` (`O:14`) — not the reverse
        // order the oracle's object-literal fixture happens to be written in
        // (`toEqual` can't tell; see module docs P3).
        assert_eq!(
            merge_pi_overlay_ui_settings(&JsValue::Null),
            JsRecord::from_pairs([
                ("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))])),
                ("hideThinkingBlock", JsValue::Bool(true)),
            ])
        );

        // :32-35 — non-record `terminal` is discarded wholesale (P5).
        assert_eq!(
            merge_pi_overlay_ui_settings(&JsValue::object([("terminal", JsValue::str("compact"))])),
            JsRecord::from_pairs([
                ("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))])),
                ("hideThinkingBlock", JsValue::Bool(true)),
            ])
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// P3: exact key order for a non-record top-level input is
    /// `["terminal", "hideThinkingBlock"]` — `terminal` first because `O:13`
    /// runs before `O:14`, both against an initially empty `merged`.
    #[test]
    fn pin_p3_non_record_input_key_order_is_terminal_then_hide_thinking_block() {
        let merged = merge_pi_overlay_ui_settings(&JsValue::Null);
        assert_eq!(
            merged,
            JsRecord::new()
                .with("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))]))
                .with("hideThinkingBlock", JsValue::Bool(true))
        );
    }

    /// P3: in the oracle's first fixture, `hideThinkingBlock` is an
    /// *existing* key (slot 2 of 4) — it is overwritten in place, not moved
    /// to the end after `terminal` (slot 4). A "move existing keys to the
    /// end" mutant would produce `defaultProvider, packages, terminal,
    /// hideThinkingBlock` here instead.
    #[test]
    fn pin_p3_existing_key_is_overwritten_in_place_not_moved_to_the_end() {
        let settings = JsValue::object([
            ("defaultProvider", JsValue::str("x")),
            ("hideThinkingBlock", JsValue::Bool(false)),
            ("packages", JsValue::array([])),
            ("terminal", JsValue::object([])),
        ]);

        let merged = merge_pi_overlay_ui_settings(&settings);

        assert_eq!(
            merged,
            JsRecord::from_pairs([
                ("defaultProvider", JsValue::str("x")),
                ("hideThinkingBlock", JsValue::Bool(true)),
                ("packages", JsValue::array([])),
                ("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))])),
            ])
        );
    }

    /// P4: the input is unchanged after the call. Structurally guaranteed by
    /// the `&JsValue -> JsRecord` (owned-return) signature — there is no
    /// `&mut` path back into `settings`, so no port shape here could mutate
    /// it. Pinned anyway as documentation of that guarantee.
    #[test]
    fn pin_p4_input_is_unchanged_after_the_call() {
        let settings = JsValue::object([
            ("hideThinkingBlock", JsValue::Bool(false)),
            ("terminal", JsValue::object([("showImages", JsValue::Bool(true))])),
        ]);
        let before = settings.clone();

        let _ = merge_pi_overlay_ui_settings(&settings);

        assert_eq!(settings, before);
    }

    /// P4: a value nested two-or-more levels under `terminal` passes through
    /// unchanged (only depth-1 `terminal.clearOnShrink` is ever touched —
    /// there is no recursive merge).
    #[test]
    fn pin_p4_deeply_nested_terminal_value_passes_through_unchanged() {
        let nested = JsValue::object([("enabled", JsValue::Bool(false)), ("level", JsValue::number(3.0))]);
        let settings = JsValue::object([("terminal", JsValue::object([("theme", nested.clone())]))]);

        let merged = merge_pi_overlay_ui_settings(&settings);

        assert_eq!(
            merged,
            JsRecord::from_pairs([(
                "terminal",
                JsValue::object([("theme", nested), ("clearOnShrink", JsValue::Bool(true))])
            )])
            .with("hideThinkingBlock", JsValue::Bool(true))
        );
    }

    /// P6: an array top-level input must NOT spread into index-keyed
    /// properties (the `Array.isArray` guard's entire purpose) — it falls
    /// into the same non-record `{}` branch as `null`.
    #[test]
    fn pin_p6_array_input_does_not_spread_into_indexed_keys() {
        let merged = merge_pi_overlay_ui_settings(&JsValue::array([JsValue::str("a"), JsValue::str("b")]));
        assert_eq!(
            merged,
            JsRecord::new()
                .with("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))]))
                .with("hideThinkingBlock", JsValue::Bool(true))
        );
    }

    /// P6/P2: every other non-record top-level shape (`undefined`, string,
    /// number, boolean) collapses to the same `{}` branch as `null`/array.
    #[test]
    fn pin_p6_every_non_record_top_level_shape_collapses_the_same_way() {
        let expected = JsRecord::new()
            .with("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))]))
            .with("hideThinkingBlock", JsValue::Bool(true));

        assert_eq!(merge_pi_overlay_ui_settings(&JsValue::Undefined), expected);
        assert_eq!(merge_pi_overlay_ui_settings(&JsValue::str("settings")), expected);
        assert_eq!(merge_pi_overlay_ui_settings(&JsValue::number(42.0)), expected);
        assert_eq!(merge_pi_overlay_ui_settings(&JsValue::Bool(true)), expected);
    }

    /// P5: every non-record shape of `terminal` specifically (not just the
    /// oracle's `'compact'` string) is discarded wholesale, same as an
    /// absent `terminal` key.
    #[test]
    fn pin_p5_non_record_terminal_shapes_are_all_discarded() {
        let expected_terminal = JsValue::object([("clearOnShrink", JsValue::Bool(true))]);

        for terminal in [JsValue::Null, JsValue::array([]), JsValue::number(42.0)] {
            let merged = merge_pi_overlay_ui_settings(&JsValue::object([("terminal", terminal)]));
            assert_eq!(merged.get("terminal"), Some(&expected_terminal));
        }

        // Absent `terminal` key entirely.
        let merged = merge_pi_overlay_ui_settings(&JsValue::object([("other", JsValue::Bool(true))]));
        assert_eq!(merged.get("terminal"), Some(&expected_terminal));
    }

    /// P7: the two constants land on the *correct* keys (`clearOnShrink` from
    /// [`PI_OVERLAY_CLEAR_ON_SHRINK`], `hideThinkingBlock` from
    /// [`PI_OVERLAY_HIDE_THINKING_BLOCK`]). Both constants are `true` today,
    /// so this cannot catch a swap of *which* constant is used where — only
    /// that both keys currently hold `true`, matching source (`O:1-2`,
    /// `:12`, `:14`). Documented as an accepted equivalent mutant in the
    /// port's report, not a coverage gap to close.
    #[test]
    fn pin_p7_each_key_gets_its_own_constant() {
        let merged = merge_pi_overlay_ui_settings(&JsValue::Null);
        let terminal = match merged.get("terminal") {
            Some(JsValue::Object(record)) => record,
            other => panic!("expected terminal to be a record, got {other:?}"),
        };
        assert_eq!(terminal.get("clearOnShrink"), Some(&JsValue::Bool(PI_OVERLAY_CLEAR_ON_SHRINK)));
        assert_eq!(merged.get("hideThinkingBlock"), Some(&JsValue::Bool(PI_OVERLAY_HIDE_THINKING_BLOCK)));
    }

    /// P8: the three assignments are unconditional — even an input that
    /// already has the "correct" value keeps it via the forced assignment,
    /// not by coincidence of an `if absent` guard passing through.
    #[test]
    fn pin_p8_already_true_input_stays_true_via_forced_assignment() {
        let settings = JsValue::object([
            ("hideThinkingBlock", JsValue::Bool(true)),
            ("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))])),
        ]);
        assert_eq!(
            merge_pi_overlay_ui_settings(&settings),
            JsRecord::from_pairs([
                ("hideThinkingBlock", JsValue::Bool(true)),
                ("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))])),
            ])
        );
    }

    /// P8: a `terminal` record that never had `clearOnShrink` gets it
    /// *appended* (new key), distinct from the oracle's overwrite case.
    #[test]
    fn pin_p8_terminal_without_clear_on_shrink_gets_it_appended() {
        let settings = JsValue::object([("terminal", JsValue::object([("showImages", JsValue::Bool(true))]))]);
        assert_eq!(
            merge_pi_overlay_ui_settings(&settings),
            JsRecord::from_pairs([(
                "terminal",
                JsValue::object([("showImages", JsValue::Bool(true)), ("clearOnShrink", JsValue::Bool(true))])
            )])
            .with("hideThinkingBlock", JsValue::Bool(true))
        );
    }

    /// P9: an empty-record input still returns a record with exactly the two
    /// forced keys — nothing carries over because there was nothing to carry.
    #[test]
    fn pin_p9_empty_record_input_yields_exactly_the_two_forced_keys() {
        let merged = merge_pi_overlay_ui_settings(&JsValue::object([]));
        assert_eq!(
            merged,
            JsRecord::new()
                .with("terminal", JsValue::object([("clearOnShrink", JsValue::Bool(true))]))
                .with("hideThinkingBlock", JsValue::Bool(true))
        );
    }

    /// P2: [`is_plain_record`] accepts only `Object` — every other variant,
    /// including the ones with no top-level oracle fixture, is rejected.
    #[test]
    fn pin_p2_only_object_variant_is_a_plain_record() {
        assert!(is_plain_record(&JsValue::object([])));
        assert!(!is_plain_record(&JsValue::Null));
        assert!(!is_plain_record(&JsValue::Undefined));
        assert!(!is_plain_record(&JsValue::Bool(false)));
        assert!(!is_plain_record(&JsValue::number(0.0)));
        assert!(!is_plain_record(&JsValue::str("")));
        assert!(!is_plain_record(&JsValue::array([])));
    }
}
