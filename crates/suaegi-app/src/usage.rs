//! Local Claude, Codex, and OpenCode usage-log scanning.
//!
//! The scanner is opt-in and read-only. It never uploads prompts or transcript
//! text; only token counters, model names, dates, and local project labels are
//! retained in memory for the settings view.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageProvider {
    Claude,
    Codex,
    OpenCode,
}

impl UsageProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DailyUsage {
    pub day: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsage {
    pub provider: UsageProvider,
    pub enabled: bool,
    pub sessions: usize,
    pub events: usize,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
    pub top_model: Option<String>,
    pub top_project: Option<String>,
    pub daily: Vec<DailyUsage>,
    pub error: Option<String>,
}

impl ProviderUsage {
    fn disabled(provider: UsageProvider) -> Self {
        Self {
            provider,
            enabled: false,
            sessions: 0,
            events: 0,
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            cache_write_tokens: 0,
            total_tokens: 0,
            estimated_cost_usd: None,
            top_model: None,
            top_project: None,
            daily: Vec::new(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageSnapshot {
    pub providers: Vec<ProviderUsage>,
    pub completed_at_unix_ms: u64,
}

impl UsageSnapshot {
    pub fn provider(&self, provider: UsageProvider) -> Option<&ProviderUsage> {
        self.providers
            .iter()
            .find(|entry| entry.provider == provider)
    }

    pub fn total_tokens(&self) -> u64 {
        self.providers.iter().map(|entry| entry.total_tokens).sum()
    }

    pub fn active_days(&self) -> usize {
        self.providers
            .iter()
            .flat_map(|entry| entry.daily.iter().map(|day| day.day.as_str()))
            .collect::<HashSet<_>>()
            .len()
    }

    pub fn cache_share(&self) -> Option<f64> {
        let cached = self
            .providers
            .iter()
            .map(|entry| entry.cached_input_tokens + entry.cache_write_tokens)
            .sum::<u64>();
        let total = self.total_tokens();
        (total > 0).then_some(cached as f64 / total as f64)
    }

    pub fn estimated_cost_usd(&self) -> Option<f64> {
        let mut any = false;
        let mut total = 0.0;
        for provider in &self.providers {
            if let Some(cost) = provider.estimated_cost_usd {
                any = true;
                total += cost;
            }
        }
        any.then_some(total)
    }
}

#[derive(Default)]
struct Accumulator {
    sessions: HashSet<String>,
    events: usize,
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    cache_write: u64,
    total: u64,
    cost: Option<f64>,
    models: HashMap<String, u64>,
    projects: HashMap<String, u64>,
    daily: BTreeMap<String, DailyUsage>,
}

struct UsageEvent<'a> {
    session: &'a str,
    day: &'a str,
    model: Option<&'a str>,
    project: Option<&'a str>,
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    cache_write: u64,
    total: u64,
    cost: Option<f64>,
}

impl Accumulator {
    fn add(&mut self, event: UsageEvent<'_>) {
        self.sessions.insert(event.session.to_string());
        self.events += 1;
        self.input = self.input.saturating_add(event.input);
        self.cached = self.cached.saturating_add(event.cached.min(event.input));
        self.output = self.output.saturating_add(event.output);
        self.reasoning = self.reasoning.saturating_add(event.reasoning);
        self.cache_write = self.cache_write.saturating_add(event.cache_write);
        self.total = self.total.saturating_add(event.total);
        if let Some(cost) = event.cost.filter(|cost| cost.is_finite() && *cost >= 0.0) {
            self.cost = Some(self.cost.unwrap_or(0.0) + cost);
        }
        if let Some(model) = event.model.filter(|value| !value.trim().is_empty()) {
            *self.models.entry(model.to_string()).or_default() += event.total;
        }
        if let Some(project) = event.project.filter(|value| !value.trim().is_empty()) {
            *self.projects.entry(project.to_string()).or_default() += event.total;
        }
        let daily = self
            .daily
            .entry(event.day.to_string())
            .or_insert_with(|| DailyUsage {
                day: event.day.to_string(),
                ..DailyUsage::default()
            });
        daily.input_tokens += event.input;
        daily.cached_input_tokens += event.cached.min(event.input);
        daily.output_tokens += event.output;
        daily.reasoning_tokens += event.reasoning;
        daily.cache_write_tokens += event.cache_write;
        daily.total_tokens += event.total;
    }

    fn finish(self, provider: UsageProvider, error: Option<String>) -> ProviderUsage {
        let top = |values: HashMap<String, u64>| {
            values
                .into_iter()
                .max_by(|(left_name, left), (right_name, right)| {
                    left.cmp(right).then_with(|| right_name.cmp(left_name))
                })
                .map(|(name, _)| name)
        };
        ProviderUsage {
            provider,
            enabled: true,
            sessions: self.sessions.len(),
            events: self.events,
            input_tokens: self.input,
            cached_input_tokens: self.cached,
            output_tokens: self.output,
            reasoning_tokens: self.reasoning,
            cache_write_tokens: self.cache_write,
            total_tokens: self.total,
            estimated_cost_usd: self.cost,
            top_model: top(self.models),
            top_project: top(self.projects),
            daily: self.daily.into_values().collect(),
            error,
        }
    }
}

fn walk_files(root: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk_files(&path, extension, files);
        } else if kind.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some(extension)
        {
            files.push(path);
        }
    }
}

fn number(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            value
                .and_then(serde_json::Value::as_i64)
                .and_then(|value| u64::try_from(value).ok())
        })
        .unwrap_or(0)
}

