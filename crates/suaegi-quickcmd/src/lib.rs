//! VERBATIM port of Orca's `src/shared/terminal-quick-commands.ts` (272 lines).
//!
//! Ported: `O:10-17` [`MAX_QUICK_COMMANDS`] and the five sibling caps, `O:18`
//! (`REMOVED_PRESET_IDS`, kept private), `O:20` (`DEFAULT_TERMINAL_QUICK_COMMANDS`,
//! kept private), `O:26-28` [`get_default_terminal_quick_commands`], `O:30-43`
//! (`normalizeTerminalQuickCommandScope`, private), `O:45-49`
//! [`get_terminal_quick_command_scope`], `O:51-57`
//! [`terminal_quick_command_matches_repo`], `O:59-63`
//! [`get_terminal_quick_command_action`], `O:65-69`
//! [`is_terminal_agent_quick_command`], `O:71-75`
//! [`supports_terminal_agent_quick_command`], `O:77-79`
//! [`get_terminal_quick_command_body`], `O:81-83`
//! [`is_terminal_quick_command_complete`], `O:85-163`
//! [`normalize_terminal_quick_commands`], `O:165-168` (`hasExactKeys`,
//! private), `O:170-232` [`parse_normalized_terminal_quick_commands`] (+
//! private `isNormalizedTerminalQuickCommand{,Scope}` helpers), `O:236-248`
//! [`apply_terminal_quick_command_mutation`], `O:250-252`
//! [`build_terminal_quick_command_input`], `O:254-272`
//! [`flatten_terminal_quick_command`].
//!
//! `tui-agent-config`'s surface used here (`O:74`) is one line —
//! `isTuiAgent(agent) && TUI_AGENT_CONFIG[agent].promptInjectionMode !==
//! 'stdin-after-start'` — carried in locally as [`TUI_AGENT_CONFIG`], a
//! small static table mirroring upstream (`suaegi-claude-roster` precedent
//! for a one-line external-module surface).
//!
//! # Traps (see the plan's §1 for full rationale; `J<N>` numbering matches
//! # `docs/superpowers/plans/2026-07-26-terminal-quick-commands.md`)
//!
//! - **J1/J2**: all six length caps (`O:42` repoId 200, `O:119` idBase 80,
//!   `O:122` idBase 76 on the collision retry, `O:129` label 80, `O:144`
//!   prompt 6000, `O:152` command 4000) are JS `.length` — **UTF-16 code
//!   units**, not bytes and not `chars().count()`. The oracle's fixtures are
//!   entirely ASCII, so all three measures agree there; the caps are
//!   unprovable by the oracle and rely entirely on the hand-written `j1_*`
//!   pins below. Slicing at a raw UTF-16 offset can land inside an astral
//!   character's surrogate pair; JS would then hold a **lone surrogate**,
//!   which a Rust `&str` cannot represent. Every cap therefore **snaps
//!   down** to the nearest whole-character boundary via [`utf16_slice_prefix`]
//!   (copied verbatim from `suaegi-forge::repo_icon`'s private helper of the
//!   same name — duplicated per-module by explicit charter), yielding
//!   79/199/5999/3999 units instead of a half-surrogate at 80/200/6000/4000.
//!   ⚠ **Second-order cross-runtime consequence** (this module's own, not
//!   shared with `repo-icon`): [`parse_normalized_terminal_quick_commands`]
//!   is an exact-equality protocol gate — it compares a client's payload
//!   against this host's own re-normalization of that payload and rejects on
//!   any mismatch. A JS client that normalized a label to exactly 80 UTF-16
//!   units by splitting a surrogate pair (producing a lone surrogate the JS
//!   string type happily holds) would send a payload that, when
//!   re-normalized by a Rust host, snaps down to 79 units. The two labels
//!   differ, so the Rust host's `parse_normalized_terminal_quick_commands`
//!   rejects the **entire list**, including every command untouched by the
//!   astral edit. This is a real interop failure mode between a JS client
//!   and a Rust host on this exact protocol boundary, not a theoretical one.
//! - **J3**: the collision retry (`O:122`) slices at
//!   `MAX_QUICK_COMMAND_ID_LENGTH - 4`. JS `slice(0, negative)` clamps to
//!   `max(0, len + end)` (total, never panics); Rust `usize` subtraction
//!   would panic/wrap if the cap were ever below 4, so
//!   `const _: () = assert!(MAX_QUICK_COMMAND_ID_LENGTH >= 4);` turns that
//!   into a compile-time guarantee instead. Separately, the `- 4` budget is
//!   simply **wrong** once the collision suffix reaches two digits: for
//!   suffix `100` the produced id is `76 + 1("-") + 3("100") = 80`... survives;
//!   but suffix `1000` (four digits) or an idBase that is itself 80 units
//!   colliding a hundred times produces `76 + 1 + len(suffix)` units, which
//!   exceeds `MAX_QUICK_COMMAND_ID_LENGTH` (80) once `len(suffix) >= 5`, and
//!   more subtly: the suffix counter itself is unbounded, so at
//!   `suffix = 10000` the id is `76 + 1 + 5 = 82` units — over the cap. This
//!   overflow is **ported faithfully, not corrected** (`j3_*` pins the
//!   suffix-100 case as the smallest suffix width the plan calls out; see
//!   that test's comment for the exact arithmetic).
//! - **J4**: the first-attempt slice width (`O:119`, 80 =
//!   `MAX_QUICK_COMMAND_ID_LENGTH`) and the collision-retry slice width
//!   (`O:122`, 76 = `MAX_QUICK_COMMAND_ID_LENGTH - 4`) are deliberately
//!   **different constants applied to the same `idBase`**. Unifying them
//!   would change both the collision key space and the produced id for any
//!   idBase of length 77-80 units. The oracle's only colliding id
//!   (`'status'`, 6 units) is far short of either width, so both widths are
//!   no-ops there — the oracle cannot distinguish them; `j4_*` pins an
//!   80-unit idBase collision where the two widths diverge.
//! - **J5**: `LINE_BREAK_RE = /\r\n|\r|\n/` (`O:254`) is a three-way
//!   alternation tried left-to-right at each position (`\r\n` before a lone
//!   `\r`). [`flatten_terminal_quick_command`] re-implements this with a
//!   hand-rolled scanner ([`split_line_breaks`]) rather than
//!   `str::split('\n')`, because every oracle fixture's lines survive a
//!   post-split `.trim()` (`O:268`) identically whether CRLF is split as one
//!   separator or as two (a bare `\r` gets trimmed away as leading/trailing
//!   whitespace on the adjacent line in every fixture) — the oracle cannot
//!   tell the two split strategies apart. `j5_*` pins a **lone `\r` with no
//!   trailing/adjacent newline** (`"a\rb"`), where a naive `\n`-only split
//!   leaves the string untouched (no `\n` present) while the faithful
//!   alternation splits on the lone `\r` and flattens to `"a; b"`.
//! - **J6**: `.trim()` and `.trimEnd()` sit on adjacent lines and are
//!   deliberately different: the label (`O:116`) is trimmed on **both**
//!   ends; the prompt (`O:142`) and the command (`O:148`) are trimmed on
//!   the **trailing** end only. No oracle fixture supplies a command or
//!   prompt with **leading** whitespace, so a single-`trim()`
//!   (both-ends) implementation would still pass every oracle case; `j6_*`
//!   pins `{command: "  git status  "}` → `"  git status"` (leading space
//!   preserved, trailing space cut).
//! - **J7**: the trim happens **before** the slice (`O:142`→`O:144`,
//!   `O:148`→`O:152`) and there is **no re-trim after** slicing. If the
//!   `MAX_QUICK_COMMAND_TERMINAL_TEXT_LENGTH`-th UTF-16 unit of a
//!   trailing-trimmed command happens to be a space (reintroduced by the
//!   cut, not present at the trimmed string's own end), that space survives
//!   into the stored value. This is the **opposite order** from
//!   `suaegi-gen-prompt/src/commit_message_generation.rs:120-124`'s
//!   `slice`-then-`trimEnd` — do not port this module from that one's
//!   memory. The oracle's over-limit command fixture is `'y'.repeat(4001)`,
//!   which has no whitespace anywhere, so it cannot pin this; `j7_*` builds
//!   a command whose 4000th UTF-16 unit is a space and asserts it survives.
//! - **J8**: every trim site here is ECMAScript whitespace semantics —
//!   [`suaegi_misc::js_ws::js_trim`] for the label's both-ends trim, and a
//!   local [`js_trim_end`] (defined here, trailing-only, built on
//!   [`suaegi_misc::js_ws::is_js_whitespace`]) for the prompt/command
//!   trailing trims — **never** `str::trim`/`str::trim_end`, which disagree
//!   with ECMAScript at U+FEFF (JS whitespace, Rust is not) and U+0085 (Rust
//!   `char::is_whitespace` is true, JS is not) — in **opposite** directions.
//!   The oracle uses only ASCII whitespace, so it cannot pin this; `j8_*`
//!   covers both codepoints on label/prompt/command/repoId.
//! - **J9**: `record.appendEnter !== false` (`O:153`) is a **strict**
//!   inequality against the boolean literal `false`, not a truthiness
//!   check. `0`, `"false"`, `null`, and an absent field are all *not*
//!   strictly-equal to `false`, so they all normalize to `appendEnter: true`;
//!   only the literal boolean `false` produces `false`. `j9_*` pins all
//!   four "truthy-looking but not `true`" inputs.
//! - **J10**: two separate count checks, different operator AND different
//!   effect. During normalization (`O:157`), `normalized.length >= 40`
//!   triggers a `break` — a **truncation** — and the check runs **after**
//!   the push for that iteration, so the 40th successfully-normalized
//!   command survives and the 41st onward is dropped. At the protocol
//!   boundary (`O:221`), `input.length > 40` triggers a `return null` — a
//!   **rejection** — so exactly 40 raw input elements are accepted and 41
//!   are rejected outright; that count is of **raw** input array elements
//!   (including `null`/malformed ones that would themselves be dropped by
//!   normalization), not of successfully-normalized commands.
//! - **J11**: `hasExactKeys` (`O:165-168`) is "own enumerable key count
//!   equals expected AND every expected key present" — expressible with
//!   neither a typed struct nor `#[serde(deny_unknown_fields)]`, because an
//!   extra key is fatal (`{type: 'global', repoId: 'x'}` is rejected as a
//!   global scope) **and** a key present with value `undefined` still
//!   counts toward the key total (`{...canonical, scope: undefined}` passes
//!   the count check, then fails the field-value check at `O:174`, both
//!   observably different from "key absent"). Modeled with [`JsValue`] /
//!   [`JsRecord`]: an untyped value tree whose object variant carries an
//!   explicit `Vec<(String, JsValue)>` of own keys, including keys whose
//!   value is [`JsValue::Undefined`] — not an `Option<T>` struct field,
//!   which cannot distinguish "absent" from "present, undefined".
//! - **J12**: the generated-id counter (`O:118`,
//!   `` `quick-command-${normalized.length + 1}` ``) is the count of
//!   commands **successfully emitted so far** (`normalized.len() + 1`), not
//!   the raw input index. A dropped malformed element earlier in the input
//!   does not consume a counter value; `j12_*` pins a leading dropped
//!   element followed by two id-less entries and asserts they get
//!   `quick-command-1`/`quick-command-2`, not `-2`/`-3`.
//! - **J13**: `idBase = rawId || ...` (`O:118`) uses JS `||`, which falls
//!   through on **any** falsy value, not just `undefined`/absent — so an id
//!   that is the empty string **after trimming** (whitespace-only, or
//!   already empty) also falls through to the generated form. A non-string
//!   `id` field becomes `''` at `O:98` (`typeof record.id === 'string' ?
//!   ... : ''`) and takes the same path. `j13_*` pins both a whitespace-only
//!   id and a non-string (numeric) id.
//! - **J14**: `if (agent === null) continue` at `O:134-136` (inside the
//!   `action === 'agent-prompt'` branch) is **unreachable** — `O:113-115`
//!   already does `if (action === 'agent-prompt' && agent === null)
//!   continue` earlier in the same iteration, so by the time control reaches
//!   `O:134` with `action === 'agent-prompt'`, `agent` cannot be `null`.
//!   Kept verbatim below with a `// J14: unreachable` comment rather than
//!   removed, matching `suaegi-project-runtime`'s E9 precedent for
//!   preserving dead-but-faithful branches.
//! - **J15**: `DEFAULT_TERMINAL_QUICK_COMMANDS` (`O:20`) is an **empty**
//!   array. The two abandoned preset ids survive only as
//!   `REMOVED_PRESET_IDS` (`O:18`), a drop-list matched against the
//!   **trimmed** raw id (`O:98`-`O:99`), so a space-padded id like
//!   `' default-pwd '` is also dropped. `get_default_terminal_quick_commands`
//!   builds a fresh `Vec` on every call (mirroring `.map(command => ({
//!   ...command }))`, `O:27`); since the source list is empty this is moot
//!   for aliasing but kept for shape-fidelity.
//! - **J16**: [`flatten_terminal_quick_command`] returns **the same object**
//!   (`O:261-262`) when the command has no line breaks — the oracle pins
//!   this with reference-identity (`toBe`, `test:339-347`), not structural
//!   equality. Reproduced here with `Cow::Borrowed` (no line break) vs
//!   `Cow::Owned` (flattened) rather than always returning an owned
//!   `TerminalCommandQuickCommand`/`String`, which would make the `j16_*`
//!   pin (`matches!(result, Cow::Borrowed(_))`) meaningless — an
//!   always-`Owned` "port" would still be structurally equal to the input
//!   and pass a naive equality-only test.
//! - **J17**: an entry survives (`O:109-111`) if **any one** of
//!   `label`/`command`/`prompt` is present as a string — the comment at
//!   `O:107-108` explains why (settings save on every edit; an incomplete
//!   draft row must not be deleted before the user finishes it). An
//!   `agent-prompt` action with no *supported* agent is dropped
//!   (`O:113-115`); any `action` value other than the literal
//!   `'agent-prompt'` defaults to `'terminal-command'` (`O:103-104`,
//!   `get_terminal_quick_command_action`/`O:59-63` mirrors the same
//!   default for already-typed values).

