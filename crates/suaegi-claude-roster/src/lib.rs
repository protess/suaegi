//! VERBATIM port of Orca's `src/shared/claude-subagent-roster.ts` (295 lines).
//!
//! Ported: `O:1` [`AGENT_STATUS_MAX_SUBAGENTS`] (local mirror, see below),
//! `O:6` [`CLAUDE_SUBAGENT_ID_MAX_LENGTH`], `O:17` [`ClaudeSubagentRoster`],
//! `O:19-35` [`TrackedClaudeSubagent`], `O:40-49`
//! [`ClaudeBackgroundAgentTask`], `O:54-57`
//! [`is_claude_teammate_lifecycle_id`], `O:59-88`
//! [`upsert_working_claude_subagent`] (+ [`ClaudeSubagentFields`]), `O:94-96`
//! [`finish_claude_subagent`], `O:101-138`
//! [`read_claude_background_agent_tasks`] (+ [`HookPayload`],
//! [`HookBackgroundTasksField`], [`HookTaskElement`], [`HookTaskObject`],
//! [`HookTaskKind`], [`ReadBackgroundAgentTasksResult`]), `O:159-243`
//! [`fold_claude_background_tasks_into_roster`] (+ [`FoldOptions`]),
//! `O:249-252` [`claude_teammate_id_matches_name`], `O:260-269`
//! [`remove_claude_teammate_by_name`], `O:271-273`
//! [`claude_roster_has_working_subagent`], `O:275-295`
//! [`claude_roster_to_snapshots`].
//!
//! Two items are carried in **locally** from Orca's `agent-status-types.ts`
//! (imported at `O:1` of the source), each documented as mirroring the
//! upstream to be unified when that module lands (per the plan's §3
//! follow-up ①②): [`AGENT_STATUS_MAX_SUBAGENTS`] (that file's line ~242,
//! value `32`) and [`AgentSubagentSnapshot`] + the full 4-variant
//! [`AgentSubagentState`] (that file's line ~69/~73). The roster only ever
//! emits `Working` snapshots, but the other three variants are load-bearing
//! for the (not-yet-ported) seeding caller that filters on
//! `state !== 'working'`.
//!
//! # Traps (see the plan's §1 for full rationale; `I<N>` numbering matches
//! # `docs/superpowers/plans/2026-07-26-claude-subagent-roster.md`)
//! - **I1**: `??` (`O:70`,`:71`,`:186`,`:187`) falls through ONLY on
//!   absent — an empty-string `agent_type`/`description` field
//!   (`Some(String::new())`) OVERWRITES the tracked value; only `None`
//!   preserves it. Implemented as `if let Some(v) = fields.agent_type {
//!   existing.agent_type = Some(v) }` — never `unwrap_or_default`, never
//!   "update if non-empty". No oracle fixture supplies an empty string (all
//!   fixtures supply either non-empty or absent), so this is
//!   hand-pinned in both [`upsert_working_claude_subagent`] and the fold's
//!   inline merge (`O:186`,`:187`).
//! - **I2**: `options?.inventoryComplete !== false` (`O:166`, `O:211`) means
//!   ABSENT ⇒ COMPLETE. Absent `options`, an absent field, and an explicit
//!   `true` all mean "complete" (clear on empty list; run the sweep); only an
//!   explicit `false` suppresses. Modeled as
//!   `options.and_then(|o| o.inventory_complete) != Some(false)` — a plain
//!   `bool` field defaulting to `false` would invert the entire sweep.
//! - **I3**: the id-length guard (`O:65`) fires for NO oracle fixture
//!   (neither an empty nor a >64-unit id is ever upserted by the 26 cases) —
//!   dropping the guard entirely still passes all of them, so it is
//!   hand-pinned at both boundaries. `.length` is UTF-16 code units, so this
//!   is `id.encode_utf16().count()`, NOT `id.len()` (UTF-8 bytes) and NOT
//!   `id.chars().count()` (code points — collapses each astral character to
//!   1 instead of the 2 UTF-16 units JS counts).
//! - **I4/I5**: the `.trim()` at `O:120` is a LENGTH TEST ONLY; the id
//!   actually stored/pushed (`O:130`, and the id parameter threaded through
//!   [`upsert_working_claude_subagent`]) is UNTRIMMED. `" a1 "` must be
//!   stored with its spaces and therefore must NOT match a roster key
//!   `"a1"`. The emptiness test uses [`suaegi_misc::js_trim`], which differs
//!   from `str::trim` at U+FEFF (JS whitespace, Rust is not) and U+0085 (JS
//!   is not whitespace, Rust `str::trim` is) — see `suaegi-misc`'s own pins.
//! - **I6**: the hex check (`O:56`, `/^[0-9a-f]+$/i`) is ASCII-only — no
//!   `/u` flag, so the `i` case-fold never widens past ASCII. Implemented
//!   with `char::is_ascii_hexdigit`; NOT the `regex` crate (whose `(?i)` is
//!   Unicode case folding and would accept e.g. Kelvin sign U+212A as `k`).
//! - **I7**: `id.startsWith('a')` (`O:56`) is case-sensitive —
//!   `"Aprobe1-ff"` is `false`. Every oracle fixture starts with lowercase
//!   `a`, so an accidentally case-insensitive port (`eq_ignore_ascii_case`)
//!   would be invisible; hand-pinned.
//! - **I8**: `separator > 1` (`O:56`) is strict: a hyphen at index one,
//!   e.g. `"a-ff"`, yields `false`. No fixture has a hyphen at that index,
//!   so `>= 1` also passes the oracle; hand-pinned.
//!   [`is_claude_teammate_lifecycle_id`] uses `str::rfind('-')`, which
//!   returns a BYTE offset, not a UTF-16 code unit offset like JS
//!   `lastIndexOf`. These provably never diverge for the strict-greater-
//!   than-one test specifically: the function already requires
//!   `id.starts_with('a')` (one ASCII byte equals one UTF-16 unit), so the
//!   boundary can only flip between the two encodings if the hyphen is the
//!   literal second character (both encodings give the same offset there,
//!   both `false`) — any character between position zero and the hyphen
//!   adds at least as many bytes as UTF-16 units (one-for-one if ASCII,
//!   more bytes than units otherwise), so both counts cross the threshold
//!   together. A byte-index `rfind` is therefore safe here, unlike the
//!   general case in I9/I10.
//! - **I9/I10**: the sort comparator (`O:293`) is explicit:
//!   `startedAt` ascending, then id. Implemented as
//!   `started_at.cmp(&started_at).then_with(|| js_utf16_cmp(id, id))` —
//!   never subtraction (i64 overflow risk, and it loses JS's
//!   `NaN`-falls-through-to-tiebreak behavior, moot here since `startedAt`
//!   is never NaN in this i64 model but the reflex is still wrong to carry
//!   forward). JS `<`/`>` compare strings in UTF-16 code-unit order; Rust
//!   `str::cmp` compares in UTF-8 byte (= Unicode code point) order — they
//!   diverge for ids mixing astral characters (U+10000+) with characters in
//!   U+E000..=U+FFFF. [`js_utf16_cmp`] compares via `encode_utf16()` to
//!   match JS exactly; this is a documented, hand-pinned policy choice (no
//!   oracle fixture uses non-ASCII ids) — a plain `str::cmp` port would
//!   still pass all 26 oracle cases, including the ASCII-only sort case at
//!   `T:365-372`.
//! - **I11**: all three caps (`O:80`, `O:123`, `O:228`) are `>=`, all
//!   "refuse the newest, evict nothing". The UPDATE path in
//!   [`upsert_working_claude_subagent`] runs BEFORE its cap check
//!   (`O:68-77` before `O:80`), so updates to existing rows succeed
//!   regardless of roster size. `O:123`'s cap counts POST-FILTER entries
//!   (after the type/id checks at `O:117`,`:120`), so 32 valid tasks
//!   followed by garbage still yields `truncated: false` — the garbage
//!   element never reaches the cap check because it's filtered out first.
//! - **I12**: both tracked flags
//!   ([`TrackedClaudeSubagent::background_tasks_authoritative`],
//!   [`TrackedClaudeSubagent::listed_as_subagent_task`]) are modeled as
//!   plain `bool`, turning the TS `= undefined` clear (`O:75`) into
//!   `= false`. `Option<bool>` + `!= Some(true)` would make every oracle
//!   fixture's `undefined` and `false` cases identical (neither value is
//!   ever `Some(false)` in the 26 cases), hiding the real asymmetry between
//!   `O:218`'s truthiness negation (`!tracked.backgroundTasksAuthoritative`,
//!   `undefined` and `false` both negate to `true`) and `O:219`'s strict
//!   compare (`!== true`, same result for `undefined`/`false`) — under a
//!   plain `bool` model these collapse to `!flag` and `flag != true`
//!   respectively, which are the same predicate, matching the source
//!   exactly rather than merely by coincidence of unexercised fixtures.
//! - **I13**: `pendingRunningTasks`' (`O:172`) insertion order IS
//!   observable — the cap `break` at `O:228` decides who gets the last free
//!   slot when the roster fills up mid-reconcile. Modeled as
//!   `Vec<(String, ClaudeBackgroundAgentTask)>` with hand-written
//!   `pending_set`/`pending_delete` helpers that reproduce `Map.set`
//!   (existing key: value replaced in place, position unchanged; new key:
//!   appended) / `Map.delete` (no-op if absent) semantics exactly — NOT a
//!   `HashMap`, which would randomize which id wins the last cap slot.
//!   By contrast [`ClaudeSubagentRoster`] ITSELF is order-independent: its
//!   only external observation point, [`claude_roster_to_snapshots`], sorts
//!   by `(started_at, id)` (I9/I10) — a total order that erases any
//!   insertion-order signal — so `HashMap` is fine there. Say so here since
//!   a green test run alone is not evidence for that choice.
//! - **I14**: `O:228`'s cap check is REDUNDANT with `O:80`'s (reached only
//!   via [`upsert_working_claude_subagent`], which re-checks it) and is
//!   unobservable under every current fixture, but is kept VERBATIM — it
//!   becomes load-bearing the moment `upsert_working_claude_subagent`'s cap
//!   semantics change without this call site being revisited.
//! - **I15**: `hasTeammateTypedTask` (`O:173`) is computed over the FULL
//!   task array BEFORE the loop (`O:174`) skips teammate-typed entries.
//!   Moving `.some()` after/inside the loop would make it structurally
//!   `false` (nothing teammate-typed ever reaches a post-skip scan). The
//!   oracle DOES catch this one (cases at `T:194,213,228,251,268`).
//! - **I16**: reconciliation is by id ONLY (`O:179`) — never by name, agent
//!   type, description, or index (oracle case at `T:354-363` pins the
//!   absence of an agent-type fallback). Sweep survival
//!   (`O:216-222`) requires ALL FOUR conditions; removal is the default.
//! - **I17**: the roster is mutated in place by every function here AND is
//!   also seeded directly by an external caller with the authoritative flag
//!   pre-set (`agent-hook-listener.ts:2400-2405`, not ported here — see
//!   `T:297-309`'s `roster.set(id, { ...backgroundTasksAuthoritative: true
//!   })`). [`ClaudeSubagentRoster`] is a plain
//!   `HashMap<String, TrackedClaudeSubagent>` alias with every
//!   [`TrackedClaudeSubagent`] field `pub`, so `roster.insert(id.to_string(),
//!   TrackedClaudeSubagent { .. })` IS that public seed path — no separate
//!   constructor function is needed or added, matching the TS surface
//!   (a bare `Map`) exactly.

