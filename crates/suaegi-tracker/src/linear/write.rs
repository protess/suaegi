//! **Linear write-back (N3a) — mutation-verifiable 백엔드 코어.** `...ForAgent` write 경로:
//! `create_issue` / `update_issue`(상태 전이=워크플로우 이동) / `add_comment` / `attach_link`.
//! 읽기 클라이언트([`super::client`])의 GraphQL-over-HTTP POST를 그대로 재사용한다(같은 엔드포인트,
//! 같은 auth). N3b(에이전트 CLI-RPC 노출 + 티켓으로 런치)는 여기 없다 — 이 파일은 백엔드만.
//!
//! **write 규율(§0.3 — 읽기의 "거짓 음성 금지"의 write 쪽 거울):**
//! - **writeId 멱등**: 클라이언트가 준 write-id(UUID)를 `input.id`로 실어 create/comment/attach를
//!   멱등하게 만든다. 같은 write-id 재시도는 두 번째 create가 아니라 [`WriteOutcome::Duplicate`]로
//!   접힌다 — 서버가 중복 id를 거부하기 때문(Orca `parseLinearWriteId`+`isDuplicateIdError` 미러).
//! - **readback 확인**: write 후 반드시 다시 읽어 랜딩을 확인한다(Orca `confirmLinearWrite`).
//!   readback이 성공하면 [`WriteOutcome::Written`]. readback **자체가** 실패(전송/GraphQL/null)하면
//!   [`WriteOutcome::Unconfirmed`] — 성공도 실패도 주장하지 않는다(write가 랜딩했는지 모른다;
//!   같은 write-id 재시도가 duplicate를 잡는다). **write 쪽 캐시-오염 가드.**
//! - **4-way 분류**(Orca `LinearWriteFailureKind` = duplicate_id/failed/network/unconfirmed):
//!   - `duplicate_id` → [`WriteOutcome::Duplicate`]
//!   - `failed`(확정 거부 — GraphQL이 입력이 나쁘다고 명시) → [`WriteOutcome::Rejected`]
//!   - `network`(일시 전송 실패) → [`WriteOutcome::Unavailable`]
//!   - `unconfirmed`(write는 보냈으나 readback 확인 불가) → [`WriteOutcome::Unconfirmed`]
//!   **write POST의 네트워크 타임아웃은 절대 `failed`/Rejected가 아니다** — 확정 "실패"는 성공했을
//!   수도 있는 write에 대한 정확성 거짓말이다(write가 랜딩했을 수 있다). Orca와 같이 timeout →
//!   `unconfirmed`, connect-refused → `network`로 접는다.
//!
//! 분류 축은 읽기와 같은 [`super::classify::classify_graphql`]의 `errors[0].extensions.type`을
//! 재사용한다(Orca의 message-substring 매칭은 미러하지 않는다). 예외는 duplicate 하나 —
//! Linear가 중복 id에 별도 extensions.type을 노출하는지 미확인이라, extensions.type을 먼저 보고
//! message-substring으로 폴백한다(human-eyes로 실측). raw message는 **탐지에만** 쓰고 결과엔 담지 않는다.

use super::classify::{classify_graphql, GraphqlOutcome};
use super::client::{parse_issue, ISSUE_FIELDS};
use super::model::{Classified, Issue, TrackerUnavailable};
use super::LinearClient;
use serde_json::{json, Value};
use std::time::Duration;
use suaegi_http::TransportError;

/// write 조회 타임아웃(읽기와 동일 30s).
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

// ---- 클라이언트 제공 멱등 키 ----

/// 클라이언트가 제공하는 write-id(멱등 키). Linear UUID(8-4-4-4-12 hex, 대소문자 무관 — Orca
/// `isLinearUuid`/`LINEAR_UUID_PATTERN` 미러). create/comment/attach의 `input.id`로 실린다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteId(String);

/// write-id가 UUID 모양이 아님(Orca `linear_invalid_write_id`). raw 값을 담지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidWriteId;

