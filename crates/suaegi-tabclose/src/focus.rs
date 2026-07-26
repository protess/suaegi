//! `pickNextActiveTab` (`O:16-28`) and `deriveActiveSurface` (`O:80-154`) of
//! `workspace-session-terminal-tab-close.ts`.
//!
//! N1: focus succession is MRU-first, not a visual neighbour. In order: (1)
//! walk `recentTabIds` from the TAIL backwards, taking the first entry that
//! is still a survivor; (2) otherwise the first survivor strictly to the
//! RIGHT of the first closing position in `tabOrder`; (3) otherwise the LAST
//! survivor; (4) otherwise `None`. When nothing in `tabOrder` is closing,
//! step (2)'s `closingIndex` is `-1` (JS `findIndex` miss), so "strictly to
//! the right of -1" is true for every survivor's `tabOrder` position and the
//! rule degenerates to "the first survivor" — reproduced via `i64` so the
//! `-1` sentinel is representable (a `usize`/`Option<usize>` encoding would
//! need an extra branch that the JS source doesn't have).
//!
//! N9: `deriveActiveSurface` is a 6-way decision with three INDEPENDENT
//! per-kind fallbacks (terminal/browser/file), each "keep the prior
//! selection if it still exists, else index 0, else none". The third
//! `activeUnified` branch (`O:129-136`) maps `contentType === 'simulator'`
//! to `'simulator'` and folds every other content type — including `'diff'`
//! — into `'editor'`.

use std::collections::HashSet;

use crate::{PersistedOpenFile, Tab, TabContentType, TabGroup, WorkspaceSessionState, WorkspaceVisibleTabType};

/// `pickNextActiveTab` (`O:16-28`).
pub fn pick_next_active_tab(group: &TabGroup, closing_ids: &HashSet<String>) -> Option<String> {
    let remaining: Vec<&String> = group
        .tab_order
        .iter()
        .filter(|id| !closing_ids.contains(*id))
        .collect();

    // O:18-23: walk `recentTabIds` from the tail backwards (oldest -> most
    // recent means the tail is the MOST recent), returning the first
    // survivor found.
    if let Some(recent) = &group.recent_tab_ids {
        for id in recent.iter().rev() {
            if remaining.contains(&id) {
                return Some(id.clone());
            }
        }
    }

    // O:24: `findIndex` yields `-1` on a miss — every survivor's position in
    // `tabOrder` is `>= 0`, so "position > -1" is true for all of them and
    // step (2) degenerates to "the first survivor" (see module docs).
    let closing_index: i64 = group
        .tab_order
        .iter()
        .position(|id| closing_ids.contains(id))
        .map(|i| i as i64)
        .unwrap_or(-1);

    // O:25-27: first survivor strictly right of `closingIndex`, else the
    // last survivor, else `None`.
    remaining
        .iter()
        .find(|id| {
            let position = group
                .tab_order
                .iter()
                .position(|x| x == **id)
                .map(|i| i as i64)
                .unwrap_or(-1);
            position > closing_index
        })
        .copied()
        .cloned()
        .or_else(|| remaining.last().map(|id| (*id).clone()))
}

/// Return type of `deriveActiveSurface` (`O:86-91`).
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSurface {
    pub terminal_tab_id: Option<String>,
    pub browser_tab_id: Option<String>,
    pub file_id: Option<String>,
    pub kind: WorkspaceVisibleTabType,
}

