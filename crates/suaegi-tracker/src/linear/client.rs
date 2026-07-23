//! Linear GraphQL-over-HTTP 클라이언트(§1.1/§1.2). `@linear/sdk`가 Rust에 없어 hand-rolled:
//! 단일 엔드포인트 POST, 본문 `{query, variables}`, **`Authorization: <raw API key>`(Bearer 아님)**.
//! 전송은 주입 가능([`HttpTransport`]) — 테스트는 fake로 real api.linear.app를 안 친다.
//!
//! 분류는 [`super::classify::classify_graphql`]에 위임한다 — 이 파일은 "성공이면 data에서 필드
//! 뽑기, 실패면 Unavailable로 접기"만 한다. 규율: **일시 실패는 결코 None/empty가 아니다**.

use super::classify::{classify_graphql, GraphqlOutcome};
use super::model::{
    Classified, Comment, Issue, IssuePage, LinearWorkspace, Lookup, TrackerUnavailable,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use suaegi_http::{HttpMethod, HttpRequest, HttpTransport, TransportError};
use suaegi_secrets::Secret;

/// Linear GraphQL 단일 엔드포인트.
pub const LINEAR_ENDPOINT: &str = "https://api.linear.app/graphql";
/// 키체인 service. account는 workspace(멀티-워크스페이스 구분).
pub const KEYCHAIN_SERVICE: &str = "suaegi-linear";

/// 읽기 조회 타임아웃.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 페이지 크기와 전체 순회 상한(bounded full traversal, §1.2).
const PAGE_SIZE: i64 = 50;
const MAX_ISSUES: usize = 250;

const VIEWER_QUERY: &str =
    "query { viewer { id displayName email organization { id name urlKey } } }";

/// 이슈 노드 필드 집합. 읽기와 write readback([`super::write`])이 같은 모양을 뽑는다.
pub(super) const ISSUE_FIELDS: &str =
    "id identifier title description url state { name } assignee { displayName }";

/// GraphQL 클라이언트. 토큰이 `None`이면 모든 op이 `Unavailable(NotAuthenticated)`.
#[derive(Clone)]
pub struct LinearClient {
    transport: Arc<dyn HttpTransport>,
    token: Option<Secret>,
}

/// Debug는 토큰을 **절대** 찍지 않는다(Secret가 이미 리댁션하지만 표면 전체를 고정 라벨로).
impl std::fmt::Debug for LinearClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearClient")
            .field("authenticated", &self.token.is_some())
            .finish()
    }
}

impl LinearClient {
    /// 전송 주입 생성자(테스트/내부). 프로덕션은 `ReqwestTransport`를 넘긴다.
    pub fn with_transport(transport: Arc<dyn HttpTransport>, token: Option<Secret>) -> Self {
        Self { transport, token }
    }

    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    /// 요청 헤더. **여기가 토큰이 노출되는 유일한 지점** — `Authorization`으로 **raw**(Bearer 아님).
    /// 토큰 없으면 None(→ 호출부가 NotAuthenticated로 접는다).
    fn auth_headers(&self) -> Option<Vec<(String, String)>> {
        let token = self.token.as_ref()?;
        Some(vec![
            // expose()는 오직 여기서만. grep 감사점. Linear는 raw 키(Bearer 접두 없음)를 요구.
            ("Authorization".to_string(), token.expose().to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            ("User-Agent".to_string(), "suaegi".to_string()),
        ])
    }

    /// GraphQL POST 한 번. 성공이면 `data` 값을, 실패면 분류된 [`Classified`]를 준다.
    /// **일시 전송 실패(타임아웃/연결)는 `Network`** — 토큰/URL을 담지 않는다.
    async fn request(&self, query: &str, variables: Value) -> Result<Value, Classified> {
        let Some(headers) = self.auth_headers() else {
            return Err(Classified::new(TrackerUnavailable::NotAuthenticated));
        };
        let body = json!({ "query": query, "variables": variables }).to_string();
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: LINEAR_ENDPOINT.to_string(),
            headers,
            body: Some(body),
            timeout: READ_TIMEOUT,
        };
        match self.transport.execute(req).await {
            Ok(resp) => match classify_graphql(resp.status, &resp.body) {
                GraphqlOutcome::Success(data) => Ok(data),
                GraphqlOutcome::Failure(c) => Err(c),
            },
            // 전송 실패는 재시도 가능한 Network. **절대 None/empty 아님.**
            Err(TransportError::Timeout) | Err(TransportError::Connect(_)) => {
                Err(Classified::new(TrackerUnavailable::Network))
            }
        }
    }

