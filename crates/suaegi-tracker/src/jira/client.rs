//! Jira REST 읽기 클라이언트(§2). `github_http` 패턴을 미러한 주입식 전송([`HttpTransport`]) —
//! 테스트는 fake로 real Jira를 안 친다. GraphQL인 Linear와 달리 REST라 상태코드로 분류한다
//! ([`super::classify`]). 규율: **일시 실패는 결코 None/empty 아님**.
//!
//! # Cloud vs Server/DC (§2, Orca `client.ts:72,335`)
//! cloud-ness는 [`JiraConnection::auth_type`]에서 온다: Cloud=`/rest/api/3`(ADF 바디),
//! Server/DC=`/rest/api/2`(plain 바디). description/comment는 provider 무관하게
//! [`super::adf::adf_to_markdown`]를 통과한다 — ADF 객체는 변환, plain 문자열은 (사실상)
//! 그대로(공백 정규화만).
//!
//! # 인증(§2, Orca `client.ts:330-339`)
//! `Server && email 빈 문자열` → `Bearer <PAT>`; 그 밖(Cloud, 또는 Server+username) →
//! `Basic base64(email:token)`. 토큰은 `suaegi-secrets`(service `suaegi-jira`, account=site).
//! **`.expose()`는 오직 [`JiraClient::authorization`] 한 곳** — grep 감사점.
//!
//! # User-Agent 함정(Orca `client.ts:25-30`)
//! Atlassian XSRF 필터가 **브라우저 UA의 POST/PUT을 403**한다. 그래서 non-browser
//! `User-Agent: suaegi`를 모든 요청에 강제한다.

use super::adf::adf_to_markdown;
use super::classify::{classify_jira_status, JiraStatus};
use super::model::{
    Classified, JiraAuthType, JiraComment, JiraConnection, JiraIssue, JiraPage, JiraProject,
    JiraViewer, Lookup, TrackerUnavailable,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use suaegi_http::{HttpMethod, HttpRequest, HttpTransport, TransportError};
use suaegi_secrets::Secret;

/// 키체인 service. account는 site(멀티-사이트 구분).
pub const KEYCHAIN_SERVICE: &str = "suaegi-jira";

/// 읽기 조회 타임아웃.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 검색 1회 상한(Orca `clampLimit` 상한 100의 절반 — 리스트 UI 초기 로드).
const SEARCH_MAX: i64 = 50;
/// 페이지네이션(comment/project) 한 페이지 크기.
const PAGE_SIZE: i64 = 100;
/// 페이지네이션 누적 상한(무한/과다 방지). 초과분은 `has_more`로 표면화.
const ITEM_CAP: usize = 500;
/// 페이지 순회 가드(Orca `for guard < 100`).
const MAX_PAGES: usize = 100;

/// getIssue/search가 가져오는 필드. Orca `ISSUE_FIELDS`(`issues.ts:36-48`)의 읽기 서브셋.
const ISSUE_FIELDS: &[&str] = &[
    "summary",
    "description",
    "project",
    "issuetype",
    "status",
    "assignee",
    "labels",
    "created",
    "updated",
];

/// 이슈 목록 필터 프리셋 → JQL(Orca `filterToJql`, `issues.ts:346-357`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraIssueFilter {
    /// 나에게 배정 + 미해결.
    Assigned,
    /// 내가 보고 + 미해결.
    Reported,
    /// 나에게 배정 + 해결됨.
    Done,
    /// 전체 미해결.
    All,
}

impl JiraIssueFilter {
    fn to_jql(self) -> &'static str {
        match self {
            JiraIssueFilter::Assigned => {
                "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
            }
            JiraIssueFilter::Reported => {
                "reporter = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
            }
            JiraIssueFilter::Done => {
                "assignee = currentUser() AND resolution IS NOT EMPTY ORDER BY updated DESC"
            }
            JiraIssueFilter::All => "resolution = Unresolved ORDER BY updated DESC",
        }
    }
}

/// 전송 후 분류된 실패. `NotFound`는 특정 리소스 404(호출부가 `Lookup::NotFound`로 접거나,
/// 컬렉션이면 `Unavailable`로 승격). `Unavailable`은 그 밖 분류된 실패.
enum RequestError {
    NotFound,
    Unavailable(Classified),
}

/// Jira REST 클라이언트. 토큰이 `None`이면 모든 op이 `Unavailable(NotAuthenticated)`.
#[derive(Clone)]
pub struct JiraClient {
    transport: Arc<dyn HttpTransport>,
    connection: JiraConnection,
    token: Option<Secret>,
}

