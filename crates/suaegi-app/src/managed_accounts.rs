use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use suaegi_core::domain::ManagedProviderAccountSetting;

#[cfg(target_os = "macos")]
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn fallback_label(self) -> &'static str {
        match self {
            Self::Claude => "Claude account",
            Self::Codex => "Codex account",
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn accounts_root() -> PathBuf {
    crate::persistence_thread::default_data_file()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("provider-accounts")
}

fn source_config_dir(provider: Provider) -> Option<PathBuf> {
    let env_name = match provider {
        Provider::Claude => "CLAUDE_CONFIG_DIR",
        Provider::Codex => "CODEX_HOME",
    };
    std::env::var_os(env_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            dirs::home_dir().map(|home| {
                home.join(match provider {
                    Provider::Claude => ".claude",
                    Provider::Codex => ".codex",
                })
            })
        })
}

fn credentials_filename(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => ".credentials.json",
        Provider::Codex => "auth.json",
    }
}

#[cfg(target_os = "macos")]
fn read_keychain_password(service: &str, account: &str) -> Option<Vec<u8>> {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .ok()?;
    (output.status.success() && !output.stdout.is_empty()).then_some(output.stdout)
}

fn read_credentials_from_config(
    provider: Provider,
    config_dir: &Path,
    allow_legacy_claude_keychain: bool,
) -> Result<Vec<u8>, String> {
    let source = config_dir.join(credentials_filename(provider));
    match fs::read(source) {
        Ok(contents) if !contents.is_empty() => return Ok(contents),
        Ok(_) | Err(_) if provider == Provider::Claude => {}
        Ok(_) | Err(_) => return Err("The provider is not signed in on this device.".to_string()),
    }

    #[cfg(target_os = "macos")]
    {
        let account = keychain_user();
        let scoped = claude_scoped_keychain_service(config_dir);
        if let Some(credentials) = read_keychain_password(&scoped, &account) {
            return Ok(credentials);
        }
        if allow_legacy_claude_keychain {
            if let Some(credentials) = read_keychain_password("Claude Code-credentials", &account) {
                return Ok(credentials);
            }
        }
    }
    Err("Claude Code is not signed in on this device.".to_string())
}

fn read_system_credentials(provider: Provider) -> Result<Vec<u8>, String> {
    let config_dir = source_config_dir(provider)
        .ok_or_else(|| "Could not resolve the provider configuration directory.".to_string())?;
    read_credentials_from_config(provider, &config_dir, true)
}

fn jwt_email(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    ["email", "https://api.openai.com/profile"]
        .into_iter()
        .find_map(|key| match json.get(key) {
            Some(serde_json::Value::String(email)) if email.contains('@') => Some(email.clone()),
            Some(serde_json::Value::Object(profile)) => profile
                .get("email")
                .and_then(serde_json::Value::as_str)
                .filter(|email| email.contains('@'))
                .map(ToOwned::to_owned),
            _ => None,
        })
}

fn account_email(provider: Provider, credentials: &[u8]) -> String {
    let json: serde_json::Value = serde_json::from_slice(credentials).unwrap_or_default();
    let candidates = match provider {
        Provider::Claude => vec![
            json.pointer("/claudeAiOauth/accessToken"),
            json.pointer("/claudeAiOauth/idToken"),
        ],
        Provider::Codex => vec![
            json.pointer("/tokens/id_token"),
            json.pointer("/tokens/access_token"),
        ],
    };
    candidates
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .find_map(jwt_email)
        .or_else(|| {
            json.pointer("/email")
                .and_then(serde_json::Value::as_str)
                .filter(|email| email.contains('@'))
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| provider.fallback_label().to_string())
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Could not secure the managed account directory.".to_string())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn secure_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Could not create the managed credential file.".to_string())?;
    file.write_all(contents)
        .map_err(|_| "Could not store the managed credentials.".to_string())?;
    file.sync_all()
        .map_err(|_| "Could not finish storing the managed credentials.".to_string())
}

#[cfg(target_os = "macos")]
fn keychain_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

