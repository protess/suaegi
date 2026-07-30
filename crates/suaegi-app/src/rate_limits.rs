//! Provider quota fetching used by Orca's status-bar usage surface.
//!
//! Gemini access is deliberately opt-in because it reads credentials created
//! by Gemini CLI or OpenCode. Tokens are never retained in app state or logs.

use chrono::DateTime;
use regex::Regex;
use serde_json::Value;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOAD_CODE_ASSIST_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const RETRIEVE_QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitProvider {
    Claude,
    Codex,
    Gemini,
    Kimi,
    Grok,
    OpenCodeGo,
    MiniMax,
    Antigravity,
}

impl RateLimitProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::Kimi => "Kimi",
            Self::Grok => "Grok",
            Self::OpenCodeGo => "OpenCode Go",
            Self::MiniMax => "MiniMax",
            Self::Antigravity => "Antigravity",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitStatus {
    Ok,
    Error,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitBucket {
    pub name: String,
    pub used_percent: u8,
    pub resets_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRateLimits {
    pub provider: RateLimitProvider,
    pub buckets: Vec<RateLimitBucket>,
    pub updated_at_unix_ms: u64,
    pub error: Option<String>,
    pub status: RateLimitStatus,
}

pub type GeminiRateLimits = ProviderRateLimits;

impl ProviderRateLimits {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::unavailable_for(RateLimitProvider::Gemini, message)
    }

    fn unavailable_for(provider: RateLimitProvider, message: impl Into<String>) -> Self {
        Self {
            provider,
            buckets: Vec::new(),
            updated_at_unix_ms: now_unix_ms(),
            error: Some(message.into()),
            status: RateLimitStatus::Unavailable,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self::failed_for(RateLimitProvider::Gemini, message)
    }

    fn failed_for(provider: RateLimitProvider, message: impl Into<String>) -> Self {
        Self {
            provider,
            buckets: Vec::new(),
            updated_at_unix_ms: now_unix_ms(),
            error: Some(message.into()),
            status: RateLimitStatus::Error,
        }
    }

    pub fn most_constrained(&self) -> Option<&RateLimitBucket> {
        self.buckets.iter().max_by_key(|bucket| bucket.used_percent)
    }

    pub fn from_runtime_value(provider: RateLimitProvider, value: &Value) -> Self {
        fn window(name: &str, value: Option<&Value>) -> Option<RateLimitBucket> {
            let value = value?;
            let used = value
                .get("usedPercent")
                .or_else(|| value.get("used_percent"))
                .and_then(Value::as_f64)?;
            Some(RateLimitBucket {
                name: name.to_string(),
                used_percent: used.round().clamp(0.0, 100.0) as u8,
                resets_at_unix_ms: value
                    .get("resetsAt")
                    .or_else(|| value.get("resets_at"))
                    .and_then(parse_reset_value),
            })
        }

        let mut buckets = [
            ("5 hour", "session"),
            ("Weekly", "weekly"),
            ("Fable weekly", "fableWeekly"),
            ("Monthly", "monthly"),
        ]
        .into_iter()
        .filter_map(|(name, key)| window(name, value.get(key)))
        .collect::<Vec<_>>();
        if let Some(extra) = value.get("buckets").and_then(Value::as_array) {
            buckets.extend(extra.iter().filter_map(|bucket| {
                let name = bucket.get("name").and_then(Value::as_str)?;
                window(name, Some(bucket))
            }));
        }
        let status = match value.get("status").and_then(Value::as_str) {
            Some("ok") => RateLimitStatus::Ok,
            Some("error") => RateLimitStatus::Error,
            _ => RateLimitStatus::Unavailable,
        };
        Self {
            provider,
            buckets,
            updated_at_unix_ms: value
                .get("updatedAt")
                .or_else(|| value.get("updated_at"))
                .and_then(Value::as_u64)
                .unwrap_or_else(now_unix_ms),
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
            status,
        }
    }
}

fn parse_reset_value(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return Some(if number < 10_000_000_000 {
            number.saturating_mul(1_000)
        } else {
            number
        });
    }
    let text = value.as_str()?.trim();
    if let Ok(number) = text.parse::<u64>() {
        return Some(if number < 10_000_000_000 {
            number.saturating_mul(1_000)
        } else {
            number
        });
    }
    DateTime::parse_from_rfc3339(text)
        .ok()
        .and_then(|date| u64::try_from(date.timestamp_millis()).ok())
}

fn quota_window(name: &str, raw: Option<&Value>, used_keys: &[&str]) -> Option<RateLimitBucket> {
    let raw = raw?;
    let used = used_keys
        .iter()
        .find_map(|key| raw.get(*key).and_then(Value::as_f64))?;
    Some(RateLimitBucket {
        name: name.to_string(),
        used_percent: used.round().clamp(0.0, 100.0) as u8,
        resets_at_unix_ms: raw.get("resets_at").and_then(parse_reset_value),
    })
}

fn parse_claude_quota(value: &Value) -> Vec<RateLimitBucket> {
    let mut buckets = Vec::new();
    if let Some(bucket) = quota_window(
        "5 hour",
        value.get("five_hour"),
        &["utilization", "used_percentage"],
    ) {
        buckets.push(bucket);
    }
    if let Some(bucket) = quota_window(
        "Weekly",
        value.get("seven_day"),
        &["utilization", "used_percentage"],
    ) {
        buckets.push(bucket);
    }
    let scoped_fable = value
        .get("limits")
        .and_then(Value::as_array)
        .and_then(|limits| {
            limits.iter().find(|limit| {
                limit.get("kind").and_then(Value::as_str) == Some("weekly_scoped")
                    && limit
                        .pointer("/scope/model/display_name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case("fable"))
            })
        });
    let legacy_fable = ["fable_weekly", "fable_seven_day", "seven_day_fable"]
        .into_iter()
        .find_map(|key| value.get(key));
    if let Some(bucket) = quota_window(
        "Fable weekly",
        scoped_fable.or(legacy_fable),
        &["percent", "utilization", "used_percentage"],
    ) {
        buckets.push(bucket);
    }
    buckets
}

fn parse_codex_quota(value: &Value) -> Option<Vec<RateLimitBucket>> {
    value.get("plan_type")?.as_str()?;
    let rate_limit = value.get("rate_limit").and_then(Value::as_object);
    let mut buckets = Vec::new();
    for (name, key) in [("5 hour", "primary_window"), ("Weekly", "secondary_window")] {
        let Some(raw) = rate_limit
            .and_then(|rate_limit| rate_limit.get(key))
            .and_then(Value::as_object)
        else {
            continue;
        };
        if let Some(used) = raw.get("used_percent").and_then(Value::as_f64) {
            buckets.push(RateLimitBucket {
                name: name.to_string(),
                used_percent: used.round().clamp(0.0, 100.0) as u8,
                resets_at_unix_ms: raw.get("reset_at").and_then(parse_reset_value),
            });
        }
    }
    Some(buckets)
}

fn numeric(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite())
    })
}

fn kimi_window(name: &str, detail: Option<&Value>) -> Option<RateLimitBucket> {
    let detail = detail?;
    let limit = numeric(detail.get("limit"))?;
    let used = numeric(detail.get("used"))
        .or_else(|| numeric(detail.get("remaining")).map(|remaining| limit - remaining))?;
    if limit <= 0.0 {
        return None;
    }
    Some(RateLimitBucket {
        name: name.to_string(),
        used_percent: ((used / limit) * 100.0).round().clamp(0.0, 100.0) as u8,
        resets_at_unix_ms: detail
            .get("resetTime")
            .or_else(|| detail.get("resetAt"))
            .and_then(parse_reset_value),
    })
}

