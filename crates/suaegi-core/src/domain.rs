use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

fn default_true() -> bool {
    true
}

fn deserialize_optional_string_vec<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    Ok(
        Option::<OneOrMany>::deserialize(deserializer)?.map(|value| match value {
            OneOrMany::One(value) => vec![value],
            OneOrMany::Many(values) => values,
        }),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorktreeId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub id: RepoId,
    pub path: PathBuf,
    pub display_name: String,
    /// worktree 생성 기본 base ref. None이면 생성 시점에 HEAD 브랜치를 감지해 사용.
    pub worktree_base_ref: Option<String>,
}

impl Repo {
    /// 앱 코드의 표준 Repo 생성 경로. canonicalize로 심볼릭 링크/상대 경로/대소문자
    /// 변형이 서로 다른 ID를 만들지 못하게 한다. (serde 역직렬화는 과거에 이 경로로
    /// 만들어 저장한 데이터를 다시 읽는 것이므로 정규화를 반복하지 않는다)
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let canonical = path.canonicalize()?;
        let display_name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        Ok(Self {
            id: RepoId(canonical.to_string_lossy().into_owned()),
            path: canonical,
            display_name,
            worktree_base_ref: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub repo_id: RepoId,
    pub path: PathBuf,
    pub branch: String,
    pub display_name: String,
    /// **`#[serde(default)]`가 데이터 손실 방어다.** 이 두 필드(6c에서 추가)가
    /// 없는 옛 저장본에서, default가 없으면 `Worktree` **하나**의 역직렬화 실패가
    /// `PersistedState` 전체를 손상 판정으로 떨어뜨려 백업 폴백으로 간다 —
    /// 멀쩡한 repo/worktree 목록을 통째로 잃는 그 사고의 한 갈래다. default가
    /// 있으면 옛 키 없는 객체도 `None`/`0`으로 조용히 읽힌다.
    #[serde(default)]
    pub created_with_agent: Option<String>,
    #[serde(default)]
    pub created_at_unix_ms: u64,
    /// 이 worktree의 브랜치에 연결된 GitHub PR 번호(Plan 7a, Orca `hosted-review.ts:45`).
    /// 이 번호로 리뷰 상태를 재해석한다. **`#[serde(default)]`가 데이터 손실 방어다** —
    /// 이 필드 없는 옛 저장본이 `None`으로 조용히 로드돼야지, 하나의 역직렬화 실패가
    /// `PersistedState` 전체를 손상 판정으로 떨어뜨리면 안 된다(위 두 필드와 같은 등급).
    #[serde(default)]
    pub linked_github_pr: Option<u64>,
    /// 이 워크트리에 링크된 Linear 이슈 식별자(예: `ENG-123`, Plan N1 §1.3). Orca는
    /// provider별 슬롯을 분리한다(`types.ts:479-489`) — GitHub PR과 나란히 **세 필드**로.
    /// 셋 다 **`#[serde(default)]`** — 이 필드 없는 옛 저장본이 `None`으로 조용히 로드돼야
    /// `PersistedState` 전체 손상 판정을 피한다(`linked_github_pr`과 같은 등급).
    #[serde(default)]
    pub linked_linear_issue: Option<String>,
    /// 다중 워크스페이스 구분(organization id). 딥링크/재연결 식별에 필요.
    #[serde(default)]
    pub linked_linear_issue_workspace_id: Option<String>,
    /// `linear.app/{urlKey}/...` 딥링크·재연결 식별자.
    #[serde(default)]
    pub linked_linear_issue_organization_url_key: Option<String>,
    /// 이 워크트리에 링크된 Jira 이슈 키(예: `PROJ-123`, Plan N2). Jira의 식별자는 Linear보다
    /// **단순하다** — 이슈 키 하나 + 어느 사이트(연결)냐, 두 조각뿐이다(Linear의 워크스페이스
    /// id/url_key 좌표 대신). **`#[serde(default)]`가 데이터 손실 방어다** — 이 필드 없는 옛
    /// 저장본이 `None`으로 조용히 로드돼야지, 하나의 역직렬화 실패가 `PersistedState` 전체를
    /// 손상 판정으로 떨어뜨리면 안 된다(`linked_linear_issue`와 같은 등급).
    #[serde(default)]
    pub linked_jira_issue: Option<String>,
    /// 어느 Jira 사이트(연결)의 이슈인지 — 사용자가 여러 사이트를 가질 수 있으므로 딥링크·재연결에
    /// 필요하다. 정규화된 `site_url`(예: `https://acme.atlassian.net`).
    #[serde(default)]
    pub linked_jira_site: Option<String>,
}

/// `pane_grid::Axis`의 serde 거울. iced 타입은 `Serialize`를 갖지 않고, 갖게
/// 만들 수도 없다(외래 타입) — 그리고 **`suaegi-core`는 iced를 모른다.**
/// 값이 둘뿐이라 거울의 유지 비용이 사실상 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedAxis {
    Horizontal,
    Vertical,
}

/// `pane_grid::Configuration<T>`의 serde 거울. 저장할 때 `State::layout()`의
/// `Node`를 걸으며 만들고, 복원할 때 `Configuration`으로 되돌린다.
///
/// **잎이 `SessionId`가 아니라 `WorktreeId`인 것이 핵심이다.** `SessionId`는
/// 실행마다 매기는 카운터라 재시작을 넘지 못하고, `pane_grid::Pane`/`Split`의
/// 내부 `usize`는 비공개라 애초에 직렬화할 수 없다. worktree id는 경로에서
/// 나오므로 앱을 껐다 켜도 같다 — 훅 상관관계(`PaneKey`)와 레이아웃 복원이
/// **같은 키**를 쓴다.
///
/// **`suaegi-app`이 아니라 여기 사는 이유**: [`SessionState`]가 이걸 필드로
/// 담고, `SessionState`는 `suaegi-core`의 타입이다. 반대 방향 의존은 없다
/// (`suaegi-app → suaegi-term → suaegi-core`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedPane {
    Split {
        axis: PersistedAxis,
        ratio: f32,
        a: Box<PersistedPane>,
        b: Box<PersistedPane>,
    },
    Leaf(WorktreeId),
}

/// **`Eq`가 없는 이유**: [`PersistedPane`]의 `ratio: f32`가 `Eq`를 막는다
/// (Task 0이 미리 경고해 둔 그대로다). 이를 담는 [`PersistedState`]에서도 같이
/// 뗐다 — `assert_eq!`는 `PartialEq`만 요구하므로 호출부는 그대로 컴파일된다.
///
/// **`SCHEMA_VERSION`은 올리지 않는다.** 영속화 가드가
/// `schema_version > SCHEMA_VERSION`에서만 발동하므로 `#[serde(default)]` 필드
/// 추가는 공짜지만(구버전은 모르는 키를 무시한다), 버전을 올리면 구버전이
/// 가드에 걸려 **저장을 아예 거부한다.**
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    #[serde(default)]
    pub active_worktree_id: Option<WorktreeId>,
    /// 마지막으로 화면에 있던 pane 트리. `None`이면 복원할 레이아웃이 없다
    /// (첫 실행, 또는 세션을 하나도 열지 않고 껐다).
    #[serde(default)]
    pub panes: Option<PersistedPane>,
    /// Provider-owned resume records for agent terminals that Suaegi
    /// intentionally hibernated. No prompt/transcript content is persisted.
    #[serde(default)]
    pub sleeping_agent_sessions: Vec<SleepingAgentSession>,
    /// Restorable in-app browser tabs. Page process state is intentionally
    /// ephemeral, while URL/profile/worktree identity survives relaunch.
    #[serde(default)]
    pub browser_tabs: Vec<BrowserTabSetting>,
    #[serde(default)]
    pub active_browser_tab_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepingAgentSession {
    pub worktree_id: WorktreeId,
    pub agent: String,
    pub provider_session_id: String,
    pub captured_at_unix_ms: u64,
}

