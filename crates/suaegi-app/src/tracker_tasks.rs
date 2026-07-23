//! Tracker(N1 Linear) 네트워크를 **UI 스레드 밖에서** 돌리고 결과를 `Message`로 태워
//! 돌려주는 얇은 배선. `forge_tasks.rs`와 **같은 패턴**이다(async fn → `Task::perform`
//! → `Message`). Linear GraphQL 호출은 네트워크라 절대 `update` 루프에서 직접 부르지 않는다.
//!
//! 위젯/리듀서가 검사하는 순수 결정은 `tracker_ui.rs`에 있고, 여기 있는 것은 실제로
//! api.linear.app을 때리는(그래서 헤드리스로 단언 불가능한) 접착제뿐이라 최대한 얇게 둔다.
//!
//! **API 키 규율**: 키는 [`suaegi_secrets::Secret`]로만 다루고 성공 시 키체인에 저장한다
//! (`suaegi-linear` 서비스). 평문 JSON·로그·Debug 어디에도 안 새고, `expose()`는 오직
//! `LinearClient::auth_headers`(tracker 크레이트)에서만 불린다.

use std::sync::Arc;

use iced::Task;
use suaegi_http::ReqwestTransport;
use suaegi_secrets::{Secret, SecretRequest};
use suaegi_tracker::jira::KEYCHAIN_SERVICE as JIRA_KEYCHAIN_SERVICE;
use suaegi_tracker::linear::KEYCHAIN_SERVICE;
use suaegi_tracker::{
    IssuePage, JiraClient, JiraConnection, JiraIssue, JiraIssueFilter, JiraPage, JiraViewer,
    LinearClient, LinearWorkspace, Lookup,
};

use crate::state::{Message, OpId};

/// 키체인 account. v1은 단일 워크스페이스라 고정값을 쓴다. 멀티-워크스페이스 구분(account를
/// url_key로)은 후속 — 리포트의 deviation 참고.
pub const LINEAR_ACCOUNT: &str = "default";
/// 키체인 miss/부재 시 fallback env 변수(헤드리스/CI). forge의 `GH_TOKEN` 관례 미러.
pub const LINEAR_ENV_VAR: &str = "LINEAR_API_KEY";

/// 저장된(또는 env fallback) Linear 키 요청. 부팅 시 재연결에 쓴다.
pub fn secret_request() -> SecretRequest {
    SecretRequest::new(KEYCHAIN_SERVICE, LINEAR_ACCOUNT).with_env_vars(&[LINEAR_ENV_VAR])
}

/// 실 전송으로 인증된 클라이언트를 만든다. 전송(`ReqwestTransport`)은 stateless라 매 호출
/// 새로 만든다(forge가 `AnyForge::select`를 매번 하는 것과 같은 얼개).
fn client(token: Secret) -> LinearClient {
    LinearClient::with_transport(Arc::new(ReqwestTransport::new()), Some(token))
}

/// 연결 확인 + (성공 시) 키 저장. **저장은 성공했을 때만** 한다 — 무효 키를 키체인에 남기지
/// 않는다(브리프의 "store → test"에서 벗어난 의도적 강화, 리포트 참고). best-effort 저장이라
/// 키체인이 없어도(헤드리스) 연결 자체는 성립한다(이번 세션은 메모리 토큰으로 동작).
pub async fn connect_now(token: Secret) -> Lookup<LinearWorkspace> {
    let result = client(token.clone()).test_connection().await;
    if matches!(result, Lookup::Found(_)) {
        // 저장 실패는 삼킨다: 표면화할 raw 에러가 토큰을 흘릴 수 있고, 이번 세션은 메모리
        // 토큰으로 이미 동작한다. 저장은 다음 실행의 재연결 편의일 뿐이다.
        let _ = suaegi_secrets::store(KEYCHAIN_SERVICE, LINEAR_ACCOUNT, &token);
    }
    result
}

/// 이슈 목록. v1은 필터 없이 워크스페이스 이슈를 bounded traversal(≤250)로 가져온다.
/// assignee/state 필터링은 후속 — 잘못된 필터는 Unavailable로 안전하게 떨어지지만(회귀
/// 방지) 지금은 검증된 `None`만 보낸다. `has_more`는 tracker 클라이언트가 표면화한다.
pub async fn list_issues_now(token: Secret) -> Lookup<IssuePage> {
    client(token).list_issues(None).await
}