use std::borrow::Cow;
use std::collections::HashSet;

use suaegi_misc::js_ws::{is_js_whitespace, js_trim};

// ---------------------------------------------------------------------------
// Constants (`O:10-18`)
// ---------------------------------------------------------------------------

/// `O:10`.
pub const MAX_QUICK_COMMANDS: usize = 40;
/// `O:11`.
pub const MAX_QUICK_COMMAND_ID_LENGTH: usize = 80;
/// `O:12`.
pub const MAX_QUICK_COMMAND_LABEL_LENGTH: usize = 80;
/// `O:13`.
pub const MAX_QUICK_COMMAND_REPO_ID_LENGTH: usize = 200;
/// `O:14`.
pub const MAX_QUICK_COMMAND_TERMINAL_TEXT_LENGTH: usize = 4000;
/// `O:15-17`. Why (from source): agent prompt quick commands still launch
/// through startup commands for argv/flag agents, so this must stay within
/// Orca's Windows shell safety cap.
pub const MAX_QUICK_COMMAND_AGENT_PROMPT_LENGTH: usize = 6000;

/// J3: the collision-retry slice (`O:122`) is
/// `MAX_QUICK_COMMAND_ID_LENGTH - 4`; this turns a would-be panic/wraparound
/// into a compile error if the cap is ever lowered below 4.
const _: () = assert!(MAX_QUICK_COMMAND_ID_LENGTH >= 4);

/// `O:18`. Kept private — never part of the public surface upstream either.
const REMOVED_PRESET_IDS: [&str; 2] = ["default-pwd", "default-git-status"];

// ---------------------------------------------------------------------------
// `tui-agent-config` local mirror (`O:1`, `O:74`)
// ---------------------------------------------------------------------------

/// Mirrors upstream `AgentPromptInjectionMode` (`tui-agent-config.ts`). Only
/// the variant identity matters here — `supports_terminal_agent_quick_command`
/// (`O:71-75`) checks a single inequality against `StdinAfterStart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptInjectionMode {
    Argv,
    StdinAfterStart,
    FlagPrompt,
    FlagPromptInteractive,
    FlagInteractive,
    HermesQuery,
}

/// Local mirror of `TUI_AGENT_CONFIG`'s keys and `promptInjectionMode`
/// values (`tui-agent-config.ts`, all 34 entries as surveyed). This crate
/// only ever reads one bit per agent (is it known, and is its mode NOT
/// `stdin-after-start`), but the full table is carried in — rather than
/// collapsed to just the derived "supported" subset — so this stays an
/// honest, auditable mirror of the upstream source rather than a
/// re-derivation baked in at write time.
const TUI_AGENT_CONFIG: &[(&str, PromptInjectionMode)] = &[
    ("claude", PromptInjectionMode::Argv),
    ("claude-agent-teams", PromptInjectionMode::StdinAfterStart),
    ("openclaude", PromptInjectionMode::Argv),
    ("codex", PromptInjectionMode::Argv),
    ("autohand", PromptInjectionMode::StdinAfterStart),
    ("ante", PromptInjectionMode::StdinAfterStart),
    ("opencode", PromptInjectionMode::FlagPrompt),
    ("mimo-code", PromptInjectionMode::FlagPrompt),
    ("pi", PromptInjectionMode::Argv),
    ("omp", PromptInjectionMode::Argv),
    ("gemini", PromptInjectionMode::FlagPromptInteractive),
    ("antigravity", PromptInjectionMode::FlagPromptInteractive),
    ("aider", PromptInjectionMode::StdinAfterStart),
    ("goose", PromptInjectionMode::StdinAfterStart),
    ("amp", PromptInjectionMode::StdinAfterStart),
    ("kilo", PromptInjectionMode::StdinAfterStart),
    ("kiro", PromptInjectionMode::StdinAfterStart),
    ("crush", PromptInjectionMode::StdinAfterStart),
    ("aug", PromptInjectionMode::StdinAfterStart),
    ("cline", PromptInjectionMode::StdinAfterStart),
    ("codebuff", PromptInjectionMode::StdinAfterStart),
    ("command-code", PromptInjectionMode::Argv),
    ("continue", PromptInjectionMode::StdinAfterStart),
    ("cursor", PromptInjectionMode::Argv),
    ("droid", PromptInjectionMode::Argv),
    ("kimi", PromptInjectionMode::StdinAfterStart),
    ("mistral-vibe", PromptInjectionMode::StdinAfterStart),
    ("qwen-code", PromptInjectionMode::StdinAfterStart),
    ("rovo", PromptInjectionMode::StdinAfterStart),
    ("hermes", PromptInjectionMode::HermesQuery),
    ("openclaw", PromptInjectionMode::StdinAfterStart),
    ("copilot", PromptInjectionMode::FlagInteractive),
    ("grok", PromptInjectionMode::Argv),
    ("devin", PromptInjectionMode::StdinAfterStart),
];

// ---------------------------------------------------------------------------
// Untyped input model (J11)
// ---------------------------------------------------------------------------

/// A JS-like untyped value tree — mirrors TS `unknown` for every "parses
/// arbitrary input" function in this module. Object records carry an
/// explicit `Vec<(String, JsValue)>` of own keys rather than a `HashMap`, so
/// a key present with [`JsValue::Undefined`] is distinguishable from an
/// absent key (J11) and duplicate-key shadowing follows JS object-literal
/// last-write-wins if ever constructed that way.
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

    /// Convenience constructor for an object value from `(key, value)` pairs.
    pub fn object<I>(pairs: I) -> JsValue
    where
        I: IntoIterator<Item = (&'static str, JsValue)>,
    {
        JsValue::Object(JsRecord::from_pairs(pairs))
    }

    /// Convenience constructor for an array value.
    pub fn array<I>(items: I) -> JsValue
    where
        I: IntoIterator<Item = JsValue>,
    {
        JsValue::Array(items.into_iter().collect())
    }
}

/// An untyped object record: an ordered list of own `(key, value)` pairs.
/// See [`JsValue`] for why this isn't a `HashMap`.
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

    /// Builder-style single-key append (own-key semantics: a repeated key
    /// shadows, matching a JS object literal's last-write-wins).
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

    /// `Object.hasOwn(record, key)` (`O:167`).
    fn has_own(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    /// `Object.keys(record).length` (`O:166`) — counts every own key
    /// regardless of whether its value is [`JsValue::Undefined`] (J11).
    fn key_count(&self) -> usize {
        self.0.len()
    }
}

