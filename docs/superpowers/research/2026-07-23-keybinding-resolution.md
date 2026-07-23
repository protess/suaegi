# Keybinding resolution — research (next backend milestone)

Research phase, no implementation. Ground truth: Orca vendored at
`…/scratchpad/orca-src` @ `v1.4.150-rc.0`. Every Orca claim is cited `file:line`
from source actually read; suaegi claims from the working tree on branch
`plan4-terminal-widget`. Rust was not written for this doc.

**Recommendation:** port Orca's **keybinding registry + chord parser +
event→action resolver + conflict detector + file (read/write/migrate)** into a
new leaf crate **`suaegi-keys`**. It is the highest-value subsystem that is
still (a) entirely pure/mutation-verifiable, (b) unported, and (c) not blocked
on any deferred UI. suaegi today has **zero** app-level keyboard-action layer —
`crates/suaegi-app/src/terminal/input.rs` only forwards raw iced key events into
the terminal widget; nothing resolves "Cmd+P → Go to File". Every future
non-terminal surface (tab switching, sidebar toggles, quick-open, workspace
ops) needs this layer to exist first.

---

## §0 What Orca does (cited)

Orca keeps its entire shortcut system in **one pure shared module** plus a thin
file/service pair in main:

- `src/shared/keybindings.ts` — **2299 lines**, the pure heart. Its only imports
  are types and a display-name map (`keybindings.ts:2-3`): no `fs`, no network,
  no Electron, no DOM. Test file `src/shared/keybindings.test.ts` is **1721
  lines**.
- `src/main/keybindings/keybinding-file.ts` (470 lines) — JSON read / write /
  migrate for `~/.orca/keybindings.json` (`keybinding-file.ts:25-27`,
  `FILE_VERSION = 1` at `:21`). Test: `keybinding-file.test.ts` (~13 KB).
- `src/main/keybindings/keybinding-service.ts` (91 lines) — lazy-cached snapshot
  wrapper + legacy migrations (`keybinding-service.ts:29`). Test: ~7.8 KB.

### The pure core (`src/shared/keybindings.ts`)

1. **Registry.** `KEYBINDING_DEFINITIONS` — **84** action definitions
   (`keybindings.ts:197`), each with `id`, `title`, `group`, `scope`
   (`global | tabs | terminal | browser | editor | fileExplorer | composer |
   settings`, `:5-13`), `searchKeywords`, per-platform `defaultBindings`
   (`{darwin, linux, win32}`, `:134-138`), and optional flags
   `allowInTerminal`, `allowBareKeybindings`, `allowShiftOnlyKeybindings`,
   `conflictGroup` (`:140-151`). Action ids are a closed union
   (`KeybindingActionId`, `:28-114`) including a templated
   `tab.newAgent.${TuiAgent}` family (`:26`, built at `:1059`).

2. **Chord parser / canonicalizer.** `parseKeybinding` (`:1294`),
   `parseModifierToken` (`:1219`), `normalizeKeyToken` (`:1137`),
   `canonicalizeParsedKeybinding` (`:1327`), plus double-tap chords
   (`parseDoubleTapKeybinding` `:1258`, `isDoubleTapBinding` `:1418`). Modifier
   grammar is `Mod | Cmd | Ctrl | Alt | Shift` (`:153`), where **`Mod` is
   virtual** — resolved to Cmd on darwin, Ctrl elsewhere (`platformModifiers`
   `:1837-1848`).

3. **Normalization + validation.** `normalizeKeybinding` (`:1414`),
   `normalizeKeybindingList` (`:1443`), and per-action variants
   `normalizeKeybindingListForAction` / `normalizeKeybindingArrayForAction`
   (`:1506`, `:1516`) which apply per-action rules: bare keys, shift-only, and
   **digit-index** actions (a chord over `1-9` canonicalized to `1` for stable
   display/conflict, `canonicalizeDigitIndexBinding` `:1475`,
   `finalizeDigitIndexBindings` `:1486`). Returns a discriminated
   `KeybindingValidationResult` (`:186`).