fn day(timestamp: Option<&str>) -> &str {
    timestamp
        .filter(|value| value.len() >= 10)
        .map(|value| &value[..10])
        .unwrap_or("Unknown")
}

fn project_label(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?;
    let parts = cwd
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts[parts.len().saturating_sub(2)..].join("/"))
    }
}

#[derive(Clone, Copy)]
struct ClaudePricing {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    long_context: bool,
}

fn claude_pricing(model: Option<&str>) -> Option<ClaudePricing> {
    let mut model = model?.trim().to_ascii_lowercase();
    if let Some(unprefixed) = model
        .strip_prefix("anthropic/")
        .or_else(|| model.strip_prefix("anthropic:"))
    {
        model = unprefixed.to_string();
    }
    let model = model.replace('.', "-");
    let current_opus = ClaudePricing {
        input: 5.0,
        output: 25.0,
        cache_read: 0.5,
        cache_write: 6.25,
        long_context: false,
    };
    let legacy_opus = ClaudePricing {
        input: 15.0,
        output: 75.0,
        cache_read: 1.5,
        cache_write: 18.75,
        long_context: false,
    };
    let sonnet = ClaudePricing {
        input: 3.0,
        output: 15.0,
        cache_read: 0.3,
        cache_write: 3.75,
        long_context: model.contains("sonnet-4"),
    };
    if matches!(
        model.as_str(),
        "model_placeholder_m26" | "claude-opus-4-8-thinking" | "claude-opus-4-6-thinking"
    ) || ["opus-4-8", "opus-4-7", "opus-4-6", "opus-4-5"]
        .iter()
        .any(|version| model.contains(version))
    {
        Some(current_opus)
    } else if model.contains("opus-4-1") {
        Some(legacy_opus)
    } else if model.contains("opus-4") {
        // Current Orca pricing intentionally treats unknown future Opus 4 point
        // releases as the current lower-price family.
        let legacy_base = model.ends_with("opus-4")
            || model.ends_with("opus-4-thinking")
            || model.split("opus-4-").nth(1).is_some_and(|suffix| {
                let date = suffix.chars().take(8).collect::<String>();
                date.len() == 8 && date.chars().all(|c| c.is_ascii_digit())
            });
        Some(if legacy_base {
            legacy_opus
        } else {
            current_opus
        })
    } else if model == "model_placeholder_m35"
        || model.contains("sonnet-4")
        || model.contains("sonnet-3-7")
        || model.contains("sonnet-3-5")
        || model.contains("3-5-sonnet")
    {
        Some(sonnet)
    } else if model.contains("haiku-4-5") {
        Some(ClaudePricing {
            input: 1.0,
            output: 5.0,
            cache_read: 0.1,
            cache_write: 1.25,
            long_context: false,
        })
    } else if model.contains("haiku-3-5") || model.contains("3-5-haiku") {
        Some(ClaudePricing {
            input: 0.8,
            output: 4.0,
            cache_read: 0.08,
            cache_write: 1.0,
            long_context: false,
        })
    } else if model.contains("haiku-3") {
        Some(ClaudePricing {
            input: 0.25,
            output: 1.25,
            cache_read: 0.03,
            cache_write: 0.3,
            long_context: false,
        })
    } else {
        None
    }
}