/// Shorthand for a value at `key` that is exactly a JS string, i.e.
/// `typeof record.key === 'string'`.
fn get_str<'a>(record: &'a JsRecord, key: &str) -> Option<&'a str> {
    match record.get(key) {
        Some(JsValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// `value === literal` for a JS string field, tolerating any non-string
/// input (numbers, booleans, `null`/absent all compare unequal, exactly like
/// JS strict equality against a string literal).
fn str_eq(value: Option<&JsValue>, literal: &str) -> bool {
    matches!(value, Some(JsValue::Str(s)) if s == literal)
}

// ---------------------------------------------------------------------------
// Copied UTF-16 helpers (J1/J2)
//
// Duplicated verbatim (documentation style matched) from
// `suaegi-forge/src/repo_icon.rs:180-200`'s private `utf16_len` /
// `utf16_slice_prefix` per this repo's explicit per-module duplication
// charter — this module has no other reason to depend on `suaegi-forge`.
// ---------------------------------------------------------------------------

/// UTF-16 code-unit length of `s` (JS `.length` semantics) — the measure
/// behind all six size caps in this module (J1). Unlike `repo_icon`'s
/// production use, this module's production code only ever unconditionally
/// slices (never measures-then-decides), so this helper is exercised only
/// from the `j1_*`/`j3_*` test pins that assert the exact resulting UTF-16
/// length of a capped value; kept `#[cfg(test)]` accordingly.
#[cfg(test)]
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Largest prefix of `s` whose UTF-16-code-unit count is `<= max_units`,
/// snapping down to a whole-character boundary rather than splitting an
/// astral character's surrogate pair (J2, mirroring e.g.
/// `terminal-quick-commands.ts:129`'s `label.slice(0, 80)`).
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

/// J8: local ECMAScript trailing-only trim, since the prompt (`O:142`) and
/// command (`O:148`) sites use `.trimEnd()`, not `.trim()` (that's the
/// label, J6) — built on the shared [`is_js_whitespace`] predicate, not
/// `str::trim_end` (which disagrees with ECMAScript at U+FEFF and U+0085,
/// see `suaegi_misc::js_ws`'s module docs).
fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(|ch: char| is_js_whitespace(ch))
}

// ---------------------------------------------------------------------------
// Domain types (`types.ts:2518-2547`)
// ---------------------------------------------------------------------------

/// `TerminalQuickCommandScope` (`types.ts:2518-2525`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalQuickCommandScope {
    Global,
    Repo { repo_id: String },
}

/// `TerminalQuickCommandAction` (`types.ts:2527`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalQuickCommandAction {
    TerminalCommand,
    AgentPrompt,
}

/// `TerminalCommandQuickCommand` (`types.ts:2535-2539`). `scope` is
/// `Option` because `TerminalQuickCommandBase.scope` is optional in the TS
/// source (`types.ts:2532`) — hand-built values (as in several oracle
/// fixtures) may omit it entirely, distinct from an explicitly-normalized
/// `Global`.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalCommandQuickCommand {
    pub id: String,
    pub label: String,
    pub scope: Option<TerminalQuickCommandScope>,
    pub command: String,
    pub append_enter: bool,
}

/// `TerminalAgentQuickCommand` (`types.ts:2541-2545`).
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalAgentQuickCommand {
    pub id: String,
    pub label: String,
    pub scope: Option<TerminalQuickCommandScope>,
    pub agent: String,
    pub prompt: String,
}

/// `TerminalQuickCommand` (`types.ts:2547`), the discriminated union.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalQuickCommand {
    Terminal(TerminalCommandQuickCommand),
    Agent(TerminalAgentQuickCommand),
}

impl TerminalQuickCommand {
    pub fn id(&self) -> &str {
        match self {
            TerminalQuickCommand::Terminal(c) => &c.id,
            TerminalQuickCommand::Agent(a) => &a.id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            TerminalQuickCommand::Terminal(c) => &c.label,
            TerminalQuickCommand::Agent(a) => &a.label,
        }
    }

    pub fn scope(&self) -> Option<&TerminalQuickCommandScope> {
        match self {
            TerminalQuickCommand::Terminal(c) => c.scope.as_ref(),
            TerminalQuickCommand::Agent(a) => a.scope.as_ref(),
        }
    }
}

/// `TerminalQuickCommandMutation` (`O:22-24`).
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalQuickCommandMutation {
    Upsert { command: TerminalQuickCommand },
    Delete { id: String },
}

// ---------------------------------------------------------------------------
// Defaults (`O:20`, `O:26-28`; J15)
// ---------------------------------------------------------------------------

/// `DEFAULT_TERMINAL_QUICK_COMMANDS` (`O:20`) — empty. J15: the two
/// abandoned preset ids survive only in [`REMOVED_PRESET_IDS`] as a
/// drop-list, not as entries here.
const DEFAULT_TERMINAL_QUICK_COMMANDS: &[TerminalQuickCommand] = &[];

/// `getDefaultTerminalQuickCommands` (`O:26-28`). Builds a fresh `Vec` on
/// every call, mirroring `.map(command => ({ ...command }))`'s shallow-copy
/// intent — moot while the source list is empty, kept for shape-fidelity.
pub fn get_default_terminal_quick_commands() -> Vec<TerminalQuickCommand> {
    DEFAULT_TERMINAL_QUICK_COMMANDS.to_vec()
}

// ---------------------------------------------------------------------------
// Scope normalization (`O:30-49`)
// ---------------------------------------------------------------------------

/// `normalizeTerminalQuickCommandScope` (`O:30-43`), operating on an untyped
/// [`JsValue`] (the `unknown` input side).
fn normalize_terminal_quick_command_scope(input: &JsValue) -> TerminalQuickCommandScope {
    let record = match input {
        JsValue::Object(record) => record,
        _ => return TerminalQuickCommandScope::Global,
    };
    if !str_eq(record.get("type"), "repo") {
        return TerminalQuickCommandScope::Global;
    }
    let repo_id = match get_str(record, "repoId") {
        Some(s) => js_trim(s),
        None => "",
    };
    if repo_id.is_empty() {
        return TerminalQuickCommandScope::Global;
    }
    TerminalQuickCommandScope::Repo {
        repo_id: utf16_slice_prefix(repo_id, MAX_QUICK_COMMAND_REPO_ID_LENGTH).to_string(),
    }
}

/// Adapter for [`get_terminal_quick_command_scope`]: re-runs the same
/// validation/re-cap logic as [`normalize_terminal_quick_command_scope`] but
/// against an already-typed `Option<&TerminalQuickCommandScope>` (what a
/// concrete [`TerminalQuickCommand`]'s `scope` field holds), rather than a
/// raw [`JsValue`] — the TS source passes `command.scope` (typed
/// `TerminalQuickCommandScope | undefined`) through the very same `unknown`
/// validator (`O:48`); this is the typed-Rust equivalent of that call.
fn normalize_typed_scope(scope: Option<&TerminalQuickCommandScope>) -> TerminalQuickCommandScope {
    match scope {
        None => TerminalQuickCommandScope::Global,
        Some(TerminalQuickCommandScope::Global) => TerminalQuickCommandScope::Global,
        Some(TerminalQuickCommandScope::Repo { repo_id }) => {
            let trimmed = js_trim(repo_id);
            if trimmed.is_empty() {
                TerminalQuickCommandScope::Global
            } else {
                TerminalQuickCommandScope::Repo {
                    repo_id: utf16_slice_prefix(trimmed, MAX_QUICK_COMMAND_REPO_ID_LENGTH)
                        .to_string(),
                }
            }
        }
    }
}

/// `getTerminalQuickCommandScope` (`O:45-49`).
pub fn get_terminal_quick_command_scope(
    command: &TerminalQuickCommand,
) -> TerminalQuickCommandScope {
    normalize_typed_scope(command.scope())
}

/// `terminalQuickCommandMatchesRepo` (`O:51-57`).
pub fn terminal_quick_command_matches_repo(
    command: &TerminalQuickCommand,
    repo_id: Option<&str>,
) -> bool {
    match get_terminal_quick_command_scope(command) {
        TerminalQuickCommandScope::Global => true,
        TerminalQuickCommandScope::Repo {
            repo_id: scope_repo_id,
        } => repo_id.is_some_and(|r| r == scope_repo_id),
    }
}

// ---------------------------------------------------------------------------
// Action / body / completeness (`O:59-83`)
// ---------------------------------------------------------------------------

/// `getTerminalQuickCommandAction` (`O:59-63`).
pub fn get_terminal_quick_command_action(
    command: &TerminalQuickCommand,
) -> TerminalQuickCommandAction {
    match command {
        TerminalQuickCommand::Agent(_) => TerminalQuickCommandAction::AgentPrompt,
        TerminalQuickCommand::Terminal(_) => TerminalQuickCommandAction::TerminalCommand,
    }
}

/// `isTerminalAgentQuickCommand` (`O:65-69`).
pub fn is_terminal_agent_quick_command(command: &TerminalQuickCommand) -> bool {
    matches!(
        get_terminal_quick_command_action(command),
        TerminalQuickCommandAction::AgentPrompt
    )
}

/// `supportsTerminalAgentQuickCommand` (`O:71-75`): `isTuiAgent(agent) &&
/// TUI_AGENT_CONFIG[agent].promptInjectionMode !== 'stdin-after-start'`,
/// against the local [`TUI_AGENT_CONFIG`] mirror.
pub fn supports_terminal_agent_quick_command(agent: &JsValue) -> bool {
    match agent {
        JsValue::Str(s) => TUI_AGENT_CONFIG
            .iter()
            .any(|(name, mode)| name == s && *mode != PromptInjectionMode::StdinAfterStart),
        _ => false,
    }
}

/// `getTerminalQuickCommandBody` (`O:77-79`).
pub fn get_terminal_quick_command_body(command: &TerminalQuickCommand) -> &str {
    match command {
        TerminalQuickCommand::Agent(a) => &a.prompt,
        TerminalQuickCommand::Terminal(c) => &c.command,
    }
}

/// `isTerminalQuickCommandComplete` (`O:81-83`). Uses the both-ends
/// [`js_trim`], matching the source's `.trim()` (not the trailing-only
/// [`js_trim_end`] used during normalization).
pub fn is_terminal_quick_command_complete(command: &TerminalQuickCommand) -> bool {
    !js_trim(command.label()).is_empty()
        && !js_trim(get_terminal_quick_command_body(command)).is_empty()
}

// ---------------------------------------------------------------------------
// Normalization (`O:85-163`)
// ---------------------------------------------------------------------------

/// `O:119-125`: assign a unique id derived from `idBase`, threading a
/// per-call `seen_ids` set exactly like the source's
/// `while (seenIds.has(id))` retry loop. J1/J2: the first attempt slices at
/// [`MAX_QUICK_COMMAND_ID_LENGTH`] (80) units; J4: every collision retry
/// re-slices the SAME `idBase` at `MAX_QUICK_COMMAND_ID_LENGTH - 4` (76)
/// units instead — a deliberately different, narrower width, not unified
/// with the first attempt's. J3: the `- 4` budget assumes the numeric
/// suffix stays short and is never re-validated as it grows, so a suffix
/// reaching 4+ digits (`>= 1000`) yields an id 81+ units long, over the cap
/// — ported faithfully, not corrected. Extracted into its own function
/// (rather than inlined in [`normalize_terminal_quick_commands`]) so the
/// `j3_*` overflow pin can drive many collisions directly, independent of
/// the unrelated [`MAX_QUICK_COMMANDS`] (40) truncation (`O:157`) that
/// would otherwise cap how many collisions a single
/// `normalize_terminal_quick_commands` call can ever produce.
fn assign_unique_quick_command_id(id_base: &str, seen_ids: &mut HashSet<String>) -> String {
    let mut id = utf16_slice_prefix(id_base, MAX_QUICK_COMMAND_ID_LENGTH).to_string();
    let mut suffix: u64 = 2;
    while seen_ids.contains(&id) {
        let base76 = utf16_slice_prefix(id_base, MAX_QUICK_COMMAND_ID_LENGTH - 4);
        id = format!("{base76}-{suffix}");
        suffix += 1;
    }
    seen_ids.insert(id.clone());
    id
}