    /// **write 경로 전용 저수준 POST**([`super::write`]). auth 헤더 조립 + 단일 실행만 하고,
    /// 분류는 하지 않는다 — write는 읽기와 분류가 다르기 때문(전송 타임아웃 → `unconfirmed`,
    /// 확정 거부만 `rejected`; 읽기처럼 전송실패를 뭉뚱그려 Network로 접으면 "실패인지 성공인지
    /// 모름"을 잃는다). 미인증이면 `None`(호출부가 NotAuthenticated로 접는다).
    pub(super) async fn post_graphql(
        &self,
        query: &str,
        variables: Value,
        timeout: Duration,
    ) -> Option<Result<suaegi_http::HttpResponse, TransportError>> {
        let headers = self.auth_headers()?;
        let body = json!({ "query": query, "variables": variables }).to_string();
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: LINEAR_ENDPOINT.to_string(),
            headers,
            body: Some(body),
            timeout,
        };
        Some(self.transport.execute(req).await)
    }

    /// 연결 확인 + 워크스페이스 레코드(§1.2). auth 실패 → `Unavailable(NotAuthenticated)`.
    pub async fn test_connection(&self) -> Lookup<LinearWorkspace> {
        let data = match self.request(VIEWER_QUERY, json!({})).await {
            Ok(d) => d,
            Err(c) => return Lookup::Unavailable(c),
        };
        let viewer = &data["viewer"];
        let org = &viewer["organization"];
        // 성공 응답인데 기대 필드가 없다 → 예상 밖 모양. **None이 아니라** Unknown.
        let (Some(id), Some(name), Some(url_key), Some(email)) = (
            org["id"].as_str(),
            org["name"].as_str(),
            org["urlKey"].as_str(),
            viewer["email"].as_str(),
        ) else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(LinearWorkspace {
            id: id.to_string(),
            name: name.to_string(),
            url_key: url_key.to_string(),
            viewer_email: email.to_string(),
        })
    }

    /// 이슈 목록 — **bounded full traversal + stuck-cursor 가드**(§1.2). limit(`MAX_ISSUES`)나
    /// `!hasNextPage`까지 페이지를 돈다. `has_more`로 truncation을 표면화(무성 절단 금지).
    ///
    /// `filter`는 Linear `IssueFilter`(예: `{"state":{"type":{"eq":"started"}}}`). `None`이면 전체.
    pub async fn list_issues(&self, filter: Option<Value>) -> Lookup<IssuePage> {
        const LIST_QUERY: &str = "query ListIssues($first: Int!, $after: String, $filter: IssueFilter) { \
             issues(first: $first, after: $after, filter: $filter) { \
                 nodes { id identifier title description url state { name } assignee { displayName } } \
                 pageInfo { hasNextPage endCursor } } }";
        let filter = filter.unwrap_or(Value::Null);
        let mut issues: Vec<Issue> = Vec::new();
        let mut after: Option<String> = None;
        let mut has_more = false;

        loop {
            let vars = json!({ "first": PAGE_SIZE, "after": after, "filter": filter });
            let data = match self.request(LIST_QUERY, vars).await {
                Ok(d) => d,
                Err(c) => return Lookup::Unavailable(c),
            };
            let conn = &data["issues"];
            // 성공인데 issues 연결이 없다 → 예상 밖 모양. **None/빈 목록 아님** → Unavailable.
            let Some(nodes) = conn["nodes"].as_array() else {
                return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
            };
            for node in nodes {
                issues.push(parse_issue(node));
            }
            let page_has_next = conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false);
            let end_cursor = conn["pageInfo"]["endCursor"].as_str().map(str::to_string);

            // limit 도달 → 서버에 더 있으면 has_more로 표면화하고 중단.
            if issues.len() >= MAX_ISSUES {
                has_more = page_has_next;
                break;
            }
            if !page_has_next {
                break;
            }
            // **stuck-cursor 가드**(무한루프 방지): 커서가 없거나 직전과 같으면 진전이 없다 →
            // 더 있을 수 있음을 표면화하고 중단. Orca readIssueConnectionPages 규율 미러.
            match end_cursor {
                Some(ref c) if !c.is_empty() && after.as_deref() != Some(c.as_str()) => {
                    after = Some(c.clone());
                }
                _ => {
                    has_more = true;
                    break;
                }
            }
        }
        Lookup::Found(IssuePage { issues, has_more })
    }

    /// 이슈 검색 — **단일 호출**(§1.2, Orca `searchIssues`는 커서 페이지를 안 한다). 빈 nodes는
    /// 진짜 "결과 없음"(Found(empty)), 일시 실패는 Unavailable.
    pub async fn search_issues(&self, term: &str) -> Lookup<Vec<Issue>> {
        const SEARCH_QUERY: &str = "query SearchIssues($term: String!) { \
             searchIssues(term: $term) { \
                 nodes { id identifier title description url state { name } assignee { displayName } } } }";
        let data = match self.request(SEARCH_QUERY, json!({ "term": term })).await {
            Ok(d) => d,
            Err(c) => return Lookup::Unavailable(c),
        };
        let Some(nodes) = data["searchIssues"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(nodes.iter().map(parse_issue).collect())
    }

    /// 단건 이슈. **Linear의 "not found"는 GraphQL 에러**라 실제로는 `Unavailable`로 온다(§1.2).
    /// 성공인데 `issue`가 null인 경우는 예상 밖 → `Unavailable(Unknown)`(human-eyes로 실측 후
    /// None 매핑 여부 결정, §1.2 TODO). **절대 조용한 None으로 접지 않는다.**
    pub async fn get_issue(&self, id: &str) -> Lookup<Issue> {
        let query = format!("query GetIssue($id: String!) {{ issue(id: $id) {{ {ISSUE_FIELDS} }} }}");
        let data = match self.request(&query, json!({ "id": id })).await {
            Ok(d) => d,
            Err(c) => return Lookup::Unavailable(c),
        };
        let node = &data["issue"];
        if node.is_null() {
            // TODO(human-eyes): 실제 not-found가 여기(성공+null)로 오는지, 아니면 GraphQL 에러로
            // 오는지 실측. 그 전까진 안전하게 Unavailable(절대 None/empty 아님).
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        }
        Lookup::Found(parse_issue(node))
    }

    /// 이슈 코멘트. transient=Unavailable≠빈 목록. 유효 이슈에 코멘트가 없으면 Found(empty).
    pub async fn get_issue_comments(&self, id: &str) -> Lookup<Vec<Comment>> {
        const COMMENTS_QUERY: &str = "query IssueComments($id: String!) { \
             issue(id: $id) { comments { nodes { id body createdAt user { displayName } } } } }";
        let data = match self.request(COMMENTS_QUERY, json!({ "id": id })).await {
            Ok(d) => d,
            Err(c) => return Lookup::Unavailable(c),
        };
        let issue = &data["issue"];
        if issue.is_null() {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        }
        let Some(nodes) = issue["comments"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(nodes.iter().map(parse_comment).collect())
    }
}

/// `data` 노드 → [`Issue`]. 없는 필드는 빈 문자열/None(파싱 실패로 전체를 떨구지 않는다).
/// write readback([`super::write`])도 같은 파서로 확인된 이슈를 만든다.
pub(super) fn parse_issue(v: &Value) -> Issue {
    Issue {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        identifier: v["identifier"].as_str().unwrap_or_default().to_string(),
        title: v["title"].as_str().unwrap_or_default().to_string(),
        description: v["description"].as_str().map(str::to_string),
        url: v["url"].as_str().map(str::to_string),
        state: v["state"]["name"].as_str().map(str::to_string),
        assignee: v["assignee"]["displayName"].as_str().map(str::to_string),
    }
}

/// `data` 노드 → [`Comment`].
fn parse_comment(v: &Value) -> Comment {
    Comment {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        body: v["body"].as_str().unwrap_or_default().to_string(),
        author: v["user"]["displayName"].as_str().map(str::to_string),
        created_at: v["createdAt"].as_str().map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_http::{FakeTransport, HttpResponse};

    // ---- 실제 Linear GraphQL JSON 모양 픽스처 ----

    const VIEWER_OK: &str = r#"{"data":{"viewer":{"id":"user_123","displayName":"Ada",
        "email":"ada@acme.com","organization":{"id":"org_1","name":"Acme","urlKey":"acme"}}}}"#;

    /// **200-with-errors** — Linear가 HTTP 200에 errors를 실어 보내는 실제 모양. raw `message`는
    /// 쿼리 내부, `userPresentableMessage`가 사용자용.
    const ERRORS_200_RATELIMIT: &str = r#"{"errors":[{"message":"complexity limit for query XYZ",
        "extensions":{"type":"ratelimited","userPresentableMessage":"You are being rate limited."}}]}"#;

    /// 401 + authentication error(errors 동봉).
    const ERRORS_401_AUTH: &str = r#"{"errors":[{"message":"no auth header",
        "extensions":{"type":"authentication error","userPresentableMessage":"Not authenticated."}}]}"#;

    const DATA_NULL: &str = r#"{"data":null}"#;

    fn client(t: Arc<FakeTransport>) -> LinearClient {
        LinearClient::with_transport(t, Some(Secret::new("lin_api_rawkey_ABC")))
    }

    fn ok(status: u16, body: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status,
            headers: Vec::new(),
            body: body.to_string(),
        })
    }

    fn issues_page(nodes_json: &str, has_next: bool, end_cursor: &str) -> String {
        format!(
            r#"{{"data":{{"issues":{{"nodes":{nodes_json},
                "pageInfo":{{"hasNextPage":{has_next},"endCursor":"{end_cursor}"}}}}}}}}"#
        )
    }

    const ONE_NODE: &str = r#"[{"id":"iss_1","identifier":"ENG-1","title":"Fix the bug",
        "description":"d","url":"https://linear.app/acme/issue/ENG-1",
        "state":{"name":"In Progress"},"assignee":{"displayName":"Ada"}}]"#;

    // ---- auth / transport ----

    /// **§Q2 회귀**: Authorization은 **raw 키**(Bearer 접두 없음)이고, 엔드포인트/메서드가 맞다.
    #[tokio::test]
    async fn auth_header_is_raw_key_and_endpoint_is_post() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, VIEWER_OK));
        let c = client(t.clone());
        let _ = c.test_connection().await;
        assert_eq!(t.last_header("Authorization").as_deref(), Some("lin_api_rawkey_ABC"));
        let reqs = t.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].url, LINEAR_ENDPOINT);
        assert_eq!(reqs[0].method, HttpMethod::Post);
        // 바디는 {query, variables} 모양.
        let body: Value = serde_json::from_str(reqs[0].body.as_deref().unwrap()).unwrap();
        assert!(body["query"].as_str().unwrap().contains("viewer"));
    }

    /// 토큰 없음 → 전송조차 안 하고 NotAuthenticated. raw 키가 Debug에도 안 샌다.
    #[tokio::test]
    async fn no_token_is_not_authenticated_and_debug_is_redacted() {
        let t = Arc::new(FakeTransport::default());
        let c = LinearClient::with_transport(t.clone(), None);
        assert!(!c.is_authenticated());
        match c.test_connection().await {
            Lookup::Unavailable(cl) => assert_eq!(cl.kind, TrackerUnavailable::NotAuthenticated),
            other => panic!("expected Unavailable(NotAuthenticated), got {other:?}"),
        }
        assert_eq!(t.requests().len(), 0, "no token → no request sent");
        // 토큰이 있어도 Debug에 안 샌다.
        let dbg = format!("{:?}", client(Arc::new(FakeTransport::default())));
        assert!(!dbg.contains("rawkey"), "raw key leaked into Debug: {dbg}");
    }

    /// 전송 실패(타임아웃/연결) → Network. **절대 None/empty 아님.**
    #[tokio::test]
    async fn transport_error_is_network() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(Err(TransportError::Timeout));
        match client(t).list_issues(None).await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Network),
            other => panic!("expected Unavailable(Network), got {other:?}"),
        }
    }

    // ---- test_connection ----

    #[tokio::test]
    async fn test_connection_populates_workspace() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, VIEWER_OK));
        match client(t).test_connection().await {
            Lookup::Found(ws) => {
                assert_eq!(ws.id, "org_1");
                assert_eq!(ws.name, "Acme");
                assert_eq!(ws.url_key, "acme");
                assert_eq!(ws.viewer_email, "ada@acme.com");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_connection_401_is_not_authenticated() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(401, ERRORS_401_AUTH));
        match client(t).test_connection().await {
            Lookup::Unavailable(c) => {
                assert_eq!(c.kind, TrackerUnavailable::NotAuthenticated);
                assert_eq!(c.user_message.as_deref(), Some("Not authenticated."));
            }
            other => panic!("expected Unavailable(NotAuthenticated), got {other:?}"),
        }
    }

    // ---- list_issues: bounded traversal + stuck-cursor + crux ----

    #[tokio::test]
    async fn list_issues_single_page_found() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, &issues_page(ONE_NODE, false, "c1")));
        match client(t).list_issues(None).await {
            Lookup::Found(page) => {
                assert_eq!(page.issues.len(), 1);
                assert_eq!(page.issues[0].identifier, "ENG-1");
                assert_eq!(page.issues[0].state.as_deref(), Some("In Progress"));
                assert!(!page.has_more, "single page, no more");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// bounded full traversal: hasNextPage=true면 다음 페이지를 돈다(첫 페이지-only 아님).
    #[tokio::test]
    async fn list_issues_traverses_pages_until_no_next() {
        let t = Arc::new(FakeTransport::default());
        // page1 → hasNext true, 새 커서. page2 → hasNext false.
        t.push_response(ok(200, &issues_page(ONE_NODE, true, "cursor-1")));
        let node2 = ONE_NODE.replace("iss_1", "iss_2").replace("ENG-1", "ENG-2");
        t.push_response(ok(200, &issues_page(&node2, false, "cursor-2")));
        match client(t.clone()).list_issues(None).await {
            Lookup::Found(page) => {
                assert_eq!(page.issues.len(), 2, "both pages read");
                assert_eq!(page.issues[1].identifier, "ENG-2");
                assert!(!page.has_more);
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert_eq!(t.requests().len(), 2, "traversed exactly two pages");
        // 두 번째 요청은 첫 페이지의 endCursor를 after로 실어야 한다.
        let vars: Value =
            serde_json::from_str(t.requests()[1].body.as_deref().unwrap()).unwrap();
        assert_eq!(vars["variables"]["after"], "cursor-1");
    }

    /// **mutation (c): stuck-cursor 가드.** 커서가 진전 없이 반복되면 무한루프/과다읽기를 막고
    /// 정확히 2요청에서 멈추며 truncation을 has_more로 표면화한다. 가드를 끄면(항상 전진) 이
    /// 픽스처가 더 많은 요청을 소비해 이 assert가 깨진다.
    #[tokio::test]
    async fn list_issues_stuck_cursor_guard_stops_and_flags_more() {
        let t = Arc::new(FakeTransport::default());
        // 같은 커서를 계속 돌려주는 페이지 3장(가드 없으면 다 소비).
        for _ in 0..3 {
            t.push_response(ok(200, &issues_page(ONE_NODE, true, "same-cursor")));
        }
        match client(t.clone()).list_issues(None).await {
            Lookup::Found(page) => {
                assert!(page.has_more, "stuck cursor must surface truncation");
                assert_eq!(page.issues.len(), 2, "read two pages then stopped");
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert_eq!(
            t.requests().len(),
            2,
            "stuck-cursor guard must stop after the cursor fails to advance"
        );
    }

    /// **crux (a): 200-with-errors는 절대 None/empty(=Found(빈 목록))로 안 읽힌다.** 이게 뭉개지면
    /// 레이트리밋이 "이슈 없음"으로 캐시되는 회귀. Unavailable(RateLimited)여야 한다.
    #[tokio::test]
    async fn list_issues_200_with_errors_is_unavailable_not_empty() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, ERRORS_200_RATELIMIT));
        match client(t).list_issues(None).await {
            Lookup::Found(page) => panic!(
                "a GraphQL error must not read as 'no issues'; got Found({} issues)",
                page.issues.len()
            ),
            Lookup::NotFound => panic!("a GraphQL error must not read as NotFound"),
            Lookup::Unavailable(c) => {
                assert_eq!(c.kind, TrackerUnavailable::RateLimited);
                assert_eq!(c.user_message.as_deref(), Some("You are being rate limited."));
            }
        }
    }

    /// **crux (b): data:null → Unavailable(Unknown), 절대 None/empty 아님.**
    #[tokio::test]
    async fn list_issues_data_null_is_unavailable_unknown() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, DATA_NULL));
        match client(t).list_issues(None).await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Unknown),
            other => panic!("expected Unavailable(Unknown), got {other:?}"),
        }
    }

    // ---- search / get_issue / comments ----

    /// 검색은 단일 호출. 빈 결과는 진짜 Found(empty)이지 Unavailable이 아니다.
    #[tokio::test]
    async fn search_issues_single_call_empty_is_found_empty() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"data":{"searchIssues":{"nodes":[]}}}"#));
        match client(t.clone()).search_issues("bug").await {
            Lookup::Found(v) => assert!(v.is_empty(), "empty search is Found(empty)"),
            other => panic!("expected Found(empty), got {other:?}"),
        }
        assert_eq!(t.requests().len(), 1, "search does not cursor-page");
    }

    /// 검색 200-with-errors도 빈 결과로 안 뭉갠다.
    #[tokio::test]
    async fn search_issues_200_with_errors_is_unavailable() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, ERRORS_200_RATELIMIT));
        match client(t).search_issues("bug").await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::RateLimited),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_issue_found_and_null_is_unavailable() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            &format!(r#"{{"data":{{"issue":{}}}}}"#, &ONE_NODE[1..ONE_NODE.len() - 1]),
        ));
        match client(t).get_issue("ENG-1").await {
            Lookup::Found(iss) => assert_eq!(iss.identifier, "ENG-1"),
            other => panic!("expected Found, got {other:?}"),
        }
        // issue:null(성공+null) → Unavailable(Unknown), 절대 조용한 None 아님.
        let t2 = Arc::new(FakeTransport::default());
        t2.push_response(ok(200, r#"{"data":{"issue":null}}"#));
        match client(t2).get_issue("ENG-404").await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Unknown),
            other => panic!("expected Unavailable(Unknown), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_issue_comments_empty_is_found_empty() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"data":{"issue":{"comments":{"nodes":[]}}}}"#));
        match client(t).get_issue_comments("ENG-1").await {
            Lookup::Found(v) => assert!(v.is_empty(), "no comments is Found(empty)"),
            other => panic!("expected Found(empty), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_issue_comments_parses_author_and_body() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"issue":{"comments":{"nodes":[
                {"id":"c1","body":"looks good","createdAt":"2026-07-23T00:00:00Z",
                 "user":{"displayName":"Ada"}}]}}}}"#,
        ));
        match client(t).get_issue_comments("ENG-1").await {
            Lookup::Found(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].body, "looks good");
                assert_eq!(v[0].author.as_deref(), Some("Ada"));
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }
}
