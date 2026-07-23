//! The keybinding registry: platforms, scopes, the closed action-id enum, and
//! the 84 keybinding definitions with their per-platform defaults and flags.
//!
//! Ported from Orca `src/shared/keybindings.ts`:
//!   - `KeybindingScope` (:5-13)
//!   - `KeybindingPlatform` (:17)
//!   - `KeybindingActionId` (:28-114) — MINUS the templated `tab.newAgent.${agent}`
//!     family (:26,:61,:1059), which is deferred to the app boundary (M6, plan F2).
//!   - `KeybindingDefinition` / `PlatformBindings` (:134-151)
//!   - `KEYBINDING_DEFINITIONS` (:197-1044) — all 84 concrete rows.

use serde::{Deserialize, Serialize};

/// The platform a keybinding is resolved against. Always **injected** by the
/// caller — never auto-detected inside this leaf crate. Mirror of Orca
/// `KeybindingPlatform` (`keybindings.ts:17`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeybindingPlatform {
    Darwin,
    Linux,
    Win32,
}

/// Which UI surface an action belongs to. Mirror of Orca `KeybindingScope`
/// (`keybindings.ts:5-13`). `Composer` has no default binding today but is part
/// of the scope union, so it is carried faithfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Global,
    Tabs,
    Terminal,
    Browser,
    Editor,
    FileExplorer,
    Composer,
    Settings,
}

/// The closed set of remappable actions. Faithful transcription of Orca
/// `KeybindingActionId` (`keybindings.ts:28-114`) **excluding** the templated
/// `tab.newAgent.${TuiAgent}` family (`:61`) — that per-agent family is built at
/// the app boundary in M6 from suaegi-term's live agent table (plan F2), so it
/// cannot live in this leaf crate.
///
/// Variant order matches [`KEYBINDING_DEFINITIONS`] 1:1. The canonical dotted id
/// (used for serde + display) is the single source of truth in [`Self::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindingActionId {
    WorktreeQuickOpen,
    AppSettings,
    AppForceReload,
    WorktreePalette,
    WorktreeNavigateUp,
    WorktreeNavigateDown,
    WorkspaceCreate,
    WorkspaceRename,
    WorkspaceDelete,
    WorkspaceOpenBoard,
    WorkspaceSelectByIndex,
    VoiceDictation,
    ViewTasks,
    SidebarLeftToggle,
    SidebarRightToggle,
    SidebarExplorerToggle,
    SidebarSearchToggle,
    SidebarSourceControlToggle,
    SidebarChecksToggle,
    SidebarPortsToggle,
    SidebarSleepingWorkspacesToggle,
    SidebarFocusWorktreeList,
    FloatingTerminalToggle,
    FloatingWorkspaceMaximize,
    FloatingWorkspaceMinimize,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    WorktreeHistoryBack,
    WorktreeHistoryForward,
    TabNewTerminal,
    TabNewAgent,
    TabNewBrowser,
    TabNewSimulator,
    TabNewMarkdown,
    TabOpenMarkdown,
    TabClose,
    TabCloseAll,
    TabRename,
    TabReopenClosed,
    TabNextSameType,
    TabPreviousSameType,
    TabNextAllTypes,
    TabPreviousAllTypes,
    TabPreviousRecent,
    TabNextTerminal,
    TabPreviousTerminal,
    TabSelectByIndex,
    TabOpenQuickCommandsMenu,
    BrowserFind,
    BrowserBack,
    BrowserForward,
    BrowserReload,
    BrowserHardReload,
    BrowserFocusAddressBar,
    BrowserGrabElement,
    EditorFind,
    EditorReplace,
    EditorSave,
    EditorMarkdownPreview,
    EditorCopyContext,
    EditorPreviousChange,
    EditorNextChange,
    EditorAddReviewNote,
    FileExplorerUndo,
    FileExplorerRedo,
    FileExplorerCopyPath,
    FileExplorerCopyRelativePath,
    FileExplorerDelete,
    SettingsSearch,
    TerminalCopySelection,
    TerminalPaste,
    TerminalSearch,
    TerminalClear,
    TerminalFocusNextPane,
    TerminalFocusPreviousPane,
    TerminalEqualizePaneSizes,
    TerminalExpandPane,
    TerminalSetTitle,
    TerminalClearPaneTitle,
    TerminalClosePane,
    TerminalSplitRight,
    TerminalSplitDown,
    TerminalSwitchInputSource,
}

