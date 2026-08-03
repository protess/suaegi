use iced::widget::{
    button, checkbox, column, container, mouse_area, pick_list, row, scrollable, text_input, Space,
};
use iced::{Alignment, Color, Element, Length};

use suaegi_core::domain::{Repo, RepoId};
#[cfg(test)]
use suaegi_forge::{ChecksSummary, ReviewState};
use suaegi_git::worktree::WorktreeEntry;

/// 사이드바 create 입력의 **안정적 위젯 id**. 이 입력에 `operation::focus`를 걸면
/// 비매칭 focusable(터미널 pane)이 전부 unfocus된다 — 사이드바에 타이핑할 때 키가
/// 활성 터미널로 새지 않게 하는 상호배타의 열쇠다(포커스된 터미널이 그대로 남아
/// 키를 먹던 버그의 수정).
pub fn name_input_id(repo_id: &RepoId) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("suaegi-wt-name-{}", repo_id.0))
}
pub fn prompt_input_id(repo_id: &RepoId) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("suaegi-wt-prompt-{}", repo_id.0))
}
use suaegi_term::presence::AgentPresence;

use crate::agent_status::contract::BadgeState;
#[cfg(test)]
use crate::forge_ui::{self, PrIndicator};
use crate::i18n::text;
use crate::icons::{self, Icon};
use crate::persistence_thread::{LoadOrigin, SaveStatus};
use crate::state::{worktree_id_for, AppState, CreatePrDraft, JiraState, LinearState, Message};
use crate::theme;
use crate::tracker_ui::{self, IssueListView, JiraIssueListView};
use suaegi_core::domain::WorktreeId;

// PR 표시자 색. 배지와 같은 팔레트 계열이되(사이드바 톤 통일) 상태를 색으로 구별한다.
const PR_NEUTRAL: Color = Color::from_rgb(0.53, 0.53, 0.53);
const PR_OPEN: Color = Color::from_rgb(0.18, 0.63, 0.26);
#[cfg(test)]
const PR_MERGED: Color = Color::from_rgb(0.52, 0.34, 0.72);
const PR_CLOSED: Color = Color::from_rgb(0.75, 0.22, 0.17);
#[cfg(test)]
const PR_UNKNOWN: Color = Color::from_rgb(0.85, 0.55, 0.0);
// Linear 링크/이슈 색. PR 팔레트와 구별되는 보라 계열(트래커 vs forge).
const LINEAR_LINK: Color = Color::from_rgb(0.42, 0.45, 0.85);
// 이슈 조회 실패 색. **"no issues"와 시각적으로 구별**한다 — Unavailable≠none의 UI 계약.
const LINEAR_UNAVAILABLE: Color = Color::from_rgb(0.85, 0.55, 0.0);
// Jira 링크/이슈 색. Linear(보라)와도 구별되는 파랑 계열(provider 구분).
const JIRA_LINK: Color = Color::from_rgb(0.16, 0.52, 0.86);
// Jira 이슈 조회 실패 색. Linear와 같은 주황(둘 다 "실패"의 시각 언어). **"no issues"와는 구별.**
const JIRA_UNAVAILABLE: Color = Color::from_rgb(0.85, 0.55, 0.0);

/// 사이드바 고정 폭. `pane_grid`는 고정 폭 pane이 없고(비율 분할만) 사이드바가
/// 터미널 격자 한가운데로 드래그될 수 있으므로, 사이드바는 pane이 아니라 상위
/// `row!` 레이아웃에서 이 폭으로 못박은 별도 위젯이다.
pub const WIDTH: f32 = 207.0;
const CONTEXT_WIDTH: f32 = 300.0;

