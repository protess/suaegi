//! VERBATIM port of Orca's
//! `src/shared/workspace-session-terminal-tab-close.ts` (276L) @
//! v1.4.146-rc.0. Line citations below (`O:N`) refer to that source file.
//! Its single import (`Tab`, `TabGroup`, `TabGroupLayoutNode`,
//! `WorkspaceSessionState`, `WorkspaceVisibleTabType` from `./types`) is
//! entirely type-only, so there is zero runtime dependency.
//!
//! Ported: `O:16-28` [`focus::pick_next_active_tab`], `O:30-49`
//! [`layout::prune_group_layout`], `O:51-68` [`collect_tab_pty_ids`],
//! `O:70-78` [`find_unified_terminal_tabs`], `O:80-154`
//! [`focus::derive_active_surface`], `O:156-276`
//! [`close_terminal_tab_in_workspace_session`].
//!
//! The `types.ts` imports are modeled NARROWLY — only the fields this module
//! actually reads or writes are represented:
//! - [`Tab`]: `id`, `entityId` -> `entity_id`, `groupId` -> `group_id`,
//!   `contentType` -> `content_type`, `isPinned` -> `is_pinned` (optional in
//!   TS, JS-falsy when absent — modeled as a plain `bool`). `worktreeId`,
//!   `label`, `color`, `sortOrder`, `createdAt` etc. are never read here.
//! - [`TabGroup`]: `id`, `activeTabId` -> `active_tab_id`, `tabOrder` ->
//!   `tab_order`, `recentTabIds` -> `recent_tab_ids`. `worktreeId` unused.
//! - [`TabGroupLayoutNode`] / `TabGroupSplitDirection`: modeled in full in
//!   [`layout`] (every field is read or written by `pruneGroupLayout`).
//! - [`WorkspaceSessionState`]: only the ~18 fields this module touches
//!   (`activeRepoId`, `markdownFrontmatterVisible`,
//!   `browserPagesByWorkspace`, `browserUrlHistory`,
//!   `activeConnectionIdsAtShutdown`, `lastVisitedAtByWorktreeId` are never
//!   read or written and are omitted entirely).
//! - [`TerminalTab`]: `id`, `ptyId` -> `pty_id`, `isPinned` -> `is_pinned`.
//! - [`PersistedOpenFile`]: `filePath` -> `file_path` only (N7 — the file
//!   fallback keys on this, not an id).
//! - `BrowserWorkspace` (read via `browserTabsByWorktree`, not in the
//!   caller's "read only" list but reached transitively for the browser
//!   fallback): narrowed to [`BrowserTab`] with just `id`.
//! - `SleepingAgentSessionRecord` (via `agent-session-resume`, same
//!   situation): narrowed to [`SleepingAgentSessionRecord`] with just
//!   `tab_id` — the only field the pane-key cleanup loop reads (`O:236-239`).
//!
//! `ptyIdsByLeafId` (`TerminalLayoutSnapshot`, `types.ts:1043`) is modeled as
//! `Vec<(String, String)>`, not a map, because N10 makes its iteration order
//! observable in `ptyIdsToKill` and this crate carries no ordered-map
//! dependency (`[dependencies]` is empty — see `Cargo.toml`).
//!
//! # Traps (see the plan's §1 for full rationale;
//! # `docs/superpowers/plans/2026-07-27-terminal-tab-close.md`)
//!
//! - **N1**: focus succession is MRU-first — see [`focus::pick_next_active_tab`].
//! - **N2**: the layout prune's collapse replaces a split with its surviving
//!   child, discarding `direction`/`ratio` — see [`layout::prune_group_layout`].
//!   The tree pruned is read from the ORIGINAL `session` (`O:208`,
//!   `session.tabGroupLayouts?.[worktreeId]`), never from the partially-built
//!   `next` state.
//! - **N3**: [`collect_tab_pty_ids`] is invoked once per tab across
//!   `session.tabs_by_worktree.values()` (`O:172-180`, `Object.values(session.tabsByWorktree)`
//!   — ALL worktrees), not just `worktree_id`. Every tab whose id differs
//!   from the closing `tab_id` contributes its PTYs to `other_pty_ids`; only
//!   scanning the target worktree would let another worktree's live PTY be
//!   killed. See `n3_pty_used_by_tab_in_a_different_worktree_is_not_killed`.
//! - **N4**: [`find_unified_terminal_tabs`] matches by `entity_id == tab_id
//!   OR id == tab_id` (`O:76`) — an OR, so multiple unified tabs can match
//!   one `tab_id`. See `n4_*`.
//! - **N5**: `closed_visible_ids` always contains the raw `tab_id` (`O:183`,
//!   `closedVisibleIds.add(tabId)`), even when no unified tab carries it.
//!   See `n5_*`.
//! - **N6**: two early returns (`O:163-168`), both leaving `session`
//!   unchanged: not-found (`closed: false, pinned: false`) and pinned
//!   (`closed: false, pinned: true`). The pinned check is `terminal_row.is_pinned
//!   || unified.iter().any(is_pinned)` (`O:166`, `||` not `??`) — a `false`
//!   row flag still falls through to the `.any()`. See `n6_*`.
//! - **N7**: the file fallback keys on `file_path` (`O:109`,
//!   `file.filePath === priorFile`) — see [`focus::derive_active_surface`]
//!   and its `n7_*` test.
//! - **N8**: the active unified tab requires BOTH `id` and `group_id` to
//!   match (`O:93-96`); the active group itself falls back to `find(id)`
//!   then the FIRST group, then `None` (`O:92`) — see
//!   [`focus::derive_active_surface`] and its `n8_*` tests.
//! - **N9**: the 6-way surface decision, three independent per-kind
//!   fallbacks, `diff` collapsing to `editor` — see
//!   [`focus::derive_active_surface`].
//! - **N10**: there is no sort anywhere in this module; every collection
//!   preserves insertion/original order throughout. `pty_ids_to_kill`'s
//!   order is: the row's own pty id, then the leaf-map values in insertion
//!   order, then the remote session id (`O:181`,
//!   `[...closingPtyIds].filter(...)`, where `closingPtyIds` is built by
//!   [`collect_tab_pty_ids`] in exactly that order). The oracle sorts before
//!   comparing (`T:111`, `.sort()`), so it does NOT pin this — see `n10_*`.
//! - **N11**: a group whose `tab_order` empties after the close is dropped
//!   entirely (`O:202`, `.filter(group => group.tabOrder.length > 0)`). If
//!   every group dies, `activeGroupIdByWorktree[worktreeId]` and
//!   `tabGroupLayouts[worktreeId]` are DELETED (`O:231-235`,
//!   `O:226-230`), but `tabGroups[worktreeId]` is still written as `[]`
//!   (`O:218`, unconditional spread-assign) — see `n11_*`.
//! - **N12**: the input is never mutated. [`close_terminal_tab_in_workspace_session`]
//!   takes `&WorkspaceSessionState` and returns an owned, independently
//!   cloned [`WorkspaceSessionState`] — no oracle test asserts reference
//!   identity (`toEqual`, never `toBe`, on the session), so this port clones
//!   freely rather than trying to preserve any sharing.
//! - **N13**: [`collect_tab_pty_ids`]'s row-pty-id and remote-session-id
//!   sources are JS-truthiness gated (`O:57`, `if (rowPtyId)`; `O:64`, `if
//!   (remoteSessionId)`) — an empty string is falsy and is NOT collected.
//!   The middle source (the leaf-id -> pty-id values, `O:60-62`) has NO such
//!   guard in the original and is ported the same way (unconditional). See
//!   `n13_*`.