use std::collections::{HashMap, HashSet};

use suaegi_misc::js_trim;

// ---------------------------------------------------------------------------
// Locally-mirrored items from `agent-status-types.ts` (see module doc).
// ---------------------------------------------------------------------------

/// Mirrors `agent-status-types.ts`'s `AGENT_STATUS_MAX_SUBAGENTS` (that
/// file's line ~242). To be unified with a single shared definition once
/// `agent-status-types.ts` is ported (plan §3 follow-up ②).
pub const AGENT_STATUS_MAX_SUBAGENTS: usize = 32;

/// Mirrors `agent-status-types.ts`'s `AgentSubagentState` (that file's line
/// ~69) — the full 4-variant union. This roster only ever emits `Working`
/// (see [`claude_roster_to_snapshots`]), but the other three variants are
/// carried because the (not-yet-ported) seeding caller filters snapshots on
/// `state !== 'working'`, so dropping them would be an incomplete mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSubagentState {
    Working,
    Blocked,
    Waiting,
    Idle,
}

/// Mirrors `agent-status-types.ts`'s `AgentSubagentSnapshot` (that file's
/// line ~73). `model` is never populated by this module (this roster's
/// tracked entries carry no model field at all) — always `None` in every
/// snapshot [`claude_roster_to_snapshots`] emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSubagentSnapshot {
    pub id: String,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub description: Option<String>,
    pub state: AgentSubagentState,
    pub started_at: i64,
}

// ---------------------------------------------------------------------------
// O:6 CLAUDE_SUBAGENT_ID_MAX_LENGTH
// ---------------------------------------------------------------------------

/// `O:6`. I3: `.length` in the source is UTF-16 code units — compared
/// against `id.encode_utf16().count()`, never `id.len()`/`id.chars().count()`.
const CLAUDE_SUBAGENT_ID_MAX_LENGTH: usize = 64;

// ---------------------------------------------------------------------------
// O:17 ClaudeSubagentRoster / O:19-35 TrackedClaudeSubagent
// ---------------------------------------------------------------------------

/// `O:17`. I13: order-independent (see module doc) — safe as a `HashMap`.
/// I17: every [`TrackedClaudeSubagent`] field is `pub`, so external seeding
/// (`roster.insert(id, TrackedClaudeSubagent { .. })`) is the public seed
/// path; no separate constructor is added.
pub type ClaudeSubagentRoster = HashMap<String, TrackedClaudeSubagent>;

/// `O:19-35`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedClaudeSubagent {
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub started_at: i64,
    /// `O:28`. I12: plain `bool`; the TS `= undefined` clear (`O:75`) is
    /// modeled as `= false`.
    pub background_tasks_authoritative: bool,
    /// `O:34`. I12: plain `bool` (TS type is `true | undefined`, i.e. never
    /// explicitly `false` in the source — this is its only valid non-absent
    /// value, so `bool` loses nothing).
    pub listed_as_subagent_task: bool,
}

// ---------------------------------------------------------------------------
// O:59-62 fields argument of upsertWorkingClaudeSubagent
// ---------------------------------------------------------------------------

/// The `fields` parameter object at `O:62`.
#[derive(Debug, Clone, Default)]
pub struct ClaudeSubagentFields {
    pub agent_type: Option<String>,
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// O:40-49 ClaudeBackgroundAgentTask
// ---------------------------------------------------------------------------

/// `O:40-49`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeBackgroundAgentTask {
    pub id: String,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub running: bool,
    pub teammate: bool,
}

// ---------------------------------------------------------------------------
// O:54-57 isClaudeTeammateLifecycleId
// ---------------------------------------------------------------------------

