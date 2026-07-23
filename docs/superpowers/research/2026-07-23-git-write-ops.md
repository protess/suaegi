# Git write-ops depth — research (next backend milestone)

Research phase, no implementation. Ground truth: Orca vendored at
`…/scratchpad/orca-src` @ `v1.4.150-rc.0`. Every Orca claim is cited `file:line`
from source actually read; suaegi claims are from the working tree on branch
`plan4-terminal-widget`. No Rust was written for this doc.

**Recommendation:** extend **`suaegi-git`** with the **staging + commit +
discard + conflict-detail** write-ops layer — Orca's `stageFile` / `unstageFile`
/ `bulkStageFiles` / `bulkUnstageFiles` / `commitChanges` / `getStagedCommitContext`
/ `discardChanges` / `bulkDiscardChanges`, plus the conflict-detail parser
(`parseUnmergedEntry` / `parseConflictKind` / `getConflictCompatibilityStatus`),
conflict-operation detection (`detectConflictOperation` / `resolveGitDir` /
`abortMerge` / `abortRebase`), and — the security core — the shared
untracked-discard path-safety module (`src/shared/git-discard-path-safety.ts`).

This is the single highest-value backend milestone that is (a) rich in
mutation-verifiable pure + real-fs logic, (b) genuinely security/data-loss
sensitive (worth the diligence), (c) unported, and (d) not blocked on any
deferred UI. suaegi today can **show** a worktree's diff (`compare.rs`) and
**classify** its status (`status.rs`) but cannot **stage, commit, or discard**
anything — the coding-agent-on-worktrees loop (agent works → human reviews diff
→ commit / discard / resolve) is a read-only dead end without it. It extends the
crate that already owns the exact idioms this needs: `:(literal)` pathspecs, WSL
path conversion, worktree-boundary path safety, and the established
`git init`-in-tempdir integration harness (53 test hits in `suaegi-git/src`).

---

## §0 What Orca does (cited)

Orca's git write path lives almost entirely in one large main module,
`src/main/git/status.ts` (2225 lines — read/status/diff **and** the mutating
ops), with the data-loss-critical helper factored into a pure shared file.

### Staging (argv construction, pure + real-git)

- `stageFile` (`status.ts:1882`) → `git add -- :(literal)<path>`.
- `unstageFile` (`status.ts:1901`) → `git restore --staged -- :(literal)<path>`.
- `bulkStageFiles` (`status.ts:2173`) / `bulkUnstageFiles` (`status.ts:2198`) —
  same, batched in `BULK_CHUNK_SIZE` chunks to avoid `E2BIG`.
- `literalPathspec` (`status.ts:2043`): `` `:(literal)${runtimePath}` ``, and for
  WSL only, backslashes → `/`. This is the same pathspec-literal discipline
  suaegi-git already applies on the **read** side (`status.rs` header cites
  `status-pathspec-literals`), now needed on the **write** side.
- Every op brackets the git call with `invalidateGitReadCaches()` in a
  `try/finally` (`status.ts:1887`, `:1894`) — a caching detail suaegi does not
  replicate (suaegi has no status cache), so it drops out of the port.

### Commit (argv + error-channel extraction, pure)

- `commitChanges` (`status.ts:1962`) → `git commit -m <message>`, returning
  `{ success, error? }`. The load-bearing logic (`status.ts:1972-1986`): on
  failure prefer **stderr**, then **stdout**, then the JS error message — because
  hook/GPG failures surface on stderr while "nothing to commit" is on stdout.
  This channel-preference rule is a pure, mutation-verifiable decision.
- `getStagedCommitContext` (`status.ts:1916`) gathers commit-message draft
  context: `branch --show-current`, `diff --cached --name-status` (returns
  `null` when the staged summary is empty → nothing staged), and best-effort
  `diff --cached --patch --minimal --no-color --no-ext-diff`, degrading to
  name-only on a max-buffer overflow (`status.ts:1944-1953`). The AI
  message-generation consumer is out of scope; the git-context assembly + the
  "empty ⇒ null" gate are pure.

### Discard — the security core (real-fs, data-loss sensitive)