pub mod focus;
pub mod layout;

use std::collections::{HashMap, HashSet};

pub use focus::{derive_active_surface, pick_next_active_tab, ActiveSurface};
pub use layout::{prune_group_layout, TabGroupLayoutNode, TabGroupSplitDirection};

// ---------------------------------------------------------------------------
// types.ts:789 WorkspaceVisibleTabType
// ---------------------------------------------------------------------------

/// `types.ts:789`, `'terminal' | 'editor' | 'browser' | 'simulator'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceVisibleTabType {
    Terminal,
    Editor,
    Browser,
    Simulator,
}

// ---------------------------------------------------------------------------
// types.ts:780-787 TabContentType
// ---------------------------------------------------------------------------

/// `types.ts:780-787`. Only `Terminal`/`Browser`/`Simulator` are singled out
/// by `deriveActiveSurface`'s branches (`O:113`, `O:121`, `O:134`) — every
/// other member (`Editor`, `Diff`, `ConflictReview`, `CheckDetails`) folds
/// into the third branch's `editor` result (N9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabContentType {
    Terminal,
    Editor,
    Diff,
    ConflictReview,
    CheckDetails,
    Browser,
    Simulator,
}

// ---------------------------------------------------------------------------
// types.ts:792-812 Tab (narrowed)
// ---------------------------------------------------------------------------

/// `types.ts:792-812`, narrowed to the fields this module reads: `id`,
/// `entityId`, `groupId`, `contentType`, `isPinned`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub id: String,
    pub entity_id: String,
    pub group_id: String,
    pub content_type: TabContentType,
    /// `isPinned?: boolean` (`types.ts:806`) — optional/JS-falsy-when-absent
    /// in the source; modeled as a plain `bool` since the only use
    /// (`O:166`, `.some(tab => tab.isPinned)`) treats absence and `false`
    /// identically.
    pub is_pinned: bool,
}

// ---------------------------------------------------------------------------
// types.ts:814-826 TabGroup (narrowed)
// ---------------------------------------------------------------------------

/// `types.ts:814-826`, narrowed to `id`, `activeTabId`, `tabOrder`,
/// `recentTabIds`.
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroup {
    pub id: String,
    pub active_tab_id: Option<String>,
    pub tab_order: Vec<String>,
    pub recent_tab_ids: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// types.ts:829-876 TerminalTab (narrowed, legacy row)
// ---------------------------------------------------------------------------

/// `types.ts:829-876`, narrowed to `id`, `ptyId`, `isPinned`.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalTab {
    pub id: String,
    pub pty_id: Option<String>,
    pub is_pinned: bool,
}

impl TerminalTab {
    /// Test/caller convenience constructor for the common unpinned case.
    pub fn new(id: impl Into<String>, pty_id: Option<&str>) -> Self {
        Self {
            id: id.into(),
            pty_id: pty_id.map(|s| s.to_string()),
            is_pinned: false,
        }
    }
}

// ---------------------------------------------------------------------------
// types.ts:1029-1051 TerminalLayoutSnapshot (narrowed)
// ---------------------------------------------------------------------------

/// The narrow slice of `TerminalLayoutSnapshot` (`types.ts`, around line
/// 1029) this module reads: only `ptyIdsByLeafId` (`O:60`). Modeled as
/// `Vec<(String, String)>` rather than a map — see the module docs on N10.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TerminalLayoutSnapshot {
    pub pty_ids_by_leaf_id: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// types.ts:1056-1073 PersistedOpenFile (narrowed)
// ---------------------------------------------------------------------------

/// `types.ts:1056-1073`, narrowed to `filePath` — N7: the file fallback keys
/// on this, not on any id-shaped field.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedOpenFile {
    pub file_path: String,
}

// ---------------------------------------------------------------------------
// BrowserWorkspace (narrowed) — reached transitively via browserTabsByWorktree
// ---------------------------------------------------------------------------

