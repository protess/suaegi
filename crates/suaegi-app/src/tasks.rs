use iced::widget::{button, column, container, row, text_input, Space};
use iced::{Alignment, Element, Length};
use serde_json::Value;
use std::path::PathBuf;

use crate::i18n::text;
use crate::state::{AppState, Message, TaskKind, TaskPreset, TaskProvider};
use crate::{icons, theme};
use suaegi_core::domain::RepoId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskWorkItem {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub updated_at: String,
    pub url: String,
}

pub const GITHUB_TASK_PAGE_SIZE: usize = 50;
const GITHUB_SEARCH_RESULT_WINDOW: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPageResult {
    pub items: Vec<TaskWorkItem>,
    pub total_pages: usize,
}

fn github_search_query(kind: TaskKind, query: &str, repository: &str) -> String {
    let mut query = query.trim().to_string();
    let qualifier = if kind == TaskKind::PullRequests {
        "is:pr"
    } else {
        "is:issue"
    };
    if !query.split_whitespace().any(|part| part == qualifier) {
        if !query.is_empty() {
            query.push(' ');
        }
        query.push_str(qualifier);
    }
    if !query.is_empty() {
        query.push(' ');
    }
    query.push_str("repo:");
    query.push_str(repository);
    query
}

fn parse_github_search_page(raw: &str) -> Result<TaskPageResult, String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| "GitHub returned unexpected task data.".to_string())?;
    let total_count = value
        .get("total_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let reachable_count = total_count.min(GITHUB_SEARCH_RESULT_WINDOW);
    let total_pages = reachable_count.div_ceil(GITHUB_TASK_PAGE_SIZE).max(1);
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some(TaskWorkItem {
                number: value.get("number")?.as_u64()?,
                title: value.get("title")?.as_str()?.to_string(),
                state: value
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("open")
                    .to_uppercase(),
                updated_at: value
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url: value.get("html_url")?.as_str()?.to_string(),
            })
        })
        .collect();
    Ok(TaskPageResult { items, total_pages })
}

pub fn load_github_work(
    op: crate::state::OpId,
    repo_id: RepoId,
    repo_path: PathBuf,
    kind: TaskKind,
    query: String,
    page: usize,
) -> iced::Task<Message> {
    iced::Task::perform(
        async move {
            if kind == TaskKind::Projects {
                let runner = suaegi_forge::GhRunner::new();
                let output = runner
                    .run(
                        &repo_path,
                        &[
                            "project", "list", "--owner", "@me", "--format", "json", "--limit",
                            "100",
                        ],
                    )
                    .await
                    .map_err(|error| {
                        if error.is_gh_not_found() {
                            "GitHub CLI is not installed.".to_string()
                        } else {
                            "GitHub projects could not be loaded.".to_string()
                        }
                    })?;
                return parse_projects(&output.stdout, &query).map(|items| TaskPageResult {
                    items,
                    total_pages: 1,
                });
            }
            let runner = suaegi_forge::GhRunner::new();
            let repository = runner
                .run(
                    &repo_path,
                    &[
                        "repo",
                        "view",
                        "--json",
                        "nameWithOwner",
                        "--jq",
                        ".nameWithOwner",
                    ],
                )
                .await
                .map_err(|error| {
                    if error.is_gh_not_found() {
                        "GitHub CLI is not installed.".to_string()
                    } else {
                        "GitHub repository identity could not be resolved.".to_string()
                    }
                })?;
            let repository = repository.stdout.trim();
            if repository.is_empty() {
                return Err("GitHub repository identity could not be resolved.".to_string());
            }
            let query = github_search_query(kind, &query, repository);
            let query_arg = format!("q={query}");
            let page_arg = format!("page={}", page.saturating_add(1));
            let per_page_arg = format!("per_page={GITHUB_TASK_PAGE_SIZE}");
            let output = runner
                .run(
                    &repo_path,
                    &[
                        "api",
                        "-X",
                        "GET",
                        "search/issues",
                        "-f",
                        query_arg.as_str(),
                        "-F",
                        page_arg.as_str(),
                        "-F",
                        per_page_arg.as_str(),
                    ],
                )
                .await
                .map_err(|error| {
                    if error.is_gh_not_found() {
                        "GitHub CLI is not installed.".to_string()
                    } else if error
                        .to_string()
                        .to_ascii_lowercase()
                        .contains("first 1000 search results")
                    {
                        format!(
                            "Page {} is beyond what GitHub search can return.",
                            page.saturating_add(1)
                        )
                    } else {
                        format!(
                            "Page {} could not be loaded from GitHub.",
                            page.saturating_add(1)
                        )
                    }
                })?;
            parse_github_search_page(&output.stdout)
        },
        move |result| Message::TaskItemsLoaded {
            op,
            repo_id: repo_id.clone(),
            kind,
            result,
        },
    )
}