fn tiered(tokens: u64, base: f64, threshold: u64, above: f64) -> f64 {
    let below = tokens.min(threshold);
    let above_tokens = tokens.saturating_sub(threshold);
    below as f64 * base + above_tokens as f64 * above
}

fn estimate_claude_cost(
    model: Option<&str>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> Option<f64> {
    let pricing = claude_pricing(model)?;
    let raw = if pricing.long_context {
        tiered(input, pricing.input, 200_000, 6.0)
            + tiered(output, pricing.output, 200_000, 22.5)
            + tiered(cache_read, pricing.cache_read, 200_000, 0.6)
            + tiered(cache_write, pricing.cache_write, 200_000, 7.5)
    } else {
        input as f64 * pricing.input
            + output as f64 * pricing.output
            + cache_read as f64 * pricing.cache_read
            + cache_write as f64 * pricing.cache_write
    };
    Some(raw / 1_000_000.0)
}

#[derive(Clone, Copy)]
struct CodexPricing {
    input: f64,
    cached: f64,
    output: f64,
    long_input: Option<f64>,
    long_cached: Option<f64>,
    long_output: Option<f64>,
}

fn codex_pricing(model: Option<&str>) -> Option<CodexPricing> {
    let mut model = model?.trim().to_ascii_lowercase();
    if let Some(open) = model.rfind('(') {
        let tier = model.get(open + 1..model.len().saturating_sub(1))?;
        if model.ends_with(')')
            && ["minimal", "low", "medium", "high", "xhigh", "auto", "none"].contains(&tier)
        {
            model.truncate(open);
        }
    }
    for _ in 0..4 {
        let Some(tier) = ["minimal", "low", "medium", "high", "xhigh", "auto", "none"]
            .into_iter()
            .find(|tier| model.ends_with(&format!("-{tier}")))
        else {
            break;
        };
        model.truncate(model.len() - tier.len() - 1);
    }
    let simple = |input, cached, output| CodexPricing {
        input,
        cached,
        output,
        long_input: None,
        long_cached: None,
        long_output: None,
    };
    let matches_family = |family: &str| model == family || model.starts_with(&format!("{family}-"));
    if model == "gpt-5" || model == "gpt-5-codex" || matches_family("gpt-5.1") {
        Some(simple(1.25, 0.125, 10.0))
    } else if matches_family("gpt-5.2") || matches_family("gpt-5.3") {
        Some(simple(1.75, 0.175, 14.0))
    } else if matches_family("gpt-5.4-mini") {
        Some(simple(0.75, 0.075, 4.5))
    } else if matches_family("gpt-5.4-nano") {
        Some(simple(0.2, 0.02, 1.25))
    } else if matches_family("gpt-5.4-pro") || matches_family("gpt-5.5-pro") {
        Some(CodexPricing {
            input: 30.0,
            cached: 30.0,
            output: 180.0,
            long_input: Some(60.0),
            long_cached: Some(60.0),
            long_output: Some(270.0),
        })
    } else if matches_family("gpt-5.4") {
        Some(CodexPricing {
            input: 2.5,
            cached: 0.25,
            output: 15.0,
            long_input: Some(5.0),
            long_cached: Some(0.5),
            long_output: Some(22.5),
        })
    } else if matches_family("gpt-5.5") {
        Some(CodexPricing {
            input: 5.0,
            cached: 0.5,
            output: 30.0,
            long_input: Some(10.0),
            long_cached: Some(1.0),
            long_output: Some(45.0),
        })
    } else {
        None
    }
}

fn estimate_codex_cost(model: Option<&str>, input: u64, cached: u64, output: u64) -> Option<f64> {
    let pricing = codex_pricing(model)?;
    let cached = cached.min(input);
    let uncached = input.saturating_sub(cached);
    let price = |tokens: u64, base: f64, above: Option<f64>| {
        above.map_or(tokens as f64 * base, |above| {
            tiered(tokens, base, 272_000, above)
        })
    };
    Some(
        (price(uncached, pricing.input, pricing.long_input)
            + price(cached, pricing.cached, pricing.long_cached)
            + price(output, pricing.output, pricing.long_output))
            / 1_000_000.0,
    )
}

fn unique_existing_config_dirs(
    fallback_name: &str,
    environment_name: &str,
    extra: Vec<PathBuf>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(fallback_name));
    }
    if let Some(path) = std::env::var_os(environment_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        roots.push(path);
    }
    roots.extend(extra);
    roots.retain(|path| path.is_dir());
    roots.sort();
    roots.dedup();
    roots
}

