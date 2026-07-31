pub mod activity;
pub mod agent_status;
pub mod appearance;
pub mod attribution;
pub mod automation;
pub mod background;
pub mod branch_rename;
pub mod browser;
pub mod claude_agent_teams;
pub mod cli;
pub mod computer;
pub mod content_search;
pub mod diff_panel;
pub mod editor;
pub mod editor_font;
pub mod emulator;
pub mod ephemeral_vm;
pub mod external_editor;
pub mod file_explorer;
pub mod forge_tasks;
pub mod forge_ui;
pub mod fork_sync;
pub mod ghostty_import;
pub mod git_tasks;
pub mod hosted_integrations;
pub mod i18n;
pub mod icons;
pub mod keybinding_adapter;
pub mod keybindings;
pub mod layout;
pub mod local_rpc;
pub mod localhost_labels;
pub mod managed_accounts;
pub mod mcp_config;
pub mod memory;
pub mod mobile;
pub mod notification_sound;
pub mod onboarding;
pub mod orchestration;
pub mod persistence_thread;
pub mod plugin_content;
pub mod plugin_host;
pub mod plugin_kill_list;
pub mod plugin_marketplace;
pub mod plugin_panel;
pub mod plugin_worker;
pub mod plugins;
pub mod ports;
pub mod pr_panel;
pub mod presence_poll;
pub mod prompt_inject;
pub mod provider_credentials;
pub mod quick_open;
pub mod rate_limits;
pub mod reaper;
pub mod remote_fs;
pub mod remote_git;
pub mod remote_orca;
pub mod remote_runtime;
pub mod remote_search;
pub mod repo_hooks;
pub mod runtime_server;
pub mod runtime_terminal_bridge;
pub mod session_store;
pub mod settings;
pub mod sidebar;
pub mod source_control;
pub mod source_control_ai;
pub mod sparse_checkout;
pub mod speech;
pub mod speech_models;
pub mod spellcheck;
pub mod ssh;
pub mod state;
pub mod tab_title;
pub mod tasks;
pub mod terminal;
pub mod terminal_history;
pub mod theme;
pub mod tracker_tasks;
pub mod tracker_ui;
pub mod usage;
pub mod warp_theme_import;
pub mod workbench;
pub mod workspace_board;
pub mod worktree_linked_paths;

use i18n::text;
use iced::widget::{
    button, column, container, mouse_area, opaque, row, scrollable, stack, text_input, Space,
};
use iced::{mouse, Alignment, Color, Element, Length, Padding, Size, Subscription};

pub use state::{
    AgentScope, AppState, BoardStatus, FloatingWorkspaceContent, HelpAction, Message,
    MobileIosChannel, MobilePlatform, MobileStage, OpId, RightSidebarTab, SecretDraft,
    SettingsSection, StatusPopover, TaskKind, TaskPreset, TaskProvider, UiSetting,
    VoiceDictationState,
};

fn application_theme(state: &AppState) -> iced::Theme {
    theme::app_theme(&state.ui_settings().theme)
}

fn configured_app_font() -> iced::Font {
    use iced::font::Family;

    let mut store = suaegi_core::persistence::Store::new(persistence_thread::default_data_file());
    let family = match store.load().state.settings.ui.app_font_family.as_str() {
        "Geist" => Family::Name("Geist"),
        "SF Pro" => Family::Name("SF Pro Text"),
        "Inter" => Family::Name("Inter"),
        _ => Family::SansSerif,
    };
    iced::Font {
        family,
        ..iced::Font::DEFAULT
    }
}

fn terminal_renderer_backend(mode: &str) -> Option<&'static str> {
    match mode {
        "on" => Some("wgpu"),
        "off" => Some("tiny-skia"),
        _ => None,
    }
}

fn configure_renderer_backend() {
    let mut store = suaegi_core::persistence::Store::new(persistence_thread::default_data_file());
    if let Some(backend) =
        terminal_renderer_backend(&store.load().state.settings.ui.terminal_gpu_acceleration)
    {
        // Iced chooses one compositor when the application starts. Its WGPU
        // renderer is the native equivalent of Orca's WebGL path; tiny-skia
        // supplies the compatibility/CPU path. This environment change is
        // process-local and happens before Iced creates the compositor.
        std::env::set_var("ICED_BACKEND", backend);
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn title(&self) -> String {
        if self.ui_settings().show_titlebar_app_name {
            "Suaegi".to_string()
        } else {
            String::new()
        }
    }
    // The real `update` logic lives on `AppState` in `state.rs` — it dispatches
    // Task 3's git operations, so it needs `&mut self` and the full `Message`
    // match, not a thin wrapper here.
    pub fn view(&self) -> Element<'_, Message> {
        i18n::set_language(&self.ui_settings().ui_language);
        if self.integrations_open() {
            let shell = column![
                settings_window_title_bar(),
                settings::view(self),
                status_bar(self)
            ]
            .height(Length::Fill);
            let base: Element<'_, Message> = container(shell)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(if self.ui_settings().window_background_blur {
                    theme::app_canvas_translucent
                } else {
                    theme::app_canvas
                })
                .into();
            let mut layers = vec![base];
            if let Some(modal) = onboarding::view(self) {
                layers.push(opaque(
                    container(modal)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(theme::scrim),
                ));
            }
            if let Some(dialog) = voice_transcript_dialog(self) {
                layers.push(opaque(
                    container(dialog)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(theme::scrim),
                ));
            }
            return stack(layers)
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        let main_browser_open = self.browser_open() && !self.floating_workspace_owns_browser();
        let main: Element<'_, Message> = if self.emulator_panel_open() {
            emulator::view(self)
        } else if main_browser_open {
            browser::view(self)
        } else if self.workspace_board_open() {
            workspace_board::view(self)
        } else if self.mobile_open() {
            mobile::view(self)
        } else if self.activity_open() {
            activity::view(self)
        } else if self.automation_ui().open {
            automation::view(self).unwrap_or_else(empty_workspace)
        } else if self.tasks_open() || self.selected_worktree().is_none() {
            tasks::view(self)
        } else {
            if self.floating_workspace_owns_editor() {
                workbench::view(self)
            } else if self.workspace_editor_active() {
                editor::view(self).unwrap_or_else(|| workbench::view(self))
            } else {
                workbench::view(self)
            }
        };

        let left: Element<'_, Message> = if self.left_sidebar_open() {
            column![window_title_bar(self), sidebar::view(self)]
                .width(Length::Fixed(sidebar::WIDTH))
                .height(Length::Fill)
                .into()
        } else {
            collapsed_title_bar()
        };

        let mut regions: Vec<Element<'_, Message>> = vec![left, main];
        if !self.emulator_panel_open()
            && !main_browser_open
            && !self.workspace_board_open()
            && !self.mobile_open()
            && !self.activity_open()
            && !self.automation_ui().open
            && !self.tasks_open()
        {
            if let Some(panel) = right_sidebar(self) {
                regions.push(panel);
            } else if self.selected_worktree().is_some() {
                regions.push(collapsed_right_bar());
            }
        }

        let shell =
            column![row(regions).height(Length::Fill), status_bar(self),].height(Length::Fill);
        let base: Element<'_, Message> = container(shell)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(if self.ui_settings().window_background_blur {
                theme::app_canvas_translucent
            } else {
                theme::app_canvas
            })
            .into();

        let mut layers = vec![base];
        if self.ui_settings().experimental_pet {
            layers.push(pet_overlay(self));
        }
        if let Some(modal) = onboarding::view(self) {
            layers.push(opaque(
                container(modal)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::scrim),
            ));
        }
        if self.help_open() {
            layers.push(help_popover());
        }
        if self.workspace_options_open() {
            layers.push(workspace_options_popover(self));
        }
        if let Some(project_actions) = project_actions_popover(self) {
            layers.push(project_actions);
        }
        if let Some(worktree_actions) = worktree_actions_popover(self) {
            layers.push(worktree_actions);
        }
        if let Some(status) = status_popover(self) {
            layers.push(status);
        }
        if let Some(dialog) = project_remove_dialog(self) {
            layers.push(opaque(
                container(dialog)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::scrim),
            ));
        }
        if let Some(dialog) = worktree_remove_dialog(self) {
            layers.push(opaque(
                container(dialog)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::scrim),
            ));
        }
        if let Some(palette) = quick_open::view(self) {
            layers.push(opaque(
                container(
                    column![
                        Space::new().height(Length::Fixed(104.0)),
                        palette,
                        Space::new().height(Length::Fill),
                    ]
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::scrim),
            ));
        }
        if let Some(dialog) = pinned_pane_close_dialog(self) {
            layers.push(opaque(
                container(dialog)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::scrim),
            ));
        }
        if let Some(dialog) = running_pane_close_dialog(self) {
            layers.push(opaque(
                container(dialog)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::scrim),
            ));
        }
        if let Some(dialog) = voice_transcript_dialog(self) {
            layers.push(opaque(
                container(dialog)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::scrim),
            ));
        }
        if self.ui_settings().floating_workspace_enabled {
            if self.floating_workspace_open() {
                layers.push(floating_workspace_panel(self));
            }
            if self.ui_settings().floating_workspace_trigger == "floating-button" {
                layers.push(floating_workspace_button(self));
            }
        }
        stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// `workbench::subscription`(세션별 generation 피드)과
    /// `presence_poll::subscription`(티어링된 존재 폴링 타이머)을 하나로
    /// 묶는다. 둘 다 앱 전체에 딱 하나씩만 존재해야 하는 구독이므로
    /// `run()`이 이 함수 하나만 `.subscription(...)`에 건다 — 둘을 따로
    /// 걸면 나중에 셋째가 생겼을 때 배선 지점이 두 곳으로 늘어난다.
    pub fn subscription(&self) -> Subscription<Message> {
        keybindings::set_terminal_shortcut_policy(&self.ui_settings().terminal_shortcut_policy);
        let cursor_blink = if self.ui_settings().terminal_cursor_blink && self.panes().is_some() {
            iced::time::every(std::time::Duration::from_millis(530))
                .map(|_| Message::TerminalCursorBlinkTick)
        } else {
            Subscription::none()
        };
        let browser_location = if self.browser_open() {
            iced::time::every(std::time::Duration::from_millis(350))
                .map(|_| Message::BrowserLocationTick)
        } else {
            Subscription::none()
        };
        let emulator_frames = if self.emulator_panel_open() {
            iced::time::every(std::time::Duration::from_millis(750))
                .map(|_| Message::EmulatorFrameTick)
        } else {
            Subscription::none()
        };
        let filesystem_watch = if self.file_explorer().is_open() || self.editor().is_open() {
            iced::time::every(std::time::Duration::from_secs(2))
                .map(|_| Message::FileExplorerWatchTick)
        } else {
            Subscription::none()
        };
        let plugin_panel_watchdog = if self.active_plugin_panel().is_some() {
            iced::time::every(std::time::Duration::from_secs(10))
                .map(|_| Message::PluginPanelWatchdogTick)
        } else {
            Subscription::none()
        };
        let memory_refresh = if self.ui_settings().show_resource_status {
            iced::time::every(std::time::Duration::from_secs(3))
                .map(|_| Message::MemorySnapshotRefreshRequested)
        } else {
            Subscription::none()
        };
        let ports_refresh = if self.ui_settings().show_ports_status {
            iced::time::every(std::time::Duration::from_secs(30))
                .map(|_| Message::PortsRefreshRequested)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            iced::window::open_events().map(Message::WindowOpened),
            iced::event::listen_with(window_focus_message),
            workbench::subscription(self),
            presence_poll::subscription(self),
            keybindings::subscription(),
            quick_open::subscription(),
            automation::subscription(),
            plugin_panel::subscription(),
            plugin_worker::subscription(),
            cursor_blink,
            browser_location,
            emulator_frames,
            filesystem_watch,
            plugin_panel_watchdog,
            memory_refresh,
            ports_refresh,
        ])
    }
}