4. **Event→action resolver (the state machine).** `keybindingFromInput`
   (`:1739`), `keybindingMatchesInput` (`:2018`), `keybindingMatchesAction`
   (`:2083`), `matchKeybindingDigitIndex` (`:2113`). Input is a plain struct
   `KeybindingInput` (logical `key`, physical `code`, modifier booleans,
   `:156-169`). The resolver encodes the genuinely hard, easy-to-get-wrong
   platform edge cases — this is where the clone value concentrates:
   - macOS Option composes letters/punctuation (Option+A → å), so it falls back
     to physical `code` (`shouldUseMacOptionLetterPhysicalFallback` `:1864`,
     `…PunctuationPhysicalFallback` `:1878`, `letterKeyMatches` `:1892`).
   - Non-Latin / AltGr layouts fall back to physical code but must not turn
     international text entry into app shortcuts (`shouldUseSemanticPunctuation`
     `:1935`, `canFallBackToPhysicalCode` `:1621`, non-Latin fallback `:1598`).
   - Terminal-shortcut policy: `orca-first` keeps app chords inside terminals,
     `terminal-first` is the escape hatch (`keybindingIsActiveInContext`
     `:1823-1835`, `normalizeTerminalShortcutPolicy` `:1809`).

5. **Effective-binding resolution + conflicts.** `getEffectiveKeybindingsForAction`
   merges defaults with overrides (`:1772`); `findKeybindingConflicts`
   (`:2235`) buckets bindings by `conflictGroup ?? scope` and a
   platform-specific "conflict identity" (`:2043-2065`), reporting only
   conflicts that touch a *customized* action, with digit-index special-casing.

6. **Formatting.** `formatKeybinding` / `formatKeybindingList` (`:2156`,
   `:2186`) render human glyphs (⌘⌥⇧ on mac) — pure string output.

### The file layer (`src/main/keybindings/keybinding-file.ts`)

`readKeybindingFile` (`:248`) parses the document into a
`KeybindingFileSnapshot` (common + per-platform overrides + merged `overrides` +
`diagnostics`), tolerating a legacy flat root shape (`:273-276`) and
**dropping conflicting overrides with a diagnostic instead of failing** (bounded
fixpoint loop, `removeConflictingOverrides` `:212-246`). `writeKeybindingOverride`
(`:426`) validates, rejects conflicts, and writes atomically via temp-file +
rename (`writeJsonDocument` `:68-84`) into the **active platform section only**
(`:454-468`). Two one-shot migrations exist: `migrateLegacyKeybindings`
(overrides once lived in global settings, `:302`) and
`seedLegacyTabSwitchBindings` (per-action pin so an upgrade never changes an
existing user's effective bindings, `:335-381`). Note: Orca writes to
`~/.orca/keybindings.json` — suaegi's own config root applies; **we never write
Orca's path or the user's global config.**

---

## §1 Mutation-verifiable backend surface vs deferred UI

**In scope (pure, mutation-testable) — the whole `shared/keybindings.ts` +
`keybinding-file.ts` surface:**

| Surface | Nature | Test style |
|---|---|---|
| Registry (84 defs, per-platform defaults, scopes, flags) | static data | table/snapshot |
| Chord parse + canonicalize (incl. double-tap) | pure string→struct | property + unit |
| Normalize/validate (bare, shift-only, digit-index, per-action) | pure | unit |
| **Event→action resolver** (mac Option, non-Latin/AltGr, terminal policy) | pure struct→bool | unit (the crown jewel) |
| Effective bindings + conflict detection | pure over registry+overrides | unit |
| Formatting (glyphs) | pure string | unit |
| File read/parse/diagnostics/migrate/write | fs I/O over a temp dir | tempdir integration |

All of this is verifiable with **no human eyes, no live network, no
platform-specific runtime** — platform is a plain `KeybindingPlatform` argument
(`getKeybindingPlatform` `:1109`), so darwin/win32/linux behavior is tested on
any host. This is an unusually clean mutation-testing target: pure functions
with a large, existing, high-coverage test file to port as oracle.

**Deferred UI (do NOT build in this milestone) — the ~45 renderer files under
`src/renderer/…/settings/` that consume the shared module** (Settings shortcut
editor, `ShortcutRecorderButton.tsx`, `shortcut-recording-state.ts`,
`shortcuts-search.ts`, filter rail, etc.). Also deferred: the **dispatch side** —
actually *doing* the action when it fires (open quick-open palette, toggle
sidebar) — because most target surfaces are themselves deferred UI (Plan 9/10).
This milestone delivers the layer that **answers "which action id does this key
event map to?"** and persists customizations; wiring a resolved action to an
effect is a follow-up per surface as those surfaces land.

---

## §2 Where it lands + layering

**New leaf crate `suaegi-keys`** (added to the workspace members in
`Cargo.toml`). Dependencies: `serde`/`serde_json` (file layer) and `thiserror`
only — **no iced, no tokio, no other suaegi crate.** This keeps mutation runs
fast and isolated and respects suaegi's one-directional dep rule.

- `suaegi-keys` (leaf) — registry, parse/normalize, resolver, conflicts,
  format, file read/write/migrate. Zero UI, zero platform runtime.
- `suaegi-app → suaegi-keys` — the app owns the **one** impurity: an adaptor
  that maps an iced `keyboard::Key` / `Modifiers` / physical `Physical` into the
  crate's `KeybindingInput` struct (the iced types already flow through
  `crates/suaegi-app/src/terminal/input.rs:11-12`), calls the resolver, and
  routes the resolved `ActionId`. That adaptor is thin and the only part that
  isn't pure-testable — isolate it so the crate stays 100% autonomous-verifiable.

