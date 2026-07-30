//! Orca-compatible Source Control AI text generation.
//!
//! The generator never invokes a shell. Agent commands are tokenized into an
//! executable plus argv, prompts are sent over stdin whenever the selected CLI
//! supports it, and repository context is bounded before it leaves Suaegi.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use suaegi_core::domain::{
    RepoSourceControlAiSetting, SourceControlAiActionRecipeSetting, SourceControlAiSetting,
};
use suaegi_git::runner::GitRunner;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const GENERATION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_AGENT_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PATCH_CHARS: usize = 96_000;

pub const AGENT_IDS: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "pi",
    "amp",
    "cursor",
    "kimi",
    "copilot",
    "antigravity",
    "custom",
];

pub fn models_for(agent_id: &str) -> &'static [&'static str] {
    match agent_id {
        "claude" => &["haiku", "sonnet", "opus"],
        "codex" => &[
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.3-codex",
            "gpt-5.3-codex-spark",
            "gpt-5.2",
        ],
        "opencode" => &["opencode/deepseek-v4-flash-free", "opencode/gpt-5.4-mini"],
        "pi" => &["github-copilot/gpt-5.4-mini"],
        "amp" => &["smart", "rush", "large", "deep"],
        "cursor" => &["auto"],
        "copilot" => &[
            "auto",
            "claude-haiku-4.5",
            "claude-sonnet-4.5",
            "claude-sonnet-4.6",
            "claude-opus-4.5",
            "claude-opus-4.6",
            "claude-opus-4.6-fast",
            "claude-opus-4.7",
            "gpt-4.1",
            "gpt-5-mini",
            "gpt-5.2",
            "gpt-5.2-codex",
            "gpt-5.3-codex",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.5",
        ],
        "kimi" => &["default", "kimi-code/kimi-for-coding"],
        "antigravity" => &[
            "Gemini 3.5 Flash (Medium)",
            "Gemini 3.5 Flash (High)",
            "Gemini 3.5 Flash (Low)",
        ],
        "custom" => &["default"],
        _ => &["default"],
    }
}

pub fn default_model_for(agent_id: &str) -> &'static str {
    match agent_id {
        "claude" => "sonnet",
        "copilot" => "gpt-5.4",
        _ => models_for(agent_id).first().copied().unwrap_or("default"),
    }
}

pub fn thinking_levels_for(agent_id: &str, model: &str) -> &'static [&'static str] {
    match agent_id {
        "claude" if model != "haiku" => &["low", "medium", "high", "xhigh", "max"],
        "codex" => &["low", "medium", "high", "xhigh"],
        "copilot" if model.contains("gpt-5") || model.contains("codex") => {
            &["low", "medium", "high", "xhigh"]
        }
        "opencode" if model.contains("gpt-5") || model.contains("codex") => {
            &["low", "medium", "high", "xhigh"]
        }
        "pi" => &["off", "low", "medium", "high", "xhigh"],
        "amp" if matches!(model, "large" | "deep") => &["low", "medium", "high"],
        "kimi" if model != "default" => &["on", "off"],
        _ => &[""],
    }
}

pub fn has_dynamic_models(agent_id: &str) -> bool {
    matches!(
        agent_id,
        "codex" | "opencode" | "pi" | "cursor" | "antigravity"
    )
}

