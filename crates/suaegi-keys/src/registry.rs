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

impl Scope {
    /// The scope's string form as used for conflict *bucketing* — identical to
    /// Orca's `KeybindingScope` string union (`keybindings.ts:5-13`), i.e. the
    /// serde camelCase spelling. This must match a `conflict_group` string
    /// verbatim so that (for example) the `"global"` conflict group buckets an
    /// action together with every `Scope::Global` action. Mirror of the raw
    /// `definition.scope` string flowing into Orca `findKeybindingConflicts`
    /// (`keybindings.ts:2253`).
    // F5 (INFO): only `"global"` (matched against the `"global"` conflict group)
    // is externally observable; the exact camelCase of the multi-word scopes
    // (e.g. `"fileExplorer"`) is unobserved — no conflict group uses those names,
    // so intra-scope bucketing only needs this fn to be internally consistent.
    // Kept camelCase to match Orca's `KeybindingScope` union verbatim.
    pub const fn as_bucket_str(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Tabs => "tabs",
            Scope::Terminal => "terminal",
            Scope::Browser => "browser",
            Scope::Editor => "editor",
            Scope::FileExplorer => "fileExplorer",
            Scope::Composer => "composer",
            Scope::Settings => "settings",
        }
    }
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

    /// The definition row for this action. Mirror of Orca `DEFINITIONS_BY_ID.get`
    /// (`keybindings.ts:1078`). Returns `None` only in the theoretically
    /// impossible case that a variant has no row (kept as `Option` to match
    /// Orca's graceful `definition?` lookups; the `defs_cover_every_variant`
    /// golden test proves it is always `Some`).
    pub fn definition(self) -> Option<&'static KeybindingDefinition> {
        KEYBINDING_DEFINITIONS.iter().find(|def| def.id == self)
    }
}