/// `normalizeTerminalQuickCommands` (`O:85-163`).
pub fn normalize_terminal_quick_commands(input: &JsValue) -> Vec<TerminalQuickCommand> {
    let items = match input {
        JsValue::Array(items) => items,
        _ => return get_default_terminal_quick_commands(),
    };

    let mut normalized: Vec<TerminalQuickCommand> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    for item in items {
        // `!item || typeof item !== 'object' || Array.isArray(item)` (`O:94`):
        // collapses to "not a plain object" once falsy/array/null are all
        // excluded by the `JsValue::Object` match arm.
        let record = match item {
            JsValue::Object(record) => record,
            _ => continue,
        };

        // `O:98`: non-string `id` becomes `''` (J13).
        let raw_id = match get_str(record, "id") {
            Some(s) => js_trim(s).to_string(),
            None => String::new(),
        };
        if REMOVED_PRESET_IDS.contains(&raw_id.as_str()) {
            // J15: matched against the trimmed id.
            continue;
        }

        let has_label = get_str(record, "label").is_some();
        let action = if str_eq(record.get("action"), "agent-prompt") {
            TerminalQuickCommandAction::AgentPrompt
        } else {
            TerminalQuickCommandAction::TerminalCommand
        };
        let has_command = get_str(record, "command").is_some();
        let has_prompt = get_str(record, "prompt").is_some();

        // J17: survives if ANY of label/command/prompt is present as a string.
        if !has_label && !has_command && !has_prompt {
            continue;
        }

        let agent_value = record.get("agent").cloned().unwrap_or(JsValue::Undefined);
        let agent: Option<String> = if supports_terminal_agent_quick_command(&agent_value) {
            match agent_value {
                JsValue::Str(s) => Some(s),
                _ => None,
            }
        } else {
            None
        };

        if action == TerminalQuickCommandAction::AgentPrompt && agent.is_none() {
            continue;
        }

        // J6/J7: trim (both ends) BEFORE slicing; no re-trim after.
        let label = if has_label {
            js_trim(get_str(record, "label").unwrap()).to_string()
        } else {
            String::new()
        };

        // `O:118`: J13 (`||` falls through on empty-after-trim or non-string
        // ids too) and J12 (counter is emitted count so far, not input index).
        let id_base = if !raw_id.is_empty() {
            raw_id.clone()
        } else {
            format!("quick-command-{}", normalized.len() + 1)
        };

        // J1/J2/J3/J4: see `assign_unique_quick_command_id`.
        let id = assign_unique_quick_command_id(&id_base, &mut seen_ids);

        let scope = normalize_terminal_quick_command_scope(
            record.get("scope").unwrap_or(&JsValue::Undefined),
        );
        let label = utf16_slice_prefix(&label, MAX_QUICK_COMMAND_LABEL_LENGTH).to_string();

        match action {
            TerminalQuickCommandAction::AgentPrompt => {
                // J14: unreachable — `action == AgentPrompt && agent.is_none()`
                // already `continue`d above in this same iteration. Kept
                // verbatim rather than removed/unwrapped.
                let agent = match agent {
                    Some(a) => a,
                    None => continue,
                };
                let prompt_trimmed = if has_prompt {
                    js_trim_end(get_str(record, "prompt").unwrap())
                } else {
                    ""
                };
                let prompt =
                    utf16_slice_prefix(prompt_trimmed, MAX_QUICK_COMMAND_AGENT_PROMPT_LENGTH)
                        .to_string();
                normalized.push(TerminalQuickCommand::Agent(TerminalAgentQuickCommand {
                    id,
                    label,
                    scope: Some(scope),
                    agent,
                    prompt,
                }));
            }
            TerminalQuickCommandAction::TerminalCommand => {
                let command_trimmed = if has_command {
                    js_trim_end(get_str(record, "command").unwrap())
                } else {
                    ""
                };
                let command =
                    utf16_slice_prefix(command_trimmed, MAX_QUICK_COMMAND_TERMINAL_TEXT_LENGTH)
                        .to_string();
                // J9: strict `!== false`.
                let append_enter = !matches!(record.get("appendEnter"), Some(JsValue::Bool(false)));
                normalized.push(TerminalQuickCommand::Terminal(
                    TerminalCommandQuickCommand {
                        id,
                        label,
                        scope: Some(scope),
                        command,
                        append_enter,
                    },
                ));
            }
        }

        // J10: `>=` and runs AFTER the push, so this element (the 40th
        // successfully-normalized one) survives; the 41st is never reached.
        if normalized.len() >= MAX_QUICK_COMMANDS {
            break;
        }
    }

    normalized
}

// ---------------------------------------------------------------------------
// Protocol-boundary exact-shape check (`O:165-232`; J11)
// ---------------------------------------------------------------------------

/// `hasExactKeys` (`O:165-168`).
fn has_exact_keys(record: &JsRecord, keys: &[&str]) -> bool {
    record.key_count() == keys.len() && keys.iter().all(|key| record.has_own(key))
}

/// `isNormalizedTerminalQuickCommandScope` (`O:170-186`).
fn is_normalized_terminal_quick_command_scope(
    value: &JsValue,
    expected: &TerminalQuickCommandScope,
) -> bool {
    let record = match value {
        JsValue::Object(record) => record,
        _ => return false,
    };
    match expected {
        TerminalQuickCommandScope::Global => {
            has_exact_keys(record, &["type"]) && str_eq(record.get("type"), "global")
        }
        TerminalQuickCommandScope::Repo { repo_id } => {
            has_exact_keys(record, &["type", "repoId"])
                && str_eq(record.get("type"), "repo")
                && str_eq(record.get("repoId"), repo_id)
        }
    }
}

/// `isNormalizedTerminalQuickCommand` (`O:188-214`).
fn is_normalized_terminal_quick_command(value: &JsValue, expected: &TerminalQuickCommand) -> bool {
    let record = match value {
        JsValue::Object(record) => record,
        _ => return false,
    };
    let expected_scope = get_terminal_quick_command_scope(expected);
    if !str_eq(record.get("id"), expected.id())
        || !str_eq(record.get("label"), expected.label())
        || !is_normalized_terminal_quick_command_scope(
            record.get("scope").unwrap_or(&JsValue::Undefined),
            &expected_scope,
        )
    {
        return false;
    }
    match expected {
        TerminalQuickCommand::Agent(a) => {
            has_exact_keys(
                record,
                &["id", "label", "action", "agent", "prompt", "scope"],
            ) && str_eq(record.get("action"), "agent-prompt")
                && str_eq(record.get("agent"), &a.agent)
                && str_eq(record.get("prompt"), &a.prompt)
        }
        TerminalQuickCommand::Terminal(c) => {
            has_exact_keys(
                record,
                &["id", "label", "action", "command", "appendEnter", "scope"],
            ) && str_eq(record.get("action"), "terminal-command")
                && str_eq(record.get("command"), &c.command)
                && matches!(record.get("appendEnter"), Some(JsValue::Bool(b)) if *b == c.append_enter)
        }
    }
}

/// `parseNormalizedTerminalQuickCommands` (`O:218-232`). J10: `input.len() >
/// MAX_QUICK_COMMANDS` rejects on the RAW array length (including malformed
/// elements), distinct from normalization's truncate-at-`>=`-40 behavior.
///
/// Why (from source, `O:216-217`): a full-list client must reject any
/// "authoritative" payload that would change under normalization, or its
/// next mutation could persist silent loss.
pub fn parse_normalized_terminal_quick_commands(
    input: &JsValue,
) -> Option<Vec<TerminalQuickCommand>> {
    let items = match input {
        JsValue::Array(items) if items.len() <= MAX_QUICK_COMMANDS => items,
        _ => return None,
    };
    let normalized = normalize_terminal_quick_commands(input);
    if normalized.len() != items.len() {
        return None;
    }
    for (item, command) in items.iter().zip(normalized.iter()) {
        if !is_normalized_terminal_quick_command(item, command) {
            return None;
        }
    }
    Some(normalized)
}

// ---------------------------------------------------------------------------
// Mutation application (`O:236-248`)
// ---------------------------------------------------------------------------