/// Debug는 토큰·사이트 크리덴셜을 **절대** 찍지 않는다(고정 라벨 + 사이트 URL만).
impl std::fmt::Debug for JiraClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraClient")
            .field("site_url", &self.connection.site_url)
            .field("auth_type", &self.connection.auth_type)
            .field("authenticated", &self.token.is_some())
            .finish()
    }
}

/// **인증 헤더 조립(순수, 직접 테스트 대상).** `.expose()`와 분리해 브랜치만 검증할 수 있다.
/// `Server && email 빈 문자열` → Bearer PAT; 그 밖 → Basic base64(email:token). Orca
/// `authHeader`(`client.ts:330-339`) 미러.
pub fn build_auth_header(email: &str, token: &str, auth_type: JiraAuthType) -> String {
    if auth_type == JiraAuthType::Server && email.is_empty() {
        format!("Bearer {token}")
    } else {
        let encoded = STANDARD.encode(format!("{email}:{token}"));
        format!("Basic {encoded}")
    }
}

impl JiraClient {
    /// 전송 주입 생성자. 프로덕션은 `ReqwestTransport`를 넘긴다.
    pub fn with_transport(
        transport: Arc<dyn HttpTransport>,
        connection: JiraConnection,
        token: Option<Secret>,
    ) -> Self {
        Self {
            transport,
            connection,
            token,
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    /// 요청 `Authorization` 값. **여기가 토큰이 노출되는 유일한 지점**(`.expose()`). 토큰 없으면 None.
    fn authorization(&self) -> Option<String> {
        let token = self.token.as_ref()?;
        Some(build_auth_header(
            &self.connection.email,
            token.expose(),
            self.connection.auth_type,
        ))
    }

    fn base(&self) -> &'static str {
        self.connection.auth_type.api_base_path()
    }

    /// REST 요청 1회. 성공이면 파싱된 `data`(204/빈 바디는 `Null`), 실패면 분류된 [`RequestError`].
    /// **일시 전송 실패(타임아웃/연결)는 `Network`** — 토큰/URL을 담지 않는다.
    async fn request(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, RequestError> {
        let Some(auth) = self.authorization() else {
            return Err(RequestError::Unavailable(Classified::new(
                TrackerUnavailable::NotAuthenticated,
            )));
        };
        let headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            // UA 함정: 브라우저 UA면 Atlassian XSRF 필터가 POST/PUT을 403.
            ("User-Agent".to_string(), "suaegi".to_string()),
            // 토큰 노출 유일 경로.
            ("Authorization".to_string(), auth),
        ];
        let req = HttpRequest {
            method,
            url: self.connection.url(path),
            headers,
            body: body.map(|b| b.to_string()),
            timeout: READ_TIMEOUT,
        };
        match self.transport.execute(req).await {
            Ok(resp) => {
                let retry_after = resp.header("retry-after");
                match classify_jira_status(resp.status, retry_after) {
                    JiraStatus::Success => {
                        if resp.body.trim().is_empty() {
                            return Ok(Value::Null);
                        }
                        match serde_json::from_str(&resp.body) {
                            Ok(v) => Ok(v),
                            // 2xx인데 파싱 불가 = 예상 밖 출력. **성공/None 아님** → Unknown.
                            Err(_) => Err(RequestError::Unavailable(Classified::new(
                                TrackerUnavailable::Unknown,
                            ))),
                        }
                    }
                    JiraStatus::NotFound => Err(RequestError::NotFound),
                    JiraStatus::Unavailable(kind) => {
                        Err(RequestError::Unavailable(Classified::new(kind)))
                    }
                }
            }
            // 전송 실패는 재시도 가능한 Network. **절대 None/empty 아님.**
            Err(TransportError::Timeout) | Err(TransportError::Connect(_)) => Err(
                RequestError::Unavailable(Classified::new(TrackerUnavailable::Network)),
            ),
        }
    }

    /// 연결 확인 + 계정 레코드(§2). `/myself`. 404는 여기선 **API/사이트 문제**(→ Unavailable),
    /// 특정 리소스 not-found가 아니다.
    pub async fn test_connection(&self) -> Lookup<JiraViewer> {
        let path = format!("{}/myself", self.base());
        match self.request(HttpMethod::Get, &path, None).await {
            Ok(v) if !v.is_null() => Lookup::Found(parse_viewer(&v, &self.connection.email)),
            // 성공인데 null/빈 응답 → 예상 밖. **None 아님** → Unknown.
            Ok(_) => Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown)),
            // /myself 404 = 엔드포인트가 늘 존재하므로 사이트/경로 문제 → Unavailable.
            Err(RequestError::NotFound) => {
                Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown))
            }
            Err(RequestError::Unavailable(c)) => Lookup::Unavailable(c),
        }
    }

    /// JQL 검색(단일 호출, `maxResults` 상한). Cloud=`/rest/api/3/search/jql`, Server=
    /// `/rest/api/2/search`(Orca `searchIssuesForClient`, `issues.ts:359-378`). truncation은
    /// `has_more`로 표면화(무성 절단 금지). 컬렉션 엔드포인트라 404=Unavailable.
    pub async fn search_issues(&self, jql: &str) -> Lookup<JiraPage<JiraIssue>> {
        let path = if self.connection.auth_type.is_cloud() {
            format!("{}/search/jql", self.base())
        } else {
            format!("{}/search", self.base())
        };
        let body = json!({ "jql": jql, "maxResults": SEARCH_MAX, "fields": ISSUE_FIELDS });
        match self.request(HttpMethod::Post, &path, Some(body)).await {
            Ok(v) => {
                // 성공인데 issues 배열이 없다 → 예상 밖 모양. **빈 목록 아님** → Unavailable.
                let Some(arr) = v["issues"].as_array() else {
                    return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
                };
                let items = arr.iter().map(|r| self.map_issue(r)).collect::<Vec<_>>();
                let has_more = search_has_more(&v, items.len());
                Lookup::Found(JiraPage { items, has_more })
            }
            Err(RequestError::NotFound) => {
                Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown))
            }
            Err(RequestError::Unavailable(c)) => Lookup::Unavailable(c),
        }
    }

    /// 필터 프리셋 → JQL → 검색(Orca `listIssues`).
    pub async fn list_issues(&self, filter: JiraIssueFilter) -> Lookup<JiraPage<JiraIssue>> {
        self.search_issues(filter.to_jql()).await
    }

    /// 단건 이슈. **404 = 그 이슈 없음 = `Lookup::NotFound`**(특정 리소스 엔드포인트,
    /// forge `review_by_number` 404→None 미러). 일시 실패는 Unavailable(절대 None 아님).
    pub async fn get_issue(&self, key: &str) -> Lookup<JiraIssue> {
        let path = format!(
            "{}/issue/{}?fields={}",
            self.base(),
            encode_segment(key),
            ISSUE_FIELDS.join(",")
        );
        match self.request(HttpMethod::Get, &path, None).await {
            Ok(v) if v.get("key").is_some() || v.get("id").is_some() => {
                Lookup::Found(self.map_issue(&v))
            }
            // 성공인데 이슈 모양이 아님 → 예상 밖. **None 아님** → Unknown.
            Ok(_) => Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown)),
            // 특정 이슈의 404 = 진짜 없음.
            Err(RequestError::NotFound) => Lookup::NotFound,
            Err(RequestError::Unavailable(c)) => Lookup::Unavailable(c),
        }
    }

    /// 이슈 코멘트(startAt 페이지네이션, Orca `getIssueComments`). 특정 이슈라 404=NotFound.
    /// transient=Unavailable≠빈 목록. 유효 이슈에 코멘트 없으면 Found(empty).
    pub async fn get_issue_comments(&self, key: &str) -> Lookup<JiraPage<JiraComment>> {
        let base = self.base();
        let key_enc = encode_segment(key);
        let path_for = |start_at: i64, max_results: i64| {
            format!(
                "{base}/issue/{key_enc}/comment?maxResults={max_results}&orderBy=created&startAt={start_at}"
            )
        };
        match self.paginate("comments", path_for).await {
            Ok((records, has_more)) => {
                let items = records.iter().map(map_comment).collect();
                Lookup::Found(JiraPage { items, has_more })
            }
            Err(RequestError::NotFound) => Lookup::NotFound,
            Err(RequestError::Unavailable(c)) => Lookup::Unavailable(c),
        }
    }

    /// 프로젝트 목록(피커). Cloud=`/project/search`(startAt 페이지), Server=`/project`(플랫 배열).
    /// 컬렉션 엔드포인트라 404=Unavailable.
    pub async fn list_projects(&self) -> Lookup<JiraPage<JiraProject>> {
        let base = self.base();
        let result = if self.connection.auth_type.is_cloud() {
            let path_for =
                |start_at: i64, max_results: i64| format!("{base}/project/search?maxResults={max_results}&startAt={start_at}");
            self.paginate("values", path_for).await
        } else {
            // Server/DC `/project`는 플랫 배열(페이지네이션 없음).
            match self.request(HttpMethod::Get, &format!("{base}/project"), None).await {
                Ok(v) => match v.as_array() {
                    Some(arr) => Ok((arr.clone(), false)),
                    // 성공인데 배열 아님 → 예상 밖. **빈 목록 아님** → Unknown.
                    None => Err(RequestError::Unavailable(Classified::new(
                        TrackerUnavailable::Unknown,
                    ))),
                },
                Err(e) => Err(e),
            }
        };
        match result {
            Ok((records, has_more)) => {
                let items = records.iter().map(map_project).collect();
                Lookup::Found(JiraPage { items, has_more })
            }
            Err(RequestError::NotFound) => {
                Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown))
            }
            Err(RequestError::Unavailable(c)) => Lookup::Unavailable(c),
        }
    }

    /// **bounded 페이지네이션**(Orca `fetchPagedRecords` + `shouldFetchNextPage` 미러). ITEM_CAP
    /// 초과 시 `has_more=true`로 표면화(무성 절단 금지). 일시 실패는 propagate(빈 목록 금지).
    async fn paginate<F>(&self, key: &str, path_for: F) -> Result<(Vec<Value>, bool), RequestError>
    where
        F: Fn(i64, i64) -> String,
    {
        let mut records: Vec<Value> = Vec::new();
        let mut start_at: i64 = 0;
        for _guard in 0..MAX_PAGES {
            let v = self.request(HttpMethod::Get, &path_for(start_at, PAGE_SIZE), None).await?;
            let items = page_items(&v, key);
            let fetched = items.len() as i64;
            records.extend(items);
            if !should_fetch_next(&v, start_at, fetched, PAGE_SIZE) {
                return Ok((records, false));
            }
            // cap 도달 → 서버에 더 있음을 has_more로 표면화하고 중단.
            if records.len() >= ITEM_CAP {
                return Ok((records, true));
            }
            start_at += v["maxResults"].as_i64().unwrap_or(PAGE_SIZE);
        }
        // 가드 소진 = 아직 끝 신호를 못 봤다 → 더 있을 수 있음.
        Ok((records, true))
    }

    /// raw 이슈 레코드 → [`JiraIssue`]. description은 ADF/plain 무관하게 [`adf_to_markdown`] 통과.
    fn map_issue(&self, raw: &Value) -> JiraIssue {
        let fields = &raw["fields"];
        let key = as_str(&raw["key"]);
        let id = raw["id"].as_str().filter(|s| !s.is_empty()).unwrap_or(key);
        let title = fields["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(if key.is_empty() { "Untitled issue" } else { key });
        JiraIssue {
            id: id.to_string(),
            key: key.to_string(),
            title: title.to_string(),
            description: adf_to_markdown(&fields["description"]),
            url: format!("{}/browse/{}", self.connection.site_url, key),
            project_key: opt_str(&fields["project"]["key"]),
            issue_type: opt_str(&fields["issuetype"]["name"]),
            status: opt_str(&fields["status"]["name"]),
            assignee: opt_str(&fields["assignee"]["displayName"]),
            labels: string_array(&fields["labels"]),
        }
    }
}