fn kimi_window_minutes(window: Option<&Value>) -> Option<u64> {
    let window = window?;
    let duration = numeric(window.get("duration"))?;
    let unit = window
        .get("timeUnit")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    let minutes = if unit.contains("SECOND") {
        duration / 60.0
    } else if unit.contains("HOUR") {
        duration * 60.0
    } else if unit.contains("DAY") {
        duration * 1_440.0
    } else {
        duration
    };
    Some(minutes.round().max(0.0) as u64)
}

fn parse_kimi_quota(value: &Value) -> Vec<RateLimitBucket> {
    let mut buckets = Vec::new();
    if let Some(weekly) = kimi_window("Weekly", value.get("usage")) {
        buckets.push(weekly);
    }
    let session = value
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|limit| {
            let minutes = kimi_window_minutes(limit.get("window"))?;
            let bucket = kimi_window("5 hour", limit.get("detail"))?;
            Some((minutes.abs_diff(300), bucket))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, bucket)| bucket);
    if let Some(session) = session {
        buckets.insert(0, session);
    }
    buckets
}

#[derive(Clone)]
struct GrokSession {
    access_token: String,
    user_id: Option<String>,
    expires_at_unix_ms: Option<u64>,
}

fn read_grok_session() -> Option<GrokSession> {
    let home = std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".grok")))?;
    let auth = read_json(&home.join("auth.json"))?;
    let object = auth.as_object()?;
    let mut fallback = None;
    let mut expired_preferred = None;
    for (issuer, entry) in object {
        let Some(token) = entry
            .get("key")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let expires_at_unix_ms = entry
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(parse_reset_time);
        let session = GrokSession {
            access_token: token,
            user_id: entry
                .get("user_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            expires_at_unix_ms,
        };
        let preferred = issuer == "https://auth.x.ai" || issuer.starts_with("https://auth.x.ai::");
        let fresh = session
            .expires_at_unix_ms
            .is_none_or(|expires| expires > now_unix_ms() + 5 * 60_000);
        if preferred && fresh {
            return Some(session);
        }
        if preferred {
            expired_preferred.get_or_insert(session);
        } else {
            fallback.get_or_insert(session);
        }
    }
    expired_preferred.or(fallback)
}

fn grok_period_end(config: &Value) -> Option<u64> {
    config
        .pointer("/currentPeriod/end")
        .or_else(|| config.get("billingPeriodEnd"))
        .and_then(parse_reset_value)
}

fn parse_grok_weekly(value: &Value) -> Option<RateLimitBucket> {
    let config = value.get("config").unwrap_or(value);
    let used = numeric(config.get("creditUsagePercent")).or_else(|| {
        let period = config.get("currentPeriod")?;
        let weekly = period.get("type").and_then(Value::as_str) == Some("USAGE_PERIOD_TYPE_WEEKLY");
        let same_bounds = period.get("start") == config.get("billingPeriodStart")
            && period.get("end") == config.get("billingPeriodEnd");
        (weekly && same_bounds).then_some(0.0)
    })?;
    Some(RateLimitBucket {
        name: "Weekly".to_string(),
        used_percent: used.round().clamp(0.0, 100.0) as u8,
        resets_at_unix_ms: grok_period_end(config),
    })
}

fn parse_grok_monthly(value: &Value) -> Option<RateLimitBucket> {
    let config = value.get("config").unwrap_or(value);
    let limit = numeric(config.pointer("/monthlyLimit/val"))?;
    let used = numeric(config.pointer("/used/val"))?;
    if limit <= 0.0 {
        return None;
    }
    Some(RateLimitBucket {
        name: "Monthly".to_string(),
        used_percent: ((used / limit) * 100.0).round().clamp(0.0, 100.0) as u8,
        resets_at_unix_ms: grok_period_end(config),
    })
}

fn normalize_opencode_cookie(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains(';')
        || trimmed.starts_with("auth=")
        || trimmed.starts_with("__Host-auth=")
    {
        return trimmed.to_string();
    }
    if trimmed.starts_with("Fe26.2**")
        || trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    {
        return format!("auth={trimmed}");
    }
    trimmed.to_string()
}