// ---- 얇은 Task<Message> 래퍼: 검사 불가능한 접착제, 최대한 작게. ----

/// 연결(또는 부팅 시 재연결)을 발급한다.
pub fn connect(op: OpId, token: Secret) -> Task<Message> {
    Task::perform(connect_now(token), move |result| Message::LinearConnected {
        op,
        result,
    })
}

/// 이슈 목록 조회를 발급한다(연결 성공 직후 + 수동 새로고침).
pub fn list_issues(op: OpId, token: Secret) -> Task<Message> {
    Task::perform(list_issues_now(token), move |result| {
        Message::LinearIssuesFetched { op, result }
    })
}

// ============================ N2: Jira ============================
//
// Linear와 **같은 패턴**(async fn → `Task::perform` → `Message`), 같은 토큰 규율(`Secret`만,
// 키체인 저장, `expose()`는 tracker의 `JiraClient::authorization`에서만). 차이는 클라이언트가
// non-secret 연결 설정([`JiraConnection`]: site/email/auth_type)을 함께 받고, 키체인 account가
// 고정값이 아니라 **사이트 URL**이라는 점(사용자가 여러 사이트를 가질 수 있다).

/// Jira 토큰 키체인 miss/부재 시 fallback env 변수(헤드리스/CI). Linear의 `LINEAR_API_KEY` 관례 미러.
pub const JIRA_ENV_VAR: &str = "JIRA_API_TOKEN";

/// 저장된(또는 env fallback) Jira 토큰 요청. account는 **사이트 URL**이다 — 부팅 시 재연결에 쓴다.
pub fn jira_secret_request(site_url: &str) -> SecretRequest {
    SecretRequest::new(JIRA_KEYCHAIN_SERVICE, site_url).with_env_vars(&[JIRA_ENV_VAR])
}

/// 실 전송으로 인증된 Jira 클라이언트를 만든다. 전송은 stateless라 매 호출 새로 만든다(Linear 미러).
fn jira_client(connection: JiraConnection, token: Secret) -> JiraClient {
    JiraClient::with_transport(Arc::new(ReqwestTransport::new()), connection, Some(token))
}

/// 연결 확인 + (성공 시) 토큰 저장. **저장은 성공했을 때만** — 무효 토큰을 키체인에 남기지 않는다
/// (Linear `connect_now`와 같은 "store → test" 강화). account는 사이트 URL. best-effort 저장이라
/// 키체인이 없어도(헤드리스) 이번 세션은 메모리 토큰으로 동작한다.
pub async fn jira_connect_now(connection: JiraConnection, token: Secret) -> Lookup<JiraViewer> {
    let site = connection.site_url.clone();
    let result = jira_client(connection, token.clone())
        .test_connection()
        .await;
    if matches!(result, Lookup::Found(_)) {
        // 저장 실패는 삼킨다: 표면화할 raw 에러가 토큰을 흘릴 수 있고, 이번 세션은 메모리 토큰으로
        // 이미 동작한다. 저장은 다음 실행의 재연결 편의일 뿐이다.
        let _ = suaegi_secrets::store(JIRA_KEYCHAIN_SERVICE, &site, &token);
    }
    result
}

/// 이슈 목록. v1은 **나에게 배정된 미해결**(`Assigned`) 프리셋을 쓴다 — 워크트리에 링크할 "내 일"이
/// 가장 흔한 뷰다(다른 프리셋은 후속). `has_more`는 tracker 클라이언트가 표면화한다.
pub async fn jira_list_issues_now(
    connection: JiraConnection,
    token: Secret,
) -> Lookup<JiraPage<JiraIssue>> {
    jira_client(connection, token)
        .list_issues(JiraIssueFilter::Assigned)
        .await
}

/// 연결(또는 부팅 시 재연결)을 발급한다.
pub fn jira_connect(op: OpId, connection: JiraConnection, token: Secret) -> Task<Message> {
    Task::perform(jira_connect_now(connection, token), move |result| {
        Message::JiraConnected { op, result }
    })
}

/// 이슈 목록 조회를 발급한다(연결 성공 직후 + 수동 새로고침).
pub fn jira_list_issues(op: OpId, connection: JiraConnection, token: Secret) -> Task<Message> {
    Task::perform(jira_list_issues_now(connection, token), move |result| {
        Message::JiraIssuesFetched { op, result }
    })
}
