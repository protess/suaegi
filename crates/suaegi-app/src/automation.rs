//! Persisted scheduled-prompt controls and deterministic due-run calculation.

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, pick_list, row, scrollable, text_input, Space};
use iced::{Alignment, Color, Element, Length, Subscription};
use suaegi_automation::{
    format_automation_schedule, is_valid_automation_schedule,
    latest_automation_occurrence_at_or_before, next_automation_occurrence_after, Tz,
};
use suaegi_core::domain::{AutomationConfig, WorktreeId};

use crate::i18n::text;
use crate::state::{AppState, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationTemplate {
    RepoHealth,
    ReleasePrep,
    DailyChanges,
    HourlyQueue,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Default)]
pub struct AutomationUiState {
    pub open: bool,
    pub editor_open: bool,
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    pub timezone: String,
    pub worktree: Option<WorktreeId>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub delete_confirm: Option<String>,
}

impl AutomationUiState {
    pub fn open(&mut self, selected: Option<WorktreeId>) {
        self.open = true;
        if self.worktree.is_none() {
            self.worktree = selected;
        }
        if self.schedule.is_empty() {
            self.schedule = "0 9 * * 1-5".to_string();
        }
        if self.timezone.is_empty() {
            self.timezone = "Asia/Seoul".to_string();
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.editor_open = false;
        self.error = None;
        self.delete_confirm = None;
    }

    pub fn reset_form(&mut self) {
        self.name.clear();
        self.prompt.clear();
        self.error = None;
    }

    pub fn begin_new(&mut self) {
        self.editor_open = true;
        self.error = None;
    }

    pub fn apply_template(&mut self, template: AutomationTemplate) {
        self.begin_new();
        let (name, schedule, prompt) = match template {
            AutomationTemplate::RepoHealth => (
                "Repository health check",
                "0 9 * * 1",
                "Review repository health, tests, and stale work.",
            ),
            AutomationTemplate::ReleasePrep => (
                "Release prep",
                "0 9 * * 5",
                "Review release readiness and summarize blockers.",
            ),
            AutomationTemplate::DailyChanges => (
                "Daily changes",
                "0 17 * * 1-5",
                "Summarize today's changes and remaining work.",
            ),
            AutomationTemplate::HourlyQueue => (
                "Hourly queue check",
                "0 * * * *",
                "Review queued tasks and report anything blocked.",
            ),
        };
        self.name = name.to_string();
        self.schedule = schedule.to_string();
        self.prompt = prompt.to_string();
    }
}

pub fn validate_draft(ui: &AutomationUiState, now: i64) -> Result<(WorktreeId, Tz), String> {
    let worktree = ui
        .worktree
        .clone()
        .ok_or_else(|| "Choose a worktree".to_string())?;
    if ui.name.trim().is_empty() {
        return Err("Name is required".to_string());
    }
    if ui.prompt.trim().is_empty() {
        return Err("Prompt is required".to_string());
    }
    let tz = Tz::from_str(ui.timezone.trim())
        .map_err(|_| "Timezone must be a valid IANA name".to_string())?;
    if !is_valid_automation_schedule(ui.schedule.trim(), now, tz) {
        return Err("Schedule is invalid or has no possible occurrence".to_string());
    }
    Ok((worktree, tz))
}

/// Returns only the latest due instant. If the app was offline for a week, it
/// dispatches once rather than replaying every missed minute on successive ticks.
pub fn due_at(config: &AutomationConfig, now: i64) -> Result<Option<i64>, String> {
    if !config.enabled {
        return Ok(None);
    }
    let tz = Tz::from_str(&config.timezone)
        .map_err(|_| format!("invalid timezone {}", config.timezone))?;
    let latest = latest_automation_occurrence_at_or_before(
        &config.schedule,
        config.dtstart_unix_ms,
        now,
        tz,
    )
    .map_err(|error| error.to_string())?;
    Ok(latest.filter(|latest| {
        config
            .last_dispatched_unix_ms
            .is_none_or(|last| *latest > last)
    }))
}

pub fn next_label(config: &AutomationConfig, now: i64) -> String {
    let Ok(tz) = Tz::from_str(&config.timezone) else {
        return "invalid timezone".to_string();
    };
    let after = config
        .last_dispatched_unix_ms
        .unwrap_or(now.max(config.dtstart_unix_ms).saturating_sub(1));
    match next_automation_occurrence_after(&config.schedule, config.dtstart_unix_ms, after, tz) {
        Ok(next) => format!(
            "{} · next {}",
            format_automation_schedule(&config.schedule, now, tz),
            next
        ),
        Err(error) => error.to_string(),
    }
}

pub fn subscription() -> Subscription<Message> {
    iced::time::every(std::time::Duration::from_secs(30)).map(|_| Message::AutomationTick(now_ms()))
}

pub fn view(state: &AppState) -> Option<Element<'_, Message>> {
    let ui = state.automation_ui();
    if !ui.open {
        return None;
    }
    let worktrees = state.automation_worktree_choices();
    let mut list = column![].spacing(7);
    let now = now_ms();
    for config in state.automations() {
        let id_for_toggle = config.id.clone();
        let id_for_run = config.id.clone();
        let id_for_delete = config.id.clone();
        list = list.push(
            column![
                row![
                    text(&config.name).size(14).width(Length::Fill),
                    button(if config.enabled { "Pause" } else { "Enable" })
                        .on_press(Message::AutomationToggled(id_for_toggle))
                        .style(crate::theme::ghost_button),
                    button("Run")
                        .on_press(Message::AutomationRunNow(id_for_run))
                        .style(crate::theme::ghost_button),
                    button("Delete")
                        .on_press(Message::AutomationDeleteRequested(id_for_delete))
                        .style(crate::theme::danger_ghost_button),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
                text(next_label(config, now)).size(12),
            ]
            .spacing(3),
        );
    }

    let form = column![
        text_input("Automation name", &ui.name).on_input(Message::AutomationNameChanged),
        pick_list(
            worktrees,
            ui.worktree.as_ref().map(|worktree| worktree.0.clone()),
            Message::AutomationWorktreeSelected,
        )
        .placeholder("Choose worktree"),
        text_input("Cron or RRULE", &ui.schedule).on_input(Message::AutomationScheduleChanged),
        text_input("IANA timezone", &ui.timezone).on_input(Message::AutomationTimezoneChanged),
        text_input("Prompt to send", &ui.prompt).on_input(Message::AutomationPromptChanged),
        button("Add automation").on_press(Message::AutomationCreated),
    ]
    .spacing(6);

    let templates = column![
        text("Start from a template").size(14),
        template_button(
            "REPO HEALTH",
            "Weekday repo audit",
            "Check dependencies, failing tests, and risky open changes each weekday.",
            AutomationTemplate::RepoHealth,
        ),
        template_button(
            "RELEASE PREP",
            "Release readiness",
            "Prepare a weekly release risk summary from the current project state.",
            AutomationTemplate::ReleasePrep,
        ),
        template_button(
            "RECURRING REVIEW",
            "Daily change review",
            "Scan recent work and call out correctness, UX, and test coverage risks.",
            AutomationTemplate::DailyChanges,
        ),
        template_button(
            "MAINTENANCE",
            "Hourly queue check",
            "Look for stuck work, stale generated files, and failed local validation.",
            AutomationTemplate::HourlyQueue,
        ),
        button(row![text("+").size(16), text("Add new").size(14)].spacing(7))
            .on_press(Message::AutomationNewRequested)
            .width(Length::Fill)
            .padding([7, 8])
            .style(crate::theme::ghost_button),
    ]
    .spacing(7);

    let left = column![
        templates,
        text("Scheduled").size(13).color(crate::theme::MUTED),
        scrollable(list).height(Length::Fill),
    ]
    .spacing(10)
    .padding(12);

    let mut details = column![row![
        text("Overview").size(14),
        text("Runs").size(14).color(crate::theme::MUTED),
    ]
    .spacing(18),]
    .spacing(12)
    .padding(16);
    if let Some(delete_id) = &ui.delete_confirm {
        let name = state
            .automations()
            .iter()
            .find(|item| &item.id == delete_id)
            .map(|item| item.name.as_str())
            .unwrap_or("this automation");
        details = details
            .push(text(format!("Delete {name}?")).size(18))
            .push(
                text("This removes the schedule and its local run history.")
                    .size(13)
                    .color(crate::theme::MUTED),
            )
            .push(
                row![
                    button("Cancel")
                        .on_press(Message::AutomationDeleteCancelled)
                        .style(crate::theme::ghost_button),
                    button("Delete")
                        .on_press(Message::AutomationDeleted(delete_id.clone()))
                        .style(crate::theme::danger_ghost_button),
                ]
                .spacing(6),
            );
    } else if ui.editor_open {
        details = details
            .push(text("New automation").size(18))
            .push(
                text("Choose a workspace, schedule, and prompt.")
                    .size(13)
                    .color(crate::theme::MUTED),
            )
            .push(container(form).width(Length::Fixed(440.0)));
    } else {
        details = details.push(
            container(
                column![text("Create an automation to start scheduling agent work.")
                    .size(13)
                    .color(crate::theme::MUTED),]
                .spacing(6)
                .align_x(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        );
    }

    let mut body = column![
        Space::new().height(Length::Fixed(28.0)),
        container(
            row![
                button("×")
                    .on_press(Message::AutomationClosed)
                    .padding([2, 7])
                    .style(crate::theme::ghost_button),
                text("▣").size(14),
                text("Automations").size(15).width(Length::Fill),
                button("+")
                    .on_press(Message::AutomationNewRequested)
                    .padding([2, 7])
                    .style(crate::theme::ghost_button),
                button("↻")
                    .on_press(Message::AutomationTick(now_ms()))
                    .padding([2, 7])
                    .style(crate::theme::ghost_button),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        )
        .height(Length::Fixed(36.0))
        .padding([5, 7])
        .style(crate::theme::top_bar),
        row![
            container(left)
                .width(Length::Fixed(268.0))
                .height(Length::Fill)
                .style(crate::theme::context_panel),
            container(details)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(crate::theme::app_canvas),
        ]
        .height(Length::Fill),
    ];
    if let Some(error) = &ui.error {
        body = body.push(
            text(error)
                .size(13)
                .color(Color::from_rgb(0.75, 0.22, 0.17)),
        );
    }
    if let Some(notice) = &ui.notice {
        body = body.push(
            text(notice)
                .size(13)
                .color(Color::from_rgb(0.18, 0.58, 0.31)),
        );
    }
    Some(
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(crate::theme::app_canvas)
            .into(),
    )
}

fn template_button(
    category: &'static str,
    title: &'static str,
    description: &'static str,
    template: AutomationTemplate,
) -> Element<'static, Message> {
    button(
        column![
            text(category).size(11).color(crate::theme::MUTED),
            text(title).size(14),
            text(description).size(12).color(crate::theme::MUTED),
        ]
        .spacing(3),
    )
    .on_press(Message::AutomationTemplateSelected(template))
    .width(Length::Fill)
    .padding(9)
    .style(crate::theme::template_card)
    .into()
}

#[cfg(test)]
mod tests {
    use super::{due_at, validate_draft, AutomationUiState};
    use suaegi_core::domain::{AutomationConfig, WorktreeId};

    #[test]
    fn due_run_is_emitted_once_per_occurrence() {
        let mut config = AutomationConfig {
            id: "a".into(),
            name: "a".into(),
            worktree_id: WorktreeId("wt".into()),
            schedule: "* * * * *".into(),
            prompt: "go".into(),
            timezone: "UTC".into(),
            provider: "claude".into(),
            dtstart_unix_ms: 0,
            enabled: true,
            last_dispatched_unix_ms: None,
        };
        let due = due_at(&config, 60_000).unwrap().unwrap();
        config.last_dispatched_unix_ms = Some(due);
        assert_eq!(due_at(&config, 60_000).unwrap(), None);
    }

    #[test]
    fn draft_requires_a_valid_timezone_schedule_prompt_and_worktree() {
        let mut ui = AutomationUiState {
            name: "nightly".into(),
            prompt: "run tests".into(),
            schedule: "0 9 * * *".into(),
            timezone: "UTC".into(),
            ..Default::default()
        };
        assert!(validate_draft(&ui, 0).is_err());
        ui.worktree = Some(WorktreeId("wt".into()));
        assert!(validate_draft(&ui, 0).is_ok());
        ui.timezone = "Mars/Olympus".into();
        assert!(validate_draft(&ui, 0).is_err());
    }
}
