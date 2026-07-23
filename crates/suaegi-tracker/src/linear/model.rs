//! Linear 도메인 레코드 + 조회 결과 shape. 규율은 forge와 동일하다:
//! **found / none / unavailable을 절대 뭉개지 않는다** — 일시 실패(transient)는 결코 None/empty가
//! 아니다(캐시-오염 방지 crux, §1.1).

/// 분류된 실패 축. **고정 라벨 enum** — raw GraphQL 에러 문자열을 담지 않는다(쿼리 내부 누출
/// 방지). 사용자에게 보여도 되는 문자열은 오직 [`Classified::user_message`]
/// (`errors[0].extensions.userPresentableMessage`)뿐이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerUnavailable {
    /// 인증 실패(`authentication error` / HTTP 401·기타 4xx).
    NotAuthenticated,
    /// 레이트리밋(`ratelimited` / HTTP 429) — 재시도 가능.
    RateLimited,
    /// 권한 없음(`forbidden` / HTTP 403).
    Forbidden,
    /// 네트워크/전송 실패(`network error` / HTTP 5xx / 타임아웃) — 재시도 가능.
    Network,
    /// 서버 내부 오류(`internal error` / HTTP 500).
    Internal,
    /// 잘못된 입력(`invalid input` / `user error`) — 재시도 무의미하지만 **None이 아니다**.
    InvalidInput,
    /// 매핑 못한 상태, 또는 `data:null`처럼 성공도 실패-분류도 아닌 미지 상태. **절대 None/empty
    /// 아님** — "모른다"이지 "없다"가 아니다.
    Unknown,
}

/// 분류 결과: 축 + (있다면) 사용자에게 보여도 되는 메시지. `user_message`는 오직
/// `errors[0].extensions.userPresentableMessage`에서만 온다 — raw `errors[0].message`(쿼리
/// 내부 누출 위험)는 절대 여기 담기지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub kind: TrackerUnavailable,
    pub user_message: Option<String>,
}

impl Classified {
    pub fn new(kind: TrackerUnavailable) -> Self {
        Self {
            kind,
            user_message: None,
        }
    }
}

/// 조회 결과. forge의 `ReviewLookup`과 같은 3-way: 찾음 / 진짜 없음 / (분류된) 사용 불가.
/// **`Unavailable`은 절대 `NotFound`로 접히지 않는다** — 일시 실패가 "없음"으로 캐시되면 안 된다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<T> {
    Found(T),
    /// 진짜로 없음(예: get_issue가 성공 응답에 유효한 결과가 오되 비어있음). Linear의 "not
    /// found"는 GraphQL 에러라 실제로는 대부분 `Unavailable`로 온다(§1.2, human-eyes TODO).
    NotFound,
    Unavailable(Classified),
}

/// 연결된 워크스페이스 레코드(연결-계정 UI + 딥링크). `test_connection`이 채운다(§1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearWorkspace {
    /// organization id.
    pub id: String,
    pub name: String,
    /// `linear.app/{url_key}/...` 딥링크·재연결 식별자.
    pub url_key: String,
    /// 연결된 계정(viewer) 이메일.
    pub viewer_email: String,
}

/// 이슈 하나(리스트/검색/단건 공통). Orca 최소 필드 미러 — 표/미디어 등 깊은 필드는 보류(§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Linear 내부 id(uuid).
    pub id: String,
    /// 사람이 읽는 식별자(예: `ENG-123`). 워크트리 링크에 저장하는 값.
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    /// `linear.app/...` 딥링크.
    pub url: Option<String>,
    /// 상태 이름(예: `In Progress`).
    pub state: Option<String>,
    /// 담당자 표시 이름.
    pub assignee: Option<String>,
}

/// `list_issues`의 결과 한 장. `has_more`는 **무성 절단 금지**(회귀 메모리)를 위한 truncation
/// 신호 — limit에 걸려 끊었거나 stuck-cursor로 멈췄으면 true(UI "더 보기").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuePage {
    pub issues: Vec<Issue>,
    pub has_more: bool,
}

/// 이슈 코멘트 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub body: String,
    /// 작성자 표시 이름.
    pub author: Option<String>,
    /// ISO8601 생성 시각(원문 그대로).
    pub created_at: Option<String>,
}