/// `O:54-57`. I6/I7/I8: ASCII-only hex, case-sensitive `a` prefix, strict
/// `separator > 1`.
pub fn is_claude_teammate_lifecycle_id(id: &str) -> bool {
    // I7: case-sensitive, never `eq_ignore_ascii_case`.
    if !id.starts_with('a') {
        return false;
    }
    // I8: byte-index rfind is safe here specifically — see module doc I8.
    let Some(separator) = id.rfind('-') else {
        return false;
    };
    if separator <= 1 {
        return false;
    }
    let suffix = &id[separator + 1..];
    // `/^[0-9a-f]+$/i` requires at least one character (`+`) and is
    // ASCII-only (I6): no `/u` flag, so `is_ascii_hexdigit` matches exactly.
    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// O:59-88 upsertWorkingClaudeSubagent
// ---------------------------------------------------------------------------

/// `O:59-88`. I1: `??` overwrites on `Some` (even empty string), preserves
/// only on `None`. I3: UTF-16 length guard, unreachable by any oracle
/// fixture but hand-pinned at both boundaries. I11: the update path runs
/// BEFORE the cap check, so existing rows always succeed.
pub fn upsert_working_claude_subagent(
    roster: &mut ClaudeSubagentRoster,
    id: &str,
    fields: ClaudeSubagentFields,
    now: i64,
) {
    let unit_len = id.encode_utf16().count();
    if unit_len == 0 || unit_len > CLAUDE_SUBAGENT_ID_MAX_LENGTH {
        return;
    }
    if let Some(existing) = roster.get_mut(id) {
        // I1: `fields.agentType ?? existing.agentType` — overwrite on
        // `Some(_)` (including `Some("")`), preserve on `None`.
        if let Some(agent_type) = fields.agent_type {
            existing.agent_type = Some(agent_type);
        }
        if let Some(description) = fields.description {
            existing.description = Some(description);
        }
        // O:75. I12: `= undefined` -> `= false`.
        existing.background_tasks_authoritative = false;
        return;
    }
    // O:80. I11: `>=`, refuse the newest, evict nothing.
    if roster.len() >= AGENT_STATUS_MAX_SUBAGENTS {
        return;
    }
    roster.insert(
        // I4/I5: the id stored here is UNTRIMMED — the caller's `id` as-is.
        id.to_string(),
        TrackedClaudeSubagent {
            started_at: now,
            agent_type: fields.agent_type,
            description: fields.description,
            background_tasks_authoritative: false,
            listed_as_subagent_task: false,
        },
    );
}

// ---------------------------------------------------------------------------
// O:94-96 finishClaudeSubagent
// ---------------------------------------------------------------------------

/// `O:94-96`.
pub fn finish_claude_subagent(roster: &mut ClaudeSubagentRoster, id: &str) {
    roster.remove(id);
}

// ---------------------------------------------------------------------------
// O:101-138 readClaudeBackgroundAgentTasks (+ hand-rolled input surface)
// ---------------------------------------------------------------------------

/// Hand-rolled stand-in for the `Record<string, unknown>` hook payload at
/// `O:101`. The function only ever reads one key (`background_tasks`,
/// `O:106`), and `Array.isArray` is `false` for EVERY non-array JS value
/// (`null`, a plain object, a number, a string, or an absent key alike) —
/// the source never distinguishes those cases — so this models exactly that
/// one lookup instead of a general JSON value.
#[derive(Debug, Clone)]
pub struct HookPayload {
    pub background_tasks: HookBackgroundTasksField,
}

/// `hookPayload['background_tasks']` (`O:106`), reduced to its one
/// observable distinction: is it an array.
#[derive(Debug, Clone, Default)]
pub enum HookBackgroundTasksField {
    /// `background_tasks` was a JS array.
    Array(Vec<HookTaskElement>),
    /// Anything else: absent key, `null`, a plain object, a number, a
    /// string, etc. — all observably identical (`O:107`).
    #[default]
    NotArray,
}

/// One element of the `background_tasks` array (`O:112`).
#[derive(Debug, Clone)]
pub enum HookTaskElement {
    /// `typeof item === 'object' && item !== null` (`O:113`).
    Object(HookTaskObject),
    /// Any non-object-like element (a string, a number, `null`, an array).
    Other,
}

/// The observable surface of one object-like `background_tasks` element:
/// is `type` one of the two exact literals; is `id` a string (and, if so,
/// its untrimmed value — the emptiness test is applied separately, `O:120`);
/// are `agent_type`/`description` strings; is `status` exactly `"running"`.
/// Nothing else about the element (extra keys, key order, nesting) is ever
/// observed by the source, so nothing else is modeled.
#[derive(Debug, Clone)]
pub struct HookTaskObject {
    pub kind: HookTaskKind,
    /// `Some(_)` only if `typeof obj.id === 'string'` (`O:120`); the string
    /// is UNTRIMMED (I4/I5) — trimming is used only to test emptiness.
    pub id: Option<String>,
    /// `Some(_)` only if `typeof obj.agent_type === 'string'` (`O:131`).
    pub agent_type: Option<String>,
    /// `Some(_)` only if `typeof obj.description === 'string'` (`O:132`).
    pub description: Option<String>,
    /// `obj.status === 'running'` (`O:133`).
    pub running: bool,
}

/// `obj.type` (`O:117`), reduced to its two literal matches — anything else
/// (including a non-string, `undefined`, or any other string) is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTaskKind {
    Subagent,
    Teammate,
    Other,
}

/// Return value of [`read_claude_background_agent_tasks`] (`O:101-105`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadBackgroundAgentTasksResult {
    pub present: bool,
    pub tasks: Vec<ClaudeBackgroundAgentTask>,
    pub truncated: bool,
}

/// `O:101-138`. I3: UTF-16-length-adjacent emptiness test via
/// [`suaegi_misc::js_trim`] (length-test only, I4/I5 — the pushed id stays
/// untrimmed). I6: ASCII-only hex is irrelevant here (this function doesn't
/// call [`is_claude_teammate_lifecycle_id`]); I11: the cap (`O:123`) counts
/// POST-FILTER entries only.
pub fn read_claude_background_agent_tasks(payload: &HookPayload) -> ReadBackgroundAgentTasksResult {
    let raw = match &payload.background_tasks {
        HookBackgroundTasksField::Array(items) => items,
        HookBackgroundTasksField::NotArray => {
            return ReadBackgroundAgentTasksResult {
                present: false,
                tasks: Vec::new(),
                truncated: false,
            };
        }
    };
    let mut tasks: Vec<ClaudeBackgroundAgentTask> = Vec::new();
    let mut truncated = false;
    for item in raw {
        let HookTaskElement::Object(obj) = item else {
            continue;
        };
        if obj.kind != HookTaskKind::Subagent && obj.kind != HookTaskKind::Teammate {
            continue;
        }
        let Some(id) = &obj.id else {
            continue;
        };
        // I4/I5: `.trim().length === 0` is a length test ONLY; `id` (below)
        // stays untrimmed.
        if js_trim(id).is_empty() {
            continue;
        }
        if tasks.len() >= AGENT_STATUS_MAX_SUBAGENTS {
            truncated = true;
            break;
        }
        tasks.push(ClaudeBackgroundAgentTask {
            id: id.clone(),
            agent_type: obj.agent_type.clone(),
            description: obj.description.clone(),
            running: obj.running,
            teammate: obj.kind == HookTaskKind::Teammate,
        });
    }
    ReadBackgroundAgentTasksResult {
        present: true,
        tasks,
        truncated,
    }
}

// ---------------------------------------------------------------------------
// O:159-243 foldClaudeBackgroundTasksIntoRoster
// ---------------------------------------------------------------------------

/// `options` parameter of [`fold_claude_background_tasks_into_roster`]
/// (`O:163`). I2: `inventory_complete: None` (absent) and `Some(true)` both
/// mean complete; only `Some(false)` suppresses.
#[derive(Debug, Clone, Copy, Default)]
pub struct FoldOptions {
    pub inventory_complete: Option<bool>,
}

/// I13: last-wins upsert reproducing `Map.set` order semantics — an
/// existing key's value is replaced IN PLACE (position unchanged); a new
/// key is appended.
fn pending_set(
    pending: &mut Vec<(String, ClaudeBackgroundAgentTask)>,
    id: String,
    task: ClaudeBackgroundAgentTask,
) {
    if let Some(entry) = pending
        .iter_mut()
        .find(|(existing_id, _)| *existing_id == id)
    {
        entry.1 = task;
    } else {
        pending.push((id, task));
    }
}

/// I13: reproduces `Map.delete` — a no-op if the id isn't present.
fn pending_delete(pending: &mut Vec<(String, ClaudeBackgroundAgentTask)>, id: &str) {
    pending.retain(|(existing_id, _)| existing_id != id);
}

