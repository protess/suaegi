use iced::widget::{
    button, column, container, image, pick_list, row, rule, scrollable, stack, text_input, Space,
};
use iced::{Alignment, Element, Length, Padding};

use crate::i18n::text;
use crate::icons::{self, Icon};
use crate::sidebar;
use crate::state::{
    AppState, HelpAction, Message, SecretDraft, SettingsSection, SourceControlAiPrDefault,
    UiChoice, UiSetting, UiTextSetting, VoiceDictationState,
};
use crate::theme;

const SIDEBAR_WIDTH: f32 = 207.0;
const CONTENT_WIDTH: f32 = 620.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForkSyncChoice(&'static str);

impl ForkSyncChoice {
    const ASK: Self = Self("ask");
    const SAFE_AUTO: Self = Self("safe-auto");
    const OFF: Self = Self("off");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExternalWorktreeChoice(&'static str);

impl ExternalWorktreeChoice {
    const HIDE: Self = Self("hide");
    const SHOW: Self = Self("show");
}

impl std::fmt::Display for ExternalWorktreeChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(if self.0 == "show" {
            "Show in sidebar"
        } else {
            "Hide from sidebar"
        })
    }
}

impl std::fmt::Display for ForkSyncChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self.0 {
            "safe-auto" => "Safe Auto",
            "off" => "Off",
            _ => "Ask",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostSetupChoice {
    id: String,
    label: String,
}

impl std::fmt::Display for HostSetupChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label)
    }
}

pub fn search_input_id() -> iced::widget::Id {
    iced::widget::Id::new("settings-search")
}

struct NavItem {
    section: SettingsSection,
    label: &'static str,
    icon: Icon,
    badge: Option<&'static str>,
}

const AI_CAPABILITIES: &[NavItem] = &[
    NavItem {
        section: SettingsSection::Agents,
        label: "Agents",
        icon: Icon::Bot,
        badge: None,
    },
    NavItem {
        section: SettingsSection::ProviderAccounts,
        label: "AI Provider Accounts",
        icon: Icon::Settings,
        badge: Some("OPTIONAL"),
    },
    NavItem {
        section: SettingsSection::Orchestration,
        label: "Orchestration",
        icon: Icon::ListChecks,
        badge: None,
    },
    NavItem {
        section: SettingsSection::ComputerUse,
        label: "Computer Use",
        icon: Icon::PanelRight,
        badge: None,
    },
    NavItem {
        section: SettingsSection::Voice,
        label: "Voice",
        icon: Icon::CircleHelp,
        badge: None,
    },
];

const SET_UP: &[NavItem] = &[
    NavItem {
        section: SettingsSection::General,
        label: "General",
        icon: Icon::Settings,
        badge: None,
    },
    NavItem {
        section: SettingsSection::Integrations,
        label: "Integrations",
        icon: Icon::Files,
        badge: None,
    },
];

const WORKFLOWS: &[NavItem] = &[
    NavItem {
        section: SettingsSection::Git,
        label: "Git & Source Control",
        icon: Icon::GitBranch,
        badge: None,
    },
    NavItem {
        section: SettingsSection::TaskSources,
        label: "Task Sources",
        icon: Icon::ListChecks,
        badge: None,
    },
    NavItem {
        section: SettingsSection::Terminal,
        label: "Terminal",
        icon: Icon::PanelRight,
        badge: None,
    },
    NavItem {
        section: SettingsSection::QuickCommands,
        label: "Quick Commands",
        icon: Icon::ArrowRight,
        badge: None,
    },
    NavItem {
        section: SettingsSection::Browser,
        label: "Browser",
        icon: Icon::Search,
        badge: None,
    },
    NavItem {
        section: SettingsSection::MobileEmulator,
        label: "Mobile Emulator",
        icon: Icon::Smartphone,
        badge: None,
    },
    NavItem {
        section: SettingsSection::FloatingWorkspace,
        label: "Floating Workspace",
        icon: Icon::PanelLeft,
        badge: None,
    },
];

const INTERFACE: &[NavItem] = &[
    NavItem {
        section: SettingsSection::Appearance,
        label: "Appearance",
        icon: Icon::CircleHelp,
        badge: None,
    },
    NavItem {
        section: SettingsSection::InputEditing,
        label: "Input & Editing",
        icon: Icon::Settings,
        badge: None,
    },
    NavItem {
        section: SettingsSection::Notifications,
        label: "Notifications",
        icon: Icon::CircleHelp,
        badge: None,
    },
    NavItem {
        section: SettingsSection::Shortcuts,
        label: "Shortcuts",
        icon: Icon::ClipboardList,
        badge: None,
    },
    NavItem {
        section: SettingsSection::StatsUsage,
        label: "Stats & Usage",
        icon: Icon::ListChecks,
        badge: None,
    },
];

const REMOTE_HOSTS: &[NavItem] = &[
    NavItem {
        section: SettingsSection::SshHosts,
        label: "SSH Hosts",
        icon: Icon::Files,
        badge: None,
    },
    NavItem {
        section: SettingsSection::RemoteServers,
        label: "Remote Suaegi Servers",
        icon: Icon::PanelRight,
        badge: Some("BETA"),
    },
];

const PRIVACY_SECURITY: &[NavItem] = &[
    NavItem {
        section: SettingsSection::MacPermissions,
        label: "macOS Permissions",
        icon: Icon::Settings,
        badge: None,
    },
    NavItem {
        section: SettingsSection::Privacy,
        label: "Privacy & Telemetry",
        icon: Icon::CircleHelp,
        badge: None,
    },
];

const ADVANCED: &[NavItem] = &[NavItem {
    section: SettingsSection::Advanced,
    label: "Advanced",
    icon: Icon::Settings,
    badge: None,
}];

const EXPERIMENTAL: &[NavItem] = &[
    NavItem {
        section: SettingsSection::Plugins,
        label: "Plugins",
        icon: Icon::Files,
        badge: Some("EXPERIMENTAL"),
    },
    NavItem {
        section: SettingsSection::EphemeralVms,
        label: "Ephemeral VMs",
        icon: Icon::PanelRight,
        badge: Some("EXPERIMENTAL"),
    },
    NavItem {
        section: SettingsSection::Experimental,
        label: "Experimental",
        icon: Icon::Settings,
        badge: None,
    },
];

pub fn view(state: &AppState) -> Element<'_, Message> {
    crate::i18n::set_language(&state.ui_settings().ui_language);
    row![settings_sidebar(state), settings_content(state)]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn settings_sidebar(state: &AppState) -> Element<'_, Message> {
    let query = state.settings_search_query().trim().to_lowercase();
    let mut navigation = column![].spacing(11);
    for (title, items) in [
        ("AI CAPABILITIES", AI_CAPABILITIES),
        ("SET UP", SET_UP),
        ("WORKFLOWS", WORKFLOWS),
        ("INTERFACE", INTERFACE),
        ("REMOTE HOSTS", REMOTE_HOSTS),
        ("PRIVACY & SECURITY", PRIVACY_SECURITY),
        ("ADVANCED", ADVANCED),
        ("EXPERIMENTAL", EXPERIMENTAL),
    ] {
        let visible: Vec<&NavItem> = items
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.label.to_lowercase().contains(&query)
                    || section_search_text(item.section).contains(&query)
            })
            .collect();
        if visible.is_empty() {
            continue;
        }
        let mut group = column![text(title).size(11).color(theme::MUTED)].spacing(2);
        for item in visible {
            let capability_status = match item.section {
                SettingsSection::Orchestration => {
                    Some(if installed_agent_skill("orchestration").is_some() {
                        "INSTALLED"
                    } else {
                        "NOT INSTALLED"
                    })
                }
                SettingsSection::ComputerUse => {
                    Some(if installed_agent_skill("computer-use").is_some() {
                        "INSTALLED"
                    } else {
                        "NOT INSTALLED"
                    })
                }
                SettingsSection::Voice => {
                    let model = state.ui_settings().voice_model.as_str();
                    let ready = if model.starts_with("openai-") {
                        state.ui_settings().voice_openai_api_key_configured
                    } else {
                        crate::speech_models::is_ready(model, &state.ui_settings().voice_models_dir)
                    };
                    Some(if ready { "INSTALLED" } else { "NOT INSTALLED" })
                }
                _ => item.badge,
            };
            let mut label = row![
                icons::view(item.icon, 12.0, theme::MUTED),
                text(item.label)
                    .size(11)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .width(Length::Fill),
            ]
            .spacing(5)
            .align_y(Alignment::Center);
            if let Some(badge) = capability_status {
                label = label.push(
                    container(text(badge).size(8).color(theme::MUTED))
                        .padding([1, 3])
                        .style(theme::chip),
                );
            }
            group = group.push(
                button(label)
                    .on_press(Message::SettingsSectionSelected(item.section))
                    .width(Length::Fill)
                    .padding([4, 5])
                    .style(if state.settings_section() == item.section {
                        theme::selected_button
                    } else {
                        theme::ghost_button
                    }),
            );
        }
        navigation = navigation.push(group);
    }

    let project_rows = state.repos().iter().fold(
        column![text("PROJECTS").size(11).color(theme::MUTED)].spacing(2),
        |column, repo| {
            column.push(
                button(
                    row![
                        icons::view(Icon::GitBranch, 11.0, theme::MUTED),
                        text(&repo.display_name).size(12)
                    ]
                    .spacing(7),
                )
                .on_press(Message::SettingsSectionSelected(SettingsSection::Git))
                .width(Length::Fill)
                .padding([4, 6])
                .style(theme::ghost_button),
            )
        },
    );

    let sidebar_body = column![
        container(
            button(
                row![
                    icons::view(Icon::ArrowLeft, 12.0, theme::MUTED),
                    text("Back to app").size(12)
                ]
                .spacing(7)
            )
            .on_press(Message::IntegrationsToggled)
            .width(Length::Fill)
            .padding([5, 6])
            .style(theme::ghost_button)
        )
        .padding([7, 8])
        .style(theme::sidebar_top_bar),
        container(stack![
            text_input("Search settings", state.settings_search_query())
                .id(search_input_id())
                .on_input(Message::SettingsSearchChanged)
                .size(12)
                .padding(Padding {
                    top: 6.0,
                    right: 42.0,
                    bottom: 6.0,
                    left: 28.0,
                }),
            container(
                row![
                    icons::view(Icon::Search, 12.0, theme::MUTED),
                    Space::new().width(Length::Fill),
                    text("⌘ F").size(9).color(theme::MUTED),
                ]
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(iced::alignment::Vertical::Center)
            .padding([0, 8]),
        ])
        .padding(Padding {
            top: 15.0,
            right: 8.0,
            bottom: 7.0,
            left: 8.0,
        })
        .style(theme::sidebar_top_bar),
        container(
            button(
                row![
                    text("◔").size(13).color(theme::MUTED),
                    text("Onboarding checklist").size(12)
                ]
                .spacing(7)
            )
            .on_press(Message::OnboardingOpened)
            .width(Length::Fill)
            .padding([5, 6])
            .style(theme::ghost_button)
        )
        .padding([9, 8])
        .style(theme::sidebar_top_bar),
        scrollable(
            column![navigation, project_rows]
                .spacing(13)
                .padding([10, 10])
        )
        .height(Length::Fill),
    ]
    .height(Length::Fill);

    container(sidebar_body)
        .width(Length::Fixed(SIDEBAR_WIDTH))
        .height(Length::Fill)
        .style(theme::sidebar)
        .into()
}

