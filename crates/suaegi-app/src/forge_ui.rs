//! Plan 7a-1: GitHub PR 상태·생성의 **순수 로직**. iced를 의존하지 않는다 —
//! `()` 렌더러 아래에서 그림은 단언할 수 없으므로, 검사 가능한 결정
//! (ReviewLookup→표시자, 자격→어포던스, 에러→문구)만 여기 값으로 뽑는다.
//! 픽셀·상호작용은 사이드바에 남고 사람 눈으로 본다.

use suaegi_forge::{
    ChecksSummary, CommentLookup, CreationBlockedReason, CreationEligibility, ForgeError,
    ForgeUnavailable, MergeOutcome, MergeRejection, MergeabilityState, PrReview, PrReviewState,
    ReviewLookup, ReviewState, ReviewThreadLookup,
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

// ================= Plan 7b: PR 패널 순수 로직 =================
// 7a와 같은 규율을 UI 값에서도 지킨다: 백엔드가 애써 보존한 구별을 여기서 접으면
// 안 된다 — Unavailable≠none(리뷰·코멘트), Rejected≠일시실패(머지 결과),
// Mergeable만 Enabled(파괴적 버튼). `()` 렌더러 아래 그림은 단언할 수 없으므로
// 검사 가능한 결정만 여기 값으로 뽑고, 픽셀은 `pr_panel`에 남겨 사람 눈으로 본다.

/// 패널이 **한 번의 조회로** 받는 PR 세부 3종. 하나의 op/staleness 게이트로 다룬다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDetails {
    pub mergeability: MergeabilityState,
    pub reviews: ReviewThreadLookup,
    pub comments: CommentLookup,
}

/// Merge 버튼의 어포던스. **`Enabled`는 오직 머지가능성이 `Mergeable`일 때만** 나온다
/// (§4.6, brief). 그 밖은 죽은 버튼이 아니라 **이유를 단** `Disabled`다 — 파괴적
/// 머지가 Blocked/Conflicting/Unknown에서 눌리면 안 된다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeButton {
    Enabled,
    Disabled(String),
}

/// 머지가능성 → 버튼 어포던스. **이 매핑이 load-bearing이다**: `Mergeable`이 아닌
/// 어떤 상태를 `Enabled`로 접는 뮤턴트든 §5 테스트를 깨야 한다. 특히 `Unknown`
/// (일시 실패·불완전 메타데이터의 흡수 상태)은 절대 Enabled가 아니다.
pub fn merge_button(mergeability: MergeabilityState) -> MergeButton {
    match mergeability {
        MergeabilityState::Mergeable => MergeButton::Enabled,
        MergeabilityState::Blocked => {
            MergeButton::Disabled("blocked — needs approvals or passing checks".to_string())
        }
        MergeabilityState::Conflicting => {
            MergeButton::Disabled("conflicts with the base branch".to_string())
        }
        // 일시 실패·불완전 메타데이터 → 절대 Enabled가 아니다.
        MergeabilityState::Unknown => {
            MergeButton::Disabled("mergeability unknown — refresh to retry".to_string())
        }
    }
}

/// 리뷰 요약 표시. **`Summary`(빈 = 진짜 리뷰 없음)와 `Unavailable`(일시 실패)을
/// 구별**한다 — 조회 실패가 "리뷰 없음"으로 렌더되면 안 된다(캐시-오염의 UI 계약,
/// 7a `Unavailable`≠`NoPr`와 같은 규율).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewsLine {
    Summary(String),
    Unavailable(String),
}

pub fn reviews_line(lookup: &ReviewThreadLookup) -> ReviewsLine {
    match lookup {
        ReviewThreadLookup::Found(reviews) => ReviewsLine::Summary(summarize_reviews(reviews)),
        // 일시 실패 → "리뷰 없음"이 아니라 재시도 안내.
        ReviewThreadLookup::Unavailable(u) => {
            ReviewsLine::Unavailable(format!("reviews unavailable — {}", unavailable_text(u)))
        }
    }
}

/// 리뷰 벡터 → 한 줄 요약. **빈 벡터는 "no reviews yet"**(진짜 없음) — 절대 실패
/// 문구가 아니다.
fn summarize_reviews(reviews: &[PrReview]) -> String {
    if reviews.is_empty() {
        return "no reviews yet".to_string();
    }
    let mut approved = 0usize;
    let mut changes = 0usize;
    let mut commented = 0usize;
    for r in reviews {
        match r.state {
            PrReviewState::Approved => approved += 1,
            PrReviewState::ChangesRequested => changes += 1,
            _ => commented += 1,
        }
    }
    format!("{approved} approved · {changes} changes requested · {commented} commented")
}

