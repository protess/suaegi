# Orca Rust port status

Last audited: 2026-08-03

Reference: `reference/orca` at `7c716702` (2026-08-02).

## Completion target

The active target is feature parity with the desktop Orca app, implemented in
Rust, except for mobile companion pairing/relay. The earlier lightweight-MVP
scope is superseded. A settings row is not considered ported until its value is
persisted, its runtime behavior is wired, and the behavior has been exercised
by tests or live-app QA.

## Implemented and wired

- Repository registration, worktree create/list/remove, persistence and restore.
- Native PTY sessions, GPU terminal rendering, splits, focus, mouse, clipboard,
  IME commit handling and all 35 Orca agent launch/detection entries, including
  Trae CLI and Claude Agent Teams.
- Orca's Agents pane launch profiles are wired end-to-end: PATH refresh,
  enable/disable filtering, default selection, per-agent command/argument/env
  overrides, reserved-environment protection, and Yolo/Manual permission
  presets all persist and affect spawned PTYs. Claude Agent Teams supports both
  Orca modes: in-process injects the experimental environment and
  `--teammate-mode in-process` exactly once, while native panes install a
  private `tmux` shim and translate Claude's bounded tmux protocol into native
  Suaegi splits.
- Claude hooks plus process/title-based agent status badges.
- Diff panel and restored pane layouts.
- GitHub and GitLab review providers; PR/MR create, status, review/comments and
  confirm-gated merge. Integrations settings run the real `gh`/`glab`
  preflights and distinguish connected, missing, unauthenticated, and outdated
  CLI states, with install guidance and an explicit re-check action.
- GitHub Tasks uses numbered Search API pages instead of truncating every
  project at the first 100 items. Page counts are capped at GitHub's reachable
  1,000-result window, multi-project counts use the longest project, manual
  refreshes retain the active page, and query/scope changes reset to page one.
- Linear and Jira read/link UI, with secrets kept outside the JSON state.
- Safe filesystem backend: containment, symlink policy, directory listing,
  ignore/status queries, bounded reads, stale-write protection and external
  editor launch.
- Git history/show, staging, commit, discard, fetch/pull/push and branch reads.
- Backend crates for Quick Open, content search, automation schedules, browser
  URLs/screencast, MCP inspection and the smaller Orca protocol/normalizer ports.
- Orca-compatible editable `keybindings.json`, terminal shortcut policy, and
  live shortcut dispatch for the currently available Rust surfaces.
- Floating Workspace with repository-independent persistent PTYs, multiple
  selectable/closable terminal tabs, Claude/browser/Markdown launchers,
  configurable cwd, proxy/history/terminal preferences, button/status-bar
  triggers, attention state, and draggable/resizable geometry that survives
  relaunch.
- Orca terminal themes, scoped shell history, cursor blinking, OSC 52 clipboard,
  macOS Option-as-Alt/JIS yen handling, bell and agent-completion notifications.
- macOS terminal typography resolves Orca's `SF Mono` CSS alias to the native
  `.SF NS Mono` family, uses Orca's 500 default weight, and migrates the former
  400 default once without overwriting a user-selected weight.
- Extended Orca terminal controls now affect the native renderer and input
  path: horizontal/vertical grid padding, normal/fast/TUI wheel multipliers,
  cursor/background and focused/unfocused pane opacity, divider thickness,
  hide-pointer-while-typing, and semantic-selection word separators.
- Native macOS keep-awake assertion while agents are working or waiting.
- Voice dictation follows Orca's microphone preference model: Settings lists
  live input devices alongside the system default, persists a stable id and
  cached label, heals a rotated id through a unique label match, retains an
  unplugged preference as unavailable, and falls back to the system default
  without losing that preference. Device discovery and the native picker were
  exercised in the live macOS app.
- Orca-style confirmation before an active coding-agent terminal is stopped,
  independently configurable from pinned-tab confirmation.
- Orchestration and Computer Use settings now detect installed global skills,
  distinguish install from update, and run Orca's exact skill commands in a
  real PTY instead of exposing non-Orca enable switches.
- Native in-app browser on macOS using a child WKWebView, with Orca-compatible
  URL/search normalization, workspace sidebar geometry, address synchronization,
  back/forward/reload, default zoom, external handoff, window-resize handling,
  and `Open links in Suaegi` routing.
