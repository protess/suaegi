//! Linear GraphQL-over-HTTP 클라이언트(§1.1/§1.2). `@linear/sdk`가 Rust에 없어 hand-rolled:
//! 단일 엔드포인트 POST, 본문 `{query, variables}`, **`Authorization: <raw API key>`(Bearer 아님)**.
//! 전송은 주입 가능([`HttpTransport`]) — 테스트는 fake로 real api.linear.app를 안 친다.
//!
//! 분류는 [`super::classify::classify_graphql`]에 위임한다 — 이 파일은 "성공이면 data에서 필드
//! 뽑기, 실패면 Unavailable로 접기"만 한다. 규율: **일시 실패는 결코 None/empty가 아니다**.

use super::classify::{classify_graphql, GraphqlOutcome};
use super::model::{
    Classified, Comment, Issue, IssuePage, LinearIssueContextOptions, LinearLabel, LinearMember,
    LinearProject, LinearRelation, LinearRelationPage, LinearState, LinearTeam, LinearWorkspace,
    Lookup, TrackerUnavailable,
};
use regex::Regex;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
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
type ChildrenReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(Vec<Value>, bool), Classified>> + Send + 'a>>;

const VIEWER_QUERY: &str =
    "query { viewer { id displayName email organization { id name urlKey } } }";

