//! suaegi-forge — GitHub/GitLab 등 forge와의 통신을 `ForgeProvider` 트레잇 뒤에 둔다.
//! 7a-1은 gh CLI shell-out(`GhForge`)만 구현한다; 7a-2(HTTP+시크릿), 7c(GitLab)가 뒤따른다.

pub mod any;
pub mod classify;
pub mod eligibility;
pub mod github;
pub mod github_http;
pub mod gitlab;
pub mod parse;
pub mod pr_actions;
pub mod provider;
pub mod runner;

pub use any::AnyForge;
pub use eligibility::{creation_eligibility, CreationBlockedReason, CreationEligibility};
pub use github::{preflight, GhForge, Preflight, MIN_GH_VERSION};
pub use github_http::{
    choose_github_backend, http_creation_eligibility, GithubBackend, HttpGhForge, HttpTransport,
    ReqwestTransport,
};
pub use gitlab::{
    glab_creation_eligibility, glab_preflight, GlabError, GlabForge, GlabOutput, GlabPreflight,
    GlabRunner, MIN_GLAB_VERSION,
};
pub use pr_actions::{
    classify_merge_failure, mergeability_from_fields, CommentLookup, MergeFailure, MergeMethod,
    MergeOptions, MergeOutcome, MergeRejection, MergeabilityState, PrActions, PrComment, PrReview,
    PrReviewState, ReviewThreadLookup,
};
pub use provider::{
    ChecksSummary, CreateReviewInput, ForgeError, ForgeProvider, ForgeUnavailable, RepoCoords,
    Review, ReviewLookup, ReviewState,
};
pub use runner::{GhError, GhOutput, GhRunner};