/// 코멘트 요약 표시. 리뷰와 같은 규율 — 일시 실패는 빈 요약("no comments")이 아니라
/// `Unavailable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentsLine {
    Summary(String),
    Unavailable(String),
}

pub fn comments_line(lookup: &CommentLookup) -> CommentsLine {
    match lookup {
        CommentLookup::Found(comments) => {
            let n = comments.len();
            CommentsLine::Summary(match n {
                0 => "no comments".to_string(),
                1 => "1 comment".to_string(),
                _ => format!("{n} comments"),
            })
        }
        CommentLookup::Unavailable(u) => {
            CommentsLine::Unavailable(format!("comments unavailable — {}", unavailable_text(u)))
        }
    }
}

/// merge 호출 결과의 표시. **세 갈래가 서로 다르고, 실패 둘 다 성공으로 안 읽힌다**:
/// `Merged`(성공) / `Rejected`(확정적 거부, 분류된 사유) / `Unavailable`(일시 실패, 재시도).
/// brief §2: `Rejected`를 성공으로, 일시 실패를 `Rejected`로 접으면 안 된다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResultDisplay {
    Merged,
    Rejected(String),
    Unavailable(String),
}

pub fn merge_result_display(result: Result<MergeOutcome, ForgeError>) -> MergeResultDisplay {
    match result {
        Ok(MergeOutcome::Merged) => MergeResultDisplay::Merged,
        // 확정적 거부 — 분류된 사유(raw stderr 아님).
        Ok(MergeOutcome::Rejected(reason)) => MergeResultDisplay::Rejected(rejection_text(reason)),
        // 일시 실패 → "거부됨"이 아니라 재시도 안내.
        Err(ForgeError::Unavailable(u)) => {
            MergeResultDisplay::Unavailable(format!("{} — retry", unavailable_text(&u)))
        }
        // Validation/Parse는 merge_pr가 실제로는 내지 않지만(백엔드는 실패를 Unavailable로만
        // 접는다), 방어적으로 **성공도 확정 거부도 아닌** 실패로 표시한다.
        Err(ForgeError::Validation(m)) | Err(ForgeError::Parse(m)) => {
            MergeResultDisplay::Unavailable(m)
        }
    }
}

