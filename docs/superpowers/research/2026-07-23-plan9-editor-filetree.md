# Plan 9 research — editor + file tree

Research phase (no implementation). Ground truth: Orca vendored at
`…/scratchpad/orca-src` @ `v1.4.150-rc.0` (commit `b25c298`). Every Orca claim
below is cited `file:line` from source actually read. suaegi citations are from
the current `plan4-terminal-widget` working tree.

---

## §0 Decision-forcing finding

**Orca embeds a full in-app code editor. It is a real Monaco (VS Code) embed —
not a launcher, not a diff viewer, not a read-only preview.** The
external-editor-launch code is a *side feature* ("Open in VS Code/Cursor"), not
the editor story.

Evidence the embedded editor is real and primary:
- A full Monaco integration: `MonacoEditor.tsx` (34k), `EditorContent.tsx`
  (37k), `EditorPanel.tsx` (16k), plus ~120 supporting modules under
  `src/renderer/src/components/editor/` (autosave, undo retention, conflict
  decorations, large-paste handling, context-menu paste, etc.).
- A real read/write file loop over IPC: `fs:readFile`
  (`src/main/ipc/filesystem.ts:542`) returns editable text; `fs:writeFile`
  (`filesystem.ts:806`) writes it back — `writeFile(filePath, content,'utf-8')`
  (`filesystem.ts:829`).
- A headless autosave controller with debounce, conflict detection, and
  external-change reconciliation: `editor-autosave-controller.ts`,
  `editor-self-write-registry.ts`, `ExternalFileChangeBanner.tsx`.
- Type-specific viewers layered on top of Monaco: image/PDF/CSV/ipynb/markdown
  (`editor-panel-render-model.ts:158-161`).

Evidence the external launcher is a *side feature*, not the editor:
- One small pure module, `src/main/external-editor-launch.ts` (188 lines), whose
  only job is to turn an editor command + a path into a spawn spec.
- Invoked from a single IPC handler `shell:openInExternalEditor`
  (`src/main/ipc/shell.ts:142-146`) → `openInExternalEditor` → `spawn(..,
  {detached:true, stdio:'ignore'})` (`shell.ts:64-71`). Default command is
  `code` when unset (`external-editor-launch.ts:7,165`). It detaches and forgets
  — it does not participate in editing.

### What this means for the plan shape

A faithful embedded-editor **parity** port is enormous and **mostly
human-eyes** (it is a text-editing widget: caret, selection, syntax highlight,
undo, IME, large-file virtualization). suaegi should **not** attempt to port
Monaco. Two things follow:

1. **The mutation-verifiable milestone is the backend, not the widget.** The
   file-tree lister, ignore filtering, git-status decoration, the safe file
   read/write surface (path-traversal + symlink guards), and the
   external-editor launcher (argv construction) are all pure, testable Rust with
   no pixels. That is where Plan 9's verifiable value is.

2. **The editor *widget* itself should reuse iced 0.14's built-in
   `text_editor`** (a genuine multi-line editor backed by cosmic-text, already
   in the dependency tree suaegi uses) rather than porting Monaco. Wiring that
   widget to the safe read/write backend is human-eyes work but small; a Monaco
   equivalent is out of scope and should be deferred (see Defer list).

**Net: Plan 9 = "safe worktree filesystem + file tree + minimal embedded editor
+ open-in-external-editor", where the backend is fully mutation-verifiable and
the two UI surfaces (tree widget, editor widget) need James's eyes but are thin
over the backend.**

---

## §1 File tree

Important framing discovered in the source: Orca has **two distinct
file-listing subsystems**, and they must not be conflated.

### 1a. The File Explorer tree (the sidebar tree) — lazy per-directory readdir

This is the persistent worktree file browser, and it is what suaegi lacks
entirely today.

- **Backend = `fs:readDir` IPC**, one directory level at a time:
  `readdir(dirPath,{withFileTypes:true})` → `DirEntry[]` of
  `{name, isDirectory, isSymlink}`, sorted dirs-first then by name
  (`src/main/ipc/filesystem.ts:497-540`, esp. `:511-526`). Symlinks are reported
  but treated as non-directories (`:447-462`).
