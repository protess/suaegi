//! Linear plain-list attribute filter (state/priority/assignee/label facets).
//! Ported verbatim from Orca `src/shared/linear-issue-attribute-filter.ts`
//! (@ v1.4.150-rc.0, no imports — fully self-contained). Shared wire + cache
//! identity: IPC/RPC parses untrusted input ([`parse_linear_issue_attribute_filter`]),
//! the renderer-equivalent canonicalizes already-typed state
//! ([`canonicalize_linear_issue_attribute_filter`]). An empty filter is
//! omitted from transport so unfiltered list requests stay equivalent
//! ([`optional_parsed_linear_issue_attribute_filter`]).
//!
//! # Contract decisions vs. the JS source (plan `2026-07-26-linear-attribute-filter.md`)
//!
//! - **P1** — trimming uses [`suaegi_misc::js_trim`], not `str::trim()`. JS
//!   `.trim()` strips U+FEFF and keeps U+0085; Rust's `str::trim()` does the
//!   opposite. An id consisting solely of U+FEFF must trim to empty.
//! - **P2** — the array-length caps (`MAX_STATE_IDS`/`MAX_LABEL_IDS`/
//!   `MAX_PRIORITIES`) are measured on the raw array length, before
//!   trim/dedup, and **reject** rather than truncate. Exactly-at-limit
//!   passes (`len > max`, not `>=`). The array-length check runs before any
//!   per-entry check, so a too-long array with a bad entry still reports the
//!   cap error. The id-length cap is applied *after* trimming.
//! - **P3** — [`parse_linear_issue_attribute_filter`] checks, in order:
//!   null → not-an-object → unknown key → missing required key → per-field
//!   asserts (stateIds → priorities → assignee → labelIds). This ordering
//!   can't be reproduced with `#[serde(deny_unknown_fields)]`, so the object
//!   is hand-walked as `&serde_json::Value`.
//! - **P4** — null/absent rules differ per function. `parse_...(&Value)`
//!   treats `Value::Null` as an error. `optional_parsed_...(Option<&Value>)`
//!   treats `None` (absent) as `Ok(None)` but `Some(Value::Null)` as an
//!   error. [`is_empty_linear_issue_attribute_filter`] and
//!   [`linear_issue_attribute_filter_signature`] treat `None` as *empty*.
//!   For the `assignee` key specifically: missing key → required-key error;
//!   present with JSON `null` → `Ok(None)` (no assignee filter).
//! - **P5** — priority validation on the parse path is a SINGLE combined
//!   check, not two. Orca's `assertPriorities`
//!   (`linear-issue-attribute-filter.ts:166-171`) throws one message —
//!   `priorities[{index}] must be an integer from 0 to 4` — from a single
//!   `||` chain: `typeof entry !== 'number' || !Number.isInteger(entry) ||
//!   entry < 0 || entry > 4`. There is exactly one
//!   [`LinearIssueAttributeFilterError`] variant
//!   ([`LinearIssueAttributeFilterError::PriorityInvalid`]) for any of:
//!   non-number, fractional, negative, or `> 4`.
//!   [`canonicalize_linear_issue_attribute_filter`] silently drops anything
//!   `> 4` (Q4) — that is a separate, more lenient policy on a different
//!   path, not a second parse-time error kind. `Number.isInteger` semantics
//!   apply to the numeric *value*, so a JSON float like `4.0` is integral
//!   and in range and must be **accepted** (JS `Number.isInteger(4.0) ===
//!   true`).
//! - **P6** — canonicalize output is canonical: dedup by value, then sort
//!   (strings via `str::cmp`/[`Ord`], numbers ascending). `canonicalize_ids`
//!   has *no* cap — caps live only on the parse path.
//! - **P7** — "empty" is judged *after* canonicalization, requiring all of
//!   state_ids empty, priorities empty, assignee `None`, label_ids empty.
//!   `priorities: [0]` and `assignee: Unassigned` are *not* empty (the
//!   falsy-zero trap the oracle locks at `test.ts:59-69`).
//! - **Q1** — [`Vec::sort_unstable`] on `Vec<String>` uses `str::cmp`
//!   (code-point order). JS's `Array.prototype.sort` default comparator
//!   compares UTF-16 code units, so astral characters (≥U+10000) order
//!   differently between the two. This is an accepted, documented
//!   divergence: [`linear_issue_attribute_filter_signature`] is an internal
//!   suaegi cache key with no JS peer to byte-compare against (suaegi is a
//!   Rust port end to end), and the oracle only requires that *equivalent*
//!   filters share a signature (`test.ts:37-39`) — satisfied by any total
//!   order.
//! - **Q1b** — the signature JSON is built from `#[derive(Serialize))]`
//!   structs (not `serde_json::Map`/`json!`, which sort keys alphabetically
//!   via `BTreeMap`), so field order matches Orca's object-literal order:
//!   `stateIds, priorities, assignee, labelIds`, and inside `assignee`:
//!   `kind, id`.
//! - **Q2** — the id-length cap (`ID_MAX_LENGTH`) counts UTF-16 code units
//!   (`str::encode_utf16().count()`), matching JS `string.length`. Not bytes,
//!   not `chars().count()`.
//! - **Q3** — `canonicalize_assignee` on a `User { id }` whose id trims to
//!   empty demotes to `None` (no assignee filter), *not* `Unassigned`. This
//!   looks like a bug but is the real, oracle-consistent Orca behavior
//!   (`canonicalizeAssignee`, `:74-78`) — it is intentionally not "fixed"
//!   into an error. It is unreachable via [`parse_linear_issue_attribute_filter`]
//!   (which rejects empty ids before canonicalize ever runs) — only a direct
//!   [`canonicalize_linear_issue_attribute_filter`] call can trigger it.
//! - **Q4** — `priorities` is `Vec<u8>`. This makes negative/non-integer
//!   values type-level unrepresentable, so JS's `p < 0`/
//!   `!Number.isInteger(p)` drop branches in `canonicalizePriorities` are
//!   unreachable on the canonicalize path (a sanctioned, type-level
//!   divergence) — the `> 4` drop is the one live branch, and it is mirrored
//!   verbatim. The parse path still validates from JSON, where all of those
//!   shapes are reachable, as the single combined check described in P5.
//! - **Q5** — errors are the structured [`LinearIssueAttributeFilterError`]
//!   enum, carrying field name, array index, and numeric limit where
//!   relevant, but never the raw id value (house style, matching
//!   `linear::write::InvalidWriteId`).

