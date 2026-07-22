//! suaegi-forge — GitHub/GitLab 등 forge와의 통신을 `ForgeProvider` 트레잇 뒤에 둔다.
//! 7a-1은 gh CLI shell-out(`GhForge`)만 구현한다; 7a-2(HTTP+시크릿), 7c(GitLab)가 뒤따른다.

pub mod classify;
pub mod eligibility;
pub mod github;
pub mod parse;
pub mod provider;
pub mod runner;

pub use eligibility::{creation_eligibility, CreationBlockedReason, CreationEligibility};
pub use github::{preflight, GhForge, Preflight, MIN_GH_VERSION};
pub use provider::{
    ChecksSummary, CreateReviewInput, ForgeError, ForgeProvider, ForgeUnavailable, RepoCoords,
    Review, ReviewLookup, ReviewState,
};
pub use runner::{GhError, GhOutput, GhRunner};
