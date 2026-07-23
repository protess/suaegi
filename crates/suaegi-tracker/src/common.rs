//! 트래커 공용 결과 shape — Linear(N1)와 Jira(N2)가 **둘 다** 쓴다. jira→linear 모듈 의존
//! (레이어링 스멜)을 피하려 여기로 올렸다. 규율은 forge와 동일하다: **found / none /
//! unavailable을 절대 뭉개지 않는다** — 일시 실패(transient)는 결코 None/empty가 아니다
//! (캐시-오염 방지 crux).
//!
//! `user_message`의 출처는 provider마다 다르다: Linear는 `errors[0].extensions.
//! userPresentableMessage`(안전한 사용자용 문자열)를 담고, **Jira는 담지 않는다**(REST
//! 에러 바디 `errorMessages`가 JQL 등 내부를 노출할 수 있어 고정 라벨 `kind`만 쓴다).

/// 분류된 실패 축. **고정 라벨 enum** — raw 에러 문자열/바디를 담지 않는다(내부 누출 방지).
/// 사용자에게 보여도 되는 문자열은 오직 [`Classified::user_message`]뿐이고, 그마저도 provider가
/// 안전하다고 보장한 필드에서만 채운다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerUnavailable {
    /// 인증 실패(Linear `authentication error` / Jira HTTP 401). **401만** 크리덴셜 무효로
    /// 본다 — Jira는 프로젝트/API 권한 갭에 403을 내므로 403은 `Forbidden`이지 여기 아님.
    NotAuthenticated,
    /// 레이트리밋(`ratelimited` / HTTP 429) — 재시도 가능.
    RateLimited,
    /// 권한 없음(`forbidden` / HTTP 403). 크리덴셜 자체는 유효하나 이 리소스에 권한 부족.
    Forbidden,
    /// 네트워크/전송 실패(`network error` / HTTP 5xx / 타임아웃) — 재시도 가능.
    Network,
    /// 서버 내부 오류(`internal error` / HTTP 500).
    Internal,
    /// 잘못된 입력(`invalid input` / `user error` / HTTP 400) — 재시도 무의미하지만 **None이 아니다**.
    InvalidInput,
    /// 매핑 못한 상태, 또는 성공도 실패-분류도 아닌 미지 상태(예상 밖 출력, 컬렉션 엔드포인트의
    /// 404 등). **절대 None/empty 아님** — "모른다"이지 "없다"가 아니다.
    Unknown,
}

/// 분류 결과: 축 + (있다면) 사용자에게 보여도 되는 메시지. `user_message`는 provider가 안전하다고
/// 보장한 필드에서만 온다(Linear의 `userPresentableMessage`). **raw 에러 바디는 절대 여기 안 온다.**
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
    /// 진짜로 없음. Jira는 특정 리소스 엔드포인트(`GET /issue/{key}`)의 **404**가 여기로 온다
    /// (forge `review_by_number` 404→None 미러). 컬렉션/전역 404는 `Unavailable`.
    NotFound,
    Unavailable(Classified),
}
