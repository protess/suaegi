//! suaegi-tracker — 이슈트래커 통합. N1 **Linear 읽기 + 워크트리 링크**, N2 **Jira 읽기**(REST),
//! N3a **Linear write-back**([`linear::write`] — writeId 멱등 + readback 확인 + 4-way 분류; N3b
//! 에이전트 CLI-RPC 노출은 후속). PR forge(`suaegi-forge`)가 아니라 `suaegi-http`(§Q5 추출된
//! leaf 전송)에 의존한다 — 이슈트래커→PR-forge 레이어링 스멜을 피한다.
//!
//! 핵심 규율(forge와 공유): **일시 실패(transient)를 절대 None/empty로 오독하지 않는다** —
//! 캐시-오염 방지. 결과 shape([`Lookup`]/[`TrackerUnavailable`]/[`Classified`])는 Linear·Jira
//! 공용이라 [`common`]에 산다. 분류 계약은 [`linear::classify`](GraphQL)·[`jira::classify`](REST).

pub mod common;
pub mod jira;
pub mod linear;
pub mod link;

pub use common::{Classified, Lookup, TrackerUnavailable};
pub use jira::{
    JiraAuthType, JiraClient, JiraComment, JiraConnection, JiraIssue, JiraIssueFilter, JiraPage,
    JiraProject, JiraViewer,
};
pub use linear::{
    canonicalize_linear_issue_attribute_filter, is_empty_linear_issue_attribute_filter,
    linear_issue_attribute_filter_signature, optional_parsed_linear_issue_attribute_filter,
    parse_linear_issue_attribute_filter, Comment, CreatedAttachment, CreatedComment,
    InvalidWriteId, Issue, IssuePage, IssueUpdate, LinearClient, LinearIssueAttributeAssignee,
    LinearIssueAttributeFilter, LinearIssueAttributeFilterError, LinearIssueContextOptions,
    LinearLabel, LinearMember, LinearProject, LinearRelation, LinearRelationPage, LinearState,
    LinearTeam, LinearWorkspace, NewIssue, WriteId, WriteOutcome,
    LINEAR_ISSUE_ATTRIBUTE_FILTER_ID_MAX_LENGTH, LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_LABEL_IDS,
    LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_PRIORITIES, LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_STATE_IDS,
};
pub use link::{
    resolve_current_issue, resolve_current_jira_issue, LinkedJiraIssue, LinkedLinearIssue,
};
