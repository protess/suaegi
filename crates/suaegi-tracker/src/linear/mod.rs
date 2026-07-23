//! Linear 이슈트래커 통합(§1). GraphQL-over-HTTP 읽기 클라이언트 + 분류 crux + 도메인 레코드.
//! write-back(N3)은 여기 없다 — 읽기만.

pub mod classify;
pub mod client;
pub mod model;

pub use classify::{classify_graphql, GraphqlOutcome};
pub use client::{LinearClient, KEYCHAIN_SERVICE, LINEAR_ENDPOINT};
pub use model::{
    Classified, Comment, Issue, IssuePage, LinearWorkspace, Lookup, TrackerUnavailable,
};