pub fn view(state: &AppState) -> Element<'_, Message> {
    let automation_active = state.automation_ui().open;
    let activity_active = state.activity_open();
    let tasks_active = !automation_active && !activity_active && state.tasks_open();
    let mut nav = column![button(
        row![
            icons::view(Icon::ListChecks, 13.0, theme::MUTED),
            text("Onboarding checklist").size(14)
        ]
        .spacing(7)
    )
    .on_press(Message::OnboardingOpened)
    .width(Length::Fill)
    .padding([5, 6])
    .style(theme::ghost_button),]
    .spacing(1);
    if state.ui_settings().show_tasks_button {
        nav = nav.push(
            button(
                row![
                    icons::view(Icon::ClipboardList, 13.0, theme::MUTED),
                    text("Tasks").size(14)
                ]
                .spacing(7),
            )
            .on_press(Message::TasksOpened)
            .width(Length::Fill)
            .padding([5, 6])
            .style(if tasks_active {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
        );
    }
    if state.ui_settings().show_automations_button {
        nav = nav.push(
            button(
                row![
                    icons::view(Icon::CalendarClock, 13.0, theme::MUTED),
                    text("Automations").size(14)
                ]
                .spacing(7),
            )
            .on_press(Message::AutomationOpened)
            .width(Length::Fill)
            .padding([5, 6])
            .style(if automation_active {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
        );
    }
    if state.ui_settings().experimental_activity {
        nav = nav.push(
            button(
                row![
                    icons::view(Icon::ListChecks, 13.0, theme::MUTED),
                    text("Activity").size(14)
                ]
                .spacing(7),
            )
            .on_press(Message::ActivityOpened)
            .width(Length::Fill)
            .padding([5, 6])
            .style(if activity_active {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
        );
    }
    nav = nav.push(
        button(
            row![
                icons::view(Icon::Search, 13.0, theme::MUTED),
                text("Search").size(14).width(Length::Fill)
            ]
            .spacing(7),
        )
        .on_press(Message::WorkspaceSearchRequested)
        .width(Length::Fill)
        .padding([5, 6])
        .style(theme::search_button),
    );

    let workspace_header = row![
        text("Projects")
            .size(13)
            .color(theme::MUTED)
            .width(Length::Fill),
        button(text("☷").size(13))
            .on_press(Message::WorkspaceOptionsToggled)
            .padding([3, 5])
            .style(if state.workspace_options_open() {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
        button(if state.is_adding_repo() { "×" } else { "▣" })
            .on_press(Message::RepoAddToggled)
            .padding([3, 5])
            .style(theme::ghost_button),
        button(text("+").size(17))
            .on_press_maybe(
                state
                    .repos()
                    .first()
                    .map(|repo| Message::WorktreeCreateToggled(repo.id.clone())),
            )
            .padding([1, 5])
            .style(theme::ghost_button),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    let mut workspace_tools = column![workspace_header].spacing(8);
    if state.is_adding_repo() {
        workspace_tools = workspace_tools.push(
            container(add_repo_row(state))
                .padding(6)
                .width(Length::Fill)
                .style(theme::active_card),
        );
    }

    let mut workspaces = column![].spacing(2);
    let pinned = state
        .repos()
        .iter()
        .flat_map(|repo| state.worktrees_for(&repo.id).iter())
        .filter(|entry| {
            state.worktree_is_visible(entry)
                && state.worktree_is_pinned(&worktree_id_for(&entry.path))
        })
        .collect::<Vec<_>>();
    if !pinned.is_empty() {
        let mut pinned_rows = column![text("Pinned").size(11).color(theme::MUTED)].spacing(2);
        for entry in pinned {
            pinned_rows = pinned_rows.push(worktree_entry(state, entry));
        }
        workspaces = workspaces.push(
            container(pinned_rows)
                .width(Length::Fill)
                .padding([5, 2])
                .style(theme::configured_sidebar(state.ui_settings())),
        );
    }
    for group in grouped_worktrees(state) {
        workspaces = workspaces.push(repo_group(state, &group));
    }

    let integrations = button(icons::view(Icon::Settings, 13.0, theme::MUTED))
        .on_press(Message::IntegrationsToggled)
        .padding([3, 6])
        .style(if state.integrations_open() {
            theme::selected_button
        } else {
            theme::ghost_button
        });

    let mut footer = column![row![
        integrations,
        button(icons::view(Icon::CircleHelp, 13.0, theme::MUTED))
            .on_press(Message::HelpToggled)
            .padding([3, 6])
            .style(if state.help_open() {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
        Space::new().width(Length::Fill),
        button(text("⌖").size(14).color(theme::MUTED))
            .on_press(Message::RevealActiveWorkspace)
            .padding([3, 5])
            .style(theme::ghost_button),
        button(text("▦").size(14).color(theme::MUTED))
            .on_press(Message::WorkspaceBoardToggled)
            .padding([3, 5])
            .style(if state.workspace_board_open() {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
    ]
    .spacing(3)
    .align_y(Alignment::Center)]
    .spacing(4);
    if let Some(error) = state.last_error() {
        footer = footer.push(text(format!("! {error}")).size(14));
    }
    if let Some(status) = status_line(state) {
        footer = footer.push(text(status).size(14));
    }
    if state.future_schema_guarded() {
        let controls: Element<'static, Message> = if state.future_schema_override_confirming() {
            row![
                button(text("Cancel").size(11))
                    .on_press(Message::FutureSchemaOverrideCancelled)
                    .padding([3, 6])
                    .style(theme::ghost_button),
                button(text("Back up & replace").size(11))
                    .on_press(Message::FutureSchemaOverrideConfirmed)
                    .padding([3, 6])
                    .style(theme::danger_ghost_button),
            ]
            .spacing(3)
            .into()
        } else {
            button(text("Review save options…").size(11))
                .on_press(Message::FutureSchemaOverrideRequested)
                .padding([3, 6])
                .style(theme::ghost_button)
                .into()
        };
        footer = footer.push(controls);
    }

    let layout = column![
        container(nav).padding([4, 7]),
        container(workspace_tools).padding([7, 8]),
        scrollable(container(workspaces).padding([0, 5])).height(Length::Fill),
        container(footer)
            .padding([5, 7])
            .style(theme::configured_sidebar_top_bar(state.ui_settings())),
    ]
    .height(Length::Fill);

    container(layout)
        .width(Length::Fixed(WIDTH))
        .height(Length::Fill)
        .style(theme::configured_sidebar(state.ui_settings()))
        .into()
}

/// 연결 설정은 좁은 탐색 메뉴가 아니라 하나의 문맥 패널을 사용한다. 긴 Jira URL,
/// 이메일, 토큰 입력이 작업공간 이름을 밀어내거나 사이드바를 가로로 넘치지 않는다.
pub fn integrations_view(state: &AppState) -> Option<Element<'_, Message>> {
    if !state.integrations_open() {
        return None;
    }

    let header = row![
        text("Integrations").size(17).width(Length::Fill),
        button("×")
            .on_press(Message::IntegrationsToggled)
            .style(theme::ghost_button),
    ]
    .align_y(Alignment::Center);
    let providers = column![linear_panel(state), jira_panel(state)].spacing(10);
    let body = column![header, scrollable(providers).height(Length::Fill)]
        .spacing(10)
        .padding(12);

    Some(
        container(body)
            .width(Length::Fixed(CONTEXT_WIDTH))
            .height(Length::Fill)
            .style(theme::context_panel)
            .into(),
    )
}

/// PR 생성 역시 우측 문맥 슬롯을 사용한다. 폼이 worktree 목록 사이에 끼어들지
/// 않으므로 사용자가 무엇을 대상으로 작업 중인지 계속 볼 수 있다.
pub fn create_pr_view(state: &AppState) -> Option<Element<'_, Message>> {
    let dialog = state.create_pr_dialog()?;
    Some(
        container(create_pr_form(state, dialog))
            .width(Length::Fixed(CONTEXT_WIDTH))
            .height(Length::Fill)
            .padding(4)
            .style(theme::context_panel)
            .into(),
    )
}

fn add_repo_row(state: &AppState) -> Element<'_, Message> {
    let value = state.repo_path_input();
    row![
        text_input("/path/to/repo", value)
            .on_input(Message::RepoPathInputChanged)
            .on_submit(Message::AddRepoSubmitted)
            .width(Length::Fill),
        button("Browse")
            .on_press(Message::RepoBrowseRequested)
            .padding([5, 7])
            .style(theme::ghost_button),
        button("Add")
            .on_press_maybe((!value.trim().is_empty()).then_some(Message::AddRepoSubmitted)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

struct RepoGroup<'a> {
    repo: &'a Repo,
    worktrees: Vec<&'a WorktreeEntry>,
}

/// 뷰가 그리는 repo → worktree 그룹. `state.repos()`의 등록 순서를 그대로
/// 따르므로 (HashMap 반복 순서가 아니므로) 프레임마다 순서가 흔들리지 않는다.
/// 삭제된 repo를 가리키는 worktree 항목은 이 repo 목록을 기준으로 순회하는
/// 이상 애초에 방문되지 않는다 — 패닉 없이 조용히 빠진다.
fn grouped_worktrees(state: &AppState) -> Vec<RepoGroup<'_>> {
    state
        .repos()
        .iter()
        .map(|repo| RepoGroup {
            repo,
            worktrees: state
                .worktrees_for(&repo.id)
                .iter()
                .filter(|entry| {
                    state.worktree_is_visible(entry)
                        && (state.ui_settings().show_pinned_worktrees_in_groups
                            || !state.worktree_is_pinned(&worktree_id_for(&entry.path)))
                })
                .collect(),
        })
        .collect()
}

fn repo_group<'a>(state: &'a AppState, group: &RepoGroup<'a>) -> Element<'a, Message> {
    let repo_id = group.repo.id.clone();
    let draft = state.worktree_name_draft(&repo_id);

    let create_toggle_id = repo_id.clone();
    let create_is_open = state.is_creating_worktree_for(&repo_id);
    let actions_id = repo_id.clone();
    let badge_color = state
        .ui_settings()
        .repo_badge_colors
        .get(&repo_id.0)
        .and_then(|value| theme::parse_hex(value))
        .unwrap_or(theme::MUTED);
    let project_icon: Element<'_, Message> = state
        .ui_settings()
        .repo_icons
        .get(&repo_id.0)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || icons::view(Icon::GitBranch, 11.0, badge_color).into(),
            |value| text(value.clone()).size(13).color(badge_color).into(),
        );
    let header = row![
        project_icon,
        text(group.repo.display_name.clone())
            .size(14)
            .width(Length::Fill),
        button(icons::view(Icon::Ellipsis, 12.0, theme::MUTED))
            .on_press(Message::ProjectActionsToggled(actions_id))
            .padding([2, 4])
            .style(if state.project_actions_open() == Some(&repo_id) {
                theme::selected_button
            } else {
                theme::ghost_button
            }),
        button(if create_is_open { "×" } else { "+" })
            .on_press(Message::WorktreeCreateToggled(create_toggle_id))
            .padding([2, 5])
            .style(theme::ghost_button),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let repo_id_for_input = repo_id.clone();
    let repo_id_for_submit = repo_id.clone();
    let repo_id_for_button = repo_id.clone();
    let name_row = row![
        text_input("workspace name (optional)", draft)
            .id(name_input_id(&repo_id))
            .on_input(move |value| Message::WorktreeNameInputChanged {
                repo_id: repo_id_for_input.clone(),
                value,
            })
            .on_submit(Message::CreateWorktreeSubmitted {
                repo_id: repo_id_for_submit.clone()
            })
            .width(Length::Fill),
        button("+ worktree").on_press(Message::CreateWorktreeSubmitted {
            repo_id: repo_id_for_button.clone(),
        }),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    // 에이전트 피커. 옵션은 로그인 셸(기본) + **설치된** 에이전트만 — 목록에
    // 있으면 곧 설치돼 있다는 뜻이라 고른 게 exec 실패로 이어지지 않는다. 기본
    // 선택이 "Login shell"이라 피커를 무시하면 오늘의 동작 그대로다.
    let repo_id_for_agent = repo_id.clone();
    let agent_picker = pick_list(
        state.agent_picker_choices(),
        Some(state.worktree_agent_selection(&repo_id)),
        move |choice| Message::WorktreeAgentSelected {
            repo_id: repo_id_for_agent.clone(),
            choice,
        },
    )
    .width(Length::Fill)
    .text_size(12);

    // 선택적 초기 프롬프트. 비워두면 주입 없음(기본). 채우면 새 worktree의 첫
    // 세션에 한 번 실린다 — argv/flag 에이전트는 스폰 인자로, stdin-after-start
    // 에이전트는 composer 준비 후 PTY로. **영속화하지 않는다**(일회성 launch 인자).
    let repo_id_for_prompt = repo_id.clone();
    let repo_id_for_prompt_submit = repo_id.clone();
    let prompt_input = text_input(
        "initial prompt (optional)",
        state.worktree_prompt_draft(&repo_id),
    )
    .id(prompt_input_id(&repo_id))
    .on_input(move |value| Message::WorktreePromptInputChanged {
        repo_id: repo_id_for_prompt.clone(),
        value,
    })
    .on_submit(Message::CreateWorktreeSubmitted {
        repo_id: repo_id_for_prompt_submit.clone(),
    })
    .width(Length::Fill)
    .size(14);
    let name_hint = text(format!(
        "Blank name uses the prompt or “{}”.",
        state.worktree_suggested_name(&repo_id)
    ))
    .size(11)
    .color(theme::MUTED);
    let setup_choice: Element<'_, Message> = if state.repo_has_setup_script(&repo_id) {
        let run = state.worktree_setup_run_selection(&repo_id);
        let setup_repo = repo_id.clone();
        row![
            column![
                text("Workspace setup script").size(11),
                text(if run {
                    "Runs after this worktree is created."
                } else {
                    "Skipped for this worktree."
                })
                .size(10)
                .color(theme::MUTED),
            ]
            .spacing(1)
            .width(Length::Fill),
            button(text(if run { "✓ Run setup" } else { "Skip setup" }).size(10))
                .on_press(Message::WorktreeSetupRunToggled(setup_repo))
                .padding([4, 7])
                .style(if run {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
        ]
        .align_y(Alignment::Center)
        .into()
    } else {
        Space::new().height(0).into()
    };
    let sparse_choices = state.sparse_preset_choices(&repo_id);
    let sparse_choice: Element<'_, Message> = if sparse_choices.len() > 1 {
        let selected = state.selected_sparse_preset_choice(&repo_id);
        let sparse_repo = repo_id.clone();
        row![
            column![
                text("Sparse checkout").size(11),
                text("Limit this workspace to a saved directory set.")
                    .size(10)
                    .color(theme::MUTED),
            ]
            .spacing(1)
            .width(Length::Fill),
            pick_list(sparse_choices, Some(selected), move |choice| {
                Message::WorktreeSparsePresetSelected(sparse_repo.clone(), choice.id)
            })
            .width(Length::Fixed(150.0))
            .text_size(10),
        ]
        .align_y(Alignment::Center)
        .into()
    } else {
        Space::new().height(0).into()
    };
    let vm_recipe_choices = state.vm_recipe_choices(&repo_id);
    let vm_recipe_choice: Element<'_, Message> =
        if state.ui_settings().experimental_ephemeral_vms && vm_recipe_choices.len() > 1 {
            let selected = state.selected_vm_recipe_choice(&repo_id);
            let recipe_repo = repo_id.clone();
            row![
                column![
                    text("Run workspace on").size(11),
                    text("Choose local or a per-workspace environment recipe.")
                        .size(10)
                        .color(theme::MUTED),
                ]
                .spacing(1)
                .width(Length::Fill),
                pick_list(vm_recipe_choices, Some(selected), move |choice| {
                    Message::WorktreeVmRecipeSelected(recipe_repo.clone(), choice.id)
                })
                .width(Length::Fixed(190.0))
                .text_size(10),
            ]
            .align_y(Alignment::Center)
            .into()
        } else {
            Space::new().height(0).into()
        };
    let create_row = column![
        agent_picker,
        prompt_input,
        setup_choice,
        sparse_choice,
        vm_recipe_choice,
        name_row,
        name_hint
    ]
    .spacing(6);

    let mut rows = column![container(header).padding([2, 5])].spacing(2);
    if create_is_open {
        rows = rows.push(
            container(create_row)
                .padding(6)
                .width(Length::Fill)
                .style(theme::active_card),
        );
    }
    if let Some(inbox) = external_worktree_inbox(state, group.repo) {
        rows = rows.push(inbox);
    }

    for entry in &group.worktrees {
        rows = rows.push(worktree_entry(state, entry));
    }

    container(rows)
        .width(Length::Fill)
        .padding([6, 2])
        .style(theme::configured_sidebar(state.ui_settings()))
        .into()
}

fn external_worktree_inbox<'a>(
    state: &'a AppState,
    repo: &'a Repo,
) -> Option<Element<'a, Message>> {
    let candidates = state.external_worktree_inbox(&repo.id);
    if candidates.is_empty() {
        return None;
    }
    let count = candidates.len();
    let mut entries = column![text("These worktrees were created outside of Suaegi.")
        .size(10)
        .color(theme::MUTED)]
    .spacing(3);
    for entry in candidates {
        let import_repo = repo.id.clone();
        let worktree = worktree_id_for(&entry.path);
        let name = entry
            .branch
            .clone()
            .unwrap_or_else(|| entry.path.to_string_lossy().into_owned());
        entries = entries.push(
            row![
                column![
                    text(name).size(11),
                    text(entry.path.to_string_lossy().into_owned())
                        .size(9)
                        .color(theme::MUTED),
                ]
                .spacing(1)
                .width(Length::Fill),
                button(text("Import").size(9))
                    .on_press(Message::RepoExternalWorktreeImported(import_repo, worktree,))
                    .padding([3, 6])
                    .style(theme::ghost_button),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        );
    }
    let import_all_repo = repo.id.clone();
    let keep_hidden_repo = repo.id.clone();
    let suppress_repo = repo.id.clone();
    entries = entries.push(
        row![
            button(text("Keep hidden").size(9))
                .on_press(Message::RepoExternalWorktreesKeptHidden(keep_hidden_repo))
                .padding([3, 6])
                .style(theme::ghost_button),
            button(text("Import all").size(9))
                .on_press(Message::RepoExternalWorktreesImportedAll(import_all_repo))
                .padding([3, 6])
                .style(theme::ghost_button),
            button(text("Don't show again").size(9))
                .on_press(Message::RepoExternalWorktreeDiscoverySuppressed(
                    suppress_repo
                ))
                .padding([3, 6])
                .style(theme::ghost_button),
        ]
        .spacing(4),
    );
    Some(
        container(
            column![
                text(format!("New externally-created worktrees  {count}"))
                    .size(11)
                    .color(theme::MUTED),
                entries,
            ]
            .spacing(4),
        )
        .padding([5, 8])
        .style(theme::active_card)
        .into(),
    )
}

fn worktree_entry<'a>(state: &'a AppState, entry: &'a WorktreeEntry) -> Element<'a, Message> {
    let worktree_id = worktree_id_for(&entry.path);
    let is_selected = state.selected_worktree() == Some(&worktree_id);
    let mut rows = column![worktree_row(
        entry,
        WorktreeRowState {
            is_selected,
            actions_open: state.worktree_actions_open() == Some(&worktree_id),
            is_pinned: state.worktree_is_pinned(&worktree_id),
            is_unread: state.worktree_is_unread(&worktree_id),
            is_sleeping: state.worktree_is_sleeping(&worktree_id),
            is_dragging: state.worktree_is_dragging(&worktree_id),
            badge: state.worktree_badge(&worktree_id),
            presence: state.worktree_presence(&worktree_id),
            prompt_cache_seconds: state.prompt_cache_remaining_seconds(&worktree_id),
            compact: state.ui_settings().compact_worktree_cards,
            display_name: state.worktree_display_name(&worktree_id, entry),
        },
    )]
    .spacing(1);
    if is_selected {
        if let Some(comment) = state.worktree_comment(&worktree_id) {
            rows = rows.push(text(format!("  {comment}")).size(11).color(theme::MUTED));
        }
        if let Some(issue) = state.linked_linear_issue(&worktree_id) {
            rows = rows.push(text(format!("  ⌁ {issue}")).size(13).color(LINEAR_LINK));
        }
        if let Some(issue) = state.linked_jira_issue(&worktree_id) {
            rows = rows.push(text(format!("  ◈ {issue}")).size(13).color(JIRA_LINK));
        }
    }
    rows.into()
}

/// 에이전트 상태 배지. **`Unknown`은 `Working`과 시각적으로 구별한다** — "모른다"와
/// "바쁘다"는 다른 상태이고, 사용자가 그 둘을 구별할 수 있어야 한다. 같은 글리프를
/// 옅게만 쓰면 색 대비가 약한 화면에서 구별이 사라지므로 **글리프도 색도** 다르다.
///
/// **오류 스타일링만 `AgentPresence`를 직접 읽는다.** `BadgeState`에는 일부러 오류
/// 변형이 없다 — 리듀서 반환에 변형을 더하면 배지 상태와 프로세스 사실이 두 곳에서
/// 관리된다. 리듀서는 "무슨 상태인가"만 답하고, "어떻게 끝났는가"는 여기서 본다.
///
/// `Element`는 직접 검사할 수 없으므로 매핑 자체를 순수 함수로 뽑아 테스트한다.
fn badge_glyph(badge: BadgeState, presence: AgentPresence) -> (&'static str, Color) {
    // 0이 아닌 종료 코드는 상태와 무관하게 오류로 보여야 한다.
    if let AgentPresence::Exited { code } = presence {
        if code != 0 {
            return ("×", Color::from_rgb8(0xc0, 0x39, 0x2b));
        }
    }
    match badge {
        BadgeState::Working => ("●", Color::from_rgb8(0x2e, 0xa0, 0x43)),
        // 사람을 기다린다 — 이 플랜에서 사용자가 가장 알고 싶은 상태다.
        BadgeState::Waiting => ("◆", Color::from_rgb8(0xd8, 0x8c, 0x00)),
        BadgeState::Done => ("○", Color::from_rgb8(0x88, 0x88, 0x88)),
        // 글리프와 색이 **둘 다** Working과 다르다.
        BadgeState::Unknown => ("·", Color::from_rgb8(0xbb, 0xbb, 0xbb)),
    }
}

fn presence_badge(badge: BadgeState, presence: AgentPresence) -> Element<'static, Message> {
    let (label, color) = badge_glyph(badge, presence);
    container(text(label).size(12).color(color))
        .width(Length::Fixed(10.0))
        .height(Length::Fixed(10.0))
        .into()
}

/// git이 non-forced `worktree remove`로 main 체크아웃을 항상 거부하므로
/// 지우는 버튼을 눌러도 안전은 하지만, 애초에 버튼을 안 보여주는 게 낫다 —
/// 눌러도 아무 일도 안 일어나는 죽은 버튼보다 명확하다.
#[cfg(test)]
fn worktree_is_removable(entry: &WorktreeEntry) -> bool {
    !entry.is_main
}

struct WorktreeRowState {
    is_selected: bool,
    actions_open: bool,
    is_pinned: bool,
    is_unread: bool,
    is_sleeping: bool,
    is_dragging: bool,
    badge: BadgeState,
    presence: AgentPresence,
    prompt_cache_seconds: Option<u64>,
    compact: bool,
    display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorktreeRowMetrics {
    label_size: u32,
    detail_size: u32,
    content_padding: [u16; 2],
    show_detail: bool,
}

fn worktree_row_metrics(compact: bool) -> WorktreeRowMetrics {
    if compact {
        WorktreeRowMetrics {
            label_size: 13,
            detail_size: 11,
            content_padding: [1, 3],
            show_detail: false,
        }
    } else {
        WorktreeRowMetrics {
            label_size: 14,
            detail_size: 12,
            content_padding: [3, 4],
            show_detail: true,
        }
    }
}

fn worktree_row(entry: &WorktreeEntry, state: WorktreeRowState) -> Element<'static, Message> {
    let worktree_id = worktree_id_for(&entry.path);
    let label = state.display_name;
    let metrics = worktree_row_metrics(state.compact);

    // Selection already has a row background. Replacing the badge with a green selection
    // dot hid Claude's Waiting/Done state on the pane the user was actually looking at.
    // Orca keeps the status glyph authoritative even for the selected worktree.
    let status: Element<'static, Message> = presence_badge(state.badge, state.presence);
    let mut identity = row![
        status,
        text(label.clone()).size(metrics.label_size),
        Space::new().width(Length::Fill),
    ]
    .spacing(5)
    .align_y(Alignment::Center);
    if state.is_unread {
        identity = identity.push(text("●").size(9).color(Color::from_rgb8(0x4f, 0x7f, 0xff)));
    }
    if state.is_pinned {
        identity = identity.push(text("⚑").size(10).color(theme::MUTED));
    }
    if state.is_sleeping {
        identity = identity.push(text("z").size(10).color(theme::MUTED));
    }
    if let Some(seconds) = state.prompt_cache_seconds {
        identity = identity.push(
            container(
                text(format!("◷ {}:{:02}", seconds / 60, seconds % 60))
                    .size(10)
                    .color(theme::MUTED),
            )
            .padding([1, 4])
            .style(theme::chip),
        );
    }
    let identity = if entry.is_main {
        identity.push(
            container(text("primary").size(11).color(theme::MUTED))
                .padding([2, 5])
                .style(theme::chip),
        )
    } else {
        identity
    };
    let content: Element<'static, Message> = if metrics.show_detail {
        column![
            identity,
            text(label).size(metrics.detail_size).color(theme::MUTED),
        ]
        .spacing(1)
        .into()
    } else {
        identity.into()
    };

    let select_id = worktree_id.clone();
    let hover_id = worktree_id.clone();
    let drag_id = worktree_id.clone();
    let actions: Element<'static, Message> = if state.is_selected || state.actions_open {
        button(icons::view(Icon::Ellipsis, 11.0, theme::MUTED))
            .on_press(Message::WorktreeActionsToggled(worktree_id))
            .padding([8, 4])
            .style(theme::selected_button)
            .into()
    } else {
        Space::new()
            .width(Length::Fixed(19.0))
            .height(Length::Fixed(27.0))
            .into()
    };

    mouse_area(
        row![
            mouse_area(
                container(text("⠿").size(9).color(theme::MUTED))
                    .padding([8, 2])
                    .style(if state.is_dragging {
                        theme::active_card
                    } else {
                        theme::card
                    }),
            )
            .on_press(Message::WorktreeDragStarted(drag_id)),
            button(container(content).padding(metrics.content_padding))
                .on_press(Message::WorktreeSelected(select_id))
                .padding(0)
                .width(Length::Fill)
                .style(if state.is_selected {
                    theme::selected_button
                } else {
                    theme::ghost_button
                }),
            actions,
        ]
        .spacing(1)
        .align_y(Alignment::Center),
    )
    .on_enter(Message::WorktreeDragHovered(hover_id))
    .into()
}

/// PR 표시자 → (문구, 색). `None` = 아무것도 안 그린다(Hidden). **`Unknown`은
/// `NoPr`와 문구·색이 모두 다르다** — "상태 모름"이 "PR 없음"으로 보이면 안 된다.
#[cfg(test)]
fn pr_indicator_label(indicator: &PrIndicator) -> Option<(String, Color)> {
    match indicator {
        PrIndicator::Hidden => None,
        PrIndicator::Checking => Some(("PR …".to_string(), PR_NEUTRAL)),
        PrIndicator::NoPr => Some(("no PR".to_string(), PR_NEUTRAL)),
        PrIndicator::Present {
            number,
            state,
            checks,
        } => {
            let mut label = format!("PR #{number} {}", pr_state_text(*state));
            if let Some(summary) = checks_text(*checks) {
                label.push(' ');
                label.push_str(&summary);
            }
            Some((label, pr_state_color(*state)))
        }
        // 실행 가능한 힌트를 붙인다(NotAuthenticated → "run gh auth login" 등).
        PrIndicator::Unknown(reason) => Some((
            format!("PR ? {}", forge_ui::unavailable_text(reason)),
            PR_UNKNOWN,
        )),
    }
}

#[cfg(test)]
fn pr_state_text(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Open => "open",
        ReviewState::Merged => "merged",
        ReviewState::Closed => "closed",
        ReviewState::Draft => "draft",
    }
}

#[cfg(test)]
fn pr_state_color(state: ReviewState) -> Color {
    match state {
        ReviewState::Open => PR_OPEN,
        ReviewState::Merged => PR_MERGED,
        ReviewState::Closed => PR_CLOSED,
        ReviewState::Draft => PR_NEUTRAL,
    }
}

/// CI 체크 요약 "✓passing ✗failing •pending". 체크가 하나도 없으면 `None`(조용).
#[cfg(test)]
fn checks_text(checks: ChecksSummary) -> Option<String> {
    if checks.passing == 0 && checks.failing == 0 && checks.pending == 0 {
        return None;
    }
    Some(format!(
        "✓{} ✗{} •{}",
        checks.passing, checks.failing, checks.pending
    ))
}

/// Create-PR 다이얼로그 폼. **픽셀·상호작용은 사람 눈**이다 — 자격 게이팅과 성공 시
/// 링크 영속화는 `state`/`forge_ui`에서 검사한다.
fn create_pr_form<'a>(state: &'a AppState, dialog: &'a CreatePrDraft) -> Element<'a, Message> {
    let mut form = column![
        text("Create hosted review").size(16),
        text_input("title", &dialog.title)
            .on_input(Message::CreatePrTitleChanged)
            .size(14),
        text_input("base branch", &dialog.base)
            .on_input(Message::CreatePrBaseChanged)
            .size(14),
        text_input("description", &dialog.body)
            .on_input(Message::CreatePrBodyChanged)
            .size(14),
        checkbox(dialog.draft)
            .label("Draft")
            .on_toggle(Message::CreatePrDraftToggled)
            .text_size(12),
        checkbox(dialog.use_template)
            .label("Use repository PR/MR template")
            .on_toggle(Message::CreatePrUseTemplateToggled)
            .text_size(12),
    ]
    .spacing(6);

    if let Some(err) = &dialog.error {
        form = form.push(text(format!("! {err}")).size(13).color(PR_CLOSED));
    }

    // 제출 중이면 버튼을 잠근다(중복 제출 방지).
    let submit_label = if dialog.submitting {
        "Creating…"
    } else {
        "Create"
    };
    let submit = button(text(submit_label).size(14)).on_press_maybe(
        (!dialog.submitting && !dialog.generating).then_some(Message::CreatePrSubmitted),
    );
    let generate = button(
        text(if dialog.generating {
            "Generating…"
        } else {
            "Generate details"
        })
        .size(14),
    )
    .on_press_maybe(
        (state.ui_settings().source_control_ai.enabled && !dialog.submitting && !dialog.generating)
            .then_some(Message::CreatePrGenerateDetailsRequested),
    );
    let cancel = button(text("Cancel").size(14)).on_press(Message::CreatePrCancelled);
    form = form.push(row![submit, generate, cancel].spacing(6));

    container(form)
        .padding(8)
        .width(Length::Fill)
        .style(theme::context_panel)
        .into()
}

/// N1: Linear 패널. 미연결이면 **마스킹된** 키 입력 + Connect, 연결 중이면 진행 표시,
/// 연결됐으면 워크스페이스(org) 이름 + 이슈 목록을 그린다.
///
/// **픽셀·상호작용은 사람 눈으로 본다.** 검사되는 결정은 `tracker_ui`가 값으로 뽑는다:
/// 연결 결과 매핑(성공/실패), 그리고 crux인 **Unavailable≠no issues**(이슈 목록 매핑).
pub(crate) fn linear_panel(state: &AppState) -> Element<'_, Message> {
    let linear: &LinearState = state.linear();
    let selected = state.selected_worktree().cloned();

    let mut panel = column![text("Linear").size(16)].spacing(6);

    match &linear.workspace {
        // 연결됨: org 이름 + 이슈 새로고침 + 이슈 목록.
        Some(ws) => {
            panel = panel.push(
                row![
                    text(format!("● {}", ws.name)).size(14).color(PR_OPEN),
                    button(text("↻ issues").size(13))
                        .on_press(Message::LinearIssuesRefreshRequested),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
            panel = panel.push(linear_issue_list(linear, selected.as_ref()));
        }
        // 미연결(또는 연결 실패): 키 입력 + Connect. 연결 중이면 버튼을 잠근다.
        None => {
            // **마스킹된 secure 입력** — 키가 화면에 평문으로 안 뜬다(로그/Debug에도 안 샌다).
            panel = panel.push(
                text_input("Linear API key", &linear.api_key_input)
                    .secure(true)
                    .on_input(|value| Message::LinearApiKeyChanged(crate::SecretDraft::new(value)))
                    .on_submit(Message::LinearConnectSubmitted)
                    .size(14),
            );
            let connect_label = if linear.connecting {
                "Connecting…"
            } else {
                "Connect"
            };
            panel =
                panel.push(button(text(connect_label).size(14)).on_press_maybe(
                    (!linear.connecting).then_some(Message::LinearConnectSubmitted),
                ));
            if let Some(err) = &linear.connect_error {
                panel = panel.push(text(format!("! {err}")).size(13).color(PR_CLOSED));
            }
        }
    }

    container(panel)
        .padding(10)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// 이슈 목록 렌더. **`tracker_ui::issue_list`가 Unavailable≠no issues를 정하고**, 여기선
/// 그 값을 픽셀로 옮길 뿐이다(사람 눈). 링크 버튼은 **선택된** worktree를 이 이슈에 링크한다.
fn linear_issue_list(
    linear: &LinearState,
    selected: Option<&WorktreeId>,
) -> Element<'static, Message> {
    if linear.issues_loading && linear.issues.is_none() {
        return text("loading issues…").size(13).color(PR_NEUTRAL).into();
    }
    let Some(lookup) = &linear.issues else {
        return text("no issues loaded yet")
            .size(13)
            .color(PR_NEUTRAL)
            .into();
    };

    match tracker_ui::issue_list(lookup) {
        IssueListView::Unavailable(msg) => {
            // **절대 "no issues"가 아니다** — 조회 실패는 구별된 색·문구로.
            text(format!("issues unavailable — {msg}"))
                .size(13)
                .color(LINEAR_UNAVAILABLE)
                .into()
        }
        IssueListView::Issues { issues, has_more } => {
            if issues.is_empty() {
                return text("no issues").size(13).color(PR_NEUTRAL).into();
            }
            let mut list = column![].spacing(4);
            for issue in &issues {
                list = list.push(issue_row(issue, selected));
            }
            if has_more {
                // 무성 절단 금지 — bounded traversal이 끊었음을 표면화한다.
                list = list.push(
                    text("…more (showing a bounded page)")
                        .size(12)
                        .color(PR_NEUTRAL),
                );
            }
            list.into()
        }
    }
}

/// 이슈 한 줄: 식별자 + 제목 (+ 상태) + "link" 버튼. 링크 버튼은 선택된 worktree가 있을 때만
/// 눌린다(없으면 무엇에 링크할지 모른다 — 죽은 버튼 대신 비활성).
fn issue_row(
    issue: &suaegi_tracker::Issue,
    selected: Option<&WorktreeId>,
) -> Element<'static, Message> {
    let state_suffix = issue
        .state
        .as_deref()
        .map(|s| format!(" · {s}"))
        .unwrap_or_default();
    let label = text(format!(
        "{} {}{}",
        issue.identifier, issue.title, state_suffix
    ))
    .size(13);

    let link_msg = selected.map(|wt| Message::LinearIssueLinked {
        worktree: wt.clone(),
        issue: issue.clone(),
    });
    let link_btn = button(text("link").size(12)).on_press_maybe(link_msg);

    row![label, link_btn]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

/// N2: Jira 패널. 미연결이면 **site/email/토큰(마스킹)/Cloud-Server 토글** 입력 + Connect,
/// 연결됐으면 계정 이름 + 이슈 새로고침 + 이슈 목록을 그린다.
///
/// **픽셀·상호작용은 사람 눈으로 본다.** 검사되는 결정은 `tracker_ui`가 값으로 뽑는다:
/// 연결 결과 매핑(성공/실패), 그리고 crux인 **Unavailable≠no issues**(이슈 목록 매핑).
pub(crate) fn jira_panel(state: &AppState) -> Element<'_, Message> {
    let jira: &JiraState = state.jira();
    let selected = state.selected_worktree().cloned();

    let mut panel = column![text("Jira").size(16)].spacing(6);

    match &jira.viewer {
        // 연결됨: 계정 이름 + 이슈 새로고침 + 이슈 목록.
        Some(viewer) => {
            panel = panel.push(
                row![
                    text(format!("● {}", viewer.display_name))
                        .size(14)
                        .color(PR_OPEN),
                    button(text("↻ issues").size(13)).on_press(Message::JiraIssuesRefreshRequested),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
            panel = panel.push(jira_issue_list(jira, selected.as_ref()));
        }
        // 미연결(또는 연결 실패): 연결 폼. 연결 중이면 버튼을 잠근다.
        None => {
            panel = panel.push(
                text_input(
                    "Jira site URL (https://acme.atlassian.net)",
                    &jira.site_url_input,
                )
                .on_input(Message::JiraSiteUrlChanged)
                .size(14),
            );
            panel = panel.push(
                text_input("email (Cloud / Server-Basic)", &jira.email_input)
                    .on_input(Message::JiraEmailChanged)
                    .size(14),
            );
            // **마스킹된 secure 입력** — 토큰이 화면에 평문으로 안 뜬다(로그/Debug에도 안 샌다).
            panel = panel.push(
                text_input("API token / PAT", &jira.token_input)
                    .secure(true)
                    .on_input(|value| Message::JiraTokenChanged(crate::SecretDraft::new(value)))
                    .on_submit(Message::JiraConnectSubmitted)
                    .size(14),
            );
            // Cloud/Server 토글: 체크 = Cloud(v3/ADF), 해제 = Server/DC(v2/plain).
            panel = panel.push(
                checkbox(jira.is_cloud)
                    .label("Cloud (uncheck for Server/DC)")
                    .on_toggle(Message::JiraCloudToggled)
                    .size(16)
                    .text_size(11),
            );
            let connect_label = if jira.connecting {
                "Connecting…"
            } else {
                "Connect"
            };
            panel = panel.push(
                button(text(connect_label).size(14))
                    .on_press_maybe((!jira.connecting).then_some(Message::JiraConnectSubmitted)),
            );
            if let Some(err) = &jira.connect_error {
                panel = panel.push(text(format!("! {err}")).size(13).color(PR_CLOSED));
            }
        }
    }

    container(panel)
        .padding(10)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// Jira 이슈 목록 렌더. **`tracker_ui::jira_issue_list`가 Unavailable≠no issues를 정하고**,
/// 여기선 그 값을 픽셀로 옮길 뿐이다(사람 눈). 링크 버튼은 **선택된** worktree를 이 이슈에 링크한다.
fn jira_issue_list(jira: &JiraState, selected: Option<&WorktreeId>) -> Element<'static, Message> {
    if jira.issues_loading && jira.issues.is_none() {
        return text("loading issues…").size(13).color(PR_NEUTRAL).into();
    }
    let Some(lookup) = &jira.issues else {
        return text("no issues loaded yet")
            .size(13)
            .color(PR_NEUTRAL)
            .into();
    };

    match tracker_ui::jira_issue_list(lookup) {
        JiraIssueListView::Unavailable(msg) => {
            // **절대 "no issues"가 아니다** — 조회 실패는 구별된 색·문구로.
            text(format!("issues unavailable — {msg}"))
                .size(13)
                .color(JIRA_UNAVAILABLE)
                .into()
        }
        JiraIssueListView::Issues { issues, has_more } => {
            if issues.is_empty() {
                return text("no issues").size(13).color(PR_NEUTRAL).into();
            }
            let mut list = column![].spacing(4);
            for issue in &issues {
                list = list.push(jira_issue_row(issue, selected));
            }
            if has_more {
                // 무성 절단 금지 — bounded 검색이 끊었음을 표면화한다.
                list = list.push(
                    text("…more (showing a bounded page)")
                        .size(12)
                        .color(PR_NEUTRAL),
                );
            }
            list.into()
        }
    }
}

/// Jira 이슈 한 줄: 키 + 제목 (+ 상태) + "link" 버튼. 링크 버튼은 선택된 worktree가 있을 때만
/// 눌린다(없으면 무엇에 링크할지 모른다 — 죽은 버튼 대신 비활성).
fn jira_issue_row(
    issue: &suaegi_tracker::JiraIssue,
    selected: Option<&WorktreeId>,
) -> Element<'static, Message> {
    let status_suffix = issue
        .status
        .as_deref()
        .map(|s| format!(" · {s}"))
        .unwrap_or_default();
    let label = text(format!("{} {}{}", issue.key, issue.title, status_suffix)).size(13);

    let link_msg = selected.map(|wt| Message::JiraIssueLinked {
        worktree: wt.clone(),
        issue: issue.clone(),
    });
    let link_btn = button(text("link").size(12)).on_press_maybe(link_msg);

    row![label, link_btn]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

/// `LoadOrigin::Fresh`(신규 설치)와 `Loaded`(정상 로드)는 경고가 없다.
/// `Recovered`/`RecoveryFailed`는 알린다. 저장 실패(`SaveStatus::Failed`)는
/// 항상 최우선으로 드러나야 하고, 정상적인 디바운스 대체(`Superseded`)는
/// 절대 에러처럼 보이면 안 된다 — 안 그러면 사용자가 상태 표시줄 자체를
/// 무시하는 법을 배운다.
fn status_line(state: &AppState) -> Option<String> {
    if state.future_schema_guarded() {
        return Some(
            "Settings came from a newer Suaegi. Saving is paused to protect them.".to_string(),
        );
    }
    if let Some(SaveStatus::Failed(message)) = state.last_save_status() {
        return Some(format!("Save failed: {message}"));
    }
    match state.load_origin() {
        LoadOrigin::Fresh | LoadOrigin::Loaded => None,
        LoadOrigin::Recovered { slot } => Some(format!(
            "Recovered from backup #{slot} — a recent save may be missing."
        )),
        LoadOrigin::RecoveryFailed => {
            Some("Could not read saved data — starting from an empty state.".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::persistence_thread::{LoadDiagnostics, SaveReport};
    use crate::state::OpId;
    use suaegi_core::domain::PersistedState;

    fn repo(name: &str) -> Repo {
        Repo {
            id: RepoId(format!("/tmp/{name}")),
            path: PathBuf::from(format!("/tmp/{name}")),
            display_name: name.to_string(),
            worktree_base_ref: None,
        }
    }

    fn entry(name: &str) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(format!("/tmp/wt/{name}")),
            branch: Some(name.to_string()),
            head: None,
            is_main: false,
        }
    }

    #[test]
    fn worktree_rows_group_under_their_repo_in_a_stable_order() {
        let mut state = AppState::default();
        let repo_b = repo("b-repo");
        let repo_a = repo("a-repo");
        // 등록 순서를 일부러 알파벳 역순으로 해서, "정렬됐다"가 아니라
        // "등록 순서를 보존한다"는 걸 검증한다.
        state.upsert_repo(repo_b.clone());
        state.upsert_repo(repo_a.clone());

        state.note_list_issued(repo_a.id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_a.id.clone(),
            OpId(1),
            vec![entry("a1"), entry("a2")],
        );
        state.note_list_issued(repo_b.id.clone(), OpId(1));
        state.apply_authoritative_listing(repo_b.id.clone(), OpId(1), vec![entry("b1")]);

        let groups = grouped_worktrees(&state);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0].repo.id, repo_b.id,
            "registration order must win, not alphabetical"
        );
        assert_eq!(groups[1].repo.id, repo_a.id);
        assert_eq!(
            groups[1]
                .worktrees
                .iter()
                .map(|w| w.branch.clone())
                .collect::<Vec<_>>(),
            vec![Some("a1".to_string()), Some("a2".to_string())],
        );

        // 순서는 호출마다 안정적이어야 한다 (HashMap 반복 순서에 기대면 흔들린다).
        let groups_again = grouped_worktrees(&state);
        let ids: Vec<_> = groups.iter().map(|g| g.repo.id.clone()).collect();
        let ids_again: Vec<_> = groups_again.iter().map(|g| g.repo.id.clone()).collect();
        assert_eq!(ids, ids_again);
    }

    #[test]
    fn a_worktree_whose_repo_is_gone_is_skipped_without_panicking() {
        let mut state = AppState::default();
        let gone = RepoId("/tmp/deleted-repo".into());
        // repo는 등록돼 있지 않다 — 영속화된 worktree가 삭제된 repo를 가리키는
        // 상황을 흉내낸다.
        state.note_list_issued(gone.clone(), OpId(1));
        state.apply_authoritative_listing(gone, OpId(1), vec![entry("orphan")]);

        let groups = grouped_worktrees(&state);
        assert!(
            groups.is_empty(),
            "an orphaned worktree entry must not surface a group"
        );
    }

    #[test]
    fn status_line_text_distinguishes_fresh_install_from_recovery_failure() {
        assert!(status_line(&AppState::fresh()).is_none());
        assert!(status_line(&AppState::recovery_failed()).is_some());
        assert!(status_line(&AppState::recovered(0)).is_some());
    }

    #[test]
    fn a_future_schema_guard_takes_priority_over_a_generic_save_failure() {
        let state = AppState::from_load(LoadDiagnostics {
            state: PersistedState::default(),
            origin: LoadOrigin::RecoveryFailed,
            save_blocked: true,
        });
        assert!(state.future_schema_guarded());
        assert!(status_line(&state)
            .expect("a guarded boot must always be visible")
            .contains("newer Suaegi"));
    }

    /// Task 8: `PersistenceHandle::spawn`이 만드는 `LoadDiagnostics`가 실제로
    /// `AppState::from_load`(부팅이 쓰는 바로 그 함수)를 거쳐 상태 표시줄까지
    /// 흘러가는지. 위 테스트는 손으로 만든 `AppState::fresh()`류 헬퍼로
    /// `status_line`의 순수 매핑만 검증하지만, `from_load`가 `load.origin`을
    /// `state.load_origin`에 대입하는 걸 빠뜨리는 mutation은 그걸로는 못
    /// 잡는다 — 이 테스트가 그 배선 자체를 태운다. **`Fresh`는 절대 경고를
    /// 내면 안 된다**: 신규 설치가 데이터 손실처럼 보이면 안 되기 때문이다.
    #[test]
    fn load_diagnostics_reach_the_status_line_through_the_real_boot_wiring_for_all_four_origins() {
        let cases = [
            (LoadOrigin::Fresh, false),
            (LoadOrigin::Loaded, false),
            (LoadOrigin::Recovered { slot: 2 }, true),
            (LoadOrigin::RecoveryFailed, true),
        ];
        for (origin, expects_warning) in cases {
            let load = LoadDiagnostics {
                state: PersistedState::default(),
                origin,
                save_blocked: false,
            };
            let state = AppState::from_load(load);
            assert_eq!(
                status_line(&state).is_some(),
                expects_warning,
                "origin {origin:?} must {} a status-line warning",
                if expects_warning {
                    "produce"
                } else {
                    "not produce"
                }
            );
        }
    }

    #[test]
    fn a_failed_save_is_visible_in_the_status_line() {
        assert!(status_line(&AppState::with_save_error("disk full"))
            .unwrap()
            .contains("disk full"));
    }

    /// 위 테스트는 손으로 만든 `with_save_error` 헬퍼로 `status_line`의 순수
    /// 매핑만 본다. 이 테스트는 실제 `Message::Saved` 디스패치(`AppState::boot`가
    /// `results` 스트림을 연결하면 실제로 도착하는 바로 그 메시지)를 태워
    /// `last_save_status`에 반영되는 배선 자체를 검증한다.
    #[test]
    fn a_failed_save_status_reaches_the_status_line_through_real_dispatch() {
        let mut state = AppState::fresh();
        let _ = state.update(Message::Saved(SaveReport {
            seq: 1,
            status: SaveStatus::Failed("disk full".to_string()),
        }));
        assert!(status_line(&state)
            .expect("a failed save must surface a warning")
            .contains("disk full"));
    }

    /// **`Unknown`과 `Working`은 반드시 구별된다.** "모른다"와 "바쁘다"는 다른
    /// 상태이고, 이 구별이 사라지면 훅이 안 붙은 pane(신뢰 대화상자 대기 등)이
    /// 열심히 일하는 것처럼 보인다.
    #[test]
    fn every_badge_state_is_visually_distinct() {
        let agent = AgentPresence::Agent("claude");
        let glyphs: Vec<(&str, Color)> = [
            BadgeState::Working,
            BadgeState::Waiting,
            BadgeState::Done,
            BadgeState::Unknown,
        ]
        .into_iter()
        .map(|b| badge_glyph(b, agent))
        .collect();

        for (i, (glyph, color)) in glyphs.iter().enumerate() {
            assert!(!glyph.is_empty(), "state {i} must render something");
            for (j, (other_glyph, other_color)) in glyphs.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        glyph, other_glyph,
                        "badge states {i} and {j} share a glyph — 'we don't know' must not \
                         look like 'it is busy'"
                    );
                    assert_ne!(
                        (color.r, color.g, color.b),
                        (other_color.r, other_color.g, other_color.b),
                        "badge states {i} and {j} share a colour"
                    );
                }
            }
        }
    }

    /// 오류 스타일링은 **리듀서가 아니라** `AgentPresence::Exited{{code}}`에서 온다.
    /// `BadgeState`에 오류 변형을 더하면 배지 상태와 프로세스 사실이 두 곳에서
    /// 관리된다.
    #[test]
    fn a_nonzero_exit_is_styled_as_an_error_whatever_the_badge_says() {
        let (glyph, color) = badge_glyph(BadgeState::Done, AgentPresence::Exited { code: 1 });
        assert_eq!(glyph, "×");
        assert_eq!((color.r, color.g, color.b), {
            let red = Color::from_rgb8(0xc0, 0x39, 0x2b);
            (red.r, red.g, red.b)
        });

        // 대조군: 정상 종료(0)는 오류로 보이지 않는다 — 그렇지 않으면 성공적으로
        // 끝난 세션이 전부 빨간 ×가 된다.
        let (ok_glyph, _) = badge_glyph(BadgeState::Done, AgentPresence::Exited { code: 0 });
        assert_ne!(
            ok_glyph, "×",
            "exit code 0 is a normal finish, not a failure"
        );
        assert_eq!(
            ok_glyph,
            badge_glyph(BadgeState::Done, AgentPresence::NoAgent).0
        );
    }

    /// 최종 리뷰 항목 3: `list_worktrees`가 첫 엔트리에 `is_main: true`를
    /// 세우는데(`suaegi-git`), 여기서 그걸 읽지 않으면 git이 항상 거부할
    /// main 체크아웃에도 remove 버튼이 뜬다.
    #[test]
    fn the_main_worktree_checkout_is_not_removable() {
        let main = WorktreeEntry {
            is_main: true,
            ..entry("main")
        };
        let secondary = WorktreeEntry {
            is_main: false,
            ..entry("feature")
        };
        assert!(!worktree_is_removable(&main));
        assert!(worktree_is_removable(&secondary));
    }

    #[test]
    fn a_superseded_save_does_not_look_like_an_error() {
        // Superseded는 정상적인 debounce 대체다 — 에러처럼 보이면 사용자가
        // 상태 표시줄을 무시하는 법을 배운다.
        let mut state = AppState::fresh();
        let _ = state.update(Message::Saved(SaveReport {
            seq: 1,
            status: SaveStatus::Superseded { by: 2 },
        }));
        assert!(status_line(&state).is_none());
    }

    use suaegi_forge::ForgeUnavailable;

    /// **"상태 모름"과 "PR 없음"은 시각적으로 구별된다** — `indicator_for`(§5 (a))가
    /// 둘을 다른 변형으로 나눠도, 라벨이 같은 문구·색이면 화면에서 구별이 사라진다.
    /// 이 테스트가 뷰 쪽 계약을 잠근다.
    #[test]
    fn unknown_status_never_looks_like_no_pr() {
        let no_pr = pr_indicator_label(&PrIndicator::NoPr).expect("no PR has a label");
        let unknown = pr_indicator_label(&PrIndicator::Unknown(ForgeUnavailable::NotAuthenticated))
            .expect("unknown has a label");
        assert_ne!(
            no_pr.0, unknown.0,
            "'no PR' and 'status unknown' must read differently"
        );
        assert_ne!(
            (no_pr.1.r, no_pr.1.g, no_pr.1.b),
            (unknown.1.r, unknown.1.g, unknown.1.b),
            "'no PR' and 'status unknown' must not share a colour"
        );
        // 인증 안 됨은 실행 가능한 힌트를 노출한다.
        assert!(unknown.0.contains("gh auth login"));
    }

    /// Hidden(조회 전/비-GitHub)은 아무 라벨도 없다 — worktree 행에 잡음을 안 남긴다.
    #[test]
    fn a_hidden_indicator_renders_no_label() {
        assert!(pr_indicator_label(&PrIndicator::Hidden).is_none());
    }

    /// Present는 번호·상태·체크 요약을 한 줄에 담는다.
    #[test]
    fn a_present_pr_shows_number_state_and_checks() {
        let label = pr_indicator_label(&PrIndicator::Present {
            number: 12,
            state: ReviewState::Open,
            checks: ChecksSummary {
                passing: 2,
                failing: 1,
                pending: 0,
            },
        })
        .expect("present has a label");
        assert!(label.0.contains("#12"));
        assert!(label.0.contains("open"));
        assert!(label.0.contains("✓2"));
        assert!(label.0.contains("✗1"));
    }

    #[test]
    fn checks_summary_is_silent_when_there_are_no_checks() {
        assert!(checks_text(ChecksSummary::default()).is_none());
    }

    #[test]
    fn compact_worktree_cards_remove_the_duplicate_detail_line_and_tighten_spacing() {
        let regular = worktree_row_metrics(false);
        let compact = worktree_row_metrics(true);
        assert!(regular.show_detail);
        assert!(!compact.show_detail);
        assert!(compact.label_size < regular.label_size);
        assert!(compact.content_padding[0] < regular.content_padding[0]);
    }
}