/// 부팅 시 Jira 재연결에 필요한 **non-secret** 연결 설정(Plan N2). 토큰은 여기 없다 —
/// `suaegi-secrets` 키체인(service `suaegi-jira`, account=`site_url`)에서 별도로 온다(Orca가
/// `jira-sites.json`을 평문으로, 토큰만 keychain에 두는 것과 동형). `suaegi-core`는
/// `suaegi-tracker`를 모르므로 `JiraAuthType`(Cloud/Server) 대신 `is_cloud: bool`로 평평하게
/// 담는다 — 매핑은 앱 레이어가 한다. **토큰은 절대 이 구조체에 들어가지 않는다.**
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraConnectionConfig {
    /// 정규화된 사이트 URL(예: `https://acme.atlassian.net`), 끝 슬래시 없음. 키체인 account이기도.
    pub site_url: String,
    /// 로그인 이메일(Cloud/Server-Basic). Server PAT면 빈 문자열(→ Bearer).
    pub email: String,
    /// Cloud면 true(`/rest/api/3`, ADF), Server/DC면 false(`/rest/api/2`, plain).
    pub is_cloud: bool,
}

/// A persisted scheduled prompt. Runtime-only execution state stays in the app;
/// these fields are sufficient to deterministically compute the next due run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationConfig {
    pub id: String,
    pub name: String,
    pub worktree_id: WorktreeId,
    pub schedule: String,
    pub prompt: String,
    pub timezone: String,
    #[serde(default = "default_automation_provider")]
    pub provider: String,
    pub dtstart_unix_ms: i64,
    #[serde(default = "automation_enabled_default")]
    pub enabled: bool,
    #[serde(default)]
    pub last_dispatched_unix_ms: Option<i64>,
}

fn default_automation_provider() -> String {
    "claude".to_string()
}