fn voice_transcript_dialog(state: &AppState) -> Option<Element<'_, Message>> {
    let transcript = state.pending_voice_transcript()?;
    Some(
        container(
            column![
                text("Insert dictation?").size(16),
                text("Review the transcription before inserting it into the focused terminal.")
                    .size(11)
                    .color(theme::MUTED),
                container(scrollable(text(transcript).size(12)).height(Length::Fixed(150.0)))
                    .padding(10)
                    .width(Length::Fill)
                    .style(theme::card),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(12))
                        .on_press(Message::VoiceTranscriptInsertCancelled)
                        .padding([7, 12])
                        .style(theme::ghost_button),
                    button(text("Insert").size(12))
                        .on_press(Message::VoiceTranscriptInsertConfirmed)
                        .padding([7, 12])
                        .style(theme::primary_dark_button),
                ]
                .spacing(7)
                .align_y(Alignment::Center),
            ]
            .spacing(12),
        )
        .padding(18)
        .width(Length::Fixed(460.0))
        .style(theme::modal)
        .into(),
    )
}

fn window_focus_message(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(iced::window::Event::Focused) => {
            Some(Message::AppWindowFocusChanged(true))
        }
        iced::Event::Window(iced::window::Event::Unfocused) => {
            Some(Message::AppWindowFocusChanged(false))
        }
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Message::AppWindowResized(size))
        }
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::FloatingWorkspacePointerMoved(position))
        }
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
            Some(Message::FloatingWorkspacePointerReleased)
        }
        _ => None,
    }
}

fn help_popover<'a>() -> Element<'a, Message> {
    let item = |label: &'static str, action: HelpAction| {
        button(text(label).size(12))
            .on_press(Message::HelpActionSelected(action))
            .width(Length::Fill)
            .padding([5, 7])
            .style(theme::ghost_button)
    };
    let popup = container(
        column![
            item("Keyboard Shortcuts", HelpAction::KeyboardShortcuts),
            item("Send Feedback", HelpAction::Feedback),
            button(text("Milestones      4 of 8").size(12))
                .on_press(Message::OnboardingOpened)
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            item("Docs", HelpAction::Docs),
            item("Changelog", HelpAction::Changelog),
            item("GitHub", HelpAction::Github),
            item("Discord", HelpAction::Discord),
            item("X", HelpAction::X),
            item("Check for Updates", HelpAction::CheckForUpdates),
        ]
        .spacing(1),
    )
    .width(Length::Fixed(210.0))
    .padding(6)
    .style(theme::modal);

    container(
        column![
            Space::new().height(Length::Fill),
            row![
                Space::new().width(Length::Fixed(8.0)),
                popup,
                Space::new().width(Length::Fill)
            ],
            Space::new().height(Length::Fixed(31.0)),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn workspace_options_popover(state: &AppState) -> Element<'_, Message> {
    let value_row = |label: &'static str, value: &'static str| {
        row![
            text(label).size(12).width(Length::Fill),
            text(value).size(11).color(theme::MUTED)
        ]
        .padding([4, 7])
        .align_y(Alignment::Center)
    };
    let toggle = |label: &'static str, enabled: bool, setting: UiSetting| {
        button(
            row![
                text(label).size(11).width(Length::Fill),
                text(if enabled { "●" } else { "○" })
                    .size(12)
                    .color(theme::MUTED)
            ]
            .align_y(Alignment::Center),
        )
        .on_press(Message::UiSettingToggled(setting))
        .width(Length::Fill)
        .padding([4, 7])
        .style(theme::ghost_button)
    };
    let popup = container(
        column![
            text("Workspace layout").size(11).color(theme::MUTED),
            value_row("Group by", "Project"),
            value_row("Sort by", "Recent"),
            value_row("Card layout", "Detailed"),
            text("Filters").size(11).color(theme::MUTED),
            toggle(
                "Hide sleeping",
                state.ui_settings().hide_sleeping,
                UiSetting::HideSleeping
            ),
            toggle(
                "Hide default branch",
                state.ui_settings().hide_default_branch,
                UiSetting::HideDefaultBranch
            ),
            toggle(
                "Hide detached HEAD",
                state.ui_settings().hide_detached_head,
                UiSetting::HideDetachedHead
            ),
        ]
        .spacing(1),
    )
    .width(Length::Fixed(210.0))
    .padding(7)
    .style(theme::modal);

    container(
        column![
            Space::new().height(Length::Fixed(185.0)),
            row![
                Space::new().width(Length::Fixed(8.0)),
                popup,
                Space::new().width(Length::Fill)
            ],
            Space::new().height(Length::Fill),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn project_actions_popover(state: &AppState) -> Option<Element<'_, Message>> {
    let repo = state.project_actions_open()?.clone();
    let repo_name = state.project_actions_repo_name().unwrap_or("Project");
    let popup = container(
        column![
            text(repo_name).size(11).color(theme::MUTED),
            button(text("Project Settings").size(12))
                .on_press(Message::SettingsOpened(SettingsSection::Git))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            button(text("Remove Project").size(12))
                .on_press(Message::ProjectRemoveRequested(repo))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::danger_ghost_button),
        ]
        .spacing(1),
    )
    .width(Length::Fixed(210.0))
    .padding(7)
    .style(theme::modal);

    Some(
        container(
            column![
                Space::new().height(Length::Fixed(225.0)),
                row![
                    Space::new().width(Length::Fixed(8.0)),
                    popup,
                    Space::new().width(Length::Fill)
                ],
                Space::new().height(Length::Fill),
            ]
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    )
}

fn project_remove_dialog(state: &AppState) -> Option<Element<'_, Message>> {
    let name = state.project_remove_confirm_name()?;
    Some(
        container(
            column![
                text(format!("Remove {name}?")).size(16),
                text("This removes the project from Suaegi. Repository files and worktrees remain on disk.")
                    .size(12)
                    .color(theme::MUTED),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(12))
                        .on_press(Message::ProjectRemoveCancelled)
                        .padding([6, 10])
                        .style(theme::ghost_button),
                    button(text("Remove Project").size(12))
                        .on_press(Message::ProjectRemoveConfirmed)
                        .padding([6, 10])
                        .style(theme::danger_ghost_button),
                ]
                .spacing(6),
            ]
            .spacing(12),
        )
        .width(Length::Fixed(400.0))
        .padding(18)
        .style(theme::modal)
        .into(),
    )
}

