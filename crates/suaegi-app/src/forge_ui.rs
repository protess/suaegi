//! Plan 7a-1: GitHub PR 상태·생성의 **순수 로직**. iced를 의존하지 않는다 —
//! `()` 렌더러 아래에서 그림은 단언할 수 없으므로, 검사 가능한 결정
//! (ReviewLookup→표시자, 자격→어포던스, 에러→문구)만 여기 값으로 뽑는다.
//! 픽셀·상호작용은 사이드바에 남고 사람 눈으로 본다.

use suaegi_forge::{
    ChecksSummary, CreationBlockedReason, CreationEligibility, ForgeError, ForgeUnavailable,
    ReviewLookup, ReviewState,
};

/// worktree 하나의 PR 상태 캐시. **on-activate 1회 + 수동 새로고침으로만** 채워진다
/// (배경 폴링 없음, `PresenceTick`에서 건드리지 않는다 — 플랜 §3.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubStatus {
    /// 조회가 진행 중.
    Checking,
    /// 조회가 끝났다. `fetch`가 표시자를, `eligibility`가 Create-PR 어포던스를 정한다.
    Fetched {
        fetch: GithubFetch,
        eligibility: CreationEligibility,
    },
}

/// resolve_repository + review 조회의 합성 결과. **`NotGitHub`(리포가 GitHub 아님)와
/// `Unavailable`(조회 실패)을 구별**한다 — 백엔드가 애써 보존한 구별을 UI가 지켜야 한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubFetch {
    /// GitHub 원격이 아니다 — 표시자도 어포던스도 없다.
    NotGitHub,
    /// resolve 단계가 실패(분류된 사유). 알려진 PR 상태를 지우지 않는다.
    Unavailable(ForgeUnavailable),
    /// review 조회까지 도달. `ReviewLookup`이 Found/None/Unavailable을 나른다.
    Resolved(ReviewLookup),
}

/// 사이드바가 그릴 PR 표시자. **`Unknown`은 `NoPr`와 절대 같은 변형이 아니다** —
/// 이것이 캐시-오염 구별의 UI 쪽 계약이고, [`indicator_for`]의 §5 mutation이 지킨다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrIndicator {
    /// 아직 조회 안 함(캐시 없음) 또는 GitHub 리포 아님 — 아무것도 안 그린다.
    Hidden,
    /// 조회 중.
    Checking,
    /// PR 없음(확정).
    NoPr,
    /// PR 있음.
    Present {
        number: u64,
        state: ReviewState,
        checks: ChecksSummary,
    },
    /// 조회 실패 — **상태 모름**. 절대 `NoPr`로 접지 않는다. 사유는 사이드바가
    /// 실행 가능한 힌트("gh auth login" 등)로 번역한다.
    Unknown(ForgeUnavailable),
}

/// 캐시 → 표시자. **이 매핑이 `Unavailable`을 `NoPr`로 접으면 안 된다**(§5 mutation).
pub fn indicator_for(status: Option<&GithubStatus>) -> PrIndicator {
    match status {
        None => PrIndicator::Hidden,
        Some(GithubStatus::Checking) => PrIndicator::Checking,
        Some(GithubStatus::Fetched { fetch, .. }) => match fetch {
            GithubFetch::NotGitHub => PrIndicator::Hidden,
            // resolve 단계 실패도 "모름"이다 — "PR 없음"이 아니다.
            GithubFetch::Unavailable(u) => PrIndicator::Unknown(u.clone()),
            GithubFetch::Resolved(ReviewLookup::Found(r)) => PrIndicator::Present {
                number: r.number,
                state: r.state,
                checks: r.checks,
            },
            GithubFetch::Resolved(ReviewLookup::None) => PrIndicator::NoPr,
            // review 조회 실패도 "모름"이다 — 알려진 PR을 지우지 않는다.
            GithubFetch::Resolved(ReviewLookup::Unavailable(u)) => PrIndicator::Unknown(u.clone()),
        },
    }
}

/// Create-PR 어포던스. 자격이 있을 때만 버튼을 제안하고, 막혔으면 **죽은 버튼 대신
/// 이유**를 보여준다. 이미 PR이 있거나 GitHub 리포가 아니면 숨긴다(표시자가 대신 말한다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatePrAffordance {
    /// 자격 있음 → "Create PR" 버튼.
    Offer,
    /// 막힘 → 이유 문구(버튼 없음).
    Blocked(String),
    /// 어포던스 자체를 숨김.
    Hidden,
}