- Browser tabs own independent WKWebViews, DOM/history and profile stores.
  Switching tabs preserves live page state, the bounded tab strip remains usable
  with many tabs, async JavaScript awaits promises, and downloads use an exact
  app-owned destination with completion reporting instead of blocking the macOS
  main run loop on protected Downloads-folder access. Download navigations
  tolerate a temporarily nil WebKit URL instead of crashing. Agent browser
  commands also cover viewport/device/media emulation, bounded fetch
  interception, explicit local-file upload, and click-triggered download.
- Browser session profiles can be created, selected, removed, and persisted.
  Non-default profiles use deterministic WKWebView data-store identifiers for
  isolated cookies/login state; active session data can be cleared in Settings,
  Netscape-format cookie files can be imported; macOS Chrome, Edge, Brave, Arc,
  and Comet profiles are detected and decrypted through their Keychain Safe
  Storage keys; Firefox SQLite profiles and Safari binary-cookie stores are
  imported directly. Removing an isolated profile drops its WebView and deletes
  the corresponding WKWebsiteDataStore, so cookies/cache are not orphaned.
  Cookie values never surface through state/debug output.
- Remote runtime pairing metadata is persisted without secrets, credentials
  stay in the OS keychain, and server selection performs Orca-compatible NaCl
  E2EE hello/authentication plus an encrypted `status.get` before changing the
  active host.
- Paired servers now expose remote Claude/Codex account rosters and usage
  windows through the encrypted runtime RPC. Supported headless installs can
  also be checked, downloaded, installed, and verified after reconnect through
  Orca's `updater.remote-control.v1` flow.
- Local usage analytics are opt-in per Claude, Codex, and OpenCode provider and
  scan local JSONL/SQLite history into aggregate token, cache, session, project,
  model, and active-day summaries. Claude and Codex events use Orca's
  model-normalization and API-equivalent pricing tables, including long-context
  tiers; OpenCode keeps its recorded cost.
- The embedded editor supports shortcut-driven find, previous/next with wrap,
  replace-one and replace-all in addition to safe load/save and conflict
  handling. Editor minimap rendering and native Markdown preview/review links
  are wired to their settings; preview links honor the in-app browser routing
  preference. Multiple editor documents remain live at once: opening another
  file preserves dirty buffers and in-flight saves, tab selection restores the
  exact document, close confirmation applies only to the selected dirty tab,
  and worktree removal retires every tab owned by that worktree. Agent
  terminals and editor documents now share the workspace tab strip, so opening
  a file from Explorer adds a sibling tab instead of replacing the Claude
  surface; switching either way preserves both the PTY and editor buffers.
- File editors and diff views share Orca's explicit editor-font preference,
  including the empty-value fallback to the configured terminal font.
- `orca.yaml worktree.sharedDirectories` is normalized with Orca's path safety
  rules. Only existing gitignored directories are linked, shared directories
  remain symlinks even on APFS, and known links are safely removed before
  worktree deletion without touching regular files.
- The experimental plugin master switch, v1 manifest discovery, content-hash
  install layout, immutable `current` pointer, engine/capability/contribution
  bounds, artifact containment, symlink-escape rejection, development paths,
  enablement, and review-screen fingerprinted consent are implemented. Local
  and HTTPS/SSH Git `#ref` installation, current-plus-previous retention,
  integrity-checked rollback, confirmed removal of install/data directories,
  and declarative built-in command aliases are wired. Content-pack manifests
  are strictly validated and included in consent fingerprints. The bounded
  plugin-private storage/settings stores, OS-keychain secret store and exact
  host-method capability gate are implemented. Git-backed marketplace catalogs
  now use Orca's bounded source/snapshot schemas, managed official source,
  system-Git credential path, exact commit cache, stale-cache fallback,
  official/reserved-identity provenance rules, unsupported-category filtering,
  source management UI, reviewed marketplace-commit guard, and manifest/listing
  identity check before immutable installation. Orca's bounded remote plugin
  safety list is cached atomically with future-clock and older-snapshot
  defenses; a revoked identity cannot be installed, consented, enabled, or
  invoked. Approved declarative plugin commands now publish their reviewed
  keyboard shortcuts into the app-focus resolver ahead of built-in defaults;
  overlapping cross-plugin chords fail closed for both owners. Worker-backed
  commands now run out of process through Orca's default-export `activate` and
  command-registration contract with a scrubbed environment, bounded JSON-line
  protocol/timeouts, pre-spawn installed-content verification, and Rust-side
  capability re-gating for storage/settings/secrets calls. Persistent lazy
  worker supervision/restarts/events and app-context host methods are wired.
  Approved panel contributions render in native child WKWebViews with
  post-parse content-integrity checks, Orca's restrictive CSP and design-token
  shell, navigation/form suppression, the exact three-action host bridge,
  per-plugin 64 KiB and 30-request/10-second admission budgets, revocation on
  plugin revision changes, and a 10-second ping/5-second pong health watchdog.
  Language-pack JSON and VM recipe artifacts are applied after activation with
  Orca's byte/depth/entry/command bounds,
  protected security-copy namespace, paired suspend/resume rule and `destroy:
  none` semantics; the consent review displays contributed shortcuts and exact
  bounded VM commands instead of trusting filenames alone.