/// paged 응답에서 항목 배열 추출(Orca `getPageItems`: keyed || values).
fn page_items(resp: &Value, key: &str) -> Vec<Value> {
    if let Some(arr) = resp[key].as_array() {
        return arr.clone();
    }
    resp["values"].as_array().cloned().unwrap_or_default()
}

/// 다음 페이지를 더 읽을지(Orca `shouldFetchNextPage`, `issues.ts:140-158` 미러).
fn should_fetch_next(resp: &Value, start_at: i64, items_len: i64, requested_max: i64) -> bool {
    if resp["isLast"].as_bool() == Some(true) || items_len == 0 {
        return false;
    }
    let page_size = resp["maxResults"].as_i64();
    if let Some(total) = resp["total"].as_i64() {
        return start_at + items_len < total && page_size.unwrap_or(requested_max) > 0;
    }
    if resp["isLast"].as_bool() == Some(false) {
        return page_size.unwrap_or(requested_max) > 0;
    }
    // total도 isLast도 없으면 꽉 찬 페이지일 때만 더 읽는다.
    page_size.map(|ps| items_len >= ps).unwrap_or(false)
}

/// 검색 응답 truncation 신호. `isLast`/`nextPageToken`(신 Cloud) 또는 `total`/`startAt`로.
fn search_has_more(resp: &Value, count: usize) -> bool {
    if resp["isLast"].as_bool() == Some(true) {
        return false;
    }
    if resp["nextPageToken"].as_str().map(|s| !s.is_empty()).unwrap_or(false) {
        return true;
    }
    if let Some(total) = resp["total"].as_i64() {
        let start = resp["startAt"].as_i64().unwrap_or(0);
        return start + (count as i64) < total;
    }
    resp["isLast"].as_bool() == Some(false)
}

