# Quick Open (fuzzy file finder) — research

Status: research only. No implementation. Deferred from Plan 9
(`docs/superpowers/research/2026-07-23-plan9-editor-filetree.md` §1b).

Orca ground truth vendored at `.../scratchpad/orca-src` @ `v1.4.150-rc.0`. All
`file:line` citations below are against that tree. Note: the task brief's paths
were stale — the real files live under `src/shared/` (pure filter/walk/scorer),
`src/main/ipc/` (the spawn drivers), and `src/renderer/src/components/` (the
scorer + palette). The brief's `src/main/git/quick-open-file-list.ts` does not
exist; the renderer hook `quick-open-file-list.ts` is the entry point.

**Headline finding: the fuzzy matcher is a ~100-line pure Orca-authored scorer,
not a library.** That decides the plan shape — M1 is a self-contained pure port,
mutation-verifiable end to end, no `nucleo`/`fuzzy-matcher` dependency required
(though §4 asks whether to adopt one anyway).

---

## §0 What Orca does (cited)

Two independent subsystems, do not conflate (this is the same split Plan 9 §1
flagged for the file tree):

1. **The lister** — produces a flat `string[]` of root-relative, `/`-separated
   paths for the whole worktree. Lives in the main process (`src/main/ipc/`) +
   pure shared policy (`src/shared/`).
2. **The scorer** — ranks that flat list against the query on every keystroke.
   Lives in the renderer (`src/renderer/src/components/quick-open-search.ts`),
   pure, no IO.

### 0a. The lister cascade (rg → git ls-files → readdir walk)

Entry: `listQuickOpenFiles(rootPath, store, excludePaths?, signal?, maxResults?)`
(`src/main/ipc/filesystem-list-files.ts:19`). It resolves/authorizes the root,
builds root-relative exclude prefixes, then branches:

**Tier 1 — ripgrep (primary).** Availability is checked *upfront* via
`checkRgAvailable` (`src/main/ipc/rg-availability.ts`), spawning `rg --version`
with a 5s timeout and a `settled` guard because spawn emits `error`+`close` in
non-deterministic order when rg is missing (`filesystem-list-files.ts:39-52`).
If unavailable → Tier 2. No caching (rg can be installed/removed mid-session).

When available it runs **two rg passes** built by `buildRgArgsForQuickOpen`
(`src/shared/quick-open-filter.ts:220-251`):
- primary: `--files --hidden [--path-separator /] <hidden-dir-globs> <exclude-globs> .`
- ignoredPass: same **plus `--no-ignore-vcs`** (surfaces gitignored files).
- Deliberately omits `--follow` so symlinks can't escape the root
  (`quick-open-filter.ts:218-219`).
- Hidden-dir pruning globs (`buildHiddenDirExcludeGlobs`,
  `quick-open-filter.ts:185-195`) use the **directory form `!**/name`** not the
  contents form `!**/name/**` — rg still descends into a dir matched only by the
  contents form, so only the directory form prunes traversal.
- cwd is `rootPath` and searchRoot is `.` so the root-relative exclude globs
  evaluate against cwd (`filesystem-list-files.ts:64-73`). An absolute target
  would filter output but not prune the nested-worktree traversal.
- Unbounded mode runs both passes in parallel (`Promise.all`); bounded
  (`maxResults`) runs primary first, then ignored only if budget remains, so
  source files claim the cap before a large ignored tree
  (`filesystem-list-files.ts:224-234`).

Per-pass spawn discipline (`filesystem-list-files.ts:75-222`) — this is the
transient≠empty heart of the design:
- **10s timeout** → discard buffer, kill, **reject** (never return a truncated
  prefix as if complete) (`:187-193`).
