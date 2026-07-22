# Plan 6 research — agent registry expansion

Research date: 2026-07-22. Orca source pinned at `release: v1.4.150-rc.0`
(`git log` in the clone), path
`/private/tmp/claude-501/-Users-james-projects-james-suaegi/cae3d45a-29c1-4dde-b3a0-4b77b7b8a4ad/scratchpad/orca-src`.
Every factual claim about Orca cites `file:line` in that clone. suaegi
citations are against the working tree at `/Users/james/projects/james/suaegi`.

suaegi target shape read first:
`crates/suaegi-term/src/agent.rs:22-49` — `AgentDef { kind, launch_program,
launch_args, process_names, prompt_injection }`, table `AGENT_DEFS` with only
`Claude` + `Codex`; `PromptInjection::{Argv,None}` (`agent.rs:15-20`);
`match_agent`/`segment_matches`/`basename_matches`/`build_spawn`
(`agent.rs:71-177`). suaegi-side hook/settings injection lives in
`crates/suaegi-app/src/agent_status/inject.rs` (per-worktree
`.claude/settings.local.json`, hook script, `spawn_env`) and the status
contract in `crates/suaegi-app/src/agent_status/mod.rs`.

---

## §0 Central registry finding — THE backbone

**Orca has exactly one declarative agent table, and it is the spine of the
whole feature.**

`src/shared/tui-agent-config.ts:46` — `export const TUI_AGENT_CONFIG:
Record<TuiAgent, TuiAgentConfig>`. It defines **34 agents** in one object
literal (lines 47–295). The row type is `TuiAgentConfig`
(`tui-agent-config.ts:19-44`):

```ts
type TuiAgentConfig = {
  detectCmd: string                                   // PATH binary that proves installed
  detectCmdAliases?: readonly string[]                // extra PATH names → same agent
  detectRequiredCommands?: readonly string[]          // AND-gate: these must also be present
  detectUnsupportedRuntimes?: readonly (NodeJS.Platform|'wsl')[]
  launchCmd: string                                   // command string (may carry fixed args)
  launchCmdByPlatform?: Partial<Record<NodeJS.Platform,string>>
  expectedProcess: string                             // process-table name for detection
  promptInjectionMode: AgentPromptInjectionMode        // 6 variants (see below)
  argvPromptSeparator?: '--'                          // option terminator before positional prompt
  draftPromptFlag?: string                            // native "seed input, don't submit" flag
  draftPromptEnvVar?: string                          // env-var equivalent for agents w/o a flag
  preflightTrust?: 'cursor'|'copilot'|'codex'         // pre-write trust artifact
  draftPasteReadySignal?: DraftPasteReadySignal        // renderer paste-timing signal
  windowsShiftEnterEncoding?: 'csi-u'                 // Win newline-key encoding override
}
```

`promptInjectionMode` union (`tui-agent-config.ts:4-10`): `'argv' |
'flag-prompt' | 'flag-prompt-interactive' | 'flag-interactive' |
'hermes-query' | 'stdin-after-start'`. **suaegi's `PromptInjection` has only
`Argv | None` — this union is the single largest gap.**

The table feeds four consumers, all derived, none hand-maintained:
- **Prompt injection separator** `argvPromptSeparator: '--'`
  (`tui-agent-config.ts:33`) — this is exactly suaegi's `--` push in
  `build_spawn` (`agent.rs:92-93`), but in Orca it is **per-agent opt-in**
  (only grok sets it, `tui-agent-config.ts:287`), not applied to every argv
  agent as suaegi does.
- **Process→agent detection map** is built by iterating the table:
  `src/shared/agent-process-recognition.ts:60-80` folds each row's
  `expectedProcess` + `detectCmd(+aliases)` + first token of `launchCmd` into
  `PROCESS_TO_AGENT`.