- `discardChanges` (`status.ts:1995`): resolves the target, rejects paths
  outside the worktree via `isWithinWorktree` (`status.ts:2156`), probes tracked
  vs untracked with `git ls-files --error-unmatch`, then either
  `git restore --worktree --source=HEAD -- :(literal)<path>` (tracked) **or**
  hands untracked paths to the safety-gated cleaner.
- `bulkDiscardChanges` (`status.ts:2103`): validates **every** path is inside the
  worktree before mutating anything, partitions tracked/untracked via
  `listTrackedPathSpecs` + `isTrackedPathSpec` (`status.ts:2049`), restores
  tracked in chunks and clean-removes untracked.
- `cleanUntrackedPaths` (`status.ts:2081`) → `git clean -ffdx -- :(literal)…`
  (batched). Comment (`status.ts:2089`): pathspec cleanup is chosen over raw
  recursive `rm` specifically to avoid deletion through symlinked parents.
- **`src/shared/git-discard-path-safety.ts` (126 lines) — the pure heart.**
  `validateUntrackedDiscardTarget` resolves `realpath(worktree)` and requires
  the target's real path to stay inside it; a symlink **leaf** is allowed (git
  clean should delete the link itself) but a symlink **parent** is validated at
  `dirname` so it can't redirect recursive removal outside the real worktree
  (`git-discard-path-safety.ts:80-94`). On `ENOENT` it walks up to the nearest
  existing parent and validates that instead (`:31-49`). `assertTargetIsWorktreeChild`
  (`:51-70`) rejects `''`, `.`, `..`, `../…`, and absolute — force-removing the
  worktree root is never a valid discard. `removeSafeUntrackedDiscardTargets`
  (`:113`) validates all paths, runs the tracked-restore `beforeRemove` hook,
  then **re-validates** before the git-bounded clean (TOCTOU re-check).

### Conflict detail (pure parse + real-fs marker probe)

suaegi-git currently collapses all unmerged states to one `FileStatus::Conflicted`
(`status.rs:44-45`). Orca resolves the detail:

- `parseConflictKind` (`status.ts:877`): pure `XY` → enum over the 7 unmerged
  codes (`UU`→both_modified, `AA`→both_added, `DD`→both_deleted, `AU`→added_by_us,
  `UA`→added_by_them, `DU`→deleted_by_us, `UD`→deleted_by_them; else `null`).
- `parseUnmergedEntry` (`status.ts:842`): porcelain-v2 `u` records are
  **space**-separated (not tab); XY is field 2, stage modes are fields 4-6, path
  is field 10+ joined (may contain spaces) then C-unquoted. Submodule conflicts
  (mode `160000`) are dropped as out-of-scope (`status.ts:858`).