fn filter_opencode_cookie(raw: &str) -> String {
    raw.split(';')
        .filter_map(|pair| {
            let pair = pair.trim();
            let (name, value) = pair.split_once('=')?;
            (matches!(name.trim(), "auth" | "__Host-auth") && !value.trim().is_empty())
                .then(|| format!("{}={}", name.trim(), value.trim()))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_opencode_workspace_ids(text: &str) -> Vec<String> {
    let expression =
        Regex::new(r#"\bid\s*:\s*["']((?:wrk|wk)_[a-zA-Z0-9]+)["']"#).expect("fixed regex");
    let mut ids = Vec::new();
    for captures in expression.captures_iter(text) {
        let Some(id) = captures.get(1).map(|capture| capture.as_str().to_string()) else {
            continue;
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn top_level_number(object: &str, field: &str) -> Option<f64> {
    let expression = Regex::new(&format!(
        r"\b{}\b\s*:\s*(-?[0-9]+(?:\.[0-9]+)?)",
        regex::escape(field)
    ))
    .ok()?;
    let mut depth = 0_i32;
    for (index, character) in object.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ if depth == 1 => {
                if let Some(capture) = expression.captures(&object[index..]) {
                    if capture.get(0).is_some_and(|matched| matched.start() == 0) {
                        return capture.get(1)?.as_str().parse().ok();
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn opencode_usage_block<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let expression = Regex::new(&format!(r"\b{}\b\s*:", regex::escape(key))).ok()?;
    for key_match in expression.find_iter(text) {
        let search_start = key_match.end();
        let window_end = (search_start + 30).min(text.len());
        let Some(brace_offset) = text[search_start..window_end].find('{') else {
            continue;
        };
        let open = search_start + brace_offset;
        let mut depth = 0_i32;
        for (offset, character) in text[open..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let block = &text[open..open + offset + 1];
                        if top_level_number(block, "usagePercent").is_some()
                            && top_level_number(block, "resetInSec").is_some()
                        {
                            return Some(block);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
struct OpenCodeUsage {
    rolling_percent: f64,
    weekly_percent: f64,
    monthly_percent: Option<f64>,
    rolling_reset_seconds: f64,
    weekly_reset_seconds: f64,
    monthly_reset_seconds: Option<f64>,
}

fn parse_opencode_usage(text: &str) -> Option<OpenCodeUsage> {
    if text.is_empty() || text.len() > 10_000_000 {
        return None;
    }
    let rolling = opencode_usage_block(text, "rollingUsage")?;
    let weekly = opencode_usage_block(text, "weeklyUsage")?;
    let monthly = opencode_usage_block(text, "monthlyUsage");
    Some(OpenCodeUsage {
        rolling_percent: top_level_number(rolling, "usagePercent")?.clamp(0.0, 100.0),
        weekly_percent: top_level_number(weekly, "usagePercent")?.clamp(0.0, 100.0),
        monthly_percent: monthly
            .and_then(|block| top_level_number(block, "usagePercent"))
            .map(|percent| percent.clamp(0.0, 100.0)),
        rolling_reset_seconds: top_level_number(rolling, "resetInSec")?,
        weekly_reset_seconds: top_level_number(weekly, "resetInSec")?,
        monthly_reset_seconds: monthly.and_then(|block| top_level_number(block, "resetInSec")),
    })
}

fn relative_reset(seconds: f64) -> Option<u64> {
    seconds.is_finite().then(|| {
        let milliseconds = (seconds.max(0.0) * 1_000.0)
            .round()
            .clamp(0.0, u64::MAX as f64) as u64;
        now_unix_ms().saturating_add(milliseconds)
    })
}

fn opencode_bucket(name: &str, percent: f64, reset_seconds: f64) -> RateLimitBucket {
    RateLimitBucket {
        name: name.to_string(),
        used_percent: percent.round().clamp(0.0, 100.0) as u8,
        resets_at_unix_ms: relative_reset(reset_seconds),
    }
}

fn cookie_pairs(raw: &str) -> Vec<(String, String)> {
    let mut pairs = raw
        .split(';')
        .filter_map(|part| {
            let normalized = part.trim().trim_start_matches("Cookie:").trim();
            let (name, value) = normalized.split_once('=')?;
            (!name.trim().is_empty() && !value.trim().is_empty())
                .then(|| (name.trim().to_string(), value.trim().to_string()))
        })
        .collect::<Vec<_>>();
    let quoted =
        Regex::new(r#"(?:^|[;\s])([A-Za-z0-9_.-]+)\s*:\s*["']([^"']+)["']"#).expect("fixed regex");
    pairs.extend(quoted.captures_iter(raw).filter_map(|capture| {
        Some((
            capture.get(1)?.as_str().trim().to_string(),
            capture.get(2)?.as_str().trim().to_string(),
        ))
    }));
    pairs
}

fn minimax_cookie_value(raw: &str, name: &str) -> Option<String> {
    cookie_pairs(raw)
        .into_iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value)
}

fn normalized_cookie_header(raw: &str) -> String {
    cookie_pairs(raw)
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_minimax_quota(value: &Value, models: &str) -> Option<RateLimitBucket> {
    let preferred = models
        .split(',')
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    let preferred = if preferred.is_empty() {
        vec!["general"]
    } else {
        preferred
    };
    let snapshots = value
        .get("model_remains")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|item| {
            let model = item.get("model_name")?.as_str()?;
            let remaining = numeric(item.get("current_interval_remaining_percent"))?;
            let end = item.get("end_time").and_then(parse_reset_value)?;
            Some((
                model,
                RateLimitBucket {
                    name: "5 hour".to_string(),
                    used_percent: (100.0 - remaining).round().clamp(0.0, 100.0) as u8,
                    resets_at_unix_ms: Some(end),
                },
            ))
        })
        .collect::<Vec<_>>();
    preferred
        .into_iter()
        .find_map(|model| {
            snapshots
                .iter()
                .find(|(candidate, _)| *candidate == model)
                .map(|(_, bucket)| bucket.clone())
        })
        .or_else(|| (snapshots.len() == 1).then(|| snapshots[0].1.clone()))
}

#[derive(Clone)]
struct GeminiCredentials {
    access_token: String,
    refresh_token: String,
    expires_at_unix_ms: u64,
    project_hint: Option<String>,
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn provider_config_dir(
    explicit: Option<&Path>,
    environment_name: &str,
    fallback_name: &str,
) -> Option<PathBuf> {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os(environment_name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(fallback_name)))
}

#[cfg(target_os = "macos")]
fn claude_keychain_service(config_dir: Option<&Path>) -> String {
    let Some(config_dir) = config_dir else {
        return "Claude Code-credentials".to_string();
    };
    let digest = Sha256::digest(config_dir.to_string_lossy().as_bytes());
    format!(
        "Claude Code-credentials-{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

#[cfg(target_os = "macos")]
fn read_claude_keychain(config_dir: Option<&Path>) -> Option<Value> {
    let account = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string());
    let scoped = claude_keychain_service(config_dir);
    let mut services = vec![scoped.as_str()];
    if scoped != "Claude Code-credentials" {
        services.push("Claude Code-credentials");
    }
    services.into_iter().find_map(|service| {
        let output = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-a", &account, "-w"])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| serde_json::from_slice(&output.stdout).ok())
            .flatten()
    })
}

#[cfg(not(target_os = "macos"))]
fn read_claude_keychain(_config_dir: Option<&Path>) -> Option<Value> {
    None
}

fn claude_access_token(config_dir: Option<&Path>) -> Option<String> {
    let config_dir = provider_config_dir(config_dir, "CLAUDE_CONFIG_DIR", ".claude")?;
    read_json(&config_dir.join(".credentials.json"))
        .or_else(|| read_claude_keychain(Some(&config_dir)))
        .and_then(|json| {
            json.pointer("/claudeAiOauth/accessToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn codex_auth(config_dir: Option<&Path>) -> Option<(String, Option<String>)> {
    let config_dir = provider_config_dir(config_dir, "CODEX_HOME", ".codex")?;
    let json = read_json(&config_dir.join("auth.json"))?;
    let token = json
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())?
        .to_string();
    let account_id = json
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .filter(|account| !account.is_empty())
        .map(ToOwned::to_owned);
    Some((token, account_id))
}

#[cfg(target_os = "macos")]
fn macos_extra_root_certificates() -> &'static [reqwest::Certificate] {
    use std::sync::OnceLock;

    static CERTIFICATES: OnceLock<Vec<reqwest::Certificate>> = OnceLock::new();
    CERTIFICATES.get_or_init(|| {
        let mut bundles = ["SSL_CERT_FILE", "CURL_CA_BUNDLE"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        bundles.push(PathBuf::from("/etc/ssl/cert.pem"));
        bundles.sort();
        bundles.dedup();
        let mut certificates = bundles
            .into_iter()
            .filter_map(|path| {
                fs::metadata(&path)
                    .ok()
                    .filter(|metadata| metadata.is_file() && metadata.len() <= 5 * 1024 * 1024)
                    .and_then(|_| fs::read(path).ok())
            })
            .filter_map(|bytes| reqwest::Certificate::from_pem_bundle(&bytes).ok())
            .flatten()
            .collect::<Vec<_>>();

        // rustls-native-certs intentionally exports only trust anchors. macOS
        // can also explicitly trust an enterprise forwarding intermediate in
        // System.keychain; SecureTransport (used by Orca) accepts it, so export
        // that bounded public-certificate set as additional trust anchors.
        if let Ok(output) = Command::new("security")
            .args([
                "find-certificate",
                "-a",
                "-p",
                "/Library/Keychains/System.keychain",
            ])
            .output()
        {
            if output.status.success() && output.stdout.len() <= 5 * 1024 * 1024 {
                if let Ok(system_certificates) =
                    reqwest::Certificate::from_pem_bundle(&output.stdout)
                {
                    certificates.extend(system_certificates);
                }
            }
        }
        certificates
    })
}

fn quota_client(proxy_url: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(10));
    #[cfg(target_os = "macos")]
    for certificate in macos_extra_root_certificates() {
        builder = builder.add_root_certificate(certificate.clone());
    }
    if !proxy_url.trim().is_empty() {
        let proxy = reqwest::Proxy::all(proxy_url.trim())
            .map_err(|_| "The configured HTTP proxy URL is invalid.".to_string())?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|_| "Could not initialize the provider quota client.".to_string())
}

pub async fn fetch_claude(config_dir: Option<PathBuf>, proxy_url: String) -> ProviderRateLimits {
    let Some(token) = claude_access_token(config_dir.as_deref()) else {
        return ProviderRateLimits::unavailable_for(
            RateLimitProvider::Claude,
            "Claude OAuth credentials were not found.",
        );
    };
    let client = match quota_client(&proxy_url) {
        Ok(client) => client,
        Err(error) => {
            return ProviderRateLimits::failed_for(RateLimitProvider::Claude, error);
        }
    };
    let response = match client
        .get(CLAUDE_USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-code/2.1.0")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::Claude,
                "Could not contact the Claude usage service.",
            );
        }
    };
    if !response.status().is_success() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Claude,
            format!(
                "Claude usage fetch failed (HTTP {}).",
                response.status().as_u16()
            ),
        );
    }
    match response.json::<Value>().await {
        Ok(value) => ProviderRateLimits {
            provider: RateLimitProvider::Claude,
            buckets: parse_claude_quota(&value),
            updated_at_unix_ms: now_unix_ms(),
            error: None,
            status: RateLimitStatus::Ok,
        },
        Err(_) => ProviderRateLimits::failed_for(
            RateLimitProvider::Claude,
            "Claude usage response could not be read.",
        ),
    }
}

pub async fn fetch_codex(config_dir: Option<PathBuf>, proxy_url: String) -> ProviderRateLimits {
    let Some((token, account_id)) = codex_auth(config_dir.as_deref()) else {
        return ProviderRateLimits::unavailable_for(
            RateLimitProvider::Codex,
            "Codex credentials were not found.",
        );
    };
    let client = match quota_client(&proxy_url) {
        Ok(client) => client,
        Err(error) => return ProviderRateLimits::failed_for(RateLimitProvider::Codex, error),
    };
    let mut request = client
        .get(CODEX_USAGE_URL)
        .bearer_auth(token)
        .header("User-Agent", "codex-cli")
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop");
    if let Some(account_id) = account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::Codex,
                "Could not contact the Codex usage service.",
            );
        }
    };
    if !response.status().is_success() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Codex,
            format!(
                "Codex usage fetch failed (HTTP {}).",
                response.status().as_u16()
            ),
        );
    }
    match response.json::<Value>().await {
        Ok(value) => match parse_codex_quota(&value) {
            Some(buckets) => ProviderRateLimits {
                provider: RateLimitProvider::Codex,
                buckets,
                updated_at_unix_ms: now_unix_ms(),
                error: None,
                status: RateLimitStatus::Ok,
            },
            None => ProviderRateLimits::failed_for(
                RateLimitProvider::Codex,
                "Codex usage response was not recognized.",
            ),
        },
        Err(_) => ProviderRateLimits::failed_for(
            RateLimitProvider::Codex,
            "Codex usage response could not be read.",
        ),
    }
}

pub async fn fetch_kimi(proxy_url: String) -> ProviderRateLimits {
    let home = std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".kimi-code")));
    let credentials = home
        .as_deref()
        .and_then(|home| read_json(&home.join("credentials/kimi-code.json")));
    let Some(credentials) = credentials else {
        return ProviderRateLimits::unavailable_for(
            RateLimitProvider::Kimi,
            "Not signed in to Kimi Code.",
        );
    };
    let Some(token) = credentials
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
    else {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Kimi,
            "Kimi credentials are missing an access token.",
        );
    };
    let fresh = credentials
        .get("expires_at")
        .and_then(Value::as_u64)
        .is_some_and(|expires| expires > now_unix_ms() / 1_000 + 5);
    if !fresh {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Kimi,
            "Kimi session expired — run kimi, then refresh usage.",
        );
    }
    let client = match quota_client(&proxy_url) {
        Ok(client) => client,
        Err(error) => return ProviderRateLimits::failed_for(RateLimitProvider::Kimi, error),
    };
    let base = std::env::var("KIMI_CODE_BASE_URL")
        .unwrap_or_else(|_| "https://api.kimi.com/coding/v1".to_string());
    let response = match client
        .get(format!("{}/usages", base.trim_end_matches('/')))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::Kimi,
                "Could not contact the Kimi usage service.",
            );
        }
    };
    if !response.status().is_success() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Kimi,
            format!(
                "Kimi usage fetch failed (HTTP {}).",
                response.status().as_u16()
            ),
        );
    }
    match response.json::<Value>().await {
        Ok(value) => {
            let buckets = parse_kimi_quota(&value);
            if buckets.is_empty() {
                ProviderRateLimits::failed_for(
                    RateLimitProvider::Kimi,
                    "Kimi usage response did not include quota windows.",
                )
            } else {
                ProviderRateLimits {
                    provider: RateLimitProvider::Kimi,
                    buckets,
                    updated_at_unix_ms: now_unix_ms(),
                    error: None,
                    status: RateLimitStatus::Ok,
                }
            }
        }
        Err(_) => ProviderRateLimits::failed_for(
            RateLimitProvider::Kimi,
            "Kimi usage response could not be read.",
        ),
    }
}