- The optional Claude prompt-cache timer follows Orca's hook lifecycle: it
  starts on a working-to-done transition, renders a live sidebar countdown,
  and clears on resumed work, session reset/close, expiry, or setting disable.
- Optional first-prompt tab titles use Orca's bounded 512-character scan and
  40-character output rules, put PR/MR/issue identifiers first, remain stable
  after later prompts, and never expose prompt text through diagnostic output.
- New workspaces use Orca's global marine-creature suggestions when a name is
  omitted. Suaegi-owned creature branches are renamed from the first work
  prompt with the configured prefix, collision suffix, upstream and
  current-branch safety checks; imported or deliberately named branches are
  never changed.
- Rich Markdown spellcheck uses the macOS system dictionary while excluding
  fenced/inline code and URLs. The tab-order preference now switches between
  stable and most-recently-used order, and programming-ligature preferences
  select the terminal's basic or advanced shaping path.
- Localhost workspace labels use Orca-compatible `*.orca.localhost` names and a
  loopback HTTP/WebSocket proxy that rewrites the upstream Host header. Port
  rows can open either the labeled route or the original localhost listener.
- Gemini CLI OAuth is opt-in and now reads the same Gemini/OpenCode credential
  sources as Orca, refreshes expired tokens from the installed Gemini CLI
  bundle, loads the Code Assist project, and renders deduplicated model quota
  buckets with reset time and used/remaining percentage modes.
- Experimental Activity, terminal attention and native chat are connected to
  live agent sessions: lifecycle events populate a bounded feed, background
  completions/bells remain highlighted until focus, and a terminal tab can
  switch to a composer that sends through the same PTY. Supported Claude,
  OpenClaude, Codex and Grok tabs also honor Orca's Terminal-chat/Chat-UI
  default-view preference. The pet toggle renders a dismissible dolphin
  companion whose mood follows aggregate agent state.
- Compact workspace cards now alter the actual sidebar density instead of only
  persisting a toggle. On macOS the menu-bar dolphin item and native
  behind-window vibrancy are also created/removed live by their Appearance
  settings.
- GitHub attribution now applies both to Suaegi's source-control commit action
  and to `git commit` / non-interactive `gh pr create` / `gh issue create`
  inside Suaegi-managed terminals. Private executable shims are scoped to each
  spawned PTY and never mutate the user's global Git configuration.
- Experimental agent hibernation now uses the Claude hook's provider session
  identity, Orca's 1-minute-to-24-hour idle window, completed/background
  eligibility and draft-input guard. The PTY is stopped only after the
  asynchronous kill succeeds, its pane and a content-free resume record remain
  persisted, and wake uses `claude --resume <session-id>` in the same pane.
- iOS/Android device discovery and native simulator launch, plus system OpenSSH
  target import, advanced proxy/jump/reuse settings, connection tests, and
  interactive connect are available from Settings.
- Local privacy diagnostics review files that exclude terminal output, files,
  prompts, secrets, proxy URLs, environment variables, and repository paths.
- Runtime UI language selection loads Orca's shipped English, Chinese, Korean,
  Japanese, and Spanish catalogs (with system-locale detection and Suaegi
  rebranding) across the shared settings/workbench/sidebar surfaces.
- The Explorer periodically reconciles every expanded directory and Git status
  without collapsing the tree, covering external create/rename/delete changes
  for local, SSH and paired-runtime workspaces. Clean editor documents use the
  same bounded disk-signature model to auto-reload external changes; dirty
  documents are never overwritten.