fn worktree_remove_dialog(state: &AppState) -> Option<Element<'_, Message>> {
    let name = state.worktree_remove_confirm_name()?;
    Some(
        container(
            column![
                text(format!("Delete {name}?")).size(16),
                text("The worktree checkout will be removed from disk. Dirty worktrees are protected by Git and will not be force-deleted.")
                    .size(12)
                    .color(theme::MUTED),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(12))
                        .on_press(Message::RemoveWorktreeCancelled)
                        .padding([6, 10])
                        .style(theme::ghost_button),
                    button(text("Delete Worktree").size(12))
                        .on_press(Message::RemoveWorktreeConfirmed)
                        .padding([6, 10])
                        .style(theme::danger_ghost_button),
                ]
                .spacing(6),
            ]
            .spacing(12),
        )
        .width(Length::Fixed(430.0))
        .padding(18)
        .style(theme::modal)
        .into(),
    )
}

fn pinned_pane_close_dialog(state: &AppState) -> Option<Element<'_, Message>> {
    let name = state.pinned_pane_close_confirm_name()?;
    Some(
        container(
            column![
                text(format!("Close pinned workspace {name}?")).size(16),
                text("The terminal session will stop. The workspace remains pinned and can be opened again.")
                    .size(12)
                    .color(theme::MUTED),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(12))
                        .on_press(Message::PinnedPaneCloseCancelled)
                        .padding([6, 10])
                        .style(theme::ghost_button),
                    button(text("Close").size(12))
                        .on_press(Message::PinnedPaneCloseConfirmed)
                        .padding([6, 10])
                        .style(theme::danger_ghost_button),
                ]
                .spacing(6),
            ]
            .spacing(12),
        )
        .width(Length::Fixed(430.0))
        .padding(18)
        .style(theme::modal)
        .into(),
    )
}

fn running_pane_close_dialog(state: &AppState) -> Option<Element<'_, Message>> {
    let name = state.running_pane_close_confirm_name()?;
    Some(
        container(
            column![
                text(format!("Stop the running agent in {name}?")).size(16),
                text("The terminal still has an active coding agent. Closing it will stop that process and end its session.")
                    .size(12)
                    .color(theme::MUTED),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Keep Running").size(12))
                        .on_press(Message::RunningPaneCloseCancelled)
                        .padding([6, 10])
                        .style(theme::ghost_button),
                    button(text("Stop and Close").size(12))
                        .on_press(Message::RunningPaneCloseConfirmed)
                        .padding([6, 10])
                        .style(theme::danger_ghost_button),
                ]
                .spacing(6),
            ]
            .spacing(12),
        )
        .width(Length::Fixed(430.0))
        .padding(18)
        .style(theme::modal)
        .into(),
    )
}

fn worktree_actions_popover(state: &AppState) -> Option<Element<'_, Message>> {
    let worktree = state.worktree_actions_open()?.clone();
    let remove_message = state.worktree_actions_remove_message();
    let pin_label = if state.worktree_is_pinned(&worktree) {
        "Unpin"
    } else {
        "Pin"
    };
    let unread_label = if state.worktree_is_unread(&worktree) {
        "Mark Read"
    } else {
        "Mark Unread"
    };
    let sleep_label = if state.worktree_is_sleeping(&worktree) {
        "Wake"
    } else {
        "Sleep"
    };
    let label = state
        .worktree_actions_label()
        .unwrap_or_else(|| "Workspace".to_string());
    let y = 235.0 + state.worktree_actions_index().unwrap_or(0) as f32 * 36.0;
    let popup = container(
        column![
            text("Workspace").size(11).color(theme::MUTED),
            text(label).size(12),
            button(text("Update").size(12))
                .on_press(Message::WorktreeRefreshRequested(worktree.clone()))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            text("Move to Status").size(11).color(theme::MUTED),
            button(text("  Todo").size(12))
                .on_press(Message::WorktreeBoardStatusSet(
                    worktree.clone(),
                    BoardStatus::Todo,
                ))
                .width(Length::Fill)
                .padding([4, 7])
                .style(theme::ghost_button),
            button(text("  In progress").size(12))
                .on_press(Message::WorktreeBoardStatusSet(
                    worktree.clone(),
                    BoardStatus::InProgress,
                ))
                .width(Length::Fill)
                .padding([4, 7])
                .style(theme::ghost_button),
            button(text("  In review").size(12))
                .on_press(Message::WorktreeBoardStatusSet(
                    worktree.clone(),
                    BoardStatus::InReview,
                ))
                .width(Length::Fill)
                .padding([4, 7])
                .style(theme::ghost_button),
            button(text("  Done").size(12))
                .on_press(Message::WorktreeBoardStatusSet(
                    worktree.clone(),
                    BoardStatus::Done,
                ))
                .width(Length::Fill)
                .padding([4, 7])
                .style(theme::ghost_button),
            button(text("Open in Finder").size(12))
                .on_press(Message::WorktreeOpenInFinder(worktree.clone()))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            open_in_app_rows(state, &worktree),
            button(text("Copy Path").size(12))
                .on_press(Message::WorktreeCopyPath(worktree.clone()))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            button(text(pin_label).size(12))
                .on_press(Message::WorktreePinToggled(worktree.clone()))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            button(text(unread_label).size(12))
                .on_press(Message::WorktreeUnreadToggled(worktree.clone()))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            button(text(sleep_label).size(12))
                .on_press(Message::WorktreeSleepToggled(worktree.clone()))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
            button(text("Delete").size(12))
                .on_press_maybe(remove_message)
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::danger_ghost_button),
        ]
        .spacing(1),
    )
    .width(Length::Fixed(210.0))
    .padding(7)
    .style(theme::modal);

    Some(
        container(
            column![
                Space::new().height(Length::Fixed(y)),
                row![
                    Space::new().width(Length::Fixed(8.0)),
                    popup,
                    Space::new().width(Length::Fill)
                ],
                Space::new().height(Length::Fill),
            ]
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    )
}

fn open_in_app_rows<'a>(
    state: &'a AppState,
    worktree: &suaegi_core::domain::WorktreeId,
) -> Element<'a, Message> {
    let mut rows = column![].spacing(1);
    for (index, application) in state.ui_settings().open_in_applications.iter().enumerate() {
        rows = rows.push(
            button(text(format!("Open in {}", application.label)).size(12))
                .on_press(Message::WorktreeOpenInApplication(worktree.clone(), index))
                .width(Length::Fill)
                .padding([5, 7])
                .style(theme::ghost_button),
        );
    }
    rows.into()
}