- **Lazy, on-demand tree** — NOT one flat full-tree list. `useFileExplorerTree.ts`
  holds a `dirCache` keyed by dir path; expanding a folder fires that folder's
  own `fs:readDir` (`useFileExplorerTree.ts:144-205`;
  `file-explorer-directory-listing.ts:40-60`). Stale responses are discarded via
  a per-dir load-token when the worktree switches or a watcher refresh races
  (`useFileExplorerTree.ts:148,254-256`).
- **Node shape**: `{name, path (absolute), relativePath, isDirectory, isSymlink,
  depth}`, where `relativePath = normalizeRelativePath(path.slice(worktreePath
  .length+1))` — relative to worktree root, forward-slash normalized
  (`file-explorer-directory-listing.ts:24-37`).
- **Ignore handling in the tree is *decoration, not filtering*.** The backend
  `fs:readDir` does **no** ignore filtering. The renderer hides only `.git` and
  `node_modules` (`file-explorer-entries.ts:3-5`); gitignored files are
  **shown but dimmed**, gated by a `showGitIgnoredFiles` toggle
  (`FileExplorer.tsx:192-194,653-654`). Ignore status comes from a separate IPC,
  **`git:checkIgnored`** → `git check-ignore --stdin`
  (`filesystem.ts:1148-1168`; `check-ignored-paths.ts:18-30`), debounced 300ms
  over the *visible* paths only (`use-file-explorer-ignored-paths.ts:63-88`).
  **Git is the ignore authority.**
- **Git-status decoration**: status comes from `git status` (working tree) via
  `git.status` runtime RPC, stored as `GitStatusEntry[]` keyed by worktree
  (`FileExplorer.tsx:102,267-268`; `runtime-git-client.ts:134-175`), collapsed
  into a `Map<relativePath, GitFileStatus>` (`buildStatusMap`,
  `FileExplorer.tsx:270`), with folder roll-up (`buildFolderStatusMap`).
  Refreshed on fs-watch events, debounced 125ms
  (`git-status-file-watch-refresh.ts:38-58,111-132`).
- **Tree mutations** (from context menu / drag): create file/dir, rename/move,
  copy/duplicate, delete, import — see §1c.

### 1b. Quick Open (Cmd+Shift+P fuzzy finder) — flat full list

A *separate* lister used only by the fuzzy file palette
(`quick-open-file-list.ts`). Not the tree. Worth porting eventually but a
distinct, lower-priority feature.

- **Primary = ripgrep** `rg --files --hidden` (+ a `--no-ignore-vcs` "ignored"
  pass), gated by `checkRgAvailable`
  (`filesystem-list-files.ts:43-52,64-73,224-234`;
  `quick-open-filter.ts:232-243`).
- **Fallback when rg missing = `git ls-files`**: primary pass `-z -s --cached
  --others --exclude-standard --directory --no-empty-directory` (tracked +
  untracked-not-ignored, directory-collapsed), plus an ignored pass with
  `--ignored` (`quick-open-filter.ts:322-339`). Collapsed directory placeholders
  are expanded on the filesystem afterwards
  (`filesystem-list-files-git-fallback.ts:117-119,292-299`; contract in
  `filesystem-list-files-git-directory-expansion.test.ts:49-80`).
- **Non-git fallback = raw fs walk**, hard-capped at
  `QUICK_OPEN_READDIR_MAX_FILES = 10_000` (`quick-open-readdir-budget.ts:1`;
  `filesystem-list-files-git-fallback.ts:88-95`).
- Returns a **flat `string[]` of relative paths** capped to `maxResults`
  (`filesystem-list-files.ts:19-25,239`). Each rg/git spawn has a **10s
  timeout** (`filesystem-list-files.ts:193`; `-git-fallback.ts:73,231`).
- Nested linked-worktree paths are excluded via caller-supplied `excludePaths`
  (`filesystem-list-files.ts:37,97`;
  `quick-open-file-list.ts:41-52` computes them).

### 1c. Tree/editor file mutations (main-side, all path-gated)