fn automation_enabled_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRunRecord {
    pub id: String,
    pub automation_id: String,
    pub trigger: String,
    pub status: String,
    pub scheduled_for_unix_ms: i64,
    pub started_at_unix_ms: i64,
    #[serde(default)]
    pub finished_at_unix_ms: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserProfileSetting {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserTabSetting {
    pub id: String,
    #[serde(default)]
    pub worktree_id: Option<WorktreeId>,
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_browser_profile_id")]
    pub profile_id: String,
}

fn default_browser_profile_id() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub workspace_root: PathBuf,
    /// Interface preferences shared by the native shell. Keeping these under a
    /// defaulted nested object lets older v1 state files load without a schema
    /// bump while still making settings controls survive restarts.
    #[serde(default)]
    pub ui: UiSettings,
    /// Plan N2: 활성 Jira 연결(있으면). 부팅이 이걸로 클라이언트를 재조립하고 키체인 토큰으로
    /// 재검증한다 — "연결"이 앱 재시작을 넘어 지속되게 한다. **토큰은 절대 여기 없다**(키체인 전용).
    /// **`#[serde(default)]`가 데이터 손실 방어다** — 이 필드 없는 옛 저장본이 `None`으로 조용히
    /// 로드돼야 `Settings` 역직렬화가 실패해 파일 전체가 손상 판정으로 떨어지지 않는다.
    #[serde(default)]
    pub jira_connection: Option<JiraConnectionConfig>,
    /// Scheduled prompts contain no credentials and live with the rest of the
    /// app configuration. Missing in older v1 files means no automations.
    #[serde(default)]
    pub automations: Vec<AutomationConfig>,
    #[serde(default)]
    pub automation_runs: Vec<AutomationRunRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceUserModelSetting {
    pub id: String,
    pub model_type: String,
    pub dir: String,
    pub sample_rate: Option<u32>,
}

impl Default for VoiceUserModelSetting {
    fn default() -> Self {
        Self {
            id: String::new(),
            model_type: "transducer".to_string(),
            dir: String::new(),
            sample_rate: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalCustomThemeSetting {
    pub id: String,
    pub name: String,
    pub source: String,
    pub mode: String,
    pub terminal: HashMap<String, String>,
    pub imported_at: String,
    pub source_label: Option<String>,
    pub unsupported_features: Vec<String>,
}

/// Per-repository worktree hook preferences. The string values intentionally
/// mirror Orca's persisted union values so existing exports remain portable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoHookSetting {
    pub mode: String,
    pub setup_run_policy: String,
    pub setup_agent_startup_policy: String,
    pub command_source_policy: Option<String>,
    pub setup_script: String,
    pub archive_script: String,
}

impl Default for RepoHookSetting {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
            setup_run_policy: "run-by-default".to_string(),
            setup_agent_startup_policy: "start-immediately".to_string(),
            command_source_policy: None,
            setup_script: String::new(),
            archive_script: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparsePresetSetting {
    pub id: String,
    pub name: String,
    pub directories: Vec<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceControlAiActionRecipeSetting {
    pub agent_id: Option<String>,
    pub command_input_template: String,
    pub agent_args: String,
}

impl Default for SourceControlAiActionRecipeSetting {
    fn default() -> Self {
        Self {
            agent_id: None,
            command_input_template: "{basePrompt}".to_string(),
            agent_args: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceControlAiPrCreationDefaults {
    pub draft: bool,
    pub use_template: bool,
    pub generate_details_on_open: bool,
    pub open_after_create: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceControlAiSetting {
    pub enabled: bool,
    pub agent_id: String,
    pub model: String,
    pub thinking_level: String,
    pub custom_agent_command: String,
    pub actions: HashMap<String, SourceControlAiActionRecipeSetting>,
    pub pr_creation_defaults: SourceControlAiPrCreationDefaults,
}

impl Default for SourceControlAiSetting {
    fn default() -> Self {
        Self {
            enabled: true,
            agent_id: "claude".to_string(),
            model: "sonnet".to_string(),
            thinking_level: "low".to_string(),
            custom_agent_command: String::new(),
            actions: [
                "commitMessage",
                "pullRequest",
                "branchName",
                "fixCommitFailure",
                "fixPushFailure",
                "fixChecks",
                "resolveConflicts",
                "resolveComments",
            ]
            .into_iter()
            .map(|id| {
                (
                    id.to_string(),
                    SourceControlAiActionRecipeSetting::default(),
                )
            })
            .collect(),
            pr_creation_defaults: SourceControlAiPrCreationDefaults::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoSourceControlAiPrCreationOverrides {
    pub draft: Option<bool>,
    pub use_template: Option<bool>,
    pub generate_details_on_open: Option<bool>,
    pub open_after_create: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoSourceControlAiSetting {
    pub enabled: Option<bool>,
    pub custom_agent_command: Option<String>,
    pub action_overrides: HashMap<String, SourceControlAiActionRecipeSetting>,
    pub pr_creation_defaults: RepoSourceControlAiPrCreationOverrides,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRepositoryIdentitySetting {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub confirm_close_pinned_tabs: bool,
    pub confirm_close_running_terminal: bool,
    pub confirm_workspace_delete: bool,
    pub confirm_automation_delete: bool,
    pub nest_workspaces: bool,
    pub tab_order_mru: bool,
    pub auto_save_files: bool,
    pub auto_save_delay_ms: u64,
    pub editor_font_family: String,
    pub editor_word_wrap: bool,
    pub editor_minimap: bool,
    pub rich_markdown_spellcheck: bool,
    pub markdown_review_tools: bool,
    pub primary_selection_middle_click_paste: bool,
    pub diff_default_side_by_side: bool,
    pub diff_word_wrap: bool,
    #[serde(default)]
    pub combined_diff_file_tree_visible_by_default: bool,
    pub refresh_local_base_ref: bool,
    pub auto_rename_branch_from_work: bool,
    pub branch_prefix: String,
    pub branch_prefix_custom: String,
    pub enable_github_attribution: bool,
    pub show_git_ignored_files: bool,
    pub source_control_compare_against_upstream: bool,
    pub source_control_view_mode: String,
    #[serde(default = "default_source_control_group_order")]
    pub source_control_group_order: String,
    pub source_control_ai: SourceControlAiSetting,
    pub repo_source_control_ai: HashMap<String, RepoSourceControlAiSetting>,
    pub theme: String,
    pub ui_language: String,
    pub app_font_family: String,
    pub ui_zoom_percent: u16,
    pub app_icon: String,
    pub left_sidebar_appearance: String,
    #[serde(default = "default_left_sidebar_tint_color")]
    pub left_sidebar_tint_color: String,
    #[serde(default = "default_left_sidebar_tint_opacity_percent")]
    pub left_sidebar_tint_opacity_percent: u8,
    pub usage_percentage_mode: String,
    pub show_titlebar_app_name: bool,
    pub show_tasks_button: bool,
    pub show_automations_button: bool,
    #[serde(default)]
    pub show_pinned_worktrees_in_groups: bool,
    pub show_menu_bar_icon: bool,
    pub window_background_blur: bool,
    pub terminal_font_size: u16,
    pub terminal_font_family: String,
    pub terminal_font_weight: u16,
    /// `false` only for settings written before Suaegi matched Orca's macOS
    /// terminal typography defaults. A field-level default is intentional:
    /// `UiSettings::default()` is a new install (`true`), while a missing JSON
    /// key is an older install that still needs the one-time 400 → 500
    /// migration.
    #[serde(default)]
    pub terminal_font_defaults_orca_v2: bool,
    pub terminal_line_height_percent: u16,
    pub terminal_scroll_sensitivity_percent: u16,
    pub terminal_fast_scroll_sensitivity_percent: u16,
    pub terminal_tui_scroll_multiplier: u8,
    pub terminal_gpu_acceleration: String,
    pub terminal_ligatures: String,
    pub terminal_cursor_style: String,
    pub terminal_cursor_blink: bool,
    pub terminal_theme_dark: String,
    pub terminal_use_separate_light_theme: bool,
    pub terminal_theme_light: String,
    pub terminal_custom_themes: Vec<TerminalCustomThemeSetting>,
    pub setup_script_launch_mode: String,
    pub repo_hook_settings: HashMap<String, RepoHookSetting>,
    pub repo_worktree_base_paths: HashMap<String, String>,
    pub repo_symlink_paths: HashMap<String, Vec<String>>,
    pub repo_sparse_presets: HashMap<String, Vec<SparsePresetSetting>>,
    pub repo_badge_colors: HashMap<String, String>,
    pub repo_icons: HashMap<String, String>,
    pub repo_github_upstreams: HashMap<String, GithubRepositoryIdentitySetting>,
    pub repo_fork_sync_modes: HashMap<String, String>,
    pub repo_external_worktree_visibility: HashMap<String, String>,
    pub repo_external_worktree_inbox_baseline_paths: HashMap<String, Vec<String>>,
    pub repo_imported_external_worktree_paths: HashMap<String, Vec<String>>,
    pub repo_external_worktree_discovery_suppressed_at: HashMap<String, u64>,
    /// Additional host-specific checkouts for each logical project. The map
    /// key is the local repository ID; the local checkout itself remains the
    /// implicit `local` setup and is not duplicated here.
    pub repo_host_setups: HashMap<String, Vec<ProjectHostSetupSetting>>,
    #[serde(default)]
    pub terminal_color_overrides: HashMap<String, String>,
    pub terminal_divider_color_dark: String,
    pub terminal_divider_color_light: String,
    pub terminal_inactive_pane_opacity_percent: u8,
    pub terminal_active_pane_opacity_percent: u8,
    pub terminal_pane_opacity_transition_ms: u16,
    pub terminal_divider_thickness_px: u8,
    pub terminal_background_opacity_percent: u8,
    pub terminal_padding_x: u16,
    pub terminal_padding_y: u16,
    pub terminal_mouse_hide_while_typing: bool,
    pub terminal_word_separator: String,
    pub terminal_cursor_opacity_percent: u8,
    pub terminal_focus_follows_mouse: bool,
    pub terminal_clipboard_on_select: bool,
    pub terminal_allow_osc52_clipboard: bool,
    pub terminal_scope_history_by_worktree: bool,
    pub terminal_scrollback_rows: u32,
    pub terminal_shortcut_policy: String,
    pub terminal_right_click_to_paste: bool,
    pub terminal_mac_option_as_alt: String,
    pub terminal_jis_yen_to_backslash: bool,
    pub notifications_enabled: bool,
    pub notification_agent_task_complete: bool,
    pub notification_terminal_bell: bool,
    pub notification_suppress_when_focused: bool,
    pub notification_sound: String,
    pub notification_custom_sound_path: Option<PathBuf>,
    pub notification_volume: u8,
    pub anonymous_telemetry: bool,
    pub show_usage_status: bool,
    #[serde(default)]
    pub claude_usage_enabled: bool,
    #[serde(default)]
    pub codex_usage_enabled: bool,
    #[serde(default)]
    pub opencode_usage_enabled: bool,
    pub show_resource_status: bool,
    pub show_ports_status: bool,
    pub show_mobile_sidebar: bool,
    pub floating_workspace_enabled: bool,
    pub floating_workspace_cwd: String,
    pub floating_workspace_trigger: String,
    pub floating_workspace_panel_x: i32,
    pub floating_workspace_panel_y: i32,
    pub floating_workspace_panel_width: u16,
    pub floating_workspace_panel_height: u16,
    pub floating_workspace_trigger_x: i32,
    pub floating_workspace_trigger_y: i32,
    pub browser_home_page: String,
    pub browser_search_engine: String,
    pub browser_default_zoom_percent: u16,
    pub browser_default_profile_id: String,
    pub browser_profiles: Vec<BrowserProfileSetting>,
    pub open_links_in_app: bool,
    pub localhost_worktree_labels: bool,
    pub mobile_emulator_enabled: bool,
    pub mobile_emulator_default_device_udid: Option<String>,
    pub android_sdk_path: String,
    pub default_agent: String,
    #[serde(default)]
    pub disabled_agents: Vec<String>,
    #[serde(default)]
    pub agent_command_overrides: HashMap<String, String>,
    #[serde(default = "orca_yolo_agent_args")]
    pub agent_default_args: HashMap<String, String>,
    #[serde(default = "orca_yolo_agent_env")]
    pub agent_default_env: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub runtime_worktree_create_receipts: HashMap<String, String>,
    pub agent_status_hooks_enabled: bool,
    pub claude_agent_teams_mode: String,
    pub tab_auto_generate_title: bool,
    pub keep_computer_awake_while_agents_run: bool,
    pub prompt_cache_timer_enabled: bool,
    pub prompt_cache_ttl_minutes: u16,
    pub provider_runtime_scope: String,
    pub codex_managed_accounts: Vec<ManagedProviderAccountSetting>,
    pub active_codex_managed_account_id: Option<String>,
    pub claude_managed_accounts: Vec<ManagedProviderAccountSetting>,
    pub active_claude_managed_account_id: Option<String>,
    pub gemini_cli_oauth_enabled: bool,
    #[serde(default)]
    pub opencode_workspace_id: String,
    #[serde(default)]
    pub minimax_group_id: String,
    #[serde(default = "default_minimax_usage_models")]
    pub minimax_usage_models: String,
    pub orchestration_enabled: bool,
    pub computer_use_enabled: bool,
    pub voice_enabled: bool,
    pub voice_model: String,
    pub voice_models_dir: String,
    pub voice_language: String,
    pub voice_dictation_mode: String,
    pub voice_terminal_confirm_before_insert: bool,
    pub voice_openai_api_key_configured: bool,
    pub voice_user_models: Vec<VoiceUserModelSetting>,
    pub show_github_tasks: bool,
    #[serde(default = "default_true")]
    pub show_gitlab_tasks: bool,
    pub show_jira_tasks: bool,
    pub show_linear_tasks: bool,
    #[serde(default = "default_task_source")]
    pub default_task_source: String,
    #[serde(default = "default_task_view_preset")]
    pub default_task_view_preset: String,
    #[serde(default, deserialize_with = "deserialize_optional_string_vec")]
    pub default_repo_selection: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_string_vec")]
    pub default_linear_team_selection: Option<Vec<String>>,
    pub http_proxy_url: String,
    pub http_proxy_bypass_rules: String,
    pub electron_http1_compatibility_mode: bool,
    pub terminal_hidden_view_parking: bool,
    pub terminal_main_side_effect_authority: bool,
    pub terminal_hidden_delivery_gate: bool,
    pub terminal_model_query_authority: bool,
    pub experimental_native_chat: bool,
    pub open_agent_tabs_in_chat_by_default: bool,
    pub experimental_pet: bool,
    pub experimental_activity: bool,
    pub experimental_terminal_attention: bool,
    pub experimental_agent_hibernation: bool,
    pub agent_hibernation_idle_ms: u64,
    pub compact_worktree_cards: bool,
    pub experimental_ephemeral_vms: bool,
    pub plugin_system_enabled: bool,
    pub disabled_plugins: Vec<String>,
    pub plugin_consents: HashMap<String, String>,
    pub dev_plugin_paths: Vec<String>,
    pub quick_commands: Vec<QuickCommandSetting>,
    pub open_in_applications: Vec<OpenInApplicationSetting>,
    pub ssh_hosts: Vec<SshHostSetting>,
    #[serde(default)]
    pub runtime_environments: Vec<RuntimeEnvironmentSetting>,
    #[serde(default)]
    pub active_runtime_environment_id: Option<String>,
    pub hide_default_branch: bool,
    pub hide_detached_head: bool,
    pub hide_sleeping: bool,
    #[serde(default)]
    pub pinned_worktrees: Vec<String>,
    #[serde(default)]
    pub sleeping_worktrees: Vec<String>,
    #[serde(default)]
    pub unread_worktrees: Vec<String>,
    #[serde(default)]
    pub board_statuses: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub worktree_comments: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub worktree_parents: HashMap<String, String>,
}

pub fn orca_yolo_agent_args() -> HashMap<String, String> {
    [
        ("claude", "--dangerously-skip-permissions"),
        ("openclaude", "--dangerously-skip-permissions"),
        ("codex", "--dangerously-bypass-approvals-and-sandbox"),
        ("gemini", "--yolo"),
        ("antigravity", "--dangerously-skip-permissions"),
        ("aider", "--yes-always"),
        ("amp", "--dangerously-allow-all"),
        ("kiro", "--trust-all-tools"),
        ("crush", "--yolo"),
        ("autohand", "--unrestricted"),
        ("cline", "--auto-approve true"),
        ("command-code", "--yolo"),
        ("continue", "--allow \"*\""),
        ("cursor", "--yolo"),
        ("kimi", "--yolo"),
        ("mistral-vibe", "--agent auto-approve"),
        ("qwen-code", "--approval-mode yolo"),
        ("rovo", "--yolo"),
        ("hermes", "--yolo"),
        ("copilot", "--yolo"),
        ("grok", "--permission-mode bypassPermissions"),
        ("devin", "--permission-mode bypass"),
        ("ante", "--yolo"),
        ("trae", "--yolo"),
    ]
    .into_iter()
    .map(|(agent, args)| (agent.to_string(), args.to_string()))
    .collect()
}

pub fn orca_yolo_agent_env() -> HashMap<String, HashMap<String, String>> {
    HashMap::from([(
        "goose".to_string(),
        HashMap::from([("GOOSE_MODE".to_string(), "auto".to_string())]),
    )])
}

fn default_task_source() -> String {
    "github".to_string()
}

fn default_task_view_preset() -> String {
    "all".to_string()
}

fn default_source_control_group_order() -> String {
    "changes-first".to_string()
}

fn default_minimax_usage_models() -> String {
    "general".to_string()
}

fn default_left_sidebar_tint_color() -> String {
    "#18181b".to_string()
}

fn default_left_sidebar_tint_opacity_percent() -> u8 {
    8
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            confirm_close_pinned_tabs: true,
            confirm_close_running_terminal: true,
            confirm_workspace_delete: true,
            confirm_automation_delete: true,
            nest_workspaces: true,
            tab_order_mru: true,
            auto_save_files: false,
            auto_save_delay_ms: 800,
            editor_font_family: String::new(),
            editor_word_wrap: true,
            editor_minimap: false,
            rich_markdown_spellcheck: true,
            markdown_review_tools: true,
            primary_selection_middle_click_paste: true,
            diff_default_side_by_side: false,
            diff_word_wrap: false,
            combined_diff_file_tree_visible_by_default: false,
            refresh_local_base_ref: false,
            auto_rename_branch_from_work: true,
            branch_prefix: "git-username".to_string(),
            branch_prefix_custom: String::new(),
            enable_github_attribution: false,
            show_git_ignored_files: true,
            source_control_compare_against_upstream: false,
            source_control_view_mode: "list".to_string(),
            source_control_group_order: default_source_control_group_order(),
            source_control_ai: SourceControlAiSetting::default(),
            repo_source_control_ai: HashMap::new(),
            theme: "system".to_string(),
            ui_language: "system".to_string(),
            app_font_family: "system".to_string(),
            ui_zoom_percent: 100,
            app_icon: "classic".to_string(),
            left_sidebar_appearance: "default".to_string(),
            left_sidebar_tint_color: default_left_sidebar_tint_color(),
            left_sidebar_tint_opacity_percent: default_left_sidebar_tint_opacity_percent(),
            usage_percentage_mode: "used".to_string(),
            show_titlebar_app_name: true,
            show_tasks_button: true,
            show_automations_button: true,
            show_pinned_worktrees_in_groups: false,
            show_menu_bar_icon: true,
            window_background_blur: false,
            terminal_font_size: 14,
            terminal_font_family: "SF Mono".to_string(),
            terminal_font_weight: 500,
            terminal_font_defaults_orca_v2: true,
            terminal_line_height_percent: 100,
            terminal_scroll_sensitivity_percent: 115,
            terminal_fast_scroll_sensitivity_percent: 500,
            terminal_tui_scroll_multiplier: 1,
            terminal_gpu_acceleration: "auto".to_string(),
            terminal_ligatures: "auto".to_string(),
            terminal_cursor_style: "block".to_string(),
            terminal_cursor_blink: true,
            terminal_theme_dark: "Ghostty Default Style Dark".to_string(),
            terminal_use_separate_light_theme: true,
            terminal_theme_light: "Builtin Tango Light".to_string(),
            terminal_custom_themes: Vec::new(),
            setup_script_launch_mode: "new-tab".to_string(),
            repo_hook_settings: HashMap::new(),
            repo_worktree_base_paths: HashMap::new(),
            repo_symlink_paths: HashMap::new(),
            repo_sparse_presets: HashMap::new(),
            repo_badge_colors: HashMap::new(),
            repo_icons: HashMap::new(),
            repo_github_upstreams: HashMap::new(),
            repo_fork_sync_modes: HashMap::new(),
            repo_external_worktree_visibility: HashMap::new(),
            repo_external_worktree_inbox_baseline_paths: HashMap::new(),
            repo_imported_external_worktree_paths: HashMap::new(),
            repo_external_worktree_discovery_suppressed_at: HashMap::new(),
            repo_host_setups: HashMap::new(),
            terminal_color_overrides: HashMap::new(),
            terminal_divider_color_dark: "#3f3f46".to_string(),
            terminal_divider_color_light: "#d4d4d8".to_string(),
            terminal_inactive_pane_opacity_percent: 80,
            terminal_active_pane_opacity_percent: 100,
            terminal_pane_opacity_transition_ms: 140,
            terminal_divider_thickness_px: 3,
            terminal_background_opacity_percent: 100,
            terminal_padding_x: 4,
            terminal_padding_y: 4,
            terminal_mouse_hide_while_typing: false,
            terminal_word_separator: " ()[]{},'\"`".to_string(),
            terminal_cursor_opacity_percent: 100,
            terminal_focus_follows_mouse: false,
            terminal_clipboard_on_select: false,
            terminal_allow_osc52_clipboard: false,
            terminal_scope_history_by_worktree: true,
            terminal_scrollback_rows: 10_000,
            terminal_shortcut_policy: "orca-first".to_string(),
            terminal_right_click_to_paste: false,
            terminal_mac_option_as_alt: "auto".to_string(),
            terminal_jis_yen_to_backslash: false,
            notifications_enabled: true,
            notification_agent_task_complete: true,
            notification_terminal_bell: true,
            notification_suppress_when_focused: true,
            notification_sound: "system".to_string(),
            notification_custom_sound_path: None,
            notification_volume: 100,
            anonymous_telemetry: false,
            show_usage_status: true,
            claude_usage_enabled: false,
            codex_usage_enabled: false,
            opencode_usage_enabled: false,
            show_resource_status: true,
            show_ports_status: true,
            show_mobile_sidebar: true,
            floating_workspace_enabled: true,
            floating_workspace_cwd: "~".to_string(),
            floating_workspace_trigger: "floating-button".to_string(),
            floating_workspace_panel_x: -1,
            floating_workspace_panel_y: -1,
            floating_workspace_panel_width: 650,
            floating_workspace_panel_height: 400,
            floating_workspace_trigger_x: -1,
            floating_workspace_trigger_y: -1,
            browser_home_page: "about:blank".to_string(),
            browser_search_engine: "google".to_string(),
            browser_default_zoom_percent: 100,
            browser_default_profile_id: "default".to_string(),
            browser_profiles: Vec::new(),
            open_links_in_app: false,
            localhost_worktree_labels: false,
            mobile_emulator_enabled: true,
            mobile_emulator_default_device_udid: None,
            android_sdk_path: String::new(),
            default_agent: "auto".to_string(),
            disabled_agents: Vec::new(),
            agent_command_overrides: HashMap::new(),
            agent_default_args: orca_yolo_agent_args(),
            agent_default_env: orca_yolo_agent_env(),
            runtime_worktree_create_receipts: HashMap::new(),
            agent_status_hooks_enabled: true,
            claude_agent_teams_mode: "off".to_string(),
            tab_auto_generate_title: false,
            keep_computer_awake_while_agents_run: false,
            prompt_cache_timer_enabled: false,
            prompt_cache_ttl_minutes: 5,
            provider_runtime_scope: "host".to_string(),
            codex_managed_accounts: Vec::new(),
            active_codex_managed_account_id: None,
            claude_managed_accounts: Vec::new(),
            active_claude_managed_account_id: None,
            gemini_cli_oauth_enabled: false,
            opencode_workspace_id: String::new(),
            minimax_group_id: String::new(),
            minimax_usage_models: default_minimax_usage_models(),
            orchestration_enabled: false,
            computer_use_enabled: false,
            voice_enabled: false,
            voice_model: String::new(),
            voice_models_dir: String::new(),
            voice_language: "en".to_string(),
            voice_dictation_mode: "toggle".to_string(),
            voice_terminal_confirm_before_insert: false,
            voice_openai_api_key_configured: false,
            voice_user_models: Vec::new(),
            show_github_tasks: true,
            show_gitlab_tasks: true,
            show_jira_tasks: true,
            show_linear_tasks: true,
            default_task_source: default_task_source(),
            default_task_view_preset: default_task_view_preset(),
            default_repo_selection: None,
            default_linear_team_selection: None,
            http_proxy_url: String::new(),
            http_proxy_bypass_rules: String::new(),
            electron_http1_compatibility_mode: false,
            terminal_hidden_view_parking: true,
            terminal_main_side_effect_authority: true,
            terminal_hidden_delivery_gate: true,
            terminal_model_query_authority: true,
            experimental_native_chat: false,
            open_agent_tabs_in_chat_by_default: false,
            experimental_pet: false,
            experimental_activity: false,
            experimental_terminal_attention: false,
            experimental_agent_hibernation: false,
            agent_hibernation_idle_ms: 30 * 60 * 1_000,
            compact_worktree_cards: false,
            experimental_ephemeral_vms: false,
            plugin_system_enabled: false,
            disabled_plugins: Vec::new(),
            plugin_consents: HashMap::new(),
            dev_plugin_paths: Vec::new(),
            quick_commands: vec![
                QuickCommandSetting {
                    id: "git-status".to_string(),
                    label: "Git status".to_string(),
                    command: "git status".to_string(),
                    append_enter: true,
                },
                QuickCommandSetting {
                    id: "list-files".to_string(),
                    label: "List files".to_string(),
                    command: "ls -la".to_string(),
                    append_enter: true,
                },
            ],
            open_in_applications: vec![OpenInApplicationSetting {
                id: "vscode".to_string(),
                label: "VS Code".to_string(),
                command: "code".to_string(),
            }],
            ssh_hosts: Vec::new(),
            runtime_environments: Vec::new(),
            active_runtime_environment_id: None,
            hide_default_branch: false,
            hide_detached_head: false,
            hide_sleeping: false,
            pinned_worktrees: Vec::new(),
            sleeping_worktrees: Vec::new(),
            unread_worktrees: Vec::new(),
            board_statuses: HashMap::new(),
            worktree_comments: HashMap::new(),
            worktree_parents: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickCommandSetting {
    pub id: String,
    pub label: String,
    pub command: String,
    pub append_enter: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenInApplicationSetting {
    pub id: String,
    pub label: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshHostSetting {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub config_host: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_file: String,
    #[serde(default)]
    pub proxy_command: String,
    #[serde(default)]
    pub jump_host: String,
    #[serde(default = "default_true")]
    pub system_ssh_connection_reuse: bool,
    #[serde(default)]
    pub relay_grace_period_seconds: u32,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHostSetupSetting {
    pub id: String,
    pub host_id: String,
    pub path: String,
    pub display_name: String,
    #[serde(default)]
    pub worktree_base_path: String,
    #[serde(default)]
    pub git_username: String,
    /// `git` or `folder`.
    pub kind: String,
    /// Orca-compatible lifecycle value: `ready`, `not-set-up`,
    /// `setting-up`, `error`, or `unsupported`.
    pub setup_state: String,
    /// `imported-existing-folder`, `cloned`, or `provisioned`.
    pub setup_method: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEnvironmentSetting {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub credentials_configured: bool,
    #[serde(default)]
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProviderAccountSetting {
    pub id: String,
    pub email: String,
    pub config_dir: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub last_authenticated_at_unix_ms: u64,
}

impl Settings {
    pub fn default_with_home(home: &Path) -> Self {
        Self {
            workspace_root: home.join("suaegi-workspaces"),
            ui: UiSettings::default(),
            jira_connection: None,
            automations: Vec::new(),
            automation_runs: Vec::new(),
        }
    }
}

/// `Eq`가 없는 이유는 [`SessionState`] 참고(`PersistedPane::Split::ratio`가 f32다).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    pub schema_version: u32,
    #[serde(default)]
    pub repos: Vec<Repo>,
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
    #[serde(default)]
    pub session: SessionState,
    pub settings: Settings,
}

impl Default for PersistedState {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            schema_version: SCHEMA_VERSION,
            repos: Vec::new(),
            worktrees: Vec::new(),
            session: SessionState::default(),
            settings: Settings::default_with_home(&home),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn terminal_typography_defaults_match_orca_and_detect_legacy_json() {
        let current = UiSettings::default();
        assert_eq!(current.terminal_font_family, "SF Mono");
        assert_eq!(current.terminal_font_weight, 500);
        assert!(current.terminal_font_defaults_orca_v2);

        let mut legacy = serde_json::to_value(current).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("terminal_font_defaults_orca_v2");
        legacy["terminal_font_weight"] = serde_json::json!(400);
        let legacy: UiSettings = serde_json::from_value(legacy).unwrap();
        assert!(
            !legacy.terminal_font_defaults_orca_v2,
            "a missing marker must identify settings written by the old renderer"
        );
        assert_eq!(legacy.terminal_font_weight, 400);
    }

    #[test]
    fn persisted_state_round_trips_through_json() {
        let state = PersistedState {
            schema_version: SCHEMA_VERSION,
            repos: vec![Repo {
                id: RepoId("/tmp/demo".into()),
                path: PathBuf::from("/tmp/demo"),
                display_name: "demo".into(),
                worktree_base_ref: Some("main".into()),
            }],
            worktrees: vec![Worktree {
                id: WorktreeId("/tmp/ws/demo/fix-bug".into()),
                repo_id: RepoId("/tmp/demo".into()),
                path: PathBuf::from("/tmp/ws/demo/fix-bug"),
                branch: "fix-bug".into(),
                display_name: "fix-bug".into(),
                created_with_agent: Some("claude".into()),
                created_at_unix_ms: 1_700_000_000_000,
                linked_github_pr: Some(42),
                linked_linear_issue: Some("ENG-123".into()),
                linked_linear_issue_workspace_id: Some("org-1".into()),
                linked_linear_issue_organization_url_key: Some("acme".into()),
                linked_jira_issue: Some("PROJ-99".into()),
                linked_jira_site: Some("https://acme.atlassian.net".into()),
            }],
            session: SessionState {
                active_worktree_id: Some(WorktreeId("/tmp/ws/demo/fix-bug".into())),
                panes: Some(PersistedPane::Split {
                    axis: PersistedAxis::Vertical,
                    ratio: 0.375,
                    a: Box::new(PersistedPane::Leaf(WorktreeId("/tmp/ws/demo/a".into()))),
                    b: Box::new(PersistedPane::Split {
                        axis: PersistedAxis::Horizontal,
                        ratio: 0.5,
                        a: Box::new(PersistedPane::Leaf(WorktreeId("/tmp/ws/demo/b".into()))),
                        b: Box::new(PersistedPane::Leaf(WorktreeId("/tmp/ws/demo/c".into()))),
                    }),
                }),
                sleeping_agent_sessions: Vec::new(),
                browser_tabs: Vec::new(),
                active_browser_tab_id: None,
            },
            settings: Settings {
                workspace_root: PathBuf::from("/tmp/ws"),
                ui: UiSettings::default(),
                jira_connection: Some(JiraConnectionConfig {
                    site_url: "https://acme.atlassian.net".into(),
                    email: "ada@acme.com".into(),
                    is_cloud: true,
                }),
                automations: Vec::new(),
                automation_runs: Vec::new(),
            },
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn default_state_is_empty_with_current_schema() {
        let d = PersistedState::default();
        assert_eq!(d.schema_version, SCHEMA_VERSION);
        assert!(d.repos.is_empty() && d.worktrees.is_empty());
        assert_eq!(d.session, SessionState::default());
        assert!(d.settings.automations.is_empty());
    }

    #[test]
    fn settings_written_before_automations_still_load_with_an_empty_list() {
        let legacy = r#"{
            "schema_version": 1,
            "settings": { "workspace_root": "/tmp/ws" }
        }"#;
        let state: PersistedState = serde_json::from_str(legacy).unwrap();
        assert!(state.settings.automations.is_empty());
        assert_eq!(state.settings.ui, UiSettings::default());
    }

    /// Plan 5의 하드 제약 하나: **레이아웃 필드를 더하면서 `SCHEMA_VERSION`을
    /// 올리지 않는다.** 영속화 가드가 `schema_version > SCHEMA_VERSION`에서만
    /// 발동하므로, 값이 1로 남아 있는 한 구버전 앱도 이 파일을 열어 계속 저장할
    /// 수 있다. 올리는 순간 구버전은 **저장을 아예 거부한다** — 그 회귀를
    /// 컴파일이 아니라 테스트로 잡는다(상수 변경은 조용히 통과하기 때문).
    #[test]
    fn adding_the_layout_field_did_not_bump_the_schema_version() {
        assert_eq!(
            SCHEMA_VERSION, 1,
            "bumping this makes every older build refuse to save at all"
        );
        let json = serde_json::to_string(&PersistedState::default()).unwrap();
        let probe: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            probe["schema_version"], 1,
            "what we actually write must still be readable by a build that only knows v1"
        );
    }

    /// Plan 4 이전 빌드가 쓴 파일에는 `panes` 키가 아예 없다. `#[serde(default)]`가
    /// 그걸 `None`으로 채워야 한다 — 아니면 기존 사용자의 파일이 전부 손상으로
    /// 판정돼 백업 폴백으로 떨어진다.
    #[test]
    fn a_file_written_before_layout_persistence_still_loads() {
        let legacy = r#"{
            "schema_version": 1,
            "session": { "active_worktree_id": "/tmp/ws/demo/fix-bug" },
            "settings": { "workspace_root": "/tmp/ws" }
        }"#;
        let state: PersistedState =
            serde_json::from_str(legacy).expect("a pre-Plan-5 file must still parse");
        assert_eq!(
            state.session.panes, None,
            "a missing layout means 'nothing to restore', not a parse failure"
        );
        // 대조군: 같은 역직렬화가 실제로 내용을 읽고 있다는 것 — 위의 None이
        // "전부 기본값으로 뭉갰다"로도 설명되면 안 된다.
        assert_eq!(
            state.session.active_worktree_id,
            Some(WorktreeId("/tmp/ws/demo/fix-bug".into())),
            "control: the fields that WERE present must have been read"
        );
        assert_eq!(state.settings.workspace_root, PathBuf::from("/tmp/ws"));
    }

    /// 6c 이전 빌드가 쓴 worktree 객체에는 `created_with_agent`/`created_at_unix_ms`
    /// 키가 아예 없다. `#[serde(default)]`가 그걸 `None`/`0`으로 채워야 한다 —
    /// 아니면 이 worktree 하나의 파싱 실패가 저장 파일 전체를 손상으로 판정해
    /// 백업 폴백으로 떨어뜨린다(data-loss 등급).
    #[test]
    fn a_worktree_written_before_the_agent_fields_still_loads() {
        let legacy = r#"{
            "id": "/tmp/ws/demo/fix-bug",
            "repo_id": "/tmp/demo",
            "path": "/tmp/ws/demo/fix-bug",
            "branch": "fix-bug",
            "display_name": "fix-bug"
        }"#;
        let wt: Worktree =
            serde_json::from_str(legacy).expect("a pre-6c worktree object must still parse");
        assert_eq!(
            wt.created_with_agent, None,
            "a missing agent key means 'login shell', not a parse failure"
        );
        assert_eq!(
            wt.created_at_unix_ms, 0,
            "a missing timestamp key must default to 0, not fail the whole file"
        );
        // 대조군: 존재하던 필드는 실제로 읽혔다 — 위의 default가 "전부 기본값으로
        // 뭉갰다"로도 설명되면 안 된다.
        assert_eq!(wt.branch, "fix-bug", "control: present fields must be read");
        assert_eq!(wt.repo_id, RepoId("/tmp/demo".into()));
    }

    /// Plan 7a 이전 빌드가 쓴 worktree 객체에는 `linked_github_pr` 키가 아예 없다.
    /// `#[serde(default)]`가 그걸 `None`으로 채워야 한다 — 아니면 이 worktree 하나의
    /// 파싱 실패가 저장 파일 전체를 손상으로 판정해 백업 폴백으로 떨어뜨린다(data-loss 등급).
    #[test]
    fn a_worktree_written_before_the_linked_pr_field_still_loads() {
        let legacy = r#"{
            "id": "/tmp/ws/demo/fix-bug",
            "repo_id": "/tmp/demo",
            "path": "/tmp/ws/demo/fix-bug",
            "branch": "fix-bug",
            "display_name": "fix-bug",
            "created_with_agent": "claude",
            "created_at_unix_ms": 1700000000000
        }"#;
        let wt: Worktree =
            serde_json::from_str(legacy).expect("a pre-7a worktree object must still parse");
        assert_eq!(
            wt.linked_github_pr, None,
            "a missing linked_github_pr key means 'no PR linked', not a parse failure"
        );
        // 대조군: 존재하던 필드는 실제로 읽혔다.
        assert_eq!(wt.branch, "fix-bug", "control: present fields must be read");
        assert_eq!(wt.created_with_agent, Some("claude".to_string()));
    }

    /// `linked_github_pr`가 있는 값이 JSON을 왕복해도 보존돼야 한다(재해석의 근거).
    #[test]
    fn linked_github_pr_round_trips() {
        let wt = Worktree {
            id: WorktreeId("/tmp/ws/demo/fix-bug".into()),
            repo_id: RepoId("/tmp/demo".into()),
            path: PathBuf::from("/tmp/ws/demo/fix-bug"),
            branch: "fix-bug".into(),
            display_name: "fix-bug".into(),
            created_with_agent: None,
            created_at_unix_ms: 0,
            linked_github_pr: Some(1234),
            linked_linear_issue: None,
            linked_linear_issue_workspace_id: None,
            linked_linear_issue_organization_url_key: None,
            linked_jira_issue: None,
            linked_jira_site: None,
        };
        let json = serde_json::to_string(&wt).unwrap();
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back.linked_github_pr, Some(1234));
    }

    /// Plan N1 이전 빌드가 쓴 worktree 객체에는 `linked_linear_issue*` 세 키가 아예 없다.
    /// `#[serde(default)]`가 그걸 `None`으로 채워야 한다 — 아니면 이 worktree 하나의 파싱
    /// 실패가 저장 파일 전체를 손상으로 판정해 백업 폴백으로 떨어뜨린다(data-loss 등급,
    /// `a_worktree_written_before_the_linked_pr_field_still_loads` 미러).
    #[test]
    fn a_worktree_written_before_the_linear_link_fields_still_loads() {
        let legacy = r#"{
            "id": "/tmp/ws/demo/fix-bug",
            "repo_id": "/tmp/demo",
            "path": "/tmp/ws/demo/fix-bug",
            "branch": "fix-bug",
            "display_name": "fix-bug",
            "created_with_agent": "claude",
            "created_at_unix_ms": 1700000000000,
            "linked_github_pr": 42
        }"#;
        let wt: Worktree =
            serde_json::from_str(legacy).expect("a pre-N1 worktree object must still parse");
        assert_eq!(
            wt.linked_linear_issue, None,
            "a missing linked_linear_issue key means 'no issue linked', not a parse failure"
        );
        assert_eq!(wt.linked_linear_issue_workspace_id, None);
        assert_eq!(wt.linked_linear_issue_organization_url_key, None);
        // N2도 같은 등급: Jira 링크 두 키가 없어도 `None`으로 읽힌다(파싱 실패 아님).
        assert_eq!(
            wt.linked_jira_issue, None,
            "a missing linked_jira_issue key means 'no issue linked', not a parse failure"
        );
        assert_eq!(wt.linked_jira_site, None);
        // 대조군: 존재하던 필드는 실제로 읽혔다 — 위의 default가 "전부 기본값으로 뭉갰다"로도
        // 설명되면 안 된다.
        assert_eq!(wt.branch, "fix-bug", "control: present fields must be read");
        assert_eq!(wt.linked_github_pr, Some(42));
    }

    /// Linear 링크 세 필드가 JSON을 왕복해도 보존돼야 한다(딥링크·재연결의 근거).
    #[test]
    fn linear_link_fields_round_trip() {
        let wt = Worktree {
            id: WorktreeId("/tmp/ws/demo/fix-bug".into()),
            repo_id: RepoId("/tmp/demo".into()),
            path: PathBuf::from("/tmp/ws/demo/fix-bug"),
            branch: "fix-bug".into(),
            display_name: "fix-bug".into(),
            created_with_agent: None,
            created_at_unix_ms: 0,
            linked_github_pr: None,
            linked_linear_issue: Some("ENG-7".into()),
            linked_linear_issue_workspace_id: Some("org-9".into()),
            linked_linear_issue_organization_url_key: Some("acme".into()),
            linked_jira_issue: None,
            linked_jira_site: None,
        };
        let json = serde_json::to_string(&wt).unwrap();
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back.linked_linear_issue.as_deref(), Some("ENG-7"));
        assert_eq!(
            back.linked_linear_issue_workspace_id.as_deref(),
            Some("org-9")
        );
        assert_eq!(
            back.linked_linear_issue_organization_url_key.as_deref(),
            Some("acme")
        );
    }

    /// Jira 링크 두 필드가 JSON을 왕복해도 보존돼야 한다(딥링크·재연결의 근거, N1 미러).
    #[test]
    fn jira_link_fields_round_trip() {
        let wt = Worktree {
            id: WorktreeId("/tmp/ws/demo/fix-bug".into()),
            repo_id: RepoId("/tmp/demo".into()),
            path: PathBuf::from("/tmp/ws/demo/fix-bug"),
            branch: "fix-bug".into(),
            display_name: "fix-bug".into(),
            created_with_agent: None,
            created_at_unix_ms: 0,
            linked_github_pr: None,
            linked_linear_issue: None,
            linked_linear_issue_workspace_id: None,
            linked_linear_issue_organization_url_key: None,
            linked_jira_issue: Some("PROJ-123".into()),
            linked_jira_site: Some("https://acme.atlassian.net".into()),
        };
        let json = serde_json::to_string(&wt).unwrap();
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back.linked_jira_issue.as_deref(), Some("PROJ-123"));
        assert_eq!(
            back.linked_jira_site.as_deref(),
            Some("https://acme.atlassian.net")
        );
    }

    /// N2 이전 빌드가 쓴 `Settings`에는 `jira_connection` 키가 아예 없다. `#[serde(default)]`가
    /// 그걸 `None`으로 채워야 한다 — 아니면 `Settings` 역직렬화가 실패해 파일 전체가 손상으로
    /// 판정돼 백업 폴백으로 떨어진다(data-loss 등급).
    #[test]
    fn settings_written_before_the_jira_connection_field_still_loads() {
        let legacy = r#"{ "workspace_root": "/tmp/ws" }"#;
        let settings: Settings =
            serde_json::from_str(legacy).expect("a pre-N2 settings object must still parse");
        assert_eq!(
            settings.jira_connection, None,
            "a missing jira_connection key means 'not connected', not a parse failure"
        );
        // 대조군: 존재하던 필드는 실제로 읽혔다.
        assert_eq!(settings.workspace_root, PathBuf::from("/tmp/ws"));
    }

    /// Jira 연결 설정이 JSON을 왕복해도 보존돼야 한다(부팅 재연결의 근거). **토큰은 이 구조체에
    /// 없다** — site/email/is_cloud만 담긴다(키체인 account를 짚고 클라이언트를 재조립하는 데 충분).
    #[test]
    fn jira_connection_config_round_trips() {
        let cfg = JiraConnectionConfig {
            site_url: "https://acme.atlassian.net".into(),
            email: "ada@acme.com".into(),
            is_cloud: true,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: JiraConnectionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        // 이 구조체가 실수로 토큰류 필드를 얻지 않았는지 확인(non-secret 계약).
        assert!(
            !json.to_lowercase().contains("token") && !json.to_lowercase().contains("secret"),
            "the persisted Jira connection config must never carry a credential: {json}"
        );
    }

    #[test]
    fn default_settings_places_workspace_root_under_home() {
        let s = Settings::default_with_home(&PathBuf::from("/home/u"));
        assert_eq!(s.workspace_root, PathBuf::from("/home/u/suaegi-workspaces"));
    }

    #[test]
    fn task_project_defaults_accept_legacy_strings_and_current_arrays() {
        let mut value = serde_json::to_value(UiSettings::default()).unwrap();
        value["default_repo_selection"] = serde_json::json!("/tmp/legacy");
        value["default_linear_team_selection"] = serde_json::json!(["team-one", "team-two"]);

        let settings: UiSettings = serde_json::from_value(value).unwrap();
        assert_eq!(
            settings.default_repo_selection,
            Some(vec!["/tmp/legacy".to_string()])
        );
        assert_eq!(
            settings.default_linear_team_selection,
            Some(vec!["team-one".to_string(), "team-two".to_string()])
        );

        let encoded = serde_json::to_value(settings).unwrap();
        assert_eq!(
            encoded["default_repo_selection"],
            serde_json::json!(["/tmp/legacy"])
        );
    }

    #[test]
    fn repo_from_path_canonicalizes_id() {
        let dir = tempfile::tempdir().unwrap();
        // 상대 경로 요소가 섞여도 동일 디렉토리는 동일 ID가 되어야 한다
        let messy = dir.path().join("sub").join("..");
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let a = Repo::from_path(dir.path()).unwrap();
        let b = Repo::from_path(&messy).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.path, b.path);
        assert!(!a.display_name.is_empty());
    }
}