/// 이슈 노드 필드 집합. 읽기와 write readback([`super::write`])이 같은 모양을 뽑는다.
pub(super) const ISSUE_FIELDS: &str =
    "id identifier title description url priority estimate dueDate branchName createdAt updatedAt state { id name type color } assignee { id displayName avatarUrl } creator { id } team { id key name color } labels { nodes { id name color } } project { id name color } cycle { id name } parent { id identifier }";

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

    pub async fn viewer_id(&self) -> Lookup<String> {
        let data = match self.request("query { viewer { id } }", json!({})).await {
            Ok(data) => data,
            Err(error) => return Lookup::Unavailable(error),
        };
        match data["viewer"]["id"].as_str() {
            Some(id) if !id.is_empty() => Lookup::Found(id.to_string()),
            _ => Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown)),
        }
    }

    pub async fn list_teams(&self) -> Lookup<Vec<LinearTeam>> {
        const QUERY: &str = "query { teams(first: 250) { nodes { id key name } } }";
        let data = match self.request(QUERY, json!({})).await {
            Ok(data) => data,
            Err(error) => return Lookup::Unavailable(error),
        };
        let Some(nodes) = data["teams"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(
            nodes
                .iter()
                .map(|node| LinearTeam {
                    id: node["id"].as_str().unwrap_or_default().to_string(),
                    key: node["key"].as_str().unwrap_or_default().to_string(),
                    name: node["name"].as_str().unwrap_or_default().to_string(),
                })
                .collect(),
        )
    }

    pub async fn list_team_members(&self, team: &str) -> Lookup<Vec<LinearMember>> {
        const QUERY: &str = "query TeamMembers($team: String!) { \
            team(id: $team) { members(first: 250) { nodes { id name displayName email } } } }";
        let data = match self.request(QUERY, json!({"team": team})).await {
            Ok(data) => data,
            Err(error) => return Lookup::Unavailable(error),
        };
        let Some(nodes) = data["team"]["members"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(
            nodes
                .iter()
                .map(|node| LinearMember {
                    id: node["id"].as_str().unwrap_or_default().to_string(),
                    name: node["displayName"]
                        .as_str()
                        .or_else(|| node["name"].as_str())
                        .unwrap_or_default()
                        .to_string(),
                    email: node["email"].as_str().map(str::to_string),
                })
                .collect(),
        )
    }

    pub async fn list_team_states(&self, team: &str) -> Lookup<Vec<LinearState>> {
        const QUERY: &str = "query TeamStates($team: String!) { \
            team(id: $team) { states(first: 250) { nodes { id name type } } } }";
        let data = match self.request(QUERY, json!({"team": team})).await {
            Ok(data) => data,
            Err(error) => return Lookup::Unavailable(error),
        };
        let Some(nodes) = data["team"]["states"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(
            nodes
                .iter()
                .map(|node| LinearState {
                    id: node["id"].as_str().unwrap_or_default().to_string(),
                    name: node["name"].as_str().unwrap_or_default().to_string(),
                    state_type: node["type"].as_str().map(str::to_string),
                })
                .collect(),
        )
    }

    pub async fn list_team_labels(&self, team: &str) -> Lookup<Vec<LinearLabel>> {
        const QUERY: &str = "query TeamLabels($team: String!) { \
            team(id: $team) { labels(first: 250) { nodes { id name color } } } }";
        let data = match self.request(QUERY, json!({"team": team})).await {
            Ok(data) => data,
            Err(error) => return Lookup::Unavailable(error),
        };
        let Some(nodes) = data["team"]["labels"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(
            nodes
                .iter()
                .map(|node| LinearLabel {
                    id: node["id"].as_str().unwrap_or_default().to_string(),
                    name: node["name"].as_str().unwrap_or_default().to_string(),
                    color: node["color"].as_str().map(str::to_string),
                })
                .collect(),
        )
    }

    pub async fn list_projects(&self) -> Lookup<Vec<LinearProject>> {
        const QUERY: &str = "query { projects(first: 250) { nodes { id name state url } } }";
        let data = match self.request(QUERY, json!({})).await {
            Ok(data) => data,
            Err(error) => return Lookup::Unavailable(error),
        };
        let Some(nodes) = data["projects"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        Lookup::Found(
            nodes
                .iter()
                .map(|node| LinearProject {
                    id: node["id"].as_str().unwrap_or_default().to_string(),
                    name: node["name"].as_str().unwrap_or_default().to_string(),
                    state: node["state"].as_str().map(str::to_string),
                    url: node["url"].as_str().map(str::to_string),
                })
                .collect(),
        )
    }

    /// 이슈 목록 — **bounded full traversal + stuck-cursor 가드**(§1.2). limit(`MAX_ISSUES`)나
    /// `!hasNextPage`까지 페이지를 돈다. `has_more`로 truncation을 표면화(무성 절단 금지).
    ///
    /// `filter`는 Linear `IssueFilter`(예: `{"state":{"type":{"eq":"started"}}}`). `None`이면 전체.
    pub async fn list_issues(&self, filter: Option<Value>) -> Lookup<IssuePage> {
        const LIST_QUERY: &str = "query ListIssues($first: Int!, $after: String, $filter: IssueFilter) { \
             issues(first: $first, after: $after, filter: $filter) { \
                 nodes { id identifier title description url priority estimate dueDate state { id name type } assignee { id displayName } creator { id } team { id key name } labels { nodes { id name } } project { id name } parent { id identifier } } \
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
                 nodes { id identifier title description url priority estimate dueDate state { id name type } assignee { id displayName } creator { id } team { id key name } labels { nodes { id name } } project { id name } parent { id identifier } } } }";
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
        let query =
            format!("query GetIssue($id: String!) {{ issue(id: $id) {{ {ISSUE_FIELDS} }} }}");
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
        match self.read_comments(id).await {
            Ok((comments, _)) => Lookup::Found(comments),
            Err(error) => Lookup::Unavailable(error),
        }
    }

    /// Orca agent issue-context contract. Optional sections fail independently,
    /// preserve bounded partial results, and expose collection truncation.
    pub async fn get_issue_context(
        &self,
        id: &str,
        options: LinearIssueContextOptions,
    ) -> Lookup<Value> {
        let issue = match self.get_issue(id).await {
            Lookup::Found(issue) => issue,
            Lookup::NotFound => return Lookup::NotFound,
            Lookup::Unavailable(error) => return Lookup::Unavailable(error),
        };
        let include = json!({
            "comments": options.comments,
            "children": options.children,
            "attachments": options.attachments,
            "relations": options.relations,
            "activity": options.activity,
        });
        let mut result = json!({
            "issue": issue_context_json(&issue),
            "meta": {
                "requested": {
                    "id": id,
                    "current": false,
                    "include": include,
                    "depth": options.depth.clamp(0, 5),
                },
                "resolved": {
                    "id": issue.id,
                    "identifier": issue.identifier,
                },
                "partial": false,
                "includeErrors": [],
                "sections": {},
            }
        });
        let mut inline_media = extract_inline_media(
            issue.description.as_deref().unwrap_or_default(),
            "description",
            None,
        );
        if options.comments {
            match self.read_comments(&issue.id).await {
                Ok((comments, meta)) => {
                    let values = comments
                        .iter()
                        .map(|comment| {
                            let media = comment.inline_media.clone();
                            inline_media.extend(media.clone());
                            let mut value = json!({
                                "id": comment.id,
                                "body": comment.body,
                                "bodyTruncated": comment.body_truncated,
                                "createdAt": comment.created_at,
                                "updatedAt": comment.updated_at,
                                "parentId": comment.parent_id,
                                "user": {
                                    "id": comment.author_id,
                                    "displayName": comment.author,
                                    "avatarUrl": comment.author_avatar_url,
                                }
                            });
                            if !media.is_empty() {
                                value["inlineMedia"] = Value::Array(media);
                            }
                            value
                        })
                        .collect();
                    result["comments"] = Value::Array(values);
                    result["meta"]["sections"]["comments"] = meta;
                }
                Err(error) => push_include_error(&mut result, "comments", &error),
            }
        }
        if options.children {
            let mut returned = 0usize;
            match self
                .read_children(&issue.id, 1, options.depth.clamp(0, 5), &mut returned)
                .await
            {
                Ok((children, may_have_more)) => {
                    collect_child_inline_media(&children, &mut inline_media);
                    result["children"] = Value::Array(children);
                    result["meta"]["sections"]["children"] = json!({
                        "returned": returned,
                        "cap": 200,
                        "capReached": returned >= 200,
                        "mayHaveMore": may_have_more,
                    });
                }
                Err(error) => push_include_error(&mut result, "children", &error),
            }
        }
        if options.attachments {
            match self.read_attachments(&issue.id).await {
                Ok((items, meta)) => {
                    result["attachments"] = Value::Array(items);
                    result["meta"]["sections"]["attachments"] = meta;
                }
                Err(error) => push_include_error(&mut result, "attachments", &error),
            }
        }
        if options.relations {
            match self.read_relations_context(&issue.id).await {
                Ok((items, meta)) => {
                    result["relations"] = Value::Array(items);
                    result["meta"]["sections"]["relations"] = meta;
                }
                Err(error) => push_include_error(&mut result, "relations", &error),
            }
        }
        if options.activity {
            match self.read_activity(&issue.id).await {
                Ok((items, meta)) => {
                    result["activity"] = Value::Array(items);
                    result["meta"]["sections"]["activity"] = meta;
                }
                Err(error) => push_include_error(&mut result, "activity", &error),
            }
        }
        if !inline_media.is_empty() {
            result["inlineMedia"] = Value::Array(inline_media);
        }
        result["meta"]["partial"] = Value::Bool(
            result["meta"]["includeErrors"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty()),
        );
        Lookup::Found(result)
    }

    pub async fn get_issue_relations(&self, id: &str) -> Lookup<LinearRelationPage> {
        const QUERY: &str = "query IssueRelations($id: String!) { issue(id: $id) { \
            relations(first: 250) { nodes { id type issue { id identifier title url } relatedIssue { id identifier title url } } pageInfo { hasNextPage } } \
            inverseRelations(first: 250) { nodes { id type issue { id identifier title url } relatedIssue { id identifier title url } } pageInfo { hasNextPage } } \
        } }";
        let data = match self.request(QUERY, json!({"id": id})).await {
            Ok(data) => data,
            Err(error) => return Lookup::Unavailable(error),
        };
        let issue = &data["issue"];
        if issue.is_null() {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        }
        let Some(outbound) = issue["relations"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        let Some(inbound) = issue["inverseRelations"]["nodes"].as_array() else {
            return Lookup::Unavailable(Classified::new(TrackerUnavailable::Unknown));
        };
        let relations = outbound
            .iter()
            .map(|node| parse_relation(node, true))
            .chain(inbound.iter().map(|node| parse_relation(node, false)))
            .collect();
        Lookup::Found(LinearRelationPage {
            relations,
            has_more: issue["relations"]["pageInfo"]["hasNextPage"]
                .as_bool()
                .unwrap_or(false)
                || issue["inverseRelations"]["pageInfo"]["hasNextPage"]
                    .as_bool()
                    .unwrap_or(false),
        })
    }

    async fn read_issue_connection(
        &self,
        id: &str,
        query: &str,
        field: &str,
        limit: usize,
    ) -> Result<(Vec<Value>, bool), Classified> {
        let mut nodes = Vec::new();
        let mut after: Option<String> = None;
        let mut has_more = false;
        while nodes.len() < limit {
            let first = (limit - nodes.len()).min(50);
            let mut variables = json!({"id": id, "first": first});
            if let Some(cursor) = after.as_ref() {
                variables["after"] = Value::String(cursor.clone());
            }
            let data = self.request(query, variables).await?;
            if data["issue"].is_null() {
                return Err(Classified::new(TrackerUnavailable::Unknown));
            }
            let connection = &data["issue"][field];
            let page = connection["nodes"]
                .as_array()
                .ok_or_else(|| Classified::new(TrackerUnavailable::Unknown))?;
            nodes.extend(page.iter().take(limit - nodes.len()).cloned());
            has_more = connection["pageInfo"]["hasNextPage"]
                .as_bool()
                .unwrap_or(false);
            let next = connection["pageInfo"]["endCursor"]
                .as_str()
                .map(str::to_string);
            if !has_more || next.is_none() || next == after || page.is_empty() {
                break;
            }
            after = next;
        }
        Ok((nodes, has_more))
    }

    async fn read_comments(&self, id: &str) -> Result<(Vec<Comment>, Value), Classified> {
        const QUERY: &str = "query IssueComments($id: String!, $first: Int, $after: String) { \
            issue(id: $id) { comments(first: $first, after: $after) { nodes { \
            id body createdAt updatedAt parent { id } user { id displayName avatarUrl } \
            } pageInfo { hasNextPage endCursor } } } }";
        let (nodes, has_more) = self
            .read_issue_connection(id, QUERY, "comments", 500)
            .await?;
        let comments = nodes.iter().map(parse_comment).collect::<Vec<_>>();
        Ok((comments, collection_meta(nodes.len(), 500, has_more)))
    }

    fn read_children<'a>(
        &'a self,
        id: &'a str,
        level: usize,
        depth: usize,
        returned: &'a mut usize,
    ) -> ChildrenReadFuture<'a> {
        Box::pin(async move {
            if depth == 0 || level > depth || *returned >= 200 {
                return Ok((Vec::new(), level > depth || *returned >= 200));
            }
            const QUERY: &str =
                "query IssueChildren($id: String!, $first: Int, $after: String) { \
                issue(id: $id) { children(first: $first, after: $after) { nodes { \
                id identifier title description url priority estimate dueDate branchName createdAt updatedAt \
                state { id name type color } assignee { id displayName avatarUrl } team { id key name color } \
                labels { nodes { id name color } } project { id name color } cycle { id name } \
                } pageInfo { hasNextPage endCursor } } } }";
            let remaining = 200usize.saturating_sub(*returned);
            let (nodes, page_has_more) = self
                .read_issue_connection(id, QUERY, "children", remaining)
                .await?;
            let mut values = Vec::new();
            let mut may_have_more = page_has_more;
            for node in nodes {
                if *returned >= 200 {
                    may_have_more = true;
                    break;
                }
                *returned += 1;
                let child_id = node["id"].as_str().unwrap_or_default().to_string();
                let mut child = issue_node_json(&node);
                if level < depth && *returned < 200 {
                    let (nested, nested_more) = self
                        .read_children(&child_id, level + 1, depth, returned)
                        .await?;
                    if !nested.is_empty() {
                        child["children"] = Value::Array(nested);
                    }
                    may_have_more |= nested_more;
                } else {
                    may_have_more = true;
                }
                child["mayHaveMore"] =
                    Value::Bool(level >= depth || *returned >= 200 || page_has_more);
                values.push(child);
            }
            Ok((values, may_have_more))
        })
    }

    async fn read_attachments(&self, id: &str) -> Result<(Vec<Value>, Value), Classified> {
        const QUERY: &str = "query IssueAttachments($id: String!, $first: Int, $after: String) { \
            issue(id: $id) { attachments(first: $first, after: $after) { nodes { \
            id title url source subtitle createdAt \
            } pageInfo { hasNextPage endCursor } } } }";
        let (nodes, has_more) = self
            .read_issue_connection(id, QUERY, "attachments", 100)
            .await?;
        let values = nodes
            .iter()
            .map(|node| {
                json!({
                    "id": node["id"],
                    "title": node["title"],
                    "url": node["url"],
                    "source": node["source"],
                    "subtitle": node["subtitle"],
                    "createdAt": node["createdAt"],
                    "metadataOnly": true,
                })
            })
            .collect();
        Ok((values, collection_meta(nodes.len(), 100, has_more)))
    }

    async fn read_relations_context(&self, id: &str) -> Result<(Vec<Value>, Value), Classified> {
        const OUTBOUND: &str = "query IssueRelations($id: String!, $first: Int, $after: String) { \
            issue(id: $id) { relations(first: $first, after: $after) { nodes { \
            id type relatedIssue { id identifier title url } \
            } pageInfo { hasNextPage endCursor } } } }";
        const INBOUND: &str =
            "query IssueInverseRelations($id: String!, $first: Int, $after: String) { \
            issue(id: $id) { inverseRelations(first: $first, after: $after) { nodes { \
            id type issue { id identifier title url } relatedIssue { id identifier title url } \
            } pageInfo { hasNextPage endCursor } } } }";
        let (outbound, outbound_more) = self
            .read_issue_connection(id, OUTBOUND, "relations", 100)
            .await?;
        let remaining = 100usize.saturating_sub(outbound.len());
        let (inverse_probe, inverse_more) = self
            .read_issue_connection(id, INBOUND, "inverseRelations", remaining.max(1))
            .await?;
        let inverse_overflow = inverse_probe.len() > remaining;
        let mut values = outbound
            .iter()
            .take(100)
            .map(|node| relation_context_json(node, true))
            .collect::<Vec<_>>();
        values.extend(
            inverse_probe
                .iter()
                .take(remaining)
                .map(|node| relation_context_json(node, false)),
        );
        let has_more = outbound_more || inverse_more || inverse_overflow;
        Ok((values.clone(), collection_meta(values.len(), 100, has_more)))
    }

    async fn read_activity(&self, id: &str) -> Result<(Vec<Value>, Value), Classified> {
        const QUERY: &str =
            "query IssueActivity($id: String!, $first: Int, $after: String) { issue(id: $id) { \
            history(first: $first, after: $after) { nodes { id createdAt updatedAt \
            actor { id displayName avatarUrl } botActor { id name avatarUrl type subType userDisplayName } \
            fromTitle toTitle updatedDescription fromPriority toPriority fromEstimate toEstimate \
            fromDueDate toDueDate fromAssignee { id displayName avatarUrl } toAssignee { id displayName avatarUrl } \
            fromDelegate { id displayName avatarUrl } toDelegate { id displayName avatarUrl } \
            fromState { id name type color } toState { id name type color } \
            fromProject { id name color } toProject { id name color } fromCycle { id name } toCycle { id name } \
            fromParent { id identifier title url } toParent { id identifier title url } \
            fromTeam { id name key color } toTeam { id name key color } \
            fromProjectMilestone { id name } toProjectMilestone { id name } \
            addedLabels { id name color } removedLabels { id name color } \
            relationChanges { identifier type } attachment { id title url } archived autoArchived autoClosed trashed \
            } pageInfo { hasNextPage endCursor } } } }";
        let (nodes, has_more) = self
            .read_issue_connection(id, QUERY, "history", 250)
            .await?;
        let values = nodes.iter().map(activity_json).collect::<Vec<_>>();
        Ok((values, collection_meta(nodes.len(), 250, has_more)))
    }
}

fn collection_meta(returned: usize, cap: usize, has_more: bool) -> Value {
    json!({
        "returned": returned,
        "cap": cap,
        "capReached": returned >= cap || has_more,
        "hasMore": has_more,
    })
}

fn push_include_error(result: &mut Value, include: &str, error: &Classified) {
    let code = match error.kind {
        TrackerUnavailable::NotAuthenticated => "linear_auth_expired",
        TrackerUnavailable::RateLimited => "linear_rate_limited",
        TrackerUnavailable::Forbidden => "linear_permission_denied",
        TrackerUnavailable::Network | TrackerUnavailable::Internal => "linear_network_error",
        TrackerUnavailable::InvalidInput | TrackerUnavailable::Unknown => "linear_include_failed",
    };
    let message = error
        .user_message
        .as_deref()
        .unwrap_or("This Linear section could not be loaded.");
    if let Some(errors) = result["meta"]["includeErrors"].as_array_mut() {
        errors.push(json!({"include": include, "code": code, "message": message}));
    }
}

fn issue_context_json(issue: &Issue) -> Value {
    json!({
        "id": issue.id,
        "identifier": issue.identifier,
        "title": issue.title,
        "url": issue.url,
        "description": issue.description,
        "state": {
            "id": issue.state_id,
            "name": issue.state,
            "type": issue.state_type,
        },
        "team": {
            "id": issue.team_id,
            "name": issue.team_name,
            "key": issue.team_key,
        },
        "project": {
            "id": issue.project_id,
            "name": issue.project_name,
        },
        "cycle": {
            "id": issue.cycle_id,
            "name": issue.cycle_name,
        },
        "assignee": {
            "id": issue.assignee_id,
            "displayName": issue.assignee,
            "avatarUrl": issue.assignee_avatar_url,
        },
        "labels": issue.label_ids.iter().zip(issue.label_names.iter()).map(|(id, name)| {
            json!({"id": id, "name": name})
        }).collect::<Vec<_>>(),
        "priority": issue.priority,
        "estimate": issue.estimate,
        "dueDate": issue.due_date,
        "branchName": issue.branch_name,
        "createdAt": issue.created_at,
        "updatedAt": issue.updated_at,
    })
}

fn issue_node_json(node: &Value) -> Value {
    json!({
        "id": node["id"],
        "identifier": node["identifier"],
        "title": node["title"],
        "url": node["url"],
        "description": node["description"],
        "state": node["state"],
        "team": node["team"],
        "project": node["project"],
        "cycle": node["cycle"],
        "assignee": node["assignee"],
        "labels": node["labels"]["nodes"].as_array().cloned().unwrap_or_default(),
        "priority": node["priority"],
        "estimate": node["estimate"],
        "dueDate": node["dueDate"],
        "branchName": node["branchName"],
        "createdAt": node["createdAt"],
        "updatedAt": node["updatedAt"],
    })
}

fn relation_context_json(node: &Value, outbound: bool) -> Value {
    let relation_type = node["type"].as_str();
    let relationship = match (relation_type, outbound) {
        (Some("blocks"), true) => "blocks",
        (Some("blocks"), false) => "blockedBy",
        (Some("duplicate"), true) => "duplicateOf",
        (Some("duplicate"), false) => "duplicatedBy",
        (Some("similar"), _) => "similar",
        _ => "relatedTo",
    };
    json!({
        "id": node["id"],
        "type": node["type"],
        "direction": if outbound { "outbound" } else { "inbound" },
        "relationship": relationship,
        "relatedIssue": if outbound { node["relatedIssue"].clone() } else { node["issue"].clone() },
    })
}

fn activity_json(node: &Value) -> Value {
    let mut changes = Vec::new();
    push_activity_change(&mut changes, "title", &node["fromTitle"], &node["toTitle"]);
    if node["updatedDescription"].as_bool().unwrap_or(false) {
        changes.push(json!({"field": "description"}));
    }
    for (field, from, to) in [
        ("priority", "fromPriority", "toPriority"),
        ("estimate", "fromEstimate", "toEstimate"),
        ("dueDate", "fromDueDate", "toDueDate"),
        ("assignee", "fromAssignee", "toAssignee"),
        ("delegate", "fromDelegate", "toDelegate"),
        ("state", "fromState", "toState"),
        ("project", "fromProject", "toProject"),
        ("cycle", "fromCycle", "toCycle"),
        ("parent", "fromParent", "toParent"),
        ("team", "fromTeam", "toTeam"),
        ("milestone", "fromProjectMilestone", "toProjectMilestone"),
    ] {
        push_activity_change(&mut changes, field, &node[from], &node[to]);
    }
    if node["addedLabels"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        changes.push(json!({"field": "labelsAdded", "to": node["addedLabels"]}));
    }
    if node["removedLabels"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        changes.push(json!({"field": "labelsRemoved", "from": node["removedLabels"]}));
    }
    if node["relationChanges"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        changes.push(json!({"field": "relations", "to": node["relationChanges"]}));
    }
    if !node["attachment"].is_null() {
        changes.push(json!({"field": "attachment", "to": node["attachment"]}));
    }
    for field in ["archived", "autoArchived", "autoClosed", "trashed"] {
        if !node[field].is_null() {
            changes.push(json!({"field": field, "to": node[field]}));
        }
    }
    let actor = if !node["actor"].is_null() {
        let mut actor = node["actor"].clone();
        actor["kind"] = Value::String("user".into());
        actor
    } else if !node["botActor"].is_null() {
        json!({
            "id": node["botActor"]["id"],
            "displayName": node["botActor"]["userDisplayName"]
                .as_str()
                .or_else(|| node["botActor"]["name"].as_str()),
            "avatarUrl": node["botActor"]["avatarUrl"],
            "name": node["botActor"]["name"],
            "type": node["botActor"]["type"],
            "subType": node["botActor"]["subType"],
            "kind": "bot",
        })
    } else {
        json!({"kind": "system", "displayName": "Linear"})
    };
    json!({
        "id": node["id"],
        "createdAt": node["createdAt"],
        "updatedAt": node["updatedAt"],
        "actor": actor,
        "changes": changes,
    })
}

fn push_activity_change(changes: &mut Vec<Value>, field: &str, from: &Value, to: &Value) {
    if from.is_null() && to.is_null() {
        return;
    }
    changes.push(json!({"field": field, "from": from, "to": to}));
}

fn extract_inline_media(body: &str, source: &str, source_id: Option<&str>) -> Vec<Value> {
    static MARKDOWN: OnceLock<Regex> = OnceLock::new();
    static HTML: OnceLock<Regex> = OnceLock::new();
    let markdown = MARKDOWN.get_or_init(|| {
        Regex::new(r#"!\[([^\]]*)\]\(\s*(<[^>]+>|[^)\s]+)(?:\s+["'][^"']*["'])?\s*\)"#)
            .expect("valid Linear markdown media regex")
    });
    let html = HTML.get_or_init(|| {
        Regex::new(r#"(?i)<(?:img|video|audio|source)\b[^>]*\bsrc=["']([^"']+)["'][^>]*>"#)
            .expect("valid Linear HTML media regex")
    });
    let mut media = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for captures in markdown.captures_iter(body) {
        let raw = captures.get(2).map(|value| value.as_str()).unwrap_or("");
        let raw = raw
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .unwrap_or(raw);
        add_inline_media(
            &mut media,
            &mut seen,
            raw,
            source,
            source_id,
            captures.get(1).map(|value| value.as_str()),
        );
    }
    for captures in html.captures_iter(body) {
        add_inline_media(
            &mut media,
            &mut seen,
            captures.get(1).map(|value| value.as_str()).unwrap_or(""),
            source,
            source_id,
            None,
        );
    }
    media
}

fn add_inline_media(
    media: &mut Vec<Value>,
    seen: &mut std::collections::HashSet<String>,
    raw_url: &str,
    source: &str,
    source_id: Option<&str>,
    alt_text: Option<&str>,
) {
    let url = raw_url.trim();
    let parsed = url::Url::parse(url).ok();
    let supported = url.starts_with("data:image/")
        || url.starts_with("data:video/")
        || parsed
            .as_ref()
            .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
    if !supported || !seen.insert(url.to_string()) {
        return;
    }
    let file_name = parsed.as_ref().and_then(|url| {
        url.path_segments()?
            .rfind(|segment| !segment.is_empty())
            .map(str::to_string)
    });
    let mut value = json!({
        "source": source,
        "url": url,
        "altText": alt_text.filter(|value| !value.is_empty()),
        "fileName": file_name,
        "linearUpload": parsed.as_ref().is_some_and(|url| url.host_str() == Some("uploads.linear.app")),
    });
    if let Some(source_id) = source_id {
        value["sourceId"] = Value::String(source_id.to_string());
    }
    media.push(value);
}

fn collect_child_inline_media(children: &[Value], target: &mut Vec<Value>) {
    for child in children {
        if let Some(description) = child["description"].as_str() {
            target.extend(extract_inline_media(
                description,
                "child-description",
                child["id"].as_str(),
            ));
        }
        if let Some(nested) = child["children"].as_array() {
            collect_child_inline_media(nested, target);
        }
    }
}

/// `data` 노드 → [`Issue`]. 없는 필드는 빈 문자열/None(파싱 실패로 전체를 떨구지 않는다).
/// write readback([`super::write`])도 같은 파서로 확인된 이슈를 만든다.
pub(super) fn parse_issue(v: &Value) -> Issue {
    let labels = v["labels"]["nodes"].as_array();
    Issue {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        identifier: v["identifier"].as_str().unwrap_or_default().to_string(),
        title: v["title"].as_str().unwrap_or_default().to_string(),
        description: v["description"].as_str().map(str::to_string),
        url: v["url"].as_str().map(str::to_string),
        state: v["state"]["name"].as_str().map(str::to_string),
        state_id: v["state"]["id"].as_str().map(str::to_string),
        state_type: v["state"]["type"].as_str().map(str::to_string),
        assignee: v["assignee"]["displayName"].as_str().map(str::to_string),
        assignee_id: v["assignee"]["id"].as_str().map(str::to_string),
        creator_id: v["creator"]["id"].as_str().map(str::to_string),
        team_id: v["team"]["id"].as_str().map(str::to_string),
        team_key: v["team"]["key"].as_str().map(str::to_string),
        team_name: v["team"]["name"].as_str().map(str::to_string),
        priority: v["priority"].as_i64(),
        estimate: match &v["estimate"] {
            Value::Number(number) => Some(number.to_string()),
            Value::String(value) => Some(value.clone()),
            _ => None,
        },
        due_date: v["dueDate"].as_str().map(str::to_string),
        label_ids: labels
            .into_iter()
            .flatten()
            .filter_map(|label| label["id"].as_str().map(str::to_string))
            .collect(),
        label_names: labels
            .into_iter()
            .flatten()
            .filter_map(|label| label["name"].as_str().map(str::to_string))
            .collect(),
        project_id: v["project"]["id"].as_str().map(str::to_string),
        project_name: v["project"]["name"].as_str().map(str::to_string),
        parent_id: v["parent"]["id"].as_str().map(str::to_string),
        parent_identifier: v["parent"]["identifier"].as_str().map(str::to_string),
        branch_name: v["branchName"].as_str().map(str::to_string),
        created_at: v["createdAt"].as_str().map(str::to_string),
        updated_at: v["updatedAt"].as_str().map(str::to_string),
        cycle_id: v["cycle"]["id"].as_str().map(str::to_string),
        cycle_name: v["cycle"]["name"].as_str().map(str::to_string),
        assignee_avatar_url: v["assignee"]["avatarUrl"].as_str().map(str::to_string),
    }
}

/// `data` 노드 → [`Comment`].
fn parse_comment(v: &Value) -> Comment {
    let full_body = v["body"].as_str().unwrap_or_default();
    let body_truncated = full_body.chars().count() > 20_000;
    Comment {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        body: full_body.chars().take(20_000).collect(),
        author: v["user"]["displayName"].as_str().map(str::to_string),
        created_at: v["createdAt"].as_str().map(str::to_string),
        updated_at: v["updatedAt"].as_str().map(str::to_string),
        parent_id: v["parent"]["id"].as_str().map(str::to_string),
        author_id: v["user"]["id"].as_str().map(str::to_string),
        author_avatar_url: v["user"]["avatarUrl"].as_str().map(str::to_string),
        body_truncated,
        inline_media: extract_inline_media(
            full_body,
            "comment",
            v["id"].as_str().filter(|id| !id.is_empty()),
        ),
    }
}

pub(super) fn parse_relation(v: &Value, outbound: bool) -> LinearRelation {
    let relation_type = v["type"].as_str().map(str::to_string);
    let neighbor = if outbound {
        &v["relatedIssue"]
    } else {
        &v["issue"]
    };
    let relationship = match (relation_type.as_deref(), outbound) {
        (Some("blocks"), true) => "blocks",
        (Some("blocks"), false) => "blockedBy",
        (Some("duplicate"), true) => "duplicateOf",
        (Some("duplicate"), false) => "duplicatedBy",
        (Some("similar"), _) => "similar",
        _ => "relatedTo",
    };
    LinearRelation {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        relation_type,
        direction: if outbound { "outbound" } else { "inbound" }.into(),
        relationship: relationship.into(),
        related_issue_id: neighbor["id"].as_str().unwrap_or_default().to_string(),
        related_identifier: neighbor["identifier"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        related_title: neighbor["title"].as_str().unwrap_or_default().to_string(),
        related_url: neighbor["url"].as_str().map(str::to_string),
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
        assert_eq!(
            t.last_header("Authorization").as_deref(),
            Some("lin_api_rawkey_ABC")
        );
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
        let vars: Value = serde_json::from_str(t.requests()[1].body.as_deref().unwrap()).unwrap();
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
                assert_eq!(
                    c.user_message.as_deref(),
                    Some("You are being rate limited.")
                );
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
            &format!(
                r#"{{"data":{{"issue":{}}}}}"#,
                &ONE_NODE[1..ONE_NODE.len() - 1]
            ),
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

    #[tokio::test]
    async fn issue_context_walks_comment_cursors_and_extracts_inline_media() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            &format!(
                r#"{{"data":{{"issue":{}}}}}"#,
                &ONE_NODE[1..ONE_NODE.len() - 1]
            ),
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"issue":{"comments":{"nodes":[
                {"id":"c1","body":"![shot](https://uploads.linear.app/acme/file/image-1?sig=x)",
                 "createdAt":"2026-07-23T00:00:00Z","user":{"id":"u1","displayName":"Ada"}}
              ],"pageInfo":{"hasNextPage":true,"endCursor":"cursor-1"}}}}}"#,
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"issue":{"comments":{"nodes":[
                {"id":"c2","body":"<video src=\"https://cdn.example/video.mp4\"></video>"}
              ],"pageInfo":{"hasNextPage":false,"endCursor":"cursor-2"}}}}}"#,
        ));
        let options = LinearIssueContextOptions {
            comments: true,
            depth: 2,
            ..LinearIssueContextOptions::default()
        };
        match client(t.clone()).get_issue_context("ENG-1", options).await {
            Lookup::Found(context) => {
                assert_eq!(context["comments"].as_array().unwrap().len(), 2);
                assert_eq!(context["meta"]["sections"]["comments"]["returned"], 2);
                assert_eq!(context["inlineMedia"].as_array().unwrap().len(), 2);
                assert_eq!(context["inlineMedia"][0]["linearUpload"], true);
                assert_eq!(context["inlineMedia"][0]["sourceId"], "c1");
                assert_eq!(context["meta"]["partial"], false);
            }
            other => panic!("expected context, got {other:?}"),
        }
        assert_eq!(t.requests().len(), 3);
        let second_page: Value =
            serde_json::from_str(t.requests()[2].body.as_deref().unwrap()).unwrap();
        assert_eq!(second_page["variables"]["after"], "cursor-1");
    }

    #[tokio::test]
    async fn issue_context_keeps_other_sections_when_one_include_fails() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            &format!(
                r#"{{"data":{{"issue":{}}}}}"#,
                &ONE_NODE[1..ONE_NODE.len() - 1]
            ),
        ));
        t.push_response(ok(200, ERRORS_200_RATELIMIT));
        t.push_response(ok(
            200,
            r#"{"data":{"issue":{"attachments":{"nodes":[
                {"id":"a1","title":"Review","url":"https://example.com/review"}
              ],"pageInfo":{"hasNextPage":false}}}}}"#,
        ));
        let options = LinearIssueContextOptions {
            comments: true,
            attachments: true,
            depth: 2,
            ..LinearIssueContextOptions::default()
        };
        match client(t).get_issue_context("ENG-1", options).await {
            Lookup::Found(context) => {
                assert_eq!(context["meta"]["partial"], true);
                assert_eq!(
                    context["meta"]["includeErrors"][0]["code"],
                    "linear_rate_limited"
                );
                assert_eq!(context["attachments"][0]["metadataOnly"], true);
            }
            other => panic!("expected partial context, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn issue_context_maps_activity_actor_and_changes() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            &format!(
                r#"{{"data":{{"issue":{}}}}}"#,
                &ONE_NODE[1..ONE_NODE.len() - 1]
            ),
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"issue":{"history":{"nodes":[{
                "id":"h1","createdAt":"2026-07-23T00:00:00Z",
                "botActor":{"id":"b1","name":"Workflow","userDisplayName":"Automation"},
                "fromTitle":"Old","toTitle":"New","updatedDescription":true,
                "addedLabels":[{"id":"l1","name":"Bug"}],"archived":false
              }],"pageInfo":{"hasNextPage":false}}}}}"#,
        ));
        let options = LinearIssueContextOptions {
            activity: true,
            depth: 2,
            ..LinearIssueContextOptions::default()
        };
        match client(t).get_issue_context("ENG-1", options).await {
            Lookup::Found(context) => {
                let activity = &context["activity"][0];
                assert_eq!(activity["actor"]["kind"], "bot");
                assert_eq!(activity["actor"]["displayName"], "Automation");
                assert_eq!(activity["changes"][0]["field"], "title");
                assert!(activity["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|change| change["field"] == "labelsAdded"));
            }
            other => panic!("expected activity context, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn metadata_queries_preserve_ids_and_provider_fields() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"teams":{"nodes":[{"id":"team_1","key":"ENG","name":"Engineering"}]}}}"#,
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"team":{"members":{"nodes":[{"id":"user_1","name":"Ada","displayName":"Ada L","email":"ada@example.com"}]}}}}"#,
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"team":{"states":{"nodes":[{"id":"state_1","name":"In Progress","type":"started"}]}}}}"#,
        ));
        t.push_response(ok(
            200,
            r##"{"data":{"team":{"labels":{"nodes":[{"id":"label_1","name":"Bug","color":"#ff0000"}]}}}}"##,
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"projects":{"nodes":[{"id":"project_1","name":"Launch","state":"started","url":"https://linear.app/project/launch"}]}}}"#,
        ));
        let c = client(t.clone());
        match c.list_teams().await {
            Lookup::Found(values) => assert_eq!(values[0].key, "ENG"),
            other => panic!("expected teams, got {other:?}"),
        }
        match c.list_team_members("team_1").await {
            Lookup::Found(values) => assert_eq!(values[0].name, "Ada L"),
            other => panic!("expected members, got {other:?}"),
        }
        match c.list_team_states("team_1").await {
            Lookup::Found(values) => assert_eq!(values[0].state_type.as_deref(), Some("started")),
            other => panic!("expected states, got {other:?}"),
        }
        match c.list_team_labels("team_1").await {
            Lookup::Found(values) => assert_eq!(values[0].color.as_deref(), Some("#ff0000")),
            other => panic!("expected labels, got {other:?}"),
        }
        match c.list_projects().await {
            Lookup::Found(values) => assert_eq!(values[0].name, "Launch"),
            other => panic!("expected projects, got {other:?}"),
        }
        assert_eq!(t.requests().len(), 5);
        assert!(t.requests()[1]
            .body
            .as_deref()
            .is_some_and(|body| body.contains(r#""team":"team_1""#)));
    }

    #[tokio::test]
    async fn relation_query_normalizes_outbound_and_inbound_perspectives() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"issue":{
                "relations":{"nodes":[{"id":"r1","type":"blocks",
                    "issue":{"id":"a"},"relatedIssue":{"id":"b","identifier":"ENG-2","title":"B","url":null}}],
                    "pageInfo":{"hasNextPage":false}},
                "inverseRelations":{"nodes":[{"id":"r2","type":"blocks",
                    "issue":{"id":"c","identifier":"ENG-3","title":"C","url":null},"relatedIssue":{"id":"a"}}],
                    "pageInfo":{"hasNextPage":false}}
            }}}"#,
        ));
        match client(t).get_issue_relations("a").await {
            Lookup::Found(page) => {
                assert!(!page.has_more);
                assert_eq!(page.relations[0].relationship, "blocks");
                assert_eq!(page.relations[0].related_identifier, "ENG-2");
                assert_eq!(page.relations[1].relationship, "blockedBy");
                assert_eq!(page.relations[1].related_identifier, "ENG-3");
            }
            other => panic!("expected relations, got {other:?}"),
        }
    }
}
