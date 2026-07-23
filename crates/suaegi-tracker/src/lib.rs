//! suaegi-tracker — 이슈트래커 통합. N1은 **Linear 읽기 + 워크트리 링크**만 구현한다
//! (N2 Jira, N3 Linear write-back은 후속). PR forge(`suaegi-forge`)가 아니라 `suaegi-http`
//! (§Q5 추출된 leaf 전송)에 의존한다 — 이슈트래커→PR-forge 레이어링 스멜을 피한다.
//!
//! 핵심 규율(forge와 공유): **일시 실패(transient)를 절대 None/empty로 오독하지 않는다** —
//! 캐시-오염 방지. 자세한 계약은 [`linear::classify`].

pub mod link;
pub mod linear;

pub use link::{resolve_current_issue, LinkedLinearIssue};
pub use linear::{
    Classified, Comment, Issue, IssuePage, LinearClient, LinearWorkspace, Lookup,
    TrackerUnavailable,
};
