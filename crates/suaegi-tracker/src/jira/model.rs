//! Jira 도메인 레코드 + 연결 설정(§2). 결과 shape([`Lookup`]/[`TrackerUnavailable`]/[`Classified`])는
//! Linear와 공용이라 [`crate::common`]에서 온다 — 규율 동일: **일시 실패는 결코 None/empty 아님**.

pub use crate::common::{Classified, Lookup, TrackerUnavailable};

/// 인증/배포 종류. Cloud와 Server/DC는 REST 버전(`apiBasePath`)·인증 헤더·바디 포맷(ADF vs
/// plain)이 갈린다. **cloud-ness는 이 per-connection 설정에서 온다**(Orca `JiraSite.authType`
/// 미러, `client.ts:72,335`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraAuthType {
    /// Atlassian Cloud — `/rest/api/3`, description/comment 바디는 ADF(JSON).
    Cloud,
    /// Self-hosted Server/Data Center — `/rest/api/2`, 바디는 plain text.
    Server,
}

impl JiraAuthType {
    /// REST 베이스 경로(Orca `apiBasePath`, `client.ts:72-74`). Cloud=v3, Server=v2.
    pub fn api_base_path(self) -> &'static str {
        match self {
            JiraAuthType::Cloud => "/rest/api/3",
            JiraAuthType::Server => "/rest/api/2",
        }
    }

    pub fn is_cloud(self) -> bool {
        matches!(self, JiraAuthType::Cloud)
    }
}

/// 한 Jira 연결(사이트)의 non-secret 설정. 토큰은 여기 없다 — `suaegi-secrets`(키체인)에서
/// 별도로 온다(Orca가 `jira-sites.json`을 평문, 토큰만 keychain에 두는 것과 동형). `site_url`은
/// 정규화된(끝 슬래시 없는) 절대 URL, `email`은 Server PAT면 빈 문자열일 수 있다(→ Bearer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraConnection {
    /// 정규화된 사이트 URL(예: `https://acme.atlassian.net`), 끝 슬래시 없음.
    pub site_url: String,
    /// 로그인 이메일(Cloud/Server-Basic). Server PAT면 빈 문자열.
    pub email: String,
    pub auth_type: JiraAuthType,
}

impl JiraConnection {
    /// `{site_url}{path}` 절대 URL. `path`는 `/rest/api/...`로 시작하는 것으로 가정.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.site_url, path)
    }
}

/// 연결 검증(`/myself`)이 채우는 계정 레코드(연결-계정 UI). Server/DC `/myself`는 accountId가
/// 없어 name/key가 안정 식별자(Orca `toViewer`, `client.ts:295-317`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraViewer {
    /// Cloud=accountId, Server=name/key. 담당자 지정 등의 안정 키.
    pub account_id: String,
    pub display_name: String,
    /// `emailAddress`(있으면). 없으면 연결 이메일로 폴백.
    pub email: Option<String>,
}

/// 이슈 하나(리스트/검색/단건 공통). Orca `mapJiraIssue`(`issues.ts:317-338`) 미러. `description`은
/// **이미 Markdown**으로 변환됨(Cloud ADF→Markdown, Server는 plain 그대로).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraIssue {
    /// 내부 id(숫자 문자열). 없으면 key로 폴백.
    pub id: String,
    /// 사람이 읽는 키(예: `ENG-123`).
    pub key: String,
    pub title: String,
    /// 본문(Markdown). ADF Cloud 바디는 변환됨(deferred 노드는 [`super::adf`] 참고).
    pub description: String,
    /// `{site_url}/browse/{key}` 딥링크.
    pub url: String,
    /// 프로젝트 키(예: `ENG`).
    pub project_key: Option<String>,
    /// 이슈 타입 이름(예: `Task`).
    pub issue_type: Option<String>,
    /// 상태 이름(예: `In Progress`).
    pub status: Option<String>,
    /// 담당자 표시 이름.
    pub assignee: Option<String>,
    pub labels: Vec<String>,
}

/// 이슈 코멘트 하나. `body`는 이미 Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraComment {
    pub id: String,
    pub body: String,
    /// 작성자 표시 이름.
    pub author: Option<String>,
    /// 생성 시각(원문 그대로).
    pub created_at: Option<String>,
}

/// 프로젝트 하나(피커용). Orca `mapProject`(`issues.ts:208-217`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
}

/// 페이지네이션 결과 한 장. `has_more`는 **무성 절단 금지**(회귀 메모리)를 위한 truncation 신호 —
/// limit(cap)에 걸렸거나 `isLast=false`/`total` 미달로 더 있으면 true(UI "더 보기").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraPage<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}