use serde::Serialize;
use serde_json::Value;
use suaegi_misc::js_trim;

pub const LINEAR_ISSUE_ATTRIBUTE_FILTER_ID_MAX_LENGTH: usize = 256;
pub const LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_STATE_IDS: usize = 100;
pub const LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_LABEL_IDS: usize = 100;
pub const LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_PRIORITIES: usize = 5;

/// Mirrors Orca's `LinearIssueAttributeAssignee` union (`{kind:'user',
/// id} | {kind:'unassigned'}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearIssueAttributeAssignee {
    User { id: String },
    Unassigned,
}

/// Mirrors Orca's `LinearIssueAttributeFilter`. `Default` is the Rust
/// stand-in for both `EMPTY_LINEAR_ISSUE_ATTRIBUTE_FILTER` and
/// `emptyLinearIssueAttributeFilter()` — there is no frozen-singleton-identity
/// concept in Rust to preserve (Q7).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinearIssueAttributeFilter {
    pub state_ids: Vec<String>,
    pub priorities: Vec<u8>,
    pub assignee: Option<LinearIssueAttributeAssignee>,
    pub label_ids: Vec<String>,
}

/// Structured parse-time error (Q5). Carries field name / index / limit
/// where relevant; never the raw offending value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearIssueAttributeFilterError {
    /// Top-level value is JSON `null` (mirrors JS `undefined`/`null`).
    Required,
    /// Top-level value is not a plain JSON object (e.g. an array, string,
    /// or number).
    NotAnObject,
    /// An unrecognized top-level key was present.
    UnknownKey(String),
    /// One or more of the four required keys (`stateIds`, `priorities`,
    /// `assignee`, `labelIds`) is missing.
    MissingRequiredKeys,
    /// `stateIds`/`labelIds` was present but not a JSON array.
    IdArrayNotAnArray { field: &'static str },
    /// `stateIds`/`labelIds` array length exceeds `max`, checked on the raw
    /// array length before any per-entry validation (P2).
    IdArrayExceedsCap { field: &'static str, max: usize },
    /// `field[index]` was not a JSON string.
    IdNotAString { field: &'static str, index: usize },
    /// `field[index]` trimmed (via `js_trim`) to the empty string.
    IdEmpty { field: &'static str, index: usize },
    /// `field[index]`, after trimming, exceeds `max` UTF-16 code units (Q2).
    IdTooLong {
        field: &'static str,
        index: usize,
        max: usize,
    },
    /// `priorities` was present but not a JSON array.
    PrioritiesNotAnArray,
    /// `priorities` array length exceeds `max`.
    PrioritiesExceedsCap { max: usize },
    /// `priorities[index]` fails the single combined Orca check (P5):
    /// non-number, non-integer (fractional), negative, or `> 4`. Message
    /// reads like `priorities[{index}] must be an integer from 0 to 4`,
    /// mirroring `linear-issue-attribute-filter.ts:166-171`'s one `||` chain
    /// and single thrown message — there is deliberately only one variant
    /// here, not one per rejected shape.
    PriorityInvalid { index: usize },
    /// `assignee` is present but is neither JSON `null` nor an object.
    AssigneeNotObjectOrNull,
    /// `assignee.kind` is not `"user"` or `"unassigned"`.
    AssigneeInvalidKind,
    /// The `assignee` object carries keys beyond what its `kind` allows.
    AssigneeUnknownKeys,
    /// `assignee.id` (for `kind: "user"`) was missing or not a string.
    AssigneeIdNotAString,
    /// `assignee.id` trimmed to the empty string.
    AssigneeIdEmpty,
    /// `assignee.id`, after trimming, exceeds `max` UTF-16 code units.
    AssigneeIdTooLong { max: usize },
}

const ATTRIBUTE_FILTER_KEYS: [&str; 4] = ["stateIds", "priorities", "assignee", "labelIds"];

/// `canonicalizeIds` (`:36-49`). Trims (P1), drops empties/dupes, sorts
/// (`str::cmp` — Q1). No cap (P6): caps live only on the parse path.
fn canonicalize_ids(ids: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut next: Vec<String> = Vec::new();
    for raw in ids {
        let id = js_trim(raw);
        if id.is_empty() {
            continue;
        }
        let id = id.to_string();
        if !seen.insert(id.clone()) {
            continue;
        }
        next.push(id);
    }
    // Dedup means no two elements compare equal, so `sort_unstable`'s lack of
    // stability guarantee is unobservable here.
    next.sort_unstable();
    next
}

/// `canonicalizePriorities` (`:51-63`). See Q4 for why the `p < 0`/
/// non-integer drop branches are unreachable with `Vec<u8>`.
fn canonicalize_priorities(priorities: &[u8]) -> Vec<u8> {
    let mut seen: std::collections::HashSet<u8> = std::collections::HashSet::new();
    let mut next: Vec<u8> = Vec::new();
    for &priority in priorities {
        if priority > 4 || !seen.insert(priority) {
            continue;
        }
        next.push(priority);
    }
    next.sort_unstable();
    next
}

/// `canonicalizeAssignee` (`:65-79`). See Q3 for the `User`-with-empty-id →
/// `None` demotion.
fn canonicalize_assignee(
    assignee: Option<LinearIssueAttributeAssignee>,
) -> Option<LinearIssueAttributeAssignee> {
    match assignee {
        None => None,
        Some(LinearIssueAttributeAssignee::Unassigned) => {
            Some(LinearIssueAttributeAssignee::Unassigned)
        }
        Some(LinearIssueAttributeAssignee::User { id }) => {
            let trimmed = js_trim(&id);
            if trimmed.is_empty() {
                // Q3 — surprising but verbatim: demotes to "no assignee
                // filter", not `Unassigned`.
                None
            } else {
                Some(LinearIssueAttributeAssignee::User {
                    id: trimmed.to_string(),
                })
            }
        }
    }
}

/// `canonicalizeLinearIssueAttributeFilter` (`:90-99`). Takes the input by
/// reference and returns a fresh value, so non-mutation of the caller's
/// filter is structural (enforced by the borrow checker), not merely
/// tested.
pub fn canonicalize_linear_issue_attribute_filter(
    filter: &LinearIssueAttributeFilter,
) -> LinearIssueAttributeFilter {
    LinearIssueAttributeFilter {
        state_ids: canonicalize_ids(&filter.state_ids),
        priorities: canonicalize_priorities(&filter.priorities),
        assignee: canonicalize_assignee(filter.assignee.clone()),
        label_ids: canonicalize_ids(&filter.label_ids),
    }
}

/// `isEmptyLinearIssueAttributeFilter` (`:101-114`). `None` counts as empty
/// (P4). Judged *after* canonicalization (P7) — `priorities: [0]` and
/// `assignee: Unassigned` are NOT empty.
pub fn is_empty_linear_issue_attribute_filter(filter: Option<&LinearIssueAttributeFilter>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    let canonical = canonicalize_linear_issue_attribute_filter(filter);
    canonical.state_ids.is_empty()
        && canonical.priorities.is_empty()
        && canonical.assignee.is_none()
        && canonical.label_ids.is_empty()
}

/// Signature-only assignee shape: `kind` then `id` (Q1b), `id` omitted for
/// `unassigned` (matches the JS object literal `{ kind: 'unassigned' }`
/// having no `id` key at all).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssigneeSignature {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

/// Signature JSON shape, field order `stateIds, priorities, assignee,
/// labelIds` (Q1b) — a `#[derive(Serialize)]` struct so serde preserves
/// declaration order, unlike `serde_json::Map`'s alphabetical `BTreeMap`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FilterSignature<'a> {
    state_ids: &'a [String],
    priorities: &'a [u8],
    assignee: Option<AssigneeSignature>,
    label_ids: &'a [String],
}

/// `linearIssueAttributeFilterSignature` (`:116-129`). `None` and "empty
/// after canonicalization" both signature to `""` (P4/P7).
pub fn linear_issue_attribute_filter_signature(
    filter: Option<&LinearIssueAttributeFilter>,
) -> String {
    let Some(filter) = filter else {
        return String::new();
    };
    if is_empty_linear_issue_attribute_filter(Some(filter)) {
        return String::new();
    }
    let canonical = canonicalize_linear_issue_attribute_filter(filter);
    let assignee = canonical.assignee.map(|assignee| match assignee {
        LinearIssueAttributeAssignee::Unassigned => AssigneeSignature {
            kind: "unassigned",
            id: None,
        },
        LinearIssueAttributeAssignee::User { id } => AssigneeSignature {
            kind: "user",
            id: Some(id),
        },
    });
    let payload = FilterSignature {
        state_ids: &canonical.state_ids,
        priorities: &canonical.priorities,
        assignee,
        label_ids: &canonical.label_ids,
    };
    serde_json::to_string(&payload).expect("FilterSignature has no non-serializable fields")
}

/// Internal result of validating a single id string: which failure mode, if
/// any (mapped by the caller into a field-specific
/// [`LinearIssueAttributeFilterError`] variant).
enum IdAssertError {
    NotAString,
    Empty,
    TooLong,
}

/// `assertId` (`:131-145`). Trim (P1) then length-cap in UTF-16 code units
/// (Q2), applied *after* trimming (P2).
fn assert_id(value: &Value) -> Result<String, IdAssertError> {
    let Some(raw) = value.as_str() else {
        return Err(IdAssertError::NotAString);
    };
    let id = js_trim(raw);
    if id.is_empty() {
        return Err(IdAssertError::Empty);
    }
    if id.encode_utf16().count() > LINEAR_ISSUE_ATTRIBUTE_FILTER_ID_MAX_LENGTH {
        return Err(IdAssertError::TooLong);
    }
    Ok(id.to_string())
}

/// `assertIdArray` (`:147-155`). Array-length cap checked on the raw length
/// before any per-entry validation (P2).
fn assert_id_array(
    value: &Value,
    field: &'static str,
    max: usize,
) -> Result<Vec<String>, LinearIssueAttributeFilterError> {
    let Some(entries) = value.as_array() else {
        return Err(LinearIssueAttributeFilterError::IdArrayNotAnArray { field });
    };
    if entries.len() > max {
        return Err(LinearIssueAttributeFilterError::IdArrayExceedsCap { field, max });
    }
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        match assert_id(entry) {
            Ok(id) => out.push(id),
            Err(IdAssertError::NotAString) => {
                return Err(LinearIssueAttributeFilterError::IdNotAString { field, index })
            }
            Err(IdAssertError::Empty) => {
                return Err(LinearIssueAttributeFilterError::IdEmpty { field, index })
            }
            Err(IdAssertError::TooLong) => {
                return Err(LinearIssueAttributeFilterError::IdTooLong {
                    field,
                    index,
                    max: LINEAR_ISSUE_ATTRIBUTE_FILTER_ID_MAX_LENGTH,
                })
            }
        }
    }
    Ok(out)
}

/// `assertPriorities` (`:157-174`). See P5: a single combined check —
/// `typeof entry !== 'number' || !Number.isInteger(entry) || entry < 0 ||
/// entry > 4` — one `||` chain, one thrown message, so one
/// [`LinearIssueAttributeFilterError::PriorityInvalid`] here for every
/// rejected shape. `Number.isInteger` is a value-level check (not a JSON
/// type-level one), so a JSON float with a zero fractional part — `4.0` —
/// is integral and must be accepted, same as the JSON integer `4`.
fn assert_priorities(value: &Value) -> Result<Vec<u8>, LinearIssueAttributeFilterError> {
    let Some(entries) = value.as_array() else {
        return Err(LinearIssueAttributeFilterError::PrioritiesNotAnArray);
    };
    if entries.len() > LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_PRIORITIES {
        return Err(LinearIssueAttributeFilterError::PrioritiesExceedsCap {
            max: LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_PRIORITIES,
        });
    }
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let n = entry
            .as_f64()
            .filter(|n| n.fract() == 0.0 && (0.0..=4.0).contains(n));
        match n {
            Some(n) => out.push(n as u8),
            None => return Err(LinearIssueAttributeFilterError::PriorityInvalid { index }),
        }
    }
    Ok(out)
}