/// `O:159-243`. I2/I11/I12/I13/I15/I16 all apply here — see module doc.
pub fn fold_claude_background_tasks_into_roster(
    roster: &mut ClaudeSubagentRoster,
    tasks: &[ClaudeBackgroundAgentTask],
    now: i64,
    options: Option<FoldOptions>,
) {
    // I2: absent options/field, or an explicit `true`, both mean complete;
    // only an explicit `false` suppresses.
    let inventory_complete = options.and_then(|o| o.inventory_complete) != Some(false);

    if tasks.is_empty() {
        if inventory_complete {
            roster.clear();
        }
        return;
    }

    let mut listed_ids: HashSet<String> = HashSet::new();
    let mut pending_running_tasks: Vec<(String, ClaudeBackgroundAgentTask)> = Vec::new();
    // I15: computed over the FULL array, before the loop below skips
    // teammate-typed entries.
    let has_teammate_typed_task = tasks.iter().any(|task| task.teammate);

    for task in tasks {
        if task.teammate {
            continue;
        }
        // I16: reconciliation is by id only.
        listed_ids.insert(task.id.clone());
        if roster.contains_key(&task.id) {
            if !task.running {
                roster.remove(&task.id);
                pending_delete(&mut pending_running_tasks, &task.id);
                continue;
            }
            let existing = roster
                .get_mut(&task.id)
                .expect("checked contains_key above");
            // I1: overwrite on `Some(_)`, preserve on `None`.
            if let Some(agent_type) = &task.agent_type {
                existing.agent_type = Some(agent_type.clone());
            }
            if let Some(description) = &task.description {
                existing.description = Some(description.clone());
            }
            existing.listed_as_subagent_task = true;
            continue;
        }
        if !task.running {
            pending_delete(&mut pending_running_tasks, &task.id);
            continue;
        }
        upsert_working_claude_subagent(
            roster,
            &task.id,
            ClaudeSubagentFields {
                agent_type: task.agent_type.clone(),
                description: task.description.clone(),
            },
            now,
        );
        if let Some(created) = roster.get_mut(&task.id) {
            created.background_tasks_authoritative = true;
            created.listed_as_subagent_task = true;
        } else {
            // Why: a full roster may still contain stale entries this same
            // inventory will reap below. Retry after cleanup.
            pending_set(&mut pending_running_tasks, task.id.clone(), task.clone());
        }
    }

    if inventory_complete {
        // I16: sweep survival requires ALL FOUR conditions; removal is the
        // default. Collected first to avoid mutating `roster` mid-iteration.
        let ids_to_remove: Vec<String> = roster
            .iter()
            .filter(|(id, tracked)| {
                if listed_ids.contains(*id) {
                    return false;
                }
                if has_teammate_typed_task
                    && !tracked.background_tasks_authoritative
                    && !tracked.listed_as_subagent_task
                    && is_claude_teammate_lifecycle_id(id)
                {
                    return false;
                }
                true
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids_to_remove {
            roster.remove(&id);
        }
    }

    // I13: iteration order over `pending_running_tasks` (a `Vec`, not a
    // `HashMap`) decides who gets the last free slot at the cap.
    for (id, task) in pending_running_tasks {
        // O:228. I14: kept verbatim though unobservable today (redundant
        // with the cap check inside `upsert_working_claude_subagent`).
        if roster.len() >= AGENT_STATUS_MAX_SUBAGENTS {
            break;
        }
        upsert_working_claude_subagent(
            roster,
            &id,
            ClaudeSubagentFields {
                agent_type: task.agent_type.clone(),
                description: task.description.clone(),
            },
            now,
        );
        if let Some(created) = roster.get_mut(&id) {
            created.background_tasks_authoritative = true;
            created.listed_as_subagent_task = true;
        }
    }
}

// ---------------------------------------------------------------------------
// O:249-252 claudeTeammateIdMatchesName
// ---------------------------------------------------------------------------

/// `O:249-252`.
pub fn claude_teammate_id_matches_name(id: &str, name: &str) -> bool {
    let prefix = format!("a{name}-");
    id.starts_with(&prefix) && !id[prefix.len()..].contains('-')
}

// ---------------------------------------------------------------------------
// O:260-269 removeClaudeTeammateByName
// ---------------------------------------------------------------------------

/// `O:260-269`.
pub fn remove_claude_teammate_by_name(roster: &mut ClaudeSubagentRoster, name: &str) -> bool {
    let ids_to_remove: Vec<String> = roster
        .keys()
        .filter(|id| claude_teammate_id_matches_name(id, name))
        .cloned()
        .collect();
    let changed = !ids_to_remove.is_empty();
    for id in ids_to_remove {
        roster.remove(&id);
    }
    changed
}

// ---------------------------------------------------------------------------
// O:271-273 claudeRosterHasWorkingSubagent
// ---------------------------------------------------------------------------

/// `O:271-273`.
pub fn claude_roster_has_working_subagent(roster: Option<&ClaudeSubagentRoster>) -> bool {
    roster.is_some_and(|roster| !roster.is_empty())
}

// ---------------------------------------------------------------------------
// O:275-295 claudeRosterToSnapshots
// ---------------------------------------------------------------------------

/// I9/I10: compares ids in UTF-16 code-unit order to match JS `<`/`>`
/// string comparison exactly (`str::cmp` compares in UTF-8 byte / code
/// point order and diverges for astral characters vs U+E000..=U+FFFF).
fn js_utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `O:275-295`. I9/I10: explicit `(started_at, id)` comparator, never
/// subtraction.
pub fn claude_roster_to_snapshots(
    roster: Option<&ClaudeSubagentRoster>,
) -> Option<Vec<AgentSubagentSnapshot>> {
    let roster = roster?;
    if roster.is_empty() {
        return None;
    }
    let mut snapshots: Vec<AgentSubagentSnapshot> = roster
        .iter()
        .map(|(id, tracked)| AgentSubagentSnapshot {
            id: id.clone(),
            agent_type: tracked.agent_type.clone(),
            // Never populated by this module (I: absence of `model`).
            model: None,
            description: tracked.description.clone(),
            state: AgentSubagentState::Working,
            started_at: tracked.started_at,
        })
        .collect();
    snapshots.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| js_utf16_cmp(&a.id, &b.id))
    });
    Some(snapshots)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a [`ClaudeBackgroundAgentTask`] with the oracle's `task()`
    /// helper defaults (`T:14-24`): `id: "a1"`, no `agent_type`/
    /// `description`, `running: true`, `teammate: false`.
    fn task(id: &str) -> ClaudeBackgroundAgentTask {
        ClaudeBackgroundAgentTask {
            id: id.to_string(),
            agent_type: None,
            description: None,
            running: true,
            teammate: false,
        }
    }

    fn fields(agent_type: Option<&str>, description: Option<&str>) -> ClaudeSubagentFields {
        ClaudeSubagentFields {
            agent_type: agent_type.map(str::to_string),
            description: description.map(str::to_string),
        }
    }

    fn hook_payload(items: Vec<HookTaskElement>) -> HookPayload {
        HookPayload {
            background_tasks: HookBackgroundTasksField::Array(items),
        }
    }

    fn subagent_obj(id: Option<&str>, status: &str) -> HookTaskElement {
        HookTaskElement::Object(HookTaskObject {
            kind: HookTaskKind::Subagent,
            id: id.map(str::to_string),
            agent_type: None,
            description: None,
            running: status == "running",
        })
    }

    // -------------------------------------------------------------------
    // Oracle: T:27-37
    // -------------------------------------------------------------------
    #[test]
    fn oracle_removes_a_finished_one_shot_subagent_on_stop() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "a1",
            fields(Some("general-purpose"), None),
            100,
        );
        assert!(claude_roster_has_working_subagent(Some(&roster)));

        finish_claude_subagent(&mut roster, "a1");
        assert_eq!(roster.len(), 0);
        assert_eq!(claude_roster_to_snapshots(Some(&roster)), None);
    }

    // T:39-47
    #[test]
    fn oracle_removes_a_finished_teammate_shaped_named_agent_on_stop() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "aprobe1-6d3cb5b5",
            fields(Some("probe1"), None),
            100,
        );
        finish_claude_subagent(&mut roster, "aprobe1-6d3cb5b5");
        assert!(!roster.contains_key("aprobe1-6d3cb5b5"));
    }

    // T:49-58
    #[test]
    fn oracle_re_adds_a_resumed_agent_as_working_with_a_fresh_started_at() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "aprobe1-6d3cb5b5",
            fields(Some("probe1"), None),
            100,
        );
        finish_claude_subagent(&mut roster, "aprobe1-6d3cb5b5");
        upsert_working_claude_subagent(
            &mut roster,
            "aprobe1-6d3cb5b5",
            fields(None, Some("round two")),
            200,
        );
        let entry = roster.get("aprobe1-6d3cb5b5").unwrap();
        assert_eq!(entry.started_at, 200);
        assert_eq!(entry.description.as_deref(), Some("round two"));
    }

    // T:60-64
    #[test]
    fn oracle_ignores_unknown_ids_on_finish_claude_subagent() {
        let mut roster = ClaudeSubagentRoster::new();
        finish_claude_subagent(&mut roster, "ghost");
        assert_eq!(roster.len(), 0);
    }

    // T:66-82
    #[test]
    fn oracle_drops_new_spawns_at_the_cap_rather_than_evicting_live_children() {
        let mut roster = ClaudeSubagentRoster::new();
        for i in 0..AGENT_STATUS_MAX_SUBAGENTS {
            upsert_working_claude_subagent(
                &mut roster,
                &format!("a{i}"),
                fields(None, None),
                i as i64,
            );
        }
        upsert_working_claude_subagent(&mut roster, "overflow", fields(None, None), 999);
        assert!(!roster.contains_key("overflow"));
        assert_eq!(roster.len(), AGENT_STATUS_MAX_SUBAGENTS);

        finish_claude_subagent(&mut roster, "a0");
        upsert_working_claude_subagent(&mut roster, "replacement", fields(None, None), 1000);
        assert!(roster.contains_key("replacement"));
        assert_eq!(roster.len(), AGENT_STATUS_MAX_SUBAGENTS);
    }

    // T:84-100
    #[test]
    fn oracle_reconciles_stale_entries_before_adding_replacement_tasks_at_the_cap() {
        let mut roster = ClaudeSubagentRoster::new();
        for i in 0..AGENT_STATUS_MAX_SUBAGENTS {
            upsert_working_claude_subagent(
                &mut roster,
                &format!("a{i}"),
                fields(None, None),
                i as i64,
            );
        }
        let tasks: Vec<ClaudeBackgroundAgentTask> = (0..AGENT_STATUS_MAX_SUBAGENTS)
            .map(|index| {
                task(&if index == 0 {
                    "replacement".to_string()
                } else {
                    format!("a{index}")
                })
            })
            .collect();

        fold_claude_background_tasks_into_roster(&mut roster, &tasks, 999, None);

        assert!(!roster.contains_key("a0"));
        assert!(roster.contains_key("replacement"));
        assert_eq!(roster.len(), AGENT_STATUS_MAX_SUBAGENTS);
    }

    // T:102-136
    #[test]
    fn oracle_reads_only_agent_typed_background_tasks_entries() {
        let payload = hook_payload(vec![
            HookTaskElement::Object(HookTaskObject {
                kind: HookTaskKind::Subagent,
                id: Some("a1".to_string()),
                agent_type: Some("general-purpose".to_string()),
                description: Some("review loop".to_string()),
                running: true,
            }),
            HookTaskElement::Object(HookTaskObject {
                kind: HookTaskKind::Teammate,
                id: Some("t1".to_string()),
                agent_type: Some("code-reviewer".to_string()),
                description: None,
                running: false,
            }),
            HookTaskElement::Object(HookTaskObject {
                kind: HookTaskKind::Other,
                id: Some("s1".to_string()),
                agent_type: None,
                description: Some("npm run dev".to_string()),
                running: true,
            }),
            subagent_obj(Some(""), "running"),
            HookTaskElement::Other,
        ]);
        let result = read_claude_background_agent_tasks(&payload);
        assert!(result.present);
        assert_eq!(
            result.tasks,
            vec![
                ClaudeBackgroundAgentTask {
                    id: "a1".to_string(),
                    agent_type: Some("general-purpose".to_string()),
                    description: Some("review loop".to_string()),
                    running: true,
                    teammate: false,
                },
                ClaudeBackgroundAgentTask {
                    id: "t1".to_string(),
                    agent_type: Some("code-reviewer".to_string()),
                    description: None,
                    running: false,
                    teammate: true,
                },
            ]
        );
    }

    // T:137-140
    #[test]
    fn oracle_reports_background_tasks_as_absent_when_missing_or_malformed() {
        assert!(
            !read_claude_background_agent_tasks(&HookPayload {
                background_tasks: HookBackgroundTasksField::NotArray,
            })
            .present
        );
        assert!(
            !read_claude_background_agent_tasks(&HookPayload {
                background_tasks: HookBackgroundTasksField::NotArray,
            })
            .present
        );
    }

    // T:142-151
    #[test]
    fn oracle_marks_a_background_task_inventory_truncated_after_the_snapshot_cap() {
        let items: Vec<HookTaskElement> = (0..=AGENT_STATUS_MAX_SUBAGENTS)
            .map(|index| subagent_obj(Some(&format!("a{index}")), "running"))
            .collect();
        let result = read_claude_background_agent_tasks(&hook_payload(items));
        assert_eq!(result.tasks.len(), AGENT_STATUS_MAX_SUBAGENTS);
        assert!(result.truncated);
    }

    // T:153-176
    #[test]
    fn oracle_trusts_id_exact_subagent_matches_and_ignores_teammate_typed_entries() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        upsert_working_claude_subagent(
            &mut roster,
            "ateam-6d3cb5b5",
            fields(Some("security-reviewer"), None),
            150,
        );

        fold_claude_background_tasks_into_roster(
            &mut roster,
            &[
                ClaudeBackgroundAgentTask {
                    id: "a1".to_string(),
                    agent_type: Some("general-purpose".to_string()),
                    description: Some("review loop".to_string()),
                    running: true,
                    teammate: false,
                },
                ClaudeBackgroundAgentTask {
                    id: "tlkjjs0jv".to_string(),
                    agent_type: None,
                    description: Some("teammate task".to_string()),
                    running: true,
                    teammate: true,
                },
            ],
            200,
            None,
        );

        assert_eq!(roster.len(), 2);
        assert_eq!(
            roster.get("a1").unwrap().description.as_deref(),
            Some("review loop")
        );
        assert_eq!(
            roster.get("ateam-6d3cb5b5").unwrap().agent_type.as_deref(),
            Some("security-reviewer")
        );
    }

    // T:177-183
    #[test]
    fn oracle_removes_an_id_matched_subagent_task_reported_not_running() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "a1",
            fields(Some("general-purpose"), None),
            100,
        );
        let mut t = task("a1");
        t.running = false;
        fold_claude_background_tasks_into_roster(&mut roster, &[t], 200, None);
        assert_eq!(roster.len(), 0);
    }

    // T:184-193
    #[test]
    fn oracle_removes_a_killed_one_shot_missing_from_a_present_list() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "akilled0000000001",
            fields(Some("general-purpose"), None),
            100,
        );
        let mut other = task("other");
        other.teammate = true;
        fold_claude_background_tasks_into_roster(&mut roster, &[other], 200, None);
        assert_eq!(roster.len(), 0);
    }

    // T:194-212
    #[test]
    fn oracle_keeps_a_live_named_agent_whose_teammate_typed_id_never_appears_in_the_list() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "aweb-research-8a76b7d7",
            fields(Some("web-research"), None),
            100,
        );
        let t = ClaudeBackgroundAgentTask {
            id: "t6s2brfv7".to_string(),
            agent_type: None,
            description: Some("named agent task".to_string()),
            running: true,
            teammate: true,
        };
        fold_claude_background_tasks_into_roster(&mut roster, &[t], 200, None);
        assert!(roster.contains_key("aweb-research-8a76b7d7"));
    }

    // T:213-227
    #[test]
    fn oracle_removes_teammate_shaped_leftovers_when_a_complete_inventory_lists_no_teammates() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "acr-triage-1-c5a0588e",
            fields(Some("cr-triage-1"), None),
            100,
        );
        let mut t = task("aunrelated0000001");
        t.agent_type = Some("general-purpose".to_string());
        fold_claude_background_tasks_into_roster(&mut roster, &[t], 200, None);
        assert!(!roster.contains_key("acr-triage-1-c5a0588e"));
        assert!(roster.contains_key("aunrelated0000001"));
    }

    // T:228-250
    #[test]
    fn oracle_removes_a_leftover_once_a_subagent_typed_task_listed_its_id_id_exact() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "av1-streaming-0b1c2d3e",
            fields(Some("v1-streaming"), None),
            100,
        );
        let mut listed = task("av1-streaming-0b1c2d3e");
        listed.agent_type = Some("v1-streaming".to_string());
        let mut teammate_task = task("tteam1");
        teammate_task.teammate = true;
        fold_claude_background_tasks_into_roster(
            &mut roster,
            &[listed, teammate_task.clone()],
            200,
            None,
        );
        fold_claude_background_tasks_into_roster(&mut roster, &[teammate_task], 300, None);
        assert!(!roster.contains_key("av1-streaming-0b1c2d3e"));
    }

    // T:251-267
    #[test]
    fn oracle_retains_an_unlisted_live_child_when_the_background_task_inventory_was_truncated() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "alive-after-cap", fields(None, None), 100);
        let items: Vec<HookTaskElement> = (0..=AGENT_STATUS_MAX_SUBAGENTS)
            .map(|index| {
                let id = if index == AGENT_STATUS_MAX_SUBAGENTS {
                    "alive-after-cap".to_string()
                } else {
                    format!("a{index}")
                };
                subagent_obj(Some(&id), "running")
            })
            .collect();
        let parsed = read_claude_background_agent_tasks(&hook_payload(items));

        fold_claude_background_tasks_into_roster(
            &mut roster,
            &parsed.tasks,
            200,
            Some(FoldOptions {
                inventory_complete: Some(!parsed.truncated),
            }),
        );
        assert!(roster.contains_key("alive-after-cap"));
    }

    // T:268-282
    #[test]
    fn oracle_recreates_unmatched_running_one_shot_subagents_after_a_listener_restart() {
        let mut roster = ClaudeSubagentRoster::new();
        let mut a9 = task("a9");
        a9.agent_type = Some("general-purpose".to_string());
        a9.description = Some("long build".to_string());
        let mut gone = task("gone");
        gone.running = false;
        fold_claude_background_tasks_into_roster(&mut roster, &[a9, gone], 500, None);
        assert_eq!(roster.get("a9").unwrap().started_at, 500);
        assert!(!roster.contains_key("gone"));
    }

    // T:283-289
    #[test]
    fn oracle_clears_the_roster_when_background_tasks_reports_nothing_alive() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        fold_claude_background_tasks_into_roster(&mut roster, &[], 100, None);
        assert_eq!(roster.len(), 0);
    }

    // T:290-296
    #[test]
    fn oracle_does_not_clear_the_roster_on_an_empty_but_incomplete_truncated_inventory() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        fold_claude_background_tasks_into_roster(
            &mut roster,
            &[],
            200,
            Some(FoldOptions {
                inventory_complete: Some(false),
            }),
        );
        assert!(roster.contains_key("a1"));
    }

    // T:297-310
    #[test]
    fn oracle_removes_a_seeded_phantom_missing_from_a_present_list() {
        let mut roster = ClaudeSubagentRoster::new();
        // I17: the public seed path — direct HashMap insertion.
        roster.insert(
            "aprobe1-6d3cb5b5".to_string(),
            TrackedClaudeSubagent {
                agent_type: Some("probe1".to_string()),
                description: None,
                started_at: 100,
                background_tasks_authoritative: true,
                listed_as_subagent_task: false,
            },
        );
        let mut other = task("other");
        other.teammate = true;
        fold_claude_background_tasks_into_roster(&mut roster, &[other], 200, None);
        assert!(!roster.contains_key("aprobe1-6d3cb5b5"));
    }

    // T:311-324
    #[test]
    fn oracle_keeps_a_re_tracked_working_named_agent_missing_from_a_present_list() {
        let mut roster = ClaudeSubagentRoster::new();
        roster.insert(
            "aprobe1-6d3cb5b5".to_string(),
            TrackedClaudeSubagent {
                agent_type: Some("probe1".to_string()),
                description: None,
                started_at: 100,
                background_tasks_authoritative: true,
                listed_as_subagent_task: false,
            },
        );
        upsert_working_claude_subagent(
            &mut roster,
            "aprobe1-6d3cb5b5",
            fields(Some("probe1"), None),
            150,
        );
        let mut other = task("other");
        other.teammate = true;
        fold_claude_background_tasks_into_roster(&mut roster, &[other], 200, None);
        assert!(roster.contains_key("aprobe1-6d3cb5b5"));
    }

    // T:325-331
    #[test]
    fn oracle_removes_fold_recreated_one_shots_missing_from_a_later_present_list() {
        let mut roster = ClaudeSubagentRoster::new();
        fold_claude_background_tasks_into_roster(&mut roster, &[task("a9")], 100, None);
        let mut other = task("other");
        other.teammate = true;
        fold_claude_background_tasks_into_roster(&mut roster, &[other], 200, None);
        assert!(!roster.contains_key("a9"));
    }

    // T:332-339
    #[test]
    fn oracle_matches_teammate_ids_by_name_only_up_to_the_hyphen_free_suffix() {
        assert!(claude_teammate_id_matches_name(
            "aprobe1-6d3cb5b5",
            "probe1"
        ));
        assert!(claude_teammate_id_matches_name(
            "alane-hooks-6d3cb5b5",
            "lane-hooks"
        ));
        assert!(!claude_teammate_id_matches_name(
            "alane-hooks-6d3cb5b5",
            "lane"
        ));
        assert!(!claude_teammate_id_matches_name(
            "aprobe1-6d3cb5b5",
            "probe"
        ));
        assert!(!claude_teammate_id_matches_name("aprobe1", "probe1"));
    }

    // T:340-353
    #[test]
    fn oracle_removes_teammates_by_the_name_embedded_in_agent_id() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "aprobe1-6d3cb5b5",
            fields(Some("probe1"), None),
            100,
        );
        upsert_working_claude_subagent(&mut roster, "aother-123", fields(Some("other"), None), 100);

        assert!(remove_claude_teammate_by_name(&mut roster, "probe1"));
        assert!(!roster.contains_key("aprobe1-6d3cb5b5"));
        assert!(roster.contains_key("aother-123"));
        assert!(!remove_claude_teammate_by_name(&mut roster, "probe1"));
        assert!(!remove_claude_teammate_by_name(&mut roster, "ghost"));
    }

    // T:354-364
    #[test]
    fn oracle_does_not_remove_an_unrelated_one_shot_whose_agent_type_matches_the_teammate_name() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "aoneshot00000001",
            fields(Some("reviewer"), None),
            100,
        );
        assert!(!remove_claude_teammate_by_name(&mut roster, "reviewer"));
        assert!(roster.contains_key("aoneshot00000001"));
    }

    // T:365-375
    #[test]
    fn oracle_serializes_snapshots_deterministically_ordered_by_started_at_then_id() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "b", fields(None, None), 200);
        upsert_working_claude_subagent(&mut roster, "z", fields(None, None), 100);
        upsert_working_claude_subagent(&mut roster, "a", fields(None, None), 100);
        let snapshots = claude_roster_to_snapshots(Some(&roster)).unwrap();
        let ids: Vec<&str> = snapshots.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "z", "b"]);
        assert!(snapshots
            .iter()
            .all(|s| s.state == AgentSubagentState::Working));
        assert_eq!(
            claude_roster_to_snapshots(Some(&ClaudeSubagentRoster::new())),
            None
        );
    }

    // =====================================================================
    // Hand-written pins (§2 "추가 핀")
    // =====================================================================

    // I1: empty-string agent_type overwrites; `None` preserves — upsert path.
    #[test]
    fn i1_empty_string_overwrites_but_none_preserves_on_upsert() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(Some("probe"), Some("desc")), 100);
        upsert_working_claude_subagent(&mut roster, "a1", fields(Some(""), None), 200);
        let entry = roster.get("a1").unwrap();
        assert_eq!(entry.agent_type.as_deref(), Some(""));
        assert_eq!(entry.description.as_deref(), Some("desc"));
    }

    // I1: empty-string agent_type overwrites; `None` preserves — fold merge path.
    #[test]
    fn i1_empty_string_overwrites_but_none_preserves_on_fold_merge() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(Some("probe"), Some("desc")), 100);
        let mut t = task("a1");
        t.agent_type = Some(String::new());
        t.description = None;
        fold_claude_background_tasks_into_roster(&mut roster, &[t], 200, None);
        let entry = roster.get("a1").unwrap();
        assert_eq!(entry.agent_type.as_deref(), Some(""));
        assert_eq!(entry.description.as_deref(), Some("desc"));
    }

    // I2: default/absent options run the sweep (clears an empty-list roster).
    #[test]
    fn i2_sweep_runs_under_default_absent_options() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        fold_claude_background_tasks_into_roster(&mut roster, &[], 200, None);
        assert_eq!(roster.len(), 0);
    }

    // I2: `Some(FoldOptions { inventory_complete: None })` (field absent, options present) still sweeps.
    #[test]
    fn i2_sweep_runs_when_options_present_but_field_absent() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        fold_claude_background_tasks_into_roster(
            &mut roster,
            &[],
            200,
            Some(FoldOptions {
                inventory_complete: None,
            }),
        );
        assert_eq!(roster.len(), 0);
    }

    // I2: explicit `true` also sweeps.
    #[test]
    fn i2_sweep_runs_on_explicit_true() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        fold_claude_background_tasks_into_roster(
            &mut roster,
            &[],
            200,
            Some(FoldOptions {
                inventory_complete: Some(true),
            }),
        );
        assert_eq!(roster.len(), 0);
    }

    // I2: ONLY explicit `false` suppresses.
    #[test]
    fn i2_only_explicit_false_suppresses_sweep() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        fold_claude_background_tasks_into_roster(
            &mut roster,
            &[],
            200,
            Some(FoldOptions {
                inventory_complete: Some(false),
            }),
        );
        assert!(roster.contains_key("a1"));
    }

    // I3: empty id refused.
    #[test]
    fn i3_empty_id_refused() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "", fields(None, None), 100);
        assert_eq!(roster.len(), 0);
    }

    // I3: exactly 64 UTF-16 units accepted.
    #[test]
    fn i3_64_unit_id_accepted() {
        let mut roster = ClaudeSubagentRoster::new();
        let id = "a".repeat(64);
        assert_eq!(id.encode_utf16().count(), 64);
        upsert_working_claude_subagent(&mut roster, &id, fields(None, None), 100);
        assert!(roster.contains_key(&id));
    }

    // I3: 65 UTF-16 units refused.
    #[test]
    fn i3_65_unit_id_refused() {
        let mut roster = ClaudeSubagentRoster::new();
        let id = "a".repeat(65);
        assert_eq!(id.encode_utf16().count(), 65);
        upsert_working_claude_subagent(&mut roster, &id, fields(None, None), 100);
        assert!(!roster.contains_key(&id));
    }

    // I3: 64 ASTRAL characters = 128 UTF-16 units -> refused. This is what
    // separates `encode_utf16().count()` from `chars().count()` (which
    // would see only 64 and wrongly accept it).
    #[test]
    fn i3_64_astral_chars_is_128_utf16_units_and_is_refused() {
        let mut roster = ClaudeSubagentRoster::new();
        // U+1F600 GRINNING FACE: one `char`, two UTF-16 code units.
        let id: String = "\u{1F600}".repeat(64);
        assert_eq!(id.chars().count(), 64);
        assert_eq!(id.encode_utf16().count(), 128);
        upsert_working_claude_subagent(&mut roster, &id, fields(None, None), 100);
        assert!(!roster.contains_key(&id));
    }

    // I4/I5: a U+FEFF-padded id is rejected as empty in the reader's
    // emptiness test (ECMAScript whitespace includes FEFF).
    #[test]
    fn i4_feff_padded_id_is_rejected_as_empty() {
        let payload = hook_payload(vec![subagent_obj(Some("\u{FEFF}"), "running")]);
        let result = read_claude_background_agent_tasks(&payload);
        assert_eq!(result.tasks.len(), 0);
    }

    // I4/I5: a U+0085-padded id is NOT rejected (ECMAScript whitespace
    // excludes NEL, unlike Rust's Unicode-based `str::trim`).
    #[test]
    fn i5_u0085_padded_id_is_not_rejected() {
        let payload = hook_payload(vec![subagent_obj(Some("\u{0085}a1\u{0085}"), "running")]);
        let result = read_claude_background_agent_tasks(&payload);
        assert_eq!(result.tasks.len(), 1);
        // I4/I5: stored id is UNTRIMMED — the U+0085 padding survives.
        assert_eq!(result.tasks[0].id, "\u{0085}a1\u{0085}");
    }

    // I4/I5: `" a1 "` is stored with its spaces and does NOT match a
    // roster key `"a1"`.
    #[test]
    fn i5_untrimmed_id_does_not_match_trimmed_roster_key() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "a1", fields(None, None), 100);
        upsert_working_claude_subagent(&mut roster, " a1 ", fields(Some("padded"), None), 200);
        assert!(roster.contains_key("a1"));
        assert!(roster.contains_key(" a1 "));
        assert_eq!(roster.len(), 2);
        assert_eq!(roster.get("a1").unwrap().agent_type, None);
    }

    // I6: uppercase hex accepted (`/i` is ASCII-only case folding, not Unicode).
    #[test]
    fn i6_uppercase_hex_accepted() {
        assert!(is_claude_teammate_lifecycle_id("ateam-6D3CB5B5"));
    }

    // I6: non-ASCII "hex-like" characters are rejected.
    #[test]
    fn i6_non_ascii_hex_rejected() {
        // Cyrillic а (U+0430) resembles Latin 'a' but must not match
        // `is_ascii_hexdigit`.
        assert!(!is_claude_teammate_lifecycle_id("ateam-6d3\u{0430}b5b5"));
    }

    // I7: `"Aprobe1-ff"` (uppercase `A`) is false — case-sensitive.
    #[test]
    fn i7_uppercase_a_prefix_is_false() {
        assert!(!is_claude_teammate_lifecycle_id("Aprobe1-ff"));
    }

    // I8: `"a-ff"` (hyphen at index 1) is false.
    #[test]
    fn i8_hyphen_at_index_one_is_false() {
        assert!(!is_claude_teammate_lifecycle_id("a-ff"));
    }

    // I8: `"ateam-"` (empty suffix) is false.
    #[test]
    fn i8_empty_suffix_is_false() {
        assert!(!is_claude_teammate_lifecycle_id("ateam-"));
    }

    // I8: `"bteam-ff"` (doesn't start with 'a') is false.
    #[test]
    fn i8_non_a_prefix_is_false() {
        assert!(!is_claude_teammate_lifecycle_id("bteam-ff"));
    }

    // I9: equal started_at falls through to the id tiebreak.
    #[test]
    fn i9_equal_started_at_falls_back_to_id_tiebreak() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(&mut roster, "z", fields(None, None), 100);
        upsert_working_claude_subagent(&mut roster, "a", fields(None, None), 100);
        let snapshots = claude_roster_to_snapshots(Some(&roster)).unwrap();
        let ids: Vec<&str> = snapshots.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "z"]);
    }

    // I9: non-ASCII ordering is UTF-16 code-unit order — pinned choice.
    // U+E000 (private-use, 1 UTF-16 unit) sorts BEFORE U+10000 (astral, a
    // surrogate pair starting at 0xD800) in code-unit order because 0xD800
    // < 0xE000, even though U+10000 > U+E000 as a code point (so `str::cmp`,
    // which compares code points, would order them the other way).
    #[test]
    fn i9_non_ascii_ordering_is_utf16_code_unit_order() {
        let mut roster = ClaudeSubagentRoster::new();
        let astral = "\u{10000}"; // surrogate pair 0xD800 0xDC00
        let pua = "\u{E000}"; // single unit 0xE000
                              // Under plain `str::cmp` (code point order) `pua < astral`, since
                              // U+E000 < U+10000 as a code point — the OPPOSITE of the UTF-16
                              // order this function must produce. That divergence is the point.
        assert!(pua < astral);
        upsert_working_claude_subagent(&mut roster, pua, fields(None, None), 100);
        upsert_working_claude_subagent(&mut roster, astral, fields(None, None), 100);
        let snapshots = claude_roster_to_snapshots(Some(&roster)).unwrap();
        let ids: Vec<&str> = snapshots.iter().map(|s| s.id.as_str()).collect();
        // UTF-16 code-unit order: astral's leading surrogate 0xD800 < 0xE000.
        assert_eq!(ids, vec![astral, pua]);
    }

    // I11: exactly-32-valid boundary yields `truncated: false`.
    #[test]
    fn i11_exactly_32_valid_yields_not_truncated() {
        let items: Vec<HookTaskElement> = (0..AGENT_STATUS_MAX_SUBAGENTS)
            .map(|index| subagent_obj(Some(&format!("a{index}")), "running"))
            .collect();
        let result = read_claude_background_agent_tasks(&hook_payload(items));
        assert_eq!(result.tasks.len(), AGENT_STATUS_MAX_SUBAGENTS);
        assert!(!result.truncated);
    }

    // I11: 32 valid + garbage yields `truncated: false` (garbage is
    // filtered before the cap check is ever reached).
    #[test]
    fn i11_32_valid_plus_garbage_yields_not_truncated() {
        let mut items: Vec<HookTaskElement> = (0..AGENT_STATUS_MAX_SUBAGENTS)
            .map(|index| subagent_obj(Some(&format!("a{index}")), "running"))
            .collect();
        items.push(HookTaskElement::Other);
        items.push(subagent_obj(Some(""), "running"));
        let result = read_claude_background_agent_tasks(&hook_payload(items));
        assert_eq!(result.tasks.len(), AGENT_STATUS_MAX_SUBAGENTS);
        assert!(!result.truncated);
    }

    // I11: an update to an existing row succeeds at a full roster.
    #[test]
    fn i11_update_succeeds_at_a_full_roster() {
        let mut roster = ClaudeSubagentRoster::new();
        for i in 0..AGENT_STATUS_MAX_SUBAGENTS {
            upsert_working_claude_subagent(
                &mut roster,
                &format!("a{i}"),
                fields(None, None),
                i as i64,
            );
        }
        upsert_working_claude_subagent(&mut roster, "a0", fields(Some("updated"), None), 999);
        assert_eq!(roster.len(), AGENT_STATUS_MAX_SUBAGENTS);
        assert_eq!(
            roster.get("a0").unwrap().agent_type.as_deref(),
            Some("updated")
        );
    }

    // I12: all four combinations of the two tracked flags against the sweep.
    #[test]
    fn i12_flag_matrix_against_the_sweep() {
        // (authoritative, listed_as_subagent_task) -> survives when
        // teammate-shaped, unlisted, has_teammate_typed_task true.
        let cases = [
            (false, false, true), // survives: teammate-shaped protection applies
            (true, false, false), // authoritative -> proof overrides, removed
            (false, true, false), // listed_as_subagent_task -> proof overrides, removed
            (true, true, false),  // both set -> removed
        ];
        for (authoritative, listed, expect_survives) in cases {
            let mut roster = ClaudeSubagentRoster::new();
            roster.insert(
                "ateam-6d3cb5b5".to_string(),
                TrackedClaudeSubagent {
                    agent_type: Some("teamed".to_string()),
                    description: None,
                    started_at: 100,
                    background_tasks_authoritative: authoritative,
                    listed_as_subagent_task: listed,
                },
            );
            let mut teammate_task = task("tteam1");
            teammate_task.teammate = true;
            fold_claude_background_tasks_into_roster(&mut roster, &[teammate_task], 200, None);
            assert_eq!(
                roster.contains_key("ateam-6d3cb5b5"),
                expect_survives,
                "authoritative={authoritative}, listed={listed}"
            );
        }
    }

    // I13: a duplicate task id in the same fold call is last-wins. Both
    // occurrences of "dup" are new (not yet in the roster) while the roster
    // is at the cap, so both attempts fail into `pendingRunningTasks`; the
    // second `pending_set` call overwrites the first IN PLACE (Map.set
    // semantics). None of the 32 pre-existing entries are listed and there
    // is no teammate-typed task, so the sweep reaps all of them, freeing
    // every slot; the pending retry then creates "dup" from whichever value
    // survived the overwrite.
    #[test]
    fn i13_duplicate_task_id_is_last_wins() {
        let mut roster = ClaudeSubagentRoster::new();
        for i in 0..AGENT_STATUS_MAX_SUBAGENTS {
            upsert_working_claude_subagent(
                &mut roster,
                &format!("old{i}"),
                fields(None, None),
                i as i64,
            );
        }
        let mut first = task("dup");
        first.agent_type = Some("first".to_string());
        let mut second = task("dup");
        second.agent_type = Some("second".to_string());
        fold_claude_background_tasks_into_roster(&mut roster, &[first, second], 999, None);
        assert_eq!(roster.len(), 1);
        assert_eq!(
            roster.get("dup").unwrap().agent_type.as_deref(),
            Some("second")
        );
    }

    // I13: the pending order decides the last free slot at the cap.
    #[test]
    fn i13_pending_order_decides_last_free_slot_at_the_cap() {
        let mut roster = ClaudeSubagentRoster::new();
        for i in 0..AGENT_STATUS_MAX_SUBAGENTS {
            upsert_working_claude_subagent(
                &mut roster,
                &format!("a{i}"),
                fields(None, None),
                i as i64,
            );
        }
        // Only "a0" is reaped (unlisted, not teammate-shaped); one new slot
        // opens. Two brand-new tasks compete for it - first-listed wins per
        // pending-Vec insertion order.
        let tasks: Vec<ClaudeBackgroundAgentTask> = (1..AGENT_STATUS_MAX_SUBAGENTS)
            .map(|i| task(&format!("a{i}")))
            .chain([task("first-new"), task("second-new")])
            .collect();
        fold_claude_background_tasks_into_roster(&mut roster, &tasks, 999, None);
        assert!(roster.contains_key("first-new"));
        assert!(!roster.contains_key("second-new"));
        assert_eq!(roster.len(), AGENT_STATUS_MAX_SUBAGENTS);
    }

    // Unexercised reader inputs: background_tasks as null/{}/42 (all NotArray).
    #[test]
    fn reader_background_tasks_null_object_number_are_all_absent() {
        for field in [
            HookBackgroundTasksField::NotArray,
            HookBackgroundTasksField::NotArray,
            HookBackgroundTasksField::NotArray,
        ] {
            assert!(
                !read_claude_background_agent_tasks(&HookPayload {
                    background_tasks: field
                })
                .present
            );
        }
    }

    // Unexercised reader input: an array element (non-object-like).
    #[test]
    fn reader_array_element_is_skipped() {
        let payload = hook_payload(vec![
            HookTaskElement::Other,
            subagent_obj(Some("a1"), "running"),
        ]);
        let result = read_claude_background_agent_tasks(&payload);
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].id, "a1");
    }

    // Unexercised reader input: a non-string id (modeled as `id: None`).
    #[test]
    fn reader_non_string_id_is_skipped() {
        let payload = hook_payload(vec![HookTaskElement::Object(HookTaskObject {
            kind: HookTaskKind::Subagent,
            id: None,
            agent_type: None,
            description: None,
            running: true,
        })]);
        assert_eq!(read_claude_background_agent_tasks(&payload).tasks.len(), 0);
    }

    // Unexercised reader input: a whitespace-only id.
    #[test]
    fn reader_whitespace_only_id_is_skipped() {
        let payload = hook_payload(vec![subagent_obj(Some("   "), "running")]);
        assert_eq!(read_claude_background_agent_tasks(&payload).tasks.len(), 0);
    }

    // Unexercised reader input: non-string agent_type/description (modeled
    // as `None`).
    #[test]
    fn reader_non_string_agent_type_and_description_become_none() {
        let payload = hook_payload(vec![HookTaskElement::Object(HookTaskObject {
            kind: HookTaskKind::Subagent,
            id: Some("a1".to_string()),
            agent_type: None,
            description: None,
            running: true,
        })]);
        let result = read_claude_background_agent_tasks(&payload);
        assert_eq!(result.tasks[0].agent_type, None);
        assert_eq!(result.tasks[0].description, None);
    }

    // Unexercised reader input: a subagent with a non-`running` status.
    #[test]
    fn reader_subagent_with_non_running_status_is_running_false() {
        let payload = hook_payload(vec![subagent_obj(Some("a1"), "idle")]);
        let result = read_claude_background_agent_tasks(&payload);
        assert_eq!(result.tasks.len(), 1);
        assert!(!result.tasks[0].running);
    }

    // has_working / to_snapshots on an absent roster.
    #[test]
    fn has_working_and_to_snapshots_on_absent_roster() {
        assert!(!claude_roster_has_working_subagent(None));
        assert_eq!(claude_roster_to_snapshots(None), None);
    }

    // Emitted snapshot's agent_type/description/started_at values and the
    // absence of `model`.
    #[test]
    fn snapshot_field_values_and_absence_of_model() {
        let mut roster = ClaudeSubagentRoster::new();
        upsert_working_claude_subagent(
            &mut roster,
            "a1",
            fields(Some("probe"), Some("desc")),
            12345,
        );
        let snapshots = claude_roster_to_snapshots(Some(&roster)).unwrap();
        assert_eq!(snapshots.len(), 1);
        let snap = &snapshots[0];
        assert_eq!(snap.id, "a1");
        assert_eq!(snap.agent_type.as_deref(), Some("probe"));
        assert_eq!(snap.description.as_deref(), Some("desc"));
        assert_eq!(snap.started_at, 12345);
        assert_eq!(snap.model, None);
        assert_eq!(snap.state, AgentSubagentState::Working);
    }

    // matches_name with an empty name.
    #[test]
    fn matches_name_with_empty_name() {
        assert!(claude_teammate_id_matches_name("a-6d3cb5b5", ""));
        assert!(!claude_teammate_id_matches_name("aprobe1-6d3cb5b5", ""));
    }

    // matches_name with an empty suffix.
    #[test]
    fn matches_name_with_empty_suffix() {
        assert!(claude_teammate_id_matches_name("aprobe1-", "probe1"));
    }
}
