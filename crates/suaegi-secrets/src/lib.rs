//! suaegi-secrets — suaegi의 **첫 비밀 저장** 하위 시스템. OS 키체인을 우선 쓰고, 키체인이
//! 없거나(헤드리스/SSH 리눅스, secret-service 데몬 없음) 값이 없으면 지정된 env 변수로 fallback.
//!
//! # 불변식(보안 크리티컬)
//! - 토큰은 suaegi의 평문 JSON 영속화(`suaegi-core/persistence.rs`)에 **절대** 안 들어간다.
//! - 토큰은 로그·에러·`Debug` 출력에 **절대** 안 나온다([`Secret`]가 타입 레벨로 강제).
//!
//! # 핵심 로직: precedence + fallback ([`resolve`])
//! 키체인 **우선**(env보다), 그리고 키체인이 unavailable **또는** not-found면 env로 fallback.
//! 키체인의 *진짜 오류*는 *not-found*와 구분해 [`Resolved::keychain_error`]로 표면화한다 —
//! 사용자가 "저장이 깨졌다"를 알 수 있게(단, 토큰 없이).
//!
//! # 레이어링
//! 이 크레이트는 내부 크레이트에 의존하지 않는다(thiserror + keyring 뿐). 그래서 `suaegi-forge`가
//! 사이클 없이 이걸 의존할 수 있다(7a-2b HTTP GitHub 구현이 토큰을 여기서 읽는다).

mod backend;
mod env;
mod secret;

pub use backend::{FakeKeychain, KeychainBackend, KeychainError, KeyringBackend};
pub use env::{EnvLookup, MapEnv, ProcessEnv};
pub use secret::Secret;

/// 비밀 하나를 어떻게 찾을지: 키체인 좌표(service, account) + 키체인 미스/부재 시 순서대로
/// 시도할 env 변수 목록.
#[derive(Debug, Clone)]
pub struct SecretRequest {
    pub service: String,
    pub account: String,
    /// 키체인 miss/unavailable 후 **순서대로** 시도할 env 변수들. 예: GitHub → `GH_TOKEN`,
    /// 그다음 `GITHUB_TOKEN`(브리프는 `Option<&str>`였으나 gh CLI 관례를 따르려면 순서 있는
    /// 목록이 필요해 확장했다 — 리포트의 deviation 참고).
    pub env_vars: Vec<String>,
}

impl SecretRequest {
    /// GitHub 토큰 요청. env fallback은 gh CLI 관례대로 `GH_TOKEN` → `GITHUB_TOKEN` 순.
    pub fn github(service: &str, account: &str) -> Self {
        Self {
            service: service.to_string(),
            account: account.to_string(),
            env_vars: vec!["GH_TOKEN".to_string(), "GITHUB_TOKEN".to_string()],
        }
    }

    /// env fallback 없는 요청(키체인만).
    pub fn new(service: &str, account: &str) -> Self {
        Self {
            service: service.to_string(),
            account: account.to_string(),
            env_vars: Vec::new(),
        }
    }

    /// env fallback 목록을 지정한다(순서대로 시도).
    pub fn with_env_vars(mut self, vars: &[&str]) -> Self {
        self.env_vars = vars.iter().map(|s| s.to_string()).collect();
        self
    }
}

/// 비밀이 어디서 왔는지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Keychain,
    /// 이 env 변수 이름이 값을 제공.
    Env(String),
}

/// [`resolve`]/[`load`]의 결과. 브리프의 `Result<Option<Secret>, _>`보다 풍부하다 — 그래야
/// (a) 값, (b) 출처, (c) 표면화된 키체인 오류를 **동시에** 실을 수 있고, error-vs-not-found
/// 구분을 반환값에서 테스트할 수 있다(리포트의 deviation 참고).
///
/// `Debug` 파생은 안전하다: `Secret`의 Debug가 값을 가리므로 `Resolved`도 토큰을 안 찍는다.
#[derive(Debug)]
pub struct Resolved {
    /// 찾은 비밀. 키체인·env 어디에도 없으면 `None`.
    pub secret: Option<Secret>,
    /// 값의 출처. `secret`이 `Some`일 때만 `Some`.
    pub source: Option<Source>,
    /// 키체인의 **진짜 오류**를 정제한 라벨. not-found/unavailable이면 `None`. `Some`이면
    /// env로 fallback은 했지만 저장소가 깨졌음을 뜻한다 — UI/로그가 사용자에게 알릴 수 있다.
    pub keychain_error: Option<KeychainError>,
}