- signal-kill on `close` → discard buffer, **reject** (`:137-145`).
- spawn `error` → discard buffer, **reject** (`:131-136`).
- exit code 0 or 1 (rg's "no matches") → resolve. Code 2 **with** parsed paths
  → resolve (unreadable subdir but usable rest); code 2 with none → reject
  (`:151-159`).
- On any pass failure, `killSurvivors()` kills the sibling pass so repeated
  Quick Open attempts don't accumulate rg processes (`:197-210`).
- Each stdout line → WSL-translate if needed → `normalizeQuickOpenRgLine`
  (`quick-open-filter.ts:266-299`, strips CR, `./` prefix, root prefix, rejects
  `..`-escapes → `null`) → `shouldIncludeQuickOpenPath` blocklist backstop →
  `shouldExcludeQuickOpenRelPath` → `Set` dedup.

**Tier 2 — `git ls-files` (fallback when rg missing).**
`listFilesWithGit` (`src/main/ipc/filesystem-list-files-git-fallback.ts:77`).
First `git rev-parse --is-inside-work-tree` (10s, failure/timeout → treat as
non-git → Tier 3) (`:23-75,84-95`). If a work tree, two passes from
`buildGitLsFilesArgsForQuickOpen` (`quick-open-filter.ts:312-344`):
- primary: `-z -s --cached --others --exclude-standard --directory --no-empty-directory [-- . :(exclude,glob)…]`
- ignoredPass: `-z -s --others --ignored --exclude-standard --directory --no-empty-directory [pathspecs]`
- `-z` = NUL-delimited (preserves paths with newlines); `-s` = stage mode so
  gitlinks (mode `160000`) are identifiable without lstat; `--directory
  --no-empty-directory` collapse untracked subtrees into directory placeholders
  (expanded later); exclude prefixes become `:(exclude,glob)` pathspecs with a
  leading positive `.` pathspec so the exclude-only specs don't hit git's
  edge-case defaults.
- Same 10s/signal/error → **reject** discipline (`:227-231,202-220`). Crucially,
  the **ignored pass is best-effort**: its failure is caught and logged, primary
  results are kept (`:264-271`) — ignored files are supplementary.
- Directory placeholders (trailing `/`) and gitlinks are then expanded on the
  filesystem by `expandQuickOpenGitFileListing`
  (`src/shared/quick-open-readdir-walk.ts:266-349`): each placeholder is
  `lstat`-classified (`classifyQuickOpenGitEntry`, `:97-127`) into
  keep / fill-nested-repo / drop-placeholder, descendant placeholders collapsed
  (`collapseQuickOpenExpansionPaths`, `src/shared/quick-open-expansion-paths.ts`),
  then walked under the shared readdir budget. Result is `.sort()`ed to restore
  git's stable order before the `maxResults` slice
  (`filesystem-list-files-git-fallback.ts:292-303`).

**Tier 3 — raw readdir walk (last resort, non-git roots).**
`listQuickOpenFilesWithReaddir` (`quick-open-readdir-walk.ts:129-264`).
BFS over directories, 32-way concurrent `readdir` batches with serial result
processing (`:163-264`). Hard bounded by `QuickOpenReaddirBudget`
(`src/shared/quick-open-readdir-budget.ts`):
- `QUICK_OPEN_READDIR_MAX_FILES = 10_000`, `QUICK_OPEN_READDIR_TIMEOUT_MS = 10_000`.
- `consumeQuickOpenReaddirFileBudget` **throws `File listing exceeded 10000
  files`** when the cap is hit; `assertQuickOpenReaddirDeadline` throws `File
  listing timed out`. **These are errors, not silent truncation** — the palette
  maps them to "install rg" guidance (`quick-open-readdir-budget.ts:21-25`;
  `quick-open-install-rg-guidance.tsx`).
- lstat-before-readdir + lstat-after-readdir guards so a placeholder swapped to a
  symlink mid-walk can't escape the root (`:196-208`;
  `quick-open-directory-validation.ts` — only the explicitly-selected root may be
  a symlink, nested dirs never followed).
- `shouldDescend` skips `node_modules` + `HIDDEN_DIR_BLOCKLIST` (`:53-55`).

Note the cap asymmetry: **only Tier 3 has the 10k file cap.** rg and git ls-files
are bounded by the 10s timeout + optional `maxResults`, not a file count.

### 0b. The blocklist (shared, `quick-open-filter.ts:11-74`)

`HIDDEN_DIR_BLOCKLIST` = `.git .next .nuxt .cache .stably .vscode .idea .yarn
.pnpm-store .terraform .docker .husky .npm .npm-global .gvfs`, plus `node_modules`
(non-dotted), plus `.local/share` (path-form). **Blocklist not allowlist** — novel
dotfiles stay discoverable; explicitly does *not* block user dirs like `.config
.ssh .github`. `shouldIncludeQuickOpenPath` walks segments (no split alloc — runs
once per file on ~100k-file repos) as a correctness backstop after the rg/git
globs.