fn scan_claude(extra_config_dirs: Vec<PathBuf>) -> ProviderUsage {
    let Some(_home) = dirs::home_dir() else {
        return Accumulator::default().finish(
            UsageProvider::Claude,
            Some("Could not locate the home directory.".into()),
        );
    };
    let mut files = Vec::new();
    for config_dir in unique_existing_config_dirs(".claude", "CLAUDE_CONFIG_DIR", extra_config_dirs)
    {
        walk_files(&config_dir.join("projects"), "jsonl", &mut files);
        walk_files(&config_dir.join("transcripts"), "jsonl", &mut files);
    }
    files.sort();
    files.dedup();
    let mut totals = Accumulator::default();
    let mut dedupe = HashSet::new();
    for path in files {
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let fallback = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown");
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value.get("type").and_then(|value| value.as_str()) != Some("assistant") {
                continue;
            }
            let Some(message) = value.get("message") else {
                continue;
            };
            let Some(usage) = message.get("usage") else {
                continue;
            };
            let key = message
                .get("id")
                .and_then(|value| value.as_str())
                .map(|id| {
                    format!(
                        "{id}:{}",
                        value
                            .get("requestId")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                    )
                })
                .or_else(|| {
                    value
                        .get("uuid")
                        .and_then(|value| value.as_str())
                        .map(|uuid| format!("uuid:{uuid}"))
                });
            if key.as_ref().is_some_and(|key| !dedupe.insert(key.clone())) {
                continue;
            }
            let input = number(usage.get("input_tokens"));
            let output = number(usage.get("output_tokens"));
            let cached = number(usage.get("cache_read_input_tokens"));
            let cache_write = number(usage.get("cache_creation_input_tokens"));
            let total = input + output + cached + cache_write;
            if total == 0 {
                continue;
            }
            let session = value
                .get("sessionId")
                .and_then(|value| value.as_str())
                .unwrap_or(fallback);
            let timestamp = value.get("timestamp").and_then(|value| value.as_str());
            let project = project_label(value.get("cwd").and_then(|value| value.as_str()));
            let model = message.get("model").and_then(|value| value.as_str());
            totals.add(UsageEvent {
                session,
                day: day(timestamp),
                model,
                project: project.as_deref(),
                input,
                cached,
                output,
                reasoning: 0,
                cache_write,
                total,
                cost: estimate_claude_cost(model, input, output, cached, cache_write),
            });
        }
    }
    totals.finish(UsageProvider::Claude, None)
}