pub fn load_gitlab_work(
    op: crate::state::OpId,
    repo_id: RepoId,
    repo_path: PathBuf,
    kind: TaskKind,
    preset: TaskPreset,
    query: String,
) -> iced::Task<Message> {
    iced::Task::perform(
        async move {
            let runner = suaegi_forge::GlabRunner::new();
            if kind == TaskKind::Projects {
                let output = runner
                    .run(&repo_path, &["api", "todos?state=pending&per_page=50"])
                    .await
                    .map_err(classify_glab_task_error)?;
                return parse_gitlab_todos(&output.stdout, &query).map(|items| TaskPageResult {
                    items,
                    total_pages: 1,
                });
            }

            let noun = if kind == TaskKind::PullRequests {
                "mr"
            } else {
                "issue"
            };
            let mut args = vec![
                noun.to_string(),
                "list".to_string(),
                "--output".to_string(),
                "json".to_string(),
                "--per-page".to_string(),
                "100".to_string(),
                "--state".to_string(),
                "opened".to_string(),
            ];
            match (kind, preset) {
                (TaskKind::Issues, TaskPreset::AssignedToMe) => {
                    args.extend(["--assignee".to_string(), "@me".to_string()]);
                }
                (TaskKind::PullRequests, TaskPreset::Mine) => {
                    args.extend(["--author".to_string(), "@me".to_string()]);
                }
                (TaskKind::PullRequests, TaskPreset::NeedsReview) => {
                    args.extend(["--reviewer".to_string(), "@me".to_string()]);
                }
                _ => {}
            }
            if !query.trim().is_empty() {
                args.extend(["--search".to_string(), query.trim().to_string()]);
            }
            let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
            let output = runner
                .run(&repo_path, &borrowed)
                .await
                .map_err(classify_glab_task_error)?;
            parse_gitlab_work_items(&output.stdout).map(|items| TaskPageResult {
                items,
                total_pages: 1,
            })
        },
        move |result| Message::TaskItemsLoaded {
            op,
            repo_id: repo_id.clone(),
            kind,
            result,
        },
    )
}

fn classify_glab_task_error(error: suaegi_forge::GlabError) -> String {
    if error.is_glab_not_found() {
        "GitLab CLI is not installed.".to_string()
    } else {
        match error {
            suaegi_forge::GlabError::Timeout { .. } => {
                "GitLab tasks timed out. Try again.".to_string()
            }
            _ => "GitLab tasks could not be loaded. Run `glab auth status` and try again."
                .to_string(),
        }
    }
}

