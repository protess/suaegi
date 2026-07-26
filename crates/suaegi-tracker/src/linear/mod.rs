//! Linear 이슈트래커 통합(§1). GraphQL-over-HTTP 읽기 클라이언트 + 분류 crux + 도메인 레코드.
//! N3a write-back([`write`])은 같은 GraphQL POST를 재사용한다 — writeId 멱등 + readback 확인 +
//! 4-way 분류. (N3b 에이전트 CLI-RPC 노출 + 티켓 런치는 후속.)

pub mod attribute_filter;
pub mod classify;
pub mod client;
pub mod model;
pub mod write;

pub use attribute_filter::{
    canonicalize_linear_issue_attribute_filter, is_empty_linear_issue_attribute_filter,
    linear_issue_attribute_filter_signature, optional_parsed_linear_issue_attribute_filter,
    parse_linear_issue_attribute_filter, LinearIssueAttributeAssignee, LinearIssueAttributeFilter,
    LinearIssueAttributeFilterError, LINEAR_ISSUE_ATTRIBUTE_FILTER_ID_MAX_LENGTH,
    LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_LABEL_IDS, LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_PRIORITIES,
    LINEAR_ISSUE_ATTRIBUTE_FILTER_MAX_STATE_IDS,
};
pub use classify::{classify_graphql, GraphqlOutcome};
pub use client::{LinearClient, KEYCHAIN_SERVICE, LINEAR_ENDPOINT};
pub use model::{
    Classified, Comment, Issue, IssuePage, LinearWorkspace, Lookup, TrackerUnavailable,
};
pub use write::{
    CreatedAttachment, CreatedComment, InvalidWriteId, IssueUpdate, NewIssue, WriteId, WriteOutcome,
};