#[derive(Clone, Copy, Default)]
struct CodexTotals {
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

fn codex_totals(value: &serde_json::Value) -> CodexTotals {
    let input = number(value.get("input_tokens"));
    let output = number(value.get("output_tokens"));
    CodexTotals {
        input,
        cached: number(
            value
                .get("cached_input_tokens")
                .or_else(|| value.get("cache_read_input_tokens")),
        )
        .min(input),
        output,
        reasoning: number(value.get("reasoning_output_tokens")),
        total: {
            let total = number(value.get("total_tokens"));
            if total == 0 {
                input + output
            } else {
                total
            }
        },
    }
}

fn subtract(current: CodexTotals, previous: CodexTotals) -> CodexTotals {
    CodexTotals {
        input: current.input.saturating_sub(previous.input),
        cached: current.cached.saturating_sub(previous.cached),
        output: current.output.saturating_sub(previous.output),
        reasoning: current.reasoning.saturating_sub(previous.reasoning),
        total: current.total.saturating_sub(previous.total),
    }
}

fn scan_codex(extra_home_dirs: Vec<PathBuf>) -> ProviderUsage {
    let Some(_home) = dirs::home_dir() else {
        return Accumulator::default().finish(
            UsageProvider::Codex,
            Some("Could not locate the home directory.".into()),
        );
    };
    let mut config_dirs = unique_existing_config_dirs(".codex", "CODEX_HOME", extra_home_dirs);
    if let Some(data) = dirs::data_dir() {
        // Continue discovering Orca's managed home so existing users see the
        // same local history while migrating to the Rust clone.
        config_dirs.push(data.join("orca/codex-home"));
        config_dirs.push(data.join("suaegi/codex-home"));
    }
    config_dirs.sort();
    config_dirs.dedup();
    let mut files = Vec::new();
    for config_dir in config_dirs {
        walk_files(&config_dir.join("sessions"), "jsonl", &mut files);
    }
    files.sort();
    files.dedup();
    let mut aggregate = Accumulator::default();
    let mut dedupe = HashSet::new();
    for path in files {
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut session = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mut cwd = None::<String>;
        let mut model = None::<String>;
        let mut previous = CodexTotals::default();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let kind = value.get("type").and_then(|value| value.as_str());
            let Some(payload) = value.get("payload") else {
                continue;
            };
            if kind == Some("session_meta") {
                if let Some(id) = payload.get("id").and_then(|value| value.as_str()) {
                    session = id.to_string();
                }
                cwd = payload
                    .get("cwd")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .or(cwd);
                continue;
            }
            if kind == Some("turn_context") {
                cwd = payload
                    .get("cwd")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .or(cwd);
                model = payload
                    .get("model")
                    .or_else(|| payload.get("model_slug"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .or(model);
                continue;
            }
            if kind != Some("event_msg")
                || payload.get("type").and_then(|value| value.as_str()) != Some("token_count")
            {
                continue;
            }
            let Some(info) = payload.get("info").and_then(|value| value.as_object()) else {
                continue;
            };
            let total = info.get("total_token_usage").map(codex_totals);
            let delta = info
                .get("last_token_usage")
                .map(codex_totals)
                .or_else(|| total.map(|total| subtract(total, previous)));
            if let Some(total) = total {
                previous = total;
            }
            let Some(delta) = delta else {
                continue;
            };
            if delta.total == 0 && delta.input == 0 && delta.output == 0 {
                continue;
            }
            let timestamp = value
                .get("timestamp")
                .and_then(|value| value.as_str())
                .unwrap_or("Unknown");
            let event_key = format!(
                "{timestamp}:{}:{}:{}:{}:{}",
                delta.input, delta.cached, delta.output, delta.reasoning, delta.total
            );
            if !dedupe.insert(event_key) {
                continue;
            }
            let project = project_label(cwd.as_deref());
            let event_model = payload
                .get("model")
                .or_else(|| payload.get("model_slug"))
                .and_then(|value| value.as_str())
                .or(model.as_deref());
            aggregate.add(UsageEvent {
                session: &session,
                day: day(Some(timestamp)),
                model: event_model,
                project: project.as_deref(),
                input: delta.input,
                cached: delta.cached,
                output: delta.output,
                reasoning: delta.reasoning,
                cache_write: 0,
                total: delta.total,
                cost: estimate_codex_cost(event_model, delta.input, delta.cached, delta.output),
            });
        }
    }
    aggregate.finish(UsageProvider::Codex, None)
}

fn open_code_databases() -> Vec<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")))
        .map(|base| base.join("opencode"));
    if let Some(path) = std::env::var_os("OPENCODE_DB").map(PathBuf::from) {
        return path.is_file().then_some(path).into_iter().collect();
    }
    let Some(base) = base else {
        return Vec::new();
    };
    std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| {
                        name == "opencode.db"
                            || (name.starts_with("opencode-") && name.ends_with(".db"))
                    })
        })
        .collect()
}

