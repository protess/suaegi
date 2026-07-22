//! precedence + fallback + 리댁션 규율 테스트. 진짜 keyring은 못 돌리므로 [`FakeKeychain`] +
//! [`MapEnv`]로 로직을 고정한다(진짜 키체인 왕복은 사람 눈 확인). 각 테스트는 mutation-verify
//! 대상 — 로직을 뒤집으면 실패하도록 타이트하게 단언한다.

use suaegi_secrets::{
    delete_with, resolve, store_with, FakeKeychain, KeychainBackend, KeychainError, MapEnv, Secret,
    SecretError, SecretRequest, Source,
};

const TOKEN: &str = "ghp_SUPERSECRET_do_not_leak_0xDEADBEEF";

// ---------------------------------------------------------------------------
// 리댁션 규율: 토큰은 Debug/Display/에러 어디에도 안 나온다
// ---------------------------------------------------------------------------

#[test]
fn secret_debug_is_redacted() {
    let secret = Secret::new(TOKEN);
    let dbg = format!("{secret:?}");
    assert_eq!(dbg, "Secret(***)");
    assert!(!dbg.contains(TOKEN), "Debug leaked the token: {dbg}");
    // expose만이 값을 낸다.
    assert_eq!(secret.expose(), TOKEN);
}

#[test]
fn resolved_debug_does_not_leak_token() {
    // env fallback으로 실린 비밀도 Resolved Debug에서 가려져야 한다.
    let backend = FakeKeychain::new();
    let env = MapEnv::new().with("GH_TOKEN", TOKEN);
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);
    let dbg = format!("{resolved:?}");
    assert!(
        !dbg.contains(TOKEN),
        "Resolved Debug leaked the token: {dbg}"
    );
    assert!(dbg.contains("Secret(***)"), "expected redacted marker: {dbg}");
}

#[test]
fn secret_error_does_not_leak_token() {
    // store가 실패해도 반환 에러에 토큰이 없어야 한다. store_with는 토큰과 에러 라벨을
    // 분리해 다루므로 구조적으로도 새지 않는다 — 이 테스트가 그 불변식을 고정한다.
    let backend = FakeKeychain::new().fail_set(KeychainError::Backend("platform-failure"));
    let secret = Secret::new(TOKEN);

    let err = store_with(&backend, "suaegi", "github.com", &secret)
        .expect_err("set failure should surface as SecretError");

    let dbg = format!("{err:?}");
    let disp = format!("{err}");
    assert!(!dbg.contains(TOKEN), "SecretError Debug leaked token: {dbg}");
    assert!(
        !disp.contains(TOKEN),
        "SecretError Display leaked token: {disp}"
    );
    assert!(matches!(err, SecretError::Backend("platform-failure")));
}

// ---------------------------------------------------------------------------
// precedence: 키체인 > env
// ---------------------------------------------------------------------------

#[test]
fn keychain_hit_wins_over_env() {
    // 키체인과 env **둘 다** 값이 있을 때 키체인이 이긴다. env를 우선하도록 뒤집으면 실패.
    let backend = FakeKeychain::new().with("suaegi", "github.com", "from_keychain");
    let env = MapEnv::new().with("GH_TOKEN", "from_env");
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);

    assert_eq!(resolved.secret.as_ref().map(Secret::expose), Some("from_keychain"));
    assert_eq!(resolved.source, Some(Source::Keychain));
    assert_eq!(resolved.keychain_error, None);
}

// ---------------------------------------------------------------------------
// fallback: 키체인 miss/unavailable → env
// ---------------------------------------------------------------------------

#[test]
fn keychain_miss_falls_back_to_env() {
    // 키체인 not-found + env 있음 → env 값. env fallback을 건너뛰게 뒤집으면 실패.
    let backend = FakeKeychain::new(); // 비어 있음 → Ok(None)
    let env = MapEnv::new().with("GH_TOKEN", "from_env");
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);

    assert_eq!(resolved.secret.as_ref().map(Secret::expose), Some("from_env"));
    assert_eq!(resolved.source, Some(Source::Env("GH_TOKEN".to_string())));
    assert_eq!(resolved.keychain_error, None);
}

#[test]
fn keychain_unavailable_falls_back_to_env_without_error() {
    // 데몬 없음(Unavailable) + env 있음 → env 값, 오류는 표면화 안 함.
    let backend = FakeKeychain::new().fail_get(KeychainError::Unavailable);
    let env = MapEnv::new().with("GH_TOKEN", "from_env");
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);

    assert_eq!(resolved.secret.as_ref().map(Secret::expose), Some("from_env"));
    assert_eq!(resolved.source, Some(Source::Env("GH_TOKEN".to_string())));
    assert_eq!(
        resolved.keychain_error, None,
        "Unavailable은 예상된 상황 — 표면화하지 않는다"
    );
}