fn status_popover(state: &AppState) -> Option<Element<'_, Message>> {
    let selected = state.status_popover()?;
    let session_count = state.panes().map_or(0, |panes| panes.iter().count());
    let body: Element<'_, Message> = match selected {
        StatusPopover::Usage => {
            let mut usage = column![text("Usage").size(13)].spacing(7);
            if let Some(snapshot) = state.usage_snapshot() {
                usage = usage.push(
                    row![
                        text("Local token ledger").size(11).width(Length::Fill),
                        text(format_compact_tokens(snapshot.total_tokens())).size(11),
                    ]
                    .align_y(Alignment::Center),
                );
                for provider in &snapshot.providers {
                    if !provider.enabled {
                        continue;
                    }
                    usage = usage.push(
                        row![
                            text(provider.provider.label()).size(11).width(Length::Fill),
                            text(format_compact_tokens(provider.total_tokens))
                                .size(11)
                                .color(theme::MUTED),
                        ]
                        .align_y(Alignment::Center),
                    );
                }
            } else {
                usage = usage.push(
                    text("Enable local usage scanning to build a token ledger.")
                        .size(11)
                        .color(theme::MUTED),
                );
            }
            for provider in [
                crate::rate_limits::RateLimitProvider::Claude,
                crate::rate_limits::RateLimitProvider::Codex,
                crate::rate_limits::RateLimitProvider::Kimi,
                crate::rate_limits::RateLimitProvider::Grok,
                crate::rate_limits::RateLimitProvider::OpenCodeGo,
                crate::rate_limits::RateLimitProvider::MiniMax,
                crate::rate_limits::RateLimitProvider::Antigravity,
            ] {
                if state.provider_rate_limits_fetching(provider) {
                    usage = usage.push(
                        row![
                            text(provider.label()).size(11).width(Length::Fill),
                            text("Refreshing…").size(10).color(theme::MUTED),
                        ]
                        .align_y(Alignment::Center),
                    );
                    continue;
                }
                let Some(limits) = state.provider_rate_limits(provider) else {
                    continue;
                };
                usage = usage.push(text(provider.label()).size(11));
                if limits.buckets.is_empty() {
                    usage = usage.push(
                        text(
                            limits
                                .error
                                .as_deref()
                                .unwrap_or("No quota windows available."),
                        )
                        .size(10)
                        .color(theme::MUTED),
                    );
                } else {
                    for bucket in &limits.buckets {
                        let percent = crate::rate_limits::displayed_percentage(
                            bucket.used_percent,
                            &state.ui_settings().usage_percentage_mode,
                        );
                        let suffix = if state.ui_settings().usage_percentage_mode == "remaining" {
                            "remaining"
                        } else {
                            "used"
                        };
                        usage = usage.push(
                            row![
                                text(&bucket.name).size(11).width(Length::Fill),
                                text(format!("{percent}% {suffix}"))
                                    .size(11)
                                    .color(theme::MUTED),
                            ]
                            .align_y(Alignment::Center),
                        );
                    }
                }
            }
            if state.ui_settings().gemini_cli_oauth_enabled {
                usage = usage.push(text("Gemini CLI").size(11));
                if state.gemini_rate_limits_fetching() {
                    usage = usage.push(text("Refreshing limits…").size(11).color(theme::MUTED));
                } else if let Some(limits) = state.gemini_rate_limits() {
                    if limits.buckets.is_empty() {
                        usage = usage.push(
                            text(
                                limits
                                    .error
                                    .as_deref()
                                    .unwrap_or("No quota buckets available."),
                            )
                            .size(10)
                            .color(theme::MUTED),
                        );
                    } else {
                        for bucket in &limits.buckets {
                            let percent = crate::rate_limits::displayed_percentage(
                                bucket.used_percent,
                                &state.ui_settings().usage_percentage_mode,
                            );
                            let suffix = if state.ui_settings().usage_percentage_mode == "remaining"
                            {
                                "remaining"
                            } else {
                                "used"
                            };
                            usage = usage.push(
                                row![
                                    text(&bucket.name).size(11).width(Length::Fill),
                                    text(format!("{percent}% {suffix}"))
                                        .size(11)
                                        .color(theme::MUTED),
                                ]
                                .align_y(Alignment::Center),
                            );
                        }
                    }
                }
            }
            usage = usage.push(
                button(text("Open Stats & Usage").size(11))
                    .on_press(Message::SettingsOpened(SettingsSection::StatsUsage))
                    .padding([5, 7])
                    .style(theme::ghost_button),
            );
            usage.into()
        }
        StatusPopover::Resources => {
            let mut resources = column![text("Resource Manager").size(13)].spacing(7);
            if let Some(snapshot) = state.memory_snapshot() {
                let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
                resources = resources
                    .push(
                        row![
                            text("Suaegi").size(11).width(Length::Fill),
                            text(format!(
                                "{:.1}% · {:.1} MiB",
                                snapshot.app.cpu,
                                mib(snapshot.app.memory)
                            ))
                            .size(11),
                        ]
                        .align_y(Alignment::Center),
                    )
                    .push(
                        row![
                            text(format!("Terminals ({session_count})"))
                                .size(11)
                                .width(Length::Fill),
                            text(format!(
                                "{:.1} MiB",
                                mib(snapshot.total_memory.saturating_sub(snapshot.app.memory))
                            ))
                            .size(11),
                        ]
                        .align_y(Alignment::Center),
                    )
                    .push(
                        row![
                            text("Host memory").size(11).width(Length::Fill),
                            text(format!(
                                "{:.0}% · {:.1}/{:.1} GiB",
                                snapshot.host.memory_usage_percent,
                                snapshot.host.used_memory as f64 / (1024.0 * 1024.0 * 1024.0),
                                snapshot.host.total_memory as f64 / (1024.0 * 1024.0 * 1024.0)
                            ))
                            .size(11),
                        ]
                        .align_y(Alignment::Center),
                    );
                for worktree in &snapshot.worktrees {
                    resources = resources.push(
                        row![
                            text(&worktree.worktree_name).size(10).width(Length::Fill),
                            text(format!("{:.1} MiB", mib(worktree.memory)))
                                .size(10)
                                .color(theme::MUTED),
                        ]
                        .align_y(Alignment::Center),
                    );
                }
            } else {
                resources = resources.push(
                    text(if state.memory_snapshot_loading() {
                        "Collecting process memory…"
                    } else {
                        "Memory snapshot unavailable"
                    })
                    .size(11)
                    .color(theme::MUTED),
                );
            }
            resources.into()
        }
        StatusPopover::Ports => {
            let mut listeners = column![].spacing(5);
            if state.ports_loading() {
                listeners = listeners.push(text("Scanning listening ports…").size(11));
            } else if let Some(error) = state.ports_error() {
                listeners = listeners.push(text(error).size(11).color(theme::MUTED));
            } else if state.port_listeners().is_empty() {
                listeners = listeners.push(
                    text("No listening ports detected")
                        .size(11)
                        .color(theme::MUTED),
                );
            } else {
                let workspace_count = state
                    .port_listeners()
                    .iter()
                    .filter(|listener| listener.workspace)
                    .count();
                let external_count = state.port_listeners().len().saturating_sub(workspace_count);
                listeners = listeners.push(
                    text(format!(
                        "{workspace_count} workspace · {external_count} external"
                    ))
                    .size(10)
                    .color(theme::MUTED),
                );
                if workspace_count > 0 {
                    listeners = listeners.push(text("Workspace").size(11));
                }
                for listener in state
                    .port_listeners()
                    .iter()
                    .filter(|listener| listener.workspace)
                    .take(8)
                {
                    listeners = listeners.push(
                        row![
                            text(&listener.process).size(11).width(Length::Fill),
                            text(&listener.address).size(11).color(theme::MUTED),
                            button("Open")
                                .on_press(Message::PortOpenRequested(listener.address.clone()))
                                .padding([2, 5])
                                .style(theme::ghost_button),
                        ]
                        .align_y(Alignment::Center),
                    );
                }
                if external_count > 0 {
                    listeners = listeners.push(text("External").size(11));
                }
                for listener in state
                    .port_listeners()
                    .iter()
                    .filter(|listener| !listener.workspace)
                    .take(8)
                {
                    listeners = listeners.push(
                        row![
                            text(&listener.process).size(11).width(Length::Fill),
                            text(&listener.address).size(11).color(theme::MUTED),
                            button("Open")
                                .on_press(Message::PortOpenRequested(listener.address.clone()))
                                .padding([2, 5])
                                .style(theme::ghost_button),
                        ]
                        .align_y(Alignment::Center),
                    );
                }
            }
            column![
                row![
                    text("Ports").size(13).width(Length::Fill),
                    button(icons::view(icons::Icon::Refresh, 11.0, theme::MUTED))
                        .on_press(Message::PortsRefreshRequested)
                        .padding([3, 4])
                        .style(theme::ghost_button)
                ]
                .align_y(Alignment::Center),
                listeners,
            ]
            .spacing(7)
            .into()
        }
    };
    let popup = container(body)
        .width(Length::Fixed(280.0))
        .padding(12)
        .style(theme::modal);
    Some(
        container(
            column![
                Space::new().height(Length::Fill),
                row![
                    Space::new().width(Length::Fill),
                    popup,
                    Space::new().width(Length::Fixed(8.0)),
                ],
                Space::new().height(Length::Fixed(22.0)),
            ]
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    )
}

