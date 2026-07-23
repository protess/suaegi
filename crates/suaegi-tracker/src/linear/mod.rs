//! Linear 이슈트래커 통합(§1). GraphQL-over-HTTP 읽기 클라이언트 + 분류 crux + 도메인 레코드.
//! N3a write-back([`write`])은 같은 GraphQL POST를 재사용한다 — writeId 멱등 + readback 확인 +
//! 4-way 분류. (N3b 에이전트 CLI-RPC 노출 + 티켓 런치는 후속.)

pub mod classify;
pub mod client;
pub mod model;
pub mod write;

pub use classify::{classify_graphql, GraphqlOutcome};
pub use client::{LinearClient, KEYCHAIN_SERVICE, LINEAR_ENDPOINT};
pub use model::{
    Classified, Comment, Issue, IssuePage, LinearWorkspace, Lookup, TrackerUnavailable,
};
pub use write::{
    CreatedAttachment, CreatedComment, InvalidWriteId, IssueUpdate, NewIssue, WriteId, WriteOutcome,
};