impl WriteId {
    /// UUID 모양이면 write-id로. Orca는 UUID가 아닌 write-id를 확정 거부한다(멱등의 전제).
    pub fn parse(value: &str) -> Result<Self, InvalidWriteId> {
        if is_linear_uuid(value) {
            Ok(Self(value.to_string()))
        } else {
            Err(InvalidWriteId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 8-4-4-4-12 hex(대소문자 무관). 정규식 의존 없이 손으로 — Orca `LINEAR_UUID_PATTERN`과 동치.
fn is_linear_uuid(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut parts = value.split('-');
    for len in GROUPS {
        match parts.next() {
            Some(part) if part.len() == len && part.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    // 정확히 5그룹이어야 한다(뒤에 더 붙으면 거부).
    parts.next().is_none()
}

// ---- write 입력 ----

/// 새 이슈 입력(`create_issue`). 최소 필드 — 깊은 필드(priority/labels 등)는 보류(§5).
#[derive(Debug, Clone)]
pub struct NewIssue {
    /// 대상 팀 id(Linear는 이슈가 팀에 속한다).
    pub team_id: String,
    pub title: String,
    pub description: Option<String>,
}

/// 이슈 갱신 입력(`update_issue`). **`state_id`가 상태 전이(워크플로우 이동)** — N3a의 핵심 write.
/// 모든 필드 `None`이면 no-op 갱신(그래도 readback로 현재 상태를 확인).
#[derive(Debug, Clone, Default)]
pub struct IssueUpdate {
    /// 워크플로우 상태 id — 이게 "티켓을 In Progress/Done으로 옮김".
    pub state_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub assignee_id: Option<String>,
}

// ---- write 결과 ----

/// 확인된 코멘트 write 레코드(readback). 읽기 [`super::Comment`]와 달리 write 호출부가 필요로 하는
/// url + 이슈 식별자를 담는다(Orca `LinearCommentWriteRecord` 축소판).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedComment {
    pub id: String,
    pub url: Option<String>,
    pub issue_identifier: Option<String>,
}

/// 확인된 첨부(링크) write 레코드(readback). Orca `LinearAttachmentWriteRecord` 축소판.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedAttachment {
    pub id: String,
    pub title: String,
    pub url: String,
    pub issue_identifier: Option<String>,
}

/// write 한 번의 구조화된 결과. **caller(N3b)가 무엇을 할지 결정** — 절대 bare bool이 아니다.
/// 4-way 분류(§0.3)를 이 다섯 변형으로 표현한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome<T> {
    /// readback으로 **확인된** 성공. 실제로 랜딩한 레코드를 담는다.
    Written(T),
    /// 같은 write-id 재시도가 확정적 멱등 충돌을 만남(`duplicate_id`). 새 레코드를 만들지 않았다 —
    /// 이미 존재하는 write의 id를 담는다. **재시도 안전성의 증거.**
    Duplicate(String),
    /// 확정적 거부(`failed`) — GraphQL이 입력이 나쁘다고 명시(invalid input/user error, 또는
    /// mutation success:false). 재시도해도 무의미. 분류된 이유(raw 바디 없음).
    Rejected(Classified),
    /// write는 전송됐으나 readback이 확인 못함(`unconfirmed`). 성공도 실패도 주장 안 함 —
    /// write가 랜딩했는지 모른다. 재시도가 같은 write-id로 duplicate를 잡는다. **write-side crux.**
    Unconfirmed,
    /// 일시/전송 실패로 사용 불가(`network` 및 기타 transient). 재시도 가능. 쓰기 도달 여부 불명.
    Unavailable(Classified),
}

/// write 도중의 종결 신호(성공 T를 아직 안 담은 상태). readback 전에 결정될 수 있는 4가지 실패.
/// [`WriteHalt::into_outcome`]가 제네릭 [`WriteOutcome`]로 승격한다.
enum WriteHalt {
    Duplicate(String),
    Rejected(Classified),
    Unconfirmed,
    Unavailable(Classified),
}

impl WriteHalt {
    fn into_outcome<T>(self) -> WriteOutcome<T> {
        match self {
            WriteHalt::Duplicate(id) => WriteOutcome::Duplicate(id),
            WriteHalt::Rejected(c) => WriteOutcome::Rejected(c),
            WriteHalt::Unconfirmed => WriteOutcome::Unconfirmed,
            WriteHalt::Unavailable(c) => WriteOutcome::Unavailable(c),
        }
    }
}

// ---- 중복 탐지 ----

/// GraphQL 에러가 **중복 id**(멱등 재시도) 신호인지. extensions.type을 **먼저** 보고, 없으면
/// message-substring으로 폴백한다(Orca `isDuplicateIdError` 미러 — Linear가 중복에 별도 type을
/// 주는지 미확인이라 message 폴백이 현재 유일한 길, human-eyes 실측 대상). raw message는 **탐지에만**
/// 쓰고 절대 [`WriteOutcome`]에 담지 않는다(누출 방지).
fn is_duplicate_write_error(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let Some(first) = value.get("errors").and_then(|e| e.get(0)) else {
        return false;
    };
    // 1순위: extensions.type에 duplicate 신호(문서화되면 여기로).
    if let Some(type_str) = first
        .get("extensions")
        .and_then(|e| e.get("type"))
        .and_then(|v| v.as_str())
    {
        if type_str.to_ascii_lowercase().contains("duplicate") {
            return true;
        }
    }
    // 폴백: raw message substring(탐지 전용, 저장 안 함).
    let message = first
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    message.contains("duplicate")
        || message.contains("already exists")
        || message.contains("already in use")
        || message.contains("id has already")
}

// ---- write 오퍼레이션 ----

impl LinearClient {
    /// 이슈 생성(`createIssueForAgent`). write-id를 `input.id`로 실어 멱등하게. 성공이면 write-id로
    /// readback해 확인된 [`Issue`]를 [`WriteOutcome::Written`]으로.
    pub async fn create_issue(&self, write_id: &WriteId, input: NewIssue) -> WriteOutcome<Issue> {
        const CREATE: &str = "mutation IssueCreate($input: IssueCreateInput!) { \
             issueCreate(input: $input) { success issue { id } } }";
        let mut fields = json!({
            "id": write_id.as_str(),
            "teamId": input.team_id,
            "title": input.title,
        });
        if let Some(desc) = &input.description {
            fields["description"] = json!(desc);
        }
        let vars = json!({ "input": fields });
        if let Err(halt) = self
            .post_mutation(CREATE, vars, "issueCreate", write_id.as_str())
            .await
        {
            return halt.into_outcome();
        }
        // create는 input.id == 레코드 id → write-id로 readback.
        match self.readback_issue(write_id.as_str()).await {
            Ok(issue) => WriteOutcome::Written(issue),
            Err(halt) => halt.into_outcome(),
        }
    }

    /// 이슈 갱신(`updateIssueForAgent`) — **`state_id`가 상태 전이(워크플로우 이동)**. 갱신 후
    /// 대상 이슈 id로 readback해 확인. update는 create가 아니라 duplicate가 없다(자연 멱등).
    pub async fn update_issue(&self, issue_id: &str, update: IssueUpdate) -> WriteOutcome<Issue> {
        const UPDATE: &str = "mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) { \
             issueUpdate(id: $id, input: $input) { success issue { id } } }";
        let mut fields = json!({});
        if let Some(v) = &update.state_id {
            fields["stateId"] = json!(v);
        }
        if let Some(v) = &update.title {
            fields["title"] = json!(v);
        }
        if let Some(v) = &update.description {
            fields["description"] = json!(v);
        }
        if let Some(v) = &update.assignee_id {
            fields["assigneeId"] = json!(v);
        }
        let vars = json!({ "id": issue_id, "input": fields });
        // update엔 write-id가 input.id로 실리지 않는다(대상은 기존 이슈). duplicate 라벨용 id는 비움.
        if let Err(halt) = self.post_mutation(UPDATE, vars, "issueUpdate", "").await {
            return halt.into_outcome();
        }
        match self.readback_issue(issue_id).await {
            Ok(issue) => WriteOutcome::Written(issue),
            Err(halt) => halt.into_outcome(),
        }
    }

    /// 코멘트 추가(`addIssueCommentForAgent`). write-id를 `input.id`로 실어 멱등하게. readback로 확인.
    pub async fn add_comment(
        &self,
        write_id: &WriteId,
        issue_id: &str,
        body: &str,
    ) -> WriteOutcome<CreatedComment> {
        const CREATE: &str = "mutation CommentCreate($input: CommentCreateInput!) { \
             commentCreate(input: $input) { success comment { id } } }";
        let vars = json!({ "input": {
            "id": write_id.as_str(),
            "issueId": issue_id,
            "body": body,
        }});
        if let Err(halt) = self
            .post_mutation(CREATE, vars, "commentCreate", write_id.as_str())
            .await
        {
            return halt.into_outcome();
        }
        match self.readback_comment(write_id.as_str()).await {
            Ok(comment) => WriteOutcome::Written(comment),
            Err(halt) => halt.into_outcome(),
        }
    }

    /// 링크 첨부(`createIssueAttachment`) — PR/산출물 링크를 티켓에 건다. write-id를 `input.id`로
    /// 실어 멱등하게. readback로 확인.
    pub async fn attach_link(
        &self,
        write_id: &WriteId,
        issue_id: &str,
        title: &str,
        url: &str,
    ) -> WriteOutcome<CreatedAttachment> {
        const CREATE: &str = "mutation AttachmentCreate($input: AttachmentCreateInput!) { \
             attachmentCreate(input: $input) { success attachment { id } } }";
        let vars = json!({ "input": {
            "id": write_id.as_str(),
            "issueId": issue_id,
            "title": title,
            "url": url,
        }});
        if let Err(halt) = self
            .post_mutation(CREATE, vars, "attachmentCreate", write_id.as_str())
            .await
        {
            return halt.into_outcome();
        }
        match self.readback_attachment(write_id.as_str()).await {
            Ok(attachment) => WriteOutcome::Written(attachment),
            Err(halt) => halt.into_outcome(),
        }
    }

    // ---- write 내부: POST + 4-way 분류 ----

    /// mutation POST 한 번을 치고 **write 규율로** 분류한다. `Ok(())`면 수용됨(→ readback 진행),
    /// `Err(halt)`면 readback 전에 종결(Duplicate/Rejected/Unavailable/Unconfirmed).
    ///
    /// **crux**: write POST의 전송 실패는 절대 Rejected가 아니다 — 타임아웃 → Unconfirmed(랜딩
    /// 했을 수 있음), connect 실패 → Unavailable(Network)(재시도 가능). GraphQL 에러는 확정
    /// 입력-오류(invalid input)만 Rejected, transient(rate/network/internal/auth/forbidden)는
    /// Unavailable. 중복은 Duplicate.
    async fn post_mutation(
        &self,
        query: &str,
        variables: Value,
        field: &str,
        write_id: &str,
    ) -> Result<(), WriteHalt> {
        let resp = match self.post_graphql(query, variables, WRITE_TIMEOUT).await {
            // 미인증 → 쓸 수 없음(재시도해도 재인증 전엔 무의미하나 확정 입력-오류는 아님).
            None => {
                return Err(WriteHalt::Unavailable(Classified::new(
                    TrackerUnavailable::NotAuthenticated,
                )))
            }
            // **write POST 타임아웃 → Unconfirmed.** 확정 "실패" 아님 — write가 랜딩했을 수 있다.
            Some(Err(TransportError::Timeout)) => return Err(WriteHalt::Unconfirmed),
            // connect 실패 → 서버에 도달 못함(write 안 남) → Network, 재시도 가능. 절대 Rejected 아님.
            Some(Err(TransportError::Connect(_))) => {
                return Err(WriteHalt::Unavailable(Classified::new(
                    TrackerUnavailable::Network,
                )))
            }
            Some(Ok(resp)) => resp,
        };

        match classify_graphql(resp.status, &resp.body) {
            GraphqlOutcome::Success(data) => match data[field]["success"].as_bool() {
                // 확정 수용 → readback 진행.
                Some(true) => Ok(()),
                // 서버가 명시적으로 실패 보고 → 확정 거부(Orca `if(!result.success) throw 'failed'`).
                Some(false) => Err(WriteHalt::Rejected(Classified::new(
                    TrackerUnavailable::InvalidInput,
                ))),
                // success 필드 부재/비-불리언 → 예상 밖 모양. 성공도 실패도 주장 안 함.
                None => Err(WriteHalt::Unconfirmed),
            },
            GraphqlOutcome::Failure(classified) => {
                // 중복 id는 멱등 신호(재시도) — Rejected보다 먼저 본다.
                if is_duplicate_write_error(&resp.body) {
                    Err(WriteHalt::Duplicate(write_id.to_string()))
                } else if classified.kind == TrackerUnavailable::InvalidInput {
                    // 확정 입력-오류만 Rejected(재시도 무의미).
                    Err(WriteHalt::Rejected(classified))
                } else {
                    // rate/network/internal/auth/forbidden/unknown → transient/재시도-가능 →
                    // **절대 Rejected 아님.** write가 랜딩 안 했을 가능성이 크나 확정 거부는 아니다.
                    Err(WriteHalt::Unavailable(classified))
                }
            }
        }
    }

    /// write 후 이슈 readback. **모든 실패(전송/GraphQL/null) → Unconfirmed** — write가 랜딩했는지
    /// 확인 못함. 성공+비-null만 확인된 [`Issue`](Orca `confirmLinearWrite`+`getCreatedIssueRecord`).
    async fn readback_issue(&self, id: &str) -> Result<Issue, WriteHalt> {
        let query = format!("query IssueByUuid($id: String!) {{ issue(id: $id) {{ {ISSUE_FIELDS} }} }}");
        let node = self.readback_node(&query, id, "issue").await?;
        Ok(parse_issue(&node))
    }

    /// write 후 코멘트 readback. 실패 → Unconfirmed.
    async fn readback_comment(&self, id: &str) -> Result<CreatedComment, WriteHalt> {
        const QUERY: &str = "query CommentByUuid($id: String!) { \
             comment(id: $id) { id url body issue { id identifier url } } }";
        let node = self.readback_node(QUERY, id, "comment").await?;
        Ok(CreatedComment {
            id: node["id"].as_str().unwrap_or_default().to_string(),
            url: node["url"].as_str().map(str::to_string),
            issue_identifier: node["issue"]["identifier"].as_str().map(str::to_string),
        })
    }

    /// write 후 첨부 readback. 실패 → Unconfirmed.
    async fn readback_attachment(&self, id: &str) -> Result<CreatedAttachment, WriteHalt> {
        const QUERY: &str = "query AttachmentByUuid($id: String!) { \
             attachment(id: $id) { id title url issue { id identifier url } } }";
        let node = self.readback_node(QUERY, id, "attachment").await?;
        Ok(CreatedAttachment {
            id: node["id"].as_str().unwrap_or_default().to_string(),
            title: node["title"].as_str().unwrap_or_default().to_string(),
            url: node["url"].as_str().unwrap_or_default().to_string(),
            issue_identifier: node["issue"]["identifier"].as_str().map(str::to_string),
        })
    }

    /// readback 공통: POST → 성공이면 `root` 노드(비-null)를 준다. **모든 실패는 Unconfirmed**(전송
    /// 실패, GraphQL 에러, 성공+null 모두). write가 랜딩했는지 확인 못함 — 성공/실패 주장 안 함.
    async fn readback_node(&self, query: &str, id: &str, root: &str) -> Result<Value, WriteHalt> {
        match self.post_graphql(query, json!({ "id": id }), WRITE_TIMEOUT).await {
            // 미인증 또는 전송 실패 → 확인 불가(랜딩했을 수 있으니 절대 실패 단정 아님).
            None | Some(Err(_)) => Err(WriteHalt::Unconfirmed),
            Some(Ok(resp)) => match classify_graphql(resp.status, &resp.body) {
                GraphqlOutcome::Success(data) => {
                    let node = &data[root];
                    if node.is_null() {
                        // 성공+null → 아직 안 보임/못 찾음 → 확인 불가.
                        Err(WriteHalt::Unconfirmed)
                    } else {
                        Ok(node.clone())
                    }
                }
                // readback **자체가** GraphQL 에러(rate/network 등) → 확인 불가. write 성공 주장 금지.
                GraphqlOutcome::Failure(_) => Err(WriteHalt::Unconfirmed),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use suaegi_http::{FakeTransport, HttpResponse};
    use suaegi_secrets::Secret;

    const WRITE_ID: &str = "3b241101-e2bb-4255-8caf-4136c566a962";

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

    fn wid() -> WriteId {
        WriteId::parse(WRITE_ID).unwrap()
    }

    fn new_issue() -> NewIssue {
        NewIssue {
            team_id: "team_1".to_string(),
            title: "New ticket".to_string(),
            description: Some("body".to_string()),
        }
    }

    // ---- 실제 Linear GraphQL write-response 모양 픽스처 ----

    /// mutation 성공(success:true + issue id). readback와 짝.
    const CREATE_OK: &str =
        r#"{"data":{"issueCreate":{"success":true,"issue":{"id":"3b241101-e2bb-4255-8caf-4136c566a962"}}}}"#;

    /// create readback 성공 — 확인된 이슈.
    const ISSUE_READBACK_OK: &str = r#"{"data":{"issue":{
        "id":"3b241101-e2bb-4255-8caf-4136c566a962","identifier":"ENG-9","title":"New ticket",
        "description":"body","url":"https://linear.app/acme/issue/ENG-9",
        "state":{"name":"Todo"},"assignee":null}}}"#;

    /// **같은 write-id 재시도** — Linear가 중복 id를 HTTP 200 + errors로 거부하는 실제 모양. raw
    /// message는 내부, extensions.type은 (현재) invalid input.
    const DUPLICATE_200: &str = r#"{"errors":[{"message":"A record with that id already exists (duplicate key value)",
        "extensions":{"type":"invalid input","userPresentableMessage":"That id is already in use."}}]}"#;