- `getConflictCompatibilityStatus` (`status.ts:900`): both_modified/both_added →
  modified, both_deleted → deleted; for the `*_by_*` variants it `existsSync`-es
  the file (git's result is merge-strategy dependent), defaulting to `modified`
  on fs error to keep the row visible.
- `detectConflictOperation` (`status.ts:923`): probes the resolved git dir for
  `MERGE_HEAD` / `rebase-merge/` / `rebase-apply/` / `CHERRY_PICK_HEAD` and
  returns `merge | rebase | cherry-pick | unknown` (precedence in that order).
- `resolveGitDir` (`status.ts:972`): reads the worktree `.git` **file**, parses
  the `gitdir: <path>` pointer (`status.ts:977`), resolves it relative to the
  worktree — the standard linked-worktree indirection. Pure parse + real-fs.
- `abortMerge` (`status.ts:954`) / `abortRebase` (`status.ts:963`) → `git merge
  --abort` / `git rebase --abort`.

---

## §1 Mutation-verifiable surface vs. deferred UI

**Pure (unit / mutation-testable, no I/O):**

- `literalPathspec` — `:(literal)` prefix + WSL backslash rule.
- `commitChanges` error-channel preference (stderr → stdout → message).
- `parseConflictKind`, `parseUnmergedEntry` (given a porcelain line),
  `parseBranchStatusChar`.
- `isWithinWorktree` / `isInsideOrEqual` / `assertTargetIsWorktreeChild` — path
  boundary predicates (the security predicates; property-testable with adversarial
  `..`, absolute, empty, embedded-NUL inputs).
- tracked/untracked partition: `isTrackedPathSpec`, chunking logic.

**Deterministic real-git / real-fs integration (the suaegi-git harness pattern —
`git init` in a `TempDir`, mutate, assert via a second git read):**

- `stageFile` / `unstageFile` / `bulkStage` / `bulkUnstage`: stage a file, assert
  `git diff --cached --name-only` shows it; unstage, assert it's gone.
- `commitChanges`: stage + commit, assert `git rev-parse HEAD` advanced and
  worktree clean; empty-index commit returns `success:false` with the stdout
  "nothing to commit" message.
- `getStagedCommitContext`: returns `null` on empty index; returns branch + name-
  status + patch when staged.
- `discardChanges` / `bulkDiscardChanges`: tracked-modified → restored to HEAD;
  untracked file → removed; **symlink-parent escape → rejected, and the outside
  target survives** (the key data-loss test); worktree-root / `..` → rejected.
- `detectConflictOperation`: craft a real merge conflict (`git merge` two
  divergent branches) in a tempdir → `merge`; a `rebase-merge/` dir → `rebase`.
  `resolveGitDir`: real linked worktree via `git worktree add` → follows the
  `.git`-file pointer (suaegi-git's `worktree.rs` tests already create these).

**Deferred UI (explicitly NOT this milestone):** the diff-panel stage/discard
buttons, the commit-message composer, the conflict-badge rendering, and any AI
commit-message generation. This milestone stops at the crate boundary: it returns
data and performs git mutations; `suaegi-app` wiring is a separate, later step
(same split as every prior suaegi backend milestone).

---

## §2 Which suaegi crate + layering

**Extends `suaegi-git`.** No new crate. Rationale:

- The idioms already live there: `:(literal)` pathspecs and WSL conversion
  (`status.rs`), worktree-boundary path safety (`fs.rs` `read_file`/`write_file`
  already resolve-and-bound-check against the worktree root), the `GitRunner`
  argv/timeout/stdin abstraction (`runner.rs`), and the `git init`-in-tempdir
  integration harness.
- It composes with what's ported: `compare.rs` produces the diff the human
  reviews; `status.rs` produces the file list; this milestone adds the verbs that
  act on that list. The conflict-detail work is a **direct upgrade** of
  `status.rs`'s existing `FileStatus::Conflicted` into a richer conflict kind +
  a new `conflict_operation()` probe.
- Layering respected: `suaegi-git` stays a leaf (depends only on `suaegi-core`
  types + the runner). It never writes user global config; discard is bounded to
  the worktree by the ported path-safety module; no secrets, no network. Suggested
  new file: `crates/suaegi-git/src/write_ops.rs` (staging/commit/discard) +
  `crates/suaegi-git/src/conflict.rs` (kind parse + operation probe + abort), or
  fold conflict detail into `status.rs` beside the existing parser to keep the
  porcelain-v2 knowledge in one place.

---

## §3 Milestone breakdown (smallest-first, per-step crux/risk)

Ordered so each step is independently mutation-verified and the security-critical
step lands with the most surrounding test scaffolding, not first.

- **M1 — Staging (`stageFile`/`unstageFile` + bulk).** *Crux:* `literalPathspec`
  correctness (unit) + argv (`add` vs `restore --staged`). *Risk:* low. Pure
  pathspec test + real-git round-trip (stage → assert `--cached` → unstage →
  assert gone). Foundational; everything else reuses `literalPathspec`.

- **M2 — Commit + staged context (`commitChanges`, `getStagedCommitContext`).**
  *Crux:* the stderr→stdout→message error-channel rule (pure, mutation-verified
  against a forced-failure fixture) and the "empty staged ⇒ `null`/`success:false`"
  gate. *Risk:* low-medium — commit hooks / GPG in the test env can perturb
  output; tests must set `commit.gpgsign=false` and a deterministic
  `user.name`/`user.email` in the tempdir repo config, and avoid touching the
  user's global git config (invariant).

- **M3 — Conflict detail (`parseConflictKind`, `parseUnmergedEntry`,
  `getConflictCompatibilityStatus`) + operation probe (`detectConflictOperation`,
  `resolveGitDir`, `abortMerge`/`abortRebase`).** *Crux:* porcelain-v2 `u`-record
  parsing is **space**-separated with a multi-field path tail — a different shape
  from the `-z` NUL records `status.rs` already parses; getting field offsets
  wrong silently mislabels conflicts. *Risk:* medium (parsing subtlety). Pure
  line-parse tests + a real merge-conflict tempdir fixture for the operation
  probe. Upgrades `FileStatus::Conflicted`; must stay backward-compatible with
  the existing `status.rs` consumers.

- **M4 — Discard (`discardChanges`, `bulkDiscardChanges`) + the shared
  path-safety port (`git-discard-path-safety.ts`).** ⚠️ **DATA-LOSS / SECURITY
  step — the reason this milestone is worth doing carefully.** *Crux:* faithfully
  porting `validateUntrackedDiscardTarget`: symlink-leaf allowed but symlink-parent
  validated at `dirname`; `realpath` of both worktree and target; ENOENT →
  nearest-existing-parent walk; the double-validate (before + after `beforeRemove`)
  TOCTOU re-check; `assertTargetIsWorktreeChild` rejecting root/`.`/`..`/absolute;
  and `git clean -ffdx` (pathspec form, never raw `rm`). *Risk:* HIGH if rushed —
  a bug deletes files outside the worktree. Mitigation: land M4 last, behind an
  adversarial test matrix (symlinked-parent-out, symlink-leaf-in, `..` escape,
  absolute path, worktree-root, ENOENT-parent), each asserting the **outside**
  target still exists after a rejected discard. This is a security-review
  candidate before merge.

Every regression test must be mutation-verified (repo history: 5 hollow tests
have slipped through here). The porcelain-parse and path-predicate steps are the
easiest to write empty-passing tests for — verify each catches a seeded mutant.

---

## §4 Open questions for Codex cross-validation

1. **Scope cut:** is M3 (conflict detail + operation probe) in-scope for v1, or
   should it defer until a conflict-resolution UI is planned? It's pure and cheap,
   but its only consumer today would be a status field nothing renders yet.
   Include for completeness, or ship M1/M2/M4 (the review→commit→discard loop)
   and hold M3?
2. **`getStagedCommitContext` boundary:** port the git-context assembly now
   (branch + name-status + capped patch), or is it dead weight until an AI/manual
   commit-message composer exists? It's the seam to the deferred composer UI.
3. **Bulk vs single:** port both single (`stageFile`) and bulk
   (`bulkStageFiles`) variants, or only bulk (single = bulk-of-one)? Orca keeps
   both; suaegi could collapse to bulk with a one-element convenience.
4. **Discard `git clean` flags:** Orca uses `-ffdx` (force, dirs, **including
   ignored** via `-x`). Is discarding *ignored* untracked files the intended
   semantics for suaegi, or should suaegi use `-ffd` (respect `.gitignore`)? This
   changes what data a "discard" can destroy — decide deliberately.
5. **Conflict `FileStatus` shape:** upgrade the existing `FileStatus::Conflicted`
   to carry a `ConflictKind`, or add a parallel field so the porcelain-v1 read
   path (`status.rs`) and a new porcelain-v2 unmerged path don't diverge? Orca
   reads v2 for conflicts specifically; suaegi's `status.rs` is v1 `-z`.
6. **Commit env isolation:** confirm the test harness fully isolates commit
   identity + signing from the user's global git config (invariant: never read/
   write user global config). Is `-c user.name=… -c user.email=… -c
   commit.gpgsign=false` per-invocation preferable to writing repo-local config?
7. **`invalidateGitReadCaches`:** suaegi has no status cache, so the port drops
   it. Confirm no consumer relies on post-mutation cache invalidation semantics
   (there is none today), i.e. the drop is safe.