All in `src/main/ipc/filesystem-mutations.ts` via
`registerFilesystemMutationHandlers` (`:70`); each gated by
`resolveAuthorizedPath`:

| Operation | Channel | Line | Notes |
|---|---|---|---|
| Create file | `fs:createFile` | `:71-87` | atomic `writeFile('',{flag:'wx'})` — TOCTOU-free |
| Create dir | `fs:createDir` | `:89-100` | asserts not-exists |
| Rename / move | `fs:rename` | `:105-126` | no-clobber, `preserveSymlink` |
| Copy / duplicate | `fs:copy` | `:128-149` | `copyFile(..,COPYFILE_EXCL)`; renderer deconflicts name |
| Import external paths | `fs:importExternalPaths` | `:151-182` | drag-in |
| Delete | `fs:deletePath` | `filesystem.ts:833-863` | routes to `shell.trashItem` (OS trash), ENOENT swallowed |

There is **no dedicated "duplicate" IPC** — duplicate = `fs:copy` with a
renderer-computed name (`filesystem-mutations.ts:145-147`). Drag-move =
`fs:rename`.

---

## §2 Mapping to suaegi

### What suaegi already has (do not rebuild)

- `suaegi-git`: `GitRunner` async shell-out with output cap, process-tree kill,
  timeout, reap (`crates/suaegi-git/src/runner.rs`); merge-base diff in
  `compare.rs` — `ChangedFile`/`ChangeStatus`(Added/Modified/Deleted/Renamed/
  Copied/Other), `file_diff` reading `WorkingTree` or `Revision` bytes with an
  8192-byte binary sniff (`BINARY_SNIFF_BYTES`, `compare.rs:10`) and a size cap
  (`FileDiff::TooLarge`). It already understands the injected-settings-file
  exclusion invariant (`compare.rs:12-23`).
- `suaegi-app`: iced 0.14 shell — sidebar (`sidebar.rs`), `pane_grid` workbench
  (`workbench.rs`), diff panel (`diff_panel.rs`), terminal widget
  (`terminal/`).
- `suaegi-core`: domain + JSON persistence (atomic write + rolling backups).

**Overlap with Orca's editor/file-tree is narrow and specific:** suaegi's
**diff panel already is Orca's *Source Control changed-files tree*** — Orca's
`source-control-tree.ts` builds a nested tree from `GitStatusEntry[]`; suaegi
renders merge-base `ChangedFile`s in `diff_panel.rs`. **Do not rebuild that.**
What suaegi lacks is the **full worktree File Explorer** (all files, lazy
readdir, not just changed files) and any **file read/write** surface.

> One nuance for Codex: Orca's File Explorer decorates with `git status`
> (working-tree porcelain), which suaegi does **not** have — suaegi only has
> merge-base diff (`git diff <merge-base>`). These are different git operations.
> The tree's status column needs a new `git status --porcelain` reader, distinct
> from `compare.rs`.

### Where each piece lands (respecting layering — no reverse deps)

| Piece | Crate | Verifiability |
|---|---|---|
| **Directory listing** (readdir one level, `{name,is_dir,is_symlink}`, sorted) | `suaegi-git` or new `suaegi-fs` module | **Mutation-verifiable** (pure fs) |
| **Path safety** (worktree-root containment, `..` rejection, symlink realpath re-check, Windows absolute-rel case) | `suaegi-git`/`suaegi-fs` | **Mutation-verifiable** — highest priority |
| **Ignore filtering** (`git check-ignore --stdin`) | `suaegi-git` (new fn) | **Mutation-verifiable** |
| **Git-status decoration** (`git status --porcelain=v1 -z`) | `suaegi-git` (new fn) | **Mutation-verifiable** |
| **Safe file read** (size cap, binary sniff → text vs binary, worktree-relative) | `suaegi-git`/`suaegi-fs` | **Mutation-verifiable** |
| **Safe file write** (re-validate path main-side, write in place) | `suaegi-git`/`suaegi-fs` | **Mutation-verifiable** |
| **External-editor launcher** (command → argv/spawn spec) | `suaegi-app` or small helper crate | **Mutation-verifiable** (pure) |
| **Autosave/conflict controller** (debounce, self-write stamp, disk-signature rebaseline) | `suaegi-app` | Mostly mutation-verifiable (state machine) |
| **File tree widget** (lazy expand, virtual rows, keyboard nav, drag) | `suaegi-app` | **Human-eyes (pixels)** |
| **Editor widget** (iced `text_editor` wired to read/write) | `suaegi-app` | **Human-eyes (pixels)** |