impl KeybindingActionId {
    /// The canonical dotted id exactly as it appears in Orca and on disk.
    pub const fn as_str(self) -> &'static str {
        use KeybindingActionId::*;
        match self {
            WorktreeQuickOpen => "worktree.quickOpen",
            AppSettings => "app.settings",
            AppForceReload => "app.forceReload",
            WorktreePalette => "worktree.palette",
            WorktreeNavigateUp => "worktree.navigateUp",
            WorktreeNavigateDown => "worktree.navigateDown",
            WorkspaceCreate => "workspace.create",
            WorkspaceRename => "workspace.rename",
            WorkspaceDelete => "workspace.delete",
            WorkspaceOpenBoard => "workspace.openBoard",
            WorkspaceSelectByIndex => "workspace.selectByIndex",
            VoiceDictation => "voice.dictation",
            ViewTasks => "view.tasks",
            SidebarLeftToggle => "sidebar.left.toggle",
            SidebarRightToggle => "sidebar.right.toggle",
            SidebarExplorerToggle => "sidebar.explorer.toggle",
            SidebarSearchToggle => "sidebar.search.toggle",
            SidebarSourceControlToggle => "sidebar.sourceControl.toggle",
            SidebarChecksToggle => "sidebar.checks.toggle",
            SidebarPortsToggle => "sidebar.ports.toggle",
            SidebarSleepingWorkspacesToggle => "sidebar.sleepingWorkspaces.toggle",
            SidebarFocusWorktreeList => "sidebar.focusWorktreeList",
            FloatingTerminalToggle => "floatingTerminal.toggle",
            FloatingWorkspaceMaximize => "floatingWorkspace.maximize",
            FloatingWorkspaceMinimize => "floatingWorkspace.minimize",
            ZoomIn => "zoom.in",
            ZoomOut => "zoom.out",
            ZoomReset => "zoom.reset",
            WorktreeHistoryBack => "worktree.history.back",
            WorktreeHistoryForward => "worktree.history.forward",
            TabNewTerminal => "tab.newTerminal",
            TabNewAgent => "tab.newAgent",
            TabNewBrowser => "tab.newBrowser",
            TabNewSimulator => "tab.newSimulator",
            TabNewMarkdown => "tab.newMarkdown",
            TabOpenMarkdown => "tab.openMarkdown",
            TabClose => "tab.close",
            TabCloseAll => "tab.closeAll",
            TabRename => "tab.rename",
            TabReopenClosed => "tab.reopenClosed",
            TabNextSameType => "tab.nextSameType",
            TabPreviousSameType => "tab.previousSameType",
            TabNextAllTypes => "tab.nextAllTypes",
            TabPreviousAllTypes => "tab.previousAllTypes",
            TabPreviousRecent => "tab.previousRecent",
            TabNextTerminal => "tab.nextTerminal",
            TabPreviousTerminal => "tab.previousTerminal",
            TabSelectByIndex => "tab.selectByIndex",
            TabOpenQuickCommandsMenu => "tab.openQuickCommandsMenu",
            BrowserFind => "browser.find",
            BrowserBack => "browser.back",
            BrowserForward => "browser.forward",
            BrowserReload => "browser.reload",
            BrowserHardReload => "browser.hardReload",
            BrowserFocusAddressBar => "browser.focusAddressBar",
            BrowserGrabElement => "browser.grabElement",
            EditorFind => "editor.find",
            EditorReplace => "editor.replace",
            EditorSave => "editor.save",
            EditorMarkdownPreview => "editor.markdownPreview",
            EditorCopyContext => "editor.copyContext",
            EditorPreviousChange => "editor.previousChange",
            EditorNextChange => "editor.nextChange",
            EditorAddReviewNote => "editor.addReviewNote",
            FileExplorerUndo => "fileExplorer.undo",
            FileExplorerRedo => "fileExplorer.redo",
            FileExplorerCopyPath => "fileExplorer.copyPath",
            FileExplorerCopyRelativePath => "fileExplorer.copyRelativePath",
            FileExplorerDelete => "fileExplorer.delete",
            SettingsSearch => "settings.search",
            TerminalCopySelection => "terminal.copySelection",
            TerminalPaste => "terminal.paste",
            TerminalSearch => "terminal.search",
            TerminalClear => "terminal.clear",
            TerminalFocusNextPane => "terminal.focusNextPane",
            TerminalFocusPreviousPane => "terminal.focusPreviousPane",
            TerminalEqualizePaneSizes => "terminal.equalizePaneSizes",
            TerminalExpandPane => "terminal.expandPane",
            TerminalSetTitle => "terminal.setTitle",
            TerminalClearPaneTitle => "terminal.clearPaneTitle",
            TerminalClosePane => "terminal.closePane",
            TerminalSplitRight => "terminal.splitRight",
            TerminalSplitDown => "terminal.splitDown",
            TerminalSwitchInputSource => "terminal.switchInputSource",
        }
    }

    /// Parse a dotted id back into its variant. Mirror of Orca
    /// `isKeybindingActionId` (`keybindings.ts:1113`), but returning the variant.
    pub fn from_id(value: &str) -> Option<Self> {
        KEYBINDING_DEFINITIONS
            .iter()
            .find(|def| def.id.as_str() == value)
            .map(|def| def.id)
    }
}

impl Serialize for KeybindingActionId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for KeybindingActionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_id(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown action id: {raw}")))
    }
}

/// Per-platform default chords for an action. Mirror of Orca `PlatformBindings`
/// (`keybindings.ts:134-138`). Each list is a `Vec`-equivalent of chord strings
/// in their pre-canonical form (e.g. `"Mod+Shift+J"`).
#[derive(Debug, Clone, Copy)]
pub struct PerPlatform {
    pub darwin: &'static [&'static str],
    pub linux: &'static [&'static str],
    pub win32: &'static [&'static str],
}

