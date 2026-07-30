//! Secret storage coordinates for web-backed provider usage meters.
//!
//! Cookies never enter `UiSettings`: only non-secret workspace/group/model
//! preferences are serialized. This keeps Settings parity without copying
//! Orca's legacy encrypted-string field into the plain JSON store.

use suaegi_secrets::{Resolved, Secret, SecretRequest};

const SERVICE: &str = "com.suaegi.provider-usage";
const OPENCODE_ACCOUNT: &str = "opencode-go-session-cookie";
const MINIMAX_ACCOUNT: &str = "minimax-session-cookie";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderSecret {
    OpenCodeGo,
    MiniMax,
}

impl ProviderSecret {
    fn account(self) -> &'static str {
        match self {
            Self::OpenCodeGo => OPENCODE_ACCOUNT,
            Self::MiniMax => MINIMAX_ACCOUNT,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenCodeGo => "OpenCode Go",
            Self::MiniMax => "MiniMax",
        }
    }
}

pub fn request(provider: ProviderSecret) -> SecretRequest {
    SecretRequest::new(SERVICE, provider.account())
}

pub fn load(provider: ProviderSecret) -> Resolved {
    suaegi_secrets::load(&request(provider))
}

pub fn has(provider: ProviderSecret) -> bool {
    load(provider).secret.is_some()
}

pub fn store(provider: ProviderSecret, value: String) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{} cookie is required.", provider.label()));
    }
    suaegi_secrets::store(SERVICE, provider.account(), &Secret::new(value))
        .map_err(|_| format!("Could not save the {} cookie securely.", provider.label()))
}

pub fn clear(provider: ProviderSecret) -> Result<(), String> {
    suaegi_secrets::delete(SERVICE, provider.account())
        .map_err(|_| format!("Could not clear the {} cookie.", provider.label()))
}