pub async fn discover_models(
    agent_id: String,
    command_override: Option<String>,
) -> Result<Vec<String>, String> {
    let discovery_args = match agent_id.as_str() {
        "codex" => vec!["debug", "models"],
        "opencode" => vec!["models"],
        "pi" => vec!["--list-models"],
        "cursor" => vec!["--list-models"],
        "antigravity" => vec!["models"],
        _ => {
            return Ok(models_for(&agent_id)
                .iter()
                .map(|value| value.to_string())
                .collect())
        }
    };
    let default_binary = match agent_id.as_str() {
        "cursor" => "cursor-agent",
        "antigravity" => "agy",
        value => value,
    };
    let mut command_parts = command_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(tokenize_command)
        .transpose()?
        .unwrap_or_else(|| vec![default_binary.to_string()]);
    if command_parts.is_empty() {
        return Err("The agent command is empty.".to_string());
    }
    let binary = command_parts.remove(0);
    command_parts.extend(discovery_args.into_iter().map(str::to_string));
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(&binary)
            .args(&command_parts)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Model discovery timed out.".to_string())?
    .map_err(|_| format!("{agent_id} CLI is not installed or could not be started."))?;
    if !output.status.success() {
        return Err(format!("{agent_id} model discovery failed."));
    }
    if output.stdout.len() > 512 * 1024 {
        return Err("Model discovery returned too much output.".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let candidates = parse_discovered_models(&agent_id, &stdout);
    if candidates.is_empty() {
        return Err("No models were reported by the agent CLI.".to_string());
    }
    Ok(candidates)
}

fn parse_discovered_models(agent_id: &str, stdout: &str) -> Vec<String> {
    let candidates = if agent_id == "codex" {
        serde_json::from_str::<serde_json::Value>(stdout)
            .ok()
            .and_then(|value| {
                value
                    .get("models")
                    .and_then(|models| models.as_array())
                    .cloned()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|model| model.get("slug")?.as_str().map(str::to_string))
            .collect::<Vec<_>>()
    } else {
        stdout
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                match agent_id {
                    "pi" => {
                        let mut fields = line.split_whitespace();
                        let provider = fields.next()?;
                        let model = fields.next()?;
                        (!provider.eq_ignore_ascii_case("provider"))
                            .then(|| format!("{provider}/{model}"))
                    }
                    "cursor" => line
                        .split_once(" - ")
                        .map(|(model, _)| model.trim().to_string()),
                    "antigravity" => (!line.is_empty()).then(|| line.to_string()),
                    _ => (!line.is_empty() && !line.contains(' ')).then(|| line.to_string()),
                }
            })
            .collect::<Vec<_>>()
    };
    let mut unique = Vec::new();
    for model in candidates {
        if !model.is_empty()
            && model.chars().count() <= 160
            && unique.len() < 512
            && !unique.contains(&model)
        {
            unique.push(model);
        }
    }
    unique
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOperation {
    CommitMessage,
    PullRequest,
    BranchName,
}

impl TextOperation {
    pub const fn action_id(self) -> &'static str {
        match self {
            Self::CommitMessage => "commitMessage",
            Self::PullRequest => "pullRequest",
            Self::BranchName => "branchName",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedPullRequestFields {
    pub base: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub operation: TextOperation,
    pub settings: SourceControlAiSetting,
    pub repo_overrides: Option<RepoSourceControlAiSetting>,
    pub agent_command_overrides: HashMap<String, String>,
    pub environment: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentPlan {
    binary: String,
    args: Vec<String>,
    stdin_payload: Option<String>,
    label: String,
}

pub async fn generate_commit_message(
    path: &Path,
    request: GenerationRequest,
) -> Result<String, String> {
    let runner = GitRunner::new();
    let branch = git_text(&runner, path, &["branch", "--show-current"]).await?;
    let staged_files = git_text(
        &runner,
        path,
        &["diff", "--cached", "--name-status", "--no-ext-diff"],
    )
    .await?;
    if staged_files.trim().is_empty() {
        return Err("Stage changes before generating a commit message.".to_string());
    }
    let staged_patch = git_text(
        &runner,
        path,
        &["diff", "--cached", "--no-ext-diff", "--unified=3"],
    )
    .await?;
    let base_prompt = build_commit_prompt(&branch, &staged_files, &staged_patch);
    let prompt = render_action_prompt(
        &request,
        &base_prompt,
        &[
            ("branch", branch.as_str()),
            ("stagedFiles", staged_files.as_str()),
            ("stagedPatch", staged_patch.as_str()),
        ],
    );
    let output = run_generation(path, request, prompt).await?;
    Ok(clean_commit_message(&output))
}

pub async fn generate_pull_request_fields(
    path: &Path,
    base: &str,
    current_title: &str,
    current_body: &str,
    current_draft: bool,
    request: GenerationRequest,
) -> Result<GeneratedPullRequestFields, String> {
    let runner = GitRunner::new();
    let branch = git_text(&runner, path, &["branch", "--show-current"]).await?;
    let range = format!("{base}...HEAD");
    let commit_range = format!("{base}..HEAD");
    let commits = git_text(
        &runner,
        path,
        &["log", "--oneline", "--no-decorate", &commit_range],
    )
    .await?;
    let changed_files =
        git_text(&runner, path, &["diff", "--stat", "--no-ext-diff", &range]).await?;
    let patch = git_text(
        &runner,
        path,
        &["diff", "--no-ext-diff", "--unified=3", &range],
    )
    .await?;
    let base_prompt = build_pull_request_prompt(
        &branch,
        base,
        current_title,
        current_body,
        current_draft,
        &commits,
        &changed_files,
        &patch,
    );
    let draft_text = current_draft.to_string();
    let prompt = render_action_prompt(
        &request,
        &base_prompt,
        &[
            ("branch", branch.as_str()),
            ("baseBranch", base),
            ("currentTitle", current_title),
            ("currentBody", current_body),
            ("currentDraft", draft_text.as_str()),
            ("commitSummary", commits.as_str()),
            ("changedFiles", changed_files.as_str()),
            ("patch", patch.as_str()),
        ],
    );
    let output = run_generation(path, request, prompt).await?;
    parse_pull_request_fields(&output, base, current_title, current_body, current_draft)
}

pub async fn generate_branch_name(
    path: &Path,
    first_prompt: &str,
    request: GenerationRequest,
) -> Result<String, String> {
    let base_prompt = format!(
        "Generate a concise lowercase kebab-case git branch name for the work below.\n\
Return only the branch name leaf, with no prefix, quotes, prose, or code fence.\n\
Use at most five words and keep only letters, numbers, and hyphens.\n\n\
First user request:\n{}",
        bounded(first_prompt, 8_000)
    );
    let prompt = render_action_prompt(
        &request,
        &base_prompt,
        &[("firstPrompt", first_prompt), ("assistantMessage", "")],
    );
    let output = run_generation(path, request, prompt).await?;
    let cleaned = output
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if cleaned.is_empty() {
        Err("The agent returned no branch name.".to_string())
    } else {
        Ok(cleaned)
    }
}

pub fn render_launch_action_prompt(
    settings: &SourceControlAiSetting,
    repo_overrides: Option<&RepoSourceControlAiSetting>,
    action_id: &str,
    detail: &str,
) -> Result<String, String> {
    if ![
        "fixCommitFailure",
        "fixPushFailure",
        "fixChecks",
        "resolveConflicts",
        "resolveComments",
    ]
    .contains(&action_id)
    {
        return Err("Unsupported Source Control AI action.".to_string());
    }
    let enabled = repo_overrides
        .and_then(|overrides| overrides.enabled)
        .unwrap_or(settings.enabled);
    if !enabled {
        return Err("Source Control AI is disabled for this repository.".to_string());
    }
    let recipe = repo_overrides
        .and_then(|overrides| overrides.action_overrides.get(action_id))
        .or_else(|| settings.actions.get(action_id))
        .cloned()
        .unwrap_or_default();
    let label = match action_id {
        "fixCommitFailure" => "Fix the failed git commit",
        "fixPushFailure" => "Fix the failed git push without force-pushing",
        "fixChecks" => "Inspect and fix the failing hosted-review checks",
        "resolveConflicts" => "Resolve the current merge conflicts safely",
        "resolveComments" => "Address the unresolved review comments",
        _ => unreachable!(),
    };
    let base_prompt = format!(
        "{label}. Inspect the repository and current source-control state, make the smallest safe changes, and verify the result.\n\nContext:\n{}",
        bounded(detail, 8_000)
    );
    Ok(recipe
        .command_input_template
        .replace("{basePrompt}", &base_prompt)
        .replace("{{basePrompt}}", &base_prompt))
}

fn render_action_prompt(
    request: &GenerationRequest,
    base_prompt: &str,
    variables: &[(&str, &str)],
) -> String {
    let recipe = resolve_recipe(request, request.operation.action_id());
    let mut rendered = recipe.command_input_template;
    let mut all = Vec::with_capacity(variables.len() + 1);
    all.push(("basePrompt", base_prompt));
    all.extend_from_slice(variables);
    for (name, value) in all {
        rendered = rendered
            .replace(&format!("{{{name}}}"), value)
            .replace(&format!("{{{{{name}}}}}"), value);
    }
    rendered
}

fn resolve_recipe(
    request: &GenerationRequest,
    action_id: &str,
) -> SourceControlAiActionRecipeSetting {
    request
        .repo_overrides
        .as_ref()
        .and_then(|overrides| overrides.action_overrides.get(action_id))
        .cloned()
        .or_else(|| request.settings.actions.get(action_id).cloned())
        .unwrap_or_default()
}

async fn run_generation(
    path: &Path,
    request: GenerationRequest,
    prompt: String,
) -> Result<String, String> {
    if !request
        .repo_overrides
        .as_ref()
        .and_then(|overrides| overrides.enabled)
        .unwrap_or(request.settings.enabled)
    {
        return Err("Source Control AI is disabled for this repository.".to_string());
    }
    let recipe = resolve_recipe(&request, request.operation.action_id());
    let agent_id = recipe
        .agent_id
        .as_deref()
        .unwrap_or(&request.settings.agent_id);
    let custom_command = request
        .repo_overrides
        .as_ref()
        .and_then(|overrides| overrides.custom_agent_command.as_deref())
        .unwrap_or(&request.settings.custom_agent_command);
    let plan = plan_agent(
        agent_id,
        &request.settings.model,
        &request.settings.thinking_level,
        custom_command,
        request.agent_command_overrides.get(agent_id),
        &recipe.agent_args,
        &prompt,
    )?;
    let mut command = Command::new(&plan.binary);
    command
        .args(&plan.args)
        .current_dir(path)
        .stdin(if plan.stdin_payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (name, value) in request.environment {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|_| {
        format!(
            "{} CLI is not installed or could not be started.",
            plan.label
        )
    })?;
    if let Some(payload) = plan.stdin_payload {
        let Some(mut stdin) = child.stdin.take() else {
            return Err(format!("{} did not accept prompt input.", plan.label));
        };
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|_| format!("Could not send the prompt to {}.", plan.label))?;
        drop(stdin);
    }
    let output = tokio::time::timeout(GENERATION_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{} generation timed out.", plan.label))?
        .map_err(|_| format!("{} generation could not be completed.", plan.label))?;
    if output.stdout.len().saturating_add(output.stderr.len()) > MAX_AGENT_OUTPUT_BYTES {
        return Err(format!("{} returned too much output.", plan.label));
    }
    if !output.status.success() {
        return Err(format!(
            "{} CLI failed with code {}.",
            plan.label,
            output.status.code().unwrap_or(-1)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(format!("{} returned no generated text.", plan.label));
    }
    Ok(stdout)
}

fn plan_agent(
    agent_id: &str,
    model: &str,
    thinking_level: &str,
    custom_command: &str,
    command_override: Option<&String>,
    agent_args: &str,
    prompt: &str,
) -> Result<AgentPlan, String> {
    if agent_id == "custom" {
        return plan_custom_command(custom_command, agent_args, prompt);
    }
    let (binary, args, stdin, label) = match agent_id {
        "claude" => (
            "claude",
            vec![
                "-p",
                "--output-format",
                "text",
                "--model",
                model,
                "--permission-mode",
                "plan",
            ],
            true,
            "Claude",
        ),
        "codex" => (
            "codex",
            vec![
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "-s",
                "read-only",
                "--model",
                model,
            ],
            true,
            "Codex",
        ),
        "opencode" => (
            "opencode",
            vec![
                "run", "--model", model, "--agent", "build", "--format", "default",
            ],
            true,
            "OpenCode",
        ),
        "pi" => (
            "pi",
            vec![
                "--print",
                "--no-session",
                "--no-tools",
                "--no-extensions",
                "--no-skills",
                "--no-context-files",
                "--mode",
                "text",
                "--model",
                model,
            ],
            true,
            "Pi",
        ),
        "amp" => (
            "amp",
            vec![
                "--execute",
                "--no-notifications",
                "--no-ide",
                "--no-jetbrains",
                "--mode",
                model,
            ],
            true,
            "Amp",
        ),
        "cursor" => (
            "cursor-agent",
            vec![
                "--print",
                "--mode",
                "ask",
                "--trust",
                "--output-format",
                "text",
                "--model",
                model,
            ],
            false,
            "Cursor",
        ),
        "kimi" => {
            let mut values = vec!["--print", "--quiet"];
            if !model.is_empty() && model != "default" {
                values.extend(["--model", model]);
            }
            ("kimi", values, true, "Kimi")
        }
        "copilot" => (
            "copilot",
            vec![
                "--prompt",
                prompt,
                "--silent",
                "--stream",
                "off",
                "--no-custom-instructions",
                "--model",
                model,
            ],
            false,
            "GitHub Copilot",
        ),
        "antigravity" => (
            "agy",
            vec!["--print", "--sandbox", "--model", model],
            true,
            "Antigravity",
        ),
        _ => {
            return Err(format!(
                "Agent \"{agent_id}\" does not support Source Control AI."
            ))
        }
    };
    let mut owned = args.into_iter().map(str::to_string).collect::<Vec<_>>();
    let effort = thinking_level.trim();
    if !effort.is_empty() {
        match agent_id {
            "claude" | "amp" | "copilot" => {
                owned.extend(["--effort".to_string(), effort.to_string()]);
            }
            "codex" => owned.extend(["-c".to_string(), format!("model_reasoning_effort={effort}")]),
            "opencode" => {
                owned.extend(["--variant".to_string(), effort.to_string()]);
            }
            "pi" => {
                owned.extend(["--thinking".to_string(), effort.to_string()]);
            }
            "kimi" if effort == "on" => owned.push("--thinking".to_string()),
            "kimi" if effort == "off" => owned.push("--no-thinking".to_string()),
            _ => {}
        }
    }
    if !stdin && agent_id == "cursor" {
        owned.push(prompt.to_string());
    }
    let additional = tokenize_command(agent_args)?;
    owned.extend(additional);
    let (binary, prefix) = command_override
        .filter(|value| !value.trim().is_empty())
        .map(|value| tokenize_command(value))
        .transpose()?
        .and_then(|mut values| {
            (!values.is_empty()).then(|| {
                let binary = values.remove(0);
                (binary, values)
            })
        })
        .unwrap_or_else(|| (binary.to_string(), Vec::new()));
    let mut final_args = prefix;
    final_args.extend(owned);
    Ok(AgentPlan {
        binary,
        args: final_args,
        stdin_payload: stdin.then(|| prompt.to_string()),
        label: label.to_string(),
    })
}

fn plan_custom_command(command: &str, agent_args: &str, prompt: &str) -> Result<AgentPlan, String> {
    let mut tokens = tokenize_command(command)?;
    if tokens.is_empty() {
        return Err("Custom command is empty.".to_string());
    }
    let uses_placeholder = tokens.iter().any(|value| value.contains("{prompt}"));
    for token in &mut tokens {
        *token = token.replace("{prompt}", prompt);
    }
    let binary = tokens.remove(0);
    tokens.extend(tokenize_command(agent_args)?);
    Ok(AgentPlan {
        label: binary.clone(),
        binary,
        args: tokens,
        stdin_payload: (!uses_placeholder).then(|| prompt.to_string()),
    })
}

fn tokenize_command(value: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote = None;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(expected) = quote {
            if ch == '\\' && expected == '"' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if ch == expected {
                quote = None;
                in_token = true;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
            in_token = true;
        } else if ch == '\\' {
            if let Some(next) = chars.next() {
                current.push(next);
                in_token = true;
            }
        } else if ch.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut current));
                in_token = false;
            }
        } else {
            current.push(ch);
            in_token = true;
        }
    }
    if quote.is_some() {
        return Err("Unclosed quote in command template.".to_string());
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

async fn git_text(runner: &GitRunner, path: &Path, args: &[&str]) -> Result<String, String> {
    runner
        .run(path, args)
        .await
        .map(|output| output.stdout)
        .map_err(|_| "Could not read source-control context.".to_string())
}

fn bounded(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let clipped = value.chars().take(max).collect::<String>();
    format!("{clipped}\n\n[truncated]")
}

fn build_commit_prompt(branch: &str, files: &str, patch: &str) -> String {
    format!(
        "You are generating a single git commit message.\n\
Return only the commit message text. Do not include a preamble, quotes, or code fences.\n\n\
Rules:\n\
- First line: imperative mood, <= 72 chars, no trailing period.\n\
- Optional body: blank line, then short wrapped bullet points or prose explaining WHY.\n\
- Capture the primary user-visible or developer-visible change.\n\
- Use only the staged changes below as context.\n\
- Do not include Co-authored-by or other git trailers.\n\n\
Branch: {}\n\nStaged files:\n{}\n\nStaged patch:\n```diff\n{}\n```",
        if branch.trim().is_empty() {
            "(detached)"
        } else {
            branch.trim()
        },
        bounded(files, 6_000),
        bounded(patch, MAX_PATCH_CHARS)
    )
}

#[allow(clippy::too_many_arguments)]
fn build_pull_request_prompt(
    branch: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
    commits: &str,
    changed_files: &str,
    patch: &str,
) -> String {
    format!(
        "You are generating pull request details.\n\
Return ONLY compact JSON with this exact shape:\n\
{{\"base\":\"branch-name\",\"title\":\"short title\",\"body\":\"markdown description\",\"draft\":false}}\n\n\
Rules:\n\
- Use the branch diff and commits below as source of truth.\n\
- Keep the base branch as the current base unless the diff clearly targets a different branch.\n\
- Title: concise, specific, no trailing period.\n\
- Body: useful Markdown summary for reviewers. Include testing notes only when evidence exists.\n\
- Preserve any review template headings, required sections, and checklists.\n\
- Leave genuinely unknown template items as TODO or unchecked instead of deleting them.\n\
- Do not include labels, reviewers, code fences, prose, or keys beyond base/title/body/draft.\n\n\
Head branch: {branch}\nCurrent base: {base}\nCurrent title: {title}\n\
Current description: {body}\nCurrent draft: {draft}\n\n\
Commits:\n{}\n\nChanged files:\n{}\n\nPatch:\n```diff\n{}\n```",
        bounded(commits, 8_000),
        bounded(changed_files, 8_000),
        bounded(patch, MAX_PATCH_CHARS)
    )
}

fn clean_commit_message(raw: &str) -> String {
    let mut value = raw.trim();
    if value.starts_with("```") && value.ends_with("```") {
        value = value
            .split_once('\n')
            .map(|(_, body)| body)
            .unwrap_or(value);
        value = value.strip_suffix("```").unwrap_or(value).trim();
    }
    let mut lines = value.lines();
    let subject = lines
        .next()
        .unwrap_or("Update project files")
        .trim()
        .trim_matches('"')
        .trim_end_matches('.')
        .chars()
        .take(72)
        .collect::<String>();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let subject = if subject.is_empty() {
        "Update project files"
    } else {
        &subject
    };
    if body.is_empty() {
        subject.to_string()
    } else {
        format!("{subject}\n\n{body}")
    }
}

fn parse_pull_request_fields(
    raw: &str,
    fallback_base: &str,
    fallback_title: &str,
    fallback_body: &str,
    fallback_draft: bool,
) -> Result<GeneratedPullRequestFields, String> {
    let trimmed = raw.trim();
    let json = trimmed
        .find('{')
        .zip(trimmed.rfind('}'))
        .filter(|(start, end)| end > start)
        .map(|(start, end)| &trimmed[start..=end])
        .unwrap_or(trimmed);
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| "The agent returned invalid pull request JSON.".to_string())?;
    let title = value
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_title)
        .trim_end_matches('.')
        .to_string();
    Ok(GeneratedPullRequestFields {
        base: value
            .get("base")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback_base)
            .to_string(),
        title: if title.is_empty() {
            "Update project files".to_string()
        } else {
            title
        },
        body: value
            .get("body")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback_body)
            .trim_end()
            .to_string(),
        draft: value
            .get("draft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(fallback_draft),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn tokenizes_without_shell_expansion_and_rejects_unclosed_quotes() {
        assert_eq!(
            tokenize_command(r#"tool --flag "two words" '$HOME' \*.rs"#).unwrap(),
            ["tool", "--flag", "two words", "$HOME", "*.rs"]
        );
        assert!(tokenize_command("tool 'oops").is_err());
    }

    #[test]
    fn custom_command_uses_stdin_without_prompt_placeholder() {
        let plan = plan_custom_command("ollama run qwen", "--format text", "large prompt").unwrap();
        assert_eq!(plan.binary, "ollama");
        assert_eq!(plan.args, ["run", "qwen", "--format", "text"]);
        assert_eq!(plan.stdin_payload.as_deref(), Some("large prompt"));
    }

    #[test]
    fn action_templates_replace_known_variables_and_leave_unknown_visible() {
        let mut request = GenerationRequest {
            operation: TextOperation::CommitMessage,
            settings: SourceControlAiSetting::default(),
            repo_overrides: None,
            agent_command_overrides: HashMap::new(),
            environment: Vec::new(),
        };
        request
            .settings
            .actions
            .get_mut("commitMessage")
            .unwrap()
            .command_input_template =
            "{basePrompt}\nBranch {branch}\nUnknown {missing}".to_string();
        assert_eq!(
            render_action_prompt(&request, "BASE", &[("branch", "feature")]),
            "BASE\nBranch feature\nUnknown {missing}"
        );
    }

    #[test]
    fn parses_fenced_pull_request_json_and_falls_back_safely() {
        let fields = parse_pull_request_fields(
            "```json\n{\"title\":\"Clone Orca.\",\"body\":\"Summary\\n\",\"draft\":true}\n```",
            "main",
            "",
            "",
            false,
        )
        .unwrap();
        assert_eq!(fields.base, "main");
        assert_eq!(fields.title, "Clone Orca");
        assert_eq!(fields.body, "Summary");
        assert!(fields.draft);
    }

    async fn git_fixture() -> (TempDir, std::path::PathBuf) {
        let temp = TempDir::new().unwrap();
        let path = temp.path().to_path_buf();
        let runner = GitRunner::new();
        runner
            .run(&path, &["init", "--initial-branch=main"])
            .await
            .unwrap();
        runner
            .run(&path, &["config", "user.email", "test@example.com"])
            .await
            .unwrap();
        runner
            .run(&path, &["config", "user.name", "Test"])
            .await
            .unwrap();
        std::fs::write(path.join("README.md"), "before\n").unwrap();
        runner.run(&path, &["add", "README.md"]).await.unwrap();
        runner
            .run(&path, &["commit", "-m", "initial"])
            .await
            .unwrap();
        (temp, path)
    }

    fn custom_request(operation: TextOperation, command: &str) -> GenerationRequest {
        let mut settings = SourceControlAiSetting {
            agent_id: "custom".to_string(),
            custom_agent_command: command.to_string(),
            ..SourceControlAiSetting::default()
        };
        settings
            .actions
            .get_mut(operation.action_id())
            .unwrap()
            .agent_id = Some("custom".to_string());
        GenerationRequest {
            operation,
            settings,
            repo_overrides: None,
            agent_command_overrides: HashMap::new(),
            environment: Vec::new(),
        }
    }

    #[tokio::test]
    async fn commit_generation_reads_staged_context_and_applies_safe_output_cleanup() {
        let (_temp, path) = git_fixture().await;
        std::fs::write(path.join("README.md"), "after\n").unwrap();
        GitRunner::new()
            .run(&path, &["add", "README.md"])
            .await
            .unwrap();
        let request = custom_request(
            TextOperation::CommitMessage,
            "/bin/sh -c 'cat >/dev/null; printf \"Update staged README.\\n\"'",
        );

        let generated = generate_commit_message(&path, request).await.unwrap();
        assert_eq!(generated, "Update staged README");
    }

    #[tokio::test]
    async fn pull_request_generation_uses_branch_comparison_and_parses_json() {
        let (_temp, path) = git_fixture().await;
        let runner = GitRunner::new();
        runner
            .run(&path, &["checkout", "-b", "feature/generated"])
            .await
            .unwrap();
        std::fs::write(path.join("README.md"), "feature\n").unwrap();
        runner.run(&path, &["add", "README.md"]).await.unwrap();
        runner
            .run(&path, &["commit", "-m", "change readme"])
            .await
            .unwrap();
        let request = custom_request(
            TextOperation::PullRequest,
            "/bin/sh -c 'cat >/dev/null; printf \"%s\" \"{\\\"base\\\":\\\"main\\\",\\\"title\\\":\\\"Improve docs.\\\",\\\"body\\\":\\\"Summary\\\",\\\"draft\\\":true}\"'",
        );

        let fields = generate_pull_request_fields(&path, "main", "", "", false, request)
            .await
            .unwrap();
        assert_eq!(fields.base, "main");
        assert_eq!(fields.title, "Improve docs");
        assert_eq!(fields.body, "Summary");
        assert!(fields.draft);
    }
}
#[test]
fn parses_dynamic_model_catalog_formats() {
    assert_eq!(
        parse_discovered_models(
            "codex",
            r#"{"models":[{"slug":"gpt-5.5"},{"slug":"gpt-5.4"}]}"#
        ),
        vec!["gpt-5.5", "gpt-5.4"]
    );
    assert_eq!(
        parse_discovered_models("opencode", "openai/gpt-5.4\nbad model\nopenai/gpt-5.4\n"),
        vec!["openai/gpt-5.4"]
    );
    assert_eq!(
        parse_discovered_models(
            "pi",
            "provider model context input thinking images\nopenai gpt-5.4 128k text yes no\n"
        ),
        vec!["openai/gpt-5.4"]
    );
    assert_eq!(
        parse_discovered_models("cursor", "auto - Auto (default)\ngpt-5.4 - GPT 5.4\n"),
        vec!["auto", "gpt-5.4"]
    );
    assert_eq!(
        parse_discovered_models("antigravity", "Gemini 3.5 Flash (High)\n"),
        vec!["Gemini 3.5 Flash (High)"]
    );
}