/// `deriveActiveSurface` (`O:80-154`). Reads `session` for the terminal
/// rows, browser tabs, open files, and the three prior-selection maps; reads
/// `tabs`/`groups`/`active_group_id` for the just-pruned unified state (the
/// caller passes the partially-built next state's tabs/groups here, not the
/// original session's — see `close_terminal_tab_in_workspace_session`).
pub fn derive_active_surface(
    session: &WorkspaceSessionState,
    worktree_id: &str,
    tabs: &[Tab],
    groups: &[TabGroup],
    active_group_id: Option<&str>,
) -> ActiveSurface {
    // O:92: `groups.find(...) ?? groups[0] ?? null` — falls back to the
    // FIRST group (not `None`) when the id doesn't match.
    let active_group = active_group_id
        .and_then(|id| groups.iter().find(|group| group.id == id))
        .or_else(|| groups.first());

    // O:93-96: requires BOTH `activeGroup.activeTabId` truthy AND the
    // candidate tab's `groupId` to equal `activeGroup.id`.
    let active_unified: Option<&Tab> = active_group.and_then(|group| {
        let active_tab_id = group.active_tab_id.as_ref()?;
        if active_tab_id.is_empty() {
            return None;
        }
        tabs.iter()
            .find(|tab| &tab.id == active_tab_id && tab.group_id == group.id)
    });

    let terminal_tabs = session
        .tabs_by_worktree
        .get(worktree_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let browsers = session
        .browser_tabs_by_worktree
        .as_ref()
        .and_then(|m| m.get(worktree_id))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let files: &[PersistedOpenFile] = session
        .open_files_by_worktree
        .as_ref()
        .and_then(|m| m.get(worktree_id))
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    // O:100-103: N7 — the file fallback keys on `filePath`, not an id;
    // terminal/browser fallbacks below key on `id`.
    let prior_terminal: Option<String> = session
        .active_tab_id_by_worktree
        .as_ref()
        .and_then(|m| m.get(worktree_id))
        .cloned()
        .flatten();
    let terminal_fallback = if terminal_tabs
        .iter()
        .any(|tab| Some(&tab.id) == prior_terminal.as_ref())
    {
        prior_terminal
    } else {
        terminal_tabs.first().map(|tab| tab.id.clone())
    };

    let prior_browser: Option<String> = session
        .active_browser_tab_id_by_worktree
        .as_ref()
        .and_then(|m| m.get(worktree_id))
        .cloned()
        .flatten();
    let browser_fallback = if browsers
        .iter()
        .any(|tab| Some(&tab.id) == prior_browser.as_ref())
    {
        prior_browser
    } else {
        browsers.first().map(|tab| tab.id.clone())
    };

    let prior_file: Option<String> = session
        .active_file_id_by_worktree
        .as_ref()
        .and_then(|m| m.get(worktree_id))
        .cloned()
        .flatten();
    let file_fallback = if files
        .iter()
        .any(|file| Some(&file.file_path) == prior_file.as_ref())
    {
        prior_file
    } else {
        files.first().map(|file| file.file_path.clone())
    };

    // O:113-120: branch 1 — active unified tab is a terminal.
    if let Some(tab) = active_unified {
        if tab.content_type == TabContentType::Terminal {
            return ActiveSurface {
                terminal_tab_id: Some(tab.entity_id.clone()),
                browser_tab_id: browser_fallback,
                file_id: file_fallback,
                kind: WorkspaceVisibleTabType::Terminal,
            };
        }
    }
    // O:121-128: branch 2 — active unified tab is a browser.
    if let Some(tab) = active_unified {
        if tab.content_type == TabContentType::Browser {
            return ActiveSurface {
                terminal_tab_id: terminal_fallback,
                browser_tab_id: Some(tab.entity_id.clone()),
                file_id: file_fallback,
                kind: WorkspaceVisibleTabType::Browser,
            };
        }
    }
    // O:129-136: branch 3 — any other active unified tab. N9: `simulator`
    // stays `simulator`; every other content type (including `diff`) folds
    // to `editor`.
    if let Some(tab) = active_unified {
        let kind = if tab.content_type == TabContentType::Simulator {
            WorkspaceVisibleTabType::Simulator
        } else {
            WorkspaceVisibleTabType::Editor
        };
        return ActiveSurface {
            terminal_tab_id: terminal_fallback,
            browser_tab_id: browser_fallback,
            file_id: Some(tab.entity_id.clone()),
            kind,
        };
    }
    // O:137-144: branch 4 — no active unified tab, but a file fallback
    // exists.
    if let Some(file_id) = file_fallback.clone() {
        return ActiveSurface {
            terminal_tab_id: terminal_fallback,
            browser_tab_id: browser_fallback,
            file_id: Some(file_id),
            kind: WorkspaceVisibleTabType::Editor,
        };
    }
    // O:145-152: branch 5 — no file fallback, but a browser fallback exists.
    if let Some(browser_id) = browser_fallback.clone() {
        return ActiveSurface {
            terminal_tab_id: terminal_fallback,
            browser_tab_id: Some(browser_id),
            file_id: None,
            kind: WorkspaceVisibleTabType::Browser,
        };
    }
    // O:153: branch 6 — nothing at all; still surfaces a terminal fallback
    // (which may itself be `None`).
    ActiveSurface {
        terminal_tab_id: terminal_fallback,
        browser_tab_id: None,
        file_id: None,
        kind: WorkspaceVisibleTabType::Terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserTab, TerminalTab};
    use std::collections::HashMap;

    fn group(
        id: &str,
        active_tab_id: Option<&str>,
        tab_order: &[&str],
        recent_tab_ids: Option<&[&str]>,
    ) -> TabGroup {
        TabGroup {
            id: id.to_string(),
            active_tab_id: active_tab_id.map(|s| s.to_string()),
            tab_order: tab_order.iter().map(|s| s.to_string()).collect(),
            recent_tab_ids: recent_tab_ids
                .map(|ids| ids.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn closing(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    // -- N1: three fallbacks in isolation -----------------------------------

    #[test]
    fn n1_fallback_one_mru_from_tail_picks_most_recent_survivor() {
        // recentTabIds oldest -> newest: a, c, b. Closing "x" (not even in
        // tabOrder). Walking from the tail: b (survivor) wins immediately,
        // even though "c" and "a" are also survivors and appear earlier in
        // tabOrder.
        let g = group(
            "g1",
            Some("x"),
            &["a", "b", "c"],
            Some(&["a", "c", "b"]),
        );
        assert_eq!(pick_next_active_tab(&g, &closing(&["x"])), Some("b".to_string()));
    }

    #[test]
    fn n1_mru_direction_is_tail_to_head_not_head_to_tail() {
        // Two candidate orderings of the SAME recency set would disagree if
        // the walk direction were reversed: recentTabIds = [b, a] means "a"
        // is most recent (tail). A head-to-tail walk would wrongly return
        // "b" first.
        let g = group("g1", None, &["a", "b"], Some(&["b", "a"]));
        assert_eq!(
            pick_next_active_tab(&g, &closing(&[])),
            Some("a".to_string()),
            "must walk recentTabIds from the tail (most recent) backwards"
        );
    }

    #[test]
    fn n1_fallback_two_first_survivor_right_of_closing_position() {
        // No recentTabIds. tabOrder = [a, b, c, d], closing "b" (index 1).
        // Survivors right of index 1: c, d -> first is "c".
        let g = group("g1", None, &["a", "b", "c", "d"], None);
        assert_eq!(
            pick_next_active_tab(&g, &closing(&["b"])),
            Some("c".to_string())
        );
    }

    #[test]
    fn n1_fallback_three_last_survivor_when_closed_tab_was_last() {
        // Closing the last tab ("d") leaves no survivor to its right, so we
        // fall back to the last remaining survivor ("c").
        let g = group("g1", None, &["a", "b", "c", "d"], None);
        assert_eq!(
            pick_next_active_tab(&g, &closing(&["d"])),
            Some("c".to_string())
        );
    }

    #[test]
    fn n1_fallback_four_none_when_every_tab_is_closing() {
        let g = group("g1", None, &["a", "b"], None);
        assert_eq!(pick_next_active_tab(&g, &closing(&["a", "b"])), None);
    }

    #[test]
    fn n1_closing_index_negative_one_degenerates_to_first_survivor() {
        // Nothing in tabOrder is in the closing set -> `findIndex` misses
        // (-1) -> "position > -1" is true for every survivor -> the FIRST
        // survivor wins, not some other tab.
        let g = group("g1", None, &["a", "b", "c"], None);
        assert_eq!(
            pick_next_active_tab(&g, &closing(&["not-present"])),
            Some("a".to_string())
        );
    }

    // -- N9: the 6-way surface derivation ------------------------------------

    fn unified_tab(id: &str, entity_id: &str, group_id: &str, content_type: TabContentType) -> Tab {
        Tab {
            id: id.to_string(),
            entity_id: entity_id.to_string(),
            group_id: group_id.to_string(),
            content_type,
            is_pinned: false,
        }
    }

    fn base_session() -> WorkspaceSessionState {
        WorkspaceSessionState {
            active_worktree_id: None,
            active_tab_id: None,
            active_workspace_key: None,
            tabs_by_worktree: HashMap::new(),
            terminal_layouts_by_tab_id: HashMap::new(),
            active_worktree_ids_on_shutdown: None,
            open_files_by_worktree: None,
            active_file_id_by_worktree: None,
            browser_tabs_by_worktree: None,
            active_browser_tab_id_by_worktree: None,
            active_tab_type_by_worktree: None,
            active_tab_id_by_worktree: None,
            unified_tabs: None,
            tab_groups: None,
            tab_group_layouts: None,
            active_group_id_by_worktree: None,
            remote_session_ids_by_tab_id: None,
            default_terminal_tabs_applied_by_worktree_id: None,
            sleeping_agent_sessions_by_pane_key: None,
        }
    }

    #[test]
    fn n9_branch1_terminal_active_unified() {
        let mut session = base_session();
        session
            .tabs_by_worktree
            .insert("wt".to_string(), vec![TerminalTab::new("t1", Some("pty-1"))]);
        let tabs = vec![unified_tab("t1", "t1", "g1", TabContentType::Terminal)];
        let groups = vec![group("g1", Some("t1"), &["t1"], None)];
        let surface = derive_active_surface(&session, "wt", &tabs, &groups, Some("g1"));
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Terminal);
        assert_eq!(surface.terminal_tab_id, Some("t1".to_string()));
    }

    #[test]
    fn n9_branch2_browser_active_unified() {
        let session = base_session();
        let tabs = vec![unified_tab("b1", "browser-1", "g1", TabContentType::Browser)];
        let groups = vec![group("g1", Some("b1"), &["b1"], None)];
        let surface = derive_active_surface(&session, "wt", &tabs, &groups, Some("g1"));
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Browser);
        assert_eq!(surface.browser_tab_id, Some("browser-1".to_string()));
    }

    #[test]
    fn n9_branch3_editor_active_unified() {
        let session = base_session();
        let tabs = vec![unified_tab("e1", "/file.ts", "g1", TabContentType::Editor)];
        let groups = vec![group("g1", Some("e1"), &["e1"], None)];
        let surface = derive_active_surface(&session, "wt", &tabs, &groups, Some("g1"));
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Editor);
        assert_eq!(surface.file_id, Some("/file.ts".to_string()));
    }

    #[test]
    fn n9_branch3_diff_content_type_collapses_to_editor() {
        let session = base_session();
        let tabs = vec![unified_tab("d1", "/diff-target.ts", "g1", TabContentType::Diff)];
        let groups = vec![group("g1", Some("d1"), &["d1"], None)];
        let surface = derive_active_surface(&session, "wt", &tabs, &groups, Some("g1"));
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Editor);
    }

    #[test]
    fn n9_branch3_simulator_stays_simulator() {
        let session = base_session();
        let tabs = vec![unified_tab("s1", "sim-1", "g1", TabContentType::Simulator)];
        let groups = vec![group("g1", Some("s1"), &["s1"], None)];
        let surface = derive_active_surface(&session, "wt", &tabs, &groups, Some("g1"));
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Simulator);
    }

    #[test]
    fn n9_branch4_no_active_unified_falls_back_to_file() {
        let mut session = base_session();
        session.open_files_by_worktree = Some(HashMap::from([(
            "wt".to_string(),
            vec![PersistedOpenFile {
                file_path: "/only-file.ts".to_string(),
            }],
        )]));
        let surface = derive_active_surface(&session, "wt", &[], &[], None);
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Editor);
        assert_eq!(surface.file_id, Some("/only-file.ts".to_string()));
    }

    #[test]
    fn n9_branch5_no_file_falls_back_to_browser() {
        let mut session = base_session();
        session.browser_tabs_by_worktree = Some(HashMap::from([(
            "wt".to_string(),
            vec![BrowserTab {
                id: "browser-only".to_string(),
            }],
        )]));
        let surface = derive_active_surface(&session, "wt", &[], &[], None);
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Browser);
        assert_eq!(surface.browser_tab_id, Some("browser-only".to_string()));
        assert_eq!(surface.file_id, None);
    }

    #[test]
    fn n9_branch6_nothing_at_all_falls_back_to_terminal() {
        let session = base_session();
        let surface = derive_active_surface(&session, "wt", &[], &[], None);
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Terminal);
        assert_eq!(surface.browser_tab_id, None);
        assert_eq!(surface.file_id, None);
    }

    // -- N7: file fallback keys on filePath, not id --------------------------

    #[test]
    fn n7_file_fallback_matches_by_file_path_not_an_id() {
        let mut session = base_session();
        session.open_files_by_worktree = Some(HashMap::from([(
            "wt".to_string(),
            vec![PersistedOpenFile {
                file_path: "/prior.ts".to_string(),
            }],
        )]));
        session.active_file_id_by_worktree = Some(HashMap::from([(
            "wt".to_string(),
            Some("/prior.ts".to_string()),
        )]));
        // If this were keyed on some other synthetic "id" field instead of
        // `filePath`, the match above would fail and it would fall back to
        // files[0] instead — which happens to be the same file here, so we
        // add a SECOND file to make a wrong-key implementation pick the
        // wrong one.
        session.open_files_by_worktree = Some(HashMap::from([(
            "wt".to_string(),
            vec![
                PersistedOpenFile {
                    file_path: "/other.ts".to_string(),
                },
                PersistedOpenFile {
                    file_path: "/prior.ts".to_string(),
                },
            ],
        )]));
        let surface = derive_active_surface(&session, "wt", &[], &[], None);
        assert_eq!(surface.file_id, Some("/prior.ts".to_string()));
    }

    // -- N8: group-id mismatch yields no active unified tab ------------------

    #[test]
    fn n8_group_id_mismatch_yields_no_active_unified_tab() {
        // Tab "t1" carries a different groupId than the group that claims it
        // active -> not a match, falls through to the no-active-unified
        // path (branch 6 here, since there is no file/browser fallback).
        let session = base_session();
        let tabs = vec![unified_tab("t1", "t1", "some-other-group", TabContentType::Terminal)];
        let groups = vec![group("g1", Some("t1"), &["t1"], None)];
        let surface = derive_active_surface(&session, "wt", &tabs, &groups, Some("g1"));
        assert_eq!(surface.kind, WorkspaceVisibleTabType::Terminal);
        assert_eq!(surface.file_id, None);
    }

    #[test]
    fn n8_active_group_falls_back_to_first_group_when_id_missing() {
        // active_group_id names a group that doesn't exist -> falls back to
        // groups[0], not None.
        let session = base_session();
        let tabs = vec![unified_tab("t1", "t1", "g1", TabContentType::Terminal)];
        let groups = vec![group("g1", Some("t1"), &["t1"], None)];
        let surface = derive_active_surface(&session, "wt", &tabs, &groups, Some("does-not-exist"));
        assert_eq!(surface.terminal_tab_id, Some("t1".to_string()));
    }
}
