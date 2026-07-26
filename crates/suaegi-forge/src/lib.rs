//! suaegi-forge — GitHub/GitLab 등 forge와의 통신을 `ForgeProvider` 트레잇 뒤에 둔다.
//! 7a-1은 gh CLI shell-out(`GhForge`)만 구현한다; 7a-2(HTTP+시크릿), 7c(GitLab)가 뒤따른다.

pub mod any;
pub mod classify;
pub mod eligibility;
pub mod github;
pub mod github_http;
pub mod github_identity;
pub mod gitlab;
pub mod hosted_review;
pub mod hosted_review_github;
pub mod hosted_review_gitlab;
pub mod hosted_review_queue;
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
pub use github_identity::{github_repo_identity_key, is_default_github_host};
pub use gitlab::{
    glab_creation_eligibility, glab_preflight, GlabError, GlabForge, GlabOutput, GlabPreflight,
    GlabRunner, MIN_GLAB_VERSION,
};
pub use hosted_review::{
    hosted_review_identity_key, is_positive_hosted_review_number, CheckStatus,
    CreateHostedReviewArgs, CreateHostedReviewErrorCode, CreateHostedReviewInput,
    CreateHostedReviewResult, HostedReviewCreationBlockedReason, HostedReviewCreationEligibility,
    HostedReviewCreationNextAction, HostedReviewDecision, HostedReviewForBranchArgs,
    HostedReviewIdentity, HostedReviewInfo, HostedReviewLookupOutcome, HostedReviewProvider,
    HostedReviewQueueClassification, HostedReviewQueueKey, HostedReviewQueueState,
    HostedReviewState, HostedReviewSummary, HostedReviewThreadDataCompleteness,
    HostedReviewThreadSummary, HostedReviewUser, PrConflictLocalMergeState, PrConflictSummary,
    PrMergeableState, PrReviewDecisionAggregate, PrState,
};
pub use hosted_review_github::{
    hosted_review_info_from_github_pr_info, hosted_review_summary_from_github_pr_info,
    CheckConclusion, CheckRunStatus, GitHubPrInfo, HostedReviewCommentInput,
    HostedReviewFromGitHubPrInfoArgs, PrCheckDetail,
};
pub use hosted_review_gitlab::{
    hosted_review_summary_from_gitlab_info, GitLabReviewInfo, HostedReviewFromGitLabInfoArgs,
};
pub use hosted_review_queue::{
    classify_hosted_review, review_needs_response, review_ready_to_merge,
    HostedReviewClassificationOptions,
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