fn settings_content(state: &AppState) -> Element<'_, Message> {
    let (title, description) = section_copy(state.settings_section());
    let body = match state.settings_section() {
        SettingsSection::General => general_content(state),
        SettingsSection::Integrations => integrations_content(state),
        SettingsSection::Agents => agents_content(state),
        SettingsSection::Git => git_content(state),
        SettingsSection::TaskSources => task_sources_content(state),
        SettingsSection::Terminal => terminal_content(state),
        SettingsSection::Shortcuts => shortcuts_content(state),
        SettingsSection::StatsUsage => stats_content(state),
        SettingsSection::Appearance => appearance_content(state),
        SettingsSection::InputEditing => input_content(state),
        SettingsSection::Notifications => notifications_content(state),
        SettingsSection::Privacy => privacy_content(state),
        SettingsSection::MacPermissions => mac_permissions_content(),
        SettingsSection::Mobile => mobile_content(state),
        SettingsSection::ProviderAccounts => provider_accounts_content(state),
        SettingsSection::Orchestration => orchestration_content(state),
        SettingsSection::ComputerUse => computer_use_content(state),
        SettingsSection::Voice => voice_content(state),
        SettingsSection::QuickCommands => quick_commands_content(state),
        SettingsSection::Browser => browser_content(state),
        SettingsSection::MobileEmulator => mobile_emulator_content(state),
        SettingsSection::FloatingWorkspace => floating_workspace_content(state),
        SettingsSection::SshHosts => ssh_hosts_content(state),
        SettingsSection::RemoteServers => remote_servers_content(state),
        SettingsSection::Advanced => advanced_content(state),
        SettingsSection::Plugins => plugins_content(state),
        SettingsSection::EphemeralVms => ephemeral_vms_content(state),
        SettingsSection::Experimental => experimental_content(state),
    };
    let page = column![
        text(title).size(22),
        text(description).size(12).color(theme::MUTED),
        Space::new().height(Length::Fixed(26.0)),
        rule::horizontal(1),
        Space::new().height(Length::Fixed(17.0)),
        body,
    ]
    .width(Length::Fixed(CONTENT_WIDTH))
    .padding(Padding {
        top: 26.0,
        right: 0.0,
        bottom: 30.0,
        left: 0.0,
    });

    scrollable(container(page).width(Length::Fill).center_x(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn general_content(state: &AppState) -> Element<'_, Message> {
    let cli_skill = installed_agent_skill("orca-cli");
    let cli_skill_command = if cli_skill.is_some() {
        "npx skills update orca-cli --global"
    } else {
        "npx skills add https://github.com/stablyai/orca --skill orca-cli --global"
    };
    let mut open_in_apps = column![].spacing(7);
    for (index, application) in state.ui_settings().open_in_applications.iter().enumerate() {
        open_in_apps = open_in_apps.push(
            row![
                text_input("App name", &application.label)
                    .on_input(move |value| Message::OpenInApplicationLabelChanged(index, value))
                    .size(11)
                    .padding([5, 7])
                    .width(Length::Fixed(145.0)),
                text_input("CLI command", &application.command)
                    .on_input(move |value| Message::OpenInApplicationCommandChanged(index, value))
                    .size(11)
                    .padding([5, 7])
                    .width(Length::Fill),
                button(text("Delete").size(11))
                    .on_press(Message::OpenInApplicationRemoved(index))
                    .padding([5, 7])
                    .style(theme::danger_ghost_button),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }
    let navigation = column![
        subsection_title("Navigation", None),
        choice_row(
            "Tab Order",
            if state.ui_settings().tab_order_mru {
                "Most recent"
            } else {
                "In order"
            },
            UiChoice::TabOrder,
        ),
        switch_row(
            "Confirm before closing pinned tabs",
            "Show a confirmation dialog before a pinned tab is closed.",
            state.ui_settings().confirm_close_pinned_tabs,
            Some(UiSetting::ConfirmClosePinnedTabs),
        ),
        switch_row(
            "Confirm before closing a running terminal",
            "Warn before stopping a terminal whose coding agent is still active.",
            state.ui_settings().confirm_close_running_terminal,
            Some(UiSetting::ConfirmCloseRunningTerminal),
        ),
    ]
    .spacing(14);

    let workspace = column![
        subsection_title(
            "Workspace",
            Some("Configure where new workspaces are created.")
        ),
        text("Workspace Directory").size(12),
        row![
            text_input("", &state.workspace_root().display().to_string())
                .on_input(Message::SettingsWorkspaceRootChanged)
                .size(12)
                .padding([6, 8])
                .width(Length::Fill),
            button(text("Browse").size(12))
                .on_press(Message::SettingsWorkspaceBrowseRequested)
                .padding([6, 9])
                .style(theme::ghost_button),
        ]
        .spacing(6),
        text("Use a relative path for a per-project location, or an absolute path for one shared folder.")
            .size(11)
            .color(theme::MUTED),
        switch_row(
            "Nest Workspaces",
            "Create workspaces inside a repo-named subfolder.",
            state.ui_settings().nest_workspaces,
            Some(UiSetting::NestWorkspaces),
        ),
        switch_row(
            "Ask Before Deleting Workspaces",
            "Show a confirmation before deleting a workspace from the context menu.",
            state.ui_settings().confirm_workspace_delete,
            Some(UiSetting::ConfirmWorkspaceDelete),
        ),
        switch_row(
            "Ask Before Deleting Automations",
            "Show a confirmation before deleting automations and their run history.",
            state.ui_settings().confirm_automation_delete,
            Some(UiSetting::ConfirmAutomationDelete),
        ),
        row![
            column![
                text("Open In Apps").size(12),
                text("Choose apps available from a workspace's Open in menu.")
                    .size(11)
                    .color(theme::MUTED)
            ]
            .spacing(2)
            .width(Length::Fill),
            button(text("+ Add app").size(11))
                .on_press(Message::OpenInApplicationAdded)
                .padding([5, 8])
                .style(theme::ghost_button),
        ],
        open_in_apps,
    ]
    .spacing(11);

    let editor = column![
        subsection_title("Editor", Some("Configure how Suaegi persists file edits.")),
        switch_row(
            "Auto Save Files",
            "Save editor and editable diff changes automatically after a short pause.",
            state.ui_settings().auto_save_files,
            Some(UiSetting::AutoSaveFiles),
        ),
        choice_row_owned(
            "Auto Save Delay",
            format!("{} ms", state.ui_settings().auto_save_delay_ms),
            UiChoice::AutoSaveDelay,
        ),
        choice_row(
            "Default Diff View",
            if state.ui_settings().diff_default_side_by_side {
                "Side by side"
            } else {
                "Inline"
            },
            UiChoice::DefaultDiffView,
        ),
        column![
            text_setting_row(
                "Editor Font Family",
                &state.ui_settings().editor_font_family,
                UiTextSetting::EditorFontFamily,
            ),
            text("Font used by file editors and diff views. Leave empty to follow the terminal font.")
                .size(11)
                .color(theme::MUTED),
        ]
        .spacing(4),
        switch_row(
            "Show Diff File Tree by Default",
            "Show the changed-file list when opening a combined diff.",
            state
                .ui_settings()
                .combined_diff_file_tree_visible_by_default,
            Some(UiSetting::CombinedDiffFileTreeVisibleByDefault),
        ),
        switch_row(
            "Editor Word Wrap",
            "Wrap long lines to the editor viewport.",
            state.ui_settings().editor_word_wrap,
            Some(UiSetting::EditorWordWrap),
        ),
        switch_row(
            "Editor Minimap",
            "Show a compact document overview beside the editor.",
            state.ui_settings().editor_minimap,
            Some(UiSetting::EditorMinimap),
        ),
        switch_row(
            "Rich Markdown Spellcheck",
            "Check spelling in rich Markdown editing surfaces.",
            state.ui_settings().rich_markdown_spellcheck,
            Some(UiSetting::RichMarkdownSpellcheck),
        ),
        switch_row(
            "Markdown Review Tools",
            "Show local review notes and the Markdown review panel.",
            state.ui_settings().markdown_review_tools,
            Some(UiSetting::MarkdownReviewTools),
        ),
    ]
    .spacing(13);

    column![
        settings_card(navigation),
        settings_card(workspace),
        settings_card(editor),
        settings_card(
            column![
                subsection_title(
                    "Suaegi CLI",
                    Some("Use Suaegi from your terminal to open the app and manage worktrees.")
                ),
                row![
                    column![
                        text("Shell command").size(12),
                        text(state.cli_install_path()).size(11).color(theme::MUTED)
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(
                        text(if state.cli_installed() {
                            "Reinstall"
                        } else {
                            "Install"
                        })
                        .size(11)
                    )
                    .on_press(Message::OnboardingInstallCli)
                    .padding([5, 8])
                    .style(theme::ghost_button)
                ]
                .align_y(Alignment::Center),
                rule::horizontal(1),
                row![
                    column![
                        text("CLI skill").size(12),
                        text(cli_skill.as_ref().map_or(
                            "Give agents workspace, terminal, and progress commands.".to_string(),
                            |path| format!("Installed at {}", path.display())
                        ))
                        .size(11)
                        .color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(
                        text(if state.cli_installed() {
                            if cli_skill.is_some() {
                                "Update skill"
                            } else {
                                "Install skill"
                            }
                        } else {
                            "Install CLI first"
                        })
                        .size(11)
                    )
                    .on_press(if state.cli_installed() {
                        Message::SettingsTerminalCommandRequested(cli_skill_command.to_string())
                    } else {
                        Message::OnboardingInstallCli
                    })
                    .padding([5, 8])
                    .style(theme::ghost_button),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(12)
        ),
        settings_card(
            column![
                subsection_title("Updates", None),
                row![
                    text("Current version: development")
                        .size(12)
                        .width(Length::Fill),
                    button(text("Check for Updates").size(11))
                        .on_press(Message::HelpActionSelected(HelpAction::CheckForUpdates))
                        .padding([5, 8])
                        .style(theme::ghost_button)
                ]
            ]
            .spacing(10)
        ),
    ]
    .spacing(14)
    .into()
}

fn integrations_content(state: &AppState) -> Element<'_, Message> {
    let statuses = state.hosted_integrations();
    column![
        cli_integration_card(
            "GitHub",
            "Pull requests, issues, checks, reviews, and project work through the gh CLI.",
            statuses.map(|statuses| &statuses.github),
            "gh",
            "https://cli.github.com",
            state.hosted_integrations_refreshing(),
        ),
        cli_integration_card(
            "GitLab",
            "Merge requests, issues, reviews, todos, and pipelines through the glab CLI.",
            statuses.map(|statuses| &statuses.gitlab),
            "glab",
            "https://gitlab.com/gitlab-org/cli#installation",
            state.hosted_integrations_refreshing(),
        ),
        token_integration_card(
            "Gitea",
            "Pull requests and commit statuses through the Gitea REST API.",
            statuses.map(|statuses| &statuses.gitea),
            "Set ORCA_GITEA_API_BASE_URL and ORCA_GITEA_TOKEN before starting Suaegi.",
            state.hosted_integrations_refreshing(),
        ),
        token_integration_card(
            "Azure DevOps",
            "Pull requests and build statuses through Azure DevOps REST API tokens.",
            statuses.map(|statuses| &statuses.azure_dev_ops),
            "Set ORCA_AZURE_DEVOPS_API_BASE_URL and ORCA_AZURE_DEVOPS_TOKEN before starting Suaegi.",
            state.hosted_integrations_refreshing(),
        ),
        token_integration_card(
            "Bitbucket",
            "Pull requests and build statuses through Bitbucket Cloud API tokens.",
            statuses.map(|statuses| &statuses.bitbucket),
            "Set ORCA_BITBUCKET_ACCESS_TOKEN, or EMAIL plus API_TOKEN, before starting Suaegi.",
            state.hosted_integrations_refreshing(),
        ),
        sidebar::linear_panel(state),
        sidebar::jira_panel(state),
    ]
    .spacing(14)
    .into()
}

fn cli_integration_card<'a>(
    title: &'a str,
    description: &'a str,
    status: Option<&'a crate::hosted_integrations::CliIntegrationStatus>,
    command: &'a str,
    install_url: &'a str,
    refreshing: bool,
) -> Element<'a, Message> {
    use crate::hosted_integrations::CliIntegrationStatus;

    let (label, detail, connected) = if refreshing || status.is_none() {
        (
            "CHECKING",
            "Checking CLI authentication…".to_string(),
            false,
        )
    } else {
        match status {
            Some(CliIntegrationStatus::Connected) => (
                "CONNECTED",
                format!("Authenticated through `{command} auth status`."),
                true,
            ),
            Some(CliIntegrationStatus::NotInstalled) => (
                "NOT INSTALLED",
                format!("Install the {command} CLI to enable this integration."),
                false,
            ),
            Some(CliIntegrationStatus::NotAuthenticated) => (
                "NOT AUTHENTICATED",
                format!("Run `{command} auth login` in a terminal, then re-check."),
                false,
            ),
            Some(CliIntegrationStatus::OutdatedVersion { found, min }) => (
                "UPDATE NEEDED",
                format!("{command} {found} is installed; version {min} or newer is required."),
                false,
            ),
            None => unreachable!("the loading state handles a missing CLI status"),
        }
    };
    let show_install = matches!(status, Some(CliIntegrationStatus::NotInstalled)) && !refreshing;
    settings_card(
        column![
            row![
                column![
                    text(title).size(13),
                    text(description).size(11).color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                container(text(label).size(9))
                    .padding([2, 6])
                    .style(if connected {
                        theme::active_card
                    } else {
                        theme::chip
                    }),
            ]
            .align_y(Alignment::Center),
            text(detail).size(11).color(theme::MUTED),
            text("Account scope follows the active local or managed provider account.")
                .size(10)
                .color(theme::MUTED),
            row![
                if show_install {
                    button(text(format!("Install {command}")).size(11))
                        .on_press(Message::ExternalUrlRequested(install_url.to_string()))
                        .padding([5, 8])
                        .style(theme::ghost_button)
                } else {
                    button(text("").size(11))
                        .padding([5, 8])
                        .style(theme::ghost_button)
                },
                Space::new().width(Length::Fill),
                button(
                    text(if refreshing {
                        "Checking…"
                    } else {
                        "Re-check"
                    })
                    .size(11)
                )
                .on_press_maybe(
                    (!refreshing).then_some(Message::HostedIntegrationsRefreshRequested)
                )
                .padding([5, 8])
                .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(9),
    )
}

fn token_integration_card<'a>(
    title: &'a str,
    description: &'a str,
    status: Option<&'a crate::hosted_integrations::TokenIntegrationStatus>,
    setup: &'a str,
    refreshing: bool,
) -> Element<'a, Message> {
    let (label, detail, connected) = if refreshing {
        ("CHECKING", "Checking authentication…".to_string(), false)
    } else {
        match status {
            Some(status) if status.authenticated => (
                "CONNECTED",
                status
                    .account
                    .as_deref()
                    .map_or_else(|| "Authenticated".to_string(), str::to_string),
                true,
            ),
            Some(status) if status.configured && status.token_configured => (
                "AUTH FAILED",
                "Credentials are configured but could not authenticate.".to_string(),
                false,
            ),
            Some(status) if status.configured => (
                "TOKEN NEEDED",
                status
                    .base_url
                    .as_deref()
                    .map_or_else(|| "Endpoint detected".to_string(), str::to_string),
                false,
            ),
            _ => ("NOT CONFIGURED", setup.to_string(), false),
        }
    };
    settings_card(
        column![
            row![
                column![
                    text(title).size(13),
                    text(description).size(11).color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                container(text(label).size(10))
                    .padding([2, 6])
                    .style(if connected {
                        theme::active_card
                    } else {
                        theme::chip
                    }),
            ]
            .align_y(Alignment::Center),
            text(detail).size(11).color(theme::MUTED),
            row![
                status
                    .and_then(|status| status.base_url.as_deref())
                    .map(|url| text(url).size(11).color(theme::MUTED))
                    .unwrap_or_else(|| text("").size(11))
                    .width(Length::Fill),
                button(text(if refreshing { "Checking…" } else { "Refresh" }).size(11))
                    .on_press_maybe(
                        (!refreshing).then_some(Message::HostedIntegrationsRefreshRequested)
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(9),
    )
}

fn agents_content(state: &AppState) -> Element<'_, Message> {
    let defs = suaegi_term::agent::agent_defs();
    let installed_count = defs
        .iter()
        .filter(|agent| state.is_agent_installed(agent.id))
        .count();
    let mut installed = column![].spacing(0);
    let mut available = column![].spacing(0);
    for agent in defs {
        let row = agent_settings_row(state, agent);
        if state.is_agent_installed(agent.id) {
            installed = installed.push(row);
        } else {
            available = available.push(row);
        }
    }
    column![
        settings_card(
            column![
                subsection_title(
                    "Default Agent",
                    Some("Choose which detected agent starts in new workspaces.")
                ),
                choice_row(
                    "New workspace agent",
                    &state.ui_settings().default_agent,
                    UiChoice::DefaultAgent,
                ),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "Agent Permissions",
                    Some(
                        "Choose fewer permission prompts or manual checks. Custom per-agent \
                         arguments are preserved."
                    )
                ),
                row![
                    button(text("Yolo").size(11))
                        .on_press(Message::AgentPermissionModeSelected(true))
                        .padding([5, 10])
                        .style(if state.agent_permission_yolo() {
                            theme::selected_button
                        } else {
                            theme::ghost_button
                        }),
                    button(text("Manual").size(11))
                        .on_press(Message::AgentPermissionModeSelected(false))
                        .padding([5, 10])
                        .style(if state.agent_permission_yolo() {
                            theme::ghost_button
                        } else {
                            theme::selected_button
                        }),
                ]
                .spacing(6),
            ]
            .spacing(10)
        ),
        settings_card(
            column![
                subsection_title("Agent behavior", None),
                switch_row(
                    "Agent status hooks",
                    "Install and use agent hooks for accurate working and completion state.",
                    state.ui_settings().agent_status_hooks_enabled,
                    Some(UiSetting::AgentStatusHooksEnabled),
                ),
                switch_row(
                    "Generate tab titles",
                    "Generate semantic terminal tab titles from agent work.",
                    state.ui_settings().tab_auto_generate_title,
                    Some(UiSetting::TabAutoGenerateTitle),
                ),
                switch_row(
                    "Keep computer awake",
                    "Prevent system sleep while an agent is actively working.",
                    state.ui_settings().keep_computer_awake_while_agents_run,
                    Some(UiSetting::KeepComputerAwakeWhileAgentsRun),
                ),
                choice_row(
                    "Claude Agent Teams",
                    match state.ui_settings().claude_agent_teams_mode.as_str() {
                        "native-panes-shim" => "Native panes",
                        "in-process" => "In process",
                        _ => "Off",
                    },
                    UiChoice::ClaudeAgentTeamsMode,
                ),
                switch_row(
                    "Prompt cache timer",
                    "Show the remaining prompt-cache lifetime after an agent becomes idle.",
                    state.ui_settings().prompt_cache_timer_enabled,
                    Some(UiSetting::PromptCacheTimerEnabled),
                ),
                choice_row(
                    "Prompt cache TTL",
                    if state.ui_settings().prompt_cache_ttl_minutes == 60 {
                        "60 min"
                    } else {
                        "5 min"
                    },
                    UiChoice::PromptCacheTtl,
                ),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                row![
                    column![
                        text("Installed").size(13),
                        text(format!("{installed_count} detected"))
                            .size(11)
                            .color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(text("Refresh").size(11))
                        .on_press(Message::AgentDetectionRefreshRequested)
                        .padding([5, 8])
                        .style(theme::ghost_button),
                ]
                .align_y(Alignment::Center),
                installed,
            ]
            .spacing(8)
        ),
        settings_card(
            column![
                subsection_title(
                    "Available to install",
                    Some("Disabled agents are hidden from workspace and automation pickers.")
                ),
                available,
            ]
            .spacing(8)
        ),
    ]
    .spacing(14)
    .into()
}

fn agent_settings_row<'a>(
    state: &'a AppState,
    agent: &'static suaegi_term::agent::AgentDef,
) -> Element<'a, Message> {
    let installed = state.is_agent_installed(agent.id);
    let enabled = !state
        .ui_settings()
        .disabled_agents
        .iter()
        .any(|disabled| disabled == agent.id);
    let is_default = state.ui_settings().default_agent == agent.id;
    let expanded = installed && state.agent_settings_expanded(agent.id);
    let command_override = state
        .ui_settings()
        .agent_command_overrides
        .get(agent.id)
        .map(String::as_str)
        .unwrap_or("");
    let args = state
        .ui_settings()
        .agent_default_args
        .get(agent.id)
        .map(String::as_str)
        .unwrap_or("");
    let env = state.agent_env_draft(agent.id);
    let summary = [
        if command_override.is_empty() {
            agent.launch_program
        } else {
            command_override
        },
        args,
        env,
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("  ");

    let agent_id = agent.id.to_string();
    let availability = button(text(if enabled { "Enabled" } else { "Disabled" }).size(11))
        .on_press(Message::AgentAvailabilityToggled(agent_id.clone()))
        .padding([4, 7])
        .style(if enabled {
            theme::selected_button
        } else {
            theme::ghost_button
        });
    let default_button: Element<'a, Message> = if installed && enabled {
        button(text(if is_default { "Default" } else { "Set default" }).size(11))
            .on_press(Message::UiChoiceSelected(
                UiChoice::DefaultAgent,
                agent.id.to_string(),
            ))
            .padding([4, 7])
            .style(if is_default {
                theme::selected_button
            } else {
                theme::ghost_button
            })
            .into()
    } else {
        Space::new().width(0).into()
    };
    let expand_button: Element<'a, Message> = if installed {
        button(text(if expanded { "Hide" } else { "Configure" }).size(11))
            .on_press(Message::AgentSettingsExpansionToggled(agent_id.clone()))
            .padding([4, 7])
            .style(theme::ghost_button)
            .into()
    } else {
        Space::new().width(0).into()
    };

    let details: Element<'a, Message> = if expanded {
        column![
            row![
                text("Command").size(11).width(Length::Fixed(88.0)),
                text_input(agent.launch_program, command_override)
                    .on_input({
                        let agent_id = agent_id.clone();
                        move |value| Message::AgentCommandOverrideChanged(agent_id.clone(), value)
                    })
                    .size(11)
                    .padding([5, 7])
                    .width(Length::Fill),
            ]
            .align_y(Alignment::Center),
            row![
                text("Arguments").size(11).width(Length::Fixed(88.0)),
                text_input("No default arguments", args)
                    .on_input({
                        let agent_id = agent_id.clone();
                        move |value| Message::AgentDefaultArgsChanged(agent_id.clone(), value)
                    })
                    .size(11)
                    .padding([5, 7])
                    .width(Length::Fill),
            ]
            .align_y(Alignment::Center),
            row![
                text("Environment").size(11).width(Length::Fixed(88.0)),
                text_input("NAME=value", env)
                    .on_input({
                        let agent_id = agent_id.clone();
                        move |value| Message::AgentDefaultEnvChanged(agent_id.clone(), value)
                    })
                    .size(11)
                    .padding([5, 7])
                    .width(Length::Fill),
            ]
            .align_y(Alignment::Center),
            text(
                "Override the binary, launch arguments, or environment for this agent. \
                 Environment entries use NAME=value."
            )
            .size(10)
            .color(theme::MUTED),
        ]
        .spacing(7)
        .padding(Padding {
            top: 7.0,
            right: 0.0,
            bottom: 7.0,
            left: 28.0,
        })
        .into()
    } else {
        Space::new().height(0).into()
    };

    column![
        rule::horizontal(1),
        row![
            icons::view(Icon::Bot, 12.0, theme::MUTED),
            column![
                row![
                    text(agent.display_name).size(12),
                    if installed {
                        text("Detected")
                            .size(10)
                            .color(iced::Color::from_rgb8(0x22, 0xa0, 0x59))
                    } else {
                        text("Not installed").size(10).color(theme::MUTED)
                    },
                ]
                .spacing(6),
                text(summary).size(10).color(theme::MUTED),
            ]
            .spacing(3)
            .width(Length::Fill),
            availability,
            default_button,
            expand_button,
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .padding([8, 0]),
        details,
    ]
    .into()
}

fn source_control_ai_card(state: &AppState) -> Element<'_, Message> {
    let settings = &state.ui_settings().source_control_ai;
    let agent_options = crate::source_control_ai::AGENT_IDS
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let mut model_options = state.source_control_ai_models(&settings.agent_id);
    if !model_options.contains(&settings.model) {
        model_options.push(settings.model.clone());
    }
    let thinking_options =
        crate::source_control_ai::thinking_levels_for(&settings.agent_id, &settings.model)
            .iter()
            .map(|value| {
                if value.is_empty() {
                    "none".to_string()
                } else {
                    value.to_string()
                }
            })
            .collect::<Vec<_>>();
    let thinking_selected = if settings.thinking_level.is_empty() {
        "none".to_string()
    } else {
        settings.thinking_level.clone()
    };
    let model_refresh: Element<'_, Message> =
        if crate::source_control_ai::has_dynamic_models(&settings.agent_id) {
            button(
                text(
                    if state.source_control_ai_models_loading(&settings.agent_id) {
                        "Loading…"
                    } else {
                        "Refresh"
                    },
                )
                .size(9),
            )
            .on_press_maybe(
                (!state.source_control_ai_models_loading(&settings.agent_id))
                    .then_some(Message::SourceControlAiModelsRefreshRequested),
            )
            .padding([4, 6])
            .style(theme::ghost_button)
            .into()
        } else {
            Space::new().width(0).into()
        };
    let model_status: Element<'_, Message> = state.source_control_ai_models_status().map_or_else(
        || Space::new().height(0).into(),
        |status| text(status).size(10).color(theme::MUTED).into(),
    );
    let custom_command: Element<'_, Message> = if settings.agent_id == "custom" {
        row![
            text("Custom command").size(12).width(Length::Fixed(130.0)),
            text_input(
                "ollama run model (prompt is sent on stdin)",
                &settings.custom_agent_command
            )
            .on_input(Message::SourceControlAiCustomCommandChanged)
            .size(11)
            .padding([6, 8])
            .width(Length::Fill),
        ]
        .align_y(Alignment::Center)
        .into()
    } else {
        Space::new().height(0).into()
    };
    let action_labels = [
        ("commitMessage", "Commit message"),
        ("pullRequest", "Pull request details"),
        ("branchName", "Branch name"),
        ("fixCommitFailure", "Commit failure fixes"),
        ("fixPushFailure", "Push failure fixes"),
        ("fixChecks", "Broken checks fixes"),
        ("resolveConflicts", "Conflict resolution"),
        ("resolveComments", "Review comment resolution"),
    ];
    let mut action_rows = column![].spacing(7);
    for (action_id, label) in action_labels {
        let recipe = settings.actions.get(action_id).cloned().unwrap_or_default();
        let mut recipe_agents = vec!["inherit".to_string()];
        recipe_agents.extend(agent_options.iter().cloned());
        let selected_agent = recipe
            .agent_id
            .clone()
            .unwrap_or_else(|| "inherit".to_string());
        let action_for_agent = action_id.to_string();
        let action_for_template = action_id.to_string();
        let action_for_args = action_id.to_string();
        action_rows = action_rows.push(
            container(
                column![
                    row![
                        text(label).size(11).width(Length::Fill),
                        pick_list(recipe_agents, Some(selected_agent), move |value| {
                            Message::SourceControlAiActionAgentSelected(
                                action_for_agent.clone(),
                                value,
                            )
                        })
                        .text_size(10)
                        .width(Length::Fixed(130.0)),
                    ]
                    .align_y(Alignment::Center),
                    text_input(
                        "Command input template; use {basePrompt} and action variables",
                        &recipe.command_input_template
                    )
                    .on_input(move |value| Message::SourceControlAiActionTemplateChanged(
                        action_for_template.clone(),
                        value
                    ))
                    .size(10)
                    .padding([5, 7]),
                    text_input("Additional CLI arguments", &recipe.agent_args)
                        .on_input(move |value| Message::SourceControlAiActionArgsChanged(
                            action_for_args.clone(),
                            value
                        ))
                        .size(10)
                        .padding([5, 7]),
                ]
                .spacing(5),
            )
            .padding(8)
            .style(theme::active_card),
        );
    }
    let defaults = &settings.pr_creation_defaults;
    settings_card(
        column![
            subsection_title(
                "Source Control AI defaults",
                Some(
                    "Agent, model, recipes, prompts, and hosted-review defaults used by source-control actions."
                )
            ),
            action_switch_row(
                "Show Source Control AI actions",
                "Show generation and repair actions in Source Control.",
                settings.enabled,
                Message::SourceControlAiEnabledToggled,
            ),
            row![
                text("Agent").size(12).width(Length::Fixed(80.0)),
                pick_list(agent_options, Some(settings.agent_id.clone()), |value| {
                    Message::SourceControlAiAgentSelected(value)
                })
                .text_size(11)
                .width(Length::FillPortion(2)),
                text("Model").size(12),
                pick_list(model_options, Some(settings.model.clone()), |value| {
                    Message::SourceControlAiModelChanged(value)
                })
                .text_size(11)
                .width(Length::FillPortion(3)),
                text("Effort").size(12),
                pick_list(thinking_options, Some(thinking_selected), |value| {
                    Message::SourceControlAiThinkingSelected(if value == "none" {
                        String::new()
                    } else {
                        value
                    })
                })
                .text_size(11)
                .width(Length::Fixed(100.0)),
                model_refresh,
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            model_status,
            custom_command,
            rule::horizontal(1),
            subsection_title(
                "Action recipes",
                Some(
                    "{basePrompt} expands to Suaegi's bounded repository context. Unknown variables stay visible."
                )
            ),
            action_rows,
            rule::horizontal(1),
            subsection_title("Hosted review creation defaults", None),
            action_switch_row(
                "Create as draft",
                "Start new hosted reviews as drafts.",
                defaults.draft,
                Message::SourceControlAiPrDefaultToggled(SourceControlAiPrDefault::Draft),
            ),
            action_switch_row(
                "Use repository template",
                "Preserve the repository PR/MR template when creating reviews.",
                defaults.use_template,
                Message::SourceControlAiPrDefaultToggled(SourceControlAiPrDefault::UseTemplate),
            ),
            action_switch_row(
                "Generate details on open",
                "Generate title and description when the review composer opens.",
                defaults.generate_details_on_open,
                Message::SourceControlAiPrDefaultToggled(
                    SourceControlAiPrDefault::GenerateDetailsOnOpen,
                ),
            ),
            action_switch_row(
                "Open hosted review after creation",
                "Open the created PR or MR after submit.",
                defaults.open_after_create,
                Message::SourceControlAiPrDefaultToggled(SourceControlAiPrDefault::OpenAfterCreate),
            ),
        ]
        .spacing(11),
    )
}

fn repo_source_control_ai_section<'a>(
    state: &'a AppState,
    repo: &'a suaegi_core::domain::Repo,
) -> Element<'a, Message> {
    let global = &state.ui_settings().source_control_ai;
    let overrides = state.ui_settings().repo_source_control_ai.get(&repo.id.0);
    let visibility = match overrides.and_then(|value| value.enabled) {
        Some(true) => "enabled",
        Some(false) => "disabled",
        None => "inherit",
    }
    .to_string();
    let visibility_repo = repo.id.clone();
    let reset_repo = repo.id.clone();
    let custom_repo = repo.id.clone();
    let mut actions = column![].spacing(5);
    for (action_id, label) in [
        ("commitMessage", "Commit message"),
        ("pullRequest", "Pull request details"),
        ("branchName", "Branch name"),
        ("fixCommitFailure", "Commit failure fixes"),
        ("fixPushFailure", "Push failure fixes"),
        ("fixChecks", "Broken checks fixes"),
        ("resolveConflicts", "Conflict resolution"),
        ("resolveComments", "Review comment resolution"),
    ] {
        let override_recipe = overrides.and_then(|value| value.action_overrides.get(action_id));
        let recipe = override_recipe
            .cloned()
            .or_else(|| global.actions.get(action_id).cloned())
            .unwrap_or_default();
        let selected_agent = override_recipe
            .and_then(|recipe| recipe.agent_id.clone())
            .unwrap_or_else(|| "inherit".to_string());
        let mut agents = vec!["inherit".to_string()];
        agents.extend(
            crate::source_control_ai::AGENT_IDS
                .iter()
                .map(|value| value.to_string()),
        );
        let agent_repo = repo.id.clone();
        let template_repo = repo.id.clone();
        let args_repo = repo.id.clone();
        let reset_action_repo = repo.id.clone();
        let agent_action = action_id.to_string();
        let template_action = action_id.to_string();
        let args_action = action_id.to_string();
        let reset_action = action_id.to_string();
        actions = actions.push(
            container(
                column![
                    row![
                        text(label).size(10).width(Length::Fill),
                        text(if override_recipe.is_some() {
                            "Override"
                        } else {
                            "Inherited"
                        })
                        .size(9)
                        .color(theme::MUTED),
                        pick_list(agents, Some(selected_agent), move |value| {
                            Message::RepoSourceControlAiActionAgentSelected(
                                agent_repo.clone(),
                                agent_action.clone(),
                                value,
                            )
                        })
                        .text_size(9)
                        .width(Length::Fixed(105.0)),
                        button(text("Reset").size(8))
                            .on_press_maybe(override_recipe.is_some().then(|| {
                                Message::RepoSourceControlAiActionReset(
                                    reset_action_repo,
                                    reset_action,
                                )
                            }))
                            .padding([3, 5])
                            .style(theme::ghost_button),
                    ]
                    .spacing(5)
                    .align_y(Alignment::Center),
                    text_input("Inherited command template", &recipe.command_input_template)
                        .on_input(move |value| {
                            Message::RepoSourceControlAiActionTemplateChanged(
                                template_repo.clone(),
                                template_action.clone(),
                                value,
                            )
                        })
                        .size(9)
                        .padding([4, 6]),
                    text_input("Inherited CLI arguments", &recipe.agent_args)
                        .on_input(move |value| {
                            Message::RepoSourceControlAiActionArgsChanged(
                                args_repo.clone(),
                                args_action.clone(),
                                value,
                            )
                        })
                        .size(9)
                        .padding([4, 6]),
                ]
                .spacing(4),
            )
            .padding(7)
            .style(theme::active_card),
        );
    }
    let pr_overrides = overrides.map(|value| &value.pr_creation_defaults);
    let tri = |value: Option<bool>| match value {
        Some(true) => "enabled".to_string(),
        Some(false) => "disabled".to_string(),
        None => "inherit".to_string(),
    };
    let pr_row = |label: &'static str,
                  setting: SourceControlAiPrDefault,
                  selected: String,
                  repo_id: suaegi_core::domain::RepoId|
     -> Element<'a, Message> {
        row![
            text(label).size(10).width(Length::Fill),
            pick_list(
                vec![
                    "inherit".to_string(),
                    "enabled".to_string(),
                    "disabled".to_string(),
                ],
                Some(selected),
                move |value| Message::RepoSourceControlAiPrDefaultSelected(
                    repo_id.clone(),
                    setting,
                    value,
                ),
            )
            .text_size(9)
            .width(Length::Fixed(105.0)),
        ]
        .align_y(Alignment::Center)
        .into()
    };
    column![
        row![
            column![
                text("Source Control AI").size(11),
                text("Project overrides inherit global recipes until customized.")
                    .size(10)
                    .color(theme::MUTED),
            ]
            .spacing(2)
            .width(Length::Fill),
            pick_list(
                vec![
                    "inherit".to_string(),
                    "enabled".to_string(),
                    "disabled".to_string(),
                ],
                Some(visibility),
                move |value| Message::RepoSourceControlAiVisibilitySelected(
                    visibility_repo.clone(),
                    value,
                ),
            )
            .text_size(10)
            .width(Length::Fixed(105.0)),
            button(text("Reset all").size(9))
                .on_press_maybe(
                    overrides
                        .is_some()
                        .then(|| Message::RepoSourceControlAiReset(reset_repo))
                )
                .padding([4, 6])
                .style(theme::ghost_button),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        text_input(
            if global.custom_agent_command.trim().is_empty() {
                "Inherit custom agent command"
            } else {
                &global.custom_agent_command
            },
            overrides
                .and_then(|value| value.custom_agent_command.as_deref())
                .unwrap_or("")
        )
        .on_input(move |value| {
            Message::RepoSourceControlAiCustomCommandChanged(custom_repo.clone(), value)
        })
        .size(9)
        .padding([4, 6]),
        actions,
        text("Hosted review defaults").size(10),
        pr_row(
            "Create as draft",
            SourceControlAiPrDefault::Draft,
            tri(pr_overrides.and_then(|value| value.draft)),
            repo.id.clone(),
        ),
        pr_row(
            "Use repository template",
            SourceControlAiPrDefault::UseTemplate,
            tri(pr_overrides.and_then(|value| value.use_template)),
            repo.id.clone(),
        ),
        pr_row(
            "Generate details on open",
            SourceControlAiPrDefault::GenerateDetailsOnOpen,
            tri(pr_overrides.and_then(|value| value.generate_details_on_open)),
            repo.id.clone(),
        ),
        pr_row(
            "Open after creation",
            SourceControlAiPrDefault::OpenAfterCreate,
            tri(pr_overrides.and_then(|value| value.open_after_create)),
            repo.id.clone(),
        ),
    ]
    .spacing(6)
    .into()
}

fn repo_fork_sync_section<'a>(
    state: &'a AppState,
    repo: &'a suaegi_core::domain::Repo,
) -> Option<Element<'a, Message>> {
    let upstream = state.ui_settings().repo_github_upstreams.get(&repo.id.0)?;
    let selected = match state
        .ui_settings()
        .repo_fork_sync_modes
        .get(&repo.id.0)
        .map(String::as_str)
        .unwrap_or("ask")
    {
        "safe-auto" => ForkSyncChoice::SAFE_AUTO,
        "off" => ForkSyncChoice::OFF,
        _ => ForkSyncChoice::ASK,
    };
    let mode_repo = repo.id.clone();
    let sync_repo = repo.id.clone();
    let syncing = state.repo_fork_syncing(&repo.id);
    let status = state.repo_fork_sync_status(&repo.id);
    let mut content = column![
        row![
            column![
                text("Keep Fork Up to Date").size(11),
                text(
                    "Safely fast-forward the fork default branch. Updates are skipped when origin has fork-only commits or cannot be advanced without conflicts."
                )
                .size(10)
                .color(theme::MUTED),
                text(format!("Fork of {}/{}", upstream.owner, upstream.repo))
                    .size(10)
                    .color(theme::MUTED),
            ]
            .spacing(3)
            .width(Length::Fill),
            button(text(if syncing { "Syncing…" } else { "Sync Now" }).size(10))
                .on_press_maybe((!syncing).then(|| Message::RepoForkSyncRequested(sync_repo)))
                .padding([5, 8])
                .style(theme::ghost_button),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        pick_list(
            vec![
                ForkSyncChoice::ASK,
                ForkSyncChoice::SAFE_AUTO,
                ForkSyncChoice::OFF,
            ],
            Some(selected),
            move |choice| Message::RepoForkSyncModeSelected(
                mode_repo.clone(),
                choice.0.to_string(),
            ),
        )
        .text_size(10)
        .width(Length::Fixed(120.0)),
    ]
    .spacing(6);
    if let Some(status) = status {
        content = content.push(text(status).size(10).color(theme::MUTED));
    }
    Some(content.into())
}