/// `applyTerminalQuickCommandMutation` (`O:236-248`).
///
/// Why (from source, `O:234-235`): paired clients can edit settings
/// concurrently. Applying one command at the host boundary preserves
/// unrelated commands added by another client.
pub fn apply_terminal_quick_command_mutation(
    commands: &[TerminalQuickCommand],
    mutation: TerminalQuickCommandMutation,
) -> Vec<TerminalQuickCommand> {
    match mutation {
        TerminalQuickCommandMutation::Delete { id } => {
            commands.iter().filter(|c| c.id() != id).cloned().collect()
        }
        TerminalQuickCommandMutation::Upsert { command } => {
            match commands.iter().position(|c| c.id() == command.id()) {
                None => {
                    let mut next = commands.to_vec();
                    next.push(command);
                    next
                }
                Some(index) => {
                    let mut next = commands.to_vec();
                    next[index] = command;
                    next
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal input building (`O:250-252`)
// ---------------------------------------------------------------------------

/// `buildTerminalQuickCommandInput` (`O:250-252`).
pub fn build_terminal_quick_command_input(command: &TerminalCommandQuickCommand) -> String {
    if command.append_enter {
        format!("{}\r", command.command)
    } else {
        command.command.clone()
    }
}

// ---------------------------------------------------------------------------
// Line flattening (`O:254-272`; J5/J16)
// ---------------------------------------------------------------------------

/// Whether `s` contains any of `\r\n`, `\r`, or `\n` — the `.test()` use of
/// `LINE_BREAK_RE` (`O:261`). Presence-only, so alternation order is
/// immaterial here (unlike [`split_line_breaks`]).
fn contains_line_break(s: &str) -> bool {
    s.contains('\r') || s.contains('\n')
}

/// `LINE_BREAK_RE = /\r\n|\r|\n/` (`O:254`) used as a *splitter*
/// (`command.command.split(LINE_BREAK_RE)`, `O:267`). JS regex alternation
/// is tried left-to-right at each position, so a `\r` immediately followed
/// by `\n` consumes both as one separator; a lone `\r` (no following `\n`)
/// or a lone `\n` each consume just themselves (J5). All three separator
/// bytes are single-byte ASCII, so every slice boundary produced here is a
/// valid `char` boundary regardless of what multi-byte UTF-8 content
/// surrounds it.
fn split_line_breaks(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                result.push(&s[start..i]);
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            b'\n' => {
                result.push(&s[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    result.push(&s[start..]);
    result
}

/// `flattenTerminalQuickCommand` (`O:258-272`).
///
/// Why (from source, `O:256-257`): quick-command lines are independent
/// shell commands; one shell command list prevents foreground programs from
/// reading later lines as stdin.
///
/// J16: returns `Cow::Borrowed` (the same command) when there is no line
/// break, matching the source's `return command` reference-identity
/// (`O:261-262`, pinned by the oracle's `toBe` at `test:339-347`).
pub fn flatten_terminal_quick_command(
    command: &TerminalCommandQuickCommand,
) -> Cow<'_, TerminalCommandQuickCommand> {
    if !contains_line_break(&command.command) {
        return Cow::Borrowed(command);
    }
    let mut flattened = command.clone();
    flattened.command = split_line_breaks(&command.command)
        .into_iter()
        .map(js_trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    Cow::Owned(flattened)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_global() -> Option<TerminalQuickCommandScope> {
        Some(TerminalQuickCommandScope::Global)
    }

    fn terminal(
        id: &str,
        label: &str,
        command: &str,
        append_enter: bool,
        scope: Option<TerminalQuickCommandScope>,
    ) -> TerminalQuickCommand {
        TerminalQuickCommand::Terminal(TerminalCommandQuickCommand {
            id: id.to_string(),
            label: label.to_string(),
            scope,
            command: command.to_string(),
            append_enter,
        })
    }

    fn agent(
        id: &str,
        label: &str,
        agent: &str,
        prompt: &str,
        scope: Option<TerminalQuickCommandScope>,
    ) -> TerminalQuickCommand {
        TerminalQuickCommand::Agent(TerminalAgentQuickCommand {
            id: id.to_string(),
            label: label.to_string(),
            scope,
            agent: agent.to_string(),
            prompt: prompt.to_string(),
        })
    }

    // -----------------------------------------------------------------
    // Oracle: `test:17-336` (18 cases)
    // -----------------------------------------------------------------

    #[test]
    fn oracle_returns_safe_defaults_when_persisted_settings_are_missing() {
        assert_eq!(
            normalize_terminal_quick_commands(&JsValue::Undefined),
            vec![]
        );
        assert_eq!(get_default_terminal_quick_commands(), vec![]);
    }

    #[test]
    fn oracle_keeps_an_intentionally_empty_command_list() {
        assert_eq!(
            normalize_terminal_quick_commands(&JsValue::array([])),
            vec![]
        );
    }

    #[test]
    fn oracle_removes_quick_commands_from_the_abandoned_preset_rollout() {
        let input = JsValue::array([
            JsValue::object([
                ("id", JsValue::str("default-pwd")),
                ("label", JsValue::str("Print Working Directory")),
                ("command", JsValue::str("pwd")),
                ("appendEnter", JsValue::Bool(true)),
            ]),
            JsValue::object([
                ("id", JsValue::str("default-git-status")),
                ("label", JsValue::str("Git Status")),
                ("command", JsValue::str("git status")),
                ("appendEnter", JsValue::Bool(true)),
            ]),
        ]);
        assert_eq!(normalize_terminal_quick_commands(&input), vec![]);
    }

    #[test]
    fn oracle_drops_malformed_entries_and_normalizes_valid_commands_and_drafts() {
        let input = JsValue::array([
            JsValue::Null,
            JsValue::object([
                ("id", JsValue::str("status")),
                ("label", JsValue::str("  Status  ")),
                ("command", JsValue::str("git status\n")),
                ("appendEnter", JsValue::Bool(false)),
            ]),
            JsValue::object([
                ("id", JsValue::str("empty-command")),
                ("label", JsValue::str("Empty")),
                ("command", JsValue::str("   ")),
            ]),
            JsValue::object([
                ("id", JsValue::str("status")),
                ("label", JsValue::str("Duplicate")),
                ("command", JsValue::str("pwd")),
            ]),
            JsValue::object([
                ("label", JsValue::str("No ID")),
                ("command", JsValue::str("date")),
            ]),
        ]);
        let expected = vec![
            terminal("status", "Status", "git status", false, scope_global()),
            terminal("empty-command", "Empty", "", true, scope_global()),
            terminal("status-2", "Duplicate", "pwd", true, scope_global()),
            terminal("quick-command-4", "No ID", "date", true, scope_global()),
        ];
        assert_eq!(normalize_terminal_quick_commands(&input), expected);
    }

    #[test]
    fn oracle_normalizes_repository_scoped_commands_and_falls_back_to_global_for_invalid_scopes() {
        let input = JsValue::array([
            JsValue::object([
                ("id", JsValue::str("repo-dev")),
                ("label", JsValue::str("Dev")),
                ("command", JsValue::str("pnpm dev")),
                (
                    "scope",
                    JsValue::object([
                        ("type", JsValue::str("repo")),
                        ("repoId", JsValue::str(" repo-1 ")),
                    ]),
                ),
            ]),
            JsValue::object([
                ("id", JsValue::str("bad-repo")),
                ("label", JsValue::str("Bad")),
                ("command", JsValue::str("echo bad")),
                (
                    "scope",
                    JsValue::object([
                        ("type", JsValue::str("repo")),
                        ("repoId", JsValue::str("   ")),
                    ]),
                ),
            ]),
        ]);
        let expected = vec![
            terminal(
                "repo-dev",
                "Dev",
                "pnpm dev",
                true,
                Some(TerminalQuickCommandScope::Repo {
                    repo_id: "repo-1".to_string(),
                }),
            ),
            terminal("bad-repo", "Bad", "echo bad", true, scope_global()),
        ];
        assert_eq!(normalize_terminal_quick_commands(&input), expected);
    }

    #[test]
    fn oracle_normalizes_agent_prompt_commands_without_storing_generated_shell_text() {
        let input = JsValue::array([
            JsValue::object([
                ("id", JsValue::str("agent-review")),
                ("label", JsValue::str("Review")),
                ("action", JsValue::str("agent-prompt")),
                ("agent", JsValue::str("codex")),
                ("prompt", JsValue::str("  Review this diff\n")),
                ("command", JsValue::str("codex 'old workaround'")),
            ]),
            JsValue::object([
                ("id", JsValue::str("unknown-agent")),
                ("label", JsValue::str("Unknown")),
                ("action", JsValue::str("agent-prompt")),
                ("agent", JsValue::str("not-real")),
                ("prompt", JsValue::str("Do work")),
            ]),
            JsValue::object([
                ("id", JsValue::str("post-start-agent")),
                ("label", JsValue::str("Aider")),
                ("action", JsValue::str("agent-prompt")),
                ("agent", JsValue::str("aider")),
                ("prompt", JsValue::str("Do work")),
            ]),
        ]);
        let expected = vec![agent(
            "agent-review",
            "Review",
            "codex",
            "  Review this diff",
            scope_global(),
        )];
        assert_eq!(normalize_terminal_quick_commands(&input), expected);
    }

    #[test]
    fn oracle_keeps_larger_reusable_agent_prompts_while_bounding_shell_commands_separately() {
        let large_prompt = "Review this diff.\n".repeat(320);
        let over_limit_prompt = "x".repeat(6001);
        let over_limit_command = "y".repeat(4001);

        let input = JsValue::array([
            JsValue::object([
                ("id", JsValue::str("large-review")),
                ("label", JsValue::str("Review")),
                ("action", JsValue::str("agent-prompt")),
                ("agent", JsValue::str("codex")),
                ("prompt", JsValue::str(large_prompt.clone())),
            ]),
            JsValue::object([
                ("id", JsValue::str("over-limit-review")),
                ("label", JsValue::str("Review with cap")),
                ("action", JsValue::str("agent-prompt")),
                ("agent", JsValue::str("codex")),
                ("prompt", JsValue::str(over_limit_prompt.clone())),
            ]),
            JsValue::object([
                ("id", JsValue::str("over-limit-command")),
                ("label", JsValue::str("Run long command")),
                ("command", JsValue::str(over_limit_command.clone())),
            ]),
        ]);
        let expected = vec![
            agent(
                "large-review",
                "Review",
                "codex",
                js_trim_end(&large_prompt),
                scope_global(),
            ),
            agent(
                "over-limit-review",
                "Review with cap",
                "codex",
                &"x".repeat(6000),
                scope_global(),
            ),
            terminal(
                "over-limit-command",
                "Run long command",
                &"y".repeat(4000),
                true,
                scope_global(),
            ),
        ];
        assert_eq!(normalize_terminal_quick_commands(&input), expected);
    }

    #[test]
    fn oracle_accepts_only_complete_canonical_command_lists_at_protocol_boundaries() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("status")),
            ("label", JsValue::str("Status")),
            ("command", JsValue::str("git status")),
            ("appendEnter", JsValue::Bool(true)),
        ])]);
        let canonical = normalize_terminal_quick_commands(&input);
        let canonical_record = JsValue::object([
            ("id", JsValue::str("status")),
            ("label", JsValue::str("Status")),
            ("action", JsValue::str("terminal-command")),
            ("command", JsValue::str("git status")),
            ("appendEnter", JsValue::Bool(true)),
            ("scope", JsValue::object([("type", JsValue::str("global"))])),
        ]);
        let canonical_js = JsValue::array([canonical_record.clone()]);

        assert_eq!(
            parse_normalized_terminal_quick_commands(&canonical_js),
            Some(canonical.clone())
        );

        let mutated = JsValue::array([JsValue::object([
            ("id", JsValue::str("status")),
            ("label", JsValue::str("Status")),
            ("action", JsValue::str("terminal-command")),
            ("command", JsValue::Number(42.0)),
            ("appendEnter", JsValue::Bool(true)),
            ("scope", JsValue::object([("type", JsValue::str("global"))])),
        ])]);
        assert_eq!(parse_normalized_terminal_quick_commands(&mutated), None);

        // `[...canonical, ...canonical.slice(0, 1)]`: the same record twice.
        // Counts match post-normalization (2 in, 2 out — the collision retry
        // makes the second entry's id `status-2`), but the second entry's
        // RAW id ('status') no longer matches its normalized id ('status-2').
        let doubled = JsValue::array([canonical_record.clone(), canonical_record]);
        assert_eq!(parse_normalized_terminal_quick_commands(&doubled), None);
    }

    #[test]
    fn oracle_applies_targeted_mutations_without_replacing_unrelated_commands() {
        let input = JsValue::array([
            JsValue::object([
                ("id", JsValue::str("first")),
                ("label", JsValue::str("First")),
                ("command", JsValue::str("echo first")),
                ("appendEnter", JsValue::Bool(true)),
            ]),
            JsValue::object([
                ("id", JsValue::str("second")),
                ("label", JsValue::str("Second")),
                ("command", JsValue::str("echo second")),
                ("appendEnter", JsValue::Bool(true)),
            ]),
        ]);
        let normalized = normalize_terminal_quick_commands(&input);
        let first = normalized[0].clone();
        let second = normalized[1].clone();
        let mut edited_command = match first.clone() {
            TerminalQuickCommand::Terminal(c) => c,
            _ => unreachable!(),
        };
        edited_command.label = "Edited".to_string();
        let edited = TerminalQuickCommand::Terminal(edited_command);

        assert_eq!(
            apply_terminal_quick_command_mutation(
                &[first.clone(), second.clone()],
                TerminalQuickCommandMutation::Upsert {
                    command: edited.clone()
                }
            ),
            vec![edited.clone(), second.clone()]
        );
        assert_eq!(
            apply_terminal_quick_command_mutation(
                &[first.clone(), second.clone()],
                TerminalQuickCommandMutation::Delete {
                    id: first.id().to_string()
                }
            ),
            vec![second]
        );
    }

    #[test]
    fn oracle_matches_global_commands_everywhere_and_repo_commands_only_in_their_repo() {
        let global = terminal("global", "Global", "date", true, scope_global());
        assert!(terminal_quick_command_matches_repo(&global, None));

        let repo = terminal(
            "repo",
            "Repo",
            "pnpm dev",
            true,
            Some(TerminalQuickCommandScope::Repo {
                repo_id: "repo-1".to_string(),
            }),
        );
        assert!(terminal_quick_command_matches_repo(&repo, Some("repo-1")));
        assert!(!terminal_quick_command_matches_repo(&repo, Some("repo-2")));
    }

    #[test]
    fn oracle_formats_terminal_input_without_assuming_shell_semantics() {
        let with_enter = TerminalCommandQuickCommand {
            id: "status".to_string(),
            label: "Status".to_string(),
            scope: None,
            command: "git status".to_string(),
            append_enter: true,
        };
        assert_eq!(
            build_terminal_quick_command_input(&with_enter),
            "git status\r"
        );

        let without_enter = TerminalCommandQuickCommand {
            append_enter: false,
            ..with_enter
        };
        assert_eq!(
            build_terminal_quick_command_input(&without_enter),
            "git status"
        );
    }

    #[test]
    fn oracle_classifies_quick_command_actions_and_body_text() {
        let terminal = TerminalQuickCommand::Terminal(TerminalCommandQuickCommand {
            id: "status".to_string(),
            label: "Status".to_string(),
            scope: None,
            command: "git status".to_string(),
            append_enter: true,
        });
        let agent = TerminalQuickCommand::Agent(TerminalAgentQuickCommand {
            id: "agent".to_string(),
            label: "Agent".to_string(),
            scope: None,
            agent: "claude".to_string(),
            prompt: "Fix the tests".to_string(),
        });

        assert_eq!(
            get_terminal_quick_command_action(&terminal),
            TerminalQuickCommandAction::TerminalCommand
        );
        assert_eq!(get_terminal_quick_command_body(&terminal), "git status");
        assert!(is_terminal_quick_command_complete(&terminal));
        assert_eq!(
            get_terminal_quick_command_action(&agent),
            TerminalQuickCommandAction::AgentPrompt
        );
        assert_eq!(get_terminal_quick_command_body(&agent), "Fix the tests");
        assert!(is_terminal_quick_command_complete(&agent));
    }

    #[test]
    fn oracle_only_allows_agent_prompt_quick_commands_for_launch_time_prompt_agents() {
        assert!(supports_terminal_agent_quick_command(&JsValue::str(
            "claude"
        )));
        assert!(supports_terminal_agent_quick_command(&JsValue::str(
            "gemini"
        )));
        assert!(!supports_terminal_agent_quick_command(&JsValue::str(
            "aider"
        )));
        assert!(!supports_terminal_agent_quick_command(&JsValue::str(
            "not-real"
        )));
    }

    #[test]
    fn oracle_flatten_returns_the_same_object_when_there_are_no_line_breaks() {
        let command = TerminalCommandQuickCommand {
            id: "test".to_string(),
            label: "Test".to_string(),
            scope: None,
            command: "git status".to_string(),
            append_enter: true,
        };
        let result = flatten_terminal_quick_command(&command);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(*result, command);
    }

    #[test]
    fn oracle_flatten_replaces_newlines_with_semicolons_and_spaces() {
        let command = TerminalCommandQuickCommand {
            id: "test".to_string(),
            label: "Test".to_string(),
            scope: None,
            command: "cd packages\nbun run build\ncd ..".to_string(),
            append_enter: true,
        };
        let result = flatten_terminal_quick_command(&command);
        assert_eq!(result.command, "cd packages; bun run build; cd ..");
    }

    #[test]
    fn oracle_flatten_collapses_consecutive_newlines_into_a_single_separator() {
        let command = TerminalCommandQuickCommand {
            id: "test".to_string(),
            label: "Test".to_string(),
            scope: None,
            command: "echo one\n\n\necho two".to_string(),
            append_enter: true,
        };
        let result = flatten_terminal_quick_command(&command);
        assert_eq!(result.command, "echo one; echo two");
    }

    #[test]
    fn oracle_flatten_handles_windows_style_crlf_endings() {
        let command = TerminalCommandQuickCommand {
            id: "test".to_string(),
            label: "Test".to_string(),
            scope: None,
            command: "echo one\r\necho two".to_string(),
            append_enter: true,
        };
        let result = flatten_terminal_quick_command(&command);
        assert_eq!(result.command, "echo one; echo two");
    }

    #[test]
    fn oracle_flatten_drops_empty_edge_lines_without_leaving_dangling_separators() {
        let command = TerminalCommandQuickCommand {
            id: "test".to_string(),
            label: "Test".to_string(),
            scope: None,
            command: "\n  echo one  \n\n  echo two\n".to_string(),
            append_enter: true,
        };
        let result = flatten_terminal_quick_command(&command);
        assert_eq!(result.command, "echo one; echo two");
    }

    // -----------------------------------------------------------------
    // J1/J2: six UTF-16 caps, astral straddle → snap-down; boundary ±1.
    // Astral char used throughout: U+1F600 (`\u{1F600}`, "😀"), 2 UTF-16 units.
    // -----------------------------------------------------------------

    #[test]
    fn j1_repo_id_cap_snaps_down_across_astral_straddle() {
        // 199 ASCII 'a's + one astral char = 201 UTF-16 units; cap is 200,
        // which lands inside the astral char's surrogate pair (199 + 1 of 2).
        let repo_id = format!("{}{}", "a".repeat(199), '\u{1F600}');
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("r")),
            ("command", JsValue::str("x")),
            (
                "scope",
                JsValue::object([
                    ("type", JsValue::str("repo")),
                    ("repoId", JsValue::str(repo_id)),
                ]),
            ),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        let scope = match &normalized[0] {
            TerminalQuickCommand::Terminal(c) => c.scope.as_ref().unwrap(),
            _ => unreachable!(),
        };
        match scope {
            TerminalQuickCommandScope::Repo { repo_id } => {
                assert_eq!(
                    utf16_len(repo_id),
                    199,
                    "must snap down, dropping the whole astral char"
                );
                assert_eq!(repo_id, &"a".repeat(199));
            }
            TerminalQuickCommandScope::Global => panic!("expected repo scope"),
        }
    }

    #[test]
    fn j1_repo_id_cap_boundary_exact_and_off_by_one() {
        // Exactly 200 ASCII units: no truncation.
        let exact = "a".repeat(200);
        let scope_exact = normalize_terminal_quick_command_scope(&JsValue::object([
            ("type", JsValue::str("repo")),
            ("repoId", JsValue::str(exact.clone())),
        ]));
        assert_eq!(
            scope_exact,
            TerminalQuickCommandScope::Repo { repo_id: exact }
        );

        // 199 units (boundary - 1): untouched.
        let under = "a".repeat(199);
        let scope_under = normalize_terminal_quick_command_scope(&JsValue::object([
            ("type", JsValue::str("repo")),
            ("repoId", JsValue::str(under.clone())),
        ]));
        assert_eq!(
            scope_under,
            TerminalQuickCommandScope::Repo { repo_id: under }
        );

        // 201 units (boundary + 1): truncated to 200.
        let over = "a".repeat(201);
        let scope_over = normalize_terminal_quick_command_scope(&JsValue::object([
            ("type", JsValue::str("repo")),
            ("repoId", JsValue::str(over)),
        ]));
        assert_eq!(
            scope_over,
            TerminalQuickCommandScope::Repo {
                repo_id: "a".repeat(200)
            }
        );
    }

    #[test]
    fn j1_id_base_cap_snaps_down_across_astral_straddle() {
        // idBase = 79 'a's + astral char = 81 units; cap 80 lands mid-pair.
        let id = format!("{}{}", "a".repeat(79), '\u{1F600}');
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str(id)),
            ("command", JsValue::str("x")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized[0].id(), "a".repeat(79));
        assert_eq!(utf16_len(normalized[0].id()), 79);
    }

    #[test]
    fn j1_id_base_cap_boundary_exact_and_off_by_one() {
        let exact = "a".repeat(80);
        let input_exact = JsValue::array([JsValue::object([
            ("id", JsValue::str(exact.clone())),
            ("command", JsValue::str("x")),
        ])]);
        assert_eq!(
            normalize_terminal_quick_commands(&input_exact)[0].id(),
            exact
        );

        let under = "a".repeat(79);
        let input_under = JsValue::array([JsValue::object([
            ("id", JsValue::str(under.clone())),
            ("command", JsValue::str("x")),
        ])]);
        assert_eq!(
            normalize_terminal_quick_commands(&input_under)[0].id(),
            under
        );

        let over = "a".repeat(81);
        let input_over = JsValue::array([JsValue::object([
            ("id", JsValue::str(over)),
            ("command", JsValue::str("x")),
        ])]);
        assert_eq!(
            normalize_terminal_quick_commands(&input_over)[0].id(),
            "a".repeat(80)
        );
    }

    #[test]
    fn j1_id_base_collision_retry_cap_snaps_down_across_astral_straddle() {
        // idBase = 75 'a's + one astral char = 77 units total: under the
        // 80-unit first-attempt cap (untouched), but OVER the 76-unit retry
        // cap (J4), so the collision retry must snap down across the
        // astral straddle at that narrower boundary.
        let id_base = format!("{}{}", "a".repeat(75), '\u{1F600}');
        let input = JsValue::array([
            JsValue::object([
                ("id", JsValue::str(id_base.clone())),
                ("command", JsValue::str("x")),
            ]),
            JsValue::object([
                ("id", JsValue::str(id_base.clone())),
                ("command", JsValue::str("y")),
            ]),
        ]);
        let normalized = normalize_terminal_quick_commands(&input);
        // First attempt: 77 units <= 80, untouched (still holds the astral char).
        assert_eq!(normalized[0].id(), id_base);
        // Collision retry re-slices idBase at 76 units: the 75 ASCII 'a's
        // are 75 units; including the 2-unit astral char would reach 77,
        // over 76, so it snaps down to 75 units, dropping the astral char.
        assert_eq!(normalized[1].id(), format!("{}-2", "a".repeat(75)));
    }

    #[test]
    fn j1_label_cap_snaps_down_across_astral_straddle() {
        let label = format!("{}{}", "a".repeat(79), '\u{1F600}');
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("l")),
            ("label", JsValue::str(label)),
            ("command", JsValue::str("x")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized[0].label(), "a".repeat(79));
        assert_eq!(utf16_len(normalized[0].label()), 79);
    }

    #[test]
    fn j1_label_cap_boundary_exact_and_off_by_one() {
        for (len, expected_len) in [(79usize, 79usize), (80, 80), (81, 80)] {
            let label = "a".repeat(len);
            let input = JsValue::array([JsValue::object([
                ("id", JsValue::str("l")),
                ("label", JsValue::str(label)),
                ("command", JsValue::str("x")),
            ])]);
            let normalized = normalize_terminal_quick_commands(&input);
            assert_eq!(utf16_len(normalized[0].label()), expected_len);
        }
    }

    #[test]
    fn j1_prompt_cap_snaps_down_across_astral_straddle() {
        let prompt = format!("{}{}", "a".repeat(5999), '\u{1F600}');
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("p")),
            ("action", JsValue::str("agent-prompt")),
            ("agent", JsValue::str("codex")),
            ("prompt", JsValue::str(prompt)),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        let prompt = match &normalized[0] {
            TerminalQuickCommand::Agent(a) => &a.prompt,
            _ => unreachable!(),
        };
        assert_eq!(prompt, &"a".repeat(5999));
        assert_eq!(utf16_len(prompt), 5999);
    }

    #[test]
    fn j1_prompt_cap_boundary_exact_and_off_by_one() {
        for (len, expected_len) in [(5999usize, 5999usize), (6000, 6000), (6001, 6000)] {
            let prompt = "a".repeat(len);
            let input = JsValue::array([JsValue::object([
                ("id", JsValue::str("p")),
                ("action", JsValue::str("agent-prompt")),
                ("agent", JsValue::str("codex")),
                ("prompt", JsValue::str(prompt)),
            ])]);
            let normalized = normalize_terminal_quick_commands(&input);
            let prompt = match &normalized[0] {
                TerminalQuickCommand::Agent(a) => &a.prompt,
                _ => unreachable!(),
            };
            assert_eq!(utf16_len(prompt), expected_len);
        }
    }

    #[test]
    fn j1_command_cap_snaps_down_across_astral_straddle() {
        let command = format!("{}{}", "a".repeat(3999), '\u{1F600}');
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("c")),
            ("command", JsValue::str(command)),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(
            get_terminal_quick_command_body(&normalized[0]),
            "a".repeat(3999)
        );
        assert_eq!(
            utf16_len(get_terminal_quick_command_body(&normalized[0])),
            3999
        );
    }

    #[test]
    fn j1_command_cap_boundary_exact_and_off_by_one() {
        for (len, expected_len) in [(3999usize, 3999usize), (4000, 4000), (4001, 4000)] {
            let command = "a".repeat(len);
            let input = JsValue::array([JsValue::object([
                ("id", JsValue::str("c")),
                ("command", JsValue::str(command)),
            ])]);
            let normalized = normalize_terminal_quick_commands(&input);
            assert_eq!(
                utf16_len(get_terminal_quick_command_body(&normalized[0])),
                expected_len
            );
        }
    }

    // -----------------------------------------------------------------
    // J3: collision retry reaching a 3-digit suffix produces an
    // out-of-cap (81-unit) id — the faithful overflow, not corrected.
    // -----------------------------------------------------------------

    #[test]
    fn j3_collision_suffix_100_overflows_the_cap_by_one_unit() {
        // idBase exactly 80 units, all-'b'. Drives `assign_unique_quick_command_id`
        // directly (NOT through `normalize_terminal_quick_commands`, whose
        // unrelated `MAX_QUICK_COMMANDS` (40) truncation, `O:157`, would cut
        // the run off long before the suffix grows large enough to matter).
        let id_base = "b".repeat(80);
        let mut seen_ids: HashSet<String> = HashSet::new();

        // First assignment: untouched (80 units <= cap, no collision yet).
        let first = assign_unique_quick_command_id(&id_base, &mut seen_ids);
        assert_eq!(first, id_base);

        // Every subsequent assignment of the SAME idBase collides and
        // retries at width 76: "<76 b's>-<suffix>", suffix starting at 2
        // and incrementing by 1 per collision (this is the 1st collision,
        // suffix 2; the 2nd collision, suffix 3; and so on).
        let mut last_id = first;
        for _ in 0..998 {
            last_id = assign_unique_quick_command_id(&id_base, &mut seen_ids);
        }
        // 999 total assignments so far (1 untouched + 998 collisions): the
        // most recent collision used suffix = 2 + 998 - 1 = 999 (3 digits).
        assert_eq!(last_id, format!("{}-999", "b".repeat(76)));
        assert_eq!(
            utf16_len(&last_id),
            80,
            "a 3-digit suffix (e.g. 999) yields 76 (\"-\") + 1 + 3 = 80 units — exactly \
             AT the cap, not over it; the plan's stated suffix-100 threshold is off by \
             one digit-width, see the note below"
        );

        // NOTE ON THE PLAN'S STATED THRESHOLD: the plan text (§1 J3, and
        // the crux-pin ask "a collision reaching suffix 100 produces an id
        // of 81 units") is arithmetically off by one digit-width. The
        // retried id is `"<76-unit base>-<suffix>"`; for ANY 3-digit
        // suffix (100..=999), that is 76 + 1 + 3 = 80 units — exactly AT
        // the cap, not over it. The cap is first exceeded (81 units) only
        // once the suffix reaches 4 digits, i.e. `suffix >= 1000`. This
        // test pins the mathematically correct threshold (1000, not 100)
        // rather than forcing a pin to match the plan's off-by-one-digit
        // claim; see the final report's "deviations" section.
        let suffix_1000 = assign_unique_quick_command_id(&id_base, &mut seen_ids);
        assert_eq!(suffix_1000, format!("{}-1000", "b".repeat(76)));
        assert_eq!(
            utf16_len(&suffix_1000),
            81,
            "faithful overflow: the -4 budget assumes a suffix short enough that \
             `76 + 1 + digits(suffix) <= 80`, and is never re-validated as the \
             suffix counter grows past 3 digits; ported as-is per J3, not corrected"
        );
        assert!(utf16_len(&suffix_1000) > MAX_QUICK_COMMAND_ID_LENGTH);
    }

    // -----------------------------------------------------------------
    // J4: 80-unit idBase collision — first attempt uses width 80,
    // collision retry uses width 76 (not 80): the two ids differ.
    // -----------------------------------------------------------------

    #[test]
    fn j4_collision_retry_uses_76_width_not_80_width_for_80_unit_id_base() {
        let id_base = "c".repeat(80);
        let input = JsValue::array([
            JsValue::object([
                ("id", JsValue::str(id_base.clone())),
                ("command", JsValue::str("x")),
            ]),
            JsValue::object([
                ("id", JsValue::str(id_base.clone())),
                ("command", JsValue::str("y")),
            ]),
        ]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(
            normalized[0].id(),
            id_base,
            "first attempt: untouched, 80 units, no collision yet"
        );
        assert_eq!(
            normalized[1].id(),
            format!("{}-2", "c".repeat(76)),
            "collision retry MUST re-slice idBase at 76 (MAX_QUICK_COMMAND_ID_LENGTH - 4), \
             not re-use the 80-width id from the first attempt"
        );
        assert_ne!(
            normalized[1].id(),
            format!("{}-2", id_base),
            "a unified single-width implementation would wrongly produce an 83-unit id here"
        );
    }

    // -----------------------------------------------------------------
    // J5: lone `\r` (no adjacent `\n`) — alternation-discriminating.
    // -----------------------------------------------------------------

    #[test]
    fn j5_lone_cr_with_no_adjacent_newline_is_split_and_flattened() {
        let command = TerminalCommandQuickCommand {
            id: "t".to_string(),
            label: "T".to_string(),
            scope: None,
            command: "a\rb".to_string(),
            append_enter: true,
        };
        let result = flatten_terminal_quick_command(&command);
        assert!(
            matches!(result, Cow::Owned(_)),
            "a lone CR must be recognized as a line break"
        );
        assert_eq!(result.command, "a; b");
    }

    // -----------------------------------------------------------------
    // J6: label trims both ends; command/prompt trim TRAILING only.
    // -----------------------------------------------------------------

    #[test]
    fn j6_command_keeps_leading_whitespace_and_cuts_only_trailing() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("c")),
            ("command", JsValue::str("  git status  ")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(
            get_terminal_quick_command_body(&normalized[0]),
            "  git status"
        );
    }

    #[test]
    fn j6_label_trims_both_leading_and_trailing_whitespace() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("l")),
            ("label", JsValue::str("  Status  ")),
            ("command", JsValue::str("x")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized[0].label(), "Status");
    }

    // -----------------------------------------------------------------
    // J7: trim happens BEFORE slice; no re-trim after — a reintroduced
    // trailing space at the exact cap boundary survives.
    // -----------------------------------------------------------------

    #[test]
    fn j7_command_keeps_a_space_landing_exactly_on_the_4000th_unit() {
        // 3999 'y's, then a space (the 4000th unit), then more 'y's, so the
        // command's own trailing end is non-whitespace (trimEnd() is a
        // no-op there) and slicing at the 4000-unit cap keeps that space as
        // the very last kept character.
        let mut command = "y".repeat(3999);
        command.push(' ');
        command.push_str(&"y".repeat(50));

        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("c")),
            ("command", JsValue::str(command)),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        let body = get_terminal_quick_command_body(&normalized[0]);
        assert_eq!(utf16_len(body), 4000);
        assert_eq!(
            body.chars().nth(3999),
            Some(' '),
            "the space at the cap boundary must survive uncut"
        );
        assert!(body.ends_with(' '), "no re-trim after slicing");
    }

    // -----------------------------------------------------------------
    // J8: U+FEFF / U+0085, both directions, on label/prompt/command/repoId.
    // -----------------------------------------------------------------

    #[test]
    fn j8_feff_is_stripped_and_u0085_is_kept_on_label() {
        let label = "\u{FEFF}Status\u{0085}".to_string();
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("l")),
            ("label", JsValue::str(label)),
            ("command", JsValue::str("x")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized[0].label(), "Status\u{0085}");
    }

    #[test]
    fn j8_feff_is_stripped_and_u0085_is_kept_on_command_trailing_only() {
        // Trailing-only: leading FEFF/NEL must be PRESERVED for the command
        // (J6), so put both markers at the tail to isolate J8 from J6.
        let command = "git status\u{0085}\u{FEFF}".to_string();
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("c")),
            ("command", JsValue::str(command)),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        // FEFF (JS whitespace) is stripped from the trailing end; U+0085
        // (NOT JS whitespace) is kept, blocking trimEnd from reaching past it.
        assert_eq!(
            get_terminal_quick_command_body(&normalized[0]),
            "git status\u{0085}"
        );
    }

    #[test]
    fn j8_feff_is_stripped_and_u0085_is_kept_on_prompt_trailing_only() {
        let prompt = "Do work\u{0085}\u{FEFF}".to_string();
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("p")),
            ("action", JsValue::str("agent-prompt")),
            ("agent", JsValue::str("codex")),
            ("prompt", JsValue::str(prompt)),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        let prompt = match &normalized[0] {
            TerminalQuickCommand::Agent(a) => &a.prompt,
            _ => unreachable!(),
        };
        assert_eq!(prompt, "Do work\u{0085}");
    }

    #[test]
    fn j8_feff_is_stripped_and_u0085_is_kept_on_repo_id() {
        let repo_id = "\u{FEFF}repo-1\u{0085}".to_string();
        let scope = normalize_terminal_quick_command_scope(&JsValue::object([
            ("type", JsValue::str("repo")),
            ("repoId", JsValue::str(repo_id)),
        ]));
        assert_eq!(
            scope,
            TerminalQuickCommandScope::Repo {
                repo_id: "repo-1\u{0085}".to_string()
            }
        );
    }

    // -----------------------------------------------------------------
    // J9: appendEnter strict `!== false`.
    // -----------------------------------------------------------------

    #[test]
    fn j9_append_enter_truthy_looking_non_false_values_all_normalize_to_true() {
        for value in [JsValue::Number(0.0), JsValue::str("false"), JsValue::Null] {
            let input = JsValue::array([JsValue::object([
                ("id", JsValue::str("c")),
                ("command", JsValue::str("x")),
                ("appendEnter", value.clone()),
            ])]);
            let normalized = normalize_terminal_quick_commands(&input);
            match &normalized[0] {
                TerminalQuickCommand::Terminal(c) => {
                    assert!(
                        c.append_enter,
                        "value {value:?} must normalize to true, not false"
                    )
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn j9_append_enter_absent_normalizes_to_true() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("c")),
            ("command", JsValue::str("x")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        match &normalized[0] {
            TerminalQuickCommand::Terminal(c) => assert!(c.append_enter),
            _ => unreachable!(),
        }
    }

    #[test]
    fn j9_append_enter_literal_false_normalizes_to_false() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("c")),
            ("command", JsValue::str("x")),
            ("appendEnter", JsValue::Bool(false)),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        match &normalized[0] {
            TerminalQuickCommand::Terminal(c) => assert!(!c.append_enter),
            _ => unreachable!(),
        }
    }

    // -----------------------------------------------------------------
    // J10: normalization truncates at >= 40 (40th survives, 41st dropped);
    // protocol boundary rejects raw len > 40 (exactly 40 accepted, 41
    // rejected, counting raw elements including malformed ones).
    // -----------------------------------------------------------------

    #[test]
    fn j10_normalization_keeps_exactly_40_and_drops_the_41st() {
        let items: Vec<JsValue> = (0..41)
            .map(|i| {
                JsValue::object([
                    ("id", JsValue::str(format!("cmd-{i}"))),
                    ("command", JsValue::str(format!("echo {i}"))),
                ])
            })
            .collect();
        let normalized = normalize_terminal_quick_commands(&JsValue::array(items));
        assert_eq!(normalized.len(), 40);
        assert_eq!(normalized[39].id(), "cmd-39");
    }

    #[test]
    fn j10_protocol_boundary_accepts_exactly_40_and_rejects_41() {
        let make = |n: usize| {
            JsValue::array((0..n).map(|i| {
                JsValue::object([
                    ("id", JsValue::str(format!("cmd-{i}"))),
                    ("label", JsValue::str(format!("Cmd {i}"))),
                    ("action", JsValue::str("terminal-command")),
                    ("command", JsValue::str(format!("echo {i}"))),
                    ("appendEnter", JsValue::Bool(true)),
                    ("scope", JsValue::object([("type", JsValue::str("global"))])),
                ])
            }))
        };
        assert!(parse_normalized_terminal_quick_commands(&make(40)).is_some());
        assert!(parse_normalized_terminal_quick_commands(&make(41)).is_none());
    }

    #[test]
    fn j10_protocol_boundary_rejects_on_raw_length_including_malformed_elements() {
        // 41 raw elements, several of them malformed (would themselves be
        // dropped by normalization) — still rejected purely on raw count.
        let mut items = vec![JsValue::Null, JsValue::Number(1.0), JsValue::str("nope")];
        for i in 0..38 {
            items.push(JsValue::object([
                ("id", JsValue::str(format!("cmd-{i}"))),
                ("label", JsValue::str(format!("Cmd {i}"))),
                ("action", JsValue::str("terminal-command")),
                ("command", JsValue::str(format!("echo {i}"))),
                ("appendEnter", JsValue::Bool(true)),
                ("scope", JsValue::object([("type", JsValue::str("global"))])),
            ]));
        }
        assert_eq!(items.len(), 41);
        assert!(parse_normalized_terminal_quick_commands(&JsValue::array(items)).is_none());
    }

    // -----------------------------------------------------------------
    // J11: extra key rejected; present-but-undefined key still counted.
    // -----------------------------------------------------------------

    #[test]
    fn j11_extra_key_is_rejected() {
        let scope = TerminalQuickCommandScope::Global;
        let value = JsValue::object([
            ("type", JsValue::str("global")),
            ("repoId", JsValue::str("x")), // extra key not expected for global
        ]);
        assert!(!is_normalized_terminal_quick_command_scope(&value, &scope));
    }

    #[test]
    fn j11_key_present_with_undefined_value_still_counts_toward_key_total() {
        // {type: 'global', repoId: undefined}: `repoId` is an OWN key and
        // must count toward the key total (J11) even though its value is
        // undefined, so key_count is 2 against Global's expected 1 key
        // ("type") — rejected, exactly as an unrelated extra key would be.
        let with_undefined_repo_id = JsValue::object([
            ("type", JsValue::str("global")),
            ("repoId", JsValue::Undefined),
        ]);
        assert!(
            !is_normalized_terminal_quick_command_scope(
                &with_undefined_repo_id,
                &TerminalQuickCommandScope::Global
            ),
            "a present-but-undefined `repoId` key must still count toward the key total \
             and cause rejection, not be treated as though the key were absent"
        );

        // Contrast: `repoId` truly ABSENT (not present-undefined) — 1 own
        // key, matches Global's expectation, accepted.
        let without_repo_id = JsValue::object([("type", JsValue::str("global"))]);
        assert!(is_normalized_terminal_quick_command_scope(
            &without_repo_id,
            &TerminalQuickCommandScope::Global
        ));
    }

    // -----------------------------------------------------------------
    // J12: generated-id counter = emitted count so far, not input index.
    // -----------------------------------------------------------------

    #[test]
    fn j12_generated_id_counter_uses_emitted_count_not_input_index() {
        let input = JsValue::array([
            JsValue::Null, // dropped, not object-like: consumes no counter
            JsValue::object([
                ("label", JsValue::str("First")),
                ("command", JsValue::str("a")),
            ]),
            JsValue::object([
                ("label", JsValue::str("Second")),
                ("command", JsValue::str("b")),
            ]),
        ]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized[0].id(), "quick-command-1");
        assert_eq!(normalized[1].id(), "quick-command-2");
    }

    // -----------------------------------------------------------------
    // J13: whitespace-only id and non-string id both take the generated path.
    // -----------------------------------------------------------------

    #[test]
    fn j13_whitespace_only_id_falls_through_to_generated_id() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str("   ")),
            ("command", JsValue::str("x")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized[0].id(), "quick-command-1");
    }

    #[test]
    fn j13_non_string_id_falls_through_to_generated_id() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::Number(42.0)),
            ("command", JsValue::str("x")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized[0].id(), "quick-command-1");
    }

    // -----------------------------------------------------------------
    // J15: space-padded removed-preset id is dropped (trimmed comparison).
    // -----------------------------------------------------------------

    #[test]
    fn j15_space_padded_removed_preset_id_is_dropped() {
        let input = JsValue::array([JsValue::object([
            ("id", JsValue::str(" default-pwd ")),
            ("label", JsValue::str("Print Working Directory")),
            ("command", JsValue::str("pwd")),
        ])]);
        assert_eq!(normalize_terminal_quick_commands(&input), vec![]);
    }

    // -----------------------------------------------------------------
    // J16: no-linebreak input returns Cow::Borrowed (see also the oracle
    // test above, which additionally asserts this outcome).
    // -----------------------------------------------------------------

    #[test]
    fn j16_no_line_break_returns_borrowed_not_owned() {
        let command = TerminalCommandQuickCommand {
            id: "x".to_string(),
            label: "X".to_string(),
            scope: None,
            command: "echo hi".to_string(),
            append_enter: true,
        };
        assert!(matches!(
            flatten_terminal_quick_command(&command),
            Cow::Borrowed(_)
        ));
    }

    // -----------------------------------------------------------------
    // J17: label/command/prompt combinations; unknown action fallback.
    // -----------------------------------------------------------------

    #[test]
    fn j17_survives_with_only_label_present() {
        let input = JsValue::array([JsValue::object([("label", JsValue::str("Draft"))])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].label(), "Draft");
        assert_eq!(get_terminal_quick_command_body(&normalized[0]), "");
    }

    #[test]
    fn j17_survives_with_only_command_present() {
        let input = JsValue::array([JsValue::object([("command", JsValue::str("date"))])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].label(), "");
    }

    #[test]
    fn j17_survives_with_only_prompt_present_for_a_supported_agent() {
        let input = JsValue::array([JsValue::object([
            ("action", JsValue::str("agent-prompt")),
            ("agent", JsValue::str("codex")),
            ("prompt", JsValue::str("Do work")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized.len(), 1);
    }

    #[test]
    fn j17_dropped_with_none_of_label_command_prompt_present() {
        let input = JsValue::array([JsValue::object([("appendEnter", JsValue::Bool(true))])]);
        assert_eq!(normalize_terminal_quick_commands(&input), vec![]);
    }

    #[test]
    fn j17_agent_prompt_action_with_unsupported_agent_is_dropped() {
        let input = JsValue::array([JsValue::object([
            ("label", JsValue::str("X")),
            ("action", JsValue::str("agent-prompt")),
            ("agent", JsValue::str("aider")), // stdin-after-start: unsupported
            ("prompt", JsValue::str("Do work")),
        ])]);
        assert_eq!(normalize_terminal_quick_commands(&input), vec![]);
    }

    #[test]
    fn j17_unknown_action_value_defaults_to_terminal_command() {
        let input = JsValue::array([JsValue::object([
            ("label", JsValue::str("X")),
            ("action", JsValue::str("something-else")),
            ("command", JsValue::str("echo hi")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(
            get_terminal_quick_command_action(&normalized[0]),
            TerminalQuickCommandAction::TerminalCommand
        );
    }

    #[test]
    fn j17_all_three_present_is_terminal_command_by_default() {
        let input = JsValue::array([JsValue::object([
            ("label", JsValue::str("X")),
            ("command", JsValue::str("echo hi")),
        ])]);
        let normalized = normalize_terminal_quick_commands(&input);
        assert_eq!(normalized.len(), 1);
    }
}