    /// **확정 거부** — 입력이 나쁨(invalid input, 중복 아님).
    const REJECT_INVALID_200: &str = r#"{"errors":[{"message":"secret query internals XYZ: title must not be empty",
        "extensions":{"type":"invalid input","userPresentableMessage":"Title is required."}}]}"#;

    /// **transient** — 레이트리밋(HTTP 200 + errors). write에서 절대 Rejected가 아니어야 한다.
    const RATELIMIT_200: &str = r#"{"errors":[{"message":"complexity limit",
        "extensions":{"type":"ratelimited","userPresentableMessage":"Slow down."}}]}"#;

    // ---- WriteId 파싱(멱등의 전제) ----

    #[test]
    fn write_id_accepts_uuid_rejects_non_uuid() {
        assert!(WriteId::parse(WRITE_ID).is_ok());
        assert!(WriteId::parse("3B241101-E2BB-4255-8CAF-4136C566A962").is_ok(), "대소문자 무관");
        assert_eq!(WriteId::parse("not-a-uuid"), Err(InvalidWriteId));
        assert_eq!(WriteId::parse(""), Err(InvalidWriteId));
        // 길이/그룹 어긋남 거부.
        assert_eq!(WriteId::parse("3b241101-e2bb-4255-8caf-4136c566a9"), Err(InvalidWriteId));
        assert_eq!(
            WriteId::parse("3b241101-e2bb-4255-8caf-4136c566a962-extra"),
            Err(InvalidWriteId)
        );
        // 비-hex 문자 거부.
        assert_eq!(WriteId::parse("zb241101-e2bb-4255-8caf-4136c566a962"), Err(InvalidWriteId));
    }

    // ---- create: 성공 경로 ----

    #[tokio::test]
    async fn create_success_readback_confirms_written() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, CREATE_OK));
        t.push_response(ok(200, ISSUE_READBACK_OK));
        match client(t.clone()).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Written(issue) => {
                assert_eq!(issue.identifier, "ENG-9");
                assert_eq!(issue.state.as_deref(), Some("Todo"));
            }
            other => panic!("expected Written, got {other:?}"),
        }
        // 정확히 2요청: mutation + readback. mutation input.id == write-id(멱등 키).
        let reqs = t.requests();
        assert_eq!(reqs.len(), 2);
        let body: Value = serde_json::from_str(reqs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["variables"]["input"]["id"], WRITE_ID, "write-id는 input.id로 실린다");
        assert_eq!(body["variables"]["input"]["teamId"], "team_1");
        // readback은 같은 write-id로 조회.
        let rb: Value = serde_json::from_str(reqs[1].body.as_deref().unwrap()).unwrap();
        assert_eq!(rb["variables"]["id"], WRITE_ID);
    }

    // ---- crux (a): write POST 전송 실패는 절대 Rejected/failed 아님 ----

    /// **mutation-verified 크럭스 (a-1)**: write POST 타임아웃 → Unconfirmed, **절대 Rejected 아님**.
    /// 확정 "실패"는 성공했을 수 있는 write에 대한 정확성 거짓말. `post_mutation`의 Timeout 팔을
    /// `WriteHalt::Rejected(..)`로 바꾸면(변형) 이 테스트가 깨진다.
    #[tokio::test]
    async fn create_write_timeout_is_unconfirmed_never_rejected() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(Err(TransportError::Timeout));
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Unconfirmed => {}
            WriteOutcome::Rejected(_) => {
                panic!("a transient write timeout must NOT be a definitive rejection")
            }
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
    }

    /// **크럭스 (a-2)**: write POST connect 실패 → Unavailable(Network), 절대 Rejected 아님.
    #[tokio::test]
    async fn create_write_connect_is_unavailable_network_never_rejected() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(Err(TransportError::Connect("network error".to_string())));
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Unavailable(c) => assert_eq!(c.kind, TrackerUnavailable::Network),
            WriteOutcome::Rejected(_) => panic!("a transport connect failure must NOT be rejected"),
            other => panic!("expected Unavailable(Network), got {other:?}"),
        }
    }

    /// **크럭스 (a-3)**: transient GraphQL 에러(rate limit)도 write에서 절대 Rejected 아님.
    #[tokio::test]
    async fn create_ratelimit_is_unavailable_never_rejected() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, RATELIMIT_200));
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Unavailable(c) => {
                assert_eq!(c.kind, TrackerUnavailable::RateLimited);
                assert_eq!(c.user_message.as_deref(), Some("Slow down."));
            }
            WriteOutcome::Rejected(_) => panic!("a rate-limited write must NOT be a rejection"),
            other => panic!("expected Unavailable(RateLimited), got {other:?}"),
        }
    }

    // ---- crux (b): readback 실패 → Unconfirmed, 절대 Written/Rejected 아님 ----

    /// **mutation-verified 크럭스 (b)**: mutation은 OK인데 readback이 전송 실패 → Unconfirmed.
    /// 절대 Written(성공 거짓말)도 Rejected(실패 거짓말)도 아님. `readback_node`의 전송-실패 팔을
    /// `Ok(..)`/Written으로 바꾸면 이 테스트가 깨진다(write-side 캐시-오염 가드).
    #[tokio::test]
    async fn create_readback_transport_fail_is_unconfirmed_not_written() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, CREATE_OK)); // mutation OK
        t.push_response(Err(TransportError::Timeout)); // readback 전송 실패
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Unconfirmed => {}
            WriteOutcome::Written(_) => {
                panic!("a failed readback must NOT be claimed as confirmed success")
            }
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
    }

    /// readback이 GraphQL 에러(rate limit)를 반환 → Unconfirmed(write 성공 주장 금지).
    #[tokio::test]
    async fn create_readback_graphql_error_is_unconfirmed() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, CREATE_OK));
        t.push_response(ok(200, RATELIMIT_200)); // readback 자체가 rate-limited
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Unconfirmed => {}
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
    }

    /// readback 성공인데 issue:null → Unconfirmed(아직 못 찾음).
    #[tokio::test]
    async fn create_readback_null_is_unconfirmed() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, CREATE_OK));
        t.push_response(ok(200, r#"{"data":{"issue":null}}"#));
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Unconfirmed => {}
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
    }

    // ---- crux (c): 같은 write-id 재시도 → Duplicate(멱등) ----

    /// **mutation-verified 크럭스 (c)**: 중복 id GraphQL 에러 → Duplicate, **readback 안 함**.
    /// `is_duplicate_write_error`를 항상 false로 바꾸면(변형) 같은 픽스처가 Rejected로 떨어져 이
    /// 테스트가 깨진다(멱등 상실).
    #[tokio::test]
    async fn create_duplicate_id_retry_is_duplicate_not_second_create() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, DUPLICATE_200));
        match client(t.clone()).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Duplicate(id) => assert_eq!(id, WRITE_ID),
            WriteOutcome::Rejected(_) => {
                panic!("a same-write-id retry must be idempotent Duplicate, not Rejected")
            }
            other => panic!("expected Duplicate, got {other:?}"),
        }
        // 중복은 종결 — readback POST를 하지 않는다(정확히 1요청).
        assert_eq!(t.requests().len(), 1, "duplicate is terminal; no readback");
    }

    // ---- 확정 거부(invalid input, 중복 아님) → Rejected ----

    #[tokio::test]
    async fn create_invalid_input_is_rejected() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, REJECT_INVALID_200));
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Rejected(c) => {
                assert_eq!(c.kind, TrackerUnavailable::InvalidInput);
                assert_eq!(c.user_message.as_deref(), Some("Title is required."));
            }
            other => panic!("expected Rejected(InvalidInput), got {other:?}"),
        }
    }

    /// mutation success:false → Rejected(확정 거부, Orca `if(!result.success) throw 'failed'`).
    #[tokio::test]
    async fn create_success_false_is_rejected() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, r#"{"data":{"issueCreate":{"success":false,"issue":null}}}"#));
        match client(t).create_issue(&wid(), new_issue()).await {
            WriteOutcome::Rejected(c) => assert_eq!(c.kind, TrackerUnavailable::InvalidInput),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    // ---- crux (d): raw 에러/토큰이 결과/Debug에 안 샌다 ----

    /// **mutation-verified 크럭스 (d)**: 확정 거부의 raw `errors[0].message`(내부 쿼리 텍스트)가
    /// [`WriteOutcome`] Debug 어디에도 안 샌다. userPresentableMessage만. duplicate 탐지에 쓴 raw
    /// message도 결과엔 담기지 않는다.
    #[tokio::test]
    async fn rejected_outcome_redacts_raw_error_message() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, REJECT_INVALID_200));
        let outcome = client(t).create_issue(&wid(), new_issue()).await;
        let rendered = format!("{outcome:?}");
        assert!(
            !rendered.contains("secret query internals"),
            "raw GraphQL error message leaked into WriteOutcome: {rendered}"
        );
    }

    /// 토큰이 write 경로 어디에도 Debug로 새지 않는다(클라이언트 Debug는 고정 라벨).
    #[tokio::test]
    async fn token_never_leaks_in_write_client_debug() {
        let c = client(Arc::new(FakeTransport::default()));
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("rawkey"), "raw key leaked into Debug: {dbg}");
    }

    // ---- update: 상태 전이(워크플로우 이동) ----

    #[tokio::test]
    async fn update_state_transition_confirms_written() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"issueUpdate":{"success":true,"issue":{"id":"iss_1"}}}}"#,
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"issue":{"id":"iss_1","identifier":"ENG-9","title":"New ticket",
                "description":"body","url":"https://linear.app/acme/issue/ENG-9",
                "state":{"name":"In Progress"},"assignee":null}}}"#,
        ));
        let update = IssueUpdate {
            state_id: Some("state_started".to_string()),
            ..Default::default()
        };
        match client(t.clone()).update_issue("iss_1", update).await {
            WriteOutcome::Written(issue) => {
                assert_eq!(issue.state.as_deref(), Some("In Progress"));
            }
            other => panic!("expected Written, got {other:?}"),
        }
        // mutation은 대상 issue id + stateId를 싣는다(상태 전이).
        let body: Value = serde_json::from_str(t.requests()[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["variables"]["id"], "iss_1");
        assert_eq!(body["variables"]["input"]["stateId"], "state_started");
    }

    /// update readback 실패도 Unconfirmed(상태가 옮겨졌는지 확인 못함).
    #[tokio::test]
    async fn update_readback_fail_is_unconfirmed() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"issueUpdate":{"success":true,"issue":{"id":"iss_1"}}}}"#,
        ));
        t.push_response(Err(TransportError::Timeout));
        let update = IssueUpdate {
            state_id: Some("s".to_string()),
            ..Default::default()
        };
        match client(t).update_issue("iss_1", update).await {
            WriteOutcome::Unconfirmed => {}
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
    }

    // ---- comment / attach ----

    #[tokio::test]
    async fn add_comment_confirms_written() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"commentCreate":{"success":true,"comment":{"id":"3b241101-e2bb-4255-8caf-4136c566a962"}}}}"#,
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"comment":{"id":"3b241101-e2bb-4255-8caf-4136c566a962",
                "url":"https://linear.app/acme/issue/ENG-9#comment-1","body":"progress",
                "issue":{"id":"iss_1","identifier":"ENG-9","url":"https://linear.app/acme/issue/ENG-9"}}}}"#,
        ));
        match client(t).add_comment(&wid(), "iss_1", "progress").await {
            WriteOutcome::Written(c) => {
                assert_eq!(c.issue_identifier.as_deref(), Some("ENG-9"));
                assert!(c.url.is_some());
            }
            other => panic!("expected Written, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn add_comment_readback_fail_is_unconfirmed() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"commentCreate":{"success":true,"comment":{"id":"3b241101-e2bb-4255-8caf-4136c566a962"}}}}"#,
        ));
        t.push_response(Err(TransportError::Connect("network error".to_string())));
        match client(t).add_comment(&wid(), "iss_1", "progress").await {
            WriteOutcome::Unconfirmed => {}
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn attach_link_confirms_written() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(
            200,
            r#"{"data":{"attachmentCreate":{"success":true,"attachment":{"id":"3b241101-e2bb-4255-8caf-4136c566a962"}}}}"#,
        ));
        t.push_response(ok(
            200,
            r#"{"data":{"attachment":{"id":"3b241101-e2bb-4255-8caf-4136c566a962",
                "title":"PR #42","url":"https://github.com/acme/repo/pull/42",
                "issue":{"id":"iss_1","identifier":"ENG-9","url":"https://linear.app/acme/issue/ENG-9"}}}}"#,
        ));
        match client(t).attach_link(&wid(), "iss_1", "PR #42", "https://github.com/acme/repo/pull/42").await {
            WriteOutcome::Written(a) => {
                assert_eq!(a.title, "PR #42");
                assert_eq!(a.issue_identifier.as_deref(), Some("ENG-9"));
            }
            other => panic!("expected Written, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn attach_link_duplicate_is_duplicate() {
        let t = Arc::new(FakeTransport::default());
        t.push_response(ok(200, DUPLICATE_200));
        match client(t).attach_link(&wid(), "iss_1", "PR #42", "https://x").await {
            WriteOutcome::Duplicate(id) => assert_eq!(id, WRITE_ID),
            other => panic!("expected Duplicate, got {other:?}"),
        }
    }

    /// 미인증(토큰 없음) write → Unavailable(NotAuthenticated), 전송조차 안 함.
    #[tokio::test]
    async fn unauthenticated_write_is_unavailable_not_authenticated() {
        let t = Arc::new(FakeTransport::default());
        let c = LinearClient::with_transport(t.clone(), None);
        match c.create_issue(&wid(), new_issue()).await {
            WriteOutcome::Unavailable(cl) => {
                assert_eq!(cl.kind, TrackerUnavailable::NotAuthenticated)
            }
            other => panic!("expected Unavailable(NotAuthenticated), got {other:?}"),
        }
        assert_eq!(t.requests().len(), 0, "no token → no write sent");
    }
}