/// 캐시 → Create-PR 어포던스. **`Offer`는 오직 `Eligible`일 때만 나온다**(§5 mutation
/// (b)): 막힌 사유를 `Offer`로 바꾸는 뮤턴트는 테스트를 깨야 한다.
pub fn create_pr_affordance(status: Option<&GithubStatus>) -> CreatePrAffordance {
    // 조회 전(캐시 없음/Checking)에는 자격을 모른다 — 죽은 버튼을 띄우지 않는다.
    let Some(GithubStatus::Fetched { eligibility, .. }) = status else {
        return CreatePrAffordance::Hidden;
    };
    match eligibility {
        CreationEligibility::Eligible => CreatePrAffordance::Offer,
        CreationEligibility::Blocked(reason) => match reason {
            // 표시자가 이미 "PR 있음"/아무것도-아님을 말한다 — 어포던스는 숨긴다.
            CreationBlockedReason::NotGitHubRepo | CreationBlockedReason::AlreadyExists => {
                CreatePrAffordance::Hidden
            }
            // 브랜치가 push 안 됨 → 죽은 버튼이 아니라 "push" 유도.
            CreationBlockedReason::NoUpstream => {
                CreatePrAffordance::Blocked("push the branch first".to_string())
            }
            CreationBlockedReason::NotInstalled => {
                CreatePrAffordance::Blocked("gh not installed".to_string())
            }
            CreationBlockedReason::NotAuthenticated => {
                CreatePrAffordance::Blocked("run gh auth login".to_string())
            }
            CreationBlockedReason::OutdatedGh { found, min } => {
                CreatePrAffordance::Blocked(format!("update gh (found {found}, need {min})"))
            }
            // 자격 확정 실패(재시도 가능) — AlreadyExists로 뭉개지 않는다.
            CreationBlockedReason::Unavailable(_) => {
                CreatePrAffordance::Blocked("GitHub unavailable — refresh to retry".to_string())
            }
        },
    }
}

/// 생성 실패의 **분류된** 사용자 문구. raw stderr를 절대 노출하지 않는다.
/// `ForgeError::Unavailable(_)`의 기본 Display는 `{:?}`라 사람 문장으로 다시 쓴다.
pub fn create_error_text(error: ForgeError) -> String {
    match error {
        ForgeError::Validation(m) => m,
        ForgeError::Parse(m) => m,
        ForgeError::Unavailable(u) => unavailable_text(&u),
    }
}

