//! Orca-compatible token source-control integration preflight.

use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenIntegrationStatus {
    pub configured: bool,
    pub authenticated: bool,
    pub account: Option<String>,
    pub base_url: Option<String>,
    pub token_configured: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliIntegrationStatus {
    Connected,
    #[default]
    NotInstalled,
    NotAuthenticated,
    OutdatedVersion {
        found: String,
        min: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedIntegrationStatuses {
    pub github: CliIntegrationStatus,
    pub gitlab: CliIntegrationStatus,
    pub gitea: TokenIntegrationStatus,
    pub azure_dev_ops: TokenIntegrationStatus,
    pub bitbucket: TokenIntegrationStatus,
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn normalize_gitea_api_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.to_ascii_lowercase().ends_with("/api/v1") {
        value.to_string()
    } else {
        format!("{value}/api/v1")
    }
}

pub fn normalize_azure_dev_ops_api_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.to_ascii_lowercase().ends_with("/_apis") {
        value[..value.len() - "/_apis".len()].to_string()
    } else {
        value.to_string()
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|error| format!("Could not create integration client: {error}"))
}

async fn gitea(client: &reqwest::Client) -> TokenIntegrationStatus {
    let base_url =
        env_value("ORCA_GITEA_API_BASE_URL").map(|value| normalize_gitea_api_base_url(&value));
    let token = env_value("ORCA_GITEA_TOKEN");
    let token_configured = token.is_some();
    if base_url.is_none() && !token_configured {
        return TokenIntegrationStatus::default();
    }
    let Some(base_url) = base_url else {
        return TokenIntegrationStatus {
            configured: true,
            authenticated: true,
            token_configured,
            ..TokenIntegrationStatus::default()
        };
    };
    if !token_configured {
        let configured = client
            .get(format!("{base_url}/version"))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .is_ok_and(|response| response.status().is_success());
        return TokenIntegrationStatus {
            configured,
            base_url: Some(base_url),
            ..TokenIntegrationStatus::default()
        };
    }
    let response = client
        .get(format!("{base_url}/user"))
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::AUTHORIZATION,
            format!("token {}", token.as_deref().unwrap_or_default()),
        )
        .send()
        .await;
    let authenticated = response
        .as_ref()
        .is_ok_and(|response| response.status().is_success());
    let account =
        match response {
            Ok(response) if authenticated => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|value| {
                    ["login", "username", "full_name"]
                        .into_iter()
                        .find_map(|key| {
                            value
                                .get(key)
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                }),
            _ => None,
        };
    TokenIntegrationStatus {
        configured: true,
        authenticated,
        account,
        base_url: Some(base_url),
        token_configured,
    }
}

async fn bitbucket(client: &reqwest::Client) -> TokenIntegrationStatus {
    let base_url = env_value("ORCA_BITBUCKET_API_BASE_URL")
        .unwrap_or_else(|| "https://api.bitbucket.org/2.0".to_string())
        .trim_end_matches('/')
        .to_string();
    let access_token = env_value("ORCA_BITBUCKET_ACCESS_TOKEN");
    let email = env_value("ORCA_BITBUCKET_EMAIL");
    let api_token = env_value("ORCA_BITBUCKET_API_TOKEN");
    let token_configured = access_token.is_some() || (email.is_some() && api_token.is_some());
    if !token_configured {
        return TokenIntegrationStatus::default();
    }
    let authorization = access_token.map_or_else(
        || {
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
                "{}:{}",
                email.as_deref().unwrap_or_default(),
                api_token.as_deref().unwrap_or_default()
            ));
            format!("Basic {encoded}")
        },
        |token| format!("Bearer {token}"),
    );
    let response = client
        .get(format!("{base_url}/user"))
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::AUTHORIZATION, authorization)
        .send()
        .await;
    let authenticated = response
        .as_ref()
        .is_ok_and(|response| response.status().is_success());
    let account =
        match response {
            Ok(response) if authenticated => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|value| {
                    ["username", "display_name", "account_id"]
                        .into_iter()
                        .find_map(|key| {
                            value
                                .get(key)
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                }),
            _ => None,
        };
    TokenIntegrationStatus {
        configured: true,
        authenticated,
        account,
        base_url: Some(base_url),
        token_configured,
    }
}