impl PerPlatform {
    /// The default chords for `platform`.
    pub const fn for_platform(&self, platform: KeybindingPlatform) -> &'static [&'static str] {
        match platform {
            KeybindingPlatform::Darwin => self.darwin,
            KeybindingPlatform::Linux => self.linux,
            KeybindingPlatform::Win32 => self.win32,
        }
    }
}

/// One remappable action's metadata. Mirror of Orca `KeybindingDefinition`
/// (`keybindings.ts:140-151`). Optional TS flags become `bool` defaulting to
/// `false`; `conflictGroup?` becomes `Option`.
#[derive(Debug, Clone, Copy)]
pub struct KeybindingDefinition {
    pub id: KeybindingActionId,
    pub title: &'static str,
    pub group: &'static str,
    pub scope: Scope,
    pub search_keywords: &'static [&'static str],
    pub default_bindings: PerPlatform,
    pub allow_in_terminal: bool,
    pub allow_bare_keybindings: bool,
    pub allow_shift_only_keybindings: bool,
    pub conflict_group: Option<&'static str>,
}

/// `darwin == linux == win32`. Mirror of Orca `platformBindings` (`:1101`).
const fn same(bindings: &'static [&'static str]) -> PerPlatform {
    PerPlatform {
        darwin: bindings,
        linux: bindings,
        win32: bindings,
    }
}

/// Build a definition with the three optional flags defaulted to `false` and no
/// conflict group — the common case. Keeps the 84-row table readable; rows that
/// need a flag or a conflict group construct the struct literal directly.
const fn def(
    id: KeybindingActionId,
    title: &'static str,
    group: &'static str,
    scope: Scope,
    search_keywords: &'static [&'static str],
    default_bindings: PerPlatform,
) -> KeybindingDefinition {
    KeybindingDefinition {
        id,
        title,
        group,
        scope,
        search_keywords,
        default_bindings,
        allow_in_terminal: false,
        allow_bare_keybindings: false,
        allow_shift_only_keybindings: false,
        conflict_group: None,
    }
}

use KeybindingActionId as A;
use Scope::*;

/// The digit-index actions whose stored chord is a representative: the digit
/// canonicalizes to `1` but the binding fires for any `1`-`9`. Orca
/// `DIGIT_INDEX_ACTION_IDS` (`keybindings.ts:1087-1090`). Consumed by M2/M3.
pub const DIGIT_INDEX_ACTION_IDS: &[KeybindingActionId] =
    &[A::TabSelectByIndex, A::WorkspaceSelectByIndex];