/// **핵심 로직.** 키체인 우선, 그다음 env fallback. 주입된 백엔드/env로 테스트한다.
///
/// 순서:
/// 1. 키체인 `get` →
///    - `Ok(Some)` → 그 값을 반환(**env는 보지 않는다** — precedence: 키체인 > env).
///    - `Ok(None)`(not-found) → env로 fallback, 오류 없음.
///    - `Err(Unavailable)`(데몬 없음) → env로 fallback, 오류 **표면화 안 함**(예상된 상황).
///    - `Err(Backend)`(진짜 오류) → env로 fallback하되 오류를 **표면화**한다.
/// 2. env fallback: `env_vars`를 순서대로 시도, 첫 번째 값이 이긴다.
/// 3. 둘 다 없으면 `secret: None`.
pub fn resolve(
    backend: &dyn KeychainBackend,
    env: &dyn EnvLookup,
    request: &SecretRequest,
) -> Resolved {
    let keychain_error = match backend.get(&request.service, &request.account) {
        // 키체인 히트 → 즉시 반환. env fallback으로 내려가지 않는다(precedence 고정점).
        Ok(Some(secret)) => {
            return Resolved {
                secret: Some(secret),
                source: Some(Source::Keychain),
                keychain_error: None,
            };
        }
        // not-found: env로 내려가되 오류는 없다.
        Ok(None) => None,
        // unavailable(데몬 없음): env로 내려가되 사용자에게 굳이 알리지 않는다.
        Err(KeychainError::Unavailable) => None,
        // 진짜 저장소 오류: env로 내려가되 표면화한다(not-found와 **구분**).
        Err(err @ KeychainError::Backend(_)) => Some(err),
    };

    // env fallback — 순서대로, 첫 값이 이긴다.
    for var in &request.env_vars {
        if let Some(value) = env.get(var) {
            return Resolved {
                secret: Some(Secret::new(value)),
                source: Some(Source::Env(var.clone())),
                keychain_error,
            };
        }
    }

    Resolved {
        secret: None,
        source: None,
        keychain_error,
    }
}

/// store/delete가 반환하는 실패. **토큰을 절대 담지 않는다**.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// 이 플랫폼에 자격 저장소가 없음/접근 불가.
    #[error("secret storage is unavailable on this platform")]
    Unavailable,
    /// 진짜 저장소 실패. 메시지는 고정 카테고리 라벨(원본 에러/토큰 아님).
    #[error("secret storage failed: {0}")]
    Backend(&'static str),
}

impl From<KeychainError> for SecretError {
    fn from(e: KeychainError) -> Self {
        match e {
            KeychainError::Unavailable => SecretError::Unavailable,
            KeychainError::Backend(label) => SecretError::Backend(label),
        }
    }
}

/// 주입 가능한 store(테스트용 진입점).
pub fn store_with(
    backend: &dyn KeychainBackend,
    service: &str,
    account: &str,
    secret: &Secret,
) -> Result<(), SecretError> {
    backend.set(service, account, secret).map_err(Into::into)
}

/// 주입 가능한 delete(테스트용 진입점).
pub fn delete_with(
    backend: &dyn KeychainBackend,
    service: &str,
    account: &str,
) -> Result<(), SecretError> {
    backend.delete(service, account).map_err(Into::into)
}

/// 비밀을 OS 키체인에 쓴다.
pub fn store(service: &str, account: &str, secret: &Secret) -> Result<(), SecretError> {
    store_with(&KeyringBackend, service, account, secret)
}

/// 비밀을 읽는다: 키체인 우선, 그다음 `request.env_vars` fallback. 자세한 순서는 [`resolve`].
pub fn load(request: &SecretRequest) -> Resolved {
    resolve(&KeyringBackend, &ProcessEnv, request)
}

/// 키체인에서 비밀을 지운다.
pub fn delete(service: &str, account: &str) -> Result<(), SecretError> {
    delete_with(&KeyringBackend, service, account)
}