async fn azure_dev_ops(client: &reqwest::Client) -> TokenIntegrationStatus {
    let base_url = env_value("ORCA_AZURE_DEVOPS_API_BASE_URL")
        .map(|value| normalize_azure_dev_ops_api_base_url(&value));
    let pat = env_value("ORCA_AZURE_DEVOPS_TOKEN").or_else(|| env_value("ORCA_AZURE_DEVOPS_PAT"));
    let access_token = env_value("ORCA_AZURE_DEVOPS_ACCESS_TOKEN");
    let token_configured = pat.is_some() || access_token.is_some();
    if base_url.is_none() && !token_configured {
        return TokenIntegrationStatus::default();
    }
    let Some(base_url) = base_url else {
        return TokenIntegrationStatus {
            configured: true,
            token_configured,
            ..TokenIntegrationStatus::default()
        };
    };
    let mut request = client
        .get(format!("{base_url}/_apis/connectionData?api-version=7.1"))
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(token) = access_token {
        request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
    } else if let Some(token) = pat {
        let username = env_value("ORCA_AZURE_DEVOPS_USERNAME").unwrap_or_default();
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{token}"));
        request = request.header(reqwest::header::AUTHORIZATION, format!("Basic {encoded}"));
    }
    let response = request.send().await;
    let authenticated = response
        .as_ref()
        .is_ok_and(|response| response.status().is_success());
    let account =
        match response {
            Ok(response) if authenticated => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|value| {
                    let user = value.get("authenticatedUser")?;
                    ["providerDisplayName", "customDisplayName", "uniqueName"]
                        .into_iter()
                        .find_map(|key| {
                            user.get(key)
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                }),
            _ => None,
        };
    TokenIntegrationStatus {
        configured: token_configured || authenticated,
        authenticated,
        account,
        base_url: Some(base_url),
        token_configured,
    }
}

pub async fn preflight() -> HostedIntegrationStatuses {
    let Ok(client) = client() else {
        return HostedIntegrationStatuses::default();
    };
    let gh_runner = suaegi_forge::GhRunner::new();
    let glab_runner = suaegi_forge::GlabRunner::new();
    let (github, gitlab, gitea, azure_dev_ops, bitbucket) = tokio::join!(
        suaegi_forge::preflight(&gh_runner),
        suaegi_forge::glab_preflight(&glab_runner),
        gitea(&client),
        azure_dev_ops(&client),
        bitbucket(&client)
    );
    HostedIntegrationStatuses {
        github: match github {
            suaegi_forge::Preflight::Ready => CliIntegrationStatus::Connected,
            suaegi_forge::Preflight::NotInstalled => CliIntegrationStatus::NotInstalled,
            suaegi_forge::Preflight::NotAuthenticated => CliIntegrationStatus::NotAuthenticated,
            suaegi_forge::Preflight::OutdatedVersion { found, min } => {
                CliIntegrationStatus::OutdatedVersion { found, min }
            }
        },
        gitlab: match gitlab {
            suaegi_forge::GlabPreflight::Ready => CliIntegrationStatus::Connected,
            suaegi_forge::GlabPreflight::NotInstalled => CliIntegrationStatus::NotInstalled,
            suaegi_forge::GlabPreflight::NotAuthenticated => CliIntegrationStatus::NotAuthenticated,
            suaegi_forge::GlabPreflight::OutdatedVersion { found, min } => {
                CliIntegrationStatus::OutdatedVersion { found, min }
            }
        },
        gitea,
        azure_dev_ops,
        bitbucket,
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_azure_dev_ops_api_base_url, normalize_gitea_api_base_url};

    #[test]
    fn provider_base_urls_match_orca_normalization() {
        assert_eq!(
            normalize_gitea_api_base_url(" https://git.example/code/ "),
            "https://git.example/code/api/v1"
        );
        assert_eq!(
            normalize_gitea_api_base_url("https://git.example/api/v1"),
            "https://git.example/api/v1"
        );
        assert_eq!(
            normalize_azure_dev_ops_api_base_url(" https://dev.azure.com/acme/_apis/ "),
            "https://dev.azure.com/acme"
        );
    }
}