/// Whether `action` is one of the ranged digit-index rows (its stored chord is a
/// `1`-`9` representative). Mirror of Orca `isDigitIndexActionId`
/// (`keybindings.ts:1097`).
pub fn is_digit_index_action_id(action: KeybindingActionId) -> bool {
    DIGIT_INDEX_ACTION_IDS.contains(&action)
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

    // --- Golden snapshot: freezes ALL 84 rows -------------------------------
    //
    // The spot-checks above only pin ~7 rows, leaving the other ~77 rows'
    // bindings/scope/flags unguarded (a hollow-test gap). This golden test
    // serializes every field that matters for each row into a deterministic,
    // id-sorted string and compares it against a frozen expected value. Any
    // change to any row's id/title/group/scope/per-platform default/flag/
    // conflict-group flips this test — so a mutation to any single row is caught.
    //
    // EXPECTED_GOLDEN is machine-generated (see `emit_golden` below), never
    // hand-typed, then eyeballed against the source rows to confirm the
    // generator reflects the data faithfully.

    /// One deterministic line per definition. Format (tab-separated):
    /// `id | group | title | scope | D=[..] | L=[..] | W=[..] | term=b bare=b shift=b | cg=..`
    fn golden_row(def: &KeybindingDefinition) -> String {
        let join = |bindings: &[&str]| bindings.join(",");
        format!(
            "{id}\t{group}\t{title}\t{scope:?}\tD=[{d}]\tL=[{l}]\tW=[{w}]\tterm={term} bare={bare} shift={shift}\tcg={cg}",
            id = def.id.as_str(),
            group = def.group,
            title = def.title,
            scope = def.scope,
            d = join(def.default_bindings.darwin),
            l = join(def.default_bindings.linux),
            w = join(def.default_bindings.win32),
            term = def.allow_in_terminal as u8,
            bare = def.allow_bare_keybindings as u8,
            shift = def.allow_shift_only_keybindings as u8,
            cg = def.conflict_group.unwrap_or("-"),
        )
    }

    /// The whole registry as an id-sorted golden string (order-independent, so a
    /// row reorder does not mask a data change).
    fn golden_string() -> String {
        let mut rows: Vec<String> = KEYBINDING_DEFINITIONS.iter().map(golden_row).collect();
        rows.sort();
        rows.join("\n")
    }

    // Run with: cargo test -p suaegi-keys emit_golden -- --ignored --nocapture
    // Paste the output verbatim into EXPECTED_GOLDEN when intentionally changing
    // the registry, then re-verify it against the source rows by eye.
    #[test]
    #[ignore = "generator for EXPECTED_GOLDEN; not an assertion"]
    fn emit_golden() {
        println!("<<<GOLDEN\n{}\nGOLDEN>>>", golden_string());
    }

    #[test]
    fn registry_matches_golden_snapshot() {
        // Guard: the golden must actually cover all 84 rows (a shrunk EXPECTED_ROWS
        // must not silently pass).
        assert_eq!(EXPECTED_ROWS.len(), 84, "golden must freeze all 84 rows");
        assert_eq!(
            golden_string(),
            EXPECTED_ROWS.join("\n"),
            "\nregistry drifted from the frozen golden snapshot. If this change is \
             intentional, regenerate EXPECTED_ROWS via the `emit_golden` test \
             and re-verify each changed row against Orca by eye.\n"
        );
    }

    // Frozen 84-row snapshot, id-sorted. Machine-generated by `emit_golden`,
    // then eyeballed against the source rows. Each line:
    // `id | group | title | scope | D=[..] | L=[..] | W=[..] | term/bare/shift | cg`.
    #[rustfmt::skip]
    const EXPECTED_ROWS: &[&str] = &[
        "app.forceReload\tGlobal\tForce Reload\tGlobal\tD=[Mod+Shift+R]\tL=[Mod+Shift+R]\tW=[Mod+Shift+R]\tterm=0 bare=0 shift=0\tcg=menu",
        "app.settings\tGlobal\tOpen Settings\tGlobal\tD=[Mod+Comma]\tL=[Mod+Comma]\tW=[Mod+Comma]\tterm=0 bare=0 shift=0\tcg=menu",
        "browser.back\tBrowser\tGo Back in Browser\tBrowser\tD=[Mod+BracketLeft]\tL=[Alt+ArrowLeft]\tW=[Alt+ArrowLeft]\tterm=0 bare=0 shift=0\tcg=-",
        "browser.find\tBrowser\tFind in Browser\tBrowser\tD=[Mod+F]\tL=[Mod+F]\tW=[Mod+F]\tterm=0 bare=0 shift=0\tcg=-",
        "browser.focusAddressBar\tBrowser\tFocus Browser Address Bar\tBrowser\tD=[Mod+L]\tL=[Mod+L]\tW=[Mod+L]\tterm=0 bare=0 shift=0\tcg=-",
        "browser.forward\tBrowser\tGo Forward in Browser\tBrowser\tD=[Mod+BracketRight]\tL=[Alt+ArrowRight]\tW=[Alt+ArrowRight]\tterm=0 bare=0 shift=0\tcg=-",
        "browser.grabElement\tBrowser\tGrab Page Element\tBrowser\tD=[Mod+C]\tL=[Mod+C]\tW=[Mod+C]\tterm=0 bare=0 shift=0\tcg=-",
        "browser.hardReload\tBrowser\tHard Reload Browser Page\tBrowser\tD=[Mod+Shift+R]\tL=[Mod+Shift+R]\tW=[Mod+Shift+R]\tterm=0 bare=0 shift=0\tcg=-",
        "browser.reload\tBrowser\tReload Browser Page\tBrowser\tD=[Mod+R]\tL=[Mod+R]\tW=[Mod+R]\tterm=0 bare=0 shift=0\tcg=-",
        "editor.addReviewNote\tEditors\tAdd Review Note\tEditor\tD=[Mod+Shift+A]\tL=[Mod+Shift+A]\tW=[Mod+Shift+A]\tterm=0 bare=0 shift=0\tcg=-",
        "editor.copyContext\tEditors\tCopy Context\tEditor\tD=[Mod+Alt+C]\tL=[Mod+Alt+C]\tW=[Mod+Alt+C]\tterm=0 bare=0 shift=0\tcg=-",
        "editor.find\tEditors\tFind in editor\tEditor\tD=[Mod+F]\tL=[Mod+F]\tW=[Mod+F]\tterm=0 bare=0 shift=0\tcg=-",
        "editor.markdownPreview\tEditors\tShow Markdown Preview\tEditor\tD=[Mod+Shift+V]\tL=[Mod+Shift+V]\tW=[Mod+Shift+V]\tterm=0 bare=0 shift=0\tcg=-",
        "editor.nextChange\tEditors\tGo to Next Change\tEditor\tD=[F7]\tL=[F7]\tW=[F7]\tterm=0 bare=1 shift=0\tcg=-",
        "editor.previousChange\tEditors\tGo to Previous Change\tEditor\tD=[Shift+F7]\tL=[Shift+F7]\tW=[Shift+F7]\tterm=0 bare=1 shift=0\tcg=-",
        "editor.replace\tEditors\tReplace in editor\tEditor\tD=[Mod+Alt+F]\tL=[Mod+H]\tW=[Mod+H]\tterm=0 bare=0 shift=0\tcg=-",
        "editor.save\tEditors\tSave File\tEditor\tD=[Mod+S]\tL=[Mod+S]\tW=[Mod+S]\tterm=0 bare=0 shift=0\tcg=-",
        "fileExplorer.copyPath\tFile Explorer\tCopy file path\tFileExplorer\tD=[Mod+Alt+C]\tL=[Alt+Shift+C]\tW=[Alt+Shift+C]\tterm=0 bare=0 shift=0\tcg=-",
        "fileExplorer.copyRelativePath\tFile Explorer\tCopy relative file path\tFileExplorer\tD=[Mod+Alt+Shift+C]\tL=[Mod+Alt+Shift+C]\tW=[Mod+Alt+Shift+C]\tterm=0 bare=0 shift=0\tcg=-",
        "fileExplorer.delete\tFile Explorer\tDelete file\tFileExplorer\tD=[Mod+Backspace,Delete]\tL=[Delete]\tW=[Delete]\tterm=0 bare=1 shift=0\tcg=-",
        "fileExplorer.redo\tFile Explorer\tRedo file operation\tFileExplorer\tD=[Mod+Shift+Z]\tL=[Mod+Shift+Z,Ctrl+Y]\tW=[Mod+Shift+Z,Ctrl+Y]\tterm=0 bare=0 shift=0\tcg=-",
        "fileExplorer.undo\tFile Explorer\tUndo file operation\tFileExplorer\tD=[Mod+Z]\tL=[Mod+Z]\tW=[Mod+Z]\tterm=0 bare=0 shift=0\tcg=-",
        "floatingTerminal.toggle\tGlobal\tToggle Floating Terminal\tGlobal\tD=[Mod+Alt+A]\tL=[Mod+Alt+A]\tW=[Mod+Alt+A]\tterm=1 bare=0 shift=0\tcg=-",
        "floatingWorkspace.maximize\tGlobal\tMaximize Floating Workspace Panel\tGlobal\tD=[Mod+Alt+Shift+A]\tL=[]\tW=[]\tterm=1 bare=0 shift=0\tcg=-",
        "floatingWorkspace.minimize\tGlobal\tMinimize Floating Workspace Panel\tGlobal\tD=[]\tL=[]\tW=[]\tterm=1 bare=0 shift=0\tcg=-",
        "settings.search\tSettings\tSearch Settings\tSettings\tD=[Mod+F]\tL=[Mod+F]\tW=[Mod+F]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.checks.toggle\tGlobal\tShow Checks\tGlobal\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.explorer.toggle\tGlobal\tShow Explorer\tGlobal\tD=[Mod+Shift+E]\tL=[Mod+Shift+E]\tW=[Mod+Shift+E]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.focusWorktreeList\tGlobal\tFocus worktree list\tGlobal\tD=[Mod+Shift+0]\tL=[Mod+Shift+0]\tW=[Mod+Shift+0]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.left.toggle\tGlobal\tToggle Sidebar\tGlobal\tD=[Mod+B]\tL=[Mod+B]\tW=[Mod+B]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.ports.toggle\tGlobal\tShow Ports\tGlobal\tD=[Mod+Shift+I]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.right.toggle\tGlobal\tToggle Right Sidebar\tGlobal\tD=[Mod+L]\tL=[Mod+L]\tW=[Mod+L]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.search.toggle\tGlobal\tShow Search\tGlobal\tD=[Mod+Shift+F]\tL=[Mod+Shift+F]\tW=[Mod+Shift+F]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.sleepingWorkspaces.toggle\tGlobal\tToggle Sleeping Workspaces\tGlobal\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "sidebar.sourceControl.toggle\tGlobal\tShow Source Control\tGlobal\tD=[Mod+Shift+G]\tL=[Mod+Shift+G]\tW=[Mod+Shift+G]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.close\tTabs\tClose active tab\tTabs\tD=[Mod+W]\tL=[Mod+W]\tW=[Mod+W]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.closeAll\tTabs\tClose all editor tabs\tTabs\tD=[Mod+Alt+W]\tL=[Mod+Alt+W]\tW=[Mod+Alt+W]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.newAgent\tTabs\tNew agent tab (default agent)\tTabs\tD=[Mod+Alt+T]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.newBrowser\tTabs\tNew browser tab\tTabs\tD=[Mod+Shift+B]\tL=[Mod+Shift+B]\tW=[Mod+Shift+B]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.newMarkdown\tTabs\tNew markdown tab\tTabs\tD=[Mod+Shift+M]\tL=[Mod+Shift+M]\tW=[Mod+Shift+M]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.newSimulator\tTabs\tNew mobile emulator tab\tTabs\tD=[Mod+Alt+Shift+E]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.newTerminal\tTabs\tNew terminal tab\tTabs\tD=[Mod+T]\tL=[Mod+T]\tW=[Mod+T]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.nextAllTypes\tTab Navigation\tNext tab (all types)\tTabs\tD=[Mod+Shift+BracketRight]\tL=[Mod+Shift+BracketRight]\tW=[Mod+Shift+BracketRight]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.nextSameType\tTab Navigation\tNext tab (same type)\tTabs\tD=[Mod+Alt+BracketRight]\tL=[Mod+Alt+BracketRight]\tW=[Mod+Alt+BracketRight]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.nextTerminal\tTab Navigation\tNext terminal tab\tTabs\tD=[Ctrl+PageDown]\tL=[Ctrl+PageDown]\tW=[Ctrl+PageDown]\tterm=1 bare=0 shift=0\tcg=-",
        "tab.openMarkdown\tTabs\tOpen markdown tab\tTabs\tD=[Mod+Shift+O]\tL=[Mod+Shift+O]\tW=[Mod+Shift+O]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.openQuickCommandsMenu\tQuick Commands\tToggle Quick Commands menu\tTabs\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=global",
        "tab.previousAllTypes\tTab Navigation\tPrevious tab (all types)\tTabs\tD=[Mod+Shift+BracketLeft]\tL=[Mod+Shift+BracketLeft]\tW=[Mod+Shift+BracketLeft]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.previousRecent\tTab Navigation\tPrevious recent tab\tTabs\tD=[Ctrl+Tab]\tL=[Ctrl+Tab]\tW=[Ctrl+Tab]\tterm=1 bare=0 shift=0\tcg=-",
        "tab.previousSameType\tTab Navigation\tPrevious tab (same type)\tTabs\tD=[Mod+Alt+BracketLeft]\tL=[Mod+Alt+BracketLeft]\tW=[Mod+Alt+BracketLeft]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.previousTerminal\tTab Navigation\tPrevious terminal tab\tTabs\tD=[Ctrl+PageUp]\tL=[Ctrl+PageUp]\tW=[Ctrl+PageUp]\tterm=1 bare=0 shift=0\tcg=-",
        "tab.rename\tTabs\tRename active tab\tTabs\tD=[Mod+R]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=workspace-shell",
        "tab.reopenClosed\tTabs\tReopen closed tab\tTabs\tD=[Mod+Shift+T]\tL=[Mod+Shift+T]\tW=[Mod+Shift+T]\tterm=0 bare=0 shift=0\tcg=-",
        "tab.selectByIndex\tTab Navigation\tSelect Tab 1\u{2013}9\tTabs\tD=[Ctrl+1]\tL=[Alt+1]\tW=[Alt+1]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.clear\tTerminal Panes\tClear active pane\tTerminal\tD=[Mod+K]\tL=[Mod+K]\tW=[Mod+K]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.clearPaneTitle\tTerminal Panes\tClear Pane Title\tTerminal\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.closePane\tTerminal Panes\tClose active pane\tTerminal\tD=[Mod+W]\tL=[Mod+W]\tW=[Mod+W]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.copySelection\tTerminal Panes\tCopy terminal selection\tTerminal\tD=[Mod+Shift+C]\tL=[Mod+Shift+C]\tW=[Mod+Shift+C]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.equalizePaneSizes\tTerminal Panes\tEqualize pane sizes\tTerminal\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.expandPane\tTerminal Panes\tExpand / collapse pane\tTerminal\tD=[Mod+Shift+Enter]\tL=[Mod+Shift+Enter]\tW=[Mod+Shift+Enter]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.focusNextPane\tTerminal Panes\tFocus next pane\tTerminal\tD=[Mod+BracketRight]\tL=[Mod+BracketRight]\tW=[Mod+BracketRight]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.focusPreviousPane\tTerminal Panes\tFocus previous pane\tTerminal\tD=[Mod+BracketLeft]\tL=[Mod+BracketLeft]\tW=[Mod+BracketLeft]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.paste\tTerminal Panes\tPaste into terminal\tTerminal\tD=[Mod+V]\tL=[Ctrl+V,Ctrl+Shift+V,Shift+Insert]\tW=[Ctrl+V,Ctrl+Shift+V,Shift+Insert]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.search\tTerminal Panes\tSearch active pane\tTerminal\tD=[Mod+F]\tL=[Mod+F]\tW=[Mod+F]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.setTitle\tTerminal Panes\tSet Title\u{2026}\tTerminal\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.splitDown\tTerminal Panes\tSplit terminal down\tTerminal\tD=[Mod+Shift+D]\tL=[Alt+Shift+D]\tW=[Alt+Shift+D]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.splitRight\tTerminal Panes\tSplit terminal right\tTerminal\tD=[Mod+D]\tL=[Mod+Shift+D]\tW=[Mod+Shift+D]\tterm=0 bare=0 shift=0\tcg=-",
        "terminal.switchInputSource\tTerminal Panes\tSwitch input source / language (native)\tTerminal\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=1\tcg=-",
        "view.tasks\tGlobal\tOpen Tasks\tGlobal\tD=[]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=-",
        "voice.dictation\tGlobal\tDictation\tGlobal\tD=[Mod+E]\tL=[Mod+E]\tW=[Mod+E]\tterm=0 bare=0 shift=0\tcg=-",
        "workspace.create\tGlobal\tCreate worktree\tGlobal\tD=[Mod+N,Mod+Shift+N]\tL=[Mod+N,Mod+Shift+N]\tW=[Mod+N,Mod+Shift+N]\tterm=0 bare=0 shift=0\tcg=-",
        "workspace.delete\tGlobal\tDelete Workspace\tGlobal\tD=[]\tL=[]\tW=[]\tterm=1 bare=0 shift=0\tcg=-",
        "workspace.openBoard\tGlobal\tOpen Workspace Board\tGlobal\tD=[]\tL=[]\tW=[]\tterm=1 bare=0 shift=0\tcg=-",
        "workspace.rename\tGlobal\tRename worktree\tGlobal\tD=[Mod+Alt+R]\tL=[]\tW=[]\tterm=0 bare=0 shift=0\tcg=workspace-shell",
        "workspace.selectByIndex\tGlobal\tSelect Workspace 1\u{2013}9\tGlobal\tD=[Mod+1]\tL=[Mod+1]\tW=[Mod+1]\tterm=0 bare=0 shift=0\tcg=-",
        "worktree.history.back\tGlobal\tWorktree History Back\tGlobal\tD=[Mod+Alt+ArrowLeft]\tL=[Mod+Alt+ArrowLeft]\tW=[Mod+Alt+ArrowLeft]\tterm=1 bare=0 shift=0\tcg=-",
        "worktree.history.forward\tGlobal\tWorktree History Forward\tGlobal\tD=[Mod+Alt+ArrowRight]\tL=[Mod+Alt+ArrowRight]\tW=[Mod+Alt+ArrowRight]\tterm=1 bare=0 shift=0\tcg=-",
        "worktree.navigateDown\tGlobal\tNext worktree\tGlobal\tD=[Mod+Shift+ArrowDown]\tL=[Mod+Shift+ArrowDown]\tW=[Mod+Shift+ArrowDown]\tterm=0 bare=0 shift=0\tcg=-",
        "worktree.navigateUp\tGlobal\tPrevious worktree\tGlobal\tD=[Mod+Shift+ArrowUp]\tL=[Mod+Shift+ArrowUp]\tW=[Mod+Shift+ArrowUp]\tterm=0 bare=0 shift=0\tcg=-",
        "worktree.palette\tGlobal\tSwitch worktree\tGlobal\tD=[Mod+J]\tL=[Mod+Shift+J]\tW=[Mod+Shift+J]\tterm=0 bare=0 shift=0\tcg=-",
        "worktree.quickOpen\tGlobal\tGo to File\tGlobal\tD=[Mod+P]\tL=[Mod+P]\tW=[Mod+P]\tterm=0 bare=0 shift=0\tcg=-",
        "zoom.in\tGlobal\tZoom In\tGlobal\tD=[Mod+Equal,Mod+Shift+Plus,Mod+NumpadAdd]\tL=[Mod+Equal,Mod+Shift+Plus,Mod+NumpadAdd]\tW=[Mod+Equal,Mod+Shift+Plus,Mod+NumpadAdd]\tterm=0 bare=0 shift=0\tcg=-",
        "zoom.out\tGlobal\tZoom Out\tGlobal\tD=[Mod+Minus,Mod+NumpadSubtract]\tL=[Mod+Minus,Mod+NumpadSubtract]\tW=[Mod+Minus,Mod+NumpadSubtract]\tterm=0 bare=0 shift=0\tcg=-",
        "zoom.reset\tGlobal\tReset Size\tGlobal\tD=[Mod+0]\tL=[Mod+0]\tW=[Mod+0]\tterm=0 bare=0 shift=0\tcg=-",
    ];
}