- **Install detection** iterates the table:
  `src/main/ipc/tui-agent-detection-commands.ts:18-24` →
  `KNOWN_TUI_AGENT_DETECTION_COMMANDS`.
- **Telemetry kind / display name** parallel records
  (`src/shared/agent-detection.ts:16-51`,
  `src/shared/tui-agent-display-names.ts:8-43`) are kept exhaustive by
  `satisfies Record<TuiAgent, …>`.

`TuiAgent` union (the closed agent id set): `src/shared/types.ts:2442-2476`
(34 members). Canonical enumerable list: `ALL_TUI_AGENTS`
(`tui-agent-display-names.ts:47`).

The launch-command resolver `getTuiAgentLaunchCommand`
(`tui-agent-config.ts:306-316`) returns `launchCmdByPlatform[platform] ??
launchCmd` — i.e. **`launchCmd` is a whole command string that Orca tokenizes,
not a program+args split.** Rows like `command-code --trust`
(`tui-agent-config.ts:203`), `kiro-cli chat --tui` (`:170`), `hermes --tui`
(`:260`), `orca claude-teams` (`:63`) carry fixed args inside the string. This
maps onto suaegi's `launch_program` + `launch_args` split, but suaegi must
decide the program/args boundary the way Orca's tokenizer does.

---

## §1 Per-agent table

Fields: **L** = launch command + fixed args, **D** = detect binary / expected
process, **P** = prompt injection, **C** = config/hook injection, **S** = status
detection beyond process polling, **I** = install/binary resolution, **X** =
does-not-fit flags. Prioritized coding-agent CLIs; every one in
`TUI_AGENT_CONFIG` is listed (34). Unless noted, **I** is identical for all
agents (§ shared note) and **S** for hookless agents is OSC-title only.

Shared **I** (install/binary resolution), for every row:
`src/main/ipc/preflight.ts:97-120` — probe `detectCmd`(+aliases +
`requiredCommands`) with `isCommandOnPath(cmd)` (`preflight.ts:104`), then a
known-user-install-dir fallback `detectCommandsInInstallDirs`
(`preflight.ts:110`), gated by `detectUnsupportedRuntimes`
(`tui-agent-detection-commands.ts:72-77`). No version pinning anywhere in the
table. suaegi currently does **no** install detection at all.

Shared **S** default: Orca runs a universal OSC-terminal-title status machine
for *every* agent — `AgentDetector.onData` parses the last OSC title and calls
`detectAgentStatusFromTitle` (`src/main/stats/agent-detector.ts:136-175`,
`detectAgentStatusFromTitle` re-exported from
`src/shared/agent-detection.ts:23-28`). Hooks (where present) are the *precise*
signal; the OSC title machine is the universal backstop. **suaegi has neither
an OSC-title status path nor hooks for anything but Claude.**

