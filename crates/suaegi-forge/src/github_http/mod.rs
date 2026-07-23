//! 7a-2b — 저장된 토큰으로 GitHub REST v3를 치는 **HTTP** 백엔드. gh CLI(`GhForge`)·
//! glab(`GlabForge`)과 **같은 `ForgeProvider`/`PrActions` 트레잇 뒤**에 들어오는 세 번째 impl.
//! James의 "두 전송을 한 트레잇 뒤에" 결정을 완성한다(gh 부재+토큰 존재 시 이 백엔드로 폴백).
//!
//! 전송은 주입 가능([`transport::HttpTransport`]) — 테스트는 fake로 real github.com을 안 친다.
//! 분류·None-vs-Unavailable 규율은 gh 백엔드와 동일하되 신호가 stderr 대신 **상태코드+헤더**다.

pub mod classify;
pub mod eligibility;
pub mod forge;
pub mod parse;

pub use eligibility::http_creation_eligibility;
pub use forge::{HttpGhForge, KEYCHAIN_ACCOUNT, KEYCHAIN_SERVICE};
// 전송 타입은 §Q5에서 `suaegi-http`로 추출됨. 기존 `suaegi_forge::{HttpTransport,ReqwestTransport}`
// 공개 경로를 유지하려 여기서 재수출한다(하위 크레이트 import churn 최소화).
pub use suaegi_http::{HttpTransport, ReqwestTransport};

/// GitHub 원격을 서빙할 백엔드 선택. gh(CLI)를 **우선**하고(이미 주력), gh가 없거나 미인증
/// 이지만 토큰이 있으면 HTTP로 폴백한다. 둘 다 아니면 gh(→ 기존 NotInstalled/NotAuthenticated
/// 표면). **순수 함수라 테스트 가능** — 실제 probe(preflight·시크릿 로드)는 `AnyForge::select`가 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubBackend {
    /// gh CLI 백엔드(`GhForge`).
    Gh,
    /// HTTP REST 백엔드(`HttpGhForge`).
    Http,
}

/// 선택 규율(브리프): gh 준비됨 → gh. gh 미준비 + 토큰 있음 → HTTP. 그 밖 → gh(폴백 표면).
pub fn choose_github_backend(gh_ready: bool, token_present: bool) -> GithubBackend {
    if gh_ready {
        GithubBackend::Gh
    } else if token_present {
        GithubBackend::Http
    } else {
        // 둘 다 없으면 gh로 — GhForge가 NotInstalled/NotAuthenticated를 그대로 표면화한다.
        GithubBackend::Gh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **핵심 회귀 방어 (d)**: gh-준비면 gh, gh-미준비+토큰이면 HTTP, 둘 다 없으면 gh.
    #[test]
    fn backend_selection_prefers_gh_then_http_then_gh_fallback() {
        // gh 준비되면 토큰 유무와 무관하게 gh.
        assert_eq!(choose_github_backend(true, false), GithubBackend::Gh);
        assert_eq!(choose_github_backend(true, true), GithubBackend::Gh);
        // gh 미준비 + 토큰 → HTTP.
        assert_eq!(choose_github_backend(false, true), GithubBackend::Http);
        // gh 미준비 + 토큰 없음 → gh(폴백 표면).
        assert_eq!(choose_github_backend(false, false), GithubBackend::Gh);
    }
}