fn parse_viewer(v: &Value, fallback_email: &str) -> JiraViewer {
    // Server/DC /myself는 accountId 없음 → name/key가 안정 식별자(Orca `toViewer`).
    let account_id = v["accountId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| v["name"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| v["key"].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string();
    let display_name = v["displayName"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_email)
        .to_string();
    JiraViewer {
        account_id,
        display_name,
        email: opt_str(&v["emailAddress"]),
    }
}

fn map_comment(raw: &Value) -> JiraComment {
    JiraComment {
        id: as_str(&raw["id"]).to_string(),
        body: adf_to_markdown(&raw["body"]),
        author: opt_str(&raw["author"]["displayName"]),
        created_at: opt_str(&raw["created"]),
    }
}

fn map_project(raw: &Value) -> JiraProject {
    let key = as_str(&raw["key"]).to_string();
    let name = raw["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&key)
        .to_string();
    JiraProject {
        id: as_str(&raw["id"]).to_string(),
        key,
        name,
    }
}

fn as_str(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// 비어있지 않은 문자열이면 `Some`, 아니면 `None`.
fn opt_str(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn string_array(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// URL path segment 최소 퍼센트 인코딩(이슈 key/프로젝트 key용). unreserved(`A-Za-z0-9-._~`)만
/// 통과, 그 밖은 `%XX`. Jira key는 보통 안전하나 방어적으로.
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_http::{FakeTransport, HttpResponse};

    // ---- 실제 Jira REST JSON 픽스처 ----

    /// Cloud `/myself`.
    const MYSELF_CLOUD: &str = r#"{"accountId":"acc-1","displayName":"Ada Lovelace",
        "emailAddress":"ada@acme.com"}"#;

    /// Cloud 검색 결과 — issues 하나, description은 ADF.
    const SEARCH_CLOUD_ONE: &str = r#"{"startAt":0,"maxResults":50,"total":1,"issues":[
        {"id":"10001","key":"ENG-1","fields":{
            "summary":"Fix the bug",
            "description":{"type":"doc","version":1,"content":[
                {"type":"paragraph","content":[{"type":"text","text":"Steps to reproduce"}]}]},
            "project":{"key":"ENG","name":"Engineering"},
            "issuetype":{"name":"Bug"},
            "status":{"name":"In Progress"},
            "assignee":{"displayName":"Ada"},
            "labels":["backend","urgent"]}}]}"#;

    /// Server/DC 검색 결과 — description은 **plain text**(ADF 아님).
    const SEARCH_SERVER_ONE: &str = r#"{"startAt":0,"maxResults":50,"total":1,"issues":[
        {"id":"200","key":"SRV-2","fields":{
            "summary":"Server ticket",
            "description":"Just plain text here.\nSecond line.",
            "project":{"key":"SRV"},
            "issuetype":{"name":"Task"},
            "status":{"name":"Open"},
            "labels":[]}}]}"#;

    /// 401 — Jira 에러 바디(errorMessages). **이 raw 문자열은 출력에 새면 안 된다.**
    const ERR_401: &str = r#"{"errorMessages":["You are not authenticated. SECRET-JQL-INTERNALS"],"errors":{}}"#;

    /// 404 not-found 바디.
    const ERR_404: &str = r#"{"errorMessages":["Issue does not exist or you do not have permission to see it."],"errors":{}}"#;

    fn cloud() -> JiraConnection {
        JiraConnection {
            site_url: "https://acme.atlassian.net".to_string(),
            email: "ada@acme.com".to_string(),
            auth_type: JiraAuthType::Cloud,
        }
    }

    fn server_pat() -> JiraConnection {
        JiraConnection {
            site_url: "https://jira.internal".to_string(),
            email: String::new(), // PAT → Bearer
            auth_type: JiraAuthType::Server,
        }
    }

    fn client(t: Arc<FakeTransport>, conn: JiraConnection) -> JiraClient {
        JiraClient::with_transport(t, conn, Some(Secret::new("jira_token_SECRET123")))
    }

    fn ok(status: u16, body: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status,
            headers: Vec::new(),
            body: body.to_string(),
        })
    }

    // ---- (c) 인증 헤더 브랜치: Basic vs Bearer ----

    #[test]
    fn auth_header_cloud_is_basic_base64() {
        let h = build_auth_header("ada@acme.com", "tok", JiraAuthType::Cloud);
        let expected = format!("Basic {}", STANDARD.encode("ada@acme.com:tok"));
        assert_eq!(h, expected);
    }

    #[test]
    fn auth_header_server_pat_is_bearer() {
        // Server + email 빈 문자열 → Bearer PAT.
        assert_eq!(build_auth_header("", "pat123", JiraAuthType::Server), "Bearer pat123");
    }

    #[test]
    fn auth_header_server_with_username_is_basic() {
        // Server + username 있음 → 클래식 Basic(구형 DC).
        let h = build_auth_header("bob", "pw", JiraAuthType::Server);
        assert_eq!(h, format!("Basic {}", STANDARD.encode("bob:pw")));
    }

    /// 헤더가 실제 요청에 실린다 + UA 함정 회피(non-browser UA).
    #[tokio::test]
    async fn request_sends_auth_and_non_browser_ua() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, MYSELF_CLOUD));
        let _ = client(t.clone(), cloud()).test_connection().await;
        let expected_auth = format!("Basic {}", STANDARD.encode("ada@acme.com:jira_token_SECRET123"));
        assert_eq!(t.last_header("Authorization").as_deref(), Some(expected_auth.as_str()));
        assert_eq!(t.last_header("User-Agent").as_deref(), Some("suaegi"));
    }

    // ---- (b) Cloud/Server apiBasePath 브랜치 ----

    #[tokio::test]
    async fn cloud_uses_api_v3_path() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, MYSELF_CLOUD));
        let _ = client(t.clone(), cloud()).test_connection().await;
        assert_eq!(t.requests()[0].url, "https://acme.atlassian.net/rest/api/3/myself");
    }

    #[tokio::test]
    async fn server_uses_api_v2_path() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"name":"bob","displayName":"Bob"}"#));
        let _ = client(t.clone(), server_pat()).test_connection().await;
        assert_eq!(t.requests()[0].url, "https://jira.internal/rest/api/2/myself");
    }

    /// 검색 경로도 분기한다: Cloud=`/rest/api/3/search/jql`, Server=`/rest/api/2/search`.
    #[tokio::test]
    async fn search_path_branches_on_cloudness() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, SEARCH_CLOUD_ONE));
        let _ = client(t.clone(), cloud()).search_issues("project = ENG").await;
        assert_eq!(t.requests()[0].url, "https://acme.atlassian.net/rest/api/3/search/jql");

        let t2 = Arc::new(FakeTransport::default());
        t2.push_response(ok(200, SEARCH_SERVER_ONE));
        let _ = client(t2.clone(), server_pat()).search_issues("project = SRV").await;
        assert_eq!(t2.requests()[0].url, "https://jira.internal/rest/api/2/search");
    }

    // ---- test_connection ----

    #[tokio::test]
    async fn test_connection_populates_viewer() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, MYSELF_CLOUD));
        match client(t, cloud()).test_connection().await {
            Lookup::Found(v) => {
                assert_eq!(v.account_id, "acc-1");
                assert_eq!(v.display_name, "Ada Lovelace");
                assert_eq!(v.email.as_deref(), Some("ada@acme.com"));
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_connection_server_uses_name_as_account_id() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"name":"bob","key":"bob","displayName":"Bob"}"#));
        match client(t, server_pat()).test_connection().await {
            Lookup::Found(v) => assert_eq!(v.account_id, "bob"),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    // ---- (a) transient must NOT read as empty/none ----

    /// **crux (a): 전송 실패(타임아웃) → Network, 절대 "이슈 없음" 아님.**
    #[tokio::test]
    async fn transport_error_is_network_not_empty() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(Err(TransportError::Timeout));
        match client(t, cloud()).search_issues("x").await {
            Lookup::Found(p) => panic!("timeout must not read as 'no issues'; got {} issues", p.items.len()),
            Lookup::NotFound => panic!("timeout must not read as NotFound"),
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Network),
        }
    }

    /// **crux (a): 429/5xx → 분류된 Unavailable, 절대 빈 목록 아님.**
    #[tokio::test]
    async fn rate_limit_and_5xx_are_unavailable_not_empty() {
        for (status, want) in [
            (429u16, TrackerUnavailable::RateLimited),
            (503, TrackerUnavailable::Network),
            (500, TrackerUnavailable::Internal),
        ] {
            let t = Arc::new(FakeTransport::default());
            t.push_response(ok(status, "{}"));
            match client(t, cloud()).search_issues("x").await {
                Lookup::Found(p) => {
                    panic!("status {status} must not read as empty; got {} issues", p.items.len())
                }
                Lookup::Unavailable(c) => assert_eq!(c.kind, want, "status={status}"),
                other => panic!("status {status}: expected Unavailable, got {other:?}"),
            }
        }
    }

    /// 401 → NotAuthenticated. **raw errorMessages(SECRET-JQL-INTERNALS)는 절대 안 샌다.**
    #[tokio::test]
    async fn unauthorized_is_not_authenticated_and_redacts_body() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(401, ERR_401));
        match client(t, cloud()).search_issues("x").await {
            Lookup::Unavailable(c) => {
                assert_eq!(c.kind, TrackerUnavailable::NotAuthenticated);
                assert_eq!(c.user_message, None, "Jira never surfaces raw error body");
                let rendered = format!("{c:?}");
                assert!(!rendered.contains("SECRET-JQL-INTERNALS"), "raw body leaked: {rendered}");
            }
            other => panic!("expected Unavailable(NotAuthenticated), got {other:?}"),
        }
    }

    /// 403 → Forbidden(권한 갭), NOT NotAuthenticated. 401만 크리덴셜 무효.
    #[tokio::test]
    async fn forbidden_is_not_reauth() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(403, r#"{"errorMessages":["no permission"]}"#));
        match client(t, cloud()).search_issues("x").await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Forbidden),
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    // ---- search parsing + ADF vs plain ----

    #[tokio::test]
    async fn search_cloud_parses_issue_with_adf_description() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, SEARCH_CLOUD_ONE));
        match client(t, cloud()).search_issues("project = ENG").await {
            Lookup::Found(page) => {
                assert_eq!(page.items.len(), 1);
                let i = &page.items[0];
                assert_eq!(i.key, "ENG-1");
                assert_eq!(i.title, "Fix the bug");
                assert_eq!(i.description, "Steps to reproduce", "ADF body converted to Markdown");
                assert_eq!(i.url, "https://acme.atlassian.net/browse/ENG-1");
                assert_eq!(i.project_key.as_deref(), Some("ENG"));
                assert_eq!(i.status.as_deref(), Some("In Progress"));
                assert_eq!(i.assignee.as_deref(), Some("Ada"));
                assert_eq!(i.labels, vec!["backend", "urgent"]);
                assert!(!page.has_more, "total=1, one issue → no more");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// Server/DC 바디는 plain text — ADF 변환 없이 그대로(공백 정규화만).
    #[tokio::test]
    async fn search_server_keeps_plain_text_description() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, SEARCH_SERVER_ONE));
        match client(t, server_pat()).search_issues("project = SRV").await {
            Lookup::Found(page) => {
                assert_eq!(page.items[0].description, "Just plain text here.\nSecond line.");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// 빈 검색 결과는 진짜 Found(empty)이지 Unavailable이 아니다.
    #[tokio::test]
    async fn search_empty_is_found_empty() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"startAt":0,"maxResults":50,"total":0,"issues":[]}"#));
        match client(t, cloud()).search_issues("x").await {
            Lookup::Found(page) => {
                assert!(page.items.is_empty());
                assert!(!page.has_more);
            }
            other => panic!("expected Found(empty), got {other:?}"),
        }
    }

    /// 검색 truncation은 has_more로 표면화(total > 반환 수).
    #[tokio::test]
    async fn search_surfaces_has_more_when_truncated() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"startAt":0,"maxResults":50,"total":120,"issues":[
            {"id":"1","key":"ENG-1","fields":{"summary":"a"}}]}"#));
        match client(t, cloud()).search_issues("x").await {
            Lookup::Found(page) => assert!(page.has_more, "total 120 > 1 returned → has_more"),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    // ---- (e) 404: not-found (None) vs API-unavailable ----

    /// **crux (e): getIssue 404 = 진짜 NotFound(None).**
    #[tokio::test]
    async fn get_issue_404_is_not_found() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(404, ERR_404));
        match client(t, cloud()).get_issue("ENG-404").await {
            Lookup::NotFound => {}
            other => panic!("a specific-issue 404 must be NotFound, got {other:?}"),
        }
    }

    /// **crux (e): 검색(컬렉션)의 404 = API/사이트 문제 = Unavailable, NOT NotFound.**
    #[tokio::test]
    async fn search_404_is_unavailable_not_notfound() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(404, ERR_404));
        match client(t, cloud()).search_issues("x").await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Unknown),
            Lookup::NotFound => panic!("a collection 404 must be Unavailable, not NotFound"),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    /// getIssue 성공.
    #[tokio::test]
    async fn get_issue_found() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"id":"10001","key":"ENG-1","fields":{
            "summary":"Fix","description":"body","status":{"name":"Done"}}}"#));
        match client(t, cloud()).get_issue("ENG-1").await {
            Lookup::Found(i) => {
                assert_eq!(i.key, "ENG-1");
                assert_eq!(i.status.as_deref(), Some("Done"));
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// getIssue transient(503) → Unavailable(Network), 절대 NotFound 아님(거짓 음성 금지).
    #[tokio::test]
    async fn get_issue_transient_is_not_notfound() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(503, ""));
        match client(t, cloud()).get_issue("ENG-1").await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Network),
            Lookup::NotFound => panic!("a 503 must NEVER read as NotFound (false negative)"),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    // ---- comments ----

    #[tokio::test]
    async fn comments_single_page_parsed() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"startAt":0,"maxResults":100,"total":1,"comments":[
            {"id":"c1","body":{"type":"doc","version":1,"content":[
                {"type":"paragraph","content":[{"type":"text","text":"Looks good"}]}]},
             "created":"2026-07-23T00:00:00.000Z",
             "author":{"displayName":"Ada"}}]}"#));
        match client(t.clone(), cloud()).get_issue_comments("ENG-1").await {
            Lookup::Found(page) => {
                assert_eq!(page.items.len(), 1);
                assert_eq!(page.items[0].body, "Looks good");
                assert_eq!(page.items[0].author.as_deref(), Some("Ada"));
                assert!(!page.has_more);
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert_eq!(t.requests().len(), 1, "single page → one request");
    }

    /// 코멘트 페이지네이션: 두 페이지를 startAt으로 순회.
    #[tokio::test]
    async fn comments_paginate_two_pages() {
        let t = Arc::new(FakeTransport::default());
        // page1: total 3, maxResults 2, 2 comments → 더 있음. page2: 1 comment → 끝.
        t.push_response(ok(200, r#"{"startAt":0,"maxResults":2,"total":3,"comments":[
            {"id":"c1","body":"a"},{"id":"c2","body":"b"}]}"#));
        t.push_response(ok(200, r#"{"startAt":2,"maxResults":2,"total":3,"comments":[
            {"id":"c3","body":"c"}]}"#));
        match client(t.clone(), cloud()).get_issue_comments("ENG-1").await {
            Lookup::Found(page) => {
                assert_eq!(page.items.len(), 3, "both pages accumulated");
                assert_eq!(page.items[2].id, "c3");
                assert!(!page.has_more);
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert_eq!(t.requests().len(), 2);
        assert!(t.requests()[1].url.contains("startAt=2"), "second page uses startAt=2");
    }

    /// 코멘트 transient(1페이지 실패) → Unavailable, 절대 부분/빈 목록 아님.
    #[tokio::test]
    async fn comments_transient_is_unavailable_not_empty() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(Err(TransportError::Timeout));
        match client(t, cloud()).get_issue_comments("ENG-1").await {
            Lookup::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Network),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    // ---- projects ----

    #[tokio::test]
    async fn list_projects_cloud_paged() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"startAt":0,"maxResults":100,"total":2,"isLast":true,"values":[
            {"id":"1","key":"ENG","name":"Engineering"},
            {"id":"2","key":"OPS","name":"Operations"}]}"#));
        match client(t.clone(), cloud()).list_projects().await {
            Lookup::Found(page) => {
                assert_eq!(page.items.len(), 2);
                assert_eq!(page.items[0].key, "ENG");
                assert!(!page.has_more);
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert!(t.requests()[0].url.contains("/rest/api/3/project/search"));
    }

    #[tokio::test]
    async fn list_projects_server_flat_array() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"[{"id":"1","key":"SRV","name":"Server Proj"}]"#));
        match client(t.clone(), server_pat()).list_projects().await {
            Lookup::Found(page) => {
                assert_eq!(page.items.len(), 1);
                assert_eq!(page.items[0].key, "SRV");
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert_eq!(t.requests()[0].url, "https://jira.internal/rest/api/2/project");
    }

    // ---- token redaction ----

    /// 토큰이 없으면 전송조차 안 하고 NotAuthenticated. 토큰이 Debug에 안 샌다.
    #[tokio::test]
    async fn no_token_is_not_authenticated_and_debug_redacted() {
        let t = Arc::new(FakeTransport::default());
        let c = JiraClient::with_transport(t.clone(), cloud(), None);
        assert!(!c.is_authenticated());
        match c.test_connection().await {
            Lookup::Unavailable(cl) => assert_eq!(cl.kind, TrackerUnavailable::NotAuthenticated),
            other => panic!("expected Unavailable(NotAuthenticated), got {other:?}"),
        }
        assert_eq!(t.requests().len(), 0, "no token → no request");
        let dbg = format!("{:?}", client(Arc::new(FakeTransport::default()), cloud()));
        assert!(!dbg.contains("SECRET"), "token leaked into Debug: {dbg}");
    }

    #[test]
    fn encode_segment_escapes_unsafe() {
        assert_eq!(encode_segment("ENG-1"), "ENG-1");
        assert_eq!(encode_segment("A B/C"), "A%20B%2FC");
    }
}