fn scan_opencode() -> ProviderUsage {
    let mut aggregate = Accumulator::default();
    let mut errors = Vec::new();
    for database in open_code_databases() {
        let query = "SELECT id, COALESCE(model,''), COALESCE(directory,''), \
            strftime('%Y-%m-%d', CASE WHEN COALESCE(time_updated,time_created)>100000000000 \
            THEN COALESCE(time_updated,time_created)/1000 ELSE COALESCE(time_updated,time_created) END, \
            'unixepoch','localtime'), tokens_input, tokens_cache_read, tokens_output, \
            tokens_reasoning, tokens_input+tokens_output+tokens_reasoning, cost \
            FROM session WHERE tokens_input+tokens_output+tokens_reasoning+tokens_cache_read>0";
        let output = Command::new("/usr/bin/sqlite3")
            .args(["-readonly", "-batch", "-noheader", "-separator", "\t"])
            .arg(&database)
            .arg(query)
            .output();
        let Ok(output) = output else {
            errors.push(format!("Could not read {}", database.display()));
            continue;
        };
        if !output.status.success() {
            errors.push(String::from_utf8_lossy(&output.stderr).trim().to_string());
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() < 10 {
                continue;
            }
            let parse = |index: usize| {
                fields
                    .get(index)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            let input = parse(4);
            let cached = parse(5).min(input);
            let output = parse(6);
            let reasoning = parse(7);
            let total = parse(8).max(input + output + reasoning);
            let project = project_label(Some(fields[2]));
            aggregate.add(UsageEvent {
                session: fields[0],
                day: fields[3],
                model: Some(fields[1]),
                project: project.as_deref(),
                input,
                cached,
                output,
                reasoning,
                cache_write: 0,
                total,
                cost: fields[9].parse::<f64>().ok(),
            });
        }
    }
    aggregate.finish(
        UsageProvider::OpenCode,
        (!errors.is_empty()).then(|| errors.join("\n")),
    )
}

pub async fn scan(
    enabled: [bool; 3],
    claude_config_dirs: Vec<PathBuf>,
    codex_home_dirs: Vec<PathBuf>,
) -> UsageSnapshot {
    tokio::task::spawn_blocking(move || {
        let providers = [
            (UsageProvider::Claude, enabled[0]),
            (UsageProvider::Codex, enabled[1]),
            (UsageProvider::OpenCode, enabled[2]),
        ]
        .into_iter()
        .map(|(provider, enabled)| {
            if !enabled {
                return ProviderUsage::disabled(provider);
            }
            match provider {
                UsageProvider::Claude => scan_claude(claude_config_dirs.clone()),
                UsageProvider::Codex => scan_codex(codex_home_dirs.clone()),
                UsageProvider::OpenCode => scan_opencode(),
            }
        })
        .collect();
        UsageSnapshot {
            providers,
            completed_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
        }
    })
    .await
    .unwrap_or_else(|_| UsageSnapshot {
        providers: Vec::new(),
        completed_at_unix_ms: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_builds_daily_and_cache_summary() {
        let mut accumulator = Accumulator::default();
        accumulator.add(UsageEvent {
            session: "one",
            day: "2026-07-29",
            model: Some("gpt"),
            project: Some("james/suaegi"),
            input: 100,
            cached: 40,
            output: 20,
            reasoning: 5,
            cache_write: 0,
            total: 120,
            cost: None,
        });
        let provider = accumulator.finish(UsageProvider::Codex, None);
        assert_eq!(provider.sessions, 1);
        assert_eq!(provider.total_tokens, 120);
        assert_eq!(provider.cached_input_tokens, 40);
        assert_eq!(provider.top_model.as_deref(), Some("gpt"));
        assert_eq!(provider.daily[0].day, "2026-07-29");
    }

    #[test]
    fn overview_combines_enabled_providers() {
        let snapshot = UsageSnapshot {
            providers: vec![
                Accumulator::default().finish(UsageProvider::Claude, None),
                ProviderUsage::disabled(UsageProvider::Codex),
            ],
            completed_at_unix_ms: 1,
        };
        assert_eq!(snapshot.total_tokens(), 0);
        assert_eq!(snapshot.active_days(), 0);
        assert_eq!(snapshot.cache_share(), None);
    }

    #[test]
    fn claude_pricing_matches_orca_long_context_and_alias_rules() {
        let cost = estimate_claude_cost(
            Some("claude-sonnet-4.6-20260217"),
            300_000,
            300_000,
            300_000,
            300_000,
        );
        assert_eq!(cost.map(|value| (value * 1000.0).round()), Some(8070.0));
        assert!(estimate_claude_cost(Some("unknown-model"), 1_000, 100, 0, 0).is_none());
    }

    #[test]
    fn codex_pricing_clamps_cached_input_and_normalizes_reasoning_suffix() {
        let cost = estimate_codex_cost(Some("gpt-5-high"), 1_000, 400, 250).unwrap();
        assert!((cost - 0.0033).abs() < 0.000_000_1);
        let current = estimate_codex_cost(Some("gpt-5.4-mini-xhigh"), 2_000, 1_000, 500);
        assert!(current.is_some_and(|value| value > 0.0));
        assert!(estimate_codex_cost(Some("gpt-5.999"), 1_000, 0, 100).is_none());
    }
}