/// 확정적 거부 사유 → 사용자 문구. raw stderr가 아니라 구조화 라벨을 번역한다.
pub fn rejection_text(reason: MergeRejection) -> String {
    match reason {
        MergeRejection::NotMergeable => "GitHub reports this PR is not mergeable.".to_string(),
        MergeRejection::Conflict => "Merge conflict with the base branch.".to_string(),
        MergeRejection::Blocked => {
            "Blocked by branch protection, required checks, or reviews.".to_string()
        }
        MergeRejection::ChangesRequested => "A review requested changes.".to_string(),
        MergeRejection::PermissionDenied => "You do not have permission to merge.".to_string(),
        MergeRejection::AlreadyClosed => "This PR is already merged or closed.".to_string(),
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

    // ---- Plan 7b ----

    fn review(state: PrReviewState) -> PrReview {
        PrReview {
            author: "a".to_string(),
            state,
            body: String::new(),
            submitted_at: String::new(),
        }
    }

    fn comment() -> suaegi_forge::PrComment {
        suaegi_forge::PrComment {
            author: "a".to_string(),
            body: String::new(),
            created_at: String::new(),
            url: String::new(),
        }
    }

    /// **§5 mutation (a): Merge 버튼은 오직 `Mergeable`일 때만 Enabled.**
    /// Unknown/Blocked/Conflicting에서 Enabled로 접는 뮤턴트는 이 테스트를 깨야 한다 —
    /// 파괴적 머지가 확인되지 않은 상태에서 눌리는 것을 막는 유일한 게이트다.
    #[test]
    fn merge_is_enabled_only_when_mergeable() {
        assert_eq!(
            merge_button(MergeabilityState::Mergeable),
            MergeButton::Enabled
        );
        for state in [
            MergeabilityState::Blocked,
            MergeabilityState::Conflicting,
            MergeabilityState::Unknown,
        ] {
            match merge_button(state) {
                MergeButton::Disabled(reason) => assert!(
                    !reason.is_empty(),
                    "{state:?} must give a reason, not a dead button"
                ),
                MergeButton::Enabled => {
                    panic!("{state:?} must NOT enable the destructive merge button")
                }
            }
        }
    }

    /// **§5 mutation (d): 일시 실패한 리뷰 조회는 절대 "리뷰 없음"으로 렌더되지
    /// 않는다.** `Found(빈)`과 `Unavailable`은 다른 변형이고 문구도 다르며, Unavailable
    /// 문구에는 "no review"가 없다. Unavailable→Summary로 접는 뮤턴트는 깨진다.
    #[test]
    fn an_unavailable_review_lookup_never_reads_as_no_reviews() {
        let none = reviews_line(&ReviewThreadLookup::Found(vec![]));
        let unavailable = reviews_line(&ReviewThreadLookup::Unavailable(ForgeUnavailable::Network));
        assert!(matches!(none, ReviewsLine::Summary(_)));
        assert!(matches!(unavailable, ReviewsLine::Unavailable(_)));
        assert_ne!(none, unavailable);
        if let ReviewsLine::Unavailable(text) = &unavailable {
            assert!(
                !text.to_lowercase().contains("no review"),
                "a failed lookup must not read as 'no reviews': {text}"
            );
        }
    }

    /// 실제 리뷰가 있으면 승인/변경요청 카운트를 요약한다(빈 요약과 구별된다).
    #[test]
    fn a_review_summary_counts_approvals_and_change_requests() {
        let line = reviews_line(&ReviewThreadLookup::Found(vec![
            review(PrReviewState::Approved),
            review(PrReviewState::ChangesRequested),
            review(PrReviewState::Commented),
        ]));
        match line {
            ReviewsLine::Summary(s) => {
                assert!(s.contains("1 approved"), "{s}");
                assert!(s.contains("1 changes requested"), "{s}");
            }
            other => panic!("found reviews must summarize, got {other:?}"),
        }
    }

    /// 코멘트도 같은 규율 — 일시 실패는 "no comments"가 아니라 Unavailable.
    #[test]
    fn an_unavailable_comment_lookup_never_reads_as_no_comments() {
        let none = comments_line(&CommentLookup::Found(vec![]));
        let some = comments_line(&CommentLookup::Found(vec![comment(), comment()]));
        let unavailable = comments_line(&CommentLookup::Unavailable(ForgeUnavailable::RateLimited));
        assert!(matches!(none, CommentsLine::Summary(_)));
        assert!(matches!(unavailable, CommentsLine::Unavailable(_)));
        assert_ne!(none, unavailable);
        if let CommentsLine::Summary(s) = &some {
            assert!(s.contains('2'), "counts real comments: {s}");
        }
        if let CommentsLine::Unavailable(text) = &unavailable {
            assert!(!text.to_lowercase().contains("no comment"), "{text}");
        }
    }

    /// **§5 mutation (c): `Rejected`와 `Unavailable`은 서로 다르고, 둘 다 성공
    /// (`Merged`)으로 안 읽힌다.** 확정 거부를 Merged로, 일시 실패를 Rejected로 접는
    /// 뮤턴트는 이 테스트를 깨야 한다.
    #[test]
    fn merge_rejection_and_unavailable_are_distinct_and_neither_is_success() {
        let merged = merge_result_display(Ok(MergeOutcome::Merged));
        let rejected = merge_result_display(Ok(MergeOutcome::Rejected(MergeRejection::Conflict)));
        let unavailable =
            merge_result_display(Err(ForgeError::Unavailable(ForgeUnavailable::RateLimited)));

        assert_eq!(merged, MergeResultDisplay::Merged);
        assert!(matches!(rejected, MergeResultDisplay::Rejected(_)));
        assert!(matches!(unavailable, MergeResultDisplay::Unavailable(_)));
        assert_ne!(rejected, unavailable);
        assert_ne!(rejected, merged);
        assert_ne!(unavailable, merged);
        for d in [&rejected, &unavailable] {
            assert!(
                !matches!(d, MergeResultDisplay::Merged),
                "a failure must never read as a successful merge: {d:?}"
            );
        }
    }

    /// 확정 거부 사유는 서로 다른 문구로 번역된다 — 라벨이 뭉개지면 사용자가 무엇을
    /// 고쳐야 하는지 모른다.
    #[test]
    fn each_rejection_reason_has_its_own_text() {
        let reasons = [
            MergeRejection::NotMergeable,
            MergeRejection::Conflict,
            MergeRejection::Blocked,
            MergeRejection::ChangesRequested,
            MergeRejection::PermissionDenied,
            MergeRejection::AlreadyClosed,
        ];
        let texts: Vec<String> = reasons.iter().map(|r| rejection_text(*r)).collect();
        for (i, a) in texts.iter().enumerate() {
            for (j, b) in texts.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "rejection reasons {i},{j} share text");
                }
            }
        }
    }
}