pub async fn fetch_grok(proxy_url: String) -> ProviderRateLimits {
    let Some(session) = read_grok_session() else {
        return ProviderRateLimits::unavailable_for(
            RateLimitProvider::Grok,
            "Not signed in to Grok CLI.",
        );
    };
    if session
        .expires_at_unix_ms
        .is_some_and(|expires| expires <= now_unix_ms() + 5 * 60_000)
    {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Grok,
            "Grok session expired — run grok, then refresh usage.",
        );
    }
    let client = match quota_client(&proxy_url) {
        Ok(client) => client,
        Err(error) => return ProviderRateLimits::failed_for(RateLimitProvider::Grok, error),
    };
    let base = std::env::var("GROK_CLI_CHAT_PROXY_BASE_URL")
        .unwrap_or_else(|_| "https://cli-chat-proxy.grok.com/v1".to_string());
    let endpoint = format!("{}/billing", base.trim_end_matches('/'));
    let send = |url: String| {
        let mut request = client
            .get(url)
            .bearer_auth(&session.access_token)
            .header("X-XAI-Token-Auth", "xai-grok-cli")
            .header("Accept", "application/json");
        if let Some(user_id) = session.user_id.as_deref() {
            request = request.header("x-userid", user_id);
        }
        request
    };

    let credits_response = match send(format!("{endpoint}?format=credits")).send().await {
        Ok(response) => response,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::Grok,
                "Could not contact the Grok usage service.",
            );
        }
    };
    if !credits_response.status().is_success() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Grok,
            format!(
                "Grok usage fetch failed (HTTP {}).",
                credits_response.status().as_u16()
            ),
        );
    }
    let credits = match credits_response.json::<Value>().await {
        Ok(value) => value,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::Grok,
                "Grok usage response could not be read.",
            );
        }
    };
    if let Some(bucket) = parse_grok_weekly(&credits) {
        return ProviderRateLimits {
            provider: RateLimitProvider::Grok,
            buckets: vec![bucket],
            updated_at_unix_ms: now_unix_ms(),
            error: None,
            status: RateLimitStatus::Ok,
        };
    }

    let legacy_response = match send(endpoint).send().await {
        Ok(response) => response,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::Grok,
                "Could not contact the Grok usage service.",
            );
        }
    };
    if !legacy_response.status().is_success() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::Grok,
            format!(
                "Grok usage fetch failed (HTTP {}).",
                legacy_response.status().as_u16()
            ),
        );
    }
    match legacy_response.json::<Value>().await {
        Ok(value) => match parse_grok_monthly(&value) {
            Some(bucket) => ProviderRateLimits {
                provider: RateLimitProvider::Grok,
                buckets: vec![bucket],
                updated_at_unix_ms: now_unix_ms(),
                error: None,
                status: RateLimitStatus::Ok,
            },
            None => ProviderRateLimits::failed_for(
                RateLimitProvider::Grok,
                "Grok usage response did not include a quota window.",
            ),
        },
        Err(_) => ProviderRateLimits::failed_for(
            RateLimitProvider::Grok,
            "Grok usage response could not be read.",
        ),
    }
}

