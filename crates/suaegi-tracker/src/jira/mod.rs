//! Jira 이슈트래커 통합(§2). REST 읽기 클라이언트 + 상태코드 분류 + ADF→Markdown 변환.
//! write-back은 Orca에 없어(§0.1) N2 스코프 밖 — **읽기만**.

pub mod adf;
pub mod classify;
pub mod client;
pub mod model;

pub use adf::adf_to_markdown;
pub use classify::{classify_jira_status, is_rate_limited, JiraStatus};
pub use client::{build_auth_header, JiraClient, JiraIssueFilter, KEYCHAIN_SERVICE};
pub use model::{
    JiraAuthType, JiraComment, JiraConnection, JiraIssue, JiraPage, JiraProject, JiraViewer,
};