/// `BrowserWorkspace` (`types.ts:951+`), narrowed to `id` — the only field
/// [`focus::derive_active_surface`]'s browser fallback reads.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserTab {
    pub id: String,
}

// ---------------------------------------------------------------------------
// SleepingAgentSessionRecord (narrowed) — from agent-session-resume.ts
// ---------------------------------------------------------------------------

/// `SleepingAgentSessionRecord` (`agent-session-resume.ts`), narrowed to
/// `tabId` — the only field the pane-key cleanup loop reads (`O:236-239`,
/// `record.tabId === tabId`).
#[derive(Debug, Clone, PartialEq)]
pub struct SleepingAgentSessionRecord {
    pub tab_id: String,
}

// ---------------------------------------------------------------------------
// types.ts:1075-1137 WorkspaceSessionState (narrowed)
// ---------------------------------------------------------------------------

/// `types.ts:1075-1137`, narrowed to only the fields
/// `closeTerminalTabInWorkspaceSession` reads or writes. Fields never
/// touched by this module (`activeRepoId`, `markdownFrontmatterVisible`,
/// `browserPagesByWorkspace`, `browserUrlHistory`,
/// `activeConnectionIdsAtShutdown`, `lastVisitedAtByWorktreeId`) are omitted
/// entirely.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspaceSessionState {
    pub active_worktree_id: Option<String>,
    pub active_tab_id: Option<String>,
    /// `activeWorkspaceKey?: WorkspaceKey | null` — the template-literal
    /// `WorkspaceKey` union is opaque to this module (only ever passed
    /// through or nulled, `O:266-267`), so it is modeled as a plain
    /// `Option<String>`.
    pub active_workspace_key: Option<String>,
    pub tabs_by_worktree: HashMap<String, Vec<TerminalTab>>,
    pub terminal_layouts_by_tab_id: HashMap<String, TerminalLayoutSnapshot>,
    pub active_worktree_ids_on_shutdown: Option<Vec<String>>,
    pub open_files_by_worktree: Option<HashMap<String, Vec<PersistedOpenFile>>>,
    pub active_file_id_by_worktree: Option<HashMap<String, Option<String>>>,
    pub browser_tabs_by_worktree: Option<HashMap<String, Vec<BrowserTab>>>,
    pub active_browser_tab_id_by_worktree: Option<HashMap<String, Option<String>>>,
    pub active_tab_type_by_worktree: Option<HashMap<String, WorkspaceVisibleTabType>>,
    pub active_tab_id_by_worktree: Option<HashMap<String, Option<String>>>,
    pub unified_tabs: Option<HashMap<String, Vec<Tab>>>,
    pub tab_groups: Option<HashMap<String, Vec<TabGroup>>>,
    pub tab_group_layouts: Option<HashMap<String, TabGroupLayoutNode>>,
    pub active_group_id_by_worktree: Option<HashMap<String, String>>,
    pub remote_session_ids_by_tab_id: Option<HashMap<String, String>>,
    /// `Record<string, true>` in TS (a set encoded as an all-`true` map).
    /// This module never writes it, only passes it through untouched.
    pub default_terminal_tabs_applied_by_worktree_id: Option<HashMap<String, bool>>,
    pub sleeping_agent_sessions_by_pane_key: Option<HashMap<String, SleepingAgentSessionRecord>>,
}

// ---------------------------------------------------------------------------
// WorkspaceSessionTerminalTabCloseResult (O:9-14)
// ---------------------------------------------------------------------------

/// `O:9-14`, `WorkspaceSessionTerminalTabCloseResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceSessionTerminalTabCloseResult {
    pub session: WorkspaceSessionState,
    pub pty_ids_to_kill: Vec<String>,
    pub closed: bool,
    pub pinned: bool,
}

// ---------------------------------------------------------------------------
// O:70-78 findUnifiedTerminalTabs
// ---------------------------------------------------------------------------