Why not fold into `suaegi-core`: core is domain + persistence for
workspaces/worktrees; keybindings is a self-contained subsystem with its own
config file and its own large test corpus. A dedicated crate mirrors Orca's own
separation (shared module vs the rest) and avoids coupling core's mutation
surface to it.

**Invariants respected:** never write the user's global/Orca config (write only
suaegi's config root, atomically via temp+rename per `keybinding-file.ts:68-84`);
transient parse issues degrade to diagnostics, not silent success
(`removeConflictingOverrides` drops-with-diagnostic, never false-negative);
no secrets involved; every ported function gets a **mutation-verified** test
(the repo's recurring empty-test failure mode — see memory
`mutation-verify-regression-tests`).

---

## §3 Smallest-first milestone breakdown

Each step is independently mutation-verifiable; ship in order.

- **M1 — Registry + chord parse/canonicalize.** Port `KeybindingActionId`,
  `KeybindingDefinition`, the 84 definitions, and `parseKeybinding` /
  `canonicalizeParsedKeybinding` / modifier grammar (`Mod` virtual).
  *Crux:* faithfully reproducing the 84-row table incl. per-platform defaults
  without transcription drift. *Risk:* the templated `tab.newAgent.${agent}`
  family — decide whether suaegi's agent set drives it or it's a fixed list.

- **M2 — Normalize/validate + digit-index + per-action rules.** Port
  `normalizeKeybinding*` and the bare/shift-only/digit-index finalizers.
  *Crux:* the digit-index canonical-to-`1` rewrite and its interaction with
  conflict identity. *Risk:* discriminated-result ergonomics in Rust (use an
  enum, not `Result<String,String>` overloaded like the TS union at `:186`).

- **M3 — Event→action resolver.** Port `KeybindingInput`,
  `keybindingMatchesInput`, `keybindingMatchesAction`, `matchKeybindingDigitIndex`
  and all the fallback helpers. **This is the crown jewel and the highest-risk
  step.** *Crux:* the macOS Option-compose and non-Latin/AltGr physical-code
  fallbacks (`:1864-1952`) — these depend on precise logical-vs-physical key
  semantics that differ between Electron's `KeyboardEvent` and iced. Port the
  logic against the *struct* first (host-independent), then reconcile the
  adaptor. *Risk:* getting iced's logical `key` vs physical `code` mapping to
  match what the fallbacks assume; validate with the ported test vectors.

- **M4 — Effective bindings + conflict detection.** Port
  `getEffectiveKeybindingsForAction` and `findKeybindingConflicts` (scope /
  conflictGroup bucketing, customized-only filtering, digit-index special-case).
  *Crux:* the conflict-identity function and the "only report if a customized
  action participates" rule. *Risk:* subtle set-intersection semantics
  (`:2277`).