### 0c. Nested-worktree excludePaths (`quick-open-file-list.ts:29-97`)

The renderer computes `getNestedWorktreeExcludePaths` — sibling worktrees of the
same repo whose path is nested *under* the active worktree path
(`isNestedWorktreePath`, case-insensitive on Windows). Without this, when the
main worktree sits at repo root, rg/git list files from every linked worktree.
Passed as absolute paths → `buildExcludePathPrefixes`
(`quick-open-filter.ts:98-133`) normalizes to root-relative `/`-prefixes,
**silently dropping** malformed / outside-root / root-equal values (a stale
exclude path must not fail the whole request). `shouldExcludeQuickOpenRelPath`
(`:139-152`) matches on **segment boundary** so `packages/app` doesn't exclude
`packages/app2`.

### 0d. The fuzzy scorer (PURE, ~100 lines — `quick-open-search.ts:38-147`)

**Orca-authored, not a library.** `rankQuickOpenFiles(query, files, limit=50)`:
- Pre-indexes files once (`prepareQuickOpenFiles`, `:18-29`): slash-normalized
  `lowerPath` + `lowerFilename` + `inputIndex`.
- Query: trimmed, `\` → `/`, lowercased. Empty query → first `limit` files, score 0.
- Query > `QUICK_OPEN_QUERY_MAX_BYTES = 2KB` → `[]` (`:31-36,46-48`).
- `fuzzyMatchIndexedFile` (`:70-103`) — **subsequence scorer, LOWER score is
  better** (ascending sort):
  - Greedy left-to-right subsequence match of query chars in `lowerPath`.
    If not all query chars consumed → `-1` (reject).
  - `score += gap` (chars skipped between consecutive matches — penalize spread).
  - `score -= 5` when the matched char follows a `/`, `.`, or `-` boundary
    (reward word-boundary starts).
  - `score -= 100` if `lowerFilename` contains the whole query as a substring
    (strongly favor filename hits over path-scattered ones).
- Top-`limit` maintained via binary-search insertion into a sorted array
  (`insertTopResult`/`findInsertionIndex`, `:109-143`); ties broken by
  `inputIndex` (**stable** — preserves lister order) (`compareRankedResult:145-147`).

`QUICK_OPEN_RESULT_LIMIT = 50`. This is the entire ranking algorithm. It is
completely deterministic and pure — every branch is mutation-verifiable with
plain unit tests. See Orca's own `quick-open-search.test.ts` for the golden cases.

---

## §1 Mutation-verifiable surface vs deferred UI

**Pure (unit-testable, mutation-verify each branch — the bulk of the value):**
- The fuzzy scorer + ranking (`quick-open-search.ts`) — gap penalty, boundary
  bonus (`/`,`.`,`-`), filename-substring `-100`, subsequence reject `-1`,
  stable `inputIndex` tie-break, empty-query passthrough, 2KB query cap, top-50
  insertion order. **This is the crux to mutation-verify** — the score ordering.
- `buildRgArgsForQuickOpen` / `buildGitLsFilesArgsForQuickOpen` — exact flag +
  glob/pathspec construction, including the directory-form-vs-contents-form glob
  distinction and glob-metachar escaping (`escapeGlobPath`).
- `buildExcludePathPrefixes` + `shouldExcludeQuickOpenRelPath` — normalization,
  outside-root/root-equal dropping, segment-boundary matching.
- `shouldIncludeQuickOpenPath` (blocklist walk) + `normalizeQuickOpenRgLine`.
- `collapseQuickOpenExpansionPaths`, `parseQuickOpenGitLsFilesEntry`,
  `create/consume/assert` budget helpers (cap = error, not truncation).

**Real-fs integration (deterministic via a tempdir git repo, still automatable):**
- The actual rg / git ls-files / readdir spawn + streaming parse + timeout/kill/
  fallback wiring. Build a fixture repo (tracked + untracked + ignored + a nested
  linked worktree + a `node_modules`), assert the flat list. rg-present vs
  rg-absent both testable (skip/gate the rg-present case on `rg` in PATH; the
  rg-absent → git path is the important one and always runnable).
- The directory-placeholder expansion (git collapses untracked trees, walker
  re-expands) — Orca pins this in
  `filesystem-list-files-git-directory-expansion.test.ts:49-80`.

**Deferred UI (James's eyes — thin over the backend, same posture as Plan 9's
tree/editor widgets):** the palette widget itself, keyboard nav/selection,
match highlighting, the "install rg" guidance surface, debounce/incremental
re-rank on keystroke, open-file dispatch. The scorer *feeding* the widget is
pure and lands in M1; only the iced rendering is deferred.

**Reuse already in suaegi-git:** `GitRunner` (`runner.rs:60`) already has
`run` / `run_expecting` / `run_bytes` / `run_with_stdin` with output-cap +
timeout + process-tree-kill — the git ls-files passes are a direct fit
(`run_expecting` for the exit-code tolerance, and `-z` output via `run_bytes`).
`fs::list_dir` (`fs.rs:58`) already does one-level readdir with symlink-refusal
and escape-rejection — the Tier-3 walker is a bounded recursion over the same
primitive. `status::check_ignored` (`status.rs:94`, `git check-ignore -z
--stdin`) and `compare::resolve_in_worktree` (`compare.rs:388`, path-safety)
exist. suaegi does **not** yet have an rg-availability check or an rg driver.

---

## §2 Which crate + layering

**Recommendation: put the lister in `suaegi-git`, the scorer in its own pure
module (either a new leaf crate `suaegi-fuzzy` or a `suaegi-core` module).**

- The lister *is* git-and-fs work: it owns `GitRunner`, `git ls-files`,
  `check-ignore` discipline, `fs::list_dir`, and `resolve_in_worktree`
  path-safety — all already in `suaegi-git`. Adding an rg driver + the pure
  filter policy (a new `quick_open` module) there keeps the ignore/path-safety
  discipline in one crate. This mirrors Orca, where the lister sits in main/ipc
  next to the git runner and the pure policy in shared.
- The scorer has **zero** git/fs dependencies — it's string ranking. It should
  not live behind `suaegi-git`. Options: (a) a tiny leaf crate `suaegi-fuzzy`
  (cleanest — reusable by any future palette, e.g. command palette, symbol
  search); (b) a module in `suaegi-core`. Prefer (a) if `suaegi-core` would
  otherwise pull it in as a dep it doesn't need. Decide in §4/Codex.

**Invariants to hold (repo memory + Orca discipline):**
- **transient ≠ false-negative.** A lister that can't run rg must fall to git,
  then to walk — **never silently return empty**. Timeout/signal-kill/spawn-error
  on any spawn must **reject (error)**, never resolve a truncated prefix. This is
  the single most important behavior to port faithfully.
- **bounded traversal, no silent truncation.** The Tier-3 cap
  (`QUICK_OPEN_READDIR_MAX_FILES`) and deadline surface as **errors** the caller
  can act on ("install rg"), matching the repo's "no silent truncation" memory
  (`suaegi-resize-seq-global` sibling discipline). Do not clamp-and-pretend.
- **never write user global config** — pure listing/ranking, no writes at all.
- Leaf discipline: `suaegi-fuzzy` reverse-deps nothing; `suaegi-git` stays a
  leaf over `GitRunner`.

---

## §3 Smallest-first milestone breakdown

**M1 — the fuzzy scorer (pure, `suaegi-fuzzy` or core module).**
Port `prepareQuickOpenFiles` + `rankQuickOpenFiles` + `fuzzyMatchIndexedFile` +
the top-N insertion. Flat `&[String]` in, ranked `Vec<{path, score}>` out.
- Crux/risk: **the ranking is the whole feature** — mutation-verify every score
  term (gap, the three boundary chars, the `-100` filename bonus, the `-1`
  reject, stable `inputIndex` tie-break, empty-query passthrough, 2KB cap, top-50
  cutoff). Port Orca's `quick-open-search.test.ts` golden cases verbatim as the
  oracle. Watch: signed scores + "lower is better" ordering is easy to invert.
- No IO, no deps. Independently shippable and useful before any lister exists.

**M2 — the lister cascade (`suaegi-git::quick_open`).**
- M2a (pure): the arg/glob/pathspec builders + exclude normalization + blocklist
  + rg-line normalization + budget helpers. All mutation-verifiable, no spawn.
- M2b (integration): the rg driver + `check_rg_available`, then the `git
  ls-files` two-pass driver over `GitRunner`, then the bounded readdir walk over
  `fs::list_dir`. Plus directory-placeholder expansion.
- Crux/risk: **the fallback discipline** — rg-missing → git → walk, and every
  spawn's timeout/kill/error path must reject, NEVER resolve empty. Second crux:
  the git-ls-files flag set + `-z` NUL parsing + stage-mode gitlink detection +
  directory-collapse-then-expand (Orca's
  `filesystem-list-files-git-directory-expansion.test.ts` is the contract).
  Build the tempdir-repo fixture (tracked/untracked/ignored/nested-worktree/
  node_modules) as the integration oracle. The rg-present path can be gated on
  `rg` in PATH; the rg-absent→git path is always runnable and is the one that
  matters for the transient≠empty guarantee.

**M3 — excludePaths (nested linked worktrees).**
`buildExcludePathPrefixes` + segment-boundary exclusion wired through both rg
globs and git pathspecs, plus `rebaseExcludePrefixesForSubtree` for the walker.
- Crux/risk: segment-boundary correctness (`packages/app` ≠ `packages/app2`) and
  silently dropping malformed/outside-root/root-equal excludes so a stale path
  can't fail the request. Small, pure, mostly covered by M2a tests; separated
  because it needs the sibling-worktree data the caller (app) supplies.

**Deferred (post-backend, James's eyes): the iced palette widget** — input,
debounced re-rank, result list, match highlighting, keyboard nav, open dispatch,
"install rg" empty-state. The Cmd+Shift+P binding already exists in the registry.

Ordering rationale: M1 is pure and self-contained (ship + mutation-verify with
zero fs), M2 is the fs-integration heavy lift, M3 is a thin pure addition. Each
milestone is independently reviewable.

---

## §4 Open questions for Codex cross-validation

1. **Port the scorer vs adopt a Rust crate (`nucleo` / `fuzzy-matcher`)?** The
   scorer is ~100 pure lines and the ranking semantics (the exact gap/boundary/
   filename weights) *are* the UX — a library would change ranking behavior and
   lose the mutation-verifiable golden-test story. Recommendation: **port
   Orca's**, don't adopt. But `nucleo` is fast/battle-tested; worth a deliberate
   decision, especially if a future command palette wants the same matcher.
2. **Is rg a hard dependency or optional-with-fallback?** Orca treats it as
   optional (git → walk fallback, "install rg" guidance). Recommendation: match
   Orca — rg optional. Confirm suaegi wants to carry the full 3-tier cascade now,
   or ship git-ls-files-first + rg-later (simpler M2, but loses the fast path and
   the ignored-file breadth on non-git roots).
3. **Reuse `check_ignored` vs Orca's `--exclude-standard`?** Orca leans on git's
   `--exclude-standard` inside `ls-files` (one spawn, gitignore respected
   inline), *not* a separate `check-ignore` pass. suaegi has `check_ignored` but
   using it here would mean a second spawn per listing. Recommendation: follow
   Orca — `--exclude-standard`; keep `check_ignored` for the tree, not Quick
   Open.
4. **The cap value + whether to add an rg/git file cap.** Orca caps only Tier-3
   walk at 10k. Confirm 10k is right for suaegi and whether the timeout-only
   bound on rg/git is acceptable (it is in Orca — the 10s timeout is the bound).
5. **Scorer home: new `suaegi-fuzzy` leaf crate vs `suaegi-core` module?**
   Leaf crate is cleaner for reuse (command palette, symbol search later) but
   adds a crate. Decide based on whether anything else will want it soon.
6. **`maxResults`/bounded mode: needed for M1/M2 or defer?** Orca uses it for a
   separate bounded-autocomplete caller. Quick Open itself is unbounded (relies
   on the scorer's top-50). Recommendation: implement unbounded first, add
   `maxResults` only if a second caller appears — but the parallel-vs-sequential
   pass logic is coupled to it, so decide before coding M2b.