/// 분류된 조회-불가 사유 → 실행 가능한 힌트. 표시자 `Unknown`과 생성 에러가 공유한다.
pub fn unavailable_text(reason: &ForgeUnavailable) -> String {
    match reason {
        ForgeUnavailable::NotInstalled => "gh is not installed".to_string(),
        ForgeUnavailable::NotAuthenticated => "run gh auth login".to_string(),
        ForgeUnavailable::RateLimited => "GitHub rate limit — try again later".to_string(),
        ForgeUnavailable::Network => "network error reaching GitHub".to_string(),
        ForgeUnavailable::Other(m) => m.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_forge::Review;

    fn found(number: u64) -> ReviewLookup {
        ReviewLookup::Found(Review {
            number,
            state: ReviewState::Open,
            title: "t".to_string(),
            url: "u".to_string(),
            checks: ChecksSummary::default(),
        })
    }

    fn fetched(fetch: GithubFetch, eligibility: CreationEligibility) -> GithubStatus {
        GithubStatus::Fetched { fetch, eligibility }
    }

    /// **§5 mutation (a): `Unavailable`은 `None`과 같은 표시자로 접히면 안 된다.**
    /// `indicator_for`가 둘을 같은 변형으로 매핑하도록 바꾸는 뮤턴트는 이 테스트를
    /// 깨야 한다 — 백엔드가 보존한 캐시-오염 구별의 UI 쪽 계약이다.
    #[test]
    fn an_unavailable_lookup_never_renders_as_no_pr() {
        let none = indicator_for(Some(&fetched(
            GithubFetch::Resolved(ReviewLookup::None),
            CreationEligibility::Eligible,
        )));
        let review_unavailable = indicator_for(Some(&fetched(
            GithubFetch::Resolved(ReviewLookup::Unavailable(ForgeUnavailable::Network)),
            CreationEligibility::Eligible,
        )));
        let resolve_unavailable = indicator_for(Some(&fetched(
            GithubFetch::Unavailable(ForgeUnavailable::NotAuthenticated),
            CreationEligibility::Blocked(CreationBlockedReason::NotAuthenticated),
        )));

        assert_eq!(none, PrIndicator::NoPr);
        assert!(
            !matches!(review_unavailable, PrIndicator::NoPr),
            "a failed review lookup must never look like 'no PR' — that would erase a known PR"
        );
        assert!(
            !matches!(resolve_unavailable, PrIndicator::NoPr),
            "a failed resolve must never look like 'no PR'"
        );
        assert_ne!(
            none, review_unavailable,
            "'no PR' and 'status unknown' must be distinct indicators"
        );
        assert_ne!(none, resolve_unavailable);
    }

    #[test]
    fn a_found_pr_surfaces_its_number_state_and_checks() {
        let ind = indicator_for(Some(&fetched(
            GithubFetch::Resolved(found(42)),
            CreationEligibility::Blocked(CreationBlockedReason::AlreadyExists),
        )));
        match ind {
            PrIndicator::Present { number, state, .. } => {
                assert_eq!(number, 42);
                assert_eq!(state, ReviewState::Open);
            }
            other => panic!("a found PR must render as Present, got {other:?}"),
        }
    }

    #[test]
    fn nothing_is_shown_before_a_fetch_or_for_a_non_github_repo() {
        assert_eq!(indicator_for(None), PrIndicator::Hidden);
        assert_eq!(indicator_for(Some(&GithubStatus::Checking)), PrIndicator::Checking);
        assert_eq!(
            indicator_for(Some(&fetched(
                GithubFetch::NotGitHub,
                CreationEligibility::Blocked(CreationBlockedReason::NotGitHubRepo),
            ))),
            PrIndicator::Hidden
        );
    }

    /// **§5 mutation (b): Create-PR은 오직 자격이 있을 때만 제안된다.**
    /// 막힌 어떤 사유든 `Offer`로 바꾸는 뮤턴트는 이 테스트를 깨야 한다.
    #[test]
    fn create_pr_is_offered_only_when_eligible() {
        assert_eq!(
            create_pr_affordance(Some(&fetched(
                GithubFetch::Resolved(ReviewLookup::None),
                CreationEligibility::Eligible,
            ))),
            CreatePrAffordance::Offer
        );

        // 막힌 모든 사유 + 조회 전 상태는 절대 Offer가 아니다.
        let never_offer = [
            fetched(
                GithubFetch::Resolved(ReviewLookup::None),
                CreationEligibility::Blocked(CreationBlockedReason::NoUpstream),
            ),
            fetched(
                GithubFetch::Resolved(found(1)),
                CreationEligibility::Blocked(CreationBlockedReason::AlreadyExists),
            ),
            fetched(
                GithubFetch::Unavailable(ForgeUnavailable::NotAuthenticated),
                CreationEligibility::Blocked(CreationBlockedReason::NotAuthenticated),
            ),
            fetched(
                GithubFetch::NotGitHub,
                CreationEligibility::Blocked(CreationBlockedReason::NotGitHubRepo),
            ),
            fetched(
                GithubFetch::Resolved(ReviewLookup::None),
                CreationEligibility::Blocked(CreationBlockedReason::Unavailable(
                    ForgeUnavailable::Network,
                )),
            ),
            GithubStatus::Checking,
        ];
        for status in &never_offer {
            assert_ne!(
                create_pr_affordance(Some(status)),
                CreatePrAffordance::Offer,
                "Create-PR must not be offered for {status:?}"
            );
        }
        assert_ne!(create_pr_affordance(None), CreatePrAffordance::Offer);
    }

    /// 막힌 사유는 죽은 버튼이 아니라 실행 가능한 문구로 번역된다.
    #[test]
    fn a_blocked_no_upstream_tells_the_user_to_push() {
        let aff = create_pr_affordance(Some(&fetched(
            GithubFetch::Resolved(ReviewLookup::None),
            CreationEligibility::Blocked(CreationBlockedReason::NoUpstream),
        )));
        match aff {
            CreatePrAffordance::Blocked(msg) => assert!(msg.contains("push")),
            other => panic!("NoUpstream must surface a push hint, got {other:?}"),
        }
    }

    #[test]
    fn create_errors_are_classified_never_raw() {
        assert_eq!(
            create_error_text(ForgeError::Validation("base and title required".to_string())),
            "base and title required"
        );
        assert_eq!(
            create_error_text(ForgeError::Unavailable(ForgeUnavailable::NotAuthenticated)),
            "run gh auth login"
        );
        assert!(create_error_text(ForgeError::Unavailable(ForgeUnavailable::NotInstalled))
            .contains("not installed"));
    }
}
