//! suaegi-tracker — 이슈트래커 통합. N1 **Linear 읽기 + 워크트리 링크**, N2 **Jira 읽기**(REST).
//! (N3 Linear write-back은 후속). PR forge(`suaegi-forge`)가 아니라 `suaegi-http`(§Q5 추출된
//! leaf 전송)에 의존한다 — 이슈트래커→PR-forge 레이어링 스멜을 피한다.
//!
//! 핵심 규율(forge와 공유): **일시 실패(transient)를 절대 None/empty로 오독하지 않는다** —
//! 캐시-오염 방지. 결과 shape([`Lookup`]/[`TrackerUnavailable`]/[`Classified`])는 Linear·Jira
//! 공용이라 [`common`]에 산다. 분류 계약은 [`linear::classify`](GraphQL)·[`jira::classify`](REST).

pub mod common;
pub mod jira;
pub mod link;
pub mod linear;

pub use common::{Classified, Lookup, TrackerUnavailable};
pub use jira::{
    JiraAuthType, JiraClient, JiraComment, JiraConnection, JiraIssue, JiraIssueFilter, JiraPage,
    JiraProject, JiraViewer,
};
pub use link::{
    resolve_current_issue, resolve_current_jira_issue, LinkedJiraIssue, LinkedLinearIssue,
};
pub use linear::{Comment, Issue, IssuePage, LinearClient, LinearWorkspace};