fn empty_workspace<'a>() -> Element<'a, Message> {
    container(text(""))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::app_canvas)
        .into()
}

fn traffic_control(color: Color, message: Message) -> Element<'static, Message> {
    button(text(""))
        .on_press(message)
        .width(Length::Fixed(12.0))
        .height(Length::Fixed(12.0))
        .padding(0)
        .style(theme::traffic_button(color))
        .into()
}

fn window_title_bar(state: &AppState) -> Element<'static, Message> {
    let drag_region = mouse_area(Space::new().width(Length::Fill).height(Length::Fixed(28.0)))
        .on_press(Message::WindowDrag)
        .on_double_click(Message::WindowMaximize);

    container(
        row![
            traffic_control(Color::from_rgb8(0xff, 0x5f, 0x57), Message::WindowClose),
            traffic_control(Color::from_rgb8(0xfe, 0xbc, 0x2e), Message::WindowMinimize),
            traffic_control(Color::from_rgb8(0x28, 0xc8, 0x40), Message::WindowMaximize),
            text("Suaegi").size(13),
            button(icons::view(icons::Icon::PanelLeft, 12.0, theme::MUTED))
                .on_press(Message::LeftSidebarToggled)
                .padding(0)
                .style(theme::ghost_button),
            drag_region,
            button(icons::view(icons::Icon::ArrowLeft, 11.0, theme::MUTED))
                .on_press_maybe(state.can_navigate_back().then_some(Message::NavigationBack))
                .padding(0)
                .style(theme::ghost_button),
            button(icons::view(icons::Icon::ArrowRight, 11.0, theme::MUTED))
                .on_press_maybe(
                    state
                        .can_navigate_forward()
                        .then_some(Message::NavigationForward),
                )
                .padding(0)
                .style(theme::ghost_button),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Length::Fixed(sidebar::WIDTH))
    .height(Length::Fixed(28.0))
    .padding([0, 10])
    .style(theme::sidebar_top_bar)
    .into()
}

fn settings_window_title_bar() -> Element<'static, Message> {
    let drag_region = mouse_area(Space::new().width(Length::Fill).height(Length::Fixed(28.0)))
        .on_press(Message::WindowDrag)
        .on_double_click(Message::WindowMaximize);

    container(
        row![
            traffic_control(Color::from_rgb8(0xff, 0x5f, 0x57), Message::WindowClose),
            traffic_control(Color::from_rgb8(0xfe, 0xbc, 0x2e), Message::WindowMinimize),
            traffic_control(Color::from_rgb8(0x28, 0xc8, 0x40), Message::WindowMaximize),
            text("Suaegi").size(13),
            drag_region,
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(28.0))
    .padding([0, 10])
    .style(theme::top_bar)
    .into()
}

fn collapsed_title_bar() -> Element<'static, Message> {
    container(
        button(icons::view(icons::Icon::PanelLeft, 13.0, theme::MUTED))
            .on_press(Message::LeftSidebarToggled)
            .padding([4, 5])
            .style(theme::ghost_button),
    )
    .width(Length::Fixed(32.0))
    .height(Length::Fill)
    .padding([2, 3])
    .style(theme::sidebar_top_bar)
    .into()
}

fn collapsed_right_bar() -> Element<'static, Message> {
    container(
        button(icons::view(icons::Icon::PanelRight, 13.0, theme::MUTED))
            .on_press(Message::RightSidebarToggled)
            .padding([4, 5])
            .style(theme::ghost_button),
    )
    .width(Length::Fixed(32.0))
    .height(Length::Fill)
    .padding([2, 3])
    .style(theme::top_bar)
    .into()
}

fn right_sidebar(state: &AppState) -> Option<Element<'_, Message>> {
    if !state.right_sidebar_open() {
        return None;
    }
    let worktree = state.selected_worktree()?.clone();
    let selected = state.right_sidebar_tab();
    let active_plugin = state.active_plugin_panel();
    let mut activity_buttons = row![
        button(icons::view(icons::Icon::Files, 14.0, theme::MUTED,))
            .on_press(Message::RightSidebarTabSelected(RightSidebarTab::Explorer))
            .padding([4, 8])
            .style(
                if active_plugin.is_none() && selected == RightSidebarTab::Explorer {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }
            ),
        button(icons::view(icons::Icon::Bot, 14.0, theme::MUTED,))
            .on_press(Message::RightSidebarTabSelected(RightSidebarTab::Agents))
            .padding([4, 8])
            .style(
                if active_plugin.is_none() && selected == RightSidebarTab::Agents {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }
            ),
        button(icons::view(icons::Icon::GitBranch, 14.0, theme::MUTED,))
            .on_press(Message::RightSidebarTabSelected(
                RightSidebarTab::SourceControl
            ))
            .padding([4, 8])
            .style(
                if active_plugin.is_none() && selected == RightSidebarTab::SourceControl {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }
            ),
        button(icons::view(icons::Icon::ListChecks, 14.0, theme::MUTED,))
            .on_press(Message::RightSidebarTabSelected(RightSidebarTab::Checks))
            .padding([4, 8])
            .style(
                if active_plugin.is_none() && selected == RightSidebarTab::Checks {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }
            ),
    ]
    .spacing(1)
    .align_y(Alignment::Center);
    for plugin in state.plugins().iter().filter(|plugin| {
        plugin.status == plugins::PluginStatus::Idle && plugin.blocked_by_kill_list.is_none()
    }) {
        for panel in &plugin.panels {
            let active = active_plugin.is_some_and(|(plugin_key, panel_id, _)| {
                plugin_key == plugin.plugin_key && panel_id == panel.id
            });
            let label = panel.title.chars().take(2).collect::<String>();
            activity_buttons = activity_buttons.push(
                button(text(label).size(10))
                    .on_press(Message::PluginPanelOpened(
                        plugin.plugin_key.clone(),
                        panel.id.clone(),
                    ))
                    .padding([4, 7])
                    .style(if active {
                        theme::selected_button
                    } else {
                        theme::ghost_button
                    }),
            );
        }
    }
    let activity_scroller = scrollable(activity_buttons)
        .direction(iced::widget::scrollable::Direction::Horizontal(
            iced::widget::scrollable::Scrollbar::new(),
        ))
        .width(Length::Fill)
        .height(Length::Fixed(24.0));
    let activity = container(
        row![
            activity_scroller,
            button(icons::view(icons::Icon::PanelRight, 14.0, theme::MUTED))
                .on_press(Message::RightSidebarToggled)
                .padding([4, 7])
                .style(theme::ghost_button),
        ]
        .spacing(1)
        .align_y(Alignment::Center),
    )
    .width(Length::Fixed(file_explorer::WIDTH))
    .height(Length::Fixed(26.0))
    .padding([2, 6])
    .style(theme::top_bar);

    let panel = if let Some((_, _, title)) = active_plugin {
        if let Some(error) = state.plugin_panel_error() {
            Some(
                container(
                    column![
                        text(title).size(13),
                        text(error)
                            .size(11)
                            .color(Color::from_rgb8(0xe0, 0x5b, 0x50)),
                        button(text("Close panel").size(11))
                            .on_press(Message::PluginPanelClosed)
                            .padding([5, 8])
                            .style(theme::ghost_button),
                    ]
                    .spacing(8),
                )
                .padding(12)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::context_panel)
                .into(),
            )
        } else {
            Some(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::context_panel)
                    .into(),
            )
        }
    } else {
        match selected {
            RightSidebarTab::Explorer => {
                content_search::view(state).or_else(|| file_explorer::view(state))
            }
            RightSidebarTab::Agents => Some(agents_panel(state)),
            RightSidebarTab::SourceControl => source_control::view(state),
            RightSidebarTab::Checks => sidebar::create_pr_view(state)
                .or_else(|| pr_panel::view(state.pr_panel()))
                .or_else(|| {
                    diff_panel::view(
                        state.diff(),
                        state.ui_settings().diff_default_side_by_side,
                        state.ui_settings().diff_word_wrap,
                        crate::editor_font::resolve(state.ui_settings()),
                    )
                })
                .or_else(|| Some(checks_panel(state, worktree))),
        }
    }?;

    Some(
        column![activity, panel]
            .height(Length::Fill)
            .width(Length::Fixed(file_explorer::WIDTH))
            .into(),
    )
}