fn parse_gitlab_work_items(raw: &str) -> Result<Vec<TaskWorkItem>, String> {
    let values: Vec<Value> = serde_json::from_str(raw)
        .map_err(|_| "GitLab returned unexpected task data.".to_string())?;
    Ok(values
        .into_iter()
        .filter_map(|value| {
            Some(TaskWorkItem {
                number: value
                    .get("iid")
                    .or_else(|| value.get("number"))
                    .and_then(Value::as_u64)?,
                title: value.get("title")?.as_str()?.to_string(),
                state: value
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("opened")
                    .to_uppercase(),
                updated_at: value
                    .get("updated_at")
                    .or_else(|| value.get("updatedAt"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url: value
                    .get("web_url")
                    .or_else(|| value.get("webUrl"))
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)?
                    .to_string(),
            })
        })
        .collect())
}

fn parse_gitlab_todos(raw: &str, query: &str) -> Result<Vec<TaskWorkItem>, String> {
    let values: Vec<Value> = serde_json::from_str(raw)
        .map_err(|_| "GitLab returned unexpected todo data.".to_string())?;
    let normalized_query = query.trim().to_lowercase();
    Ok(values
        .into_iter()
        .filter_map(|value| {
            let target = value.get("target")?;
            let title = target.get("title")?.as_str()?.to_string();
            if !normalized_query.is_empty() && !title.to_lowercase().contains(&normalized_query) {
                return None;
            }
            Some(TaskWorkItem {
                number: target
                    .get("iid")
                    .or_else(|| target.get("id"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                title,
                state: value
                    .get("action_name")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .replace('_', " ")
                    .to_uppercase(),
                updated_at: target
                    .get("updated_at")
                    .or_else(|| value.get("created_at"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url: target
                    .get("web_url")
                    .or_else(|| target.get("url"))
                    .and_then(Value::as_str)?
                    .to_string(),
            })
        })
        .collect())
}

fn parse_projects(raw: &str, query: &str) -> Result<Vec<TaskWorkItem>, String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| "GitHub returned unexpected project data.".to_string())?;
    let projects = value
        .get("projects")
        .and_then(Value::as_array)
        .ok_or_else(|| "GitHub returned unexpected project data.".to_string())?;
    let normalized_query = query.trim().to_lowercase();
    Ok(projects
        .iter()
        .filter_map(|project| {
            let title = project.get("title")?.as_str()?.to_string();
            if !normalized_query.is_empty() && !title.to_lowercase().contains(&normalized_query) {
                return None;
            }
            Some(TaskWorkItem {
                number: project.get("number").and_then(Value::as_u64).unwrap_or(0),
                title,
                state: if project
                    .get("closed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "CLOSED".to_string()
                } else {
                    "OPEN".to_string()
                },
                updated_at: project
                    .get("updatedAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url: project
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("https://github.com")
                    .to_string(),
            })
        })
        .collect())
}

pub fn view(state: &AppState) -> Element<'_, Message> {
    let selected_repos = state
        .repos()
        .iter()
        .filter(|repo| state.task_repo_selected(&repo.id))
        .collect::<Vec<_>>();
    let project = match selected_repos.as_slice() {
        [] => "No project".to_string(),
        [repo] => repo.display_name.clone(),
        _ if state.task_repo_selection_is_all() => "All projects".to_string(),
        repos => format!("{} projects", repos.len()),
    };

    let github_selected = state.task_provider() == TaskProvider::Github;
    let gitlab_selected = state.task_provider() == TaskProvider::Gitlab;
    let jira_selected = state.task_provider() == TaskProvider::Jira;
    let linear_selected = state.task_provider() == TaskProvider::Linear;

    let mut top = row![button(text("×").size(15))
        .on_press(Message::TasksClosed)
        .padding([3, 6])
        .style(theme::ghost_button),];
    if state.ui_settings().show_github_tasks {
        top = top.push(
            button(icons::view(icons::Icon::GitBranch, 13.0, theme::MUTED))
                .on_press(Message::TaskProviderSelected(TaskProvider::Github))
                .padding([4, 7])
                .style(if github_selected {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
        );
    }
    if state.ui_settings().show_gitlab_tasks {
        top = top.push(
            button(text("GL").size(10))
                .on_press(Message::TaskProviderSelected(TaskProvider::Gitlab))
                .padding([4, 7])
                .style(if gitlab_selected {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
        );
    }
    if state.ui_settings().show_jira_tasks {
        top = top.push(
            button(text("J").size(12))
                .on_press(Message::TaskProviderSelected(TaskProvider::Jira))
                .padding([4, 7])
                .style(if jira_selected {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
        );
    }
    if state.ui_settings().show_linear_tasks {
        top = top.push(
            button(text("L").size(12))
                .on_press(Message::TaskProviderSelected(TaskProvider::Linear))
                .padding([4, 7])
                .style(if linear_selected {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
        );
    }
    top = top
        .push(
            text(format!(
                "{} · Local Mac · {project}",
                if github_selected {
                    "GitHub"
                } else if gitlab_selected {
                    "GitLab"
                } else if jira_selected {
                    "Jira"
                } else {
                    "Linear"
                }
            ))
            .size(12)
            .color(theme::MUTED),
        )
        .push(Space::new().width(Length::Fill))
        .spacing(5)
        .align_y(Alignment::Center);

    let kind = state.task_kind();
    let mut project_scope = row![button(text("All projects").size(11))
        .on_press(Message::TaskRepoSelectionAll)
        .padding([3, 7])
        .style(if state.task_repo_selection_is_all() {
            theme::selected_button
        } else {
            theme::ghost_button
        })]
    .spacing(3)
    .align_y(Alignment::Center);
    for repo in state.repos() {
        project_scope = project_scope.push(
            button(text(repo.display_name.clone()).size(11))
                .on_press(Message::TaskRepoSelectionToggled(repo.id.clone()))
                .padding([3, 7])
                .style(if state.task_repo_selected(&repo.id) {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
        );
    }
    let sections = row![
        nav_button("Issues", TaskKind::Issues, kind),
        nav_button(
            if gitlab_selected { "MRs" } else { "PRs" },
            TaskKind::PullRequests,
            kind
        ),
        nav_button(
            if gitlab_selected { "Todos" } else { "Projects" },
            TaskKind::Projects,
            kind
        ),
        button(
            text(if gitlab_selected {
                "GitLab todos ↗"
            } else {
                "All projects ⌄"
            })
            .size(12)
        )
        .on_press(Message::TaskProjectsOpenRequested)
        .padding([4, 8])
        .style(theme::ghost_button),
    ]
    .spacing(3)
    .align_y(Alignment::Center);

    let scope = match kind {
        TaskKind::Issues => row![
            preset_button("Open", TaskPreset::Open, state.task_preset()),
            preset_button(
                "Assigned to me",
                TaskPreset::AssignedToMe,
                state.task_preset()
            ),
        ],
        TaskKind::PullRequests => row![
            preset_button("Open", TaskPreset::Open, state.task_preset()),
            preset_button("Mine", TaskPreset::Mine, state.task_preset()),
            preset_button("Needs review", TaskPreset::NeedsReview, state.task_preset()),
        ],
        TaskKind::Projects => row![text(if gitlab_selected {
            "Pending todos for the authenticated GitLab account"
        } else {
            "Projects visible to the current GitHub account"
        })
        .size(11)
        .color(theme::MUTED)],
    }
    .spacing(3);

    let (search_placeholder, empty_title) = match (state.task_provider(), kind) {
        (TaskProvider::Gitlab, TaskKind::PullRequests) => {
            ("Search GitLab merge requests…", "No matching GitLab MRs")
        }
        (TaskProvider::Gitlab, TaskKind::Projects) => {
            ("Search GitLab todos…", "No matching GitLab todos")
        }
        (TaskProvider::Gitlab, _) => ("Search GitLab issues…", "No matching GitLab issues"),
        (TaskProvider::Jira, _) => ("Search Jira issues…", "No matching Jira issues"),
        (TaskProvider::Linear, _) => ("Search Linear issues…", "No matching Linear issues"),
        (_, TaskKind::PullRequests) => ("Search GitHub pull requests…", "No matching GitHub PRs"),
        (_, TaskKind::Projects) => ("Search GitHub projects…", "No matching GitHub projects"),
        _ => ("Search GitHub issues…", "No matching GitHub issues"),
    };

    let query = row![
        text_input(search_placeholder, state.task_query())
            .on_input(Message::TaskQueryChanged)
            .on_submit(Message::TaskRefreshRequested)
            .size(13)
            .padding([5, 8])
            .width(Length::Fill),
        button(text("×").size(13))
            .on_press(Message::TaskQueryCleared)
            .padding([4, 7])
            .style(theme::ghost_button),
        button(text("+").size(15))
            .on_press(Message::TaskCreateRequested)
            .padding([3, 7])
            .style(theme::ghost_button),
        button(icons::view(icons::Icon::Refresh, 12.0, theme::MUTED))
            .on_press(Message::TaskRefreshRequested)
            .padding([4, 7])
            .style(theme::ghost_button),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    let header = match kind {
        TaskKind::PullRequests => row![
            text("ID").size(11).color(theme::MUTED),
            text("TITLE / CONTEXT")
                .size(11)
                .color(theme::MUTED)
                .width(Length::Fill),
            text("REVIEWERS").size(11).color(theme::MUTED),
            text("CHECKS").size(11).color(theme::MUTED),
            text("MERGE").size(11).color(theme::MUTED),
            text("UPDATED").size(11).color(theme::MUTED),
        ]
        .spacing(18),
        TaskKind::Projects => row![
            text("PROJECT").size(11).color(theme::MUTED),
            text("OWNER")
                .size(11)
                .color(theme::MUTED)
                .width(Length::Fill),
            text("ITEMS").size(11).color(theme::MUTED),
            text("UPDATED").size(11).color(theme::MUTED),
        ]
        .spacing(18),
        TaskKind::Issues => row![
            text("ID").size(11).color(theme::MUTED),
            text("TITLE / CONTEXT")
                .size(11)
                .color(theme::MUTED)
                .width(Length::Fill),
            text("ASSIGNEES").size(11).color(theme::MUTED),
            text("STATUS").size(11).color(theme::MUTED),
            text("UPDATED").size(11).color(theme::MUTED),
        ]
        .spacing(18),
    };

    let body: Element<'_, Message> = if state.task_provider() == TaskProvider::Jira {
        jira_body(state)
    } else if state.task_provider() == TaskProvider::Linear {
        linear_body(state)
    } else if state.task_items_loading() {
        container(
            text(if gitlab_selected {
                "Loading GitLab work…"
            } else {
                "Loading GitHub work…"
            })
            .size(13)
            .color(theme::MUTED),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into()
    } else if let Some(error) = state.task_items_error() {
        container(
            column![
                text(if gitlab_selected {
                    "GitLab work is unavailable"
                } else {
                    "GitHub work is unavailable"
                })
                .size(14),
                text(error).size(12).color(theme::MUTED),
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into()
    } else if state.task_items().is_empty() {
        container(
            column![
                text(empty_title).size(14),
                text("Change the query or clear it.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into()
    } else {
        let mut items = column![].spacing(2);
        for item in state.task_items() {
            let item_row: Element<'_, Message> = match kind {
                TaskKind::Projects => row![
                    text(&item.title).size(12).width(Length::Fill),
                    text(if gitlab_selected { "GitLab" } else { "GitHub" })
                        .size(11)
                        .color(theme::MUTED),
                    text(&item.state).size(11).color(theme::MUTED),
                    text(short_date(&item.updated_at))
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(18)
                .align_y(Alignment::Center)
                .into(),
                TaskKind::PullRequests => row![
                    text(format!(
                        "{}{}",
                        if gitlab_selected { "!" } else { "#" },
                        item.number
                    ))
                    .size(11)
                    .color(theme::MUTED),
                    text(&item.title).size(12).width(Length::Fill),
                    text("—").size(11).color(theme::MUTED),
                    text("—").size(11).color(theme::MUTED),
                    text(&item.state).size(11).color(theme::MUTED),
                    text(short_date(&item.updated_at))
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(18)
                .align_y(Alignment::Center)
                .into(),
                TaskKind::Issues => row![
                    text(format!("#{}", item.number))
                        .size(11)
                        .color(theme::MUTED),
                    text(&item.title).size(12).width(Length::Fill),
                    text("—").size(11).color(theme::MUTED),
                    text(&item.state).size(11).color(theme::MUTED),
                    text(short_date(&item.updated_at))
                        .size(11)
                        .color(theme::MUTED),
                ]
                .spacing(18)
                .align_y(Alignment::Center)
                .into(),
            };
            items = items.push(
                button(item_row)
                    .on_press(Message::TaskItemOpen(item.url.clone()))
                    .width(Length::Fill)
                    .padding([6, 7])
                    .style(theme::ghost_button),
            );
        }
        items.into()
    };

    let results = container(column![header, body].spacing(6))
        .padding([7, 9])
        .width(Length::Fill)
        .style(theme::session_card);

    let pagination: Element<'_, Message> =
        if github_selected && kind != TaskKind::Projects && state.task_total_pages() > 1 {
            let current = state.task_current_page();
            let total = state.task_total_pages();
            row![
                button(text("Previous").size(11))
                    .on_press_maybe(
                        (current > 0 && !state.task_items_loading())
                            .then_some(Message::TaskPageRequested(current.saturating_sub(1))),
                    )
                    .padding([4, 8])
                    .style(theme::ghost_button),
                text(format!("Page {} of {total}", current + 1))
                    .size(11)
                    .color(theme::MUTED),
                button(text("Next").size(11))
                    .on_press_maybe(
                        (current + 1 < total && !state.task_items_loading())
                            .then_some(Message::TaskPageRequested(current + 1)),
                    )
                    .padding([4, 8])
                    .style(theme::ghost_button),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            Space::new().height(Length::Fixed(0.0)).into()
        };

    container(
        column![
            top,
            project_scope,
            sections,
            scope,
            query,
            results,
            pagination
        ]
        .spacing(7)
        .padding([30, 22]),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::app_canvas)
    .into()
}

fn linear_body(state: &AppState) -> Element<'_, Message> {
    let linear = state.linear();
    if linear.workspace.is_none() {
        return container(
            column![
                text("Connect Linear to view tasks").size(14),
                text("Open Settings → Integrations and add a Linear API key.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into();
    }
    if linear.issues_loading {
        return container(text("Loading Linear issues…").size(13).color(theme::MUTED))
            .center_x(Length::Fill)
            .center_y(Length::Fixed(92.0))
            .into();
    }
    match linear.issues.as_ref() {
        Some(suaegi_tracker::Lookup::Found(page)) if !page.issues.is_empty() => {
            let query = state.task_query().trim().to_lowercase();
            let mut rows = column![].spacing(2);
            for issue in page.issues.iter().filter(|issue| {
                query.is_empty()
                    || issue.title.to_lowercase().contains(&query)
                    || issue.identifier.to_lowercase().contains(&query)
            }) {
                let Some(url) = issue.url.clone() else {
                    continue;
                };
                rows = rows.push(
                    button(
                        row![
                            text(&issue.identifier).size(11).color(theme::MUTED),
                            text(&issue.title).size(12).width(Length::Fill),
                            text(issue.assignee.as_deref().unwrap_or("Unassigned"))
                                .size(11)
                                .color(theme::MUTED),
                            text(issue.state.as_deref().unwrap_or("Open"))
                                .size(11)
                                .color(theme::MUTED),
                        ]
                        .spacing(18)
                        .align_y(Alignment::Center),
                    )
                    .on_press(Message::TaskItemOpen(url))
                    .width(Length::Fill)
                    .padding([6, 7])
                    .style(theme::ghost_button),
                );
            }
            rows.into()
        }
        Some(suaegi_tracker::Lookup::Unavailable(_)) => container(
            column![
                text("Linear is unavailable").size(14),
                text("Check the connection and retry.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into(),
        _ => container(text("No matching Linear issues").size(14))
            .center_x(Length::Fill)
            .center_y(Length::Fixed(92.0))
            .into(),
    }
}

fn jira_body(state: &AppState) -> Element<'_, Message> {
    let jira = state.jira();
    if jira.connection.is_none() {
        return container(
            column![
                text("Connect Jira to view tasks").size(14),
                text("Open Settings → Integrations and add a Jira site.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into();
    }
    if jira.issues_loading {
        return container(text("Loading Jira issues…").size(13).color(theme::MUTED))
            .center_x(Length::Fill)
            .center_y(Length::Fixed(92.0))
            .into();
    }
    match jira.issues.as_ref() {
        Some(suaegi_tracker::Lookup::Found(page)) if !page.items.is_empty() => {
            let mut rows = column![].spacing(2);
            for issue in &page.items {
                rows = rows.push(
                    button(
                        row![
                            text(&issue.key).size(11).color(theme::MUTED),
                            text(&issue.title).size(12).width(Length::Fill),
                            text(issue.assignee.as_deref().unwrap_or("Unassigned"))
                                .size(11)
                                .color(theme::MUTED),
                            text(issue.status.as_deref().unwrap_or("Open"))
                                .size(11)
                                .color(theme::MUTED),
                        ]
                        .spacing(18)
                        .align_y(Alignment::Center),
                    )
                    .on_press(Message::TaskItemOpen(issue.url.clone()))
                    .width(Length::Fill)
                    .padding([6, 7])
                    .style(theme::ghost_button),
                );
            }
            rows.into()
        }
        Some(suaegi_tracker::Lookup::Unavailable(_)) => container(
            column![
                text("Jira is unavailable").size(14),
                text("Check the connection and retry.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into(),
        _ => container(
            column![
                text("No matching Jira issues").size(14),
                text("Change the filter or refresh.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(92.0))
        .into(),
    }
}

fn short_date(value: &str) -> &str {
    value.get(..10).unwrap_or(value)
}

fn nav_button<'a>(
    label: &'static str,
    target: TaskKind,
    selected: TaskKind,
) -> iced::widget::Button<'a, Message> {
    button(text(label).size(12))
        .on_press(Message::TaskKindSelected(target))
        .padding([4, 8])
        .style(if target == selected {
            theme::selected_button
        } else {
            theme::ghost_button
        })
}

fn preset_button<'a>(
    label: &'static str,
    target: TaskPreset,
    selected: TaskPreset,
) -> iced::widget::Button<'a, Message> {
    button(text(label).size(12))
        .on_press(Message::TaskPresetSelected(target))
        .padding([4, 9])
        .style(if target == selected {
            theme::selected_button
        } else {
            theme::ghost_button
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_search_pages_and_caps_the_advertised_window() {
        let page = parse_github_search_page(
            r#"{"total_count":1400,"items":[{"number":42,"title":"Clone tasks","state":"open","updated_at":"2026-07-28T01:02:03Z","html_url":"https://github.com/stablyai/orca/issues/42"}]}"#,
        )
        .unwrap();
        assert_eq!(page.items[0].number, 42);
        assert_eq!(page.items[0].title, "Clone tasks");
        assert_eq!(short_date(&page.items[0].updated_at), "2026-07-28");
        assert_eq!(page.total_pages, 20);
    }

    #[test]
    fn github_search_query_adds_only_the_missing_kind_and_repository_qualifiers() {
        assert_eq!(
            github_search_query(TaskKind::Issues, "assignee:@me is:issue", "acme/repo"),
            "assignee:@me is:issue repo:acme/repo"
        );
        assert_eq!(
            github_search_query(TaskKind::PullRequests, "author:@me", "acme/repo"),
            "author:@me is:pr repo:acme/repo"
        );
    }

    #[test]
    fn parses_and_filters_github_projects() {
        let items = parse_projects(
            r#"{"projects":[{"number":7,"title":"Rust clone","closed":false,"updatedAt":"2026-07-29T01:02:03Z","url":"https://github.com/users/example/projects/7"},{"number":8,"title":"Website","closed":true,"url":"https://github.com/users/example/projects/8"}],"totalCount":2}"#,
            "rust",
        )
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 7);
        assert_eq!(items[0].title, "Rust clone");
        assert_eq!(items[0].state, "OPEN");
    }

    #[test]
    fn parses_gitlab_issue_and_merge_request_shapes() {
        let items = parse_gitlab_work_items(
            r#"[{"iid":17,"title":"Port GitLab tasks","state":"opened","updated_at":"2026-07-29T02:03:04Z","web_url":"https://gitlab.com/stably/orca/-/issues/17"},{"iid":18,"title":"Port MR list","state":"merged","updatedAt":"2026-07-29T03:04:05Z","webUrl":"https://gitlab.com/stably/orca/-/merge_requests/18"}]"#,
        )
        .unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].number, 17);
        assert_eq!(items[0].state, "OPENED");
        assert_eq!(items[1].number, 18);
        assert_eq!(short_date(&items[1].updated_at), "2026-07-29");
    }

    #[test]
    fn parses_and_filters_gitlab_todos() {
        let items = parse_gitlab_todos(
            r#"[{"action_name":"assigned","created_at":"2026-07-29T01:02:03Z","target":{"id":99,"iid":21,"title":"Finish the Rust clone","state":"opened","web_url":"https://gitlab.com/stably/orca/-/issues/21"}},{"action_name":"mentioned","target":{"id":100,"title":"Unrelated task","web_url":"https://gitlab.com/stably/orca/-/issues/22"}}]"#,
            "rust clone",
        )
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 21);
        assert_eq!(items[0].state, "ASSIGNED");
        assert_eq!(items[0].title, "Finish the Rust clone");
    }
}
