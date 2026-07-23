//! Linear 도메인 레코드 + 조회 결과 shape. 규율은 forge와 동일하다:
//! **found / none / unavailable을 절대 뭉개지 않는다** — 일시 실패(transient)는 결코 None/empty가
//! 아니다(캐시-오염 방지 crux, §1.1).
//!
//! 결과 shape([`Lookup`]/[`TrackerUnavailable`]/[`Classified`])는 Jira(N2)와 공용이라
//! [`crate::common`]으로 올렸다. 여기선 그걸 re-export만 한다(공개 API 경로 불변).

pub use crate::common::{Classified, Lookup, TrackerUnavailable};

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