| agent (id) | L (launchCmd @ line) | D detectCmd / expectedProcess | P promptInjectionMode | C config/hook injection | X does-not-fit flags |
|---|---|---|---|---|---|
| **claude** | `claude` `:49` | `claude` / `claude` `:48,50` | `argv` `:51`; `draftPromptFlag: --prefill` `:53` | GLOBAL `~/.claude/settings.json` `src/main/claude/hook-settings.ts:65`; events list `hook-settings.ts:30-62` | draftPromptFlag (prefill) unsupported by suaegi |
| **claude-agent-teams** | `orca claude-teams` `:63`, platform overrides `:64-67` | `orca`(+aliases `orca-dev`,`orca-ide`) req `claude` / `claude` `:57-68` | `stdin-after-start` `:69` | uses claude's hooks; wrapper | wrapper agent; `detectRequiredCommands`, `detectCmdAliases`, `detectUnsupportedRuntimes ['win32','wsl']`, `launchCmdByPlatform`, stdin injection |
| **openclaude** | `openclaude` `:73` | `openclaude`/`openclaude` `:72,74` | `argv` `:75`; `--prefill` `:76` | GLOBAL `~/.openclaude/settings.json` (`CLAUDE_HOOK_SETTINGS` variant `hook-settings.ts:25-28`) | draftPromptFlag |
| **codex** | `codex` `:80` | `codex`/`codex` `:79,81` | `argv` `:82`; `preflightTrust: codex` `:83`; `draftPasteReadySignal: codex-composer-prompt` `:84` | GLOBAL `~/.codex/config.toml` + `~/.codex/hooks.json` `src/main/codex/hook-service.ts:98,107`; Orca-managed `CODEX_HOME` mirror `codex-config-mirror.ts:21-45` | preflightTrust, TOML config, config-mirror, draftPasteReadySignal |
| **autohand** | `autohand` `:88` | `autohand`/`autohand` `:87,89` | `stdin-after-start` `:90` | none found | stdin injection |
| **ante** | `ante` `:94` | `ante`/`ante` `:93,95` | `stdin-after-start` `:97` (Why: `--prompt` is headless one-shot `:96`) | none | stdin injection; headless-flag caveat |
| **opencode** | `opencode` `:100` | `opencode`/`opencode` `:99,101` | `flag-prompt` `:103`; `draftPasteReadySignal: render-cursor-after-bracketed-paste` `:105` | `src/main/opencode/hook-service.ts` exists (no home-config path grep hit — uses different mechanism) | flag-prompt, paste-ready signal |
| **mimo-code** | `mimo` `:109` | `mimo`/`mimo` `:108,110` | `flag-prompt` `:111`; paste signal `:113` | `src/main/mimo/hook-service.ts` exists | id≠binary (`mimo-code`→`mimo`), flag-prompt |
| **pi** | `pi` `:116` | `pi`/`pi` `:115,117` | `argv` `:119`; `draftPromptEnvVar: ORCA_PI_PREFILL` `:121` | none | draftPromptEnvVar (no --prefill flag) |
| **omp** | `omp` `:124` | `omp`/`omp` `:123,125` | `argv` `:127`; `draftPromptEnvVar: ORCA_OMP_PREFILL` `:128` | none | draftPromptEnvVar |
| **gemini** | `gemini` `:131` | `gemini`/`gemini` `:130,132` | `flag-prompt-interactive` `:134` | GLOBAL `~/.gemini/settings.json` `src/main/gemini/hook-service.ts:35` | flag-prompt-interactive; node-package entrypoint (see §3) |
| **antigravity** | `agy` `:138` | `agy`/`agy` `:137,139` | `flag-prompt-interactive` `:140` | GLOBAL `~/.gemini/config/hooks.json` `src/main/antigravity/hook-service.ts:69` | id≠binary (`antigravity`→`agy`); shares `~/.gemini` tree with gemini |
| **aider** | `aider` `:143` | `aider`/`aider` `:142,144` | `stdin-after-start` `:146` | none | python-based CLI (see §3), stdin |
| **goose** | `goose` `:149` | `goose`/`goose` `:148,150` | `stdin-after-start` `:152` | none | stdin |
| **amp** | `amp` `:155` | `amp`/`amp` `:154,156` | `stdin-after-start` `:158` | `src/main/amp/hook-service.ts` exists (no home path grep hit) | stdin |
| **kilo** | `kilo` `:161` | `kilo`/`kilo` `:160,162` | `stdin-after-start` `:164` | none | stdin |
| **kiro** | `kiro-cli chat --tui` `:170` | `kiro-cli`/`kiro-cli` `:168,171` | `stdin-after-start` `:172` | none | id≠binary (`kiro`→`kiro-cli`); **fixed args on a subcommand** (`chat --tui`) |
| **crush** | `crush` `:175` | `crush`/`crush` `:174,176` | `stdin-after-start` `:178` | none | stdin |
| **aug** | `auggie` `:183` | `auggie`/`auggie` `:182,184` | `stdin-after-start` `:185` | none | id≠binary (`aug`→`auggie`) |
| **cline** | `cline` `:188` | `cline`/`cline` `:187,189` | `stdin-after-start` `:191` | none | stdin |
| **codebuff** | `codebuff` `:194` | `codebuff`/`codebuff` `:193,195` | `stdin-after-start` `:197` | none | stdin |
| **command-code** | `command-code --trust` `:203` | `command-code`/`command-code` `:200,204` | `argv` `:205` | GLOBAL `~/.commandcode/settings.json` `src/main/command-code/hook-service.ts:37` | fixed arg `--trust` (skips trust prompt via flag, not preflight file) |
| **continue** | `cn` `:210` | `cn`/`cn` `:209,211` | `stdin-after-start` `:212` | none | id≠binary (`continue`→`cn`, avoids shell builtin) |
| **cursor** | `cursor-agent` `:216` | `cursor-agent`/`cursor-agent` `:215,217` | `argv` `:218`; `preflightTrust: cursor` `:219` | GLOBAL `~/.cursor/hooks.json` `src/main/cursor/hook-service.ts:43`; trust file `~/.cursor/projects/<slug>/.workspace-trusted` `agent-trust-presets.ts:39-57` | id≠binary; preflightTrust (writes trust artifact) |
| **droid** | `droid` `:223` | `droid`/`droid` `:222,224` | `argv` `:226`; `windowsShiftEnterEncoding: csi-u` `:228` | GLOBAL `~/.factory/settings.json` `src/main/droid/hook-service.ts:59` | windowsShiftEnterEncoding |
| **kimi** | `kimi` `:231` | `kimi`/`kimi` `:230,232` | `stdin-after-start` `:234` | GLOBAL `$KIMI_CODE_HOME` or `~/.kimi-code/config.toml` `src/main/kimi/hook-service.ts:38,42` | TOML config, env-overridable home |
| **mistral-vibe** | `vibe` `:240` | `vibe`(+alias `mistral-vibe`)/`vibe` `:238-241` | `stdin-after-start` `:242` | none | id≠binary; `detectCmdAliases` |
| **qwen-code** | `qwen` `:247` | `qwen`/`qwen` `:246,248` | `stdin-after-start` `:249` | none | id≠binary (`qwen-code`→`qwen`) |
| **rovo** | `rovo` `:252` | `rovo`/`rovo` `:251,253` | `stdin-after-start` `:255` | none | stdin |
| **hermes** | `hermes --tui` `:260` | `hermes`/`hermes` `:258,261` | `hermes-query` `:263` (startup-query contract) | `src/main/hermes/hook-service.ts` exists | fixed arg `--tui`; **`hermes-query` is a bespoke injection mode** |
| **openclaw** | `openclaw` `:267` | `openclaw`/`openclaw` `:266,268` | `stdin-after-start` `:269` | none | stdin |
| **copilot** | `copilot` `:273` | `copilot`/`copilot` `:272,274` | `flag-interactive` `:276` (Why: `--prompt` exits `:275`); `preflightTrust: copilot` `:278` | trust file `~/.copilot/config.json` `trustedFolders[]` `agent-trust-presets.ts:69-101`; `src/main/copilot/hook-service.ts` exists | flag-interactive, preflightTrust |
| **grok** | `grok` `:281` | `grok`/`grok` `:280,282` | `argv` `:285`; `argvPromptSeparator: --` `:287` | `src/main/grok/hook-service.ts` exists | **the only agent that opts into `--` separator**; packaged binary `grok-*` (see §3) |
| **devin** | `devin` `:291` | `devin`/`devin` `:290,292` | `stdin-after-start` `:294` (Why: `devin -- <prompt>` auto-submits `:293`) | GLOBAL `~/.devin/...settings.json` `src/main/devin/hook-settings.ts` / `hook-config-json.ts` | stdin; auto-submit caveat |