Layering: the fs/list/ignore/status functions belong beside the existing git
shell-out in `suaegi-git` (or a sibling `suaegi-fs` if we want to keep pure-fs
separate from git-fs); `suaegi-app` depends on them, never the reverse.

### The "no global config write" invariant holds naturally

Everything here is per-worktree: readdir/read/write are scoped to the worktree
root; `git check-ignore`/`git status` run with `-C <worktree>`. The one risk is
the **external-editor command *preference*** — where the user's chosen editor
command is stored. In Orca it's a renderer setting passed into the IPC
(`shell.ts:144`). In suaegi it must live in the **per-worktree/app JSON store**,
never the user's global git/editor config. Flag for milestone M6.

---

## §3 Milestone breakdown (smallest-first, backend-before-UI)

Each milestone is independently mutation-verifiable unless marked **[eyes]**.

### M1 — Path-safety core (foundation, highest security value)
Pure functions in `suaegi-git`/`suaegi-fs`: given a worktree root + a
caller-supplied path, return an authorized absolute path or reject. Must
reproduce Orca's `filesystem-auth.ts` semantics:
- `is_descendant_or_equal(base, target)` using `relative()` — reject `""`-only,
  `..`, `../…`, and `isAbsolute(rel)` (Windows cross-drive escape)
  (`filesystem-auth.ts:134-142`).
- realpath-canonicalize-then-recheck for **existing** paths (symlink escape)
  (`:322-331`); for **missing** paths, walk to nearest existing ancestor,
  canonicalize, re-validate (`:340-369`); `preserveSymlink` variant for
  delete/rename (operate on the link, not its target) (`:299-318`).
- Reject null bytes (`:411,531`).
- **Crux/security risks**: path traversal (`../../etc/passwd`), symlink escape
  (in-worktree symlink → outside), TOCTOU between check and open, Windows
  namespaced/long paths and drive-relative paths. This milestone *is* the
  security boundary — mutation tests must kill traversal + symlink-escape
  mutants. **Fully autonomous.**