/// All 84 keybinding definitions. Faithful transcription of Orca
/// `KEYBINDING_DEFINITIONS` (`keybindings.ts:197-1044`), in source order,
/// **excluding** the spread `buildAgentTabKeybindingDefinitions()` (plan F2).
pub const KEYBINDING_DEFINITIONS: &[KeybindingDefinition] = &[
    def(
        A::WorktreeQuickOpen,
        "Go to File",
        "Global",
        Global,
        &["shortcut", "global", "file", "quick open"],
        same(&["Mod+P"]),
    ),
    KeybindingDefinition {
        conflict_group: Some("menu"),
        ..def(
            A::AppSettings,
            "Open Settings",
            "Global",
            Global,
            &["shortcut", "settings", "preferences"],
            same(&["Mod+Comma"]),
        )
    },
    KeybindingDefinition {
        conflict_group: Some("menu"),
        ..def(
            A::AppForceReload,
            "Force Reload",
            "Global",
            Global,
            &["shortcut", "reload", "refresh", "force"],
            same(&["Mod+Shift+R"]),
        )
    },
    def(
        A::WorktreePalette,
        "Switch worktree",
        "Global",
        Global,
        &["shortcut", "global", "worktree", "switch", "jump"],
        PerPlatform {
            darwin: &["Mod+J"],
            linux: &["Mod+Shift+J"],
            win32: &["Mod+Shift+J"],
        },
    ),
    def(
        A::WorktreeNavigateUp,
        "Previous worktree",
        "Global",
        Global,
        &["shortcut", "global", "worktree", "previous", "up"],
        same(&["Mod+Shift+ArrowUp"]),
    ),
    def(
        A::WorktreeNavigateDown,
        "Next worktree",
        "Global",
        Global,
        &["shortcut", "global", "worktree", "next", "down"],
        same(&["Mod+Shift+ArrowDown"]),
    ),
    def(
        A::WorkspaceCreate,
        "Create worktree",
        "Global",
        Global,
        &["shortcut", "global", "worktree", "create", "new workspace"],
        same(&["Mod+N", "Mod+Shift+N"]),
    ),
    KeybindingDefinition {
        conflict_group: Some("workspace-shell"),
        ..def(
            A::WorkspaceRename,
            "Rename worktree",
            "Global",
            Global,
            &[
                "shortcut",
                "global",
                "worktree",
                "rename",
                "workspace",
                "title",
            ],
            PerPlatform {
                darwin: &["Mod+Alt+R"],
                linux: &[],
                win32: &[],
            },
        )
    },
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::WorkspaceDelete,
            "Delete Workspace",
            "Global",
            Global,
            &[
                "shortcut",
                "global",
                "workspace",
                "current workspace",
                "worktree",
                "delete",
                "remove",
                "trash",
            ],
            same(&[]),
        )
    },
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::WorkspaceOpenBoard,
            "Open Workspace Board",
            "Global",
            Global,
            &[
                "shortcut",
                "global",
                "workspace",
                "board",
                "kanban",
                "worktree",
            ],
            same(&[]),
        )
    },
    def(
        A::WorkspaceSelectByIndex,
        "Select Workspace 1\u{2013}9",
        "Global",
        Global,
        &[
            "shortcut",
            "global",
            "workspace",
            "worktree",
            "select",
            "switch",
            "number",
            "digit",
            "1-9",
            "index",
        ],
        same(&["Mod+1"]),
    ),
    def(
        A::VoiceDictation,
        "Dictation",
        "Global",
        Global,
        &["shortcut", "dictation", "voice", "speech", "microphone"],
        same(&["Mod+E"]),
    ),
    def(
        A::ViewTasks,
        "Open Tasks",
        "Global",
        Global,
        &["shortcut", "tasks", "github issues", "linear"],
        same(&[]),
    ),
    def(
        A::SidebarLeftToggle,
        "Toggle Sidebar",
        "Global",
        Global,
        &["shortcut", "sidebar", "left"],
        same(&["Mod+B"]),
    ),
    def(
        A::SidebarRightToggle,
        "Toggle Right Sidebar",
        "Global",
        Global,
        &["shortcut", "sidebar", "right"],
        same(&["Mod+L"]),
    ),
    def(
        A::SidebarExplorerToggle,
        "Show Explorer",
        "Global",
        Global,
        &["shortcut", "sidebar", "explorer", "files"],
        same(&["Mod+Shift+E"]),
    ),
    def(
        A::SidebarSearchToggle,
        "Show Search",
        "Global",
        Global,
        &["shortcut", "sidebar", "search"],
        same(&["Mod+Shift+F"]),
    ),
    def(
        A::SidebarSourceControlToggle,
        "Show Source Control",
        "Global",
        Global,
        &["shortcut", "sidebar", "source control", "git"],
        same(&["Mod+Shift+G"]),
    ),
    def(
        A::SidebarChecksToggle,
        "Show Checks",
        "Global",
        Global,
        &["shortcut", "sidebar", "checks", "ci"],
        same(&[]),
    ),
    def(
        A::SidebarPortsToggle,
        "Show Ports",
        "Global",
        Global,
        &["shortcut", "sidebar", "ports"],
        PerPlatform {
            darwin: &["Mod+Shift+I"],
            linux: &[],
            win32: &[],
        },
    ),
    def(
        A::SidebarSleepingWorkspacesToggle,
        "Toggle Sleeping Workspaces",
        "Global",
        Global,
        &[
            "shortcut",
            "sidebar",
            "sleeping",
            "asleep",
            "workspaces",
            "worktree",
            "filter",
            "show",
            "hide",
        ],
        same(&[]),
    ),
    def(
        A::SidebarFocusWorktreeList,
        "Focus worktree list",
        "Global",
        Global,
        &["shortcut", "sidebar", "worktree", "focus"],
        same(&["Mod+Shift+0"]),
    ),
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::FloatingTerminalToggle,
            "Toggle Floating Terminal",
            "Global",
            Global,
            &["shortcut", "floating terminal", "terminal"],
            same(&["Mod+Alt+A"]),
        )
    },
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::FloatingWorkspaceMaximize,
            "Maximize Floating Workspace Panel",
            "Global",
            Global,
            &[
                "shortcut",
                "floating",
                "workspace",
                "panel",
                "floating workspace",
                "workspace panel",
                "maximize",
                "expand",
            ],
            PerPlatform {
                darwin: &["Mod+Alt+Shift+A"],
                linux: &[],
                win32: &[],
            },
        )
    },
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::FloatingWorkspaceMinimize,
            "Minimize Floating Workspace Panel",
            "Global",
            Global,
            &[
                "shortcut",
                "floating",
                "workspace",
                "panel",
                "floating workspace",
                "workspace panel",
                "minimize",
                "hide",
            ],
            PerPlatform {
                darwin: &[],
                linux: &[],
                win32: &[],
            },
        )
    },
    def(
        A::ZoomIn,
        "Zoom In",
        "Global",
        Global,
        &["shortcut", "zoom", "in", "scale"],
        same(&["Mod+Equal", "Mod+Shift+Plus", "Mod+NumpadAdd"]),
    ),
    def(
        A::ZoomOut,
        "Zoom Out",
        "Global",
        Global,
        &["shortcut", "zoom", "out", "scale"],
        same(&["Mod+Minus", "Mod+NumpadSubtract"]),
    ),
    def(
        A::ZoomReset,
        "Reset Size",
        "Global",
        Global,
        &["shortcut", "zoom", "reset", "size", "actual"],
        same(&["Mod+0"]),
    ),
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::WorktreeHistoryBack,
            "Worktree History Back",
            "Global",
            Global,
            &["shortcut", "worktree", "history", "back"],
            same(&["Mod+Alt+ArrowLeft"]),
        )
    },
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::WorktreeHistoryForward,
            "Worktree History Forward",
            "Global",
            Global,
            &["shortcut", "worktree", "history", "forward"],
            same(&["Mod+Alt+ArrowRight"]),
        )
    },
    def(
        A::TabNewTerminal,
        "New terminal tab",
        "Tabs",
        Tabs,
        &["shortcut", "tab", "terminal", "new"],
        same(&["Mod+T"]),
    ),
    def(
        A::TabNewAgent,
        "New agent tab (default agent)",
        "Tabs",
        Tabs,
        &["shortcut", "tab", "agent", "new", "default", "launch"],
        PerPlatform {
            darwin: &["Mod+Alt+T"],
            linux: &[],
            win32: &[],
        },
    ),
    def(
        A::TabNewBrowser,
        "New browser tab",
        "Tabs",
        Tabs,
        &["shortcut", "tab", "browser", "new"],
        same(&["Mod+Shift+B"]),
    ),
    def(
        A::TabNewSimulator,
        "New mobile emulator tab",
        "Tabs",
        Tabs,
        &[
            "shortcut",
            "tab",
            "simulator",
            "emulator",
            "mobile",
            "ios",
            "new",
        ],
        PerPlatform {
            darwin: &["Mod+Alt+Shift+E"],
            linux: &[],
            win32: &[],
        },
    ),
    def(
        A::TabNewMarkdown,
        "New markdown tab",
        "Tabs",
        Tabs,
        &["shortcut", "tab", "markdown", "file", "new"],
        same(&["Mod+Shift+M"]),
    ),
    def(
        A::TabOpenMarkdown,
        "Open markdown tab",
        "Tabs",
        Tabs,
        &["shortcut", "tab", "markdown", "file", "open"],
        same(&["Mod+Shift+O"]),
    ),
    def(
        A::TabClose,
        "Close active tab",
        "Tabs",
        Tabs,
        &["shortcut", "close", "tab", "pane"],
        same(&["Mod+W"]),
    ),
    def(
        A::TabCloseAll,
        "Close all editor tabs",
        "Tabs",
        Tabs,
        &["shortcut", "close", "all", "tabs", "files", "editors"],
        same(&["Mod+Alt+W"]),
    ),
    KeybindingDefinition {
        conflict_group: Some("workspace-shell"),
        ..def(
            A::TabRename,
            "Rename active tab",
            "Tabs",
            Tabs,
            &["shortcut", "tab", "rename", "title", "label"],
            PerPlatform {
                darwin: &["Mod+R"],
                linux: &[],
                win32: &[],
            },
        )
    },
    def(
        A::TabReopenClosed,
        "Reopen closed tab",
        "Tabs",
        Tabs,
        &["shortcut", "tab", "reopen", "restore", "closed"],
        same(&["Mod+Shift+T"]),
    ),
    def(
        A::TabNextSameType,
        "Next tab (same type)",
        "Tab Navigation",
        Tabs,
        &["shortcut", "tab", "next", "switch", "cycle"],
        same(&["Mod+Alt+BracketRight"]),
    ),
    def(
        A::TabPreviousSameType,
        "Previous tab (same type)",
        "Tab Navigation",
        Tabs,
        &["shortcut", "tab", "previous", "switch", "cycle"],
        same(&["Mod+Alt+BracketLeft"]),
    ),
    def(
        A::TabNextAllTypes,
        "Next tab (all types)",
        "Tab Navigation",
        Tabs,
        &["shortcut", "tab", "next", "switch", "cycle", "all", "any"],
        same(&["Mod+Shift+BracketRight"]),
    ),
    def(
        A::TabPreviousAllTypes,
        "Previous tab (all types)",
        "Tab Navigation",
        Tabs,
        &[
            "shortcut", "tab", "previous", "switch", "cycle", "all", "any",
        ],
        same(&["Mod+Shift+BracketLeft"]),
    ),
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::TabPreviousRecent,
            "Previous recent tab",
            "Tab Navigation",
            Tabs,
            &["shortcut", "tab", "recent", "mru", "switch", "last used"],
            same(&["Ctrl+Tab"]),
        )
    },
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::TabNextTerminal,
            "Next terminal tab",
            "Tab Navigation",
            Tabs,
            &["shortcut", "tab", "terminal", "next", "switch"],
            same(&["Ctrl+PageDown"]),
        )
    },
    KeybindingDefinition {
        allow_in_terminal: true,
        ..def(
            A::TabPreviousTerminal,
            "Previous terminal tab",
            "Tab Navigation",
            Tabs,
            &["shortcut", "tab", "terminal", "previous", "switch"],
            same(&["Ctrl+PageUp"]),
        )
    },
    def(
        A::TabSelectByIndex,
        "Select Tab 1\u{2013}9",
        "Tab Navigation",
        Tabs,
        &[
            "shortcut", "tab", "select", "switch", "number", "digit", "1-9", "index",
        ],
        PerPlatform {
            darwin: &["Ctrl+1"],
            linux: &["Alt+1"],
            win32: &["Alt+1"],
        },
    ),
    KeybindingDefinition {
        conflict_group: Some("global"),
        ..def(
            A::TabOpenQuickCommandsMenu,
            "Toggle Quick Commands menu",
            "Quick Commands",
            Tabs,
            &[
                "shortcut", "quick", "command", "menu", "tab", "group", "toggle",
            ],
            same(&[]),
        )
    },
    def(
        A::BrowserFind,
        "Find in Browser",
        "Browser",
        Browser,
        &["shortcut", "browser", "find", "search"],
        same(&["Mod+F"]),
    ),
    def(
        A::BrowserBack,
        "Go Back in Browser",
        "Browser",
        Browser,
        &["shortcut", "browser", "history", "back", "previous"],
        PerPlatform {
            darwin: &["Mod+BracketLeft"],
            linux: &["Alt+ArrowLeft"],
            win32: &["Alt+ArrowLeft"],
        },
    ),
    def(
        A::BrowserForward,
        "Go Forward in Browser",
        "Browser",
        Browser,
        &["shortcut", "browser", "history", "forward", "next"],
        PerPlatform {
            darwin: &["Mod+BracketRight"],
            linux: &["Alt+ArrowRight"],
            win32: &["Alt+ArrowRight"],
        },
    ),
    def(
        A::BrowserReload,
        "Reload Browser Page",
        "Browser",
        Browser,
        &["shortcut", "browser", "reload", "refresh"],
        same(&["Mod+R"]),
    ),
    def(
        A::BrowserHardReload,
        "Hard Reload Browser Page",
        "Browser",
        Browser,
        &["shortcut", "browser", "reload", "refresh", "cache"],
        same(&["Mod+Shift+R"]),
    ),
    def(
        A::BrowserFocusAddressBar,
        "Focus Browser Address Bar",
        "Browser",
        Browser,
        &["shortcut", "browser", "address", "url", "location"],
        same(&["Mod+L"]),
    ),
    def(
        A::BrowserGrabElement,
        "Grab Page Element",
        "Browser",
        Browser,
        &["shortcut", "browser", "grab", "copy", "element"],
        same(&["Mod+C"]),
    ),
    def(
        A::EditorFind,
        "Find in editor",
        "Editors",
        Editor,
        &["shortcut", "editor", "find", "search"],
        same(&["Mod+F"]),
    ),
    def(
        A::EditorReplace,
        "Replace in editor",
        "Editors",
        Editor,
        &["shortcut", "editor", "replace", "find", "search"],
        PerPlatform {
            darwin: &["Mod+Alt+F"],
            linux: &["Mod+H"],
            win32: &["Mod+H"],
        },
    ),
    def(
        A::EditorSave,
        "Save File",
        "Editors",
        Editor,
        &["shortcut", "editor", "save"],
        same(&["Mod+S"]),
    ),
    def(
        A::EditorMarkdownPreview,
        "Show Markdown Preview",
        "Editors",
        Editor,
        &["shortcut", "editor", "markdown", "preview"],
        same(&["Mod+Shift+V"]),
    ),
    def(
        A::EditorCopyContext,
        "Copy Context",
        "Editors",
        Editor,
        &["shortcut", "editor", "copy", "context"],
        same(&["Mod+Alt+C"]),
    ),
    KeybindingDefinition {
        allow_bare_keybindings: true,
        ..def(
            A::EditorPreviousChange,
            "Go to Previous Change",
            "Editors",
            Editor,
            &["shortcut", "editor", "diff", "change", "hunk", "previous"],
            same(&["Shift+F7"]),
        )
    },
    KeybindingDefinition {
        allow_bare_keybindings: true,
        ..def(
            A::EditorNextChange,
            "Go to Next Change",
            "Editors",
            Editor,
            &["shortcut", "editor", "diff", "change", "hunk", "next"],
            same(&["F7"]),
        )
    },
    def(
        A::EditorAddReviewNote,
        "Add Review Note",
        "Editors",
        Editor,
        &[
            "shortcut",
            "editor",
            "markdown",
            "note",
            "comment",
            "annotation",
            "review",
        ],
        same(&["Mod+Shift+A"]),
    ),
    def(
        A::FileExplorerUndo,
        "Undo file operation",
        "File Explorer",
        FileExplorer,
        &["shortcut", "file explorer", "undo"],
        same(&["Mod+Z"]),
    ),
    def(
        A::FileExplorerRedo,
        "Redo file operation",
        "File Explorer",
        FileExplorer,
        &["shortcut", "file explorer", "redo"],
        PerPlatform {
            darwin: &["Mod+Shift+Z"],
            linux: &["Mod+Shift+Z", "Ctrl+Y"],
            win32: &["Mod+Shift+Z", "Ctrl+Y"],
        },
    ),
    def(
        A::FileExplorerCopyPath,
        "Copy file path",
        "File Explorer",
        FileExplorer,
        &["shortcut", "file explorer", "copy", "path"],
        PerPlatform {
            darwin: &["Mod+Alt+C"],
            linux: &["Alt+Shift+C"],
            win32: &["Alt+Shift+C"],
        },
    ),
    def(
        A::FileExplorerCopyRelativePath,
        "Copy relative file path",
        "File Explorer",
        FileExplorer,
        &["shortcut", "file explorer", "copy", "relative", "path"],
        same(&["Mod+Alt+Shift+C"]),
    ),
    KeybindingDefinition {
        allow_bare_keybindings: true,
        ..def(
            A::FileExplorerDelete,
            "Delete file",
            "File Explorer",
            FileExplorer,
            &["shortcut", "file explorer", "delete", "remove", "trash"],
            PerPlatform {
                darwin: &["Mod+Backspace", "Delete"],
                linux: &["Delete"],
                win32: &["Delete"],
            },
        )
    },
    def(
        A::SettingsSearch,
        "Search Settings",
        "Settings",
        Settings,
        &["shortcut", "settings", "search", "find"],
        same(&["Mod+F"]),
    ),
    def(
        A::TerminalCopySelection,
        "Copy terminal selection",
        "Terminal Panes",
        Terminal,
        &["shortcut", "terminal", "copy", "selection"],
        same(&["Mod+Shift+C"]),
    ),
    def(
        A::TerminalPaste,
        "Paste into terminal",
        "Terminal Panes",
        Terminal,
        &["shortcut", "terminal", "paste", "clipboard"],
        PerPlatform {
            darwin: &["Mod+V"],
            linux: &["Ctrl+V", "Ctrl+Shift+V", "Shift+Insert"],
            win32: &["Ctrl+V", "Ctrl+Shift+V", "Shift+Insert"],
        },
    ),
    def(
        A::TerminalSearch,
        "Search active pane",
        "Terminal Panes",
        Terminal,
        &["shortcut", "terminal", "search", "find"],
        same(&["Mod+F"]),
    ),
    def(
        A::TerminalClear,
        "Clear active pane",
        "Terminal Panes",
        Terminal,
        &["shortcut", "pane", "clear"],
        same(&["Mod+K"]),
    ),
    def(
        A::TerminalFocusNextPane,
        "Focus next pane",
        "Terminal Panes",
        Terminal,
        &["shortcut", "pane", "focus", "next"],
        same(&["Mod+BracketRight"]),
    ),
    def(
        A::TerminalFocusPreviousPane,
        "Focus previous pane",
        "Terminal Panes",
        Terminal,
        &["shortcut", "pane", "focus", "previous"],
        same(&["Mod+BracketLeft"]),
    ),
    def(
        A::TerminalEqualizePaneSizes,
        "Equalize pane sizes",
        "Terminal Panes",
        Terminal,
        &[
            "shortcut", "pane", "split", "equalize", "resize", "balance", "size",
        ],
        same(&[]),
    ),
    def(
        A::TerminalExpandPane,
        "Expand / collapse pane",
        "Terminal Panes",
        Terminal,
        &["shortcut", "pane", "expand", "collapse"],
        same(&["Mod+Shift+Enter"]),
    ),
    def(
        A::TerminalSetTitle,
        "Set Title\u{2026}",
        "Terminal Panes",
        Terminal,
        &[
            "shortcut",
            "terminal",
            "pane",
            "set title",
            "title",
            "rename",
        ],
        same(&[]),
    ),
    def(
        A::TerminalClearPaneTitle,
        "Clear Pane Title",
        "Terminal Panes",
        Terminal,
        &[
            "shortcut",
            "terminal",
            "pane",
            "clear title",
            "remove title",
            "title",
        ],
        same(&[]),
    ),
    def(
        A::TerminalClosePane,
        "Close active pane",
        "Terminal Panes",
        Terminal,
        &["shortcut", "pane", "close"],
        same(&["Mod+W"]),
    ),
    def(
        A::TerminalSplitRight,
        "Split terminal right",
        "Terminal Panes",
        Terminal,
        &["shortcut", "pane", "split", "right"],
        PerPlatform {
            darwin: &["Mod+D"],
            linux: &["Mod+Shift+D"],
            win32: &["Mod+Shift+D"],
        },
    ),
    def(
        A::TerminalSplitDown,
        "Split terminal down",
        "Terminal Panes",
        Terminal,
        &["shortcut", "pane", "split", "down"],
        PerPlatform {
            darwin: &["Mod+Shift+D"],
            linux: &["Alt+Shift+D"],
            win32: &["Alt+Shift+D"],
        },
    ),
    KeybindingDefinition {
        allow_shift_only_keybindings: true,
        ..def(
            A::TerminalSwitchInputSource,
            "Switch input source / language (native)",
            "Terminal Panes",
            Terminal,
            &[
                "shortcut", "input", "source", "language", "korean", "english", "ime", "switch",
                "hangul", "layout",
            ],
            PerPlatform {
                darwin: &[],
                linux: &[],
                win32: &[],
            },
        )
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    // Crux: 84 rows x 3 platforms, zero transcription drift. The count pins the
    // registry against accidental additions/deletions and against the templated
    // agent family leaking in (plan F2).
    #[test]
    fn registry_has_exactly_84_definitions() {
        assert_eq!(KEYBINDING_DEFINITIONS.len(), 84);
    }

    #[test]
    fn action_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for def in KEYBINDING_DEFINITIONS {
            assert!(
                seen.insert(def.id.as_str()),
                "duplicate id: {}",
                def.id.as_str()
            );
        }
        assert_eq!(seen.len(), 84);
    }

    #[test]
    fn no_templated_agent_family_leaked() {
        // The `tab.newAgent.${agent}` family must NOT be here (only the bare
        // `tab.newAgent` default action is). Guards plan F2.
        for def in KEYBINDING_DEFINITIONS {
            assert!(
                !def.id.as_str().starts_with("tab.newAgent."),
                "templated agent id leaked: {}",
                def.id.as_str()
            );
        }
        assert!(KeybindingActionId::from_id("tab.newAgent").is_some());
        assert!(KeybindingActionId::from_id("tab.newAgent.claude").is_none());
    }

    // Spot-check representative rows across all three platforms. A mutation to
    // any of these default bindings (or scope/flags) fails here.
    #[test]
    fn spot_check_worktree_palette_per_platform() {
        let def = lookup(A::WorktreePalette);
        assert_eq!(def.default_bindings.darwin, &["Mod+J"]);
        assert_eq!(def.default_bindings.linux, &["Mod+Shift+J"]);
        assert_eq!(def.default_bindings.win32, &["Mod+Shift+J"]);
        assert_eq!(def.scope, Scope::Global);
    }

    #[test]
    fn spot_check_tab_select_by_index_per_platform() {
        let def = lookup(A::TabSelectByIndex);
        assert_eq!(def.default_bindings.darwin, &["Ctrl+1"]);
        assert_eq!(def.default_bindings.linux, &["Alt+1"]);
        assert_eq!(def.default_bindings.win32, &["Alt+1"]);
        assert_eq!(def.scope, Scope::Tabs);
    }

    #[test]
    fn spot_check_terminal_paste_per_platform() {
        let def = lookup(A::TerminalPaste);
        assert_eq!(def.default_bindings.darwin, &["Mod+V"]);
        assert_eq!(
            def.default_bindings.linux,
            &["Ctrl+V", "Ctrl+Shift+V", "Shift+Insert"]
        );
        assert_eq!(
            def.default_bindings.win32,
            &["Ctrl+V", "Ctrl+Shift+V", "Shift+Insert"]
        );
        assert_eq!(def.scope, Scope::Terminal);
    }

    #[test]
    fn spot_check_flags_and_conflict_groups() {
        assert_eq!(lookup(A::AppSettings).conflict_group, Some("menu"));
        assert_eq!(
            lookup(A::WorkspaceRename).conflict_group,
            Some("workspace-shell")
        );
        assert_eq!(
            lookup(A::TabOpenQuickCommandsMenu).conflict_group,
            Some("global")
        );
        assert!(lookup(A::WorkspaceDelete).allow_in_terminal);
        assert!(lookup(A::EditorNextChange).allow_bare_keybindings);
        assert!(lookup(A::FileExplorerDelete).allow_bare_keybindings);
        assert!(lookup(A::TerminalSwitchInputSource).allow_shift_only_keybindings);
        // A plain row carries no flags / conflict group.
        let plain = lookup(A::WorktreeQuickOpen);
        assert!(!plain.allow_in_terminal);
        assert!(!plain.allow_bare_keybindings);
        assert!(!plain.allow_shift_only_keybindings);
        assert_eq!(plain.conflict_group, None);
    }

    #[test]
    fn digit_index_action_ids() {
        assert_eq!(
            DIGIT_INDEX_ACTION_IDS,
            &[A::TabSelectByIndex, A::WorkspaceSelectByIndex]
        );
    }

    #[test]
    fn action_id_roundtrips_through_str() {
        for def in KEYBINDING_DEFINITIONS {
            assert_eq!(KeybindingActionId::from_id(def.id.as_str()), Some(def.id));
        }
    }

    #[test]
    fn action_id_serde_uses_dotted_string() {
        let json = serde_json::to_string(&A::SidebarSourceControlToggle).unwrap();
        assert_eq!(json, "\"sidebar.sourceControl.toggle\"");
        let back: KeybindingActionId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, A::SidebarSourceControlToggle);
        assert!(serde_json::from_str::<KeybindingActionId>("\"nope.nope\"").is_err());
    }

    fn lookup(id: KeybindingActionId) -> &'static KeybindingDefinition {
        KEYBINDING_DEFINITIONS
            .iter()
            .find(|def| def.id == id)
            .expect("definition present")
    }
}