- CLI runtime-environment `add/list/show/rm`, agent-hook `status/on/off`,
  `file open-changed --mode edit|diff|both`, positional file-open paths, and
  machine-readable `agent-context` commands are implemented. The bundled
  reference skill catalog is available through `skills list/get`; live and
  offline resource snapshots are available through `diagnostics memory`; VM
  recipes have a non-destructive `vm recipe doctor` path. Durable project and
  host-setup `list/setups/setup-create/setup-update/setup-delete`,
  existing-folder registration, and local clone commands share the same
  persisted model as Project settings. Browser CLI parity now includes
  viewport/device emulation, media preferences, and bounded fetch interception
  in addition to navigation, DOM interaction, profiles, cookies, capture,
  storage, screenshots and PDF. Running-app
  mutations route through authenticated loopback RPC; offline mutations update
  the durable store and Keychain without exposing pairing credentials.
- The browser automation surface now includes `dialog accept [--text]` and
  `dialog dismiss`. A retained `WKUIDelegate` owns the WebKit completion block
  for `alert`, `confirm`, and `prompt`, so JavaScript remains suspended until
  the matching CLI command resolves it. Confirm and prompt accept paths were
  exercised against a live WKWebView, including a blocked `click` request that
  resumed only after the native dialog completion.
- Linear commands now cover issue create/update/delete, comments, relations,
  labels, projects, teams, members, cycles, initiatives, documents, views,
  notifications, and attachment metadata using the same bounded authenticated
  client and durable workspace selection as the UI. Agent issue context also
  matches Orca's deep `--full` surface: independent comments, recursive
  children, attachments, bidirectional relations, and normalized activity
  sections walk Linear's 50-node cursors to their documented caps. A failed
  optional section produces a partial result instead of discarding successful
  sections, and Markdown/HTML inline media is extracted from full comment
  bodies before bounded text truncation.
- The macOS Computer Use bridge exposes the complete bounded screenshot,
  accessibility-tree, window/application, pointer, keyboard, clipboard, scroll,
  wait, and focus command family. Read-only paths were exercised against the
  running app; mutating commands retain the permission and confirmation gates.
- Orchestration commands now expose the full Orca command-name surface and use
  the persisted worker/mailbox/coordinator model. Retired coordinator commands
  report their migration path rather than silently mutating legacy state.
- Federated orchestration supports `worker-start --on` against paired
  Suaegi/Orca runtimes, exact or new-top-level remote placement, durable
  mutation IDs, setup/agent readiness, attachment show/read/stop, process-bound
  output, and protocol-v2 control mail. Worker completion, heartbeat/status
  mail, coordinator control messages, and blocking question/reply flow use
  contiguous pull/import/ack relay sequences and update the home Task and
  Dispatch lifecycle idempotently. The desktop starts a bounded two-second
  background federation relay, while explicit inbox/show/read operations also
  synchronize immediately.
- `serve` provides Orca v2 runtime pairing codes, the legacy NaCl E2EE
  hello/ready/authenticated handshake, bounded encrypted RPC forwarding to a
  running desktop authority, no-pairing readiness, and recipe JSON output.
  Headless `status.get`, repository/worktree discovery, safe worktree
  create/remove, bounded file stat/list/read/write/search, Git
  status/stage/unstage/discard/commit/fetch/pull/push/compare/diff, and detached
  PTY list/show/create/read/send/resize/wait/close/subscribe work without the
  desktop app. `--project-root` is the active headless authority root instead
  of recipe-only metadata. The encrypted terminal subscription supports
  concurrent input and end-of-process delivery. Worktree create now coalesces
  concurrent mutations and persists bounded success receipts for
  lost-response replay. Terminal create derives Orca's v2 caller/worktree/
  mutation identity into a stable daemon handle, and both idempotency
  capabilities are advertised only with their implementations active.
- Every Orca Settings route opens in the running app. A static reducer audit
  found handlers for all 183 settings messages, and representative UI toggles
  were verified to update and restore the durable JSON setting. Account,
  credential, and destructive external-integration actions were not mutated as
  part of this non-destructive pass.

## Completed implementation record

1. **Plan 9 M7/M8:** file explorer and multi-document embedded editor are
   implemented in the current worktree. The editor uses bounded UTF-8 reads,
   atomic stale-checked saves and a horizontally scrollable tab strip; dirty
   close and external-change conflicts are explicit.