### M2 — Directory listing (`readdir` one level)
`list_dir(authorized_dir) -> Vec<Entry{name, is_dir, is_symlink}>`, sorted
dirs-first then name (`filesystem.ts:511-526`). Lazy per-dir (mirrors Orca's
tree, not the flat Quick-Open lister). Symlinks reported, never auto-followed
into as directories.
- **Risks**: symlink cycle if the UI ever recurses (M2 stays one-level so it
  can't); unreadable/permission-denied subdir must degrade gracefully;
  worktree-root escape (covered by M1). **Fully autonomous.**

### M3 — Ignore filtering + git-status decoration
Two new `suaegi-git` functions over `GitRunner`:
- `check_ignored(root, rel_paths) -> set` via `git check-ignore -z --stdin`
  (`check-ignored-paths.ts:18-30`). Git is the ignore authority.
- `working_tree_status(root) -> Map<rel, Status>` via
  `git status --porcelain=v1 -z` (distinct from merge-base `compare.rs`).
- Renderer-equivalent hardcoded hides: `.git`, `node_modules`
  (`file-explorer-entries.ts:3-5`).
- **Risks**: porcelain `-z` parsing (rename records are two NUL-separated
  paths — the exact class of bug `compare.rs:33-37` already documents for
  copy/rename; reuse that discipline); path-encoding of non-UTF8 names;
  check-ignore timeout. **Fully autonomous.** (Reuse `compare.rs` parsing
  lessons — regression tests must be mutation-verified per repo convention.)

### M4 — Safe file read
`read_file(authorized_path) -> {content|binary, size}`: enforce a max size
(Orca: 50 MB text, `filesystem.ts:130,565-569`), null-byte sniff first 8192
bytes → binary flag (`filesystem.ts:426-434`, matches suaegi's existing
`BINARY_SNIFF_BYTES`). Return worktree-relative identity for the UI.
- **Risks**: reading a huge file into memory (cap before buffering — Orca
  prefix-probes at `:583`); binary file corrupting the editor (sniff gate);
  reading a symlink target outside root (M1 realpath guard). **Fully
  autonomous.**

### M5 — Safe file write + autosave/conflict state machine
`write_file(authorized_path, content)`: re-validate path main-side (never trust
the renderer's absolute path — Orca re-validates every call), lstat-guard
against writing to a directory (`filesystem.ts:816-829`).
- **Data-loss surface — the crux of Plan 9.** Orca's model
  (`editor-autosave-controller.ts`): debounced write (default 1000ms, clamped
  250–10000, `constants.ts:105-107`), per-file promise queue + generation
  counter so stale saves drop (`:68-168`), a self-write registry stamped before
  each write (TTL 750ms local) so the fs-watcher's own echo doesn't reset the
  buffer (`editor-self-write-registry.ts:14-19`), and disk-signature rebaseline
  after each save (`:143-145`). External change while dirty → preserve draft,
  suspend autosave, show reload/keep/compare banner
  (`ExternalFileChangeBanner.tsx:156-168`).
- **Decision to surface to Codex**: Orca's editor write is **NOT atomic** —
  `writeFile` in place (`filesystem.ts:829`), only imports/downloads use
  temp+rename. suaegi's *own* persistence is atomic (temp+rename+backup). Should
  suaegi's editor write be atomic (temp-sibling + rename) for crash-safety, even
  though Orca isn't? Recommend **yes** — it's cheap and suaegi already has the
  pattern in `suaegi-core`.
- **Risks**: losing edits on external change (conflict banner), self-write echo
  clobbering the buffer, partial write on crash (atomic write mitigates),
  writing outside root (M1). State-machine parts autonomous; the banner UI is
  **[eyes]**.

### M6 — External-editor launcher (pure, clean win)
Port `resolveExternalEditorLaunchSpec` (`external-editor-launch.ts:158-188`) to
Rust: command + worktree path → spawn spec. Three branches:
1. direct executable path (absolute, has separator, exists-check) → executable
   spec (`:167-175`);
2. compound command (contains whitespace) → shell spec (`/bin/sh -c` or
   `cmd.exe /d /s /c`) with POSIX single-quote / Windows double-quote path
   escaping (`:25-40,136-156`);
3. else → resolve bare CLI command (default `code`) → executable (`:181-187`).
   Special cases: Cursor gets `--new-window` (`:117-120`); win32 VS Code + WSL
   UNC path gets `--remote wsl+<distro>` (`:122-128`); nvim/vim keep the Windows
   console visible (`:8,108`). Spawn detached, stdio ignored, unref
   (`shell.ts:64-71,92-95`).
- **Risks — argv injection.** A malicious editor-command preference or a path
  with shell metacharacters must not inject a command. The shell branch's
  escaping (`escapePosixPathForShell`, `:25-30`) is the guard — mutation tests
  must kill an unescaped-path mutant. Store the editor preference in suaegi's
  **per-worktree/app JSON**, never the user's global config (invariant).
  **Fully autonomous** (pure function; mirror Orca's `.test.ts` table).

### M7 — File tree widget **[eyes]**
iced widget: lazy-expand tree over M2 listing, git-status/ignore decoration from
M3, keyboard nav, context-menu mutations (§1c). Thin over the backend. Needs
James's eyes for pixels/interaction. Defer drag-drop, inline rename, virtual
scrolling of huge dirs to a follow-up.

### M8 — Minimal embedded editor widget **[eyes]**
iced `text_editor` wired to M4 read / M5 write / M6 open-in-external. Plain-text
editing + save; type-specific viewers (image/pdf/csv/ipynb) and syntax
highlighting are follow-ups. Needs James's eyes.

**Recommended cut for the first shippable Plan 9**: M1–M6 (all backend, all
autonomous, all mutation-verifiable) + M7 as the single UI surface, with M8
(editor) as an explicit "does it feel right" checkpoint with James before
investing. Open-in-external (M6) gives a usable "edit this file" story even
before M8 lands.

---

## §4 Open questions for Codex cross-validation

1. **New crate `suaegi-fs`, or fold into `suaegi-git`?** The listing/read/write
   are pure-fs (no git); ignore/status are git. Orca keeps them together in one
   `filesystem*.ts` family. Does suaegi's layering prefer a dedicated
   `suaegi-fs` (pure fs + path safety) that `suaegi-git` and `suaegi-app` both
   use, or is co-locating fs helpers in `suaegi-git` acceptable given they share
   `GitRunner`?

2. **Atomic editor write vs Orca parity.** Orca writes editor saves in place
   (non-atomic, `filesystem.ts:829`). suaegi has atomic temp+rename+backup in
   `suaegi-core`. Adopt atomic write for editor saves (recommended), or match
   Orca and stay simple? Any interaction with the fs-watcher self-write stamp if
   we rename?

3. **Path-safety: how far to go on symlinks?** Orca realpath-canonicalizes and
   re-checks for existing paths, and walks to the nearest existing ancestor for
   missing ones (`filesystem-auth.ts:340-369`). Is full parity required for M1,
   or is "reject `..` + reject absolute-rel + realpath-recheck existing paths"
   sufficient for the MVP, deferring the missing-path ancestor walk?

4. **Git-status source: `git status --porcelain` vs reuse merge-base
   `compare.rs`?** The File Explorer wants working-tree status (dirty/untracked
   vs HEAD), which is a different operation than suaegi's merge-base diff.
   Confirm we add a new `working_tree_status` reader rather than stretching
   `compare.rs`. Any risk of two git-status notions confusing the UI?

5. **Quick Open (§1b) — in Plan 9 or deferred?** It's a separate lister
   (rg → git ls-files → readdir fallback) feeding a fuzzy palette. Does Plan 9
   include it, or is it a distinct later plan? (It needs `ripgrep` detection and
   a fuzzy matcher — non-trivial.)

6. **Editor tech decision.** Confirm iced 0.14 `text_editor` (cosmic-text) is
   the intended widget and Monaco parity (syntax highlight, LSP, multi-cursor)
   is explicitly out of scope for Plan 9.

7. **fs-watcher scope.** Orca refreshes tree status on fs-watch events
   (debounced 125ms, `git-status-file-watch-refresh.ts`). Does Plan 9 include a
   file watcher, or manual-refresh only for the MVP (watcher = later)? A watcher
   is a substantial subsystem (`filesystem-watcher*.ts`, ~15 files).

---

## Defer list (Orca features too deep for parity now)

- **Monaco-equivalent editor**: syntax highlighting, LSP/IntelliSense,
  multi-cursor, undo-retention tuning, large-paste virtualization
  (`monaco-large-text-paste.ts`), context-menu paste. Use plain `text_editor`.
- **Rich markdown editor** (TipTap): `RichMarkdownEditor.tsx` + ~80 modules.
  Huge, WYSIWYG. Defer entirely; plain-text markdown only.
- **Type-specific viewers**: image/PDF/CSV/ipynb/mermaid renderers
  (`IpynbViewer.tsx` 34k, `PdfViewer.tsx`, `CsvViewer.tsx`). Defer; show binary
  as non-editable.
- **File watcher subsystem** (`filesystem-watcher*.ts`): native parcel-watcher,
  batching, WSL, remote cancellation. Start with manual refresh.
- **Quick Open fuzzy palette** (§1b): separate lister + matcher. Later plan.
- **Drag-drop / inline rename / multi-select / undo-redo** in the tree
  (`useFileExplorerDragDrop.ts`, `fileExplorerUndoRedo.ts`): follow-up polish.
- **SSH / remote runtime + WSL** file access (`runtime-file-client.ts` remote
  branch, `local-worktree-filesystem.ts` WSL paths): suaegi is local-only; skip
  all remote/WSL branches.
- **`shell.trashItem` (OS Recycle Bin) for delete**: nice-to-have; a plain
  remove (or a suaegi-managed trash) is acceptable for MVP — flag the UX
  difference.