#[cfg(target_os = "macos")]
fn claude_scoped_keychain_service(config_dir: &Path) -> String {
    let digest = Sha256::digest(config_dir.to_string_lossy().as_bytes());
    let suffix = format!(
        "{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    );
    format!("Claude Code-credentials-{suffix}")
}

#[cfg(target_os = "macos")]
fn add_keychain_password(service: &str, account: &str, contents: &[u8]) -> Result<(), String> {
    let contents = std::str::from_utf8(contents)
        .map_err(|_| "The Claude credential payload is not UTF-8.".to_string())?;
    let status = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            service,
            "-a",
            account,
            "-w",
            contents,
        ])
        .status()
        .map_err(|_| "Could not write the Claude Code Keychain item.".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Could not write the Claude Code Keychain item.".to_string())
    }
}

#[cfg(target_os = "macos")]
fn delete_keychain_password(service: &str, account: &str) {
    let _ = Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-s", service, "-a", account])
        .status();
}

#[cfg(target_os = "macos")]
fn store_claude_keychain_credentials(
    account_id: &str,
    config_dir: &Path,
    credentials: &[u8],
) -> Result<(), String> {
    // Claude Code 2.1 scopes its active Keychain service to the first eight
    // SHA-256 hex characters of CLAUDE_CONFIG_DIR. Keep a Suaegi-owned copy as
    // well so subsequent re-authentication can be made transactional.
    add_keychain_password(
        "Suaegi Claude Code Managed Credentials",
        account_id,
        credentials,
    )?;
    let service = claude_scoped_keychain_service(config_dir);
    if let Err(error) = add_keychain_password(&service, &keychain_user(), credentials) {
        delete_keychain_password("Suaegi Claude Code Managed Credentials", account_id);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn store_claude_keychain_credentials(
    _account_id: &str,
    _config_dir: &Path,
    _credentials: &[u8],
) -> Result<(), String> {
    Ok(())
}

fn is_owned_account_path(provider: Provider, account_id: &str, path: &Path) -> bool {
    let expected_parent = accounts_root().join(provider.id());
    path.parent() == Some(expected_parent.as_path())
        && path.file_name().and_then(|value| value.to_str()) == Some(account_id)
}

fn remove_managed_credentials(
    provider: Provider,
    account: &ManagedProviderAccountSetting,
) -> Result<(), String> {
    let path = Path::new(&account.config_dir);
    if !is_owned_account_path(provider, &account.id, path) {
        return Err("Refused to remove an untrusted managed account path.".to_string());
    }
    if path.exists() {
        fs::remove_dir_all(path)
            .map_err(|_| "Could not remove the managed account directory.".to_string())?;
    }
    #[cfg(target_os = "macos")]
    if provider == Provider::Claude {
        delete_keychain_password("Suaegi Claude Code Managed Credentials", &account.id);
        delete_keychain_password(&claude_scoped_keychain_service(path), &keychain_user());
    }
    Ok(())
}

pub fn import_system_account(provider: Provider) -> Result<ManagedProviderAccountSetting, String> {
    let credentials = read_system_credentials(provider)?;
    // Parse before writing so malformed credential blobs never become selectable.
    serde_json::from_slice::<serde_json::Value>(&credentials)
        .map_err(|_| "The provider credential file is not valid JSON.".to_string())?;
    let timestamp = now_ms();
    let id = format!("{}-{timestamp}", provider.id());
    let config_dir = accounts_root().join(provider.id()).join(&id);
    fs::create_dir_all(&config_dir)
        .map_err(|_| "Could not create the managed account directory.".to_string())?;
    secure_directory(&config_dir)?;
    if let Err(error) = secure_write(
        &config_dir.join(credentials_filename(provider)),
        &credentials,
    ) {
        let _ = fs::remove_dir(&config_dir);
        return Err(error);
    }
    if provider == Provider::Claude {
        if let Err(error) = store_claude_keychain_credentials(&id, &config_dir, &credentials) {
            let _ = fs::remove_dir_all(&config_dir);
            return Err(error);
        }
    }
    Ok(ManagedProviderAccountSetting {
        id,
        email: account_email(provider, &credentials),
        config_dir: config_dir.to_string_lossy().into_owned(),
        created_at_unix_ms: timestamp,
        updated_at_unix_ms: timestamp,
        last_authenticated_at_unix_ms: timestamp,
    })
}

fn run_login(provider: Provider, config_dir: &Path) -> Result<(), String> {
    let (binary, args, environment_name) = match provider {
        Provider::Claude => (
            "claude",
            ["auth", "login", "--claudeai"].as_slice(),
            "CLAUDE_CONFIG_DIR",
        ),
        Provider::Codex => ("codex", ["login"].as_slice(), "CODEX_HOME"),
    };
    let mut child = Command::new(binary)
        .args(args)
        .env(environment_name, config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("{binary} is not installed or could not be started."))?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(format!("{binary} sign-in did not complete.")),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{binary} sign-in timed out."));
            }
            Err(_) => return Err(format!("Could not wait for {binary} sign-in.")),
        }
    }
}