2. **Quick Open and content search UI:** implemented over the existing bounded
   lister/fuzzy scorer and ripgrep/git-grep backend.
3. **Source-control write UI:** implemented. Stage, unstage, commit, fetch,
   fast-forward-only pull and non-force push refresh the detailed porcelain
   status. Discard requires an explicit confirmation.
4. **Automation runtime/UI:** implemented for persisted scheduled prompts.
   Schedules use an explicit IANA timezone, a 30-second runtime tick, latest-only
   missed-run catch-up, pause/run-now/delete controls and an idle-agent dispatch
   gate.
5. **PTY survival daemon:** implemented using Plan 8 option (b). A detached,
   authenticated session-holder owns the PTY while the app retains the
   alacritty grid. Stable worktree-derived session IDs support warm reattach;
   an 8 MiB bounded raw-output tail is replayed before live output. Explicit
   pane close kills and removes the hosted session, while application shutdown
   only disconnects it.
6. **Daemon hardening:** the POSIX runtime uses a versioned Unix socket, private
   token/runtime-directory permissions, bounded protocol frames and subscriber
   queues, a launch lock with stale-owner recovery, PID/start-time/launch-nonce
   identity, fixed-socket single-instance adoption and nonce-guarded cleanup.
   Historical terminal queries are replayed into the grid without being
   answered a second time.

Mobile companion pairing/relay is the only product feature excluded by the
current target. Remote repository/file/PTY
routing after the E2EE control handshake, SSH-backed filesystem/worktree/PTY
routing, embedded emulator display/control panes, persistent plugin workers,
language packs, VM recipes, and sandboxed plugin panels are wired. The current
CLI inventory contains all 216 reference command paths (plus four
Suaegi-specific convenience paths), but command-name coverage is not treated as
behavioral completion. Local orchestration worker starts support exact,
`current`, `new-child`, and `new-top-level` placement, setup gating, agent
readiness, durable dispatch records, and exact effect receipts. Claude's tmux
compatibility path now covers global flags, option/display queries, native
split and two-phase respawn, main-vertical placement, pane listing, key send,
capture, focus/last-pane, and safe teammate close. A live run exercised
split → respawn → send-keys → capture against the running app. Orca runtime RPC
families whose only consumers are the paired mobile/web clients are outside the
explicit mobile-connection exclusion; desktop and headless CLI application
paths remain implemented rather than counted complete by method name alone.

## Verification baseline

- `cargo test -p suaegi-app --lib`: 733 passed, 2 ignored after the current
  UI/runtime, settings-route, integration-preflight, Floating Workspace,
  browser dialog, orchestration, Linear, and daemon loop.
- `cargo test --workspace`: passing after the final native Claude Agent Teams,
  browser-dialog, Linear deep-context, and headless runtime loop; all unit,
  integration, and documentation tests completed.
- `cargo test -p suaegi-term --lib`: 286 tests passing, including authenticated
  PTY I/O, warm replay and duplicate terminal-query suppression.
- `cargo test -p suaegi-term --test daemon_survival_test`: verifies the daemon
  is a detached session leader and the PTY accepts input after disconnect and
  reattach.
- `cargo test -p suaegi-app --lib`: 733 tests passing with two opt-in live
  credential/network probes ignored, including the Floating
  Workspace lifecycle, multi-document editor tabs, browser
  URL/bounds/cookie/profile-store handling, remote pairing, usage, SSH,
  emulator, voice, encrypted runtime server, and current settings/runtime
  wiring. The native browser was also exercised live through navigation,
  link-following, history, profile isolation/removal, resize, close/reopen, and
  dialog accept/prompt/dismiss flows.
- `cargo test -p suaegi-tracker`: 137 tests passing for Linear/Jira command,
  client, model, and persistence paths.
- Explorer/editor/search/source-control/automation reducer tests cover stale
  result rejection, external-edit conflict, destructive confirmation and
  once-per-occurrence dispatch.
- `cargo clippy -p suaegi-term --all-targets -- -D warnings`: passing.
- `cargo clippy -p suaegi-app --all-targets -- -D warnings`: passing on Rust
  1.94 after the current settings/runtime loop.
- `cargo clippy -p suaegi-tracker --all-targets -- -D warnings`: passing.
- `cargo clippy --workspace --all-targets -- -D warnings`: passing.