/// `assertAssignee` (`:176-197`). `null` → `Ok(None)` (P4); unknown-key
/// checks per `kind`; `user`'s `id` reuses [`assert_id`].
fn assert_assignee(
    value: &Value,
) -> Result<Option<LinearIssueAttributeAssignee>, LinearIssueAttributeFilterError> {
    if value.is_null() {
        return Ok(None);
    }
    let Some(obj) = value.as_object() else {
        return Err(LinearIssueAttributeFilterError::AssigneeNotObjectOrNull);
    };
    match obj.get("kind").and_then(Value::as_str) {
        Some("unassigned") => {
            if obj.keys().any(|key| key != "kind") {
                return Err(LinearIssueAttributeFilterError::AssigneeUnknownKeys);
            }
            Ok(Some(LinearIssueAttributeAssignee::Unassigned))
        }
        Some("user") => {
            if obj.keys().any(|key| key != "kind" && key != "id") {
                return Err(LinearIssueAttributeFilterError::AssigneeUnknownKeys);
            }
            let id_value = obj.get("id").cloned().unwrap_or(Value::Null);
            match assert_id(&id_value) {
                Ok(id) => Ok(Some(LinearIssueAttributeAssignee::User { id })),
                Err(IdAssertError::NotAString) => {
                    Err(LinearIssueAttributeFilterError::AssigneeIdNotAString)
                }
                Err(IdAssertError::Empty) => Err(LinearIssueAttributeFilterError::AssigneeIdEmpty),
                Err(IdAssertError::TooLong) => {
                    Err(LinearIssueAttributeFilterError::AssigneeIdTooLong {
                        max: LINEAR_ISSUE_ATTRIBUTE_FILTER_ID_MAX_LENGTH,
                    })
                }
            }
        }
        _ => Err(LinearIssueAttributeFilterError::AssigneeInvalidKind),
    }
}