fn repo_external_worktree_section<'a>(
    state: &'a AppState,
    repo: &'a suaegi_core::domain::Repo,
) -> Element<'a, Message> {
    let visibility = state
        .ui_settings()
        .repo_external_worktree_visibility
        .get(&repo.id.0)
        .map(String::as_str)
        .unwrap_or("show");
    let selected = if visibility == "hide" {
        ExternalWorktreeChoice::HIDE
    } else {
        ExternalWorktreeChoice::SHOW
    };
    let repo_id = repo.id.clone();
    let inbox_count = state.external_worktree_inbox(&repo.id).len();
    let suppressed = state
        .ui_settings()
        .repo_external_worktree_discovery_suppressed_at
        .contains_key(&repo.id.0);
    column![
        row![
            column![
                text("Non-Suaegi Worktrees").size(11),
                text(
                    "Choose whether worktrees created outside Suaegi appear in the sidebar. Suaegi-created and explicitly imported worktrees always remain visible."
                )
                .size(10)
                .color(theme::MUTED),
            ]
            .spacing(3)
            .width(Length::Fill),
            pick_list(
                vec![
                    ExternalWorktreeChoice::HIDE,
                    ExternalWorktreeChoice::SHOW,
                ],
                Some(selected),
                move |choice| Message::RepoExternalWorktreeVisibilitySelected(
                    repo_id.clone(),
                    choice.0.to_string(),
                ),
            )
            .text_size(10)
            .width(Length::Fixed(150.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text(if suppressed {
            "Discovery prompts are disabled. Select “Show in sidebar” to enable them again."
                .to_string()
        } else if inbox_count > 0 {
            format!("{inbox_count} newly discovered external worktree(s) are ready to review.")
        } else {
            "No new external worktrees are waiting for review.".to_string()
        })
        .size(10)
        .color(theme::MUTED),
    ]
    .spacing(6)
    .into()
}

fn repo_host_setups_section<'a>(
    state: &'a AppState,
    repo: &'a suaegi_core::domain::Repo,
) -> Element<'a, Message> {
    let setups = state
        .ui_settings()
        .repo_host_setups
        .get(&repo.id.0)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut setup_rows = column![row![
        column![
            text("This Mac").size(11),
            text(repo.path.display().to_string())
                .size(9)
                .color(theme::MUTED),
        ]
        .spacing(2)
        .width(Length::Fill),
        container(text("Ready").size(9).color(theme::MUTED))
            .padding([2, 5])
            .style(theme::chip),
        container(text("Current").size(9).color(theme::MUTED))
            .padding([2, 5])
            .style(theme::chip),
    ]
    .spacing(7)
    .align_y(Alignment::Center)]
    .spacing(5);
    for setup in setups {
        let host_label = state
            .ui_settings()
            .ssh_hosts
            .iter()
            .find(|host| host.id == setup.host_id)
            .map(|host| host.label.as_str())
            .unwrap_or("Unavailable SSH host");
        let remove_repo = repo.id.clone();
        let open_repo = repo.id.clone();
        let setup_id = setup.id.clone();
        let open_setup_id = setup.id.clone();
        let state_label = match setup.setup_state.as_str() {
            "not-set-up" => "Not set up",
            "setting-up" => "Setting up",
            "error" => "Error",
            "unsupported" => "Unsupported",
            _ => "Ready",
        };
        setup_rows = setup_rows.push(
            row![
                column![
                    text(host_label.to_string()).size(11),
                    text(if setup.path.is_empty() {
                        "Path pending".to_string()
                    } else {
                        setup.path.clone()
                    })
                    .size(9)
                    .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                container(text(state_label).size(9).color(theme::MUTED))
                    .padding([2, 5])
                    .style(theme::chip),
                button(text("Open terminal").size(9))
                    .on_press_maybe(
                        (setup.setup_state == "ready" && !setup.path.is_empty()).then(|| {
                            Message::RepoHostSetupOpenTerminal(open_repo, open_setup_id)
                        })
                    )
                    .padding([3, 6])
                    .style(theme::ghost_button),
                button(text("Unregister").size(9))
                    .on_press(Message::RepoHostSetupRemoved(remove_repo, setup_id))
                    .padding([3, 6])
                    .style(theme::danger_ghost_button),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        );
    }

    let choices = state
        .ui_settings()
        .ssh_hosts
        .iter()
        .map(|host| HostSetupChoice {
            id: host.id.clone(),
            label: if host.label.trim().is_empty() {
                host.hostname.clone()
            } else {
                host.label.clone()
            },
        })
        .collect::<Vec<_>>();
    let draft = state.repo_host_setup_draft(&repo.id);
    let selected = choices
        .iter()
        .find(|choice| choice.id == draft.host_id)
        .cloned();
    let busy = state.repo_host_setup_busy(&repo.id);
    let controls: Element<'_, Message> = if choices.is_empty() {
        row![
            text("Add and test an SSH host before setting up this project remotely.")
                .size(10)
                .color(theme::MUTED)
                .width(Length::Fill),
            button(text("Configure SSH").size(10))
                .on_press(Message::SettingsSectionSelected(SettingsSection::SshHosts))
                .padding([5, 8])
                .style(theme::ghost_button),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        let select_repo = repo.id.clone();
        let path_repo = repo.id.clone();
        let kind_repo = repo.id.clone();
        let existing_repo = repo.id.clone();
        let clone_url_repo = repo.id.clone();
        let clone_destination_repo = repo.id.clone();
        let clone_repo = repo.id.clone();
        let planned_repo = repo.id.clone();
        column![
            row![
                text("Host").size(10).width(Length::Fixed(75.0)),
                pick_list(choices, selected, move |choice| {
                    Message::RepoHostSetupHostSelected(select_repo.clone(), choice.id)
                })
                .text_size(10)
                .width(Length::Fill),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
            text("Use an existing folder").size(10),
            row![
                text_input(
                    "/absolute/path/to/project",
                    state.repo_host_setup_path_draft(&repo.id)
                )
                .on_input(move |value| {
                    Message::RepoHostSetupPathChanged(path_repo.clone(), value)
                })
                .size(10)
                .padding([5, 7])
                .width(Length::Fill),
                pick_list(
                    vec!["git".to_string(), "folder".to_string()],
                    Some(draft.kind),
                    move |value| { Message::RepoHostSetupKindSelected(kind_repo.clone(), value) },
                )
                .text_size(10)
                .width(Length::Fixed(85.0)),
                button(text(if busy { "Checking…" } else { "Add" }).size(10))
                    .on_press_maybe(
                        (!busy).then(|| Message::RepoHostSetupExistingRequested(existing_repo))
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            text("Clone from URL").size(10),
            text_input(
                "git@github.com:owner/repository.git",
                state.repo_host_setup_clone_url_draft(&repo.id)
            )
            .on_input(move |value| {
                Message::RepoHostSetupCloneUrlChanged(clone_url_repo.clone(), value)
            })
            .size(10)
            .padding([5, 7]),
            row![
                text_input(
                    "Absolute destination directory",
                    state.repo_host_setup_clone_destination_draft(&repo.id)
                )
                .on_input(move |value| {
                    Message::RepoHostSetupCloneDestinationChanged(
                        clone_destination_repo.clone(),
                        value,
                    )
                })
                .size(10)
                .padding([5, 7])
                .width(Length::Fill),
                button(text(if busy { "Cloning…" } else { "Clone" }).size(10))
                    .on_press_maybe(
                        (!busy).then(|| Message::RepoHostSetupCloneRequested(clone_repo))
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                text("Save the host now and finish setup later.")
                    .size(10)
                    .color(theme::MUTED)
                    .width(Length::Fill),
                button(text("Plan setup").size(10))
                    .on_press_maybe(
                        (!busy).then(|| Message::RepoHostSetupPlannedRequested(planned_repo))
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        ]
        .spacing(7)
        .into()
    };
    let status: Element<'_, Message> = state
        .repo_host_setup_status(&repo.id)
        .map(|status| text(status).size(9).color(theme::MUTED).into())
        .unwrap_or_else(|| Space::new().height(0).into());
    column![
        text("Available Hosts").size(11),
        text(
            "Hosts where this project is set up. Project paths and worktree settings are host-specific."
        )
        .size(10)
        .color(theme::MUTED),
        setup_rows,
        rule::horizontal(1),
        text("Add project to host").size(10),
        controls,
        status,
    ]
    .spacing(7)
    .into()
}

fn git_content(state: &AppState) -> Element<'_, Message> {
    let mut repos = column![].spacing(8);
    let mut hooks = column![].spacing(12);
    let mut mcp_configs = column![].spacing(12);
    for repo in state.repos() {
        let hook = state.ui_settings().repo_hook_settings.get(&repo.id.0);
        let setup_script = hook.map(|value| value.setup_script.as_str()).unwrap_or("");
        let archive_script = hook
            .map(|value| value.archive_script.as_str())
            .unwrap_or("");
        let run_policy = hook
            .map(|value| value.setup_run_policy.as_str())
            .unwrap_or("run-by-default");
        let wait_for_setup = hook
            .map(|value| value.setup_agent_startup_policy.as_str())
            .unwrap_or("start-immediately")
            == "wait-for-setup";
        let source_policy = hook
            .and_then(|value| value.command_source_policy.as_deref())
            .unwrap_or(
                if setup_script.trim().is_empty() && archive_script.trim().is_empty() {
                    "shared-only"
                } else {
                    "local-only"
                },
            );
        let name_repo = repo.id.clone();
        let icon_repo = repo.id.clone();
        let color_repo = repo.id.clone();
        let base_repo = repo.id.clone();
        let location_repo = repo.id.clone();
        let draft_repo = repo.id.clone();
        let add_repo = repo.id.clone();
        let worktree_location = state
            .ui_settings()
            .repo_worktree_base_paths
            .get(&repo.id.0)
            .map(String::as_str)
            .unwrap_or("");
        let repo_icon = state
            .ui_settings()
            .repo_icons
            .get(&repo.id.0)
            .map(String::as_str)
            .unwrap_or("");
        let repo_color = state
            .ui_settings()
            .repo_badge_colors
            .get(&repo.id.0)
            .map(String::as_str)
            .unwrap_or("#737373");
        let shared_paths = state
            .ui_settings()
            .repo_symlink_paths
            .get(&repo.id.0)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut linked_path_rows = column![].spacing(4);
        if shared_paths.is_empty() {
            linked_path_rows = linked_path_rows.push(
                text("No shared paths configured for this repository.")
                    .size(10)
                    .color(theme::MUTED),
            );
        } else {
            for path in shared_paths {
                let remove_repo = repo.id.clone();
                let remove_path = path.clone();
                linked_path_rows = linked_path_rows.push(
                    row![
                        icons::view(Icon::Link, 11.0, theme::MUTED),
                        text(path.clone()).size(10).width(Length::Fill),
                        button(text("Remove").size(9))
                            .on_press(Message::RepoSharedPathRemoved(remove_repo, remove_path))
                            .padding([3, 6])
                            .style(theme::ghost_button),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );
            }
        }
        let shared_draft = state.repo_shared_path_draft(&repo.id);
        let presets = state
            .ui_settings()
            .repo_sparse_presets
            .get(&repo.id.0)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut preset_rows = column![].spacing(5);
        if presets.is_empty() {
            preset_rows = preset_rows.push(
                text("No sparse presets saved for this repository.")
                    .size(10)
                    .color(theme::MUTED),
            );
        } else {
            for preset in presets {
                let remove_repo = repo.id.clone();
                let remove_id = preset.id.clone();
                preset_rows = preset_rows.push(
                    row![
                        column![
                            text(preset.name.clone()).size(10),
                            text(preset.directories.join(", "))
                                .size(9)
                                .color(theme::MUTED),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        button(text("Remove").size(9))
                            .on_press(Message::RepoSparsePresetRemoved(remove_repo, remove_id))
                            .padding([3, 6])
                            .style(theme::ghost_button),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );
            }
        }
        let preset_name_draft = state.repo_sparse_preset_name_draft(&repo.id);
        let preset_directories_draft = state.repo_sparse_preset_directory_draft(&repo.id);
        let preset_name_repo = repo.id.clone();
        let preset_directories_repo = repo.id.clone();
        let save_preset_repo = repo.id.clone();
        let fork_sync_section = repo_fork_sync_section(state, repo);
        let has_fork_sync_section = fork_sync_section.is_some();
        repos = repos.push(
            container(
                column![
                    row![
                        icons::view(Icon::GitBranch, 12.0, theme::MUTED),
                        text(repo.display_name.clone()).size(12).width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    text("Identity").size(11),
                    text("Project-specific display details for the sidebar and tabs.")
                        .size(10)
                        .color(theme::MUTED),
                    row![
                        text_input("Display Name", &repo.display_name)
                            .on_input(move |value| Message::RepoDisplayNameChanged(
                                name_repo.clone(),
                                value
                            ))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::Fill),
                        text_input("Icon or emoji", repo_icon)
                            .on_input(move |value| Message::RepoIconChanged(
                                icon_repo.clone(),
                                value
                            ))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::Fixed(110.0)),
                        text_input("#737373", repo_color)
                            .on_input(move |value| Message::RepoBadgeColorChanged(
                                color_repo.clone(),
                                value
                            ))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::Fixed(100.0)),
                    ]
                    .spacing(7),
                    rule::horizontal(1),
                    fork_sync_section.unwrap_or_else(|| Space::new().height(0).into()),
                    if has_fork_sync_section {
                        rule::horizontal(1)
                    } else {
                        rule::horizontal(0)
                    },
                    repo_host_setups_section(state, repo),
                    rule::horizontal(1),
                    repo_external_worktree_section(state, repo),
                    rule::horizontal(1),
                    repo_source_control_ai_section(state, repo),
                    rule::horizontal(1),
                    text("Default Worktree Base").size(11),
                    text("Default base branch or ref when creating worktrees.")
                        .size(10)
                        .color(theme::MUTED),
                    text_input(
                        "Use the primary branch",
                        repo.worktree_base_ref.as_deref().unwrap_or("")
                    )
                    .on_input(move |value| Message::RepoBaseRefChanged(
                        base_repo.clone(),
                        value
                    ))
                    .size(11)
                    .padding([6, 8]),
                    text("Worktree Location").size(11),
                    text("Project-specific directory for new worktrees. Relative paths resolve from this project root.")
                        .size(10)
                        .color(theme::MUTED),
                    text_input("Use global workspace directory", worktree_location)
                        .on_input(move |value| Message::RepoWorktreeBasePathChanged(
                            location_repo.clone(),
                            value
                        ))
                        .size(11)
                        .padding([6, 8]),
                    text("Worktree Shared Paths").size(11),
                    text(
                        "APFS clone-copied on macOS when possible, otherwise symlinked from the primary checkout."
                    )
                    .size(10)
                    .color(theme::MUTED),
                    linked_path_rows,
                    row![
                        text_input("Type a path (e.g. .env or node_modules)", shared_draft)
                            .on_input(move |value| Message::RepoSharedPathsChanged(
                                draft_repo.clone(),
                                value
                            ))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::Fill),
                        button(text("Add Path").size(10))
                            .on_press_maybe(
                                (!shared_draft.trim().is_empty())
                                    .then(|| Message::RepoSharedPathAdded(add_repo))
                            )
                            .padding([6, 8])
                            .style(theme::ghost_button),
                    ]
                    .spacing(7)
                    .align_y(Alignment::Center),
                    rule::horizontal(1),
                    text("Sparse Checkout Presets").size(11),
                    text("Saved directory sets available when creating a new worktree.")
                        .size(10)
                        .color(theme::MUTED),
                    preset_rows,
                    row![
                        text_input("Preset name", preset_name_draft)
                            .on_input(move |value| Message::RepoSparsePresetNameChanged(
                                preset_name_repo.clone(),
                                value
                            ))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::Fixed(150.0)),
                        text_input(
                            "Directories, comma or newline separated",
                            preset_directories_draft
                        )
                        .on_input(move |value| Message::RepoSparsePresetDirectoriesChanged(
                            preset_directories_repo.clone(),
                            value
                        ))
                        .size(11)
                        .padding([6, 8])
                        .width(Length::Fill),
                        button(text("Save Preset").size(10))
                            .on_press_maybe(
                                (!preset_name_draft.trim().is_empty()
                                    && !preset_directories_draft.trim().is_empty())
                                .then(|| Message::RepoSparsePresetSaved(save_preset_repo))
                            )
                            .padding([6, 8])
                            .style(theme::ghost_button),
                    ]
                    .spacing(7)
                    .align_y(Alignment::Center),
                ]
                .spacing(7),
            )
            .padding(10)
            .style(theme::active_card),
        );
        let setup_repo = repo.id.clone();
        let archive_repo = repo.id.clone();
        let policy_repo = repo.id.clone();
        let wait_repo = repo.id.clone();
        let source_repo = repo.id.clone();
        let shared = crate::repo_hooks::load_shared_scripts(&repo.path).ok();
        let shared_summary = match shared {
            Some(shared) if shared.setup.is_some() || shared.archive.is_some() => {
                "orca.yaml hooks detected"
            }
            _ => "No shared orca.yaml hooks detected",
        };
        hooks = hooks.push(
            container(
                column![
                    row![
                        icons::view(Icon::GitBranch, 12.0, theme::MUTED),
                        column![
                            text(repo.display_name.clone()).size(12),
                            text(shared_summary).size(10).color(theme::MUTED),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        pick_list(
                            vec![
                                "shared-only".to_string(),
                                "local-only".to_string(),
                                "run-both".to_string(),
                            ],
                            Some(source_policy.to_string()),
                            move |value| Message::RepoHookSourcePolicySelected(
                                source_repo.clone(),
                                value
                            ),
                        )
                        .width(Length::Fixed(130.0))
                        .text_size(10),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    text("Setup Script").size(11),
                    text_input("Local command run after workspace creation", setup_script)
                        .on_input(move |value| {
                            Message::RepoSetupScriptChanged(setup_repo.clone(), value)
                        })
                        .size(11)
                        .padding([6, 8]),
                    row![
                        text("When to run").size(11).width(Length::Fill),
                        pick_list(
                            vec![
                                "ask".to_string(),
                                "run-by-default".to_string(),
                                "skip-by-default".to_string(),
                            ],
                            Some(run_policy.to_string()),
                            move |value| Message::RepoSetupRunPolicySelected(
                                policy_repo.clone(),
                                value
                            ),
                        )
                        .width(Length::Fixed(150.0))
                        .text_size(10),
                    ]
                    .align_y(Alignment::Center),
                    action_switch_row(
                        "Wait for setup before starting agent",
                        "Use when setup installs dependencies, MCP servers, or configuration needed at startup.",
                        wait_for_setup,
                        Message::RepoSetupAgentWaitToggled(wait_repo),
                    ),
                    text("Archive Script").size(11),
                    text_input("Local command run before workspace removal", archive_script)
                        .on_input(move |value| {
                            Message::RepoArchiveScriptChanged(archive_repo.clone(), value)
                        })
                        .size(11)
                        .padding([6, 8]),
                ]
                .spacing(8),
            )
            .padding(10)
            .style(theme::active_card),
        );
        mcp_configs = mcp_configs.push(mcp_config_repo_card(state, repo));
    }
    let custom_prefix: Element<'_, Message> = if state.ui_settings().branch_prefix == "custom" {
        text_setting_row(
            "Custom prefix",
            &state.ui_settings().branch_prefix_custom,
            UiTextSetting::BranchPrefixCustom,
        )
    } else {
        Space::new().height(0).into()
    };
    column![
        source_control_ai_card(state),
        settings_card(
            column![
                subsection_title(
                    "Repository defaults",
                    Some("Base branches are detected when projects are added.")
                ),
                repos,
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title("Source control", None),
                switch_row(
                    "Confirm workspace deletion",
                    "Protect dirty worktrees from accidental removal.",
                    state.ui_settings().confirm_workspace_delete,
                    Some(UiSetting::ConfirmWorkspaceDelete),
                ),
                switch_row(
                    "Refresh local base ref",
                    "Fetch and refresh the local base branch before creating a workspace.",
                    state.ui_settings().refresh_local_base_ref,
                    Some(UiSetting::RefreshLocalBaseRef),
                ),
                switch_row(
                    "Rename branch from work",
                    "Rename auto-generated branches from the first agent prompt.",
                    state.ui_settings().auto_rename_branch_from_work,
                    Some(UiSetting::AutoRenameBranchFromWork),
                ),
                choice_row(
                    "Branch prefix",
                    &state.ui_settings().branch_prefix,
                    UiChoice::BranchPrefix,
                ),
                custom_prefix,
                switch_row(
                    "GitHub attribution",
                    "Add Orca-style attribution to generated commits and reviews.",
                    state.ui_settings().enable_github_attribution,
                    Some(UiSetting::EnableGithubAttribution),
                ),
                switch_row(
                    "Show ignored files",
                    "Include Git-ignored files in the file explorer.",
                    state.ui_settings().show_git_ignored_files,
                    Some(UiSetting::ShowGitIgnoredFiles),
                ),
                switch_row(
                    "Compare against upstream",
                    "Prefer the current branch upstream as the source-control compare base.",
                    state.ui_settings().source_control_compare_against_upstream,
                    Some(UiSetting::SourceControlCompareAgainstUpstream),
                ),
                choice_row(
                    "Changes layout",
                    &state.ui_settings().source_control_view_mode,
                    UiChoice::SourceControlViewMode,
                ),
                choice_row(
                    "Group order",
                    &state.ui_settings().source_control_group_order,
                    UiChoice::SourceControlGroupOrder,
                ),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "Worktree Hooks",
                    Some(
                        "Scripts run when worktrees are created or archived. Local scripts stay on this Mac; orca.yaml scripts are shared with the repository."
                    )
                ),
                hooks,
            ]
            .spacing(12)
        ),
        settings_card(
            column![
                subsection_title(
                    "MCP Configs",
                    Some(
                        "Inspect MCP server definitions that agents can use while working in each repository."
                    )
                ),
                mcp_configs,
            ]
            .spacing(12)
        ),
    ]
    .spacing(14)
    .into()
}

fn mcp_config_repo_card<'a>(
    state: &'a AppState,
    repo: &'a suaegi_core::domain::Repo,
) -> Element<'a, Message> {
    let Some((_worktree, root)) = state.mcp_target_for_repo(&repo.id) else {
        return container(
            column![
                text(repo.display_name.clone()).size(12),
                text("No local workspace is available for MCP inspection.")
                    .size(10)
                    .color(theme::MUTED),
            ]
            .spacing(3),
        )
        .padding(10)
        .style(theme::active_card)
        .into();
    };
    let configs = crate::mcp_config::inspect_root(&root);
    let detected = configs
        .iter()
        .filter(|config| config.inspection.exists)
        .count();
    let servers = configs
        .iter()
        .map(|config| config.inspection.servers.len())
        .sum::<usize>();
    let mut rows = column![row![
        column![
            text(repo.display_name.clone()).size(12),
            text(format!(
                "{detected} detected · {servers} server{}",
                if servers == 1 { "" } else { "s" }
            ))
            .size(10)
            .color(theme::MUTED),
        ]
        .spacing(2)
        .width(Length::Fill),
        button(
            text(if state.mcp_create_confirming(&repo.id) {
                "Confirm create"
            } else {
                "Add .mcp.json"
            })
            .size(10)
        )
        .on_press_maybe(
            (detected == 0).then(|| Message::McpStarterCreateRequested(repo.id.clone()))
        )
        .padding([4, 7])
        .style(if state.mcp_create_confirming(&repo.id) {
            theme::selected_button
        } else {
            theme::ghost_button
        }),
    ]
    .align_y(Alignment::Center)]
    .spacing(7);
    for config in configs {
        let relative = config.inspection.candidate.relative_path;
        let status = if config.read_error.is_some() {
            "Unreadable".to_string()
        } else {
            match config.inspection.status {
                suaegi_mcp::McpConfigStatus::Missing => "Not found".to_string(),
                suaegi_mcp::McpConfigStatus::Invalid => "Invalid JSON".to_string(),
                suaegi_mcp::McpConfigStatus::Valid if config.inspection.servers.is_empty() => {
                    "No servers".to_string()
                }
                suaegi_mcp::McpConfigStatus::Valid => {
                    format!("{} servers", config.inspection.servers.len())
                }
            }
        };
        let repo_id = repo.id.clone();
        let open = config
            .inspection
            .exists
            .then(|| Message::McpConfigOpenRequested(repo_id, relative.to_string()));
        let mut server_rows = column![].spacing(3);
        for server in &config.inspection.servers {
            let detail = server
                .url
                .as_deref()
                .or(server.command.as_deref())
                .or(server.issue.as_deref())
                .unwrap_or("Invalid server");
            server_rows = server_rows.push(
                row![
                    text(server.name.clone())
                        .size(10)
                        .width(Length::Fixed(120.0)),
                    text(format!("{:?}", server.transport).to_lowercase())
                        .size(9)
                        .color(theme::MUTED)
                        .width(Length::Fixed(50.0)),
                    text(detail.to_string())
                        .size(9)
                        .color(theme::MUTED)
                        .width(Length::Fill),
                    text(format!("{:?}", server.status).to_lowercase())
                        .size(9)
                        .color(theme::MUTED),
                ]
                .spacing(5),
            );
        }
        let error = config
            .read_error
            .as_deref()
            .or(config.inspection.error.as_deref())
            .map(str::to_string);
        let error: Element<'_, Message> = error
            .map(|error| text(error).size(9).color(theme::MUTED).into())
            .unwrap_or_else(|| Space::new().height(0).into());
        rows = rows.push(
            column![
                row![
                    text(config.inspection.candidate.label)
                        .size(11)
                        .width(Length::Fixed(110.0)),
                    text(relative)
                        .size(10)
                        .color(theme::MUTED)
                        .width(Length::Fill),
                    text(status).size(9).color(theme::MUTED),
                    button(text("Open").size(10))
                        .on_press_maybe(open)
                        .padding([3, 6])
                        .style(theme::ghost_button),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                error,
                server_rows,
            ]
            .spacing(3),
        );
    }
    container(rows).padding(10).style(theme::active_card).into()
}

fn task_sources_content(state: &AppState) -> Element<'_, Message> {
    let linear_skill: Element<'_, Message> = if state.linear().workspace.is_some() {
        let canonical = installed_agent_skill("orca-linear");
        let legacy = installed_agent_skill("linear-tickets");
        let installed = canonical.as_ref().or(legacy.as_ref());
        let update_name = if canonical.is_some() {
            "orca-linear"
        } else {
            "linear-tickets"
        };
        let command = if installed.is_some() {
            format!("npx skills update {update_name} --global")
        } else {
            "npx skills add https://github.com/stablyai/orca --skill orca-linear --global"
                .to_string()
        };
        column![
            rule::horizontal(1),
            subsection_title(
                "Linear skill",
                Some("Give agents the skill to read and update linked Linear tickets through Suaegi.")
            ),
            row![
                text(installed.map_or(
                    "Not installed".to_string(),
                    |path| format!("Installed at {}", path.display())
                ))
                .size(11)
                .color(theme::MUTED)
                .width(Length::Fill),
                button(text(if state.cli_installed() {
                    if installed.is_some() { "Update skill" } else { "Install skill" }
                } else {
                    "Install Suaegi CLI first"
                }).size(11))
                    .on_press(if state.cli_installed() {
                        Message::SettingsTerminalCommandRequested(command)
                    } else {
                        Message::OnboardingInstallCli
                    })
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            text("Use /orca-linear to read ticket context, post updates, change status, and attach PR/MR links.")
                .size(10)
                .color(theme::MUTED),
        ]
        .spacing(10)
        .into()
    } else {
        Space::new().height(0).into()
    };
    settings_card(
        column![
            subsection_title(
                "Task providers",
                Some("Tasks can load GitHub work through gh and Jira work through its connected site.")
            ),
            choice_row(
                "Default source",
                &state.ui_settings().default_task_source,
                UiChoice::DefaultTaskSource,
            ),
            choice_row(
                "Default view",
                &state.ui_settings().default_task_view_preset,
                UiChoice::DefaultTaskViewPreset,
            ),
            row![
                text("GitHub").size(12).width(Length::Fill),
                text("gh CLI").size(11).color(theme::MUTED),
                button(text("Open Tasks").size(11))
                    .on_press(Message::TasksOpened)
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            switch_row(
                "Show GitHub",
                "Keep GitHub issues, pull requests, and projects in Tasks.",
                state.ui_settings().show_github_tasks,
                Some(UiSetting::ShowGithubTasks),
            ),
            row![
                text("GitLab").size(12).width(Length::Fill),
                text("glab CLI").size(11).color(theme::MUTED),
                button(text("Open Tasks").size(11))
                    .on_press(Message::TasksOpened)
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            switch_row(
                "Show GitLab",
                "Keep GitLab issues, merge requests, and todos in Tasks.",
                state.ui_settings().show_gitlab_tasks,
                Some(UiSetting::ShowGitlabTasks),
            ),
            row![
                text("Jira").size(12).width(Length::Fill),
                text(if state.jira().connection.is_some() {
                    "Connected"
                } else {
                    "Not connected"
                })
                .size(11)
                .color(theme::MUTED),
                button(text("Integrations").size(11))
                    .on_press(Message::SettingsOpened(SettingsSection::Integrations))
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            switch_row(
                "Show Jira",
                "Keep Jira issues available in Tasks.",
                state.ui_settings().show_jira_tasks,
                Some(UiSetting::ShowJiraTasks),
            ),
            switch_row(
                "Show Linear",
                "Keep Linear issues available in Tasks when connected.",
                state.ui_settings().show_linear_tasks,
                Some(UiSetting::ShowLinearTasks),
            ),
            linear_skill,
        ]
        .spacing(13),
    )
}

fn terminal_content(state: &AppState) -> Element<'_, Message> {
    let sessions = state.panes().map_or(0, |panes| panes.iter().count());
    column![
        settings_card(
            column![
                subsection_title(
                    "Import from Ghostty",
                    Some(
                        "Import supported typography, cursor, padding, opacity, blur, and color settings."
                    )
                ),
                row![
                    button(
                        text(if state.ghostty_importing() {
                            "Importing…"
                        } else {
                            "Import Ghostty Settings"
                        })
                        .size(11)
                    )
                    .on_press_maybe(
                        (!state.ghostty_importing()).then_some(Message::GhosttyImportRequested)
                    )
                    .padding([6, 9])
                    .style(theme::ghost_button),
                    text(state.ghostty_import_status().unwrap_or(
                        "Ghostty config files are discovered using Ghostty's platform load order."
                    ))
                    .size(10)
                    .color(theme::MUTED),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(10)
        ),
        terminal_custom_themes_card(state),
        settings_card(
            column![
                subsection_title(
                    "Workspace Setup Script",
                    Some(
                        "Where the repository setup script runs when a new workspace is created."
                    )
                ),
                choice_row(
                    "Setup Script Location",
                    &state.ui_settings().setup_script_launch_mode,
                    UiChoice::SetupScriptLaunchMode,
                ),
                text(
                    "\"New Tab\" runs Setup in the background without stealing focus. Split modes keep its output beside the primary terminal."
                )
                .size(10)
                .color(theme::MUTED),
            ]
            .spacing(10)
        ),
        settings_card(
            column![
                subsection_title(
                    "Terminal sessions",
                    Some("Suaegi keeps PTY sessions alive through its local daemon.")
                ),
                row![
                    text("Running sessions").size(12).width(Length::Fill),
                    text(sessions).size(12),
                ],
                switch_row(
                    "Scope history by workspace",
                    "Keep shell history isolated between workspaces.",
                    state.ui_settings().terminal_scope_history_by_worktree,
                    Some(UiSetting::TerminalScopeHistoryByWorktree),
                ),
                choice_row_owned(
                    "Scrollback",
                    format!("{} rows", state.ui_settings().terminal_scrollback_rows),
                    UiChoice::TerminalScrollbackRows,
                ),
                choice_row(
                    "Shortcut priority",
                    &state.ui_settings().terminal_shortcut_policy,
                    UiChoice::TerminalShortcutPolicy,
                ),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title("Appearance", None),
                choice_row(
                    "Font",
                    &state.ui_settings().terminal_font_family,
                    UiChoice::TerminalFontFamily,
                ),
                choice_row_owned(
                    "Font size",
                    format!("{} px", state.ui_settings().terminal_font_size),
                    UiChoice::TerminalFontSize,
                ),
                choice_row_owned(
                    "Font weight",
                    state.ui_settings().terminal_font_weight.to_string(),
                    UiChoice::TerminalFontWeight,
                ),
                choice_row_owned(
                    "Line height",
                    format!("{}%", state.ui_settings().terminal_line_height_percent),
                    UiChoice::TerminalLineHeight,
                ),
                choice_row_owned(
                    "Horizontal padding",
                    format!("{} px", state.ui_settings().terminal_padding_x),
                    UiChoice::TerminalPaddingX,
                ),
                choice_row_owned(
                    "Vertical padding",
                    format!("{} px", state.ui_settings().terminal_padding_y),
                    UiChoice::TerminalPaddingY,
                ),
                terminal_theme_choice_row(
                    state,
                    "Dark theme",
                    &state.ui_settings().terminal_theme_dark,
                    UiChoice::TerminalThemeDark,
                    false,
                ),
                switch_row(
                    "Separate light theme",
                    "Use a dedicated terminal palette while the app is in light mode.",
                    state.ui_settings().terminal_use_separate_light_theme,
                    Some(UiSetting::TerminalUseSeparateLightTheme),
                ),
                terminal_theme_choice_row(
                    state,
                    "Light theme",
                    &state.ui_settings().terminal_theme_light,
                    UiChoice::TerminalThemeLight,
                    true,
                ),
                choice_row_owned(
                    "Background opacity",
                    format!(
                        "{}%",
                        state.ui_settings().terminal_background_opacity_percent
                    ),
                    UiChoice::TerminalBackgroundOpacity,
                ),
                choice_row_owned(
                    "Inactive pane opacity",
                    format!(
                        "{}%",
                        state.ui_settings().terminal_inactive_pane_opacity_percent
                    ),
                    UiChoice::TerminalInactivePaneOpacity,
                ),
                choice_row_owned(
                    "Active pane opacity",
                    format!(
                        "{}%",
                        state.ui_settings().terminal_active_pane_opacity_percent
                    ),
                    UiChoice::TerminalActivePaneOpacity,
                ),
                choice_row_owned(
                    "Opacity transition",
                    format!(
                        "{} ms",
                        state.ui_settings().terminal_pane_opacity_transition_ms
                    ),
                    UiChoice::TerminalPaneOpacityTransition,
                ),
                choice_row_owned(
                    "Divider thickness",
                    format!("{} px", state.ui_settings().terminal_divider_thickness_px),
                    UiChoice::TerminalDividerThickness,
                ),
                text_setting_row(
                    "Dark divider color",
                    &state.ui_settings().terminal_divider_color_dark,
                    UiTextSetting::TerminalDividerColorDark,
                ),
                text_setting_row(
                    "Light divider color",
                    &state.ui_settings().terminal_divider_color_light,
                    UiTextSetting::TerminalDividerColorLight,
                ),
            ]
            .spacing(13)
        ),
        terminal_color_overrides_card(state),
        settings_card(
            column![
                subsection_title("Rendering & interaction", None),
                choice_row_owned(
                    "Normal scroll speed",
                    format!(
                        "{}x",
                        f32::from(state.ui_settings().terminal_scroll_sensitivity_percent) / 100.0
                    ),
                    UiChoice::TerminalScrollSensitivity,
                ),
                choice_row_owned(
                    "Fast scroll speed",
                    format!(
                        "{}x",
                        f32::from(state.ui_settings().terminal_fast_scroll_sensitivity_percent)
                            / 100.0
                    ),
                    UiChoice::TerminalFastScrollSensitivity,
                ),
                choice_row_owned(
                    "TUI wheel speed",
                    format!("{}x", state.ui_settings().terminal_tui_scroll_multiplier),
                    UiChoice::TerminalTuiScrollMultiplier,
                ),
                choice_row(
                    "GPU acceleration",
                    &state.ui_settings().terminal_gpu_acceleration,
                    UiChoice::TerminalGpuAcceleration,
                ),
                text("Auto uses the native GPU renderer when available. Off uses the CPU compatibility renderer after restarting Suaegi.")
                    .size(10)
                    .color(theme::MUTED),
                choice_row(
                    "Programming ligatures",
                    &state.ui_settings().terminal_ligatures,
                    UiChoice::TerminalLigatures,
                ),
                choice_row(
                    "Cursor style",
                    &state.ui_settings().terminal_cursor_style,
                    UiChoice::TerminalCursorStyle,
                ),
                switch_row(
                    "Blinking cursor",
                    "Animate the terminal cursor.",
                    state.ui_settings().terminal_cursor_blink,
                    Some(UiSetting::TerminalCursorBlink),
                ),
                choice_row_owned(
                    "Cursor opacity",
                    format!("{}%", state.ui_settings().terminal_cursor_opacity_percent),
                    UiChoice::TerminalCursorOpacity,
                ),
                switch_row(
                    "Hide mouse while typing",
                    "Hide the pointer after keyboard input until it moves.",
                    state.ui_settings().terminal_mouse_hide_while_typing,
                    Some(UiSetting::TerminalMouseHideWhileTyping),
                ),
                switch_row(
                    "Focus follows mouse",
                    "Focus a terminal pane when the pointer enters it.",
                    state.ui_settings().terminal_focus_follows_mouse,
                    Some(UiSetting::TerminalFocusFollowsMouse),
                ),
                switch_row(
                    "Copy on select",
                    "Copy terminal selections to the clipboard automatically.",
                    state.ui_settings().terminal_clipboard_on_select,
                    Some(UiSetting::TerminalClipboardOnSelect),
                ),
                switch_row(
                    "Allow OSC 52 clipboard",
                    "Allow terminal programs to write to the system clipboard.",
                    state.ui_settings().terminal_allow_osc52_clipboard,
                    Some(UiSetting::TerminalAllowOsc52Clipboard),
                ),
                switch_row(
                    "Right-click to paste",
                    "Paste clipboard contents with the secondary mouse button.",
                    state.ui_settings().terminal_right_click_to_paste,
                    Some(UiSetting::TerminalRightClickToPaste),
                ),
                choice_row(
                    "macOS Option as Alt",
                    &state.ui_settings().terminal_mac_option_as_alt,
                    UiChoice::TerminalMacOptionAsAlt,
                ),
                switch_row(
                    "JIS Yen as backslash",
                    "Translate the physical JIS Yen key to a backslash.",
                    state.ui_settings().terminal_jis_yen_to_backslash,
                    Some(UiSetting::TerminalJisYenToBackslash),
                ),
                text_setting_row(
                    "Word separators",
                    &state.ui_settings().terminal_word_separator,
                    UiTextSetting::TerminalWordSeparator,
                ),
            ]
            .spacing(13)
        ),
    ]
    .spacing(14)
    .into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalThemeChoice {
    value: String,
    label: String,
}

impl std::fmt::Display for TerminalThemeChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label)
    }
}

fn terminal_theme_choice_row<'a>(
    state: &'a AppState,
    title: &'a str,
    selected: &str,
    choice: UiChoice,
    light: bool,
) -> Element<'a, Message> {
    let builtins = if light {
        &["Builtin Tango Light", "Solarized Light", "One Light"][..]
    } else {
        &[
            "Ghostty Default Style Dark",
            "Dracula",
            "One Dark",
            "Nord",
            "Solarized Dark",
        ][..]
    };
    let mut choices = builtins
        .iter()
        .map(|value| TerminalThemeChoice {
            value: (*value).to_string(),
            label: (*value).to_string(),
        })
        .collect::<Vec<_>>();
    choices.extend(
        state
            .ui_settings()
            .terminal_custom_themes
            .iter()
            .filter(|theme| {
                theme.mode == "unknown"
                    || if light {
                        theme.mode == "light"
                    } else {
                        theme.mode == "dark"
                    }
            })
            .map(|theme| TerminalThemeChoice {
                value: crate::warp_theme_import::custom_selection(&theme.id),
                label: format!("{} · Custom", theme.name),
            }),
    );
    let selected = choices
        .iter()
        .find(|option| option.value == selected)
        .cloned()
        .or_else(|| choices.first().cloned());
    row![
        text(title).size(12).width(Length::Fill),
        pick_list(choices, selected, move |selected| {
            Message::UiChoiceSelected(choice, selected.value)
        })
        .width(Length::Fixed(176.0))
        .text_size(11),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn terminal_custom_themes_card(state: &AppState) -> Element<'_, Message> {
    let importing = state.terminal_theme_importing();
    let rows = if state.ui_settings().terminal_custom_themes.is_empty() {
        vec![text("No imported terminal themes.")
            .size(10)
            .color(theme::MUTED)
            .into()]
    } else {
        state
            .ui_settings()
            .terminal_custom_themes
            .iter()
            .map(|custom| {
                let detail = match custom.mode.as_str() {
                    "dark" => "Dark",
                    "light" => "Light",
                    _ => "Adaptive",
                };
                row![
                    column![
                        text(&custom.name).size(11),
                        text(format!("{detail} · {}", custom.source))
                            .size(10)
                            .color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(text("Remove").size(10))
                        .on_press(Message::CustomTerminalThemeRemoved(custom.id.clone()))
                        .padding([4, 7])
                        .style(theme::ghost_button),
                ]
                .align_y(Alignment::Center)
                .into()
            })
            .collect::<Vec<Element<'_, Message>>>()
    };
    settings_card(
        column![
            subsection_title(
                "Custom Terminal Themes",
                Some("Import Warp themes automatically or choose Warp-format YAML files.")
            ),
            row![
                button(
                    text(if importing {
                        "Importing…"
                    } else {
                        "Import from Warp"
                    })
                    .size(11)
                )
                .on_press_maybe((!importing).then_some(Message::WarpThemeImportRequested))
                .padding([6, 9])
                .style(theme::ghost_button),
                button(text("Import from YAML…").size(11))
                    .on_press_maybe((!importing).then_some(Message::YamlThemeImportRequested))
                    .padding([6, 9])
                    .style(theme::ghost_button),
            ]
            .spacing(7),
            text(state.terminal_theme_import_status().unwrap_or(
                "Warp channels and manually selected YAML files are imported into one catalog."
            ))
            .size(10)
            .color(theme::MUTED),
            column(rows).spacing(6),
        ]
        .spacing(10),
    )
}

fn terminal_color_overrides_card(state: &AppState) -> Element<'_, Message> {
    let fields = [
        (
            "Foreground",
            "foreground",
            UiTextSetting::TerminalColorForeground,
        ),
        (
            "Background",
            "background",
            UiTextSetting::TerminalColorBackground,
        ),
        ("Cursor", "cursor", UiTextSetting::TerminalColorCursor),
        (
            "Cursor accent",
            "cursorAccent",
            UiTextSetting::TerminalColorCursorAccent,
        ),
        (
            "Selection background",
            "selectionBackground",
            UiTextSetting::TerminalColorSelectionBackground,
        ),
        (
            "Selection foreground",
            "selectionForeground",
            UiTextSetting::TerminalColorSelectionForeground,
        ),
        ("Black", "black", UiTextSetting::TerminalColorBlack),
        ("Red", "red", UiTextSetting::TerminalColorRed),
        ("Green", "green", UiTextSetting::TerminalColorGreen),
        ("Yellow", "yellow", UiTextSetting::TerminalColorYellow),
        ("Blue", "blue", UiTextSetting::TerminalColorBlue),
        ("Magenta", "magenta", UiTextSetting::TerminalColorMagenta),
        ("Cyan", "cyan", UiTextSetting::TerminalColorCyan),
        ("White", "white", UiTextSetting::TerminalColorWhite),
        (
            "Bright black",
            "brightBlack",
            UiTextSetting::TerminalColorBrightBlack,
        ),
        (
            "Bright red",
            "brightRed",
            UiTextSetting::TerminalColorBrightRed,
        ),
        (
            "Bright green",
            "brightGreen",
            UiTextSetting::TerminalColorBrightGreen,
        ),
        (
            "Bright yellow",
            "brightYellow",
            UiTextSetting::TerminalColorBrightYellow,
        ),
        (
            "Bright blue",
            "brightBlue",
            UiTextSetting::TerminalColorBrightBlue,
        ),
        (
            "Bright magenta",
            "brightMagenta",
            UiTextSetting::TerminalColorBrightMagenta,
        ),
        (
            "Bright cyan",
            "brightCyan",
            UiTextSetting::TerminalColorBrightCyan,
        ),
        (
            "Bright white",
            "brightWhite",
            UiTextSetting::TerminalColorBrightWhite,
        ),
        ("Bold", "bold", UiTextSetting::TerminalColorBold),
    ];
    let mut rows = column![subsection_title(
        "Color overrides",
        Some("Optional #RGB or #RRGGBB values override the selected terminal theme.")
    )]
    .spacing(9);
    for (label, key, setting) in fields {
        rows = rows.push(text_setting_row(
            label,
            state.terminal_color_draft(key),
            setting,
        ));
    }
    settings_card(rows)
}

fn shortcuts_content(state: &AppState) -> Element<'_, Message> {
    let path = state
        .keybinding_snapshot()
        .map(|snapshot| snapshot.path.display().to_string())
        .unwrap_or_else(|| crate::keybindings::path().display().to_string());
    let mut rows = column![].spacing(9);
    let mut previous_group = "";
    for definition in suaegi_keys::KEYBINDING_DEFINITIONS {
        if definition.group != previous_group {
            rows = rows.push(text(definition.group).size(11).color(theme::MUTED));
            previous_group = definition.group;
        }
        let action = definition.id;
        let customized = state
            .keybinding_snapshot()
            .is_some_and(|snapshot| snapshot.overrides.contains_key(&action));
        rows = rows.push(
            column![row![
                column![
                    text(definition.title).size(12),
                    text(definition.id.as_str()).size(10).color(theme::MUTED),
                ]
                .spacing(1)
                .width(Length::Fixed(205.0)),
                text_input("Unassigned", state.keybinding_draft(action))
                    .on_input(move |value| Message::KeybindingDraftChanged(action, value))
                    .on_submit(Message::KeybindingApplyRequested(action))
                    .size(11)
                    .padding([5, 7])
                    .width(Length::Fill),
                button(text("Apply").size(11))
                    .on_press(Message::KeybindingApplyRequested(action))
                    .padding([5, 7])
                    .style(theme::ghost_button),
                button(text(if customized { "Reset" } else { "Default" }).size(11))
                    .on_press_maybe(customized.then_some(Message::KeybindingResetRequested(action)))
                    .padding([5, 7])
                    .style(theme::ghost_button),
            ]
            .spacing(6)
            .align_y(Alignment::Center),]
            .spacing(3),
        );
    }

    let mut diagnostics = column![].spacing(3);
    if let Some(snapshot) = state.keybinding_snapshot() {
        for diagnostic in &snapshot.diagnostics {
            diagnostics = diagnostics.push(text(diagnostic.message.clone()).size(11).color(
                if diagnostic.severity == suaegi_keys::Severity::Error {
                    iced::Color::from_rgb8(0xc0, 0x39, 0x2b)
                } else {
                    theme::MUTED
                },
            ));
        }
    }

    settings_card(
        column![
            subsection_title(
                "Keyboard Shortcuts",
                Some("Enter comma-separated chords such as Mod+P or Mod+Shift+F.")
            ),
            row![
                text(path).size(10).color(theme::MUTED).width(Length::Fill),
                button(text("Open JSON").size(11))
                    .on_press(Message::KeybindingsFileOpenRequested)
                    .padding([5, 7])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            diagnostics,
            rows,
        ]
        .spacing(12),
    )
}

fn stats_content(state: &AppState) -> Element<'_, Message> {
    let worktrees = state
        .repos()
        .iter()
        .map(|repo| state.worktrees_for(&repo.id).len())
        .sum::<usize>();
    let sessions = state.panes().map_or(0, |panes| panes.iter().count());
    let snapshot = state.usage_snapshot();
    let enabled_count = [
        state.ui_settings().claude_usage_enabled,
        state.ui_settings().codex_usage_enabled,
        state.ui_settings().opencode_usage_enabled,
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count();
    let data_provider_count = snapshot.map_or(0, |snapshot| {
        snapshot
            .providers
            .iter()
            .filter(|provider| provider.enabled && provider.total_tokens > 0)
            .count()
    });
    let usage_sessions = snapshot.map_or(0, |snapshot| {
        snapshot
            .providers
            .iter()
            .filter(|provider| provider.enabled)
            .map(|provider| provider.sessions)
            .sum::<usize>()
    });
    let total_tokens = snapshot.map_or(0, crate::usage::UsageSnapshot::total_tokens);
    let active_days = snapshot.map_or(0, crate::usage::UsageSnapshot::active_days);
    let cache_share = snapshot
        .and_then(crate::usage::UsageSnapshot::cache_share)
        .map(|value| format!("{}%", (value * 100.0).round()))
        .unwrap_or_else(|| "—".to_string());
    let estimated_cost = snapshot
        .and_then(crate::usage::UsageSnapshot::estimated_cost_usd)
        .map(|value| format!("${value:.2}"))
        .unwrap_or_else(|| "—".to_string());
    let provider_card = |provider: crate::usage::UsageProvider, enabled: bool| {
        let usage = snapshot.and_then(|snapshot| snapshot.provider(provider));
        let status = if state.usage_scanning() && enabled {
            "Scanning local logs…".to_string()
        } else if !enabled {
            "Disabled".to_string()
        } else if let Some(error) = usage.and_then(|usage| usage.error.as_deref()) {
            format!("Partial scan: {error}")
        } else if usage.is_some_and(|usage| usage.total_tokens > 0) {
            "Local usage found".to_string()
        } else {
            "No local usage found yet".to_string()
        };
        let details = usage.map_or_else(
            || "0 sessions • 0 events • 0 tokens".to_string(),
            |usage| {
                format!(
                    "{} sessions • {} events • {} tokens",
                    usage.sessions,
                    usage.events,
                    format_tokens(usage.total_tokens)
                )
            },
        );
        let breakdown = usage.map_or_else(
            || "No model or project data".to_string(),
            |usage| {
                format!(
                    "Top model: {}  •  Top project: {}",
                    usage.top_model.as_deref().unwrap_or("—"),
                    usage.top_project.as_deref().unwrap_or("—")
                )
            },
        );
        let token_mix = usage.map_or_else(
            || "Input 0 • Cached 0 • Output 0".to_string(),
            |usage| {
                format!(
                    "Input {} • Cached {} • Output {}{}",
                    format_tokens(usage.input_tokens),
                    format_tokens(usage.cached_input_tokens + usage.cache_write_tokens),
                    format_tokens(usage.output_tokens),
                    if usage.reasoning_tokens > 0 {
                        format!(" • Reasoning {}", format_tokens(usage.reasoning_tokens))
                    } else {
                        String::new()
                    }
                )
            },
        );
        let latest = usage
            .and_then(|usage| usage.daily.last())
            .map(|day| format!("Latest {} • {}", day.day, format_tokens(day.total_tokens)))
            .unwrap_or_else(|| "No daily activity".to_string());
        let provider_cost = usage
            .and_then(|usage| usage.estimated_cost_usd)
            .map(|cost| format!("API-equivalent estimate ${cost:.4}"))
            .unwrap_or_else(|| "API-equivalent cost unavailable".to_string());
        container(
            column![
                row![
                    column![
                        text(provider.label()).size(12),
                        text(status).size(11).color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(text(if enabled { "Disable" } else { "Enable" }).size(11))
                        .on_press(Message::UsageProviderToggled(provider))
                        .padding([5, 9])
                        .style(if enabled {
                            theme::ghost_button
                        } else {
                            theme::primary_dark_button
                        }),
                ]
                .align_y(Alignment::Center),
                text(details).size(11).color(theme::MUTED),
                text(breakdown).size(11).color(theme::MUTED),
                text(token_mix).size(11).color(theme::MUTED),
                text(format!("{latest} • {provider_cost}"))
                    .size(11)
                    .color(theme::MUTED),
            ]
            .spacing(6),
        )
        .padding([11, 13])
        .width(Length::Fill)
        .style(theme::context_panel)
    };
    let mut gemini_details = column![row![
        column![
            text("Gemini").size(12),
            text(if state.gemini_rate_limits_fetching() {
                "Refreshing OAuth quota…"
            } else if !state.ui_settings().gemini_cli_oauth_enabled {
                "Disabled in Providers & Accounts"
            } else if state
                .gemini_rate_limits()
                .is_some_and(|limits| limits.status == crate::rate_limits::RateLimitStatus::Ok)
            {
                "Gemini CLI quota available"
            } else {
                "Quota unavailable"
            })
            .size(11)
            .color(theme::MUTED),
        ]
        .spacing(2)
        .width(Length::Fill),
        button(
            container(
                text(if state.ui_settings().gemini_cli_oauth_enabled {
                    "●"
                } else {
                    "○"
                })
                .size(13)
            )
            .padding([2, 7])
            .style(if state.ui_settings().gemini_cli_oauth_enabled {
                theme::active_card
            } else {
                theme::chip
            })
        )
        .on_press(Message::UiSettingToggled(UiSetting::GeminiCliOauthEnabled))
        .padding(0)
        .style(theme::ghost_button),
    ]
    .align_y(Alignment::Center)]
    .spacing(6);
    if let Some(limits) = state.gemini_rate_limits() {
        for bucket in &limits.buckets {
            let percentage = crate::rate_limits::displayed_percentage(
                bucket.used_percent,
                &state.ui_settings().usage_percentage_mode,
            );
            let mode = if state.ui_settings().usage_percentage_mode == "remaining" {
                "remaining"
            } else {
                "used"
            };
            let reset = crate::rate_limits::reset_label(bucket.resets_at_unix_ms)
                .map(|label| format!(" • {label}"))
                .unwrap_or_default();
            gemini_details = gemini_details.push(
                row![
                    text(&bucket.name).size(11).width(Length::Fill),
                    text(format!("{percentage}% {mode}{reset}"))
                        .size(11)
                        .color(theme::MUTED),
                ]
                .align_y(Alignment::Center),
            );
        }
        if limits.buckets.is_empty() {
            gemini_details = gemini_details.push(
                text(
                    limits
                        .error
                        .as_deref()
                        .unwrap_or("No Gemini quota buckets were returned."),
                )
                .size(11)
                .color(theme::MUTED),
            );
        }
    }
    let gemini_card = container(gemini_details)
        .padding([11, 13])
        .width(Length::Fill)
        .style(theme::context_panel);
    column![
        settings_card(
            column![
                subsection_title(
                    "Local resources",
                    Some("Live counts from this Suaegi process.")
                ),
                stat_row("Projects", state.repos().len()),
                stat_row("Workspaces", worktrees),
                stat_row("Terminal sessions", sessions),
            ]
            .spacing(12)
        ),
        settings_card(
            column![
                row![
                    subsection_title("Usage Overview", Some(
                        if snapshot.is_some() {
                            "Updated after the latest local scan."
                        } else {
                            "Not scanned yet."
                        }
                    )),
                    Space::new().width(Length::Fill),
                    button(
                        text(if state.usage_scanning() {
                            "Scanning…"
                        } else {
                            "Refresh"
                        })
                        .size(11)
                    )
                    .on_press_maybe(
                        (!state.usage_scanning() && enabled_count > 0)
                            .then_some(Message::UsageRefreshRequested)
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
                ]
                .align_y(Alignment::Center),
                if enabled_count == 0 {
                    container(
                        column![
                            text("Start tracking tokens").size(13),
                            text(
                                "Enable a provider to scan local agent logs and build the combined token ledger."
                            )
                            .size(11)
                            .color(theme::MUTED),
                            row![
                                button(text("Enable Claude").size(11))
                                    .on_press(Message::UsageProviderToggled(
                                        crate::usage::UsageProvider::Claude
                                    ))
                                    .padding([6, 10])
                                    .style(theme::primary_dark_button),
                                button(text("Enable Codex").size(11))
                                    .on_press(Message::UsageProviderToggled(
                                        crate::usage::UsageProvider::Codex
                                    ))
                                    .padding([6, 10])
                                    .style(theme::ghost_button),
                                button(text("Enable OpenCode").size(11))
                                    .on_press(Message::UsageProviderToggled(
                                        crate::usage::UsageProvider::OpenCode
                                    ))
                                    .padding([6, 10])
                                    .style(theme::ghost_button),
                            ]
                            .spacing(8),
                        ]
                        .spacing(9),
                    )
                    .padding([14, 14])
                    .width(Length::Fill)
                    .style(theme::context_panel)
                } else {
                    container(
                        column![
                            row![
                                stat_value_card("Total tokens", format_tokens(total_tokens)),
                                stat_value_card("Est. cost", estimated_cost),
                            ]
                            .spacing(8),
                            row![
                                stat_value_card("Active days", active_days.to_string()),
                                stat_value_card("Cache share", cache_share),
                            ]
                            .spacing(8),
                        ]
                        .spacing(8),
                    )
                },
                row![
                    column![
                        text("Providers").size(13),
                        text(format!(
                            "{enabled_count} enabled • {data_provider_count} with data"
                        ))
                        .size(11)
                        .color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    container(text(format!("{usage_sessions} sessions")).size(11))
                        .padding([4, 8])
                        .style(theme::chip),
                ]
                .align_y(Alignment::Center),
                provider_card(
                    crate::usage::UsageProvider::Claude,
                    state.ui_settings().claude_usage_enabled
                ),
                provider_card(
                    crate::usage::UsageProvider::Codex,
                    state.ui_settings().codex_usage_enabled
                ),
                provider_card(
                    crate::usage::UsageProvider::OpenCode,
                    state.ui_settings().opencode_usage_enabled
                ),
                gemini_card,
                text("Usage scanning is opt-in and local-only. Prompt and response text never leaves this Mac.")
                    .size(11)
                    .color(theme::MUTED),
                switch_row(
                    "Usage in status bar",
                    "Show provider limits in the bottom status bar.",
                    state.ui_settings().show_usage_status,
                    Some(UiSetting::ShowUsageStatus),
                ),
            ]
            .spacing(12)
        ),
    ]
    .spacing(14)
    .into()
}

fn stat_row(label: &'static str, value: usize) -> Element<'static, Message> {
    row![
        text(label).size(12).width(Length::Fill),
        text(value.to_string()).size(12),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn stat_value_card(label: &'static str, value: String) -> Element<'static, Message> {
    container(
        column![
            text(label).size(11).color(theme::MUTED),
            text(value).size(16),
        ]
        .spacing(3),
    )
    .padding([9, 11])
    .width(Length::Fill)
    .style(theme::context_panel)
    .into()
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        format!("{:.1}B", tokens as f64 / 1_000_000_000.0)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn appearance_content(state: &AppState) -> Element<'_, Message> {
    let tint_controls: Element<'_, Message> =
        if state.ui_settings().left_sidebar_appearance == "tinted" {
            column![
                text_setting_row(
                    "Sidebar Tint",
                    &state.ui_settings().left_sidebar_tint_color,
                    UiTextSetting::LeftSidebarTintColor,
                ),
                choice_row_owned(
                    "Tint Strength",
                    format!("{}%", state.ui_settings().left_sidebar_tint_opacity_percent),
                    UiChoice::LeftSidebarTintOpacity,
                ),
            ]
            .spacing(8)
            .into()
        } else {
            Space::new().height(0).into()
        };
    column![
        settings_card(
            column![
                subsection_title(
                    "Interface",
                    Some("Theme, zoom, and the native workspace interface.")
                ),
                choice_row("Theme", &state.ui_settings().theme, UiChoice::Theme,),
                choice_row_owned(
                    "Language",
                    crate::i18n::language_label_owned(&state.ui_settings().ui_language),
                    UiChoice::Language,
                ),
                choice_row_owned(
                    "UI Zoom",
                    format!("{}%", state.ui_settings().ui_zoom_percent),
                    UiChoice::UiZoom,
                ),
                choice_row(
                    "IDE Font",
                    &state.ui_settings().app_font_family,
                    UiChoice::AppFontFamily,
                ),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "Terminal",
                    Some("Typography and color defaults used by terminal panes.")
                ),
                choice_row_owned(
                    "Font Size",
                    format!("{} px", state.ui_settings().terminal_font_size),
                    UiChoice::TerminalFontSize,
                ),
                choice_row(
                    "Font Family",
                    &state.ui_settings().terminal_font_family,
                    UiChoice::TerminalFontFamily,
                ),
                terminal_theme_choice_row(
                    state,
                    "Dark Theme",
                    &state.ui_settings().terminal_theme_dark,
                    UiChoice::TerminalThemeDark,
                    false,
                ),
                switch_row(
                    "Separate light theme",
                    "Use a light-specific terminal palette and divider color.",
                    state.ui_settings().terminal_use_separate_light_theme,
                    Some(UiSetting::TerminalUseSeparateLightTheme),
                ),
                terminal_theme_choice_row(
                    state,
                    "Light Theme",
                    &state.ui_settings().terminal_theme_light,
                    UiChoice::TerminalThemeLight,
                    true,
                ),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "Window & Sidebar",
                    Some("Choose sidebar treatment and visible status indicators.")
                ),
                choice_row(
                    "Left Sidebar Appearance",
                    &state.ui_settings().left_sidebar_appearance,
                    UiChoice::LeftSidebarAppearance,
                ),
                tint_controls,
                choice_row(
                    "Usage percentages",
                    &state.ui_settings().usage_percentage_mode,
                    UiChoice::UsagePercentageMode,
                ),
                switch_row(
                    "Show app name",
                    "Show Suaegi in the native title bar.",
                    state.ui_settings().show_titlebar_app_name,
                    Some(UiSetting::ShowTitlebarAppName),
                ),
                switch_row(
                    "Tasks button",
                    "Show Tasks in the left sidebar.",
                    state.ui_settings().show_tasks_button,
                    Some(UiSetting::ShowTasksButton),
                ),
                switch_row(
                    "Automations button",
                    "Show Automations in the left sidebar.",
                    state.ui_settings().show_automations_button,
                    Some(UiSetting::ShowAutomationsButton),
                ),
                switch_row(
                    "Pinned workspaces in groups",
                    "Also show pinned workspaces inside their normal project groups.",
                    state.ui_settings().show_pinned_worktrees_in_groups,
                    Some(UiSetting::ShowPinnedWorktreesInGroups),
                ),
                switch_row(
                    "Menu bar icon",
                    "Show the Suaegi menu bar item on macOS.",
                    state.ui_settings().show_menu_bar_icon,
                    Some(UiSetting::ShowMenuBarIcon),
                ),
                switch_row(
                    "Window background blur",
                    "Use native translucent window material where supported.",
                    state.ui_settings().window_background_blur,
                    Some(UiSetting::WindowBackgroundBlur),
                ),
                switch_row(
                    "Usage",
                    "Show local provider and agent usage in the status bar.",
                    state.ui_settings().show_usage_status,
                    Some(UiSetting::ShowUsageStatus),
                ),
                switch_row(
                    "Resource Manager",
                    "Show terminal session and resource controls.",
                    state.ui_settings().show_resource_status,
                    Some(UiSetting::ShowResourceStatus),
                ),
                switch_row(
                    "Ports",
                    "Show live workspace and external listener ports.",
                    state.ui_settings().show_ports_status,
                    Some(UiSetting::ShowPortsStatus),
                ),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "App Icon",
                    Some("Choose the icon shown in the Dock and window switcher.")
                ),
                app_icon_selector(&state.ui_settings().app_icon),
            ]
            .spacing(13)
        ),
    ]
    .spacing(14)
    .into()
}

fn app_icon_selector(selected: &str) -> Element<'static, Message> {
    let selected = crate::appearance::normalize_app_icon_id(selected);
    let index = crate::appearance::APP_ICON_IDS
        .iter()
        .position(|candidate| *candidate == selected)
        .unwrap_or(0);
    let previous = crate::appearance::APP_ICON_IDS[(index + crate::appearance::APP_ICON_IDS.len()
        - 1)
        % crate::appearance::APP_ICON_IDS.len()];
    let next = crate::appearance::APP_ICON_IDS[(index + 1) % crate::appearance::APP_ICON_IDS.len()];
    let label = match selected {
        "watercolor" => "Watercolor Dolphin",
        "blue" => "Blue Dolphin",
        _ => "Classic Dolphin",
    };

    column![
        row![
            button(text("‹").size(24))
                .on_press(Message::UiChoiceSelected(
                    UiChoice::AppIcon,
                    previous.to_string(),
                ))
                .padding([8, 12])
                .style(theme::ghost_button),
            image(iced::widget::image::Handle::from_bytes(
                crate::appearance::app_icon_bytes(selected),
            ))
            .width(96)
            .height(96)
            .border_radius(18),
            button(text("›").size(24))
                .on_press(Message::UiChoiceSelected(
                    UiChoice::AppIcon,
                    next.to_string(),
                ))
                .padding([8, 12])
                .style(theme::ghost_button),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        text(label).size(11).color(theme::MUTED),
    ]
    .spacing(6)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn notifications_content(state: &AppState) -> Element<'_, Message> {
    let custom_sound: Element<'_, Message> =
        if let Some(path) = &state.ui_settings().notification_custom_sound_path {
            column![
                row![
                    text(
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("Custom audio")
                    )
                    .size(11)
                    .width(Length::Fill),
                    button(text("Replace").size(11))
                        .on_press(Message::NotificationCustomSoundBrowseRequested)
                        .padding([5, 8])
                        .style(theme::ghost_button),
                    button(text("Clear").size(11))
                        .on_press(Message::NotificationCustomSoundCleared)
                        .padding([5, 8])
                        .style(theme::ghost_button),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text(path.display().to_string())
                    .size(10)
                    .color(theme::MUTED),
            ]
            .spacing(4)
            .into()
        } else {
            row![
                text("Use an OGG, MP3, WAV, M4A, AAC, or FLAC file.")
                    .size(11)
                    .color(theme::MUTED)
                    .width(Length::Fill),
                button(text("Choose audio…").size(11))
                    .on_press(Message::NotificationCustomSoundBrowseRequested)
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center)
            .into()
        };
    settings_card(
        column![
            subsection_title(
                "Desktop notifications",
                Some("Notify when an agent needs attention or completes work.")
            ),
            switch_row(
                "Enable notifications",
                "Allow Suaegi to send macOS notifications.",
                state.ui_settings().notifications_enabled,
                Some(UiSetting::NotificationsEnabled),
            ),
            switch_row(
                "Agent task complete",
                "Notify when an agent finishes work.",
                state.ui_settings().notification_agent_task_complete,
                Some(UiSetting::NotificationAgentTaskComplete),
            ),
            switch_row(
                "Terminal bell",
                "Notify when a background terminal rings its bell.",
                state.ui_settings().notification_terminal_bell,
                Some(UiSetting::NotificationTerminalBell),
            ),
            switch_row(
                "Suppress while focused",
                "Do not show notifications while Suaegi is the active application.",
                state.ui_settings().notification_suppress_when_focused,
                Some(UiSetting::NotificationSuppressWhenFocused),
            ),
            choice_row(
                "Notification sound",
                &state.ui_settings().notification_sound,
                UiChoice::NotificationSound,
            ),
            choice_row_owned(
                "Sound volume",
                format!("{}%", state.ui_settings().notification_volume),
                UiChoice::NotificationVolume,
            ),
            custom_sound,
        ]
        .spacing(13),
    )
}

fn privacy_content(state: &AppState) -> Element<'_, Message> {
    let telemetry_block = if std::env::var_os("DO_NOT_TRACK").is_some() {
        Some("Telemetry is disabled because DO_NOT_TRACK is set.")
    } else if std::env::var_os("SUAEGI_TELEMETRY_DISABLED").is_some() {
        Some("Telemetry is disabled because SUAEGI_TELEMETRY_DISABLED is set.")
    } else if std::env::var_os("CI").is_some() {
        Some("Telemetry is disabled in CI.")
    } else {
        None
    };
    settings_card(
        column![
            subsection_title(
                "Diagnostics",
                Some("Control anonymous product diagnostics. Terminal and file contents are never included.")
            ),
            switch_row(
                "Anonymous usage telemetry",
                "Share feature usage and crash diagnostics.",
                state.ui_settings().anonymous_telemetry,
                Some(UiSetting::AnonymousTelemetry),
            ),
            text("Secrets remain in the system keychain and are never written to the Suaegi settings file.")
                .size(11)
                .color(theme::MUTED),
            text(telemetry_block.unwrap_or("Telemetry consent follows the switch above."))
                .size(11)
                .color(theme::MUTED),
            rule::horizontal(1),
            subsection_title(
                "Send app diagnostics to support",
                Some("Create a local review file first. No data is uploaded automatically.")
            ),
            button(text("Create and open review file").size(11))
                .on_press(Message::DiagnosticsReviewRequested)
                .padding([5, 8])
                .style(theme::ghost_button),
            text(state.diagnostics_status().unwrap_or(
                "Terminal output, files, prompts, credentials, proxy URLs, and repository paths are excluded."
            ))
            .size(11)
            .color(theme::MUTED),
        ]
        .spacing(13),
    )
}

fn mac_permissions_content<'a>() -> Element<'a, Message> {
    settings_card(
        column![
            subsection_title(
                "Developer permissions",
                Some("Open the matching macOS Privacy & Security pane.")
            ),
            row![
                column![
                    text("Full Disk Access").size(12),
                    text("Required for agents to work across repositories.")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Open Settings").size(11))
                    .on_press(Message::OnboardingOpenFullDiskAccess)
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            permission_row(
                "Accessibility",
                "Allow agents to inspect and operate application controls.",
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            ),
            permission_row(
                "Screen Recording",
                "Allow Computer Use to inspect visible application content.",
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            ),
        ]
        .spacing(13),
    )
}

fn permission_row<'a>(
    title: &'a str,
    description: &'a str,
    settings_url: &'a str,
) -> Element<'a, Message> {
    row![
        column![
            text(title).size(12),
            text(description).size(11).color(theme::MUTED),
        ]
        .spacing(2)
        .width(Length::Fill),
        button(text("Open Settings").size(11))
            .on_press(Message::OpenSystemSettings(settings_url.to_string()))
            .padding([5, 8])
            .style(theme::ghost_button),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn mobile_content(state: &AppState) -> Element<'_, Message> {
    settings_card(
        column![
            subsection_title(
                "Orca Mobile",
                Some("Install the app and pair this Mac from the guided mobile setup.")
            ),
            row![
                column![
                    text("No paired devices").size(12),
                    text("Pairing is end-to-end encrypted.")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Open mobile setup").size(11))
                    .on_press(Message::MobileOpened)
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            switch_row(
                "Show in sidebar",
                "Keep Orca Mobile in the main navigation.",
                state.ui_settings().show_mobile_sidebar,
                Some(UiSetting::ShowMobileSidebar),
            ),
        ]
        .spacing(13),
    )
}

fn provider_accounts_content(state: &AppState) -> Element<'_, Message> {
    let active_runtime = state
        .ui_settings()
        .active_runtime_environment_id
        .as_deref()
        .and_then(|id| {
            state
                .ui_settings()
                .runtime_environments
                .iter()
                .find(|environment| environment.id == id)
        });
    let scope_label = active_runtime
        .map(|environment| format!("Remote server: {}", environment.name))
        .unwrap_or_else(|| "Local desktop".to_string());
    let scope_description = if active_runtime.is_some() {
        "Credentials and account checks are owned by the active remote server."
    } else {
        "Credentials and account checks are owned by this desktop client."
    };
    let remote_account_status: Element<'_, Message> = if active_runtime.is_none() {
        Space::new().height(0).into()
    } else if state.remote_provider_accounts_loading() {
        text("Loading remote provider accounts…")
            .size(11)
            .color(theme::MUTED)
            .into()
    } else if let Some(error) = state.remote_provider_accounts_error() {
        text(error)
            .size(11)
            .color(iced::Color::from_rgb8(0xc0, 0x39, 0x2b))
            .into()
    } else {
        text("Remote account roster and subscription usage are synchronized.")
            .size(11)
            .color(theme::MUTED)
            .into()
    };
    let mut agents = column![].spacing(8);
    for choice in state.agent_picker_choices() {
        let setup_command = active_runtime
            .is_none()
            .then(|| {
                choice.0.map(|id| match id {
                    "codex" => "codex login",
                    "opencode" => "opencode auth login",
                    "gemini" => "gemini",
                    other => other,
                })
            })
            .flatten();
        agents = agents.push(
            row![
                icons::view(Icon::Bot, 12.0, theme::MUTED),
                text(choice.label()).size(12).width(Length::Fill),
                container(
                    text(if active_runtime.is_some() {
                        "Remote-owned"
                    } else {
                        "Detected"
                    })
                    .size(11)
                )
                .padding([2, 6])
                .style(theme::chip),
                button(text("Authenticate").size(11))
                    .on_press_maybe(setup_command.map(|command| {
                        Message::SettingsTerminalCommandRequested(command.to_string())
                    }))
                    .padding([4, 7])
                    .style(theme::ghost_button),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    column![
        settings_card(
            column![
                subsection_title("Runtime scope", Some(scope_description)),
                row![
                    text("Account runtime").size(12).width(Length::Fill),
                    container(text(scope_label).size(11))
                        .padding([4, 7])
                        .style(theme::chip),
                    button(text("Refresh").size(11))
                        .on_press_maybe(
                            (active_runtime.is_some() && !state.remote_provider_accounts_loading())
                                .then_some(Message::RemoteProviderAccountsRefreshRequested)
                        )
                        .padding([4, 7])
                        .style(theme::ghost_button),
                ]
                .align_y(Alignment::Center),
                text("Change the owner in Remote Suaegi Servers > Advanced.")
                    .size(11)
                    .color(theme::MUTED),
                remote_account_status,
            ]
            .spacing(13)
        ),
        provider_managed_accounts(state, crate::managed_accounts::Provider::Claude, "Claude",),
        provider_managed_accounts(state, crate::managed_accounts::Provider::Codex, "Codex"),
        provider_cookie_card(
            state,
            crate::provider_credentials::ProviderSecret::OpenCodeGo,
        ),
        provider_cookie_card(state, crate::provider_credentials::ProviderSecret::MiniMax,),
        settings_card(
            column![
                subsection_title(
                    "Detected provider CLIs",
                    Some("Suaegi uses the same authenticated CLI homes as Orca.")
                ),
                agents,
                switch_row(
                    "Gemini CLI OAuth",
                    "Read the locally authenticated Gemini CLI account for usage limits.",
                    state.ui_settings().gemini_cli_oauth_enabled,
                    Some(UiSetting::GeminiCliOauthEnabled),
                ),
                button(text("Manage tracker accounts in Integrations").size(11))
                    .on_press(Message::SettingsOpened(SettingsSection::Integrations))
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .spacing(13)
        ),
    ]
    .spacing(14)
    .into()
}

fn provider_cookie_card(
    state: &AppState,
    provider: crate::provider_credentials::ProviderSecret,
) -> Element<'_, Message> {
    let configured = state.provider_secret_configured(provider);
    let draft = state.provider_secret_draft(provider);
    let provider_for_input = provider;
    let extra: Element<'_, Message> = match provider {
        crate::provider_credentials::ProviderSecret::OpenCodeGo => text_setting_row(
            "Workspace ID override",
            &state.ui_settings().opencode_workspace_id,
            UiTextSetting::OpenCodeWorkspaceId,
        ),
        crate::provider_credentials::ProviderSecret::MiniMax => column![
            text_setting_row(
                "Group ID override",
                &state.ui_settings().minimax_group_id,
                UiTextSetting::MiniMaxGroupId,
            ),
            text_setting_row(
                "Usage models",
                &state.ui_settings().minimax_usage_models,
                UiTextSetting::MiniMaxUsageModels,
            ),
        ]
        .spacing(8)
        .into(),
    };
    settings_card(
        column![
            subsection_title(
                provider.label(),
                Some("Web session credentials are stored only in the system keychain.")
            ),
            row![
                text_input(
                    if configured {
                        "Replace saved cookie"
                    } else {
                        "Paste session cookie"
                    },
                    draft
                )
                .on_input(move |value| Message::ProviderSecretDraftChanged(
                    provider_for_input,
                    crate::state::SecretDraft::new(value)
                ))
                .secure(true)
                .size(11)
                .padding([6, 8])
                .width(Length::Fill),
                button(text(if configured { "Replace" } else { "Save" }).size(11))
                    .on_press_maybe(
                        (!draft.trim().is_empty())
                            .then_some(Message::ProviderSecretSaveRequested(provider))
                    )
                    .padding([6, 9])
                    .style(theme::selected_button),
                button(text("Clear").size(11))
                    .on_press_maybe(
                        configured.then_some(Message::ProviderSecretClearRequested(provider))
                    )
                    .padding([6, 9])
                    .style(theme::ghost_button),
            ]
            .spacing(6),
            extra,
            text(
                state
                    .provider_secret_status(provider)
                    .unwrap_or(if configured {
                        "Cookie configured."
                    } else {
                        "Cookie not configured."
                    })
            )
            .size(10)
            .color(theme::MUTED),
        ]
        .spacing(10),
    )
}

fn provider_managed_accounts<'a>(
    state: &'a AppState,
    provider: crate::managed_accounts::Provider,
    label: &'a str,
) -> Element<'a, Message> {
    let (accounts, active) = state.provider_accounts(provider);
    let remote_owned = state.ui_settings().active_runtime_environment_id.is_some();
    let importing = state.provider_account_importing(provider);
    let quota_provider = match provider {
        crate::managed_accounts::Provider::Claude => crate::rate_limits::RateLimitProvider::Claude,
        crate::managed_accounts::Provider::Codex => crate::rate_limits::RateLimitProvider::Codex,
    };
    let mut rows = column![row![
        column![
            text("System default").size(12),
            text("Use the provider CLI's normal home and credentials.")
                .size(10)
                .color(theme::MUTED),
        ]
        .spacing(2)
        .width(Length::Fill),
        button(text(if active.is_none() { "Active" } else { "Use" }).size(11))
            .on_press_maybe(
                (!importing && active.is_some())
                    .then_some(Message::ProviderManagedAccountSelected(provider, None))
            )
            .padding([4, 7])
            .style(if active.is_none() {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
    ]
    .align_y(Alignment::Center)]
    .spacing(8);
    for account in accounts {
        let is_active = active == Some(account.id.as_str());
        let confirming_remove = state.provider_account_removal_pending(provider, &account.id);
        let account_actions: Element<'_, Message> = if confirming_remove {
            row![
                button(text("Cancel").size(10))
                    .on_press(Message::ProviderManagedAccountRemoveCancelled)
                    .padding([3, 6])
                    .style(theme::ghost_button),
                button(text("Remove").size(10))
                    .on_press(Message::ProviderManagedAccountRemoveConfirmed)
                    .padding([3, 6])
                    .style(theme::danger_ghost_button),
            ]
            .spacing(4)
            .into()
        } else {
            row![
                button(text(if is_active { "Active" } else { "Use" }).size(11))
                    .on_press_maybe((!importing && !is_active).then(|| {
                        Message::ProviderManagedAccountSelected(provider, Some(account.id.clone()))
                    }))
                    .padding([4, 7])
                    .style(if is_active {
                        theme::selected_button
                    } else {
                        theme::ghost_button
                    }),
                button(text("Re-authenticate").size(10))
                    .on_press_maybe((!remote_owned && !importing).then(|| {
                        Message::ProviderManagedAccountReauthenticateRequested(
                            provider,
                            account.id.clone(),
                        )
                    }))
                    .padding([3, 6])
                    .style(theme::ghost_button),
                button(text("Remove").size(10))
                    .on_press_maybe((!importing).then(|| {
                        Message::ProviderManagedAccountRemoveRequested(provider, account.id.clone())
                    }))
                    .padding([3, 6])
                    .style(theme::ghost_button),
            ]
            .spacing(4)
            .into()
        };
        rows = rows.push(
            row![
                column![
                    text(account.email.clone()).size(12),
                    text(if confirming_remove {
                        "Remove this account and its isolated credentials?"
                    } else if remote_owned {
                        "Managed on the active remote server"
                    } else {
                        "Managed on this device"
                    })
                    .size(10)
                    .color(if confirming_remove {
                        iced::Color::from_rgb8(0xc0, 0x39, 0x2b)
                    } else {
                        theme::MUTED
                    }),
                ]
                .spacing(2)
                .width(Length::Fill),
                account_actions,
            ]
            .align_y(Alignment::Center),
        );
    }
    let mut quota_rows = column![row![
        text("Subscription usage").size(11).width(Length::Fill),
        button(text("Refresh").size(10))
            .on_press_maybe(
                (!state.provider_rate_limits_fetching(quota_provider))
                    .then_some(Message::ProviderRateLimitsRefreshRequested)
            )
            .padding([3, 6])
            .style(theme::ghost_button),
    ]
    .align_y(Alignment::Center)]
    .spacing(5);
    if state.provider_rate_limits_fetching(quota_provider) {
        quota_rows = quota_rows.push(text("Refreshing…").size(10).color(theme::MUTED));
    } else if let Some(limits) = state.provider_rate_limits(quota_provider) {
        for bucket in &limits.buckets {
            let percent = crate::rate_limits::displayed_percentage(
                bucket.used_percent,
                &state.ui_settings().usage_percentage_mode,
            );
            let mode = if state.ui_settings().usage_percentage_mode == "remaining" {
                "remaining"
            } else {
                "used"
            };
            let reset = crate::rate_limits::reset_label(bucket.resets_at_unix_ms)
                .map(|label| format!(" • {label}"))
                .unwrap_or_default();
            quota_rows = quota_rows.push(
                row![
                    text(&bucket.name).size(10).width(Length::Fill),
                    text(format!("{percent}% {mode}{reset}"))
                        .size(10)
                        .color(theme::MUTED),
                ]
                .align_y(Alignment::Center),
            );
        }
        if limits.buckets.is_empty() {
            quota_rows = quota_rows.push(
                text(
                    limits
                        .error
                        .as_deref()
                        .unwrap_or("No subscription usage windows were returned."),
                )
                .size(10)
                .color(theme::MUTED),
            );
        }
    } else {
        quota_rows = quota_rows.push(text("Not refreshed yet.").size(10).color(theme::MUTED));
    }
    settings_card(
        column![
            subsection_title(
                label,
                Some(
                    "Managed accounts use an isolated credential home. New agent sessions use \
                     the active account."
                )
            ),
            rows,
            button(
                text(if importing {
                    "Waiting for sign-in…"
                } else {
                    "Add account"
                })
                .size(11)
            )
            .on_press_maybe(
                (!remote_owned && !importing)
                    .then_some(Message::ProviderManagedAccountAddRequested(provider))
            )
            .padding([5, 8])
            .style(theme::selected_button),
            button(
                text(if importing {
                    "Working…"
                } else {
                    "Import current signed-in account"
                })
                .size(11)
            )
            .on_press_maybe(
                (!remote_owned && !importing)
                    .then_some(Message::ProviderManagedAccountImportRequested(provider))
            )
            .padding([5, 8])
            .style(theme::ghost_button),
            quota_rows,
        ]
        .spacing(11),
    )
}

fn installed_agent_skill(name: &str) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join(".agents/skills").join(name).join("SKILL.md"),
        home.join(".codex/skills").join(name).join("SKILL.md"),
        home.join(".claude/skills").join(name).join("SKILL.md"),
        home.join(".gemini/skills").join(name).join("SKILL.md"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn orchestration_content(state: &AppState) -> Element<'_, Message> {
    let installed = installed_agent_skill("orchestration");
    let command = if installed.is_some() {
        "npx skills update orchestration --global"
    } else {
        "npx skills add https://github.com/stablyai/orca --skill orchestration --global"
    };
    settings_card(
        column![
            subsection_title(
                "Agent Orchestration",
                Some("Coordinate coding agents across handoffs, worktree handovers, and child-agent work.")
            ),
            switch_row(
                "Enable orchestration",
                "Expose orchestration workflows to installed agent skills.",
                state.ui_settings().orchestration_enabled,
                Some(UiSetting::OrchestrationEnabled),
            ),
            row![
                column![
                    text("Orchestration skill").size(12),
                    text(installed.as_ref().map_or(
                        "Not installed".to_string(),
                        |path| format!("Installed at {}", path.display())
                    ))
                    .size(11)
                    .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                container(text(if installed.is_some() { "INSTALLED" } else { "SETUP" }).size(10))
                    .padding([2, 6])
                    .style(theme::chip),
            ],
            button(text(if installed.is_some() {
                "Update orchestration skill"
            } else {
                "Install orchestration skill"
            }).size(11))
                .on_press(Message::SettingsTerminalCommandRequested(command.to_string()))
                .padding([5, 8])
                .style(theme::ghost_button),
            rule::horizontal(1),
            subsection_title(
                "How to use it",
                Some("Ask a coordinator agent to use orchestration for:")
            ),
            text("• Hand off context to another coding agent\n• Move work between worktrees\n• Run child agents sequentially or in parallel")
                .size(11)
                .color(theme::MUTED),
            row![
                text("Detected agent coverage").size(12).width(Length::Fill),
                text(state.agent_picker_choices().len().saturating_sub(1)).size(12),
            ],
        ]
        .spacing(13),
    )
}

fn computer_use_content(state: &AppState) -> Element<'_, Message> {
    let installed = installed_agent_skill("computer-use");
    let command = if installed.is_some() {
        "npx skills update computer-use --global"
    } else {
        "npx skills add https://github.com/stablyai/orca --skill computer-use --global"
    };
    settings_card(
        column![
            subsection_title(
                "Computer Use",
                Some("Allow supported agents to operate desktop applications.")
            ),
            switch_row(
                "Enable Computer Use",
                "Allow the installed Computer Use skill to operate local applications.",
                state.ui_settings().computer_use_enabled,
                Some(UiSetting::ComputerUseEnabled),
            ),
            row![
                column![
                    text("Computer Use skill").size(12),
                    text(installed.as_ref().map_or(
                        "Not installed".to_string(),
                        |path| format!("Installed at {}", path.display())
                    ))
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                container(text(if installed.is_some() { "INSTALLED" } else { "SETUP" }).size(10))
                    .padding([2, 6])
                    .style(theme::chip),
            ]
            .align_y(Alignment::Center),
            button(text(if installed.is_some() {
                "Update Computer Use skill"
            } else {
                "Install Computer Use skill"
            }).size(11))
                .on_press(Message::SettingsTerminalCommandRequested(command.to_string()))
                .padding([5, 8])
                .style(theme::ghost_button),
            rule::horizontal(1),
            row![
                column![
                    text("macOS permissions").size(12),
                    text("Accessibility and Screen Recording are required before agents can operate app windows.")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Review permissions").size(11))
                    .on_press(Message::SettingsOpened(SettingsSection::MacPermissions))
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(13),
    )
}

fn voice_content(state: &AppState) -> Element<'_, Message> {
    let enabled = state.ui_settings().voice_enabled;
    let enable_indicator = container(text(if enabled { "●" } else { "○" }).size(13))
        .padding([2, 7])
        .style(if enabled {
            theme::active_card
        } else {
            theme::chip
        });
    let dictation_label = match state.voice_dictation_state() {
        VoiceDictationState::Idle => "Start dictation",
        VoiceDictationState::Recording => "Stop dictation",
        VoiceDictationState::Transcribing => "Transcribing…",
    };
    let model_value = voice_model_label(&state.ui_settings().voice_model);
    let cloud_selected = state.ui_settings().voice_model.starts_with("openai-");
    let local_manifest = crate::speech_models::local_model(&state.ui_settings().voice_model);
    let local_ready = crate::speech_models::is_ready(
        &state.ui_settings().voice_model,
        &state.ui_settings().voice_models_dir,
    );
    let model_action: Element<'_, Message> = if let Some(manifest) = local_manifest {
        let model_id = manifest.id.to_string();
        let busy = state.voice_model_busy() == Some(manifest.id);
        row![
            text(manifest.description)
                .size(11)
                .color(theme::MUTED)
                .width(Length::Fill),
            if busy {
                button(text("Working…").size(11))
                    .padding([5, 8])
                    .style(theme::ghost_button)
            } else if local_ready {
                button(text("Delete").size(11))
                    .on_press(Message::VoiceModelDeleteRequested(model_id))
                    .padding([5, 8])
                    .style(theme::danger_ghost_button)
            } else {
                button(text(format!("Download · {} MB", manifest.size_bytes / 1_000_000)).size(11))
                    .on_press(Message::VoiceModelDownloadRequested(model_id))
                    .padding([5, 8])
                    .style(theme::ghost_button)
            },
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        text(if cloud_selected {
            "Cloud transcription requires an OpenAI API key and uploads recorded audio."
        } else {
            "Select a speech model. Local models run offline; cloud models require an API key."
        })
        .size(11)
        .color(theme::MUTED)
        .into()
    };
    let key_status = if state.ui_settings().voice_openai_api_key_configured {
        "Configured securely in macOS Keychain"
    } else {
        "Required for GPT-4o transcription models"
    };

    settings_card(
        column![
            row![
                column![
                    text("Enable Voice Dictation").size(12),
                    text("Press ⌘E to dictate text into any focused pane.")
                        .size(11)
                        .color(theme::MUTED)
                ]
                .spacing(2)
                .width(Length::Fill),
                button(enable_indicator)
                    .on_press(Message::VoiceEnabledToggled)
                    .padding(0)
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            rule::horizontal(1),
            choice_row(
                "Dictation Mode",
                &state.ui_settings().voice_dictation_mode,
                UiChoice::VoiceDictationMode,
            ),
            choice_row(
                "Recognition Language",
                &state.ui_settings().voice_language,
                UiChoice::VoiceLanguage,
            ),
            text("Toggle: press ⌘E once to start, again to stop. Hold: dictate while ⌘E is held.")
                .size(11)
                .color(theme::MUTED),
            rule::horizontal(1),
            choice_row_owned("Speech Model", model_value, UiChoice::VoiceModel,),
            model_action,
            rule::horizontal(1),
            switch_row(
                "Confirm before inserting in terminals",
                "Review transcribed text before it is pasted into the focused terminal.",
                state.ui_settings().voice_terminal_confirm_before_insert,
                Some(UiSetting::VoiceConfirmBeforeInsert),
            ),
            rule::horizontal(1),
            column![
                row![
                    column![
                        text("OpenAI Transcription").size(12),
                        text(key_status).size(11).color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    if state.ui_settings().voice_openai_api_key_configured {
                        button(text("Clear").size(11))
                            .on_press(Message::VoiceApiKeyClearRequested)
                            .padding([5, 8])
                            .style(theme::ghost_button)
                    } else {
                        button(text("Save key").size(11))
                            .on_press(Message::VoiceApiKeySaveRequested)
                            .padding([5, 8])
                            .style(theme::ghost_button)
                    },
                ]
                .align_y(Alignment::Center),
                text_input("sk-…", state.voice_api_key_draft())
                    .on_input(|value| Message::VoiceApiKeyDraftChanged(SecretDraft::new(value)))
                    .secure(true)
                    .size(12)
                    .padding([6, 8]),
            ]
            .spacing(7),
            rule::horizontal(1),
            row![
                state
                    .voice_status()
                    .map(|status| text(status).size(11).color(theme::MUTED))
                    .unwrap_or_else(|| {
                        text("Ready when a speech model is selected.")
                            .size(11)
                            .color(theme::MUTED)
                    })
                    .width(Length::Fill),
                button(text(dictation_label).size(11))
                    .on_press_maybe(
                        (enabled
                            && state.voice_dictation_state() != VoiceDictationState::Transcribing)
                            .then_some(Message::VoiceDictationToggled),
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(13),
    )
}

fn voice_model_label(model_id: &str) -> String {
    match model_id {
        "parakeet-tdt-0.6b-v3-int8" => "Parakeet TDT v3",
        "parakeet-tdt-0.6b-v2-int8" => "Parakeet TDT v2",
        "zipformer-bilingual-zh-en" => "Zipformer Bilingual",
        "paraformer-bilingual-zh-en" => "Paraformer Bilingual",
        "zipformer-streaming-en-20m" => "Zipformer Streaming EN",
        "zipformer-streaming-zh-14m" => "Zipformer Streaming ZH",
        "whisper-tiny" => "Whisper Tiny",
        "openai-gpt-4o-mini-transcribe" => "GPT-4o mini Transcribe",
        "openai-gpt-4o-transcribe" => "GPT-4o Transcribe",
        _ => "Select Model",
    }
    .to_string()
}

fn quick_commands_content(state: &AppState) -> Element<'_, Message> {
    let mut commands = column![].spacing(10);
    for (index, command) in state.ui_settings().quick_commands.iter().enumerate() {
        commands = commands.push(
            container(
                column![
                    row![
                        text_input("Label", &command.label)
                            .on_input(move |value| Message::QuickCommandLabelChanged(index, value))
                            .size(12)
                            .padding([5, 7])
                            .width(Length::Fixed(150.0)),
                        text_input("Command", &command.command)
                            .on_input(move |value| Message::QuickCommandBodyChanged(index, value))
                            .size(12)
                            .padding([5, 7])
                            .width(Length::Fill),
                        button(text("Run").size(11))
                            .on_press(Message::QuickCommandRun(index))
                            .padding([5, 8])
                            .style(theme::ghost_button),
                        button(text("Delete").size(11))
                            .on_press(Message::QuickCommandRemoved(index))
                            .padding([5, 8])
                            .style(theme::danger_ghost_button),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    row![
                        text("Press Enter after command")
                            .size(11)
                            .width(Length::Fill),
                        button(
                            container(text(if command.append_enter { "●" } else { "○" }).size(12))
                                .padding([2, 7])
                                .style(if command.append_enter {
                                    theme::active_card
                                } else {
                                    theme::chip
                                })
                        )
                        .on_press(Message::QuickCommandAppendEnterToggled(index))
                        .padding(0)
                        .style(theme::ghost_button),
                    ]
                    .align_y(Alignment::Center),
                ]
                .spacing(7),
            )
            .padding([8, 10])
            .style(theme::context_panel),
        );
    }
    settings_card(
        column![
            row![
                column![
                    text("Saved Commands").size(13),
                    text("Global commands run in the focused workspace terminal.")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("+ Add Command").size(11))
                    .on_press(Message::QuickCommandAdded)
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
            commands,
        ]
        .spacing(13),
    )
}

fn browser_content(state: &AppState) -> Element<'_, Message> {
    let active_profile = &state.ui_settings().browser_default_profile_id;
    let mut detected_imports = column![].spacing(5);
    for profile in crate::browser::detected_browser_profiles() {
        let label = profile.label().to_string();
        detected_imports = detected_imports.push(
            row![
                text(label).size(11).width(Length::Fill),
                button(text("Import").size(11))
                    .on_press_maybe(
                        (!state.browser_cookie_importing())
                            .then_some(Message::BrowserDetectedCookiesImportRequested(profile))
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
            ]
            .align_y(Alignment::Center),
        );
    }
    let profile_row = |id: String, label: String, removable: bool| {
        let active = active_profile == &id;
        let mut actions: Vec<Element<'_, Message>> = vec![button(
            row![
                text(if active { "●" } else { "○" })
                    .size(11)
                    .color(theme::MUTED),
                text(label).size(12),
                Space::new().width(Length::Fill),
                text(if active { "Active" } else { "Use" })
                    .size(11)
                    .color(theme::MUTED),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        )
        .on_press(Message::BrowserProfileSelected(id.clone()))
        .padding([7, 9])
        .width(Length::Fill)
        .style(theme::ghost_button)
        .into()];
        if removable {
            actions.push(
                button(text("Remove").size(11))
                    .on_press(Message::BrowserProfileRemoved(id))
                    .padding([6, 8])
                    .style(theme::ghost_button)
                    .into(),
            );
        } else {
            actions.push(
                button(text("Clear Data").size(11))
                    .on_press(Message::BrowserCookiesClearRequested)
                    .padding([6, 8])
                    .style(theme::ghost_button)
                    .into(),
            );
        }
        row(actions).spacing(5).align_y(Alignment::Center).into()
    };
    let mut profiles = vec![profile_row("default".into(), "Default".into(), false)];
    profiles.extend(
        state
            .ui_settings()
            .browser_profiles
            .iter()
            .map(|profile| profile_row(profile.id.clone(), profile.label.clone(), true)),
    );

    column![
        settings_card(
            column![
                subsection_title("Navigation", None),
                text_setting_row(
                    "Home page",
                    &state.ui_settings().browser_home_page,
                    UiTextSetting::BrowserHomePage,
                ),
                choice_row(
                    "Search engine",
                    &state.ui_settings().browser_search_engine,
                    UiChoice::BrowserSearchEngine,
                ),
                choice_row_owned(
                    "Default zoom",
                    format!("{}%", state.ui_settings().browser_default_zoom_percent),
                    UiChoice::BrowserDefaultZoom,
                ),
                switch_row(
                    "Open links in Suaegi",
                    "Route web links to a workspace-scoped in-app browser.",
                    state.ui_settings().open_links_in_app,
                    Some(UiSetting::OpenLinksInApp),
                ),
                switch_row(
                    "Localhost workspace labels",
                    "Use worktree-scoped localhost names to distinguish development apps.",
                    state.ui_settings().localhost_worktree_labels,
                    Some(UiSetting::LocalhostWorktreeLabels),
                ),
                button(text("Open Suaegi Browser").size(11))
                    .on_press(Message::BrowserOpenRequested)
                    .padding([6, 9])
                    .style(theme::ghost_button),
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "Session & Cookies",
                    Some("Choose the default session for new tabs and keep logins isolated.")
                ),
                column(profiles).spacing(5),
                row![
                    button(
                        text(if state.browser_cookie_importing() {
                            "Importing…"
                        } else {
                            "Import cookies.txt"
                        })
                        .size(11)
                    )
                    .on_press_maybe(
                        (!state.browser_cookie_importing())
                            .then_some(Message::BrowserCookiesImportRequested)
                    )
                    .padding([6, 8])
                    .style(theme::ghost_button),
                    text("Netscape cookie format · imported into the active profile")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(7)
                .align_y(Alignment::Center),
                detected_imports,
                if let Some(status) = state.browser_cookie_status() {
                    text(status).size(11).color(theme::MUTED)
                } else {
                    text("").size(11)
                },
                row![
                    text_input("Profile name", state.browser_profile_name_draft())
                        .on_input(Message::BrowserProfileNameChanged)
                        .on_submit(Message::BrowserProfileAdded)
                        .padding([6, 8])
                        .size(11),
                    button(text("+ Add Profile").size(11))
                        .on_press(Message::BrowserProfileAdded)
                        .padding([7, 8])
                        .style(theme::ghost_button),
                ]
                .spacing(6),
                text("Credentials are never stored in the Suaegi settings file.")
                    .size(11)
                    .color(theme::MUTED),
            ]
            .spacing(8)
        ),
    ]
    .spacing(14)
    .into()
}

fn mobile_emulator_content(state: &AppState) -> Element<'_, Message> {
    let enabled = state.ui_settings().mobile_emulator_enabled;
    let refreshing = state.emulator_refreshing();
    let availability = state.emulator_availability();
    let badge = if !enabled {
        "Disabled"
    } else if refreshing || availability.is_none() {
        "Checking…"
    } else if availability.is_some_and(|value| value.available) {
        "Ready"
    } else {
        "Needs setup"
    };
    let mut mappings = vec![("Auto-select device".to_string(), None)];
    if let Some(availability) = availability {
        mappings.extend(availability.devices.iter().map(|device| {
            (
                format!(
                    "{} · {}",
                    device.label(),
                    device.id.chars().take(8).collect::<String>()
                ),
                Some(device.id.clone()),
            )
        }));
    }
    let selected_label = state
        .ui_settings()
        .mobile_emulator_default_device_udid
        .as_ref()
        .and_then(|selected| {
            mappings
                .iter()
                .find(|(_, id)| id.as_ref() == Some(selected))
                .map(|(label, _)| label.clone())
        })
        .unwrap_or_else(|| "Auto-select device".to_string());
    let labels = mappings
        .iter()
        .map(|(label, _)| label.clone())
        .collect::<Vec<_>>();
    let selection_map = mappings.clone();
    let device_picker = pick_list(labels, Some(selected_label), move |label| {
        let id = selection_map
            .iter()
            .find(|(candidate, _)| *candidate == label)
            .and_then(|(_, id)| id.clone());
        Message::EmulatorDefaultDeviceSelected(id)
    })
    .width(Length::Fixed(245.0))
    .text_size(11);

    let mut toolchains = column![].spacing(7);
    if let Some(availability) = availability {
        toolchains = toolchains.push(
            row![
                text(if availability.android.sdk_found {
                    "✓ Android SDK"
                } else {
                    "○ Android SDK"
                })
                .size(11)
                .width(Length::Fixed(120.0)),
                text(
                    availability
                        .android
                        .sdk_path
                        .as_deref()
                        .unwrap_or(&availability.android.message)
                )
                .size(11)
                .color(theme::MUTED)
                .width(Length::Fill),
            ]
            .align_y(Alignment::Center),
        );
        if cfg!(target_os = "macos") {
            toolchains = toolchains.push(
                row![
                    text(if availability.simctl.ok && availability.serve_sim.ok {
                        "✓ iOS Simulator"
                    } else {
                        "○ iOS Simulator"
                    })
                    .size(11)
                    .width(Length::Fixed(120.0)),
                    text(
                        availability
                            .simctl
                            .message
                            .as_deref()
                            .or(availability.serve_sim.message.as_deref())
                            .unwrap_or("Ready")
                    )
                    .size(11)
                    .color(theme::MUTED)
                    .width(Length::Fill),
                ]
                .align_y(Alignment::Center),
            );
        }
    }

    column![
        settings_card(
            column![
                subsection_title(
                    "Mobile Emulator",
                    Some("Configure mobile emulator support for Suaegi and coding agents.")
                ),
                switch_row(
                    "Enable Mobile Emulator",
                    "Shows the New Mobile Emulator action and allows agents to attach to the active emulator.",
                    enabled,
                    Some(UiSetting::MobileEmulatorEnabled),
                ),
                rule::horizontal(1),
                row![
                    column![
                        text("Availability").size(12),
                        text(
                            state
                                .emulator_status()
                                .unwrap_or("Checking Android SDK and iOS Simulator support.")
                        )
                        .size(11)
                        .color(theme::MUTED)
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    container(text(badge).size(11))
                        .padding([2, 7])
                        .style(if availability.is_some_and(|value| value.available) {
                            theme::active_card
                        } else {
                            theme::chip
                        }),
                    button(text("↻").size(13))
                        .on_press_maybe((!refreshing).then_some(Message::EmulatorAvailabilityRequested))
                        .padding([3, 7])
                        .style(theme::ghost_button),
                ]
                .spacing(7)
                .align_y(Alignment::Center),
                toolchains,
                row![
                    text_input(
                        "Auto-discover Android SDK",
                        &state.ui_settings().android_sdk_path
                    )
                    .on_input(|value| Message::UiTextSettingChanged(
                        UiTextSetting::AndroidSdkPath,
                        value
                    ))
                    .size(11)
                    .padding([6, 8])
                    .width(Length::Fill),
                    button(text("Locate SDK folder").size(11))
                        .on_press(Message::AndroidSdkBrowseRequested)
                        .padding([6, 8])
                        .style(theme::ghost_button),
                ]
                .spacing(6),
                rule::horizontal(1),
                row![
                    column![
                        text("Default Device").size(12),
                        text("Auto-select prefers an already running device.")
                            .size(11)
                            .color(theme::MUTED)
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    device_picker,
                ]
                .align_y(Alignment::Center),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Launch Default Device").size(11))
                        .on_press_maybe(
                            (enabled
                                && availability.is_some_and(|value| !value.devices.is_empty()))
                            .then_some(Message::EmulatorLaunchDefaultRequested),
                        )
                        .padding([6, 9])
                        .style(theme::ghost_button),
                ],
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "Agent Mobile Emulator Control",
                    Some("Let coding agents control the active mobile emulator with Suaegi CLI commands.")
                ),
                text("1  Enable the Suaegi CLI so emulator commands run from agent shells.")
                    .size(11),
                text("2  Install or update the Orca CLI skill for your coding agents.")
                    .size(11),
                button(text("Install Orca CLI skill").size(11))
                    .on_press(Message::SettingsTerminalCommandRequested(
                        "npx skills add https://github.com/stablyai/orca --skill orca-cli --global"
                            .to_string()
                    ))
                    .padding([6, 9])
                    .style(theme::ghost_button),
                text("suaegi emulator list --json   ·   suaegi emulator attach \"iPhone 16 Pro\" --json")
                    .size(11)
                    .color(theme::MUTED),
            ]
            .spacing(10)
        ),
    ]
    .spacing(14)
    .into()
}

fn floating_workspace_content(state: &AppState) -> Element<'_, Message> {
    settings_card(
        column![
            subsection_title(
                "Floating Workspace",
                Some("Global terminal, browser, and Markdown tabs outside a repository.")
            ),
            switch_row(
                "Enable Floating Workspace",
                "Show the floating workspace button and panel.",
                state.ui_settings().floating_workspace_enabled,
                Some(UiSetting::FloatingWorkspaceEnabled),
            ),
            row![
                text_input("~", &state.ui_settings().floating_workspace_cwd)
                    .on_input(|value| Message::UiTextSettingChanged(
                        UiTextSetting::FloatingWorkspaceCwd,
                        value
                    ))
                    .size(12)
                    .padding([6, 8])
                    .width(Length::Fill),
                button(text("Browse").size(11))
                    .on_press(Message::FloatingWorkspaceBrowseRequested)
                    .padding([6, 9])
                    .style(theme::ghost_button),
            ]
            .spacing(6),
            choice_row(
                "Toggle button location",
                &state.ui_settings().floating_workspace_trigger,
                UiChoice::FloatingWorkspaceTrigger,
            ),
        ]
        .spacing(13),
    )
}

fn input_content(state: &AppState) -> Element<'_, Message> {
    settings_card(
        column![
            subsection_title("Selection & editing", None),
            switch_row(
                "Middle-click Paste from Selection",
                "Use the primary selection buffer for terminal-style middle-click paste.",
                state.ui_settings().primary_selection_middle_click_paste,
                Some(UiSetting::PrimarySelectionMiddleClickPaste),
            ),
            switch_row(
                "Diff word wrap",
                "Wrap long lines in diff viewers.",
                state.ui_settings().diff_word_wrap,
                Some(UiSetting::DiffWordWrap),
            ),
            switch_row(
                "Editor word wrap",
                "Wrap long lines in the file editor.",
                state.ui_settings().editor_word_wrap,
                Some(UiSetting::EditorWordWrap),
            ),
        ]
        .spacing(13),
    )
}

fn ssh_hosts_content(state: &AppState) -> Element<'_, Message> {
    let mut managed = column![].spacing(10);
    if state.ui_settings().ssh_hosts.is_empty() {
        managed = managed.push(
            container(
                column![
                    text("No SSH targets yet").size(12),
                    text("Import ~/.ssh/config or add an existing machine manually.")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(3),
            )
            .padding([12, 14])
            .width(Length::Fill)
            .style(theme::context_panel),
        );
    }
    for (index, host) in state.ui_settings().ssh_hosts.iter().enumerate() {
        let endpoint = format!(
            "{}{}:{}",
            if host.user.is_empty() {
                String::new()
            } else {
                format!("{}@", host.user)
            },
            if host.hostname.is_empty() {
                host.config_host.as_str()
            } else {
                host.hostname.as_str()
            },
            host.port
        );
        let test_label = if state.ssh_testing(&host.id) {
            "Testing…"
        } else {
            "Test"
        };
        let persistence = if host.relay_grace_period_seconds == 0 {
            "terminals until reset".to_string()
        } else {
            format!("terminal timeout: {}s", host.relay_grace_period_seconds)
        };
        let persistence_control: Element<'_, Message> = if host.relay_grace_period_seconds == 0 {
            text("Use End Remote Terminals or Reset Relay to stop them.")
                .size(11)
                .color(theme::MUTED)
                .into()
        } else {
            row![
                text("Timeout after disconnect (seconds)")
                    .size(11)
                    .color(theme::MUTED)
                    .width(Length::Fill),
                text_input("86400", &host.relay_grace_period_seconds.to_string())
                    .on_input(move |value| {
                        Message::SshHostRelayGracePeriodChanged(index, value)
                    })
                    .size(11)
                    .padding([6, 8])
                    .width(Length::Fixed(140.0)),
            ]
            .align_y(Alignment::Center)
            .into()
        };
        let status_line: Element<'_, Message> = if let Some(status) = state.ssh_status(&host.id) {
            text(status).size(11).color(theme::MUTED).into()
        } else {
            Space::new().height(0).into()
        };
        managed = managed.push(
            container(
                column![
                    row![
                        icons::view(Icon::PanelRight, 14.0, theme::MUTED),
                        column![
                            text(if host.label.is_empty() {
                                "New SSH Target"
                            } else {
                                &host.label
                            })
                            .size(12),
                            text(format!("{endpoint} • {persistence}"))
                                .size(11)
                                .color(theme::MUTED),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        container(
                            text(if host.source == "ssh-config" {
                                "SSH CONFIG"
                            } else {
                                "MANUAL"
                            })
                            .size(10)
                        )
                        .padding([2, 6])
                        .style(theme::chip),
                        button(text(test_label).size(11))
                            .on_press_maybe(
                                (!state.ssh_testing(&host.id))
                                    .then_some(Message::SshHostTestRequested(index))
                            )
                            .padding([5, 7])
                            .style(theme::ghost_button),
                        button(text("Connect").size(11))
                            .on_press(Message::SshHostConnect(index))
                            .padding([5, 7])
                            .style(theme::ghost_button),
                        button(text("Delete").size(11))
                            .on_press(Message::SshHostRemoved(index))
                            .padding([5, 7])
                            .style(theme::danger_ghost_button),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    row![
                        text_input("Label", &host.label)
                            .on_input(move |value| Message::SshHostLabelChanged(index, value))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::FillPortion(2)),
                        text_input("Host or SSH config alias *", &host.hostname)
                            .on_input(move |value| Message::SshHostHostnameChanged(index, value))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::FillPortion(3)),
                    ]
                    .spacing(6),
                    row![
                        text_input("User", &host.user)
                            .on_input(move |value| Message::SshHostUserChanged(index, value))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::FillPortion(2)),
                        text_input("Port", &host.port.to_string())
                            .on_input(move |value| Message::SshHostPortChanged(index, value))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::FillPortion(1)),
                        text_input("Identity file (optional)", &host.identity_file)
                            .on_input(move |value| Message::SshHostIdentityFileChanged(
                                index, value
                            ))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::FillPortion(4)),
                    ]
                    .spacing(6),
                    text("Advanced Connection").size(11).color(theme::MUTED),
                    row![
                        text_input("Proxy Command", &host.proxy_command)
                            .on_input(move |value| Message::SshHostProxyCommandChanged(
                                index, value
                            ))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::Fill),
                        text_input("Jump Host", &host.jump_host)
                            .on_input(move |value| Message::SshHostJumpHostChanged(index, value))
                            .size(11)
                            .padding([6, 8])
                            .width(Length::Fill),
                    ]
                    .spacing(6),
                    action_switch_row(
                        "Reuse SSH connection for faster setup",
                        "Uses OpenSSH multiplexing when available.",
                        host.system_ssh_connection_reuse,
                        Message::SshHostConnectionReuseToggled(index),
                    ),
                    text("Remote Terminal Persistence")
                        .size(11)
                        .color(theme::MUTED),
                    action_switch_row(
                        "Keep terminals alive until reset",
                        "Remote terminals keep running after Suaegi disconnects.",
                        host.relay_grace_period_seconds == 0,
                        Message::SshHostRelayKeepAliveToggled(index),
                    ),
                    persistence_control,
                    status_line,
                ]
                .spacing(9),
            )
            .padding([12, 14])
            .style(theme::context_panel),
        );
    }
    let import_status: Element<'_, Message> = if let Some(status) = state.ssh_status("__import__") {
        text(status).size(11).color(theme::MUTED).into()
    } else {
        Space::new().height(0).into()
    };
    column![
        settings_card(
            column![
                row![
                    column![
                        text("SSH hosts").size(13),
                        text("Add an existing machine over SSH so projects and workspaces can run there.")
                            .size(11)
                            .color(theme::MUTED)
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(
                        text(if state.ssh_importing() {
                            "Importing…"
                        } else {
                            "Import"
                        })
                        .size(11)
                    )
                        .on_press_maybe(
                            (!state.ssh_importing()).then_some(Message::SshConfigImportRequested)
                        )
                        .padding([5, 8])
                        .style(theme::ghost_button),
                    button(text("+ Add Target").size(11))
                        .on_press(Message::SshHostAdded)
                        .padding([5, 8])
                        .style(theme::ghost_button),
                ],
                import_status,
                managed,
            ]
            .spacing(13)
        ),
        settings_card(
            column![
                subsection_title(
                    "Authentication & compatibility",
                    Some("Suaegi delegates authentication and host-key policy to system OpenSSH.")
                ),
                text("ssh-agent, macOS Keychain, ~/.ssh/config Include rules, ProxyCommand, and ProxyJump are resolved by /usr/bin/ssh.")
                    .size(11)
                    .color(theme::MUTED),
            ]
            .spacing(13)
        )
    ]
    .spacing(14)
    .into()
}

fn remote_servers_content(state: &AppState) -> Element<'_, Message> {
    let mut servers = column![].spacing(8);
    if state.ui_settings().runtime_environments.is_empty() {
        servers = servers.push(
            container(
                column![
                    text("No remote servers saved").size(12),
                    text("Generate a pairing URL on a headless Suaegi/Orca server, then paste it below.")
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(3),
            )
            .padding([12, 14])
            .width(Length::Fill)
            .style(theme::context_panel),
        );
    } else {
        for (index, environment) in state.ui_settings().runtime_environments.iter().enumerate() {
            let active = state.ui_settings().active_runtime_environment_id.as_deref()
                == Some(environment.id.as_str());
            let check_label = if state.remote_runtime_checking(&environment.id) {
                "Checking…"
            } else {
                "Connect"
            };
            let status: Element<'_, Message> =
                if let Some(status) = state.remote_runtime_status(&environment.id) {
                    text(status).size(11).color(theme::MUTED).into()
                } else {
                    Space::new().height(0).into()
                };
            let update = state.remote_server_update(&environment.id);
            let update_busy = update.is_some_and(|update| {
                matches!(
                    update.phase,
                    crate::remote_runtime::RemoteUpdatePhase::Checking
                        | crate::remote_runtime::RemoteUpdatePhase::Updating
                )
            });
            let update_available = update.is_some_and(|update| {
                update.phase == crate::remote_runtime::RemoteUpdatePhase::Available
            });
            let update_status: Element<'_, Message> = update.map_or_else(
                || Space::new().height(0).into(),
                |update| text(&update.message).size(11).color(theme::MUTED).into(),
            );
            servers = servers.push(
                container(
                    column![
                        row![
                            icons::view(Icon::PanelRight, 14.0, theme::MUTED),
                            column![
                                row![
                                    text(&environment.name).size(12),
                                    container(
                                        text(if active { "ACTIVE" } else { "SAVED" }).size(10)
                                    )
                                    .padding([2, 6])
                                    .style(if active {
                                        theme::active_card
                                    } else {
                                        theme::chip
                                    }),
                                ]
                                .spacing(6)
                                .align_y(Alignment::Center),
                                text(&environment.endpoint).size(11).color(theme::MUTED),
                            ]
                            .spacing(2)
                            .width(Length::Fill),
                            button(text(check_label).size(11))
                                .on_press_maybe(
                                    (!state.remote_runtime_checking(&environment.id))
                                        .then_some(Message::RemoteRuntimeCheckRequested(index))
                                )
                                .padding([5, 8])
                                .style(theme::ghost_button),
                            button(
                                text(if update_busy {
                                    "Updating…"
                                } else if update_available {
                                    "Update"
                                } else {
                                    "Updates"
                                })
                                .size(11)
                            )
                            .on_press_maybe((!update_busy).then_some(if update_available {
                                Message::RemoteServerUpdateRequested(index)
                            } else {
                                Message::RemoteServerUpdateCheckRequested(index)
                            }))
                            .padding([5, 8])
                            .style(theme::ghost_button),
                            button(text("Remove").size(11))
                                .on_press(Message::RemoteRuntimeRemoveRequested(index))
                                .padding([5, 8])
                                .style(theme::danger_ghost_button),
                        ]
                        .spacing(7)
                        .align_y(Alignment::Center),
                        status,
                        update_status,
                    ]
                    .spacing(6),
                )
                .padding([11, 13])
                .width(Length::Fill)
                .style(theme::context_panel),
            );
        }
    }
    let mut active_options = vec!["Local desktop".to_string()];
    active_options.extend(
        state
            .ui_settings()
            .runtime_environments
            .iter()
            .map(|environment| environment.name.clone()),
    );
    let active_label = state
        .ui_settings()
        .active_runtime_environment_id
        .as_ref()
        .and_then(|active| {
            state
                .ui_settings()
                .runtime_environments
                .iter()
                .find(|environment| &environment.id == active)
        })
        .map(|environment| environment.name.clone())
        .unwrap_or_else(|| "Local desktop".to_string());
    let form_status: Element<'_, Message> =
        if let Some(status) = state.remote_runtime_status("__form__") {
            text(status).size(11).color(theme::MUTED).into()
        } else {
            Space::new().height(0).into()
        };
    column![
        settings_card(
            column![
                subsection_title(
                    "Connect to remote servers",
                    Some("Pair another Suaegi/Orca runtime, then check or select it here.")
                ),
                row![
                    text_input("Server name", state.remote_runtime_name_draft())
                        .on_input(Message::RemoteRuntimeNameChanged)
                        .size(11)
                        .padding([6, 8])
                        .width(Length::FillPortion(2)),
                    text_input("Pairing code or orca://pair URL", state.remote_runtime_pairing_draft())
                        .on_input(|value| {
                            Message::RemoteRuntimePairingCodeChanged(SecretDraft::new(value))
                        })
                        .secure(true)
                        .size(11)
                        .padding([6, 8])
                        .width(Length::FillPortion(3)),
                    button(
                        text(if state.remote_runtime_saving() {
                            "Saving…"
                        } else {
                            "Add Server"
                        })
                        .size(11)
                    )
                    .on_press_maybe(
                        (!state.remote_runtime_saving())
                            .then_some(Message::RemoteRuntimeSaveRequested)
                    )
                    .padding([6, 8])
                    .style(theme::ghost_button),
                ]
                .spacing(6),
                form_status,
                text("Pairing tokens and public keys are stored only in the macOS Keychain.")
                    .size(11)
                    .color(theme::MUTED),
                servers,
            ]
            .spacing(12)
        ),
        settings_card(
            column![
                subsection_title(
                    "Advanced",
                    Some("Choose the default Host for supported projects, files, terminals, and provider checks.")
                ),
                row![
                    text("Active Server").size(12).width(Length::Fill),
                    pick_list(active_options, Some(active_label), |selected| {
                        if selected == "Local desktop" {
                            Message::RemoteRuntimeActiveSelected(None)
                        } else {
                            Message::RemoteRuntimeActiveSelected(
                                state
                                    .ui_settings()
                                    .runtime_environments
                                    .iter()
                                    .find(|environment| environment.name == selected)
                                    .map(|environment| environment.id.clone()),
                            )
                        }
                    })
                    .text_size(11)
                    .width(Length::Fixed(220.0)),
                ]
                .align_y(Alignment::Center),
                text("Selecting a saved server persists the default Host. Reachability checks do not change it.")
                    .size(11)
                    .color(theme::MUTED),
            ]
            .spacing(12)
        ),
    ]
    .spacing(14)
    .into()
}

fn advanced_content(state: &AppState) -> Element<'_, Message> {
    let mut sessions = column![].spacing(4);
    if state.daemon_sessions().is_empty() {
        sessions = sessions.push(
            text(if state.daemon_sessions_loading() {
                "Loading…"
            } else {
                "No sessions."
            })
            .size(11)
            .color(theme::MUTED),
        );
    } else {
        for session in state.daemon_sessions() {
            let kill_id = session.session_id.clone();
            sessions = sessions.push(
                row![
                    text(if session.running { "●" } else { "○" })
                        .size(10)
                        .color(if session.running {
                            iced::Color::from_rgb8(0x22, 0xa0, 0x59)
                        } else {
                            theme::MUTED
                        }),
                    column![
                        text(session.session_id.clone()).size(10),
                        text(format!(
                            "{} × {} · sequence {}{}",
                            session.cols,
                            session.rows,
                            session.next_sequence,
                            session
                                .exit_code
                                .map(|code| format!(" · exited {code}"))
                                .unwrap_or_default()
                        ))
                        .size(9)
                        .color(theme::MUTED),
                    ]
                    .spacing(1)
                    .width(Length::Fill),
                    button(text("Kill").size(9))
                        .on_press_maybe(
                            (!state.daemon_sessions_loading())
                                .then(|| Message::DaemonSessionKillRequested(kill_id))
                        )
                        .padding([3, 6])
                        .style(theme::ghost_button),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
        }
    }
    let confirmation: Element<'_, Message> = state.daemon_action_confirm().map_or_else(
        || Space::new().height(0).into(),
        |action| {
            let description = if action == "*" {
                "Kill every terminal session? Running terminal processes will stop.".to_string()
            } else if action == "!restart" {
                "Restart the terminal daemon? All running terminal sessions will stop.".to_string()
            } else {
                format!("Kill terminal session {action}? Its running process will stop.")
            };
            container(
                column![
                    text(description).size(11),
                    row![
                        button(text("Cancel").size(10))
                            .on_press(Message::DaemonActionCancelled)
                            .padding([4, 7])
                            .style(theme::ghost_button),
                        button(text("Confirm").size(10))
                            .on_press(Message::DaemonActionConfirmed)
                            .padding([4, 7])
                            .style(theme::selected_button),
                    ]
                    .spacing(5),
                ]
                .spacing(6),
            )
            .padding(8)
            .style(theme::active_card)
            .into()
        },
    );
    column![
        settings_card(
            column![
                subsection_title(
                    "Network",
                    Some("Optional proxy configuration for HTTP clients and spawned terminals.")
                ),
                text_setting_row(
                    "HTTP proxy URL",
                    &state.ui_settings().http_proxy_url,
                    UiTextSetting::HttpProxyUrl,
                ),
                text_setting_row(
                    "Proxy bypass rules",
                    &state.ui_settings().http_proxy_bypass_rules,
                    UiTextSetting::HttpProxyBypassRules,
                ),
                switch_row(
                    "HTTP/1.1 compatibility mode",
                    "Disable HTTP/2 for networks with intercepting proxies.",
                    state.ui_settings().electron_http1_compatibility_mode,
                    Some(UiSetting::ElectronHttp1CompatibilityMode),
                ),
            ]
            .spacing(13),
        ),
        settings_card(
            column![
                row![
                    subsection_title(
                        "Manage Sessions",
                        Some(
                            "Recover from a frozen terminal by killing sessions or restarting the underlying daemon."
                        )
                    ),
                    button(text("Refresh").size(9))
                        .on_press_maybe(
                            (!state.daemon_sessions_loading())
                                .then_some(Message::DaemonSessionsRefreshRequested)
                        )
                        .padding([4, 6])
                        .style(theme::ghost_button),
                    button(text("Kill all").size(9))
                        .on_press_maybe(
                            (!state.daemon_sessions_loading()
                                && !state.daemon_sessions().is_empty())
                                .then_some(Message::DaemonKillAllRequested)
                        )
                        .padding([4, 6])
                        .style(theme::ghost_button),
                    button(text("Restart").size(9))
                        .on_press_maybe(
                            (!state.daemon_sessions_loading())
                                .then_some(Message::DaemonRestartRequested)
                        )
                        .padding([4, 6])
                        .style(theme::ghost_button),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                sessions,
                confirmation,
                text(state.daemon_sessions_status().unwrap_or(""))
                    .size(10)
                    .color(theme::MUTED),
            ]
            .spacing(8),
        ),
    ]
    .spacing(14)
    .into()
}

fn plugins_content(state: &AppState) -> Element<'_, Message> {
    let mut installed = column![].spacing(8);
    if !state.ui_settings().plugin_system_enabled {
        installed = installed.push(
            text("Plugin discovery and code paths are disabled.")
                .size(11)
                .color(theme::MUTED),
        );
    } else if state.plugins_loading() {
        installed = installed.push(text("Discovering plugins…").size(12));
    } else if state.plugins().is_empty() {
        installed = installed.push(
            text("No installed or development plugins found.")
                .size(11)
                .color(theme::MUTED),
        );
    } else {
        for plugin in state.plugins() {
            let blocked = plugin.blocked_by_kill_list.is_some();
            let enabled = plugin.status != crate::plugins::PluginStatus::Disabled && !blocked;
            let status = if blocked {
                "BLOCKED"
            } else {
                match plugin.status {
                    crate::plugins::PluginStatus::Idle => "READY",
                    crate::plugins::PluginStatus::Pending => "REVIEW",
                    crate::plugins::PluginStatus::Disabled => "DISABLED",
                    crate::plugins::PluginStatus::Invalid => "INVALID",
                }
            };
            let details = if let Some(blocked) = &plugin.blocked_by_kill_list {
                format!("Blocked by Suaegi's plugin safety list: {}", blocked.reason)
            } else if let Some(error) = &plugin.error {
                error.clone()
            } else {
                let mut parts = vec![format!("{} · {}", plugin.plugin_key, plugin.version)];
                if plugin.is_dev {
                    parts.push("development".into());
                }
                if plugin.has_worker {
                    parts.push("worker".into());
                }
                if !plugin.capabilities.is_empty() {
                    parts.push(format!("grants: {}", plugin.capabilities.join(", ")));
                }
                let content_count = plugin.language_packs.len()
                    + plugin.keybindings.len()
                    + plugin.vm_recipes.len()
                    + plugin.agents.len();
                if content_count > 0 {
                    parts.push(format!("{content_count} content contribution(s)"));
                }
                parts.join(" · ")
            };
            let mut actions = row![].spacing(5).align_y(Alignment::Center);
            if plugin.status == crate::plugins::PluginStatus::Pending
                && plugin.consent_fingerprint.is_some()
                && !blocked
            {
                actions = actions.push(
                    button(text("Review & allow").size(11))
                        .on_press(Message::PluginConsentReviewRequested(
                            plugin.plugin_key.clone(),
                        ))
                        .padding([5, 8])
                        .style(theme::ghost_button),
                );
            }
            if plugin.status != crate::plugins::PluginStatus::Invalid && !blocked {
                actions = actions.push(
                    button(text(if enabled { "Disable" } else { "Enable" }).size(11))
                        .on_press(Message::PluginEnabledToggled(plugin.plugin_key.clone()))
                        .padding([5, 8])
                        .style(theme::ghost_button),
                );
            }
            if plugin.rollback_available {
                actions =
                    actions.push(
                        button(text("Rollback").size(11))
                            .on_press_maybe((!state.plugins_loading()).then(|| {
                                Message::PluginRollbackRequested(plugin.plugin_key.clone())
                            }))
                            .padding([5, 8])
                            .style(theme::ghost_button),
                    );
            }
            if !plugin.is_dev {
                actions = actions.push(
                    button(text("Remove").size(11))
                        .on_press(Message::PluginRemoveRequested(plugin.plugin_key.clone()))
                        .padding([5, 8])
                        .style(theme::danger_ghost_button),
                );
            }
            let mut command_actions = row![].spacing(5);
            if plugin.status == crate::plugins::PluginStatus::Idle && !blocked {
                for command in plugin.commands.iter() {
                    command_actions = command_actions.push(
                        button(text(&command.title).size(10))
                            .on_press(Message::PluginCommandInvoked(
                                plugin.plugin_key.clone(),
                                command.id.clone(),
                            ))
                            .padding([4, 7])
                            .style(theme::ghost_button),
                    );
                }
            }
            installed = installed.push(
                container(
                    row![
                        column![
                            row![
                                text(if plugin.name.is_empty() {
                                    &plugin.plugin_key
                                } else {
                                    &plugin.name
                                })
                                .size(12),
                                container(text(status).size(9))
                                    .padding([2, 5])
                                    .style(theme::chip),
                            ]
                            .spacing(6)
                            .align_y(Alignment::Center),
                            text(details).size(11).color(theme::MUTED),
                            command_actions,
                        ]
                        .spacing(3)
                        .width(Length::Fill),
                        actions,
                    ]
                    .align_y(Alignment::Center),
                )
                .padding([10, 12])
                .width(Length::Fill)
                .style(theme::context_panel),
            );
        }
    }
    let error: Element<'_, Message> = state.plugins_error().map_or_else(
        || Space::new().height(0).into(),
        |error| text(error).size(11).color(theme::MUTED).into(),
    );
    let dev_paths =
        state
            .ui_settings()
            .dev_plugin_paths
            .iter()
            .fold(column![].spacing(5), |column, path| {
                column.push(
                    row![
                        text(path).size(11).width(Length::Fill),
                        button(text("Remove").size(11))
                            .on_press(Message::PluginDevPathRemoved(path.clone()))
                            .padding([4, 7])
                            .style(theme::danger_ghost_button),
                    ]
                    .align_y(Alignment::Center),
                )
            });
    let marketplace_listings = state.plugin_marketplace_listings();
    let mut marketplace_plugins = column![].spacing(8);
    if state.plugin_marketplace_loading() && marketplace_listings.is_empty() {
        marketplace_plugins =
            marketplace_plugins.push(text("Loading marketplace plugins…").size(11));
    } else if marketplace_listings.is_empty() {
        marketplace_plugins = marketplace_plugins.push(
            text("No marketplace listings are cached yet.")
                .size(11)
                .color(theme::MUTED),
        );
    } else {
        for listing in marketplace_listings {
            let installed = state
                .plugins()
                .iter()
                .any(|plugin| plugin.plugin_key == listing.entry.id);
            let safety_block = state.plugin_kill_list_reason(&listing.entry.id);
            let description = listing
                .entry
                .description
                .as_deref()
                .unwrap_or("No description provided.")
                .to_string();
            let categories = if listing.entry.categories.is_empty() {
                String::new()
            } else {
                format!(" · {}", listing.entry.categories.join(", "))
            };
            marketplace_plugins = marketplace_plugins.push(
                container(
                    row![
                        column![
                            row![
                                text(listing.entry.id.clone()).size(12),
                                container(
                                    text(if installed { "INSTALLED" } else { "AVAILABLE" }).size(9)
                                )
                                .padding([2, 5])
                                .style(theme::chip),
                            ]
                            .spacing(6)
                            .align_y(Alignment::Center),
                            text(
                                safety_block
                                    .map(|reason| format!("Blocked by safety list: {reason}"))
                                    .unwrap_or(description)
                            )
                            .size(11)
                            .color(theme::TEXT),
                            text(format!(
                                "{} · {}{}",
                                listing.marketplace_name, listing.marketplace_owner, categories
                            ))
                            .size(10)
                            .color(theme::MUTED),
                        ]
                        .spacing(3)
                        .width(Length::Fill),
                        button(
                            text(if installed {
                                "Review update"
                            } else {
                                "Review & install"
                            })
                            .size(11)
                        )
                        .on_press_maybe(
                            (!state.plugin_marketplace_loading() && safety_block.is_none()).then(
                                || {
                                    Message::PluginMarketplaceInstallRequested(
                                        listing.marketplace_source_id.clone(),
                                        listing.marketplace_commit.clone(),
                                        listing.entry.id.clone(),
                                    )
                                }
                            )
                        )
                        .padding([5, 8])
                        .style(theme::ghost_button),
                    ]
                    .align_y(Alignment::Center),
                )
                .padding([10, 12])
                .width(Length::Fill)
                .style(theme::context_panel),
            );
        }
    }
    let mut marketplace_sources = column![].spacing(6);
    for source in state.plugin_marketplaces() {
        let official = crate::plugin_marketplace::is_official_source(&source.registration.source);
        let title = source
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.marketplace.name.as_str())
            .unwrap_or(source.registration.source.url.as_str());
        let status = source.refresh_error.as_deref().map_or_else(
            || {
                source.snapshot.as_ref().map_or_else(
                    || "No cached index".to_string(),
                    |snapshot| {
                        format!(
                            "{} plugins · commit {}",
                            snapshot.marketplace.plugins.len(),
                            &snapshot.marketplace_commit[..8]
                        )
                    },
                )
            },
            |error| format!("Last refresh failed; cached data kept · {error}"),
        );
        marketplace_sources = marketplace_sources.push(
            row![
                column![
                    row![
                        text(title).size(11),
                        if official {
                            container(text("OFFICIAL").size(9))
                                .padding([2, 5])
                                .style(theme::chip)
                        } else {
                            container(text("COMMUNITY").size(9))
                                .padding([2, 5])
                                .style(theme::chip)
                        },
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    text(format!(
                        "{}#{}",
                        source.registration.source.url, source.registration.source.git_ref
                    ))
                    .size(10)
                    .color(theme::MUTED),
                    text(status).size(10).color(theme::MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Refresh").size(10))
                    .on_press_maybe((!state.plugin_marketplace_loading()).then(|| {
                        Message::PluginMarketplaceRefreshRequested(Some(
                            source.registration.id.clone(),
                        ))
                    }))
                    .padding([4, 7])
                    .style(theme::ghost_button),
                button(text("Remove").size(10))
                    .on_press_maybe((!official && !state.plugin_marketplace_loading()).then(|| {
                        Message::PluginMarketplaceRemoveRequested(source.registration.id.clone())
                    }))
                    .padding([4, 7])
                    .style(theme::danger_ghost_button),
            ]
            .align_y(Alignment::Center),
        );
    }
    let consent_review: Element<'_, Message> = if let Some(plugin) = state.plugin_consent_review() {
        let mut capabilities = column![].spacing(5);
        if plugin.capabilities.is_empty() {
            capabilities = capabilities.push(
                text("This plugin requests no host API capabilities.")
                    .size(11)
                    .color(theme::MUTED),
            );
        } else {
            for capability in &plugin.capabilities {
                let description = match capability.as_str() {
                    "workspace:read" => "Read workspace metadata and selected file contents",
                    "terminal:send" => "Send text and commands to terminal sessions",
                    "notifications:show" => "Show desktop notifications",
                    "storage" => "Store plugin-owned data",
                    "secrets" => "Read and write plugin-owned secrets",
                    "events:subscribe" => "Subscribe to approved workspace and agent events",
                    "settings:own" => "Read and update plugin-owned settings",
                    _ => "Use a host capability",
                };
                capabilities = capabilities.push(
                    text(format!("{description}  ({capability})"))
                        .size(11)
                        .color(theme::TEXT),
                );
            }
        }
        let instructional = !plugin.vm_recipes.is_empty() || !plugin.keybindings.is_empty();
        let mut contributions = column![].spacing(5);
        for keybinding in &plugin.keybindings {
            contributions = contributions.push(
                text(format!(
                    "Shortcut {} → {} ({})",
                    keybinding.key,
                    keybinding.command,
                    keybinding.when.as_deref().unwrap_or("global")
                ))
                .size(10)
                .color(theme::MUTED),
            );
        }
        for recipe in &plugin.vm_recipe_specs {
            let mut recipe_lines = column![
                text(format!("VM recipe {} ({})", recipe.name, recipe.id)).size(10),
                text(format!(
                    "create: {}",
                    recipe.create.chars().take(240).collect::<String>()
                ))
                .size(10)
                .color(theme::MUTED),
            ]
            .spacing(2);
            if let Some(command) = &recipe.suspend {
                recipe_lines = recipe_lines.push(
                    text(format!(
                        "suspend: {}",
                        command.chars().take(240).collect::<String>()
                    ))
                    .size(10)
                    .color(theme::MUTED),
                );
            }
            if let Some(command) = &recipe.resume {
                recipe_lines = recipe_lines.push(
                    text(format!(
                        "resume: {}",
                        command.chars().take(240).collect::<String>()
                    ))
                    .size(10)
                    .color(theme::MUTED),
                );
            }
            recipe_lines = recipe_lines.push(
                text(if recipe.destroy_disabled {
                    "destroy: none".to_string()
                } else {
                    recipe.destroy.as_ref().map_or_else(
                        || "destroy: implicit".to_string(),
                        |command| {
                            format!("destroy: {}", command.chars().take(240).collect::<String>())
                        },
                    )
                })
                .size(10)
                .color(theme::MUTED),
            );
            contributions = contributions.push(recipe_lines);
        }
        if !plugin.language_packs.is_empty() {
            contributions = contributions.push(
                text(format!(
                    "{} validated language pack(s): {}",
                    plugin.language_packs.len(),
                    plugin
                        .language_packs
                        .iter()
                        .map(|pack| pack.locale.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .size(10)
                .color(theme::MUTED),
            );
        }
        let warning = if plugin.has_worker {
            "This plugin includes a background worker. Capability grants limit the Suaegi API, but the worker runs as a normal local process with access to files, network, and other processes."
        } else if instructional {
            "This plugin has no worker process. Its shortcuts and recipes can still cause actions when you or an agent uses them; review the contributed content before enabling it."
        } else if !plugin.panels.is_empty() {
            "This plugin has no background worker. Its panels can use only the reviewed Suaegi host capabilities."
        } else {
            "This plugin contributes validated declarative content and does not run a background worker."
        };
        let fingerprint = plugin.consent_fingerprint.clone().unwrap_or_default();
        settings_card(
            column![
                subsection_title(
                    "Review plugin permissions",
                    Some("Approval is bound to this exact worker, capability, and contribution fingerprint.")
                ),
                row![
                    column![
                        text(format!("{} v{}", plugin.name, plugin.version)).size(13),
                        text(format!("{} · {}", plugin.publisher, plugin.plugin_key))
                            .size(10)
                            .color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    container(text(if plugin.has_worker { "WORKER" } else { "DECLARATIVE" }).size(9))
                        .padding([2, 6])
                        .style(theme::chip),
                ]
                .align_y(Alignment::Center),
                text("This plugin can").size(10).color(theme::MUTED),
                capabilities,
                if instructional || !plugin.language_packs.is_empty() {
                    column![
                        text("Contributed instructional content")
                            .size(10)
                            .color(theme::MUTED),
                        contributions,
                    ]
                    .spacing(5)
                } else {
                    column![Space::new().height(0)]
                },
                container(text(warning).size(11))
                    .padding([9, 11])
                    .width(Length::Fill)
                    .style(theme::context_panel),
                row![
                    button(text("Keep disabled").size(11))
                        .on_press(Message::PluginConsentKeptDisabled(
                            plugin.plugin_key.clone()
                        ))
                        .padding([6, 9])
                        .style(theme::ghost_button),
                    button(text("Enable plugin").size(11))
                        .on_press(Message::PluginConsentGranted(
                            plugin.plugin_key.clone(),
                            fingerprint,
                        ))
                        .padding([6, 9])
                        .style(theme::primary_dark_button),
                ]
                .spacing(7),
            ]
            .spacing(10),
        )
    } else {
        Space::new().height(0).into()
    };
    let remove_confirmation: Element<'_, Message> = if let Some(plugin_key) =
        state.plugin_remove_confirmation()
    {
        settings_card(
                column![
                    subsection_title(
                        "Remove plugin?",
                        Some("This removes installed versions and plugin-owned data. Development source directories are never deleted.")
                    ),
                    text(plugin_key).size(12),
                    row![
                        button(text("Cancel").size(11))
                            .on_press(Message::PluginRemoveCancelled)
                            .padding([6, 9])
                            .style(theme::ghost_button),
                        button(text("Remove plugin").size(11))
                            .on_press(Message::PluginRemoveConfirmed(plugin_key.to_string()))
                            .padding([6, 9])
                            .style(theme::danger_ghost_button),
                    ]
                    .spacing(7),
                ]
                .spacing(9),
            )
    } else {
        Space::new().height(0).into()
    };
    column![
        consent_review,
        remove_confirmation,
        settings_card(
            column![
                subsection_title(
                    "Experimental plugin system",
                    Some("Discovery stays off until enabled. Capability or worker changes require renewed consent.")
                ),
                switch_row(
                    "Enable plugins",
                    "Allow manifest discovery, contributed panels and commands, and approved plugin workers.",
                    state.ui_settings().plugin_system_enabled,
                    Some(UiSetting::PluginSystemEnabled),
                ),
                row![
                    text(format!(
                        "Orca plugin API v1 · host compatibility {}",
                        crate::plugins::ORCA_PLUGIN_COMPAT_VERSION
                    ))
                    .size(11)
                    .color(theme::MUTED)
                    .width(Length::Fill),
                    button(text("Refresh").size(11))
                        .on_press_maybe(
                            state
                                .ui_settings()
                                .plugin_system_enabled
                                .then_some(Message::PluginsRefreshRequested)
                        )
                        .padding([5, 8])
                        .style(theme::ghost_button),
                ]
                .align_y(Alignment::Center),
                installed,
            ]
            .spacing(12)
        ),
        settings_card(
            column![
                row![
                    subsection_title(
                        "Plugin marketplace",
                        Some("Pinned Git catalogs use system Git credentials. The last valid commit stays available when a refresh fails.")
                    ),
                    button(text(if state.plugin_marketplace_loading() {
                        "Refreshing…"
                    } else {
                        "Refresh all"
                    })
                    .size(11))
                    .on_press_maybe(
                        (!state.plugin_marketplace_loading())
                            .then_some(Message::PluginMarketplaceRefreshRequested(None))
                    )
                    .padding([5, 8])
                    .style(theme::ghost_button),
                ]
                .align_y(Alignment::Center),
                marketplace_plugins,
                subsection_title(
                    "Marketplace sources",
                    Some("The official source is managed automatically. Add HTTPS or SSH Git catalogs for community or private plugins.")
                ),
                row![
                    text_input(
                        "https://git.example.com/team/plugins.git",
                        state.plugin_marketplace_url_draft()
                    )
                    .on_input(Message::PluginMarketplaceUrlChanged)
                    .size(11)
                    .padding([6, 8])
                    .width(Length::Fill),
                    text_input("Git ref", state.plugin_marketplace_ref_draft())
                        .on_input(Message::PluginMarketplaceRefChanged)
                        .size(11)
                        .padding([6, 8])
                        .width(120),
                    button(text("Add source").size(11))
                        .on_press_maybe(
                            (!state.plugin_marketplace_loading()
                                && !state.plugin_marketplace_url_draft().trim().is_empty()
                                && !state.plugin_marketplace_ref_draft().trim().is_empty())
                            .then_some(Message::PluginMarketplaceAddRequested)
                        )
                        .padding([6, 9])
                        .style(theme::ghost_button),
                ]
                .spacing(6),
                marketplace_sources,
            ]
            .spacing(10)
        ),
        settings_card(
            column![
                subsection_title(
                    "Install and develop",
                    Some("Install from an absolute directory or HTTPS/SSH Git URL with an optional #ref. Development paths run directly from disk.")
                ),
                row![
                    text_input("Absolute directory or Git URL#ref", state.plugin_dev_path_draft())
                        .on_input(Message::PluginDevPathChanged)
                        .size(11)
                        .padding([6, 8])
                        .width(Length::Fill),
                    button(text("Add").size(11))
                        .on_press(Message::PluginDevPathAdded)
                        .padding([6, 9])
                        .style(theme::ghost_button),
                    button(text("Install").size(11))
                        .on_press_maybe(
                            (!state.plugins_loading())
                                .then_some(Message::PluginLocalInstallRequested)
                        )
                        .padding([6, 9])
                        .style(theme::ghost_button),
                ]
                .spacing(6),
                error,
                dev_paths,
            ]
            .spacing(10)
        ),
    ]
    .spacing(14)
    .into()
}

fn experimental_content(state: &AppState) -> Element<'_, Message> {
    settings_card(
        column![
            subsection_title(
                "Experimental features",
                Some("Preview features may change behavior between builds.")
            ),
            switch_row(
                "Chat UI",
                "Preview the desktop chat surface for supported agent terminal sessions.",
                state.ui_settings().experimental_native_chat,
                Some(UiSetting::ExperimentalNativeChat),
            ),
            if state.ui_settings().experimental_native_chat {
                choice_row_owned(
                    "Default view",
                    if state.ui_settings().open_agent_tabs_in_chat_by_default {
                        "Chat UI"
                    } else {
                        "Terminal chat"
                    }
                    .to_string(),
                    UiChoice::NativeChatDefaultView,
                )
            } else {
                Space::new().height(0).into()
            },
            switch_row(
                "Animated pet",
                "Show the Orca-style animated companion overlay.",
                state.ui_settings().experimental_pet,
                Some(UiSetting::ExperimentalPet),
            ),
            switch_row(
                "Agent activity feed",
                "Show threaded agent completion and blocking events.",
                state.ui_settings().experimental_activity,
                Some(UiSetting::ExperimentalActivity),
            ),
            switch_row(
                "Terminal attention ring",
                "Keep a visible attention signal after bells and completions.",
                state.ui_settings().experimental_terminal_attention,
                Some(UiSetting::ExperimentalTerminalAttention),
            ),
            switch_row(
                "Agent hibernation",
                "Sleep completed resumable background agent terminals.",
                state.ui_settings().experimental_agent_hibernation,
                Some(UiSetting::ExperimentalAgentHibernation),
            ),
            if state.ui_settings().experimental_agent_hibernation {
                choice_row_owned(
                    "Sleep after",
                    format!(
                        "{} minutes",
                        state.ui_settings().agent_hibernation_idle_ms / 60_000
                    ),
                    UiChoice::AgentHibernationIdle,
                )
            } else {
                Space::new().height(0).into()
            },
            switch_row(
                "Compact worktree cards",
                "Hide redundant metadata when title and branch match.",
                state.ui_settings().compact_worktree_cards,
                Some(UiSetting::CompactWorktreeCards),
            ),
            switch_row(
                "Ephemeral VMs",
                "Enable per-workspace environment recipes and setup surfaces.",
                state.ui_settings().experimental_ephemeral_vms,
                Some(UiSetting::ExperimentalEphemeralVms),
            ),
        ]
        .spacing(13),
    )
}

fn ephemeral_vms_content(state: &AppState) -> Element<'_, Message> {
    use crate::ephemeral_vm::{CleanupStatus, RuntimeStatus};

    let mut recipes = column![].spacing(0);
    let mut recipe_count = 0usize;
    for repo in state.repos() {
        for recipe in state
            .vm_recipe_choices(&repo.id)
            .into_iter()
            .filter(|choice| !choice.id.is_empty())
        {
            recipe_count += 1;
            let repo_id = repo.id.clone();
            let recipe_id = recipe.id.clone();
            recipes = recipes.push(
                row![
                    column![
                        text(recipe.label).size(12),
                        text(format!("{} · {}", repo.display_name, recipe.id))
                            .size(10)
                            .color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(text("Use in workspace").size(10))
                        .on_press(Message::EphemeralVmRecipeUseRequested(repo_id, recipe_id))
                        .padding([5, 8])
                        .style(theme::ghost_button),
                ]
                .padding([9, 4])
                .align_y(Alignment::Center),
            );
        }
    }
    if recipe_count == 0 {
        recipes = recipes.push(
            text("No recipes found yet. Add environmentRecipes to orca.yaml or enable a plugin recipe.")
                .size(11)
                .color(theme::MUTED),
        );
    }

    let mut runtimes = column![].spacing(0);
    if state.ephemeral_vm_runtimes().is_empty() {
        runtimes = runtimes.push(
            text("No provisioned environments.")
                .size(11)
                .color(theme::MUTED),
        );
    }
    for runtime in state.ephemeral_vm_runtimes() {
        let busy = state.ephemeral_vm_busy(&runtime.id);
        let status = match runtime.status {
            RuntimeStatus::Provisioning => "provisioning",
            RuntimeStatus::Running => "running",
            RuntimeStatus::Suspended => "suspended",
            RuntimeStatus::SuspendFailed => "suspend failed",
            RuntimeStatus::ResumeFailed => "resume failed",
            RuntimeStatus::Failed => "failed",
            RuntimeStatus::CleanupPending => "cleanup pending",
            RuntimeStatus::CleanupFailed => "cleanup failed",
            RuntimeStatus::Cleaned => "cleaned",
        };
        let title = runtime
            .workspace_name
            .as_deref()
            .unwrap_or(&runtime.recipe_id)
            .to_string();
        let project_root = match runtime.recipe_result.connection() {
            crate::ephemeral_vm::RecipeConnection::OrcaServer { project_root, .. }
            | crate::ephemeral_vm::RecipeConnection::Ssh { project_root, .. } => project_root,
        };
        let mut actions = row![].spacing(4);
        if runtime.status == RuntimeStatus::Running && runtime.recipe.suspend.is_some() {
            actions = actions.push(
                button(text(if busy { "Working…" } else { "Suspend" }).size(10))
                    .on_press_maybe(
                        (!busy).then_some(Message::EphemeralVmSuspendRequested(runtime.id.clone())),
                    )
                    .padding([4, 7])
                    .style(theme::ghost_button),
            );
        }
        if matches!(
            runtime.status,
            RuntimeStatus::Suspended | RuntimeStatus::ResumeFailed
        ) && runtime.recipe.resume.is_some()
        {
            actions = actions.push(
                button(text(if busy { "Working…" } else { "Resume" }).size(10))
                    .on_press_maybe(
                        (!busy).then_some(Message::EphemeralVmResumeRequested(runtime.id.clone())),
                    )
                    .padding([4, 7])
                    .style(theme::ghost_button),
            );
        }
        if !matches!(runtime.status, RuntimeStatus::Cleaned)
            && runtime.cleanup_status != CleanupStatus::Disabled
        {
            actions = actions.push(
                button(
                    text(if busy {
                        "Working…"
                    } else if runtime.cleanup_status == CleanupStatus::Failed {
                        "Retry cleanup"
                    } else {
                        "Cleanup"
                    })
                    .size(10),
                )
                .on_press_maybe(
                    (!busy).then_some(Message::EphemeralVmCleanupRequested(runtime.id.clone())),
                )
                .padding([4, 7])
                .style(theme::ghost_button),
            );
        }
        runtimes = runtimes.push(
            row![
                text("●").size(9).color(
                    if matches!(
                        runtime.status,
                        RuntimeStatus::Failed
                            | RuntimeStatus::SuspendFailed
                            | RuntimeStatus::ResumeFailed
                            | RuntimeStatus::CleanupFailed
                    ) {
                        iced::Color::from_rgb8(0xc0, 0x39, 0x2b)
                    } else {
                        theme::MUTED
                    }
                ),
                column![
                    row![
                        text(title).size(12),
                        text(status).size(10).color(theme::MUTED)
                    ]
                    .spacing(6),
                    text(format!("{} · {}", runtime.recipe_id, project_root))
                        .size(10)
                        .color(theme::MUTED),
                    if let Some(error) = &runtime.cleanup_last_error {
                        text(error)
                            .size(10)
                            .color(iced::Color::from_rgb8(0xc0, 0x39, 0x2b))
                    } else {
                        text("").size(1)
                    },
                ]
                .spacing(2)
                .width(Length::Fill),
                actions,
            ]
            .spacing(8)
            .padding([9, 4])
            .align_y(Alignment::Center),
        );
    }

    column![
        settings_card(
            column![
                subsection_title(
                    "Per-workspace environment recipes",
                    Some("Recipes from orca.yaml and enabled plugins are available in the workspace composer.")
                ),
                recipes,
            ]
            .spacing(10)
        ),
        settings_card(
            column![
                subsection_title(
                    "Provisioned environments",
                    Some("Suspend, resume, or destroy environments using the immutable lifecycle commands saved when they were created.")
                ),
                runtimes,
            ]
            .spacing(10)
        ),
    ]
    .spacing(14)
    .into()
}

fn settings_card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding([15, 18])
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

fn subsection_title<'a>(title: &'a str, description: Option<&'a str>) -> Element<'a, Message> {
    let mut content = column![text(title).size(13)];
    if let Some(description) = description {
        content = content.push(text(description).size(11).color(theme::MUTED));
    }
    content.spacing(2).into()
}

fn switch_row<'a>(
    title: &'a str,
    description: &'a str,
    enabled: bool,
    setting: Option<UiSetting>,
) -> Element<'a, Message> {
    let indicator = container(text(if enabled { "●" } else { "○" }).size(13))
        .padding([2, 7])
        .style(if enabled {
            theme::active_card
        } else {
            theme::chip
        });
    let control: Element<'a, Message> = match setting {
        Some(setting) => button(indicator)
            .on_press(Message::UiSettingToggled(setting))
            .padding(0)
            .style(theme::ghost_button)
            .into(),
        None => indicator.into(),
    };
    row![
        column![
            text(title).size(12),
            text(description).size(11).color(theme::MUTED)
        ]
        .spacing(2)
        .width(Length::Fill),
        control,
    ]
    .align_y(Alignment::Center)
    .into()
}

fn action_switch_row<'a>(
    title: &'a str,
    description: &'a str,
    enabled: bool,
    message: Message,
) -> Element<'a, Message> {
    let indicator = container(text(if enabled { "●" } else { "○" }).size(13))
        .padding([2, 7])
        .style(if enabled {
            theme::active_card
        } else {
            theme::chip
        });
    row![
        column![
            text(title).size(12),
            text(description).size(11).color(theme::MUTED)
        ]
        .spacing(2)
        .width(Length::Fill),
        button(indicator)
            .on_press(message)
            .padding(0)
            .style(theme::ghost_button),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn choice_row<'a>(title: &'a str, value: &'a str, choice: UiChoice) -> Element<'a, Message> {
    row![
        text(title).size(12).width(Length::Fill),
        pick_list(
            choice_options(choice),
            Some(value.to_string()),
            move |value| Message::UiChoiceSelected(choice, value),
        )
        .width(Length::Fixed(176.0))
        .text_size(11),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn choice_row_owned<'a>(title: &'a str, value: String, choice: UiChoice) -> Element<'a, Message> {
    row![
        text(title).size(12).width(Length::Fill),
        pick_list(choice_options(choice), Some(value), move |value| {
            Message::UiChoiceSelected(choice, value)
        },)
        .width(Length::Fixed(176.0))
        .text_size(11),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn choice_options(choice: UiChoice) -> Vec<String> {
    if matches!(choice, UiChoice::Language) {
        return crate::i18n::language_options();
    }
    let values: &[&str] = match choice {
        UiChoice::TabOrder => &["Most recent", "In order"],
        UiChoice::AutoSaveDelay => &["300 ms", "500 ms", "800 ms", "1500 ms", "3000 ms"],
        UiChoice::DefaultDiffView => &["Inline", "Side by side"],
        UiChoice::BranchPrefix => &["git-username", "custom", "none"],
        UiChoice::SourceControlViewMode => &["list", "tree"],
        UiChoice::SourceControlGroupOrder => &["changes-first", "staged-first", "untracked-first"],
        UiChoice::Theme => &["system", "dark", "light"],
        UiChoice::Language => unreachable!("language options return above"),
        UiChoice::UiZoom => &["80%", "90%", "100%", "110%", "125%", "150%"],
        UiChoice::AppFontFamily => &["system", "Geist", "SF Pro", "Inter"],
        UiChoice::AppIcon => &["classic", "watercolor", "blue"],
        UiChoice::LeftSidebarAppearance => &["default", "match-terminal", "tinted"],
        UiChoice::LeftSidebarTintOpacity => &["0%", "4%", "8%", "12%", "20%", "28%", "35%"],
        UiChoice::UsagePercentageMode => &["used", "remaining"],
        UiChoice::TerminalFontSize => &[
            "11 px", "12 px", "13 px", "14 px", "16 px", "18 px", "20 px",
        ],
        UiChoice::TerminalFontFamily => &["SF Mono", "Menlo", "JetBrains Mono", "Fira Code"],
        UiChoice::TerminalFontWeight => &["300", "400", "500", "600"],
        UiChoice::TerminalLineHeight => &["100%", "110%", "120%", "130%"],
        UiChoice::TerminalScrollSensitivity => &["0.5x", "1x", "1.15x", "1.5x", "2x", "3x"],
        UiChoice::TerminalFastScrollSensitivity => &["1x", "2x", "5x", "7.5x", "10x"],
        UiChoice::TerminalTuiScrollMultiplier => &["1x", "2x", "3x", "5x", "10x"],
        UiChoice::TerminalGpuAcceleration | UiChoice::TerminalLigatures => &["auto", "on", "off"],
        UiChoice::TerminalCursorStyle => &["block", "bar", "underline"],
        UiChoice::TerminalThemeDark => &[
            "Ghostty Default Style Dark",
            "Dracula",
            "One Dark",
            "Nord",
            "Solarized Dark",
        ],
        UiChoice::TerminalThemeLight => &["Builtin Tango Light", "Solarized Light", "One Light"],
        UiChoice::SetupScriptLaunchMode => &["new-tab", "split-vertical", "split-horizontal"],
        UiChoice::TerminalPaddingX | UiChoice::TerminalPaddingY => {
            &["0 px", "2 px", "4 px", "8 px", "12 px", "16 px"]
        }
        UiChoice::TerminalCursorOpacity
        | UiChoice::TerminalBackgroundOpacity
        | UiChoice::TerminalInactivePaneOpacity
        | UiChoice::TerminalActivePaneOpacity => &["0%", "25%", "50%", "75%", "80%", "90%", "100%"],
        UiChoice::TerminalPaneOpacityTransition => &["0 ms", "70 ms", "140 ms", "250 ms", "400 ms"],
        UiChoice::TerminalDividerThickness => &["1 px", "2 px", "3 px", "4 px", "6 px"],
        UiChoice::TerminalScrollbackRows => &["1000 rows", "5000 rows", "10000 rows", "50000 rows"],
        UiChoice::TerminalShortcutPolicy => &["orca-first", "terminal-first"],
        UiChoice::TerminalMacOptionAsAlt => &["auto", "true", "false", "left", "right"],
        UiChoice::NotificationSound => &crate::notification_sound::SOUND_IDS,
        UiChoice::NotificationVolume => &["0%", "25%", "50%", "75%", "100%"],
        UiChoice::FloatingWorkspaceTrigger => &["floating-button", "status-bar"],
        UiChoice::BrowserSearchEngine => &["google", "duckduckgo", "kagi", "bing"],
        UiChoice::BrowserDefaultZoom => &["80%", "90%", "100%", "110%", "125%"],
        UiChoice::DefaultTaskSource => &["github", "gitlab", "linear", "jira"],
        UiChoice::DefaultTaskViewPreset => {
            &["all", "issues", "review", "my-issues", "my-prs", "prs"]
        }
        UiChoice::DefaultAgent => &[
            "auto", "blank", "claude", "codex", "gemini", "opencode", "aider", "amp",
        ],
        UiChoice::PromptCacheTtl => &["5 min", "60 min"],
        UiChoice::AgentHibernationIdle => &[
            "1 minute",
            "5 minutes",
            "15 minutes",
            "30 minutes",
            "60 minutes",
            "120 minutes",
            "240 minutes",
            "1440 minutes",
        ],
        UiChoice::VoiceModel => &[
            "Select Model",
            "Parakeet TDT v3",
            "Parakeet TDT v2",
            "Zipformer Bilingual",
            "Paraformer Bilingual",
            "Zipformer Streaming EN",
            "Zipformer Streaming ZH",
            "Whisper Tiny",
            "GPT-4o mini Transcribe",
            "GPT-4o Transcribe",
        ],
        UiChoice::VoiceLanguage => &["auto", "en", "ko", "ja"],
        UiChoice::VoiceDictationMode => &["toggle", "hold"],
        UiChoice::NativeChatDefaultView => &["Terminal chat", "Chat UI"],
        UiChoice::ClaudeAgentTeamsMode => &["Off", "Native panes", "In process"],
    };
    values.iter().map(|value| (*value).to_string()).collect()
}

fn text_setting_row<'a>(
    title: &'a str,
    value: &'a str,
    setting: UiTextSetting,
) -> Element<'a, Message> {
    row![
        text(title).size(12).width(Length::Fixed(154.0)),
        text_input("", value)
            .on_input(move |value| Message::UiTextSettingChanged(setting, value))
            .size(12)
            .padding([6, 8])
            .width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn section_copy(section: SettingsSection) -> (&'static str, &'static str) {
    match section {
        SettingsSection::Agents => ("Agents", "Configure coding agents and default behavior."),
        SettingsSection::ProviderAccounts => (
            "AI Provider Accounts",
            "Connect optional provider accounts and manage authentication.",
        ),
        SettingsSection::Orchestration => (
            "Orchestration",
            "Coordinate multiple coding agents through Suaegi.",
        ),
        SettingsSection::ComputerUse => (
            "Computer Use",
            "Enable agents to control apps on your computer.",
        ),
        SettingsSection::Voice => (
            "Voice",
            "Local speech-to-text dictation with on-device models.",
        ),
        SettingsSection::General => ("General", "Workspace defaults, app setup, and maintenance."),
        SettingsSection::Integrations => (
            "Integrations",
            "Connect Linear, Jira, and source-hosting services.",
        ),
        SettingsSection::Mobile => ("Mobile", "Control terminals and agents from your phone."),
        SettingsSection::Git => (
            "Git & Source Control",
            "Branch naming, base refs, attribution, and Git automation.",
        ),
        SettingsSection::TaskSources => (
            "Task Sources",
            "Choose where issues and pull requests are loaded from.",
        ),
        SettingsSection::Terminal => ("Terminal", "Configure terminal appearance and behavior."),
        SettingsSection::QuickCommands => (
            "Quick Commands",
            "Create reusable commands for common workspace actions.",
        ),
        SettingsSection::Browser => ("Browser", "Configure the built-in browser workspace."),
        SettingsSection::MobileEmulator => (
            "Mobile Emulator",
            "Configure mobile preview devices and development servers.",
        ),
        SettingsSection::FloatingWorkspace => (
            "Floating Workspace",
            "Keep a compact workspace available above other windows.",
        ),
        SettingsSection::Appearance => ("Appearance", "Customize the Suaegi interface."),
        SettingsSection::InputEditing => (
            "Input & Editing",
            "Configure text input, editing, and composer behavior.",
        ),
        SettingsSection::Notifications => {
            ("Notifications", "Choose when Suaegi should notify you.")
        }
        SettingsSection::Shortcuts => ("Shortcuts", "Review and customize keyboard shortcuts."),
        SettingsSection::StatsUsage => (
            "Stats & Usage",
            "Review agent usage, limits, and local resources.",
        ),
        SettingsSection::SshHosts => ("SSH Hosts", "Connect and manage remote development hosts."),
        SettingsSection::RemoteServers => (
            "Remote Suaegi Servers",
            "Connect to experimental remote Suaegi servers.",
        ),
        SettingsSection::MacPermissions => (
            "macOS Permissions",
            "Review system permissions required by desktop features.",
        ),
        SettingsSection::Privacy => (
            "Privacy & Telemetry",
            "Control diagnostics and anonymous usage reporting.",
        ),
        SettingsSection::Advanced => ("Advanced", "Configure advanced application behavior."),
        SettingsSection::Plugins => (
            "Plugins",
            "Install and review experimental Orca-compatible extensions.",
        ),
        SettingsSection::EphemeralVms => (
            "Ephemeral VMs",
            "Create and manage per-workspace development environments.",
        ),
        SettingsSection::Experimental => (
            "Experimental",
            "Try features that are still under active development.",
        ),
    }
}

fn section_search_text(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::Agents => {
            "agent cli default status hooks title awake prompt cache claude codex opencode"
        }
        SettingsSection::ProviderAccounts => {
            "account provider credentials runtime oauth gemini claude codex"
        }
        SettingsSection::Orchestration => "multi agent orchestration coordinate parallel",
        SettingsSection::ComputerUse => {
            "computer use accessibility screen recording desktop control permissions"
        }
        SettingsSection::Voice => "voice speech dictation transcription model language microphone",
        SettingsSection::General => {
            "tab order workspace directory nest delete autosave diff editor minimap spellcheck cli update"
        }
        SettingsSection::Integrations => "github gitlab linear jira tracker api key token",
        SettingsSection::Mobile => "mobile phone ios android pairing sidebar",
        SettingsSection::Git => {
            "git source control branch prefix base ref attribution ignored upstream changes"
        }
        SettingsSection::TaskSources => "tasks github jira linear issues pull requests projects",
        SettingsSection::Terminal => {
            "terminal shell font size weight line height theme gpu ligatures cursor scrollback clipboard osc52 option alt history"
        }
        SettingsSection::QuickCommands => "quick commands saved terminal command prompt",
        SettingsSection::Browser => {
            "browser home page search engine links localhost zoom cookies profile"
        }
        SettingsSection::MobileEmulator => "emulator simulator ios android sdk xcode device",
        SettingsSection::FloatingWorkspace => {
            "floating workspace terminal browser markdown directory button status bar"
        }
        SettingsSection::Appearance => {
            "appearance theme dark light language zoom font sidebar status usage resource ports blur"
        }
        SettingsSection::InputEditing => {
            "input editing selection middle click paste clipboard wrap"
        }
        SettingsSection::Notifications => {
            "notification banner sound agent complete terminal bell focused volume"
        }
        SettingsSection::Shortcuts => "shortcut keybinding keyboard command hotkey",
        SettingsSection::StatsUsage => "stats usage tokens analytics projects workspaces sessions",
        SettingsSection::SshHosts => "ssh host remote machine config authentication",
        SettingsSection::RemoteServers => "remote server runtime pairing web handoff",
        SettingsSection::MacPermissions => {
            "macos permissions full disk accessibility privacy security"
        }
        SettingsSection::Privacy => "privacy telemetry diagnostics anonymous secrets keychain",
        SettingsSection::Advanced => {
            "advanced proxy network http terminal parking authority delivery query reliability"
        }
        SettingsSection::Plugins => {
            "plugins extensions marketplace manifest capabilities consent worker panels commands development"
        }
        SettingsSection::EphemeralVms => {
            "ephemeral vm workspace environment recipe suspend resume cleanup cloud"
        }
        SettingsSection::Experimental => {
            "experimental native chat pet activity attention hibernation compact vm"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{choice_options, UiChoice};
    use crate::state::{AppState, Message, SettingsSection};

    #[test]
    fn claude_agent_teams_exposes_every_runtime_mode() {
        assert_eq!(
            choice_options(UiChoice::ClaudeAgentTeamsMode),
            ["Off", "Native panes", "In process"]
        );
    }

    #[test]
    fn every_settings_route_builds_a_complete_view() {
        let mut state = AppState::default();
        for section in [
            SettingsSection::Agents,
            SettingsSection::ProviderAccounts,
            SettingsSection::Orchestration,
            SettingsSection::ComputerUse,
            SettingsSection::Voice,
            SettingsSection::General,
            SettingsSection::Integrations,
            SettingsSection::Mobile,
            SettingsSection::Git,
            SettingsSection::TaskSources,
            SettingsSection::Terminal,
            SettingsSection::QuickCommands,
            SettingsSection::Browser,
            SettingsSection::MobileEmulator,
            SettingsSection::FloatingWorkspace,
            SettingsSection::Appearance,
            SettingsSection::InputEditing,
            SettingsSection::Notifications,
            SettingsSection::Shortcuts,
            SettingsSection::StatsUsage,
            SettingsSection::SshHosts,
            SettingsSection::RemoteServers,
            SettingsSection::MacPermissions,
            SettingsSection::Privacy,
            SettingsSection::Advanced,
            SettingsSection::Plugins,
            SettingsSection::EphemeralVms,
            SettingsSection::Experimental,
        ] {
            let _ = state.update(Message::SettingsSectionSelected(section));
            let rendered = super::view(&state);
            drop(rendered);
        }
    }

    #[test]
    fn integration_cards_render_connected_and_missing_cli_states() {
        let mut state = AppState::default();
        let _ = state.update(Message::HostedIntegrationsRefreshFinished(
            crate::hosted_integrations::HostedIntegrationStatuses {
                github: crate::hosted_integrations::CliIntegrationStatus::Connected,
                gitlab: crate::hosted_integrations::CliIntegrationStatus::NotInstalled,
                ..Default::default()
            },
        ));
        let _ = state.update(Message::SettingsSectionSelected(
            SettingsSection::Integrations,
        ));
        let rendered = super::view(&state);
        drop(rendered);
    }
}
