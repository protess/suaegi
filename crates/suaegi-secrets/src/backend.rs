//! OS 키체인 뒤에 두는 트레잇 + 실제 `keyring` 구현 + 테스트용 인메모리 fake.
//!
//! `keyring`은 CI에서 못 돌린다(진짜 키체인/데몬이 필요). 그래서 저장소를 [`KeychainBackend`]
//! 뒤에 숨기고, precedence/fallback 로직은 [`FakeKeychain`] + 통제된 env로 검증한다. 진짜
//! 키체인 왕복은 사람 눈으로 확인한다.

use crate::secret::Secret;
use std::collections::HashMap;
use std::sync::Mutex;

/// 키체인 백엔드 실패. **토큰을 절대 담지 않는다** — 카테고리 라벨만 담는 정제된 값이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainError {
    /// 이 시스템에 자격 저장소가 없음/접근 불가(헤드리스 리눅스에 secret-service 데몬 없음 등).
    /// **예상된 상황** — env fallback으로 조용히 넘어간다. `not-found`와 함께 env로 떨어지되
    /// 사용자에게 굳이 알리지 않는다.
    Unavailable,
    /// 진짜 저장소 실패. 정제된 카테고리 라벨(원본 에러/토큰 아님). env로 떨어지되 **표면화**한다.
    Backend(&'static str),
}

/// OS 자격 저장소 추상. 실제 구현은 [`KeyringBackend`], 테스트는 [`FakeKeychain`].
pub trait KeychainBackend {
    /// `Ok(Some)` = 찾음, `Ok(None)` = **없음(not-found)**, `Err` = 사용 불가 또는 진짜 오류.
    /// not-found와 error를 반환 타입 수준에서 구분하는 게 핵심이다.
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, KeychainError>;
    fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), KeychainError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError>;
}

// ---------------------------------------------------------------------------
// 실제 keyring 백엔드
// ---------------------------------------------------------------------------

/// `keyring` 크레이트를 쓰는 실제 OS 키체인 백엔드(피처는 타깃별, Cargo.toml 참고).
pub struct KeyringBackend;

impl KeyringBackend {
    fn entry(service: &str, account: &str) -> Result<keyring::Entry, KeychainError> {
        keyring::Entry::new(service, account).map_err(map_err)
    }
}

impl KeychainBackend for KeyringBackend {
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, KeychainError> {
        let entry = Self::entry(service, account)?;
        match entry.get_password() {
            Ok(pw) => Ok(Some(Secret::new(pw))),
            // 자격이 없음 → not-found. **Unavailable/Backend과 반드시 구분**된다.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), KeychainError> {
        let entry = Self::entry(service, account)?;
        entry.set_password(secret.expose()).map_err(map_err)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        let entry = Self::entry(service, account)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            // 이미 없으면 삭제는 성공으로 본다(멱등).
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }
}

/// `keyring::Error`를 **토큰 없는** 정제된 [`KeychainError`]로 매핑한다. 원본 Display를 쓰지
/// 않고 고정 카테고리 라벨만 낸다(결정적이고, 우연한 값 노출을 원천 차단).
///
/// `NoStorageAccess` → 저장소 부재/접근 불가로 보고 [`KeychainError::Unavailable`](env로 조용히
/// fallback). 그 밖의 실패는 [`KeychainError::Backend`]로 **표면화**한다. 리눅스에서 데몬 부재가
/// 정확히 어느 변이로 오는지는 이 호스트에서 빌드 못 하므로 사람 눈 확인 대상이다.
fn map_err(e: keyring::Error) -> KeychainError {
    match e {
        keyring::Error::NoStorageAccess(_) => KeychainError::Unavailable,
        keyring::Error::NoEntry => KeychainError::Backend("no-entry"),
        keyring::Error::PlatformFailure(_) => KeychainError::Backend("platform-failure"),
        keyring::Error::Ambiguous(_) => KeychainError::Backend("ambiguous-credential"),
        keyring::Error::BadEncoding(_) => KeychainError::Backend("bad-encoding"),
        keyring::Error::TooLong(_, _) => KeychainError::Backend("attribute-too-long"),
        keyring::Error::Invalid(_, _) => KeychainError::Backend("invalid-attribute"),
        _ => KeychainError::Backend("keychain-error"),
    }
}

// ---------------------------------------------------------------------------
// 테스트용 인메모리 fake
// ---------------------------------------------------------------------------

/// 인메모리 키체인. precedence/fallback 로직을 진짜 데몬 없이 검증하기 위한 것.
/// `fail_*`로 특정 연산이 Unavailable/Backend 오류를 내도록 주입할 수 있다.
#[derive(Default)]
pub struct FakeKeychain {
    entries: Mutex<HashMap<(String, String), Secret>>,
    get_error: Option<KeychainError>,
    set_error: Option<KeychainError>,
    delete_error: Option<KeychainError>,
}

impl FakeKeychain {
    pub fn new() -> Self {
        Self::default()
    }

    /// 저장된 자격을 미리 심는다(빌더).
    pub fn with(mut self, service: &str, account: &str, secret: &str) -> Self {
        self.entries
            .get_mut()
            .unwrap()
            .insert((service.to_string(), account.to_string()), Secret::new(secret));
        self
    }

    /// `get`이 항상 이 오류를 내게 한다(Unavailable/Backend 분기 테스트용).
    pub fn fail_get(mut self, err: KeychainError) -> Self {
        self.get_error = Some(err);
        self
    }

    pub fn fail_set(mut self, err: KeychainError) -> Self {
        self.set_error = Some(err);
        self
    }

    pub fn fail_delete(mut self, err: KeychainError) -> Self {
        self.delete_error = Some(err);
        self
    }
}

impl KeychainBackend for FakeKeychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, KeychainError> {
        if let Some(err) = &self.get_error {
            return Err(err.clone());
        }
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(&(service.to_string(), account.to_string()))
            .cloned())
    }

    fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), KeychainError> {
        if let Some(err) = &self.set_error {
            return Err(err.clone());
        }
        self.entries
            .lock()
            .unwrap()
            .insert((service.to_string(), account.to_string()), secret.clone());
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        if let Some(err) = &self.delete_error {
            return Err(err.clone());
        }
        self.entries
            .lock()
            .unwrap()
            .remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}