- **M5 — File layer (read/parse/diagnostics/migrate/write).** Port
  `readKeybindingFile`, `writeKeybindingOverride`, atomic write, and
  `removeConflictingOverrides`. Tempdir-based tests. *Crux:* the drop-conflicts
  fixpoint and active-platform-only write semantics. *Risk:* atomic
  temp+rename on the target FS; decide the suaegi config path.
  (Legacy-migration one-shots M6 are optional/low-value for a fresh clone —
  `migrateLegacyKeybindings` / `seedLegacyTabSwitchBindings` guard Orca upgrade
  cohorts that suaegi has no history of; port only if we want parity, else
  document as intentionally skipped.)

- **M6 (integration, not this crate) — iced adaptor + one wired action.** In
  `suaegi-app`, map iced key events → `KeybindingInput`, resolve, and wire a
  single already-existing surface (e.g. a global no-op/log or an existing pane
  action) end-to-end to prove the boundary. Kept last and small; the only
  non-pure code.

Formatting (`formatKeybinding`) can fold into M1 or M4 — it's needed for
conflict error messages.

---

## §4 Open questions for Codex cross-validation

1. **Crate vs core.** Is a new `suaegi-keys` leaf crate right, or should this
   fold into `suaegi-core`? (I argue new crate for isolation + mutation speed.)
2. **Resolver/adaptor boundary.** Does iced 0.14 expose enough to fill
   `KeybindingInput` — logical key, physical code, and the four modifier
   booleans — including the physical-code fallback path the mac/non-Latin logic
   needs? Where exactly should the impure adaptor sit so the crate stays 100%
   autonomous-verifiable? (`terminal/input.rs` already consumes
   `keyboard::key::Physical`.)
3. **Scope of M3 fallbacks.** The macOS Option-compose and AltGr/non-Latin
   fallbacks are the riskiest, most platform-semantic logic. Port them verbatim
   now (using ported TS test vectors as oracle), or ship a documented simpler
   matcher first and layer fallbacks in? What's the false-fire risk of
   deferring?
4. **Legacy migrations (M6-file).** Skip Orca's upgrade-cohort one-shots
   (`migrateLegacyKeybindings`, `seedLegacyTabSwitchBindings`) since suaegi has
   no prior on-disk format? Or port for structural parity?
5. **Action set divergence.** Many of the 84 actions target surfaces suaegi
   hasn't built (browser, editor, fileExplorer, simulator). Port the full
   registry now (so resolution/conflict logic is exercised), or trim to
   surfaces that exist and grow it? (I lean: port the full registry — it's pure
   data and the conflict logic needs the full set to be meaningful.)
6. **Config path + format.** Confirm suaegi's config root and that we mirror
   Orca's `{version, keybindings, platforms:{darwin,linux,win32}}` document
   shape (`keybinding-file.ts:33-43`) for forward-compat, without ever touching
   the user's global config.

### Why it beats the alternatives (surveyed, cited)

- **automations/** (`hermes-cron-output.ts` 26 KB, `external-manager.ts` 17 KB,
  `precheck-runner.ts`, `run-target-resolution.ts`): large but **runtime/
  process-heavy** — cron scheduling, spawning external jobs, headless workspace
  creation. Only fragments (`run-target-resolution`, `dispatch-tokens`) are
  pure; the subsystem depends on agent/workspace machinery that's partly
  deferred. Lower purity, less self-contained.
- **skills/** (~25 files): dominated by **filesystem + WSL/plugin-cache
  scanning** (`discovery.ts`, `skill-discovery-wsl.ts`) — platform/fs-coupled
  and lower clone value (discovering Claude plugin skills on disk).
- **git depth** (stash/cherry-pick/rebase/blame): shell-out wrappers that need
  **live git fixtures** — integration, not pure-mutation — and it's a grab-bag,
  not one coherent state machine. Some porcelain parsing is pure but thin.
- **terminal search/copy-mode**: smaller value and naturally lives inside
  `suaegi-term`; copy-mode is UI-driven.

Keybindings wins on **(clone value: the whole non-terminal UX is gated on it) ×
(purity: a 2299-line pure module with a 1721-line test oracle) ×
(self-containment: one leaf crate, one thin adaptor)**.