pub async fn fetch_opencode_go(
    cookie: Option<suaegi_secrets::Secret>,
    workspace_override: String,
    proxy_url: String,
) -> ProviderRateLimits {
    const BASE: &str = "https://opencode.ai";
    const SERVER_ID: &str = "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
    let Some(cookie) = cookie else {
        return ProviderRateLimits::unavailable_for(
            RateLimitProvider::OpenCodeGo,
            "Session cookie not configured.",
        );
    };
    let cookie = filter_opencode_cookie(&normalize_opencode_cookie(cookie.expose()));
    if cookie.is_empty() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::OpenCodeGo,
            "No auth cookie found — paste the opencode.ai auth cookie.",
        );
    }
    let client = match quota_client(&proxy_url) {
        Ok(client) => client,
        Err(error) => return ProviderRateLimits::failed_for(RateLimitProvider::OpenCodeGo, error),
    };
    let override_id = workspace_override.trim();
    let valid_workspace =
        Regex::new(r"^(?:wrk|wk)_[A-Za-z0-9]+$").expect("fixed workspace id regex");
    let ids = if override_id.is_empty() {
        let url = format!("{BASE}/_server?id={SERVER_ID}");
        let instance = format!("server-fn:{}-{}", std::process::id(), now_unix_ms());
        let response = match client
            .get(url)
            .header("Cookie", &cookie)
            .header("X-Server-Id", SERVER_ID)
            .header("X-Server-Instance", instance)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            )
            .header("Origin", BASE)
            .header("Referer", BASE)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                return ProviderRateLimits::failed_for(
                    RateLimitProvider::OpenCodeGo,
                    "Could not contact the OpenCode Go workspace service.",
                );
            }
        };
        if !response.status().is_success() {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::OpenCodeGo,
                format!(
                    "OpenCode Go workspaces fetch failed (HTTP {}).",
                    response.status().as_u16()
                ),
            );
        }
        match response.text().await {
            Ok(text) if text.len() <= 10_000_000 => parse_opencode_workspace_ids(&text),
            _ => {
                return ProviderRateLimits::failed_for(
                    RateLimitProvider::OpenCodeGo,
                    "OpenCode Go workspaces response could not be read.",
                );
            }
        }
    } else if valid_workspace.is_match(override_id) {
        vec![override_id.to_string()]
    } else {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::OpenCodeGo,
            "Invalid workspace ID; expected wrk_… or wk_….",
        );
    };
    if ids.is_empty() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::OpenCodeGo,
            "No workspace ID found — set a Workspace ID override in Settings.",
        );
    }

    let mut last_error = "Could not parse usage data from any available workspace.".to_string();
    for id in ids {
        let response = match client
            .get(format!("{BASE}/workspace/{id}/go"))
            .header("Cookie", &cookie)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Origin", BASE)
            .header("Referer", BASE)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                last_error = "Could not contact the OpenCode Go usage page.".to_string();
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = format!(
                "OpenCode Go usage fetch failed (HTTP {}).",
                response.status().as_u16()
            );
            continue;
        }
        let Ok(text) = response.text().await else {
            last_error = "OpenCode Go usage response could not be read.".to_string();
            continue;
        };
        let Some(usage) = parse_opencode_usage(&text) else {
            last_error = "Could not parse OpenCode Go usage data.".to_string();
            continue;
        };
        let mut buckets = vec![
            opencode_bucket("5 hour", usage.rolling_percent, usage.rolling_reset_seconds),
            opencode_bucket("Weekly", usage.weekly_percent, usage.weekly_reset_seconds),
        ];
        if let (Some(percent), Some(reset)) = (usage.monthly_percent, usage.monthly_reset_seconds) {
            buckets.push(opencode_bucket("Monthly", percent, reset));
        }
        return ProviderRateLimits {
            provider: RateLimitProvider::OpenCodeGo,
            buckets,
            updated_at_unix_ms: now_unix_ms(),
            error: None,
            status: RateLimitStatus::Ok,
        };
    }
    ProviderRateLimits::failed_for(RateLimitProvider::OpenCodeGo, last_error)
}

pub async fn fetch_minimax(
    cookie: Option<suaegi_secrets::Secret>,
    group_override: String,
    models: String,
    proxy_url: String,
) -> ProviderRateLimits {
    const ENDPOINT: &str = "https://platform.minimax.io/v1/api/openplatform/coding_plan/remains";
    let Some(cookie) = cookie else {
        return ProviderRateLimits::unavailable_for(
            RateLimitProvider::MiniMax,
            "MiniMax session cookie not configured.",
        );
    };
    let cookie = normalized_cookie_header(cookie.expose());
    if minimax_cookie_value(&cookie, "_token").is_none() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::MiniMax,
            "MiniMax auth cookie not found — paste a Cookie header with _token.",
        );
    }
    let group = (!group_override.trim().is_empty())
        .then(|| group_override.trim().to_string())
        .or_else(|| minimax_cookie_value(&cookie, "minimax_group_id_v2"));
    let client = match quota_client(&proxy_url) {
        Ok(client) => client,
        Err(error) => return ProviderRateLimits::failed_for(RateLimitProvider::MiniMax, error),
    };
    let user_agent = if cfg!(target_os = "windows") {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0"
    } else if cfg!(target_os = "macos") {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0"
    } else {
        "Mozilla/5.0 (X11; Linux x86_64; rv:152.0) Gecko/20100101 Firefox/152.0"
    };
    let mut request = client
        .get(ENDPOINT)
        .header("Cookie", cookie)
        .header("Accept", "application/json, text/plain, */*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://platform.minimax.io/console/usage")
        .header("User-Agent", user_agent);
    if let Some(group) = group {
        request = request.header("X-Group-Id", group);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::MiniMax,
                "Could not contact the MiniMax usage service.",
            );
        }
    };
    if matches!(response.status().as_u16(), 401 | 403) {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::MiniMax,
            "MiniMax session expired. Replace the cookie in Settings.",
        );
    }
    if !response.status().is_success() {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::MiniMax,
            format!(
                "MiniMax usage fetch failed (HTTP {}).",
                response.status().as_u16()
            ),
        );
    }
    let value = match response.json::<Value>().await {
        Ok(value) => value,
        Err(_) => {
            return ProviderRateLimits::failed_for(
                RateLimitProvider::MiniMax,
                "MiniMax usage response could not be read.",
            );
        }
    };
    if value
        .pointer("/base_resp/status_code")
        .and_then(Value::as_i64)
        .is_some_and(|status| status != 0)
    {
        return ProviderRateLimits::failed_for(
            RateLimitProvider::MiniMax,
            "MiniMax returned a usage error.",
        );
    }
    match parse_minimax_quota(&value, &models) {
        Some(bucket) => ProviderRateLimits {
            provider: RateLimitProvider::MiniMax,
            buckets: vec![bucket],
            updated_at_unix_ms: now_unix_ms(),
            error: None,
            status: RateLimitStatus::Ok,
        },
        None => ProviderRateLimits::failed_for(
            RateLimitProvider::MiniMax,
            "MiniMax usage data for the configured model was not found.",
        ),
    }
}

