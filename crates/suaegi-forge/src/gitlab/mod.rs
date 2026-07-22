//! 7c — GitLab MR 지원을 `glab` CLI shell-out으로. `GhForge`(GitHub)와 **같은
//! `ForgeProvider`/`PrActions` 트레잇 뒤**에 들어오는 두 번째 impl이다. 목적은 GitLab이
//! GitHub과 **동일한 found/none/unavailable·확정거부/일시실패 보장**을 받는 것이다 — 그래서
//! 새 추상화를 만들지 않고 gh 조각(provider·pr_actions·classify 규율)을 그대로 미러한다.
//!
//! glab 커맨드 모양은 Orca `src/main/gitlab/`(`client.ts`, `merge-request-creation.ts`)를
//! 미러한다. glab의 stderr/exit 신호는 gh와 다르므로 분류는 GitLab 실제 신호로 하되
//! (`classify.rs`), **일시 실패가 확정적 부정을 날조하지 않는다**는 불변은 그대로 지킨다.

pub mod classify;
pub mod eligibility;
pub mod forge;
pub mod parse;
pub mod runner;

pub use eligibility::glab_creation_eligibility;
pub use forge::{glab_preflight, GlabForge, GlabPreflight, MIN_GLAB_VERSION};
pub use runner::{GlabError, GlabOutput, GlabRunner};