#[test]
fn env_vars_tried_in_order() {
    // GH_TOKEN이 GITHUB_TOKEN보다 우선. 순서를 뒤집으면 실패.
    let backend = FakeKeychain::new();
    let env = MapEnv::new()
        .with("GH_TOKEN", "primary")
        .with("GITHUB_TOKEN", "secondary");
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);
    assert_eq!(resolved.secret.as_ref().map(Secret::expose), Some("primary"));
    assert_eq!(resolved.source, Some(Source::Env("GH_TOKEN".to_string())));
}

#[test]
fn second_env_var_used_when_first_absent() {
    let backend = FakeKeychain::new();
    let env = MapEnv::new().with("GITHUB_TOKEN", "secondary"); // GH_TOKEN 없음
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);
    assert_eq!(resolved.secret.as_ref().map(Secret::expose), Some("secondary"));
    assert_eq!(resolved.source, Some(Source::Env("GITHUB_TOKEN".to_string())));
}

#[test]
fn both_absent_yields_none() {
    // 키체인도 env도 없음 → None.
    let backend = FakeKeychain::new();
    let env = MapEnv::new();
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);
    assert!(resolved.secret.is_none());
    assert_eq!(resolved.source, None);
    assert_eq!(resolved.keychain_error, None);
}

// ---------------------------------------------------------------------------
// error ≠ not-found: 진짜 오류는 구분되어 표면화된다
// ---------------------------------------------------------------------------

#[test]
fn keychain_backend_error_is_surfaced_and_falls_back_to_env() {
    // 진짜 저장소 오류 + env 있음 → env 값을 쓰되 keychain_error를 표면화한다.
    // Backend 오류를 not-found로 뭉개면(keychain_error=None) 실패.
    let backend = FakeKeychain::new().fail_get(KeychainError::Backend("platform-failure"));
    let env = MapEnv::new().with("GH_TOKEN", "from_env");
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);

    assert_eq!(resolved.secret.as_ref().map(Secret::expose), Some("from_env"));
    assert_eq!(resolved.source, Some(Source::Env("GH_TOKEN".to_string())));
    assert_eq!(
        resolved.keychain_error,
        Some(KeychainError::Backend("platform-failure")),
        "진짜 키체인 오류는 not-found와 달리 표면화되어야 한다"
    );
}

#[test]
fn keychain_backend_error_surfaced_even_without_env() {
    // 진짜 오류 + env 없음 → secret None이지만 keychain_error는 Some.
    let backend = FakeKeychain::new().fail_get(KeychainError::Backend("platform-failure"));
    let env = MapEnv::new();
    let req = SecretRequest::github("suaegi", "github.com");

    let resolved = resolve(&backend, &env, &req);
    assert!(resolved.secret.is_none());
    assert_eq!(
        resolved.keychain_error,
        Some(KeychainError::Backend("platform-failure"))
    );
}

// ---------------------------------------------------------------------------
// store / load / delete 왕복 (주입된 백엔드)
// ---------------------------------------------------------------------------

#[test]
fn store_then_load_roundtrip() {
    let backend = FakeKeychain::new();
    let env = MapEnv::new();
    let req = SecretRequest::new("suaegi", "github.com");

    store_with(&backend, "suaegi", "github.com", &Secret::new(TOKEN)).expect("store");

    let resolved = resolve(&backend, &env, &req);
    assert_eq!(resolved.secret.as_ref().map(Secret::expose), Some(TOKEN));
    assert_eq!(resolved.source, Some(Source::Keychain));
}

#[test]
fn delete_removes_from_keychain() {
    let backend = FakeKeychain::new().with("suaegi", "github.com", TOKEN);
    let env = MapEnv::new();
    let req = SecretRequest::new("suaegi", "github.com");

    // 존재 확인.
    assert!(resolve(&backend, &env, &req).secret.is_some());

    delete_with(&backend, "suaegi", "github.com").expect("delete");

    // 삭제 후 없음.
    let resolved = resolve(&backend, &env, &req);
    assert!(resolved.secret.is_none());
}

#[test]
fn delete_error_maps_to_secret_error() {
    let backend = FakeKeychain::new().fail_delete(KeychainError::Unavailable);
    let err = delete_with(&backend, "suaegi", "github.com").expect_err("delete should fail");
    assert!(matches!(err, SecretError::Unavailable));
}

// FakeKeychain가 KeychainBackend 트레잇을 실제로 구현함을 컴파일 타임에 고정(트레잇 객체화 가능).
fn _assert_object_safe(_: &dyn KeychainBackend) {}