/// `findUnifiedTerminalTabs` (`O:70-78`). N4: matches by `entity_id ==
/// tab_id OR id == tab_id` — an OR, so more than one unified tab can match
/// the same `tab_id`.
fn find_unified_terminal_tabs(
    session: &WorkspaceSessionState,
    worktree_id: &str,
    tab_id: &str,
) -> Vec<Tab> {
    session
        .unified_tabs
        .as_ref()
        .and_then(|by_worktree| by_worktree.get(worktree_id))
        .map(|tabs| {
            tabs.iter()
                .filter(|tab| {
                    tab.content_type == TabContentType::Terminal
                        && (tab.entity_id == tab_id || tab.id == tab_id)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// O:51-68 collectTabPtyIds
// ---------------------------------------------------------------------------

fn push_unique(ids: &mut Vec<String>, id: &str) {
    if !ids.iter().any(|existing| existing == id) {
        ids.push(id.to_string());
    }
}

/// `collectTabPtyIds` (`O:51-68`). Returns an insertion-ordered,
/// deduplicated list (mirroring JS `Set` iteration order) — N10 makes this
/// order observable in the caller's `pty_ids_to_kill`. N13: `row_pty_id`
/// (`O:57-59`) and the remote session id (`O:63-66`) are JS-truthiness
/// gated — an empty string is excluded, same as absence. The middle
/// leaf-map loop (`O:60-62`) has NO such guard in the original.
fn collect_tab_pty_ids(
    session: &WorkspaceSessionState,
    tab_id: &str,
    row_pty_id: Option<&str>,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    if let Some(id) = row_pty_id {
        if !id.is_empty() {
            push_unique(&mut ids, id);
        }
    }
    if let Some(layout) = session.terminal_layouts_by_tab_id.get(tab_id) {
        for (_leaf_id, pty_id) in &layout.pty_ids_by_leaf_id {
            push_unique(&mut ids, pty_id);
        }
    }
    if let Some(remote_id) = session
        .remote_session_ids_by_tab_id
        .as_ref()
        .and_then(|by_tab| by_tab.get(tab_id))
    {
        if !remote_id.is_empty() {
            push_unique(&mut ids, remote_id);
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// O:156-276 closeTerminalTabInWorkspaceSession
// ---------------------------------------------------------------------------

/// `closeTerminalTabInWorkspaceSession` (`O:156-276`). N12: takes `&session`
/// and returns an owned, independently cloned [`WorkspaceSessionState`] —
/// the input is never mutated.
pub fn close_terminal_tab_in_workspace_session(
    session: &WorkspaceSessionState,
    worktree_id: &str,
    tab_id: &str,
) -> WorkspaceSessionTerminalTabCloseResult {
    // O:161
    let terminal_row = session
        .tabs_by_worktree
        .get(worktree_id)
        .and_then(|tabs| tabs.iter().find(|tab| tab.id == tab_id));
    // O:162
    let unified_terminal_tabs = find_unified_terminal_tabs(session, worktree_id, tab_id);

    // O:163-165: not-found early return, session unchanged.
    if terminal_row.is_none() && unified_terminal_tabs.is_empty() {
        return WorkspaceSessionTerminalTabCloseResult {
            session: session.clone(),
            pty_ids_to_kill: Vec::new(),
            closed: false,
            pinned: false,
        };
    }
    // O:166-168: N6 — `||`, not `??`: a `false` row flag still consults
    // `.any()` on the unified tabs.
    let row_is_pinned = terminal_row.map(|row| row.is_pinned).unwrap_or(false);
    if row_is_pinned || unified_terminal_tabs.iter().any(|tab| tab.is_pinned) {
        return WorkspaceSessionTerminalTabCloseResult {
            session: session.clone(),
            pty_ids_to_kill: Vec::new(),
            closed: false,
            pinned: true,
        };
    }

    // O:170: the closing tab's own PTY set (order-preserving).
    let row_pty_id = terminal_row.and_then(|row| row.pty_id.as_deref());
    let closing_pty_ids = collect_tab_pty_ids(session, tab_id, row_pty_id);

    // O:171-180: N3 — scans EVERY worktree, not just `worktree_id`.
    let mut other_pty_ids: HashSet<String> = HashSet::new();
    for tabs in session.tabs_by_worktree.values() {
        for tab in tabs {
            if tab.id != tab_id {
                for pty_id in collect_tab_pty_ids(session, &tab.id, tab.pty_id.as_deref()) {
                    other_pty_ids.insert(pty_id);
                }
            }
        }
    }
    // O:181: N10 — order preserved from `closing_pty_ids`.
    let pty_ids_to_kill: Vec<String> = closing_pty_ids
        .into_iter()
        .filter(|pty_id| !other_pty_ids.contains(pty_id))
        .collect();

    // O:182-183: N5 — the raw `tab_id` is always in the closed set.
    let mut closed_visible_ids: HashSet<String> = unified_terminal_tabs
        .iter()
        .map(|tab| tab.id.clone())
        .collect();
    closed_visible_ids.insert(tab_id.to_string());

    // O:184-186
    let next_tabs: Vec<Tab> = session
        .unified_tabs
        .as_ref()
        .and_then(|by_worktree| by_worktree.get(worktree_id))
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|tab| !closed_visible_ids.contains(&tab.id))
        .collect();

    // O:187-202: N11 — a group whose tab_order empties is dropped entirely.
    let next_groups: Vec<TabGroup> = session
        .tab_groups
        .as_ref()
        .and_then(|by_worktree| by_worktree.get(worktree_id))
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|group| {
            let tab_order: Vec<String> = group
                .tab_order
                .iter()
                .filter(|id| !closed_visible_ids.contains(*id))
                .cloned()
                .collect();
            let active_tab_id_was_closing =
                closed_visible_ids.contains(group.active_tab_id.as_deref().unwrap_or(""));
            let active_tab_id: Option<String> = if active_tab_id_was_closing {
                pick_next_active_tab(&group, &closed_visible_ids)
            } else if group
                .active_tab_id
                .as_ref()
                .is_some_and(|id| !id.is_empty() && tab_order.contains(id))
            {
                group.active_tab_id.clone()
            } else {
                tab_order.first().cloned()
            };
            let recent_tab_ids = group.recent_tab_ids.map(|ids| {
                ids.into_iter()
                    .filter(|id| tab_order.contains(id))
                    .collect()
            });
            TabGroup {
                id: group.id,
                active_tab_id,
                tab_order,
                recent_tab_ids,
            }
        })
        .filter(|group| !group.tab_order.is_empty())
        .collect();

    let valid_group_ids: HashSet<String> =
        next_groups.iter().map(|group| group.id.clone()).collect();
    // O:204-207
    let prior_active_group_id = session
        .active_group_id_by_worktree
        .as_ref()
        .and_then(|by_worktree| by_worktree.get(worktree_id))
        .cloned();
    let next_active_group_id: Option<String> = match &prior_active_group_id {
        Some(id) if valid_group_ids.contains(id) => Some(id.clone()),
        _ => next_groups.first().map(|group| group.id.clone()),
    };
    // O:208: N2 — pruned from the ORIGINAL session's layout, not `next`.
    let next_layout = prune_group_layout(
        session
            .tab_group_layouts
            .as_ref()
            .and_then(|by_worktree| by_worktree.get(worktree_id)),
        &valid_group_ids,
    );

    // O:210-223: build `next` as a full clone of `session`, then overwrite
    // only the fields this reducer actually changes.
    let mut next = session.clone();

    let mut worktree_tabs = session
        .tabs_by_worktree
        .get(worktree_id)
        .cloned()
        .unwrap_or_default();
    worktree_tabs.retain(|tab| tab.id != tab_id);
    next.tabs_by_worktree
        .insert(worktree_id.to_string(), worktree_tabs);

    next.terminal_layouts_by_tab_id = session.terminal_layouts_by_tab_id.clone();
    next.terminal_layouts_by_tab_id.remove(tab_id); // O:224

    let mut unified_map = session.unified_tabs.clone().unwrap_or_default();
    unified_map.insert(worktree_id.to_string(), next_tabs.clone());
    next.unified_tabs = Some(unified_map);

    // O:218: `tabGroups[worktreeId]` is ALWAYS written, even as `[]`.
    let mut groups_map = session.tab_groups.clone().unwrap_or_default();
    groups_map.insert(worktree_id.to_string(), next_groups.clone());
    next.tab_groups = Some(groups_map);

    let mut remote_map = session
        .remote_session_ids_by_tab_id
        .clone()
        .unwrap_or_default();
    remote_map.remove(tab_id); // O:225
    next.remote_session_ids_by_tab_id = Some(remote_map);

    // O:226-230: layout key deleted when the pruned tree is empty.
    let mut layout_map = session.tab_group_layouts.clone().unwrap_or_default();
    match next_layout {
        Some(layout) => {
            layout_map.insert(worktree_id.to_string(), layout);
        }
        None => {
            layout_map.remove(worktree_id);
        }
    }
    next.tab_group_layouts = Some(layout_map);

    // O:231-235: active-group key deleted when no group survives.
    let mut active_group_map = session
        .active_group_id_by_worktree
        .clone()
        .unwrap_or_default();
    match &next_active_group_id {
        Some(group_id) => {
            active_group_map.insert(worktree_id.to_string(), group_id.clone());
        }
        None => {
            active_group_map.remove(worktree_id);
        }
    }
    next.active_group_id_by_worktree = Some(active_group_map);

    // O:236-240: pane-key cleanup by prefix OR by record's own tab_id.
    let mut sleeping = session
        .sleeping_agent_sessions_by_pane_key
        .clone()
        .unwrap_or_default();
    let prefix = format!("{tab_id}:");
    sleeping.retain(|pane_key, record| !(pane_key.starts_with(&prefix) || record.tab_id == tab_id));
    next.sleeping_agent_sessions_by_pane_key = Some(sleeping);

    // O:241: surface derived from `next` (post-mutation tabs/groups), NOT
    // the original `session`.
    let surface = derive_active_surface(
        &next,
        worktree_id,
        &next_tabs,
        &next_groups,
        next_active_group_id.as_deref(),
    );

    // O:242-257
    let mut active_tab_id_map = session
        .active_tab_id_by_worktree
        .clone()
        .unwrap_or_default();
    active_tab_id_map.insert(worktree_id.to_string(), surface.terminal_tab_id.clone());
    next.active_tab_id_by_worktree = Some(active_tab_id_map);

    let mut active_browser_map = session
        .active_browser_tab_id_by_worktree
        .clone()
        .unwrap_or_default();
    active_browser_map.insert(worktree_id.to_string(), surface.browser_tab_id.clone());
    next.active_browser_tab_id_by_worktree = Some(active_browser_map);

    let mut active_file_map = session
        .active_file_id_by_worktree
        .clone()
        .unwrap_or_default();
    active_file_map.insert(worktree_id.to_string(), surface.file_id.clone());
    next.active_file_id_by_worktree = Some(active_file_map);

    let mut active_type_map = session
        .active_tab_type_by_worktree
        .clone()
        .unwrap_or_default();
    active_type_map.insert(worktree_id.to_string(), surface.kind);
    next.active_tab_type_by_worktree = Some(active_type_map);

    // O:258-269
    if session.active_worktree_id.as_deref() == Some(worktree_id) {
        next.active_tab_id = surface.terminal_tab_id.clone();
        let has_surface = !next_tabs.is_empty()
            || next
                .tabs_by_worktree
                .get(worktree_id)
                .map(|tabs| !tabs.is_empty())
                .unwrap_or(false)
            || next
                .browser_tabs_by_worktree
                .as_ref()
                .and_then(|by_worktree| by_worktree.get(worktree_id))
                .map(|tabs| !tabs.is_empty())
                .unwrap_or(false)
            || next
                .open_files_by_worktree
                .as_ref()
                .and_then(|by_worktree| by_worktree.get(worktree_id))
                .map(|files| !files.is_empty())
                .unwrap_or(false);
        if !has_surface {
            next.active_worktree_id = None;
            next.active_workspace_key = None;
        }
    }

    // O:270-274
    if next
        .tabs_by_worktree
        .get(worktree_id)
        .map(|tabs| tabs.len())
        .unwrap_or(0)
        == 0
    {
        if let Some(list) = next.active_worktree_ids_on_shutdown.as_mut() {
            list.retain(|id| id != worktree_id);
        }
    }

    WorkspaceSessionTerminalTabCloseResult {
        session: next,
        pty_ids_to_kill,
        closed: true,
        pinned: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKTREE_ID: &str = "worktree-1";

    fn terminal_tab(id: &str, pty_id: Option<&str>, is_pinned: bool) -> TerminalTab {
        TerminalTab {
            id: id.to_string(),
            pty_id: pty_id.map(|s| s.to_string()),
            is_pinned,
        }
    }

    fn unified_tab(id: &str, entity_id: &str, content_type: TabContentType, group_id: &str) -> Tab {
        Tab {
            id: id.to_string(),
            entity_id: entity_id.to_string(),
            group_id: group_id.to_string(),
            content_type,
            is_pinned: false,
        }
    }

    fn base_session() -> WorkspaceSessionState {
        let mut tabs_by_worktree = HashMap::new();
        tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![terminal_tab("terminal-1", Some("pty-left"), false)],
        );

        let mut terminal_layouts_by_tab_id = HashMap::new();
        terminal_layouts_by_tab_id.insert(
            "terminal-1".to_string(),
            TerminalLayoutSnapshot {
                pty_ids_by_leaf_id: vec![
                    ("leaf-left".to_string(), "pty-left".to_string()),
                    ("leaf-right".to_string(), "pty-right".to_string()),
                ],
            },
        );

        let mut unified_tabs = HashMap::new();
        unified_tabs.insert(
            WORKTREE_ID.to_string(),
            vec![unified_tab(
                "terminal-1",
                "terminal-1",
                TabContentType::Terminal,
                "group-1",
            )],
        );

        let mut tab_groups = HashMap::new();
        tab_groups.insert(
            WORKTREE_ID.to_string(),
            vec![TabGroup {
                id: "group-1".to_string(),
                active_tab_id: Some("terminal-1".to_string()),
                tab_order: vec!["terminal-1".to_string()],
                recent_tab_ids: Some(vec!["terminal-1".to_string()]),
            }],
        );

        let mut tab_group_layouts = HashMap::new();
        tab_group_layouts.insert(
            WORKTREE_ID.to_string(),
            TabGroupLayoutNode::Leaf {
                group_id: "group-1".to_string(),
            },
        );

        let mut active_group_id_by_worktree = HashMap::new();
        active_group_id_by_worktree.insert(WORKTREE_ID.to_string(), "group-1".to_string());

        let mut active_tab_id_by_worktree = HashMap::new();
        active_tab_id_by_worktree.insert(WORKTREE_ID.to_string(), Some("terminal-1".to_string()));

        let mut default_applied = HashMap::new();
        default_applied.insert(WORKTREE_ID.to_string(), true);

        WorkspaceSessionState {
            active_worktree_id: Some(WORKTREE_ID.to_string()),
            active_tab_id: Some("terminal-1".to_string()),
            active_workspace_key: None,
            tabs_by_worktree,
            terminal_layouts_by_tab_id,
            active_worktree_ids_on_shutdown: None,
            open_files_by_worktree: None,
            active_file_id_by_worktree: None,
            browser_tabs_by_worktree: None,
            active_browser_tab_id_by_worktree: None,
            active_tab_type_by_worktree: None,
            active_tab_id_by_worktree: Some(active_tab_id_by_worktree),
            unified_tabs: Some(unified_tabs),
            tab_groups: Some(tab_groups),
            tab_group_layouts: Some(tab_group_layouts),
            active_group_id_by_worktree: Some(active_group_id_by_worktree),
            remote_session_ids_by_tab_id: None,
            default_terminal_tabs_applied_by_worktree_id: Some(default_applied),
            sleeping_agent_sessions_by_pane_key: None,
        }
    }

    // =========================================================================
    // Oracle 1/5 — T:88-117: atomically removes a dormant split tab and
    // returns every exact PTY.
    // =========================================================================

    #[test]
    fn oracle_atomically_removes_a_dormant_split_tab_and_returns_every_exact_pty() {
        let mut session = base_session();
        session.remote_session_ids_by_tab_id = Some(HashMap::from([(
            "terminal-1".to_string(),
            "pty-remote".to_string(),
        )]));
        session.sleeping_agent_sessions_by_pane_key = Some(HashMap::from([(
            "terminal-1:leaf-left".to_string(),
            SleepingAgentSessionRecord {
                tab_id: "terminal-1".to_string(),
            },
        )]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");

        assert!(result.closed);
        assert!(!result.pinned);
        let mut killed = result.pty_ids_to_kill.clone();
        killed.sort();
        assert_eq!(killed, vec!["pty-left", "pty-remote", "pty-right"]);
        assert_eq!(result.session.tabs_by_worktree[WORKTREE_ID], Vec::new());
        assert!(!result
            .session
            .terminal_layouts_by_tab_id
            .contains_key("terminal-1"));
        assert!(!result
            .session
            .remote_session_ids_by_tab_id
            .as_ref()
            .unwrap()
            .contains_key("terminal-1"));
        assert_eq!(
            result.session.sleeping_agent_sessions_by_pane_key,
            Some(HashMap::new())
        );
        assert_eq!(
            result
                .session
                .default_terminal_tabs_applied_by_worktree_id
                .as_ref()
                .unwrap()
                .get(WORKTREE_ID),
            Some(&true)
        );
    }

    // =========================================================================
    // Oracle 2/5 — T:119-147: does not kill a PTY still owned by another
    // terminal tab.
    // =========================================================================

    #[test]
    fn oracle_does_not_kill_a_pty_still_owned_by_another_terminal_tab() {
        let mut session = base_session();
        session.tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![
                terminal_tab("terminal-1", Some("shared-pty"), false),
                terminal_tab("terminal-2", Some("shared-pty"), false),
            ],
        );
        session.terminal_layouts_by_tab_id = HashMap::from([
            (
                "terminal-1".to_string(),
                TerminalLayoutSnapshot {
                    pty_ids_by_leaf_id: vec![("leaf-1".to_string(), "shared-pty".to_string())],
                },
            ),
            (
                "terminal-2".to_string(),
                TerminalLayoutSnapshot {
                    pty_ids_by_leaf_id: vec![("leaf-2".to_string(), "shared-pty".to_string())],
                },
            ),
        ]);

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");

        assert_eq!(result.pty_ids_to_kill, Vec::<String>::new());
        let remaining_ids: Vec<&str> = result.session.tabs_by_worktree[WORKTREE_ID]
            .iter()
            .map(|tab| tab.id.as_str())
            .collect();
        assert_eq!(remaining_ids, vec!["terminal-2"]);
    }

    // =========================================================================
    // Oracle 3/5 — T:149-193: lands on the active browser survivor instead
    // of an empty terminal group.
    // =========================================================================

    #[test]
    fn oracle_lands_on_the_active_browser_survivor_instead_of_an_empty_terminal_group() {
        let mut session = base_session();
        session.browser_tabs_by_worktree = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![BrowserTab {
                id: "browser-1".to_string(),
            }],
        )]));
        session.active_browser_tab_id_by_worktree = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            Some("browser-1".to_string()),
        )]));
        session.unified_tabs = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![
                unified_tab(
                    "terminal-1",
                    "terminal-1",
                    TabContentType::Terminal,
                    "group-1",
                ),
                unified_tab("browser-1", "browser-1", TabContentType::Browser, "group-1"),
            ],
        )]));
        session.tab_groups = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![TabGroup {
                id: "group-1".to_string(),
                active_tab_id: Some("terminal-1".to_string()),
                tab_order: vec!["terminal-1".to_string(), "browser-1".to_string()],
                recent_tab_ids: Some(vec!["browser-1".to_string(), "terminal-1".to_string()]),
            }],
        )]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");

        let groups = result.session.tab_groups.as_ref().unwrap();
        assert_eq!(
            groups[WORKTREE_ID][0].active_tab_id,
            Some("browser-1".to_string())
        );
        assert_eq!(
            result
                .session
                .active_tab_type_by_worktree
                .as_ref()
                .unwrap()
                .get(WORKTREE_ID),
            Some(&WorkspaceVisibleTabType::Browser)
        );
        assert_eq!(
            result
                .session
                .active_browser_tab_id_by_worktree
                .as_ref()
                .unwrap()
                .get(WORKTREE_ID)
                .unwrap(),
            &Some("browser-1".to_string())
        );
        assert_eq!(
            result.session.active_worktree_id,
            Some(WORKTREE_ID.to_string())
        );
    }

    // =========================================================================
    // Oracle 4/5 — T:195-203: rejects pinned tabs without mutating the
    // session.
    // =========================================================================

    #[test]
    fn oracle_rejects_pinned_tabs_without_mutating_the_session() {
        let mut session = base_session();
        session.tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![terminal_tab("terminal-1", Some("pty-left"), true)],
        );

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");

        assert_eq!(
            result,
            WorkspaceSessionTerminalTabCloseResult {
                session: session.clone(),
                pty_ids_to_kill: Vec::new(),
                closed: false,
                pinned: true,
            }
        );
    }

    // =========================================================================
    // Oracle 5/5 — T:205-230: has no bounded replay window after more than
    // 32 closes.
    // =========================================================================

    #[test]
    fn oracle_has_no_bounded_replay_window_after_more_than_32_closes() {
        let mut current = WorkspaceSessionState::default();
        for index in 0..40 {
            let id = format!("terminal-{index}");
            current.tabs_by_worktree.insert(
                WORKTREE_ID.to_string(),
                vec![TerminalTab::new(id.clone(), None)],
            );
            current.terminal_layouts_by_tab_id.insert(
                id.clone(),
                TerminalLayoutSnapshot {
                    pty_ids_by_leaf_id: vec![(format!("leaf-{index}"), format!("pty-{index}"))],
                },
            );
            current = close_terminal_tab_in_workspace_session(&current, WORKTREE_ID, &id).session;
        }

        assert_eq!(current.tabs_by_worktree[WORKTREE_ID], Vec::new());
        assert_eq!(current.terminal_layouts_by_tab_id, HashMap::new());
    }

    // =========================================================================
    // N3 — a PTY used by a tab in a DIFFERENT worktree is not killed.
    // =========================================================================

    #[test]
    fn n3_pty_used_by_tab_in_a_different_worktree_is_not_killed() {
        let mut session = base_session();
        session.tabs_by_worktree.insert(
            "worktree-2".to_string(),
            vec![terminal_tab("terminal-other", Some("pty-left"), false)],
        );

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");

        // "pty-left" is used by terminal-1's row AND by a tab in a
        // DIFFERENT worktree -> must survive. Only "pty-right" (unique to
        // the closing tab's split layout) is killed.
        assert_eq!(result.pty_ids_to_kill, vec!["pty-right".to_string()]);
    }

    // =========================================================================
    // N4 — matching by entity_id, by id, and multiple unified tabs matching
    // one tab_id.
    // =========================================================================

    #[test]
    fn n4_matches_by_entity_id() {
        let mut session = base_session();
        session.unified_tabs = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![unified_tab(
                "unified-x",
                "terminal-1",
                TabContentType::Terminal,
                "group-1",
            )],
        )]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");
        assert!(result.closed);
        assert!(result.session.unified_tabs.unwrap()[WORKTREE_ID].is_empty());
    }

    #[test]
    fn n4_matches_by_id() {
        let mut session = base_session();
        session.unified_tabs = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![unified_tab(
                "terminal-1",
                "some-other-entity",
                TabContentType::Terminal,
                "group-1",
            )],
        )]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");
        assert!(result.closed);
        assert!(result.session.unified_tabs.unwrap()[WORKTREE_ID].is_empty());
    }

    #[test]
    fn n4_multiple_unified_tabs_match_one_tab_id() {
        let mut session = base_session();
        session.unified_tabs = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![
                unified_tab(
                    "unified-a",
                    "terminal-1",
                    TabContentType::Terminal,
                    "group-1",
                ),
                unified_tab(
                    "terminal-1",
                    "some-other-entity",
                    TabContentType::Terminal,
                    "group-1",
                ),
                unified_tab(
                    "unrelated",
                    "unrelated-entity",
                    TabContentType::Terminal,
                    "group-1",
                ),
            ],
        )]));
        session.tab_groups = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![TabGroup {
                id: "group-1".to_string(),
                active_tab_id: Some("unrelated".to_string()),
                tab_order: vec![
                    "unified-a".to_string(),
                    "terminal-1".to_string(),
                    "unrelated".to_string(),
                ],
                recent_tab_ids: None,
            }],
        )]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");
        let remaining: Vec<&str> = result.session.unified_tabs.as_ref().unwrap()[WORKTREE_ID]
            .iter()
            .map(|tab| tab.id.as_str())
            .collect();
        // Both "unified-a" (entity_id match) and "terminal-1" (id match) are
        // removed; only "unrelated" survives.
        assert_eq!(remaining, vec!["unrelated"]);
    }

    // =========================================================================
    // N5 — the raw tab_id is treated as closed even when no unified tab
    // carries it.
    // =========================================================================

    #[test]
    fn n5_raw_tab_id_is_closed_even_with_no_matching_unified_tab() {
        let mut session = base_session();
        session.unified_tabs = Some(HashMap::from([(WORKTREE_ID.to_string(), Vec::new())]));
        session.tab_groups = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![TabGroup {
                id: "group-1".to_string(),
                active_tab_id: Some("terminal-1".to_string()),
                tab_order: vec!["terminal-1".to_string(), "terminal-2".to_string()],
                recent_tab_ids: None,
            }],
        )]));
        session.tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![
                terminal_tab("terminal-1", Some("pty-left"), false),
                terminal_tab("terminal-2", Some("pty-2"), false),
            ],
        );

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");
        let groups = result.session.tab_groups.unwrap();
        // "terminal-1" is removed from tab_order purely via the raw-id
        // membership in closed_visible_ids, even though no unified tab
        // carried it.
        assert_eq!(
            groups[WORKTREE_ID][0].tab_order,
            vec!["terminal-2".to_string()]
        );
    }

    // =========================================================================
    // N6 — is_pinned: false on the row still consults the unified tabs; and
    // the not-found return shape.
    // =========================================================================

    #[test]
    fn n6_false_row_pin_still_falls_through_to_unified_any() {
        let mut session = base_session();
        session.tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![terminal_tab("terminal-1", Some("pty-left"), false)],
        );
        session.unified_tabs = Some(HashMap::from([(
            WORKTREE_ID.to_string(),
            vec![Tab {
                id: "terminal-1".to_string(),
                entity_id: "terminal-1".to_string(),
                group_id: "group-1".to_string(),
                content_type: TabContentType::Terminal,
                is_pinned: true,
            }],
        )]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");
        assert!(!result.closed);
        assert!(result.pinned);
    }

    #[test]
    fn n6_not_found_returns_unchanged_session_with_both_flags_false() {
        let session = base_session();
        let result =
            close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "does-not-exist");
        assert_eq!(
            result,
            WorkspaceSessionTerminalTabCloseResult {
                session: session.clone(),
                pty_ids_to_kill: Vec::new(),
                closed: false,
                pinned: false,
            }
        );
    }

    // =========================================================================
    // N10 — the exact pty_ids_to_kill ORDER (row, then leaf map insertion
    // order, then remote session id) — never sorted.
    // =========================================================================

    #[test]
    fn n10_pty_ids_to_kill_preserves_exact_insertion_order() {
        let mut session = base_session();
        session.tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![terminal_tab("terminal-1", Some("pty-row"), false)],
        );
        session.terminal_layouts_by_tab_id = HashMap::from([(
            "terminal-1".to_string(),
            TerminalLayoutSnapshot {
                pty_ids_by_leaf_id: vec![
                    ("leaf-c".to_string(), "pty-c".to_string()),
                    ("leaf-a".to_string(), "pty-a".to_string()),
                    ("leaf-b".to_string(), "pty-b".to_string()),
                ],
            },
        )]);
        session.remote_session_ids_by_tab_id = Some(HashMap::from([(
            "terminal-1".to_string(),
            "pty-remote".to_string(),
        )]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");

        assert_eq!(
            result.pty_ids_to_kill,
            vec!["pty-row", "pty-c", "pty-a", "pty-b", "pty-remote"]
        );
    }

    // =========================================================================
    // N11 — the last group closing deletes both keys but leaves an empty
    // tab_groups entry.
    // =========================================================================

    #[test]
    fn n11_last_group_closing_deletes_both_keys_but_tab_groups_entry_survives_empty() {
        let session = base_session(); // single group, single tab -> dies entirely.

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");

        assert_eq!(
            result.session.tab_groups.as_ref().unwrap().get(WORKTREE_ID),
            Some(&Vec::new())
        );
        assert!(!result
            .session
            .active_group_id_by_worktree
            .as_ref()
            .unwrap()
            .contains_key(WORKTREE_ID));
        assert!(!result
            .session
            .tab_group_layouts
            .as_ref()
            .unwrap()
            .contains_key(WORKTREE_ID));
    }

    // =========================================================================
    // N13 — an empty-string pty id is not collected.
    // =========================================================================

    #[test]
    fn n13_empty_string_row_pty_id_is_not_collected() {
        let mut session = base_session();
        session.tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![terminal_tab("terminal-1", Some(""), false)],
        );
        session.terminal_layouts_by_tab_id = HashMap::new();
        session.remote_session_ids_by_tab_id = None;

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");
        assert_eq!(result.pty_ids_to_kill, Vec::<String>::new());
    }

    #[test]
    fn n13_empty_string_remote_session_id_is_not_collected() {
        let mut session = base_session();
        session.tabs_by_worktree.insert(
            WORKTREE_ID.to_string(),
            vec![terminal_tab("terminal-1", None, false)],
        );
        session.terminal_layouts_by_tab_id = HashMap::new();
        session.remote_session_ids_by_tab_id =
            Some(HashMap::from([("terminal-1".to_string(), String::new())]));

        let result = close_terminal_tab_in_workspace_session(&session, WORKTREE_ID, "terminal-1");
        assert_eq!(result.pty_ids_to_kill, Vec::<String>::new());
    }
}
