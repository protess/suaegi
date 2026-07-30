# Suaegi · Orca-aligned UI spec

Source completeness: official source, style guide, logo, and current UI
screenshots are present under `reference/orca`.

## Reference assets

- Product overview: `reference/orca/docs/assets/readme-hero.jpg`
- Worktree flow: `reference/orca/docs/assets/feature-wall/parallel-worktrees.jpg`
- Terminal splits: `reference/orca/docs/assets/feature-wall/terminal-splits.jpg`
- Native integrations: `reference/orca/docs/assets/feature-wall/github-linear.jpg`
- Official style rules: `reference/orca/docs/STYLEGUIDE.md`
- Official logo source: `reference/orca/resources/logo.svg`

## Surface tokens

- Canvas: `#0a0a0a`
- App chrome/card: `#171717`
- Editor/terminal frame: `#1e1e1e`
- Worktree sidebar: `#2a2a2a`
- Sidebar active/hover: `#353535`
- Muted fill: `#262626`
- Accent: `#404040`
- Foreground: `#fafafa`
- Muted foreground: `#a1a1a1`
- Hairline: 7% white

## Layout contract

- Title bar: 36 px.
- Left worktree sidebar: 280 px default.
- Center workspace: always receives remaining width first.
- Right context surface: one at a time, 280–460 px according to content.
- Worktree identity remains visible while actions are progressively disclosed.
- Integrations and creation forms are closed by default.

## Interaction contract

- Selecting a worktree starts or focuses its session.
- Files, search, source control, automation, diff, and PR details share one
  contextual region; opening one replaces the previous tool.
- Creating a project or worktree takes one explicit reveal action and one
  submit action.
- Destructive actions never compete visually with primary navigation.