pub fn add_account(provider: Provider) -> Result<ManagedProviderAccountSetting, String> {
    let timestamp = now_ms();
    let id = format!("{}-{timestamp}", provider.id());
    let config_dir = accounts_root().join(provider.id()).join(&id);
    fs::create_dir_all(&config_dir)
        .map_err(|_| "Could not create the managed account directory.".to_string())?;
    secure_directory(&config_dir)?;
    if let Err(error) = run_login(provider, &config_dir) {
        let _ = fs::remove_dir_all(&config_dir);
        return Err(error);
    }
    let credentials = match read_credentials_from_config(provider, &config_dir, false) {
        Ok(credentials) => credentials,
        Err(error) => {
            let _ = fs::remove_dir_all(&config_dir);
            return Err(error);
        }
    };
    serde_json::from_slice::<serde_json::Value>(&credentials)
        .map_err(|_| "The provider credential file is not valid JSON.".to_string())?;
    if provider == Provider::Claude {
        store_claude_keychain_credentials(&id, &config_dir, &credentials)?;
        let credential_file = config_dir.join(credentials_filename(provider));
        if !credential_file.exists() {
            secure_write(&credential_file, &credentials)?;
        }
    }
    Ok(ManagedProviderAccountSetting {
        id,
        email: account_email(provider, &credentials),
        config_dir: config_dir.to_string_lossy().into_owned(),
        created_at_unix_ms: timestamp,
        updated_at_unix_ms: timestamp,
        last_authenticated_at_unix_ms: timestamp,
    })
}

pub fn reauthenticate_account(
    provider: Provider,
    account: &ManagedProviderAccountSetting,
) -> Result<ManagedProviderAccountSetting, String> {
    let config_dir = Path::new(&account.config_dir);
    if !is_owned_account_path(provider, &account.id, config_dir) {
        return Err("Refused to authenticate an untrusted managed account path.".to_string());
    }
    run_login(provider, config_dir)?;
    let credentials = read_credentials_from_config(provider, config_dir, false)?;
    serde_json::from_slice::<serde_json::Value>(&credentials)
        .map_err(|_| "The provider credential file is not valid JSON.".to_string())?;
    if provider == Provider::Claude {
        store_claude_keychain_credentials(&account.id, config_dir, &credentials)?;
    }
    let timestamp = now_ms();
    let mut refreshed = account.clone();
    refreshed.email = account_email(provider, &credentials);
    refreshed.updated_at_unix_ms = timestamp;
    refreshed.last_authenticated_at_unix_ms = timestamp;
    Ok(refreshed)
}

pub fn discard_imported_account(provider: Provider, account: &ManagedProviderAccountSetting) {
    let _ = remove_managed_credentials(provider, account);
}

pub fn remove_account(
    provider: Provider,
    account: &ManagedProviderAccountSetting,
) -> Result<(), String> {
    remove_managed_credentials(provider, account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_email_reads_standard_email_claim_without_exposing_the_token() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"ada@example.com"}"#);
        assert_eq!(
            jwt_email(&format!("header.{payload}.signature")).as_deref(),
            Some("ada@example.com")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn claude_keychain_service_matches_the_cli_config_dir_hash_contract() {
        assert_eq!(
            claude_scoped_keychain_service(Path::new("/tmp/managed-claude")),
            "Claude Code-credentials-0e629078"
        );
    }

    #[test]
    fn account_removal_rejects_paths_outside_the_private_provider_root() {
        let account = ManagedProviderAccountSetting {
            id: "claude-1".to_string(),
            email: "ada@example.com".to_string(),
            config_dir: "/tmp/not-owned-by-suaegi/claude-1".to_string(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            last_authenticated_at_unix_ms: 1,
        };
        assert!(remove_account(Provider::Claude, &account).is_err());
    }
}