/// `parseLinearIssueAttributeFilter` (`:200-234`). Throwing-parser-turned-`Result`
/// for IPC/RPC wire input. `Value::Null` is an error (P4) — present partial
/// objects are invalid. Check order (P3): null → not-an-object → unknown key
/// → missing required keys → per-field asserts (stateIds → priorities →
/// assignee → labelIds).
pub fn parse_linear_issue_attribute_filter(
    value: &Value,
) -> Result<LinearIssueAttributeFilter, LinearIssueAttributeFilterError> {
    if value.is_null() {
        return Err(LinearIssueAttributeFilterError::Required);
    }
    let Some(obj) = value.as_object() else {
        return Err(LinearIssueAttributeFilterError::NotAnObject);
    };
    for key in obj.keys() {
        if !ATTRIBUTE_FILTER_KEYS.contains(&key.as_str()) {
            return Err(LinearIssueAttributeFilterError::UnknownKey(key.clone()));
        }
    }
    if !obj.contains_key("stateIds")
        || !obj.contains_key("priorities")
        || !obj.contains_key("assignee")
        || !obj.contains_key("labelIds")
    {
        return Err(LinearIssueAttributeFilterError::MissingRequiredKeys);
    }

    let state_ids = assert_id_array(
        &obj["stateIds"],
        "stateIds",
        LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_STATE_IDS,
    )?;
    let priorities = assert_priorities(&obj["priorities"])?;
    let assignee = assert_assignee(&obj["assignee"])?;
    let label_ids = assert_id_array(
        &obj["labelIds"],
        "labelIds",
        LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_LABEL_IDS,
    )?;

    let parsed = LinearIssueAttributeFilter {
        state_ids,
        priorities,
        assignee,
        label_ids,
    };
    Ok(canonicalize_linear_issue_attribute_filter(&parsed))
}