Notes on **C**: 16 agents ship a `hook-service.ts`
(`find src/main -name hook-service.ts`): claude, codex, cursor, copilot,
gemini, grok, droid, devin, kimi, mimo, amp, antigravity, opencode, openclaude,
command-code, hermes. **Every hook config is written to a GLOBAL home
directory** (`~/.claude`, `~/.codex`, `~/.cursor`, `~/.gemini`, `~/.factory`,
`~/.kimi-code`, `~/.commandcode`, `~/.devin`), **not per-worktree.** This is the
opposite of suaegi's deliberate design (`inject.rs:1-15`: "사용자의 Claude
설정을 건드리지 않는다 … suaegi가 만든 worktree 안에" — writes into the worktree,
never the user's global config). Symlink-safe write path:
`src/main/agent-hooks/hook-config-write-path.ts:3-19`.

---

## §2 Model extensions required (suaegi's `AgentDef` / `PromptInjection` /
`AgentKind`)

Concrete, in priority order.

**1. `PromptInjection` must grow from 2 to ~6 variants.** Today
`Argv | None` (`agent.rs:15-20`). Orca needs
(`tui-agent-config.ts:4-10`):
- `Argv` — with a **per-agent** `separator: Option<&'static str>` (Orca's
  `argvPromptSeparator`). suaegi currently hard-codes the `--` push for *all*
  argv agents (`agent.rs:92-93`); Orca applies it to grok only
  (`:287`). Making it per-agent is a behavior change suaegi should adopt or
  consciously keep as-is.