fn opencode_auth_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(value) = std::env::var_os("APPDATA") {
        paths.push(PathBuf::from(value).join("opencode/auth.json"));
    }
    if let Some(value) = std::env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(value).join("opencode/auth.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local/share/opencode/auth.json"));
        paths.push(home.join("Library/Application Support/opencode/auth.json"));
    }
    paths
}

fn read_credentials() -> Option<GeminiCredentials> {
    for path in opencode_auth_candidates() {
        let Some(google) = read_json(&path)
            .and_then(|value| value.get("google").cloned())
            .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("oauth"))
        else {
            continue;
        };
        let access_token = google.get("access")?.as_str()?.to_string();
        let refresh_field = google.get("refresh")?.as_str()?.to_string();
        let mut refresh_parts = refresh_field.split('|');
        let refresh_token = refresh_parts.next().unwrap_or_default().to_string();
        let project_hint = refresh_parts
            .find(|part| !part.trim().is_empty())
            .map(str::to_string);
        return Some(GeminiCredentials {
            access_token,
            refresh_token,
            expires_at_unix_ms: google.get("expires")?.as_u64()?,
            project_hint,
        });
    }

    let path = dirs::home_dir()?.join(".gemini/oauth_creds.json");
    let value = read_json(&path)?;
    Some(GeminiCredentials {
        access_token: value.get("access_token")?.as_str()?.to_string(),
        refresh_token: value.get("refresh_token")?.as_str()?.to_string(),
        expires_at_unix_ms: value.get("expiry_date")?.as_u64()?,
        project_hint: None,
    })
}

fn gemini_binary() -> Option<PathBuf> {
    let from_path = Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg("gemini")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|output| output.lines().next().map(str::trim).map(PathBuf::from))
        .filter(|path| path.is_file());
    from_path.or_else(|| {
        let home = dirs::home_dir()?;
        [
            PathBuf::from("/usr/local/bin/gemini"),
            PathBuf::from("/opt/homebrew/bin/gemini"),
            home.join(".local/bin/gemini"),
            home.join("bin/gemini"),
        ]
        .into_iter()
        .find(|path| path.is_file())
    })
}