/// `optionalParsedLinearIssueAttributeFilter` (`:236-244`). `None` (absent)
/// short-circuits to `Ok(None)` (P4/Q6) *before* touching
/// [`parse_linear_issue_attribute_filter`], so `Some(Value::Null)` still
/// hits the `Required` error there.
pub fn optional_parsed_linear_issue_attribute_filter(
    value: Option<&Value>,
) -> Result<Option<LinearIssueAttributeFilter>, LinearIssueAttributeFilterError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = parse_linear_issue_attribute_filter(value)?;
    Ok(if is_empty_linear_issue_attribute_filter(Some(&parsed)) {
        None
    } else {
        Some(parsed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> LinearIssueAttributeFilter {
        LinearIssueAttributeFilter {
            state_ids: vec![
                "b".to_string(),
                "a".to_string(),
                "a".to_string(),
                "  c  ".to_string(),
            ],
            priorities: vec![3, 1, 1, 0],
            assignee: Some(LinearIssueAttributeAssignee::User {
                id: "  user-1  ".to_string(),
            }),
            label_ids: vec!["z".to_string(), "y".to_string(), "z".to_string()],
        }
    }

    // ---- oracle case 1 (`test.ts:20-25`) ----

    #[test]
    fn oracle_1_canonical_empty_is_empty_and_signature_less() {
        assert!(is_empty_linear_issue_attribute_filter(Some(
            &LinearIssueAttributeFilter::default()
        )));
        assert!(is_empty_linear_issue_attribute_filter(Some(
            &LinearIssueAttributeFilter::default()
        )));
        assert_eq!(
            linear_issue_attribute_filter_signature(Some(&LinearIssueAttributeFilter::default())),
            ""
        );
        assert_eq!(linear_issue_attribute_filter_signature(None), "");
    }

    // ---- oracle case 2 (`test.ts:27-41`) ----

    #[test]
    fn oracle_2_canonicalize_does_not_mutate_input_and_signature_is_stable() {
        let input = sample();
        let canonical = canonicalize_linear_issue_attribute_filter(&input);
        // non-mutation: `input` must still equal a freshly-built `sample()`.
        assert_eq!(input, sample());
        assert_eq!(
            canonical,
            LinearIssueAttributeFilter {
                state_ids: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                priorities: vec![0, 1, 3],
                assignee: Some(LinearIssueAttributeAssignee::User {
                    id: "user-1".to_string()
                }),
                label_ids: vec!["y".to_string(), "z".to_string()],
            }
        );
        assert_eq!(
            linear_issue_attribute_filter_signature(Some(&input)),
            linear_issue_attribute_filter_signature(Some(&canonical))
        );
        assert!(linear_issue_attribute_filter_signature(Some(&input))
            .contains(r#""priorities":[0,1,3]"#));
    }

    // ---- oracle case 3 (`test.ts:43-57`) ----

    #[test]
    fn oracle_3_signature_changes_when_any_facet_changes() {
        let base = canonicalize_linear_issue_attribute_filter(&sample());
        let base_sig = linear_issue_attribute_filter_signature(Some(&base));

        let mut with_state_ids = base.clone();
        with_state_ids.state_ids = vec!["x".to_string()];
        assert_ne!(
            linear_issue_attribute_filter_signature(Some(&with_state_ids)),
            base_sig
        );

        let mut with_priorities = base.clone();
        with_priorities.priorities = vec![4];
        assert_ne!(
            linear_issue_attribute_filter_signature(Some(&with_priorities)),
            base_sig
        );

        let mut with_assignee = base.clone();
        with_assignee.assignee = Some(LinearIssueAttributeAssignee::Unassigned);
        assert_ne!(
            linear_issue_attribute_filter_signature(Some(&with_assignee)),
            base_sig
        );

        let mut with_label_ids = base.clone();
        with_label_ids.label_ids = vec!["other".to_string()];
        assert_ne!(
            linear_issue_attribute_filter_signature(Some(&with_label_ids)),
            base_sig
        );
    }

    // ---- oracle case 4 (`test.ts:59-69`) — falsy-zero trap (P7) ----

    #[test]
    fn oracle_4_priority_zero_and_unassigned_are_not_empty() {
        let canonical = canonicalize_linear_issue_attribute_filter(&LinearIssueAttributeFilter {
            state_ids: vec![],
            priorities: vec![0],
            assignee: Some(LinearIssueAttributeAssignee::Unassigned),
            label_ids: vec![],
        });
        assert_eq!(canonical.priorities, vec![0]);
        assert_eq!(
            canonical.assignee,
            Some(LinearIssueAttributeAssignee::Unassigned)
        );
        assert!(!is_empty_linear_issue_attribute_filter(Some(&canonical)));
    }

    // ---- oracle case 5 (`test.ts:71-136`) ----

    #[test]
    fn oracle_5_parses_valid_input() {
        let value = json!({
            "stateIds": ["s1"],
            "priorities": [0, 2],
            "assignee": { "kind": "unassigned" },
            "labelIds": ["l1"]
        });
        let parsed = parse_linear_issue_attribute_filter(&value).unwrap();
        assert_eq!(
            parsed,
            LinearIssueAttributeFilter {
                state_ids: vec!["s1".to_string()],
                priorities: vec![0, 2],
                assignee: Some(LinearIssueAttributeAssignee::Unassigned),
                label_ids: vec!["l1".to_string()],
            }
        );
    }

    #[test]
    fn oracle_5_missing_required_keys_rejected() {
        let value = json!({ "stateIds": [] });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::MissingRequiredKeys)
        );
    }

    #[test]
    fn oracle_5_unknown_key_rejected() {
        let value = json!({
            "stateIds": [],
            "priorities": [],
            "assignee": null,
            "labelIds": [],
            "extra": true
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::UnknownKey(
                "extra".to_string()
            ))
        );
    }

    #[test]
    fn oracle_5_empty_state_id_rejected() {
        let value = json!({
            "stateIds": [""],
            "priorities": [],
            "assignee": null,
            "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdEmpty {
                field: "stateIds",
                index: 0
            })
        );
    }

    #[test]
    fn oracle_5_non_integer_priority_rejected() {
        let value = json!({
            "stateIds": [],
            "priorities": [1.5],
            "assignee": null,
            "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::PriorityInvalid { index: 0 })
        );
    }

    #[test]
    fn oracle_5_out_of_range_priority_rejected() {
        let value = json!({
            "stateIds": [],
            "priorities": [5],
            "assignee": null,
            "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::PriorityInvalid { index: 0 })
        );
    }

    #[test]
    fn oracle_5_missing_assignee_id_rejected() {
        let value = json!({
            "stateIds": [],
            "priorities": [],
            "assignee": { "kind": "user" },
            "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::AssigneeIdNotAString)
        );
    }

    #[test]
    fn oracle_5_state_ids_over_cap_rejected() {
        let ids: Vec<Value> = (0..101).map(|i| json!(format!("s{i}"))).collect();
        let value = json!({
            "stateIds": ids,
            "priorities": [],
            "assignee": null,
            "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdArrayExceedsCap {
                field: "stateIds",
                max: 100
            })
        );
    }

    // ==== additional pins (oracle-silent) ====

    // ---- P2: exact-limit passes, limit+1 rejected; array cap before per-entry ----

    #[test]
    fn p2_exactly_100_state_ids_pass_101_rejected() {
        let ok: Vec<Value> = (0..100).map(|i| json!(format!("s{i}"))).collect();
        let value = json!({ "stateIds": ok, "priorities": [], "assignee": null, "labelIds": [] });
        assert!(parse_linear_issue_attribute_filter(&value).is_ok());

        let bad: Vec<Value> = (0..101).map(|i| json!(format!("s{i}"))).collect();
        let value = json!({ "stateIds": bad, "priorities": [], "assignee": null, "labelIds": [] });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdArrayExceedsCap {
                field: "stateIds",
                max: 100
            })
        );
    }

    #[test]
    fn p2_exactly_100_label_ids_pass_101_rejected() {
        let ok: Vec<Value> = (0..100).map(|i| json!(format!("l{i}"))).collect();
        let value = json!({ "stateIds": [], "priorities": [], "assignee": null, "labelIds": ok });
        assert!(parse_linear_issue_attribute_filter(&value).is_ok());

        let bad: Vec<Value> = (0..101).map(|i| json!(format!("l{i}"))).collect();
        let value = json!({ "stateIds": [], "priorities": [], "assignee": null, "labelIds": bad });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdArrayExceedsCap {
                field: "labelIds",
                max: 100
            })
        );
    }

    #[test]
    fn p2_exactly_5_priorities_pass_6_rejected() {
        let value = json!({
            "stateIds": [], "priorities": [0, 1, 2, 3, 4], "assignee": null, "labelIds": []
        });
        assert!(parse_linear_issue_attribute_filter(&value).is_ok());

        let value = json!({
            "stateIds": [], "priorities": [0, 1, 2, 3, 4, 0], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::PrioritiesExceedsCap { max: 5 })
        );
    }

    #[test]
    fn p2_exactly_256_id_length_passes_257_rejected() {
        let ok_id = "x".repeat(256);
        let value = json!({
            "stateIds": [ok_id], "priorities": [], "assignee": null, "labelIds": []
        });
        assert!(parse_linear_issue_attribute_filter(&value).is_ok());

        let bad_id = "x".repeat(257);
        let value = json!({
            "stateIds": [bad_id], "priorities": [], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdTooLong {
                field: "stateIds",
                index: 0,
                max: 256
            })
        );
    }

    /// P2 crux: a 101-element array whose 0th entry is also invalid (empty
    /// string) must still report the CAP error, not the per-entry error —
    /// the array-length check runs strictly before per-entry validation.
    #[test]
    fn p2_101_element_array_with_bad_entry_reports_cap_error() {
        let mut ids: Vec<Value> = vec![json!("")];
        ids.extend((1..101).map(|i| json!(format!("s{i}"))));
        assert_eq!(ids.len(), 101);
        let value = json!({ "stateIds": ids, "priorities": [], "assignee": null, "labelIds": [] });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdArrayExceedsCap {
                field: "stateIds",
                max: 100
            })
        );
    }

    // ---- P4: null vs absent per function ----

    #[test]
    fn p4_parse_null_errors() {
        assert_eq!(
            parse_linear_issue_attribute_filter(&Value::Null),
            Err(LinearIssueAttributeFilterError::Required)
        );
    }

    #[test]
    fn p4_optional_parsed_absent_is_ok_none() {
        assert_eq!(
            optional_parsed_linear_issue_attribute_filter(None),
            Ok(None)
        );
    }

    #[test]
    fn p4_optional_parsed_some_null_errors() {
        assert_eq!(
            optional_parsed_linear_issue_attribute_filter(Some(&Value::Null)),
            Err(LinearIssueAttributeFilterError::Required)
        );
    }

    #[test]
    fn p4_assignee_null_ok_but_missing_key_required_error() {
        let value = json!({
            "stateIds": [], "priorities": [], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value)
                .unwrap()
                .assignee,
            None
        );

        let value = json!({ "stateIds": [], "priorities": [], "labelIds": [] });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::MissingRequiredKeys)
        );
    }

    // ---- Q3: user assignee whose id trims to empty demotes to None ----

    #[test]
    fn q3_user_assignee_blank_id_canonicalizes_to_none() {
        let canonical = canonicalize_linear_issue_attribute_filter(&LinearIssueAttributeFilter {
            state_ids: vec![],
            priorities: vec![],
            assignee: Some(LinearIssueAttributeAssignee::User {
                id: "   ".to_string(),
            }),
            label_ids: vec![],
        });
        assert_eq!(canonical.assignee, None);
    }

    // ---- P1: U+FEFF-only id is dropped/rejected; `str::trim` would not strip it ----

    #[test]
    fn p1_feff_only_id_canonicalizes_away() {
        // Documents the divergence: Rust `str::trim()` does NOT strip U+FEFF,
        // so a naive port would keep this id; `js_trim` strips it to empty.
        assert_eq!("\u{FEFF}".trim(), "\u{FEFF}");
        let canonical = canonicalize_ids(&["\u{FEFF}".to_string()]);
        assert!(canonical.is_empty());
    }

    #[test]
    fn p1_feff_only_id_rejected_by_parse() {
        let value = json!({
            "stateIds": ["\u{FEFF}"], "priorities": [], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdEmpty {
                field: "stateIds",
                index: 0
            })
        );
    }

    // ---- Q2: UTF-16 code unit length cap, not bytes/chars ----

    #[test]
    fn q2_256_non_ascii_chars_pass_257_rejected() {
        let ok_id = "あ".repeat(256);
        assert_eq!(ok_id.encode_utf16().count(), 256);
        let value = json!({
            "stateIds": [ok_id], "priorities": [], "assignee": null, "labelIds": []
        });
        assert!(parse_linear_issue_attribute_filter(&value).is_ok());

        let bad_id = "あ".repeat(257);
        let value = json!({
            "stateIds": [bad_id], "priorities": [], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdTooLong {
                field: "stateIds",
                index: 0,
                max: 256
            })
        );
    }

    /// Emoji astral characters are 1 `char` but 2 UTF-16 code units each; 128
    /// of them is exactly the 256-unit cap boundary and must pass.
    #[test]
    fn q2_128_emoji_chars_is_256_utf16_units_and_passes() {
        let id = "\u{1F600}".repeat(128);
        assert_eq!(id.chars().count(), 128);
        assert_eq!(id.encode_utf16().count(), 256);
        let value = json!({
            "stateIds": [id], "priorities": [], "assignee": null, "labelIds": []
        });
        assert!(parse_linear_issue_attribute_filter(&value).is_ok());
    }

    /// Why: 129 astral (non-BMP, surrogate-pair) chars is the case that
    /// separates a UTF-16-code-unit cap from a `chars()`-count cap: it is
    /// 258 UTF-16 units (> 256, must be REJECTED) but only 129 `char`s
    /// (<= 256, so a `chars().count()` implementation would wrongly ACCEPT
    /// it). Neither existing pin discriminates this: `q2_256_non_ascii...`
    /// uses BMP `あ`, where UTF-16-unit count equals char count; and
    /// `q2_128_emoji_chars...` sits exactly at the 256-unit/128-char
    /// boundary, where both a UTF-16 cap and a chars cap agree (both
    /// `<= 256`). Only past that boundary, with an odd astral count like
    /// 129, do the two length notions diverge and disagree on the verdict.
    #[test]
    fn q2_129_astral_chars_is_258_utf16_units_and_is_rejected() {
        let id = "\u{1F600}".repeat(129);
        assert_eq!(id.chars().count(), 129);
        assert_eq!(id.encode_utf16().count(), 258);
        let value = json!({
            "stateIds": [id], "priorities": [], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::IdTooLong {
                field: "stateIds",
                index: 0,
                max: 256
            })
        );
    }

    // ---- P5: fractional, out-of-range, negative, and non-number priorities
    // all collapse to the SAME single error variant (Orca's one `||` chain,
    // `linear-issue-attribute-filter.ts:166-171`) ----

    #[test]
    fn p5_fractional_and_out_of_range_priorities_are_the_same_error() {
        let fractional = json!({
            "stateIds": [], "priorities": [1.5], "assignee": null, "labelIds": []
        });
        let out_of_range = json!({
            "stateIds": [], "priorities": [5], "assignee": null, "labelIds": []
        });
        let fractional_err = parse_linear_issue_attribute_filter(&fractional).unwrap_err();
        let range_err = parse_linear_issue_attribute_filter(&out_of_range).unwrap_err();
        assert_eq!(
            fractional_err,
            LinearIssueAttributeFilterError::PriorityInvalid { index: 0 }
        );
        assert_eq!(
            range_err,
            LinearIssueAttributeFilterError::PriorityInvalid { index: 0 }
        );
        assert_eq!(fractional_err, range_err);
    }

    /// Regression pin: a JSON *float* `4.0` is `Number.isInteger` in JS
    /// (value-level, not JSON-type-level) and must be ACCEPTED, canonicalizing
    /// to the `u8` `4` — not rejected as "not an integer".
    #[test]
    fn p5_float_4_0_is_accepted_and_canonicalizes_to_4() {
        let value = json!({
            "stateIds": [], "priorities": [4.0], "assignee": null, "labelIds": []
        });
        let parsed = parse_linear_issue_attribute_filter(&value).unwrap();
        assert_eq!(parsed.priorities, vec![4u8]);
    }

    #[test]
    fn p5_negative_priority_is_priority_invalid() {
        let value = json!({
            "stateIds": [], "priorities": [-1], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::PriorityInvalid { index: 0 })
        );
    }

    #[test]
    fn p5_string_priority_is_priority_invalid() {
        let value = json!({
            "stateIds": [], "priorities": ["3"], "assignee": null, "labelIds": []
        });
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::PriorityInvalid { index: 0 })
        );
    }

    // ---- P7: falsy-zero / unassigned are non-empty (also pinned via oracle_4) ----

    #[test]
    fn p7_priorities_zero_is_non_empty() {
        let filter = LinearIssueAttributeFilter {
            state_ids: vec![],
            priorities: vec![0],
            assignee: None,
            label_ids: vec![],
        };
        assert!(!is_empty_linear_issue_attribute_filter(Some(&filter)));
    }

    #[test]
    fn p7_unassigned_is_non_empty() {
        let filter = LinearIssueAttributeFilter {
            state_ids: vec![],
            priorities: vec![],
            assignee: Some(LinearIssueAttributeAssignee::Unassigned),
            label_ids: vec![],
        };
        assert!(!is_empty_linear_issue_attribute_filter(Some(&filter)));
    }

    // ---- P6: non-ASCII ids sort deterministically ----

    #[test]
    fn p6_non_ascii_ids_sort_deterministically() {
        let canonical = canonicalize_ids(&[
            "\u{3042}".to_string(), // あ
            "b".to_string(),
            "\u{00E9}".to_string(), // é
            "a".to_string(),
        ]);
        // `str::cmp` (byte/code-point order): 'a' < 'b' < 'é' (U+00E9) < 'あ' (U+3042).
        assert_eq!(
            canonical,
            vec![
                "a".to_string(),
                "b".to_string(),
                "\u{00E9}".to_string(),
                "\u{3042}".to_string(),
            ]
        );
    }

    // ---- top-level array input ----

    #[test]
    fn top_level_array_input_is_not_an_object() {
        let value = json!([1, 2, 3]);
        assert_eq!(
            parse_linear_issue_attribute_filter(&value),
            Err(LinearIssueAttributeFilterError::NotAnObject)
        );
    }
}