fn agents_panel(state: &AppState) -> Element<'_, Message> {
    let normalized_query = state.agent_history_query().trim().to_lowercase();
    let selected_worktree = state.selected_worktree();
    let selected_repo = selected_worktree.and_then(|selected| {
        state.repos().iter().find(|repo| {
            state
                .worktrees_for(&repo.id)
                .iter()
                .any(|entry| state::worktree_id_for(&entry.path) == *selected)
        })
    });
    let mut entries = Vec::new();
    for repo in state.repos() {
        for entry in state.worktrees_for(&repo.id) {
            let id = state::worktree_id_for(&entry.path);
            let in_scope = match state.agent_scope() {
                AgentScope::Workspace => selected_worktree == Some(&id),
                AgentScope::Project => selected_repo.is_some_and(|selected| selected.id == repo.id),
                AgentScope::All => true,
            };
            if !in_scope {
                continue;
            }
            let branch = entry.branch.as_deref().unwrap_or("(detached)").to_string();
            let status = match state.worktree_badge(&id) {
                agent_status::contract::BadgeState::Working => "Working",
                agent_status::contract::BadgeState::Waiting => "Waiting",
                agent_status::contract::BadgeState::Done => "Done",
                agent_status::contract::BadgeState::Unknown => "Unknown",
            };
            if normalized_query.is_empty()
                || branch.to_lowercase().contains(&normalized_query)
                || repo.display_name.to_lowercase().contains(&normalized_query)
                || status.to_lowercase().contains(&normalized_query)
            {
                entries.push((id, branch, repo.display_name.clone(), status));
            }
        }
    }
    let shown = entries.len();
    let scope_label = match state.agent_scope() {
        AgentScope::Workspace => "workspace",
        AgentScope::Project => "project",
        AgentScope::All => "all projects",
    };
    let mut cards = column![].spacing(5);
    for (id, branch, repo, status) in entries {
        cards = cards.push(
            container(
                column![
                    row![
                        text(branch).size(14).width(Length::Fill),
                        text(status).size(12).color(theme::MUTED)
                    ]
                    .align_y(Alignment::Center),
                    text(format!("Agent · {repo}")).size(12).color(theme::MUTED),
                    button(text("Resume in Worktree").size(12))
                        .on_press(Message::WorktreeSelected(id))
                        .padding([3, 6])
                        .style(theme::ghost_button),
                ]
                .spacing(4),
            )
            .padding([7, 8])
            .width(Length::Fill)
            .style(theme::session_card),
        );
    }

    let header = column![
        row![
            column![
                text("Agent Session History").size(14),
                text(format!("{shown} shown · {scope_label}"))
                    .size(11)
                    .color(theme::MUTED)
            ]
            .spacing(1)
            .width(Length::Fill),
            text("Local Mac").size(12).color(theme::MUTED),
            button(icons::view(icons::Icon::Refresh, 12.0, theme::MUTED))
                .on_press(Message::PresenceTick)
                .padding([3, 4])
                .style(theme::ghost_button),
        ]
        .align_y(Alignment::Center),
        row![
            button(text("Workspace").size(12))
                .on_press(Message::AgentScopeSelected(AgentScope::Workspace))
                .padding([4, 9])
                .style(if state.agent_scope() == AgentScope::Workspace {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
            button(text("Project").size(12))
                .on_press(Message::AgentScopeSelected(AgentScope::Project))
                .padding([4, 9])
                .style(if state.agent_scope() == AgentScope::Project {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
            button(text("All").size(12))
                .on_press(Message::AgentScopeSelected(AgentScope::All))
                .padding([4, 9])
                .style(if state.agent_scope() == AgentScope::All {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
        ]
        .spacing(2),
        text_input("Search sessions", state.agent_history_query())
            .on_input(Message::AgentHistoryQueryChanged)
            .size(13)
            .padding([5, 7]),
    ]
    .spacing(6);

    container(
        column![
            header,
            scrollable(cards).height(Length::Fill).width(Length::Fill)
        ]
        .spacing(7),
    )
    .padding([7, 8])
    .width(Length::Fixed(file_explorer::WIDTH))
    .height(Length::Fill)
    .style(theme::context_panel)
    .into()
}

fn checks_panel(
    state: &AppState,
    worktree: suaegi_core::domain::WorktreeId,
) -> Element<'_, Message> {
    let (headline, detail, has_review) =
        match forge_ui::indicator_for(state.github_status_for(&worktree)) {
            forge_ui::PrIndicator::Hidden | forge_ui::PrIndicator::NoPr => (
                "No pull request found",
                "Create a pull request to start checks and review.".to_string(),
                false,
            ),
            forge_ui::PrIndicator::Checking => (
                "Checking pull request…",
                "Loading checks and review status.".to_string(),
                false,
            ),
            forge_ui::PrIndicator::Present {
                number,
                state,
                checks,
            } => (
                "Checks",
                format!("PR #{number} · {state:?}\n{checks:?}"),
                true,
            ),
            forge_ui::PrIndicator::Unknown(error) => {
                ("Status unavailable", format!("{error:?}"), false)
            }
        };
    let mut body = column![
        text(headline).size(14),
        text(detail).size(12).color(theme::MUTED),
        button(text("Refresh").size(12))
            .on_press(Message::GithubRefreshRequested {
                worktree: worktree.clone(),
            })
            .padding([4, 7])
            .style(theme::ghost_button),
    ]
    .spacing(5);
    if has_review {
        body = body.push(
            button(text("Compare changes").size(12))
                .on_press(Message::DiffRequested {
                    worktree: worktree.clone(),
                })
                .padding([5, 8])
                .style(theme::ghost_button),
        );
    }
    container(body)
        .padding([10, 10])
        .width(Length::Fixed(file_explorer::WIDTH))
        .height(Length::Fill)
        .style(theme::context_panel)
        .into()
}

fn format_compact_tokens(tokens: u64) -> String {
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

fn status_bar(state: &AppState) -> Element<'_, Message> {
    let session_count = state.session_store().sessions().count();
    let mut contents = row![].spacing(8).align_y(Alignment::Center);
    if state.ui_settings().show_usage_status {
        let providers = [
            crate::rate_limits::RateLimitProvider::Claude,
            crate::rate_limits::RateLimitProvider::Codex,
            crate::rate_limits::RateLimitProvider::Kimi,
            crate::rate_limits::RateLimitProvider::Grok,
            crate::rate_limits::RateLimitProvider::OpenCodeGo,
            crate::rate_limits::RateLimitProvider::MiniMax,
            crate::rate_limits::RateLimitProvider::Antigravity,
        ];
        let mut usage_row = row![].spacing(5).align_y(Alignment::Center);
        let mut visible_provider_count = 0;
        for provider in providers {
            let Some(limits) = state.provider_rate_limits(provider) else {
                continue;
            };
            if limits.buckets.is_empty() {
                continue;
            }
            if visible_provider_count > 0 {
                usage_row = usage_row.push(Space::new().width(Length::Fixed(3.0)));
            }
            let (mark, color) = match provider {
                crate::rate_limits::RateLimitProvider::Claude => {
                    ("✦", Color::from_rgb8(0xe4, 0x7f, 0x35))
                }
                crate::rate_limits::RateLimitProvider::Codex => {
                    ("◎", Color::from_rgb8(0xe4, 0xe4, 0xe7))
                }
                crate::rate_limits::RateLimitProvider::Kimi => {
                    ("K", Color::from_rgb8(0x6e, 0x8f, 0xff))
                }
                crate::rate_limits::RateLimitProvider::Grok => {
                    ("G", Color::from_rgb8(0xe4, 0xe4, 0xe7))
                }
                crate::rate_limits::RateLimitProvider::OpenCodeGo => {
                    ("O", Color::from_rgb8(0xf2, 0xc9, 0x4c))
                }
                crate::rate_limits::RateLimitProvider::MiniMax => {
                    ("M", Color::from_rgb8(0xd0, 0x7c, 0xff))
                }
                crate::rate_limits::RateLimitProvider::Antigravity => {
                    ("A", Color::from_rgb8(0x74, 0xc0, 0xfc))
                }
                crate::rate_limits::RateLimitProvider::Gemini => unreachable!(),
            };
            usage_row = usage_row.push(text(mark).size(11).color(color));
            for (index, bucket) in limits.buckets.iter().enumerate() {
                let percent = crate::rate_limits::displayed_percentage(
                    bucket.used_percent,
                    &state.ui_settings().usage_percentage_mode,
                );
                let suffix = if state.ui_settings().usage_percentage_mode == "remaining" {
                    "left"
                } else {
                    "used"
                };
                let window = if bucket.name.eq_ignore_ascii_case("Fable weekly") {
                    Some("Fable".to_string())
                } else {
                    crate::rate_limits::compact_reset_label(bucket.resets_at_unix_ms)
                };
                if index > 0 {
                    usage_row = usage_row.push(text("·").size(11).color(theme::MUTED));
                }
                usage_row = usage_row.push(
                    text(match window {
                        Some(window) => format!("{percent}% {suffix} {window}"),
                        None => format!("{percent}% {suffix}"),
                    })
                    .size(12)
                    .color(theme::MUTED),
                );
            }
            visible_provider_count += 1;
        }
        if visible_provider_count == 0 {
            let fallback = state
                .usage_snapshot()
                .map(|snapshot| format_compact_tokens(snapshot.total_tokens()))
                .filter(|value| value != "0")
                .unwrap_or_else(|| "—".to_string());
            usage_row = usage_row
                .push(text("✦").size(11).color(Color::from_rgb8(0xe4, 0x7f, 0x35)))
                .push(text(fallback).size(12).color(theme::MUTED));
        }
        contents = contents.push(
            button(usage_row)
                .on_press(Message::StatusPopoverToggled(StatusPopover::Usage))
                .padding(0)
                .style(theme::ghost_button),
        );
        contents = contents.push(
            button(icons::view(icons::Icon::Refresh, 11.0, theme::MUTED))
                .on_press(Message::RateLimitsRefreshRequested)
                .padding(0)
                .style(theme::ghost_button),
        );
    }
    contents = contents.push(Space::new().width(Length::Fill));
    if state.ui_settings().show_resource_status {
        let memory = state
            .memory_snapshot()
            .map(|snapshot| format!("{:.1} MB", snapshot.total_memory as f64 / 1_048_576.0))
            .unwrap_or_else(|| "—".to_string());
        contents = contents.push(
            button(
                row![
                    icons::view(icons::Icon::MemoryStick, 11.0, theme::MUTED),
                    text(memory).size(12).color(theme::MUTED),
                    text("·").size(11).color(theme::MUTED),
                    icons::view(icons::Icon::Terminal, 11.0, theme::MUTED),
                    text(session_count).size(12).color(theme::MUTED),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .on_press(Message::StatusPopoverToggled(StatusPopover::Resources))
            .padding(0)
            .style(theme::ghost_button),
        );
    }
    if state.ui_settings().show_ports_status {
        let workspace_ports = state
            .port_listeners()
            .iter()
            .filter(|listener| listener.workspace)
            .count();
        contents = contents.push(
            button(
                row![
                    icons::view(icons::Icon::Plug, 11.0, theme::MUTED),
                    text(workspace_ports).size(12).color(theme::MUTED),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .on_press(Message::StatusPopoverToggled(StatusPopover::Ports))
            .padding(0)
            .style(theme::ghost_button),
        );
    }
    if state.ui_settings().floating_workspace_enabled
        && state.ui_settings().floating_workspace_trigger == "status-bar"
    {
        contents = contents.push(
            button(
                text(if state.floating_workspace_open() {
                    "− Floating Workspace"
                } else {
                    "▣ Floating Workspace"
                })
                .size(12)
                .color(theme::MUTED),
            )
            .on_press(Message::FloatingWorkspaceToggled)
            .padding(0)
            .style(theme::ghost_button),
        );
    }

    container(contents)
        .height(Length::Fixed(19.0))
        .width(Length::Fill)
        .padding([0, 10])
        .style(theme::top_bar)
        .into()
}

fn floating_workspace_button(state: &AppState) -> Element<'_, Message> {
    let (x, y) = state.floating_workspace_trigger_position();
    let icon = container(icons::view(icons::Icon::PanelsTopLeft, 18.0, theme::TEXT))
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(40.0))
        .center_x(Length::Fixed(40.0))
        .center_y(Length::Fixed(40.0))
        .style(theme::floating_workspace_trigger);
    let face: Element<'_, Message> = if state.floating_workspace_has_attention() {
        let dot = container(
            container(Space::new())
                .width(Length::Fixed(8.0))
                .height(Length::Fixed(8.0))
                .style(theme::floating_workspace_attention),
        )
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(40.0))
        .padding(5)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Top);
        stack![icon, dot].into()
    } else {
        icon.into()
    };
    let trigger = mouse_area(face)
        .on_press(Message::FloatingWorkspaceDragStarted(
            state::FloatingWorkspaceDragTarget::Trigger,
        ))
        .on_move(move |position| {
            Message::FloatingWorkspacePointerMoved(iced::Point::new(x + position.x, y + position.y))
        })
        .on_release(Message::FloatingWorkspacePointerReleased)
        .interaction(mouse::Interaction::Grab);
    container(trigger)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Top)
        .padding(Padding {
            top: y,
            right: 0.0,
            bottom: 0.0,
            left: x,
        })
        .into()
}

fn pet_overlay(state: &AppState) -> Element<'_, Message> {
    container(
        button(
            column![
                text("🐬").size(30),
                text(state.pet_mood()).size(10).color(theme::MUTED),
            ]
            .spacing(1)
            .align_x(Alignment::Center),
        )
        .on_press(Message::UiSettingToggled(UiSetting::ExperimentalPet))
        .padding([6, 10])
        .style(theme::ghost_button),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(iced::alignment::Horizontal::Right)
    .align_y(iced::alignment::Vertical::Bottom)
    .padding([28, 20])
    .into()
}

fn floating_workspace_panel(state: &AppState) -> Element<'_, Message> {
    let content = state.floating_workspace_content();
    let (x, y, width, height) = state.floating_workspace_panel_geometry();
    let new_tab = button(icons::view(icons::Icon::Plus, 14.0, theme::MUTED))
        .on_press(Message::FloatingWorkspaceLauncherRequested)
        .padding([5, 8])
        .style(theme::ghost_button);
    let mut tab_controls = row![new_tab].spacing(1).align_y(Alignment::Center);
    for id in state.floating_workspace_sessions() {
        let selected = content == FloatingWorkspaceContent::Terminal
            && state.floating_workspace_session() == Some(*id);
        let select = button(
            row![
                icons::view(icons::Icon::Terminal, 12.0, theme::MUTED),
                text(state.session_title(*id)).size(12),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .on_press(Message::FloatingWorkspaceTerminalSelected(*id))
        .padding([5, 7]);
        let select = if selected {
            select.style(theme::selected_button)
        } else {
            select.style(theme::ghost_button)
        };
        tab_controls = tab_controls.push(
            row![
                select,
                button(text("×").size(12))
                    .on_press(Message::FloatingWorkspaceTerminalClosed(*id))
                    .padding([5, 5])
                    .style(theme::ghost_button),
            ]
            .spacing(0)
            .align_y(Alignment::Center),
        );
    }
    if matches!(
        content,
        FloatingWorkspaceContent::Browser | FloatingWorkspaceContent::Markdown
    ) {
        let (icon, label) = if content == FloatingWorkspaceContent::Browser {
            (icons::Icon::Globe, "Browser")
        } else {
            (icons::Icon::FileText, "Markdown")
        };
        tab_controls = tab_controls.push(
            row![
                button(
                    row![icons::view(icon, 12.0, theme::MUTED), text(label).size(12),]
                        .spacing(5)
                        .align_y(Alignment::Center),
                )
                .on_press(Message::FloatingWorkspaceContentSelected(content))
                .padding([5, 7])
                .style(theme::selected_button),
                button(text("×").size(12))
                    .on_press(Message::FloatingWorkspaceContentClosed)
                    .padding([5, 5])
                    .style(theme::ghost_button),
            ]
            .spacing(0)
            .align_y(Alignment::Center),
        );
    }
    let tab_controls: Element<'_, Message> = tab_controls.into();
    let window_size_icon = if state.floating_workspace_maximized() {
        icons::Icon::Restore
    } else {
        icons::Icon::Maximize
    };
    let title = row![
        tab_controls,
        Space::new().width(Length::Fill),
        button(text("✦").size(16).color(Color::from_rgb8(0xe4, 0x7f, 0x35)))
            .on_press(Message::FloatingWorkspaceClaudeRequested)
            .padding([3, 8])
            .style(theme::ghost_button),
        button(icons::view(window_size_icon, 14.0, theme::MUTED,))
            .on_press(Message::FloatingWorkspaceMaximizedToggled)
            .padding([5, 7])
            .style(theme::ghost_button),
        button(icons::view(icons::Icon::Minimize, 15.0, theme::MUTED,))
            .on_press(Message::FloatingWorkspaceMinimized)
            .padding([5, 7])
            .style(theme::ghost_button),
    ]
    .spacing(3)
    .align_y(Alignment::Center);
    let title = mouse_area(
        container(title)
            .width(Length::Fill)
            .height(Length::Fixed(37.0))
            .padding([4, 7]),
    )
    .on_press(Message::FloatingWorkspaceDragStarted(
        state::FloatingWorkspaceDragTarget::Panel,
    ))
    .on_move(move |position| {
        Message::FloatingWorkspacePointerMoved(iced::Point::new(x + position.x, y + position.y))
    })
    .on_release(Message::FloatingWorkspacePointerReleased)
    .interaction(mouse::Interaction::Grab);

    let launcher_button = |icon, label: &'static str, shortcut: &'static str, message| {
        button(
            row![
                icons::view(icon, 16.0, theme::MUTED),
                text(label).size(13),
                Space::new().width(Length::Fill),
                text(shortcut).size(11).color(theme::MUTED),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .on_press(message)
        .padding([9, 11])
        .width(Length::Fixed(260.0))
        .style(theme::ghost_button)
    };

    let body: Element<'_, Message> = match content {
        FloatingWorkspaceContent::Empty => container(
            column![
                launcher_button(
                    icons::Icon::Terminal,
                    "New Terminal",
                    "⌘ T",
                    Message::FloatingWorkspaceTerminalRequested,
                ),
                launcher_button(
                    icons::Icon::FileText,
                    "New Markdown Note",
                    "⌘ ⇧ M",
                    Message::FloatingWorkspaceNewMarkdownRequested,
                ),
                launcher_button(
                    icons::Icon::FileText,
                    "Open Markdown Note",
                    "⌘ ⇧ O",
                    Message::FloatingWorkspaceOpenMarkdownRequested,
                ),
                launcher_button(
                    icons::Icon::Globe,
                    "New Browser",
                    "⌘ ⇧ B",
                    Message::FloatingWorkspaceBrowserRequested,
                ),
                launcher_button(
                    icons::Icon::Minimize,
                    "Minimize",
                    "⌘ W",
                    Message::FloatingWorkspaceMinimized,
                ),
            ]
            .spacing(2)
            .align_x(Alignment::Start),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into(),
        FloatingWorkspaceContent::Terminal => state.floating_workspace_session().map_or_else(
            || {
                container(
                    column![
                        text("Starting terminal…").size(14),
                        text(state.ui_settings().floating_workspace_cwd.as_str())
                            .size(12)
                            .color(theme::MUTED),
                    ]
                    .spacing(7)
                    .align_x(Alignment::Center),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
            },
            |id| workbench::session_body(state, id),
        ),
        FloatingWorkspaceContent::Browser => browser::view(state),
        FloatingWorkspaceContent::Markdown => editor::view(state).unwrap_or_else(|| {
            container(text("Opening Markdown note…").size(13))
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        }),
    };
    let panel_body: Element<'_, Message> = if state.floating_workspace_maximized() {
        column![title, body].into()
    } else {
        let resize = mouse_area(
            container(
                container(text("◢").size(11).color(theme::MUTED))
                    .width(Length::Fixed(18.0))
                    .height(Length::Fixed(18.0))
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center),
            )
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(18.0))
            .align_x(iced::alignment::Horizontal::Right),
        )
        .on_press(Message::FloatingWorkspaceDragStarted(
            state::FloatingWorkspaceDragTarget::Resize,
        ))
        .on_move(move |position| {
            Message::FloatingWorkspacePointerMoved(iced::Point::new(
                x + width - 24.0 + position.x,
                y + height - 18.0 + position.y,
            ))
        })
        .on_release(Message::FloatingWorkspacePointerReleased)
        .interaction(mouse::Interaction::ResizingDiagonallyDown);
        column![
            title,
            body,
            row![Space::new().width(Length::Fill), resize]
                .height(Length::Fixed(18.0))
                .align_y(Alignment::End)
        ]
        .into()
    };
    let panel = container(panel_body)
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .style(theme::floating_workspace_panel);
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Top)
        .padding(Padding {
            top: y,
            right: 0.0,
            bottom: 0.0,
            left: x,
        })
        .into()
}

/// 훅 서버의 공유 비밀. 루프백 전용이지만 **같은 기계의 다른 프로세스**가 배지를
/// 위조하는 것은 막아야 하므로 추측 불가능해야 한다.
///
/// **엔트로피를 못 얻으면 `None`을 돌려주고 훅 기능 전체를 끈다.** 시계에서
/// 유도한 값은 같은 기계의 프로세스가 근사할 수 있으므로 토큰이 아니다 —
/// 그것을 로그로 알리는 것은 안전하게 만들지 못한다. 바인딩 실패와 **똑같이**
/// 다룬다: 배지 없이 계속 간다.
fn new_hook_token() -> Option<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut bytes)
        })
        .map_err(|e| {
            eprintln!(
                "suaegi: no OS entropy for the hook token ({e}); \
                 agent badges are disabled (a clock-derived token is guessable)"
            )
        })
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn run() -> iced::Result {
    configure_renderer_backend();
    orchestration::start_federation_relay();
    // Orca is single-instance. Besides avoiding duplicate windows, this protects the
    // restart-stable hook endpoint: a short-lived second GUI must not publish its port/token,
    // exit, and strand every surviving Claude PTY on a dead endpoint.
    if matches!(
        local_rpc::call("status", serde_json::Value::Null),
        Ok(Some(_))
    ) {
        return Ok(());
    }
    // **서버가 앱보다 먼저 뜬다.** 세션 스폰이 포트를 알아야 하므로 `boot()`
    // 이전에 바인딩한다. 실패하면 배지 없이 계속 간다 — 치명적이지 않다.
    let hooks = new_hook_token().and_then(|token| {
        agent_status::server::bind(token)
            .map_err(|e| eprintln!("suaegi: hook server did not start: {e} (badges stay Unknown)"))
            .ok()
    });
    // **서버 핸들은 `AppState`가 가져간다.** 떨구면 포트가 닫히고, 버린 이벤트
    // 카운터를 읽을 곳도 거기뿐이다. 여기 남는 것은 구독 레시피뿐이다.
    let (server, hook_sub) = match hooks {
        Some((server, rx)) => (
            Some(server),
            Some(agent_status::subscription::HookSub::new(1, rx)),
        ),
        None => (None, None),
    };
    let (local_rpc_server, local_rpc_sub) = match local_rpc::bind() {
        Ok((server, subscription)) => (Some(server), Some(subscription)),
        Err(error) => {
            eprintln!("suaegi: local CLI bridge did not start: {error}");
            (None, None)
        }
    };

    // `iced::application`은 부트 클로저를 여러 번 부르지 않지만 `Fn`을 요구하므로
    // 한 번만 꺼낼 수 있는 자리에 담아 옮긴다.
    let server = std::cell::RefCell::new(server);
    // **서버를 `boot`에 넘긴다.** 복원이 시작하는 세션도 스폰 시점에 포트를
    // 알아야 하므로, 붙이는 시점이 `begin_layout_restore()`보다 늦으면 재시작
    // 직후의 모든 pane이 훅 없이 뜬다.
    let boot = move || AppState::boot(server.borrow_mut().take());

    iced::application(boot, AppState::update, AppState::view)
        .title(AppState::title)
        .theme(application_theme)
        .default_font(configured_app_font())
        .scale_factor(|state: &AppState| state.ui_settings().ui_zoom_percent as f32 / 100.0)
        .subscription(move |state: &AppState| {
            let base = AppState::subscription(state);
            let mut subscriptions = vec![base];
            if let Some(sub) = &hook_sub {
                subscriptions.push(sub.subscription());
            }
            if let Some(sub) = &local_rpc_sub {
                subscriptions.push(sub.subscription());
            }
            Subscription::batch(subscriptions)
        })
        .window_size(Size {
            width: 1280.0,
            height: 800.0,
        })
        .centered()
        .decorations(false)
        .transparent(true)
        .run()
        .inspect(|_| drop(local_rpc_server))
}

#[cfg(test)]
mod renderer_setting_tests {
    use super::terminal_renderer_backend;

    #[test]
    fn terminal_gpu_setting_selects_the_iced_compositor() {
        assert_eq!(terminal_renderer_backend("on"), Some("wgpu"));
        assert_eq!(terminal_renderer_backend("off"), Some("tiny-skia"));
        assert_eq!(terminal_renderer_backend("auto"), None);
        assert_eq!(terminal_renderer_backend("invalid"), None);
    }
}