fn oauth_credentials_from_file(path: &Path) -> Option<(String, String)> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > 32 * 1024 * 1024 {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let client_id = Regex::new(r#"OAUTH_CLIENT_ID\s*=\s*['"]([^'"]+)['"]"#)
        .ok()?
        .captures(&content)?
        .get(1)?
        .as_str()
        .to_string();
    let client_secret = Regex::new(r#"OAUTH_CLIENT_SECRET\s*=\s*['"]([^'"]+)['"]"#)
        .ok()?
        .captures(&content)?
        .get(1)?
        .as_str()
        .to_string();
    Some((client_id, client_secret))
}

fn package_root(real_binary: &Path) -> Option<PathBuf> {
    let mut current = real_binary.parent()?.to_path_buf();
    for _ in 0..=8 {
        let direct = current.join("package.json");
        if read_json(&direct)
            .and_then(|value| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .as_deref()
            == Some("@google/gemini-cli")
        {
            return Some(current);
        }
        let global = current.join("lib/node_modules/@google/gemini-cli");
        if global.join("package.json").is_file() {
            return Some(global);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn extract_oauth_client_credentials() -> Option<(String, String)> {
    let binary = gemini_binary()?;
    let real_binary = fs::canonicalize(&binary).unwrap_or(binary);
    let bin_dir = real_binary.parent()?;
    let base_dir = bin_dir.parent()?;
    let oauth_subpath = Path::new("dist/src/code_assist/oauth2.js");
    let candidates = [
        base_dir
            .join(
                "libexec/lib/node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core",
            )
            .join(oauth_subpath),
        base_dir
            .join("lib/node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core")
            .join(oauth_subpath),
        base_dir
            .join("share/gemini-cli/node_modules/@google/gemini-cli-core")
            .join(oauth_subpath),
        base_dir.join("../gemini-cli-core").join(oauth_subpath),
        base_dir
            .join("node_modules/@google/gemini-cli-core")
            .join(oauth_subpath),
    ];
    for path in candidates {
        if let Some(credentials) = oauth_credentials_from_file(&path) {
            return Some(credentials);
        }
    }

    let root = package_root(&real_binary)?;
    for path in [
        root.join("node_modules/@google/gemini-cli-core")
            .join(oauth_subpath),
        root.join(oauth_subpath),
    ] {
        if let Some(credentials) = oauth_credentials_from_file(&path) {
            return Some(credentials);
        }
    }
    let bundle = root.join("bundle");
    for entry in fs::read_dir(bundle).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("js") {
            if let Some(credentials) = oauth_credentials_from_file(&path) {
                return Some(credentials);
            }
        }
    }
    None
}

async fn refresh_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<String, String> {
    let (client_id, client_secret) = extract_oauth_client_credentials()
        .ok_or_else(|| "Gemini CLI OAuth client credentials were not found.".to_string())?;
    let response = client
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|_| "Gemini token refresh failed.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Gemini token refresh failed (HTTP {}).",
            response.status().as_u16()
        ));
    }
    response
        .json::<Value>()
        .await
        .ok()
        .and_then(|value| {
            value
                .get("access_token")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| "Gemini token refresh returned no access token.".to_string())
}

async fn load_project_id(client: &reqwest::Client, access_token: &str) -> Result<String, String> {
    let response = client
        .post(LOAD_CODE_ASSIST_URL)
        .bearer_auth(access_token)
        .json(&serde_json::json!({
            "metadata": { "ideType": "GEMINI_CLI", "pluginType": "GEMINI" }
        }))
        .send()
        .await
        .map_err(|_| "Could not contact the Gemini account service.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Gemini project lookup failed (HTTP {}).",
            response.status().as_u16()
        ));
    }
    response
        .json::<Value>()
        .await
        .ok()
        .and_then(|value| {
            value
                .get("cloudaicompanionProject")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| "Gemini project ID was not found.".to_string())
}

fn model_name(model_id: &str) -> String {
    match model_id {
        "gemini-3.1-pro" => "3.1 Pro".into(),
        "gemini-3.1-flash" => "3.1 Flash".into(),
        "gemini-3.1-flash-lite" => "3.1 Flash Lite".into(),
        "gemini-3.0-pro" => "3.0 Pro".into(),
        "gemini-3.0-flash" => "3.0 Flash".into(),
        "gemini-2.5-pro" => "Pro".into(),
        "gemini-2.5-flash" => "Flash".into(),
        "gemini-2.5-flash-lite" => "Flash Lite".into(),
        "gemini-2.0-pro" => "2.0 Pro".into(),
        "gemini-2.0-flash" => "2.0 Flash".into(),
        "gemini-2.0-flash-lite" => "2.0 Flash Lite".into(),
        "gemini-1.5-pro" => "1.5 Pro".into(),
        "gemini-1.5-flash" => "1.5 Flash".into(),
        "gemini-exp" | "gemini-experimental" => "Exp".into(),
        other => other
            .strip_prefix("gemini-")
            .unwrap_or(other)
            .split('-')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn parse_reset_time(value: &str) -> Option<u64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|date| u64::try_from(date.timestamp_millis()).ok())
}

fn parse_quota_response(value: &Value) -> Vec<RateLimitBucket> {
    let raw = value
        .as_array()
        .or_else(|| value.get("buckets").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut buckets = Vec::<(RateLimitBucket, String)>::new();
    for item in raw {
        let Some(remaining) = item.get("remainingFraction").and_then(Value::as_f64) else {
            continue;
        };
        let Some(model_id) = item.get("modelId").and_then(Value::as_str) else {
            continue;
        };
        let reset = item
            .get("resetTime")
            .and_then(Value::as_str)
            .and_then(parse_reset_time);
        let used_percent = ((1.0 - remaining) * 100.0).round().clamp(0.0, 100.0) as u8;
        let candidate = RateLimitBucket {
            name: model_name(model_id),
            used_percent,
            resets_at_unix_ms: reset,
        };
        if let Some(index) = buckets.iter().position(|(existing, _)| {
            existing.used_percent == candidate.used_percent
                && existing.resets_at_unix_ms == candidate.resets_at_unix_ms
        }) {
            let current_known = model_name(model_id) != model_id;
            let existing_known = model_name(&buckets[index].1) != buckets[index].1;
            if (current_known && !existing_known)
                || (current_known == existing_known
                    && candidate.name.len() < buckets[index].0.name.len())
            {
                buckets[index] = (candidate, model_id.to_string());
            }
        } else {
            buckets.push((candidate, model_id.to_string()));
        }
    }
    buckets.into_iter().map(|(bucket, _)| bucket).collect()
}

async fn retrieve_quota(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
) -> Result<Vec<RateLimitBucket>, String> {
    let response = client
        .post(RETRIEVE_QUOTA_URL)
        .bearer_auth(access_token)
        .json(&serde_json::json!({ "project": project_id }))
        .send()
        .await
        .map_err(|_| "Could not contact the Gemini quota service.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Gemini quota fetch failed (HTTP {}).",
            response.status().as_u16()
        ));
    }
    let value = response
        .json::<Value>()
        .await
        .map_err(|_| "Gemini quota response could not be read.".to_string())?;
    Ok(parse_quota_response(&value))
}

pub async fn fetch_gemini(enabled: bool) -> GeminiRateLimits {
    if !enabled {
        return GeminiRateLimits::unavailable("Gemini CLI OAuth is disabled in settings.");
    }
    let Some(credentials) = read_credentials() else {
        return GeminiRateLimits::unavailable("Gemini CLI credentials were not found.");
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(_) => return GeminiRateLimits::failed("Could not initialize the Gemini quota client."),
    };
    let mut access_token = credentials.access_token;
    if access_token.is_empty() || credentials.expires_at_unix_ms <= now_unix_ms() {
        access_token = match refresh_access_token(&client, &credentials.refresh_token).await {
            Ok(token) => token,
            Err(error) => return GeminiRateLimits::failed(error),
        };
    }
    let project_id = match load_project_id(&client, &access_token).await {
        Ok(project) => project,
        Err(error) => match credentials.project_hint {
            Some(project) if !project.is_empty() => project,
            _ => return GeminiRateLimits::failed(error),
        },
    };
    match retrieve_quota(&client, &access_token, &project_id).await {
        Ok(buckets) => GeminiRateLimits {
            provider: RateLimitProvider::Gemini,
            buckets,
            updated_at_unix_ms: now_unix_ms(),
            error: None,
            status: RateLimitStatus::Ok,
        },
        Err(error) => GeminiRateLimits::failed(error),
    }
}

pub fn displayed_percentage(used: u8, mode: &str) -> u8 {
    if mode == "remaining" {
        100_u8.saturating_sub(used)
    } else {
        used
    }
}

pub fn reset_label(resets_at_unix_ms: Option<u64>) -> Option<String> {
    let remaining = resets_at_unix_ms?.saturating_sub(now_unix_ms());
    let minutes = remaining.div_ceil(60_000);
    Some(if minutes >= 24 * 60 {
        format!("resets in {}d", minutes.div_ceil(24 * 60))
    } else if minutes >= 60 {
        format!("resets in {}h", minutes.div_ceil(60))
    } else {
        format!("resets in {minutes}m")
    })
}

pub fn compact_reset_label(resets_at_unix_ms: Option<u64>) -> Option<String> {
    compact_reset_label_at(resets_at_unix_ms, now_unix_ms())
}

fn compact_reset_label_at(resets_at_unix_ms: Option<u64>, now_unix_ms: u64) -> Option<String> {
    let remaining = resets_at_unix_ms?.saturating_sub(now_unix_ms);
    if remaining == 0 {
        return Some("now".to_string());
    }
    let minutes = remaining / 60_000;
    Some(if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 24 * 60 {
        let hours = minutes / 60;
        let minutes = minutes % 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else {
        let days = minutes / (24 * 60);
        let hours = (minutes / 60) % 24;
        if hours == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {hours}h")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_buckets_match_orca_names_percentages_and_deduplication() {
        let value = serde_json::json!({
            "buckets": [
                {
                    "remainingFraction": 0.25,
                    "resetTime": "2026-07-29T10:00:00Z",
                    "modelId": "gemini-2.5-pro"
                },
                {
                    "remainingFraction": 0.25,
                    "resetTime": "2026-07-29T10:00:00Z",
                    "modelId": "gemini-2.5-pro-preview"
                },
                {
                    "remainingFraction": 1.5,
                    "resetTime": "bad",
                    "modelId": "gemini-custom-fast"
                }
            ]
        });
        let buckets = parse_quota_response(&value);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].name, "Pro");
        assert_eq!(buckets[0].used_percent, 75);
        assert!(buckets[0].resets_at_unix_ms.is_some());
        assert_eq!(buckets[1].name, "Custom Fast");
        assert_eq!(buckets[1].used_percent, 0);
        assert_eq!(buckets[1].resets_at_unix_ms, None);
    }

    #[test]
    fn percentage_mode_reports_used_or_remaining() {
        assert_eq!(displayed_percentage(6, "used"), 6);
        assert_eq!(displayed_percentage(6, "remaining"), 94);
        assert_eq!(displayed_percentage(100, "remaining"), 0);
    }

    #[test]
    fn compact_reset_labels_match_orca_status_bar_units() {
        let now = 1_000_000;
        assert_eq!(
            compact_reset_label_at(Some(now), now).as_deref(),
            Some("now")
        );
        assert_eq!(
            compact_reset_label_at(Some(now + 47 * 60_000), now).as_deref(),
            Some("47m")
        );
        assert_eq!(
            compact_reset_label_at(Some(now + (3 * 60 + 54) * 60_000), now).as_deref(),
            Some("3h 54m")
        );
        assert_eq!(
            compact_reset_label_at(Some(now + (6 * 24 + 7) * 60 * 60_000), now).as_deref(),
            Some("6d 7h")
        );
        assert_eq!(
            compact_reset_label_at(Some(now + 6 * 24 * 60 * 60_000), now).as_deref(),
            Some("6d")
        );
    }

    #[test]
    fn claude_windows_support_current_and_legacy_fable_contracts() {
        let current = serde_json::json!({
            "five_hour": {"utilization": 12.6, "resets_at": 2_000_000_000},
            "seven_day": {"used_percentage": 44},
            "limits": [{
                "kind": "weekly_scoped",
                "percent": 73,
                "resets_at": "2026-08-01T10:00:00Z",
                "is_active": false,
                "scope": {"model": {"display_name": "Fable"}}
            }]
        });
        let buckets = parse_claude_quota(&current);
        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.name.as_str(), bucket.used_percent))
                .collect::<Vec<_>>(),
            [("5 hour", 13), ("Weekly", 44), ("Fable weekly", 73)]
        );
        assert_eq!(buckets[0].resets_at_unix_ms, Some(2_000_000_000_000));

        let legacy = serde_json::json!({
            "fable_weekly": {"utilization": 9}
        });
        assert_eq!(parse_claude_quota(&legacy)[0].used_percent, 9);
    }

    #[test]
    fn codex_backend_usage_requires_plan_and_maps_unix_seconds() {
        let value = serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": 6.4, "reset_at": 2_000_000_000},
                "secondary_window": {"used_percent": 18, "reset_at": 2_000_010_000}
            }
        });
        let buckets = parse_codex_quota(&value).expect("valid wham usage payload");
        assert_eq!(buckets[0].used_percent, 6);
        assert_eq!(buckets[0].resets_at_unix_ms, Some(2_000_000_000_000));
        assert!(parse_codex_quota(&serde_json::json!({
            "rate_limit": value["rate_limit"].clone()
        }))
        .is_none());

        let one_window = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {"used_percent": 24, "reset_at": 2_000_000_000},
                "secondary_window": null
            }
        });
        let buckets = parse_codex_quota(&one_window).expect("a null secondary window is supported");
        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.name.as_str(), bucket.used_percent))
                .collect::<Vec<_>>(),
            [("5 hour", 24)]
        );
    }

    #[test]
    fn kimi_selects_the_window_closest_to_five_hours_and_maps_remaining() {
        let value = serde_json::json!({
            "usage": {
                "limit": "1000",
                "remaining": "250",
                "resetTime": "2026-08-02T12:00:00Z"
            },
            "limits": [
                {
                    "window": {"duration": 1, "timeUnit": "DAY"},
                    "detail": {"limit": 100, "used": 10}
                },
                {
                    "window": {"duration": 5, "timeUnit": "HOUR"},
                    "detail": {"limit": 100, "used": 40}
                }
            ]
        });
        let buckets = parse_kimi_quota(&value);
        assert_eq!(
            buckets
                .iter()
                .map(|bucket| (bucket.name.as_str(), bucket.used_percent))
                .collect::<Vec<_>>(),
            [("5 hour", 40), ("Weekly", 75)]
        );
        assert!(buckets[1].resets_at_unix_ms.is_some());
    }

    #[test]
    fn grok_supports_weekly_credits_and_legacy_monthly_contracts() {
        let weekly = serde_json::json!({
            "config": {
                "creditUsagePercent": 37.6,
                "currentPeriod": {"end": "2026-08-03T12:00:00Z"}
            }
        });
        let bucket = parse_grok_weekly(&weekly).expect("weekly credits payload");
        assert_eq!((bucket.name.as_str(), bucket.used_percent), ("Weekly", 38));
        assert!(bucket.resets_at_unix_ms.is_some());

        let monthly = serde_json::json!({
            "config": {
                "monthlyLimit": {"val": "200"},
                "used": {"val": 50},
                "billingPeriodEnd": 2_000_000_000
            }
        });
        let bucket = parse_grok_monthly(&monthly).expect("legacy monthly payload");
        assert_eq!((bucket.name.as_str(), bucket.used_percent), ("Monthly", 25));
        assert_eq!(bucket.resets_at_unix_ms, Some(2_000_000_000_000));
    }

    #[test]
    fn opencode_cookie_workspace_and_flight_usage_contracts_match_orca() {
        assert_eq!(normalize_opencode_cookie("Fe26.2**abc"), "auth=Fe26.2**abc");
        assert_eq!(
            filter_opencode_cookie("theme=dark; auth=secret; other=value"),
            "auth=secret"
        );
        let workspaces = r#"0:{id:"wrk_FIRST123"} 1:{id: "wk_SECOND456"} 2:{id:"wrk_FIRST123"}"#;
        assert_eq!(
            parse_opencode_workspace_ids(workspaces),
            ["wrk_FIRST123", "wk_SECOND456"]
        );
        let flight = r#"
            monthlyUsage:null,
            rollingUsage:$R[28]={usagePercent:12.4,resetInSec:300},
            weeklyUsage: {nested:{usagePercent:99},usagePercent:45,resetInSec:600},
            monthlyUsage:$R[31]={usagePercent:76,resetInSec:900}
        "#;
        let usage = parse_opencode_usage(flight).expect("valid React Flight usage");
        assert_eq!(usage.rolling_percent, 12.4);
        assert_eq!(usage.weekly_percent, 45.0);
        assert_eq!(usage.monthly_percent, Some(76.0));
        assert_eq!(usage.monthly_reset_seconds, Some(900.0));
    }

    #[test]
    fn minimax_accepts_header_and_export_cookie_forms_and_selects_model() {
        let raw = r#"Cookie: _token=secret; minimax_group_id_v2=grp-1; theme=dark"#;
        assert_eq!(
            minimax_cookie_value(raw, "minimax_group_id_v2").as_deref(),
            Some("grp-1")
        );
        assert!(normalized_cookie_header(raw).contains("_token=secret"));
        assert_eq!(
            minimax_cookie_value(r#"_token : "quoted-secret""#, "_token").as_deref(),
            Some("quoted-secret")
        );
        let payload = serde_json::json!({
            "base_resp": {"status_code": 0},
            "model_remains": [
                {
                    "model_name": "other",
                    "current_interval_remaining_percent": 90,
                    "start_time": 1_999_000_000,
                    "end_time": 2_000_000_000
                },
                {
                    "model_name": "general",
                    "current_interval_remaining_percent": "24.4",
                    "start_time": 1_999_000_000,
                    "end_time": 2_000_000_000
                }
            ]
        });
        let bucket = parse_minimax_quota(&payload, "general").expect("configured model");
        assert_eq!((bucket.name.as_str(), bucket.used_percent), ("5 hour", 76));
        assert_eq!(bucket.resets_at_unix_ms, Some(2_000_000_000_000));
    }

    #[test]
    fn remote_runtime_rate_limit_snapshot_maps_orca_windows() {
        let limits = ProviderRateLimits::from_runtime_value(
            RateLimitProvider::Codex,
            &serde_json::json!({
                "session": {"usedPercent": 12.6, "resetsAt": 2_000_000_000_000_u64},
                "weekly": {"usedPercent": 44, "resetsAt": null},
                "monthly": null,
                "buckets": [{"name": "Fast", "usedPercent": 91}],
                "updatedAt": 123,
                "error": null,
                "status": "ok"
            }),
        );
        assert_eq!(limits.status, RateLimitStatus::Ok);
        assert_eq!(limits.updated_at_unix_ms, 123);
        assert_eq!(
            limits
                .buckets
                .iter()
                .map(|bucket| (bucket.name.as_str(), bucket.used_percent))
                .collect::<Vec<_>>(),
            [("5 hour", 13), ("Weekly", 44), ("Fast", 91)]
        );
    }

    #[tokio::test]
    #[ignore = "uses the locally authenticated Gemini CLI account and network"]
    async fn live_gemini_cli_quota_probe() {
        let result = fetch_gemini(true).await;
        if result.status == RateLimitStatus::Unavailable {
            assert_eq!(
                result.error.as_deref(),
                Some("Gemini CLI credentials were not found.")
            );
            return;
        }
        assert_eq!(
            result.status,
            RateLimitStatus::Ok,
            "Gemini quota probe failed: {}",
            result.error.as_deref().unwrap_or("unknown error")
        );
        assert!(!result.buckets.is_empty());
    }

    #[tokio::test]
    #[ignore = "uses the locally authenticated Grok CLI account and network"]
    async fn live_grok_cli_quota_probe() {
        let result = fetch_grok(String::new()).await;
        assert_eq!(
            result.status,
            RateLimitStatus::Ok,
            "Grok quota probe failed: {}",
            result.error.as_deref().unwrap_or("unknown error")
        );
        assert!(!result.buckets.is_empty());
    }
}
