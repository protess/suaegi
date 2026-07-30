# Orca UI reference facts

Collected: 2026-07-28

Sources:

- Official repository: https://github.com/stablyai/orca
- Official product documentation: https://www.onorca.dev/docs
- Local reference source: `reference/orca`

Verified product model:

- Orca is a desktop IDE for running multiple coding agents side by side.
- A worktree is the primary unit of work. Each worktree owns its branch, files,
  agent terminals, editor/browser tabs, review flow, and shipping actions.
- The persistent left sidebar is for global navigation and worktree switching.
- The center workspace is reserved for terminals, editors, browsers, and splits.
- Source control, file browsing, diffs, and review are contextual surfaces and
  must not continuously consume center-workspace width.

UI identity:

- Quiet, monochrome chrome; color is reserved for state and git decoration.
- 36 px title bar, compact 12–14 px interface type, 10 px base radius.
- Dark tokens use `#0a0a0a` canvas, `#171717` chrome, `#1e1e1e` editor,
  `#2a2a2a` worktree sidebar, and `#353535` sidebar selection.
- Worktree rows expose identity first and progressively disclose metadata and
  actions.