- `Flag(&'static str)` — e.g. opencode/mimo `flag-prompt` pass the prompt as a
  value to a prompt flag (`:103,111`). (Orca's flag name isn't literal in the
  config; the mode name encodes it — the plan must recover the actual flag from
  Orca's launch builder, not assumed here.)
- `FlagInteractive` / `FlagPromptInteractive` — copilot (`:276`), gemini/
  antigravity (`:134,140`): a flag that seeds prompt **without** exiting (bare
  `--prompt` runs headless and quits).
- `StdinAfterStart` — the **majority** (19 of 34) agents: launch bare TUI, then
  type the prompt into the PTY after the composer is ready. suaegi has **no**
  post-spawn PTY-write path today; this is a whole new capability, not just an
  enum variant.
- `HermesQuery` — hermes' bespoke startup-query contract (`:263`).

**2. `AgentDef` needs new fields:**
- `detect_cmd` distinct from `launch_program` — 8 agents have
  id/detect/launch/binary mismatches: `aug→auggie`, `continue→cn`,
  `kiro→kiro-cli`, `cursor→cursor-agent`, `qwen-code→qwen`, `mistral-vibe→vibe`,
  `antigravity→agy`, `mimo-code→mimo`. suaegi's single `launch_program`
  conflates these. Add `detect_cmd` + `detect_aliases: &[&str]`
  (`detectCmdAliases`, used by mistral-vibe `:239`).
- `expected_process` as a distinct scalar (Orca separates it from detectCmd;
  suaegi folds everything into `process_names`). Keep `process_names` for the
  matcher but seed it from `expected_process` + `detect_cmd` + aliases + launch
  token, exactly as `agent-process-recognition.ts:60-80` does.
- `launch_args` must support **subcommand args** — `kiro-cli chat --tui`,
  `hermes --tui`, `command-code --trust`, `orca claude-teams`. suaegi's
  `launch_args` already exists; the plan just needs to populate it (Orca stores
  the whole string and tokenizes — suaegi must split at author time).
- `required_commands: &[&str]` (Orca `detectRequiredCommands`, claude-agent-
  teams `:60`) and `unsupported_runtimes` (`:62`) for install gating.
- `config_injection` — an enum describing per-agent status wiring:
  `ClaudeSettingsJson`, `CodexToml`, `CursorHooksJson`, `GeminiSettingsJson`,
  `FactorySettingsJson`, `KimiToml`, `CommandCodeSettingsJson`, `None`, plus
  `preflight_trust: Option<Trust>` (cursor/copilot/codex,
  `agent-trust-presets.ts:8`). **Big caveat:** Orca writes all of these to the
  **global home**; suaegi's whole design writes per-worktree
  (`inject.rs`). Only Claude is known to honor a per-worktree
  `settings.local.json` (suaegi verified this empirically, `inject.rs:17-25`).
  For every other agent the plan must decide: replicate Orca's global-home
  writes (violates suaegi's "don't touch the user's config" invariant) or fall
  back to OSC-title status only. **This is the plan's central design fork.**
- `draft_prompt`: `Flag(&str)` | `EnvVar(&str)` | `None` — claude/openclaude
  `--prefill` (`:53,76`), pi/omp env vars (`:121,128`). Optional for v1 (it is a
  UX nicety, not required to launch).

**3. `AgentKind` must become open/enumerable, not a 3-value enum.** Today
`Claude | Codex | Custom` (`agent.rs:8-13`). 34 agents means either a 34-variant
enum or an id-keyed table (Orca uses the string-union + `Record`). Recommend a
`&'static str` id keyed static table (mirroring `TUI_AGENT_CONFIG`) rather than
34 Rust enum variants, so adding an agent stays "one table row" (the property
`agent.rs:32` already advertises). Add per-agent `display_name`
(`tui-agent-display-names.ts:8-43`) and an `icon` hint
(`src/renderer/src/lib/agent-icon-glyphs.tsx` — letter-glyph fallback plus a few
brand SVGs: `ClaudeIcon`, `DroidIcon`, `OpenAIIcon`,
`agent-catalog.tsx:2`) if the widget shows agent identity.

**4. New capability, not a field: an OSC-title status detector.** suaegi's
status today is Claude-hooks + presence polling only (`agent_status/mod.rs`).
Orca's universal status signal for the other 33 agents is OSC-title parsing
(`agent-detector.ts`, `detectAgentStatusFromTitle`). Without it, every non-
Claude agent in suaegi will have zero working/idle status. This is the single
biggest functional gap after prompt injection.

---

## §3 Detection reliability notes (`match_agent` misfire surface)

- **Interpreter-wrapped CLIs.** Orca specifically handles agents that appear in
  the process table as `node …/cli.js` or `python -m …`, not as their own
  binary: `NODE_PACKAGE_SCRIPT_ENTRYPOINTS` pins
  `codex → node_modules/@openai/codex/` and
  `gemini → node_modules/@google/gemini-cli/`
  (`agent-process-recognition.ts:51-54`), and there's a Python entrypoint /
  `-m module` recognizer (`:216-256`). suaegi's `match_agent` handles the node
  case for claude/codex via `SCRIPT_LAUNCHERS` + `segment_matches`
  (`agent.rs:52,130-140,168-176`) but has **no per-agent package-path marker**,
  so it would misidentify by bare directory segment. aider is Python-based and
  would only be caught by a python-module recognizer suaegi lacks.
- **Packaged platform binaries.** node-pty can report Codex/Grok as
  `codex-aarch64-…` / `grok-…`; Orca prefix-matches `codex-`/`grok-`
  (`agent-process-recognition.ts:88-95`). suaegi already lists `codex.exe` but
  not the `codex-<arch>` variants — a gap for both codex and grok.
- **Shared runtime trees.** antigravity writes into `~/.gemini/` (`agy`,
  `hook-service.ts:69`) — same tree as gemini. Distinct binaries so
  process-name detection is fine, but any config-tree logic must not cross them.
- **`command-code` vs `cmd.exe`.** Orca deliberately uses the full name
  `command-code` (not its `cmd` alias) so detection doesn't collide with
  Windows' built-in `cmd.exe` (`tui-agent-config.ts:200`). suaegi's
  `basename_matches` would collide if it ever used the short alias.
- **`continue` is a shell builtin.** Orca detects `cn`, not `continue`
  (`:208-209`). A naive `continue` process_name would never match (and could
  shadow the keyword).
- **Wrapper vs real process.** claude-agent-teams' child process *is* `claude`;
  Orca guards `PROCESS_TO_AGENT` against wrapper configs overwriting canonical
  ownership (`agent-process-recognition.ts:71-77`) and requires the
  `claude-teams` subcommand token to be present
  (`:299-301, 313-318`). If suaegi adds wrapper agents, the first-writer-wins
  ordering matters.
- **suaegi's current over-eager `--` separator** (`agent.rs:92-93`) is applied
  to *all* argv agents. In Orca only grok opts in (`:287`). For an agent whose
  CLI does not treat `--` as a positional terminator, an unconditional `--`
  could itself become a misfire (prompt shifted or `--` shown literally). Adopt
  per-agent `argvPromptSeparator`.

---

## §4 Open questions / risks for the plan author

1. **Per-worktree vs global config injection — the central fork.** suaegi's
   entire hook design writes into the worktree it owns and refuses to touch the
   user's global config (`inject.rs:1-15`). Orca writes every non-Claude agent's
   hook config to the global home (`~/.codex`, `~/.cursor`, …). suaegi
   *verified* only Claude honors a per-worktree `settings.local.json`
   (`inject.rs:17-25`). For codex/cursor/gemini/droid/kimi/command-code the plan
   must choose: (a) replicate Orca's global writes and abandon the invariant,
   (b) test whether each agent honors a project-local config (unverified — needs
   empirical work like suaegi already did for Claude), or (c) ship those agents
   with OSC-title status only and no hooks. Recommend (c) for v1, (b) as
   follow-up per agent.
2. **Where does the literal prompt flag for `flag-prompt` live?** The config
   encodes the *mode* (`flag-prompt`), not the flag string. I did **not** read
   Orca's PTY launch builder that turns `flag-prompt` into an actual `--prompt`/
   `-p`. The plan must read that builder (start from a `pty:spawn` /
   local-pty-provider consumer of `promptInjectionMode`) before hard-coding flag
   names — do not assume `-p`.
3. **`stdin-after-start` timing.** 19 agents need the prompt typed into the PTY
   after the composer mounts, gated by `draftPasteReadySignal`
   (render-quiet / cursor-after-bracketed-paste / codex-composer-prompt,
   `tui-agent-config.ts:12-15`) and bracketed-paste. suaegi has no post-spawn
   PTY-write path at all. Scope: is v1 argv/flag agents only, deferring stdin
   agents? That single decision determines whether the majority of the 34 are
   in or out.
4. **Trust-preset writes touch global user state.** cursor/copilot/codex
   preflight-trust writes files under `~/.cursor`, `~/.copilot`, `~/.codex`
   (`agent-trust-presets.ts:39-118`) — same invariant tension as (1). Without
   them the first-launch trust menu eats the injected prompt. Decide per agent.
5. **Icons/branding.** If the terminal widget shows agent identity, suaegi needs
   an icon story. Orca uses letter-glyph fallbacks plus a few brand SVGs
   (`agent-icon-glyphs.tsx`, `agent-catalog.tsx:2`). Low risk, but a data
   dependency the table should carry.
6. **Skipped as out-of-scope:** chat/provider-only and account modules
   (`claude-accounts`, `codex-accounts`, `grok-accounts`, `native-chat`,
   `ai-vault`, `text-generation`, `speech`, `emulator`) — these are not
   coding-agent CLIs in `TUI_AGENT_CONFIG` and don't map to `AgentDef`.
   `native-chat` is Orca's in-app chat, not a spawnable CLI. Noted and skipped
   deliberately; all 34 spawnable CLIs are covered above.
