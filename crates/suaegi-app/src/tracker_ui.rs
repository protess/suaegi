//! Tracker(N1 Linear) UI의 **순수 로직**. `forge_ui.rs`와 같은 규율이다 — iced를
//! 의존하지 않고, `()` 렌더러 아래에서 단언 불가능한 픽셀 대신 검사 가능한 결정
//! (연결 결과→워크스페이스/에러, `Lookup`→목록 표시, 이슈→도메인 링크 필드)만 값으로 뽑는다.
//! 픽셀·상호작용은 `sidebar`에 남고 사람 눈으로 본다.
//!
//! **crux(forge와 공유): 일시 실패(`Unavailable`)를 절대 "없음"으로 접지 않는다.** 특히
//! [`issue_list`]는 `Unavailable`을 빈 목록("no issues")과 **다른 변형**으로 낸다 — 조회
//! 실패가 "이슈 없음"으로 렌더되면 안 된다(캐시-오염의 UI 계약, forge `Unavailable`≠`NoPr`).

use suaegi_tracker::{Classified, Issue, IssuePage, LinearWorkspace, Lookup, TrackerUnavailable};
use suaegi_tracker::{JiraIssue, JiraPage, JiraViewer, LinkedJiraIssue, LinkedLinearIssue};

/// 연결 다이얼로그가 그릴 결과. **`Connected`(워크스페이스 확정)와 `Failed`(분류된 사유)**를
/// 구별한다 — 실패는 raw 에러가 아니라 실행 가능한 문구다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectView {
    Connected(LinearWorkspace),
    Failed(String),
}

/// `test_connection` 결과 → 표시 값. 성공이면 워크스페이스를, 실패면 **분류된 문구**를 낸다
/// (API 키·raw 에러 바디는 절대 여기 안 온다 — [`unavailable_text`] 참고).
pub fn connect_view(lookup: &Lookup<LinearWorkspace>) -> ConnectView {
    match lookup {
        Lookup::Found(ws) => ConnectView::Connected(ws.clone()),
        // viewer 조회는 실제로 NotFound를 내지 않지만(성공 아니면 GraphQL 에러=Unavailable),
        // 방어적으로 "없음"도 실패 문구로 접는다 — 절대 성공(Connected)으로 읽지 않는다.
        Lookup::NotFound => ConnectView::Failed("Linear returned no workspace".to_string()),
        Lookup::Unavailable(c) => ConnectView::Failed(unavailable_text(c)),
    }
}

/// 사이드바가 그릴 이슈 목록 표시. **`Issues`(빈 = 진짜 "없음")와 `Unavailable`(일시 실패)을
/// 절대 뭉개지 않는다.** 이것이 crux의 UI 쪽 계약이고 [`issue_list`]의 mutation 테스트가 지킨다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueListView {
    /// 진짜 이슈 목록. 비어 있으면 "no issues"(진짜 없음). `has_more`는 bounded traversal이
    /// 절단됐음을 표면화한다(무성 절단 금지 — 백엔드가 준 신호를 UI가 지운다면 회귀).
    Issues { issues: Vec<Issue>, has_more: bool },
    /// 일시 실패 — **절대 "no issues"가 아니다.** 재시도 안내 문구를 나른다.
    Unavailable(String),
}

/// `list_issues` 결과 → 목록 표시. **`Unavailable`을 `Issues`(빈)로 접는 뮤턴트는 §mutation
/// (a) 테스트를 깨야 한다** — 조회 실패를 "이슈 없음"으로 렌더하면 안 된다.
pub fn issue_list(lookup: &Lookup<IssuePage>) -> IssueListView {
    match lookup {
        Lookup::Found(page) => IssueListView::Issues {
            issues: page.issues.clone(),
            has_more: page.has_more,
        },
        // 컬렉션 엔드포인트는 NotFound를 내지 않지만(클라이언트가 Unknown=Unavailable로 접는다),
        // 도착하면 빈 목록으로 본다 — 절대 Unavailable을 여기로 흘리지 않는다.
        Lookup::NotFound => IssueListView::Issues {
            issues: Vec::new(),
            has_more: false,
        },
        Lookup::Unavailable(c) => IssueListView::Unavailable(unavailable_text(c)),
    }
}

/// 이슈 + 연결된 워크스페이스 → 워크트리에 굳힐 **도메인 링크 필드**. 식별자(예: `ENG-123`)와
/// 워크스페이스 좌표(딥링크·재연결)를 담는다. 워크스페이스를 아직 모르면 좌표는 `None`(식별자만).
pub fn link_for(issue: &Issue, workspace: Option<&LinearWorkspace>) -> LinkedLinearIssue {
    LinkedLinearIssue {
        issue: issue.identifier.clone(),
        workspace_id: workspace.map(|w| w.id.clone()),
        organization_url_key: workspace.map(|w| w.url_key.clone()),
    }
}

/// 분류된 조회-불가 사유 → 실행 가능한 힌트. **API 키/raw 에러 바디는 절대 노출하지 않는다** —
/// Linear가 안전하다고 보장한 `user_message`(`userPresentableMessage`)가 있으면 그걸 쓰고,
/// 없으면 고정 라벨을 사람 문장으로 번역한다.
pub fn unavailable_text(reason: &Classified) -> String {
    // provider가 안전하다고 보장한 사용자용 문자열이 있으면 우선. 그마저도 raw 바디가 아니다.
    if let Some(msg) = &reason.user_message {
        return msg.clone();
    }
    match reason.kind {
        TrackerUnavailable::NotAuthenticated => "check your Linear API key".to_string(),
        TrackerUnavailable::RateLimited => "Linear rate limit — try again later".to_string(),
        TrackerUnavailable::Forbidden => {
            "your Linear key lacks access to this resource".to_string()
        }
        TrackerUnavailable::Network => "network error reaching Linear".to_string(),
        TrackerUnavailable::Internal => "Linear had an internal error — try again".to_string(),
        TrackerUnavailable::InvalidInput => "Linear rejected the request".to_string(),
        TrackerUnavailable::Unknown => "Linear returned an unexpected response".to_string(),
    }
}

// ============================ N2: Jira ============================
//
// Linear(N1)와 **같은 규율**을 Jira에도 적용한다: 순수·iced-free 매핑, `Unavailable`을 절대
// 빈 목록("no issues")으로 접지 않는다. 차이는 (1) 연결이 워크스페이스가 아니라 [`JiraViewer`]를
// 확정하고, (2) Jira의 링크 식별자가 더 단순(이슈 키 + 사이트)하며, (3) **Jira는 `user_message`를
// 채우지 않는다**(REST 에러 바디가 JQL 등 내부를 노출할 수 있어 고정 라벨만 쓴다, `common.rs`).

/// Jira 연결 다이얼로그가 그릴 결과. `ConnectView`의 Jira 짝 — 성공은 계정([`JiraViewer`]),
/// 실패는 **분류된 문구**(raw 에러/토큰 아님)를 낸다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JiraConnectView {
    Connected(JiraViewer),
    Failed(String),
}

/// `test_connection`(`/myself`) 결과 → 표시 값. 성공이면 계정을, 실패면 **분류된 문구**를 낸다.
/// `NotFound`는 test_connection이 내지 않지만(클라이언트가 `/myself` 404를 Unavailable로 접는다),
/// 방어적으로 실패 문구로 접는다 — 절대 성공(Connected)으로 읽지 않는다.
pub fn jira_connect_view(lookup: &Lookup<JiraViewer>) -> JiraConnectView {
    match lookup {
        Lookup::Found(viewer) => JiraConnectView::Connected(viewer.clone()),
        Lookup::NotFound => JiraConnectView::Failed("Jira returned no account".to_string()),
        Lookup::Unavailable(c) => JiraConnectView::Failed(jira_unavailable_text(c)),
    }
}

/// 사이드바가 그릴 Jira 이슈 목록 표시. `IssueListView`의 Jira 짝 — **`Issues`(빈 = 진짜 "없음")와
/// `Unavailable`(일시 실패)을 절대 뭉개지 않는다.** 이것이 crux의 UI 쪽 계약이고
/// [`jira_issue_list`]의 mutation 테스트가 지킨다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JiraIssueListView {
    /// 진짜 이슈 목록. 비어 있으면 "no issues"(진짜 없음). `has_more`는 절단을 표면화한다.
    Issues { issues: Vec<JiraIssue>, has_more: bool },
    /// 일시 실패 — **절대 "no issues"가 아니다.** 재시도 안내 문구를 나른다.
    Unavailable(String),
}

/// `list_issues` 결과 → 목록 표시. **`Unavailable`을 `Issues`(빈)로 접는 뮤턴트는 §mutation
/// (a) 테스트를 깨야 한다** — 조회 실패를 "이슈 없음"으로 렌더하면 안 된다(N1 `issue_list` 미러).
pub fn jira_issue_list(lookup: &Lookup<JiraPage<JiraIssue>>) -> JiraIssueListView {
    match lookup {
        Lookup::Found(page) => JiraIssueListView::Issues {
            issues: page.items.clone(),
            has_more: page.has_more,
        },
        // 컬렉션 엔드포인트는 NotFound를 내지 않지만(클라이언트가 404를 Unavailable로 접는다),
        // 도착하면 빈 목록으로 본다 — 절대 Unavailable을 여기로 흘리지 않는다.
        Lookup::NotFound => JiraIssueListView::Issues {
            issues: Vec::new(),
            has_more: false,
        },
        Lookup::Unavailable(c) => JiraIssueListView::Unavailable(jira_unavailable_text(c)),
    }
}

/// 이슈 + 연결된 사이트 → 워크트리에 굳힐 **도메인 링크 필드**. Linear의 세 조각 대신 두 조각:
/// 이슈 키(예: `PROJ-123`)와 사이트(딥링크·다중-사이트 구분). 사이트를 아직 모르면 `None`(키만).
pub fn jira_link_for(issue: &JiraIssue, site: Option<&str>) -> LinkedJiraIssue {
    LinkedJiraIssue {
        issue: issue.key.clone(),
        site: site.map(|s| s.to_string()),
    }
}

/// 분류된 조회-불가 사유 → 실행 가능한 힌트(Jira 문구). **`unavailable_text`(Linear)와 달리
/// `user_message`를 보지 않는다** — Jira는 그 필드를 채우지 않는다(REST 에러 바디 `errorMessages`가
/// JQL 등 내부를 노출할 수 있어 고정 라벨 `kind`만 안전하다, `common.rs`). raw 바디는 절대 안 온다.
pub fn jira_unavailable_text(reason: &Classified) -> String {
    match reason.kind {
        // 401만 크리덴셜 무효. 403은 Forbidden(권한 갭)이지 여기 아님.
        TrackerUnavailable::NotAuthenticated => "check your Jira email and API token".to_string(),
        TrackerUnavailable::RateLimited => "Jira rate limit — try again later".to_string(),
        TrackerUnavailable::Forbidden => {
            "your Jira account lacks access to this resource".to_string()
        }
        TrackerUnavailable::Network => "network error reaching Jira".to_string(),
        TrackerUnavailable::Internal => "Jira had an internal error — try again".to_string(),
        TrackerUnavailable::InvalidInput => "Jira rejected the request".to_string(),
        TrackerUnavailable::Unknown => "Jira returned an unexpected response".to_string(),
    }
}

/// 사용자가 입력한 사이트 URL을 정규화한다(끝 슬래시 없는 절대 URL — `JiraConnection`의 계약).
/// 공백을 다듬고 **끝 슬래시들**을 벗긴다. 스킴 보정은 하지 않는다(사용자가 `https://`를 준다고
/// 가정 — 잘못된 URL은 연결 시 `Unavailable`로 안전하게 떨어진다). 키체인 account이기도 하므로
/// 정규화가 일관돼야 저장·재로드가 같은 키를 짚는다.
pub fn normalize_site_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(identifier: &str) -> Issue {
        Issue {
            id: format!("id_{identifier}"),
            identifier: identifier.to_string(),
            title: "t".to_string(),
            description: None,
            url: None,
            state: Some("In Progress".to_string()),
            assignee: Some("Ada".to_string()),
        }
    }

    fn workspace() -> LinearWorkspace {
        LinearWorkspace {
            id: "org_1".to_string(),
            name: "Acme".to_string(),
            url_key: "acme".to_string(),
            viewer_email: "ada@acme.com".to_string(),
        }
    }

    /// **§mutation (a): `Unavailable`은 빈 목록("no issues")과 절대 같은 변형으로 접히지
    /// 않는다.** `issue_list`가 둘을 같은 변형으로 매핑하도록 바꾸는 뮤턴트는 이 테스트를
    /// 깨야 한다 — 백엔드가 애써 보존한 캐시-오염 구별의 UI 쪽 계약이다.
    #[test]
    fn an_unavailable_lookup_never_renders_as_no_issues() {
        let empty = issue_list(&Lookup::Found(IssuePage {
            issues: vec![],
            has_more: false,
        }));
        let unavailable = issue_list(&Lookup::Unavailable(Classified::new(
            TrackerUnavailable::RateLimited,
        )));

        // 빈 목록은 진짜 "없음" — Issues(빈)으로.
        assert!(
            matches!(&empty, IssueListView::Issues { issues, .. } if issues.is_empty()),
            "an empty Found must render as an empty issue list, not as unavailable"
        );
        // 일시 실패는 절대 Issues가 아니다(그랬다면 "no issues"로 렌더된다).
        assert!(
            matches!(unavailable, IssueListView::Unavailable(_)),
            "a failed lookup must never read as 'no issues' — that erases a transient failure"
        );
        assert_ne!(
            empty, unavailable,
            "'no issues' and 'unavailable' must be distinct displays"
        );
    }

    /// bounded traversal의 truncation 신호(`has_more`)는 UI 표시까지 살아 있어야 한다 —
    /// 여기서 지우면 무성 절단이 된다(회귀 메모리).
    #[test]
    fn a_found_page_surfaces_its_issues_and_has_more() {
        let view = issue_list(&Lookup::Found(IssuePage {
            issues: vec![issue("ENG-1"), issue("ENG-2")],
            has_more: true,
        }));
        match view {
            IssueListView::Issues { issues, has_more } => {
                assert_eq!(issues.len(), 2);
                assert_eq!(issues[0].identifier, "ENG-1");
                assert!(has_more, "truncation must be surfaced, not silently dropped");
            }
            other => panic!("a found page must render as Issues, got {other:?}"),
        }
    }

    /// 연결 성공은 워크스페이스를, 실패는 **분류된 문구**를 낸다 — 절대 성공으로 안 읽힌다.
    #[test]
    fn connect_success_and_failure_are_distinct_and_neither_leaks_raw() {
        let ok = connect_view(&Lookup::Found(workspace()));
        let bad = connect_view(&Lookup::Unavailable(Classified::new(
            TrackerUnavailable::NotAuthenticated,
        )));
        assert_eq!(ok, ConnectView::Connected(workspace()));
        match bad {
            ConnectView::Failed(msg) => {
                assert!(msg.contains("Linear API key"), "actionable hint, got {msg}");
                assert!(!msg.is_empty());
            }
            other => panic!("a failed connect must not read as Connected, got {other:?}"),
        }
    }

    /// provider가 담은 안전한 `user_message`가 있으면 그걸 우선한다(고정 라벨보다 구체적).
    #[test]
    fn a_user_presentable_message_is_preferred_over_the_fixed_label() {
        let c = Classified {
            kind: TrackerUnavailable::RateLimited,
            user_message: Some("You are being rate limited.".to_string()),
        };
        assert_eq!(unavailable_text(&c), "You are being rate limited.");
    }

    /// 링크는 식별자 + 워크스페이스 좌표 세 조각으로 굳는다. 워크스페이스를 모르면 좌표는 None.
    #[test]
    fn link_captures_identifier_and_workspace_coordinates() {
        let with_ws = link_for(&issue("ENG-9"), Some(&workspace()));
        assert_eq!(with_ws.issue, "ENG-9");
        assert_eq!(with_ws.workspace_id.as_deref(), Some("org_1"));
        assert_eq!(with_ws.organization_url_key.as_deref(), Some("acme"));

        let without_ws = link_for(&issue("ENG-9"), None);
        assert_eq!(without_ws.issue, "ENG-9");
        assert_eq!(without_ws.workspace_id, None);
        assert_eq!(without_ws.organization_url_key, None);
    }

    // ---------------------- N2: Jira ----------------------

    fn jira_issue(key: &str) -> JiraIssue {
        JiraIssue {
            id: format!("id_{key}"),
            key: key.to_string(),
            title: "t".to_string(),
            description: String::new(),
            url: format!("https://acme.atlassian.net/browse/{key}"),
            project_key: Some("PROJ".to_string()),
            issue_type: Some("Task".to_string()),
            status: Some("In Progress".to_string()),
            assignee: Some("Ada".to_string()),
            labels: vec![],
        }
    }

    fn viewer() -> JiraViewer {
        JiraViewer {
            account_id: "acc_1".to_string(),
            display_name: "Ada".to_string(),
            email: Some("ada@acme.com".to_string()),
        }
    }

    /// **§mutation (a) — Jira: `Unavailable`은 빈 목록("no issues")과 절대 같은 변형으로 접히지
    /// 않는다.** `jira_issue_list`가 둘을 같은 변형으로 매핑하도록 바꾸는 뮤턴트는 이 테스트를
    /// 깨야 한다 — 백엔드가 애써 보존한 캐시-오염 구별의 UI 쪽 계약이다(N1 미러).
    #[test]
    fn a_jira_unavailable_lookup_never_renders_as_no_issues() {
        let empty = jira_issue_list(&Lookup::Found(JiraPage {
            items: Vec::<JiraIssue>::new(),
            has_more: false,
        }));
        let unavailable = jira_issue_list(&Lookup::Unavailable(Classified::new(
            TrackerUnavailable::RateLimited,
        )));

        assert!(
            matches!(&empty, JiraIssueListView::Issues { issues, .. } if issues.is_empty()),
            "an empty Found must render as an empty issue list, not as unavailable"
        );
        assert!(
            matches!(unavailable, JiraIssueListView::Unavailable(_)),
            "a failed lookup must never read as 'no issues' — that erases a transient failure"
        );
        assert_ne!(
            empty, unavailable,
            "'no issues' and 'unavailable' must be distinct displays"
        );
    }

    /// bounded 검색의 truncation 신호(`has_more`)는 UI 표시까지 살아 있어야 한다(무성 절단 금지).
    #[test]
    fn a_found_jira_page_surfaces_its_issues_and_has_more() {
        let view = jira_issue_list(&Lookup::Found(JiraPage {
            items: vec![jira_issue("PROJ-1"), jira_issue("PROJ-2")],
            has_more: true,
        }));
        match view {
            JiraIssueListView::Issues { issues, has_more } => {
                assert_eq!(issues.len(), 2);
                assert_eq!(issues[0].key, "PROJ-1");
                assert!(has_more, "truncation must be surfaced, not silently dropped");
            }
            other => panic!("a found page must render as Issues, got {other:?}"),
        }
    }

    /// 연결 성공은 계정을, 실패는 **분류된 문구**를 낸다 — 절대 성공으로 안 읽힌다. 그리고
    /// 실패 문구는 raw 바디/토큰이 아니라 실행 가능한 힌트다.
    #[test]
    fn jira_connect_success_and_failure_are_distinct_and_neither_leaks_raw() {
        let ok = jira_connect_view(&Lookup::Found(viewer()));
        let bad = jira_connect_view(&Lookup::Unavailable(Classified::new(
            TrackerUnavailable::NotAuthenticated,
        )));
        assert_eq!(ok, JiraConnectView::Connected(viewer()));
        match bad {
            JiraConnectView::Failed(msg) => {
                assert!(msg.contains("Jira email and API token"), "actionable hint, got {msg}");
                assert!(!msg.is_empty());
            }
            other => panic!("a failed connect must not read as Connected, got {other:?}"),
        }
    }

    /// **Jira는 `user_message`를 쓰지 않는다** — 설령 Classified에 그 필드가 채워져 있어도(있을 리
    /// 없지만) `jira_unavailable_text`는 고정 라벨만 낸다. raw 에러 바디가 UI로 새는 경로가 없음을
    /// 못박는다(Linear의 `unavailable_text`와 의도적으로 다른 계약).
    #[test]
    fn jira_unavailable_text_ignores_user_message_and_uses_fixed_labels() {
        let c = Classified {
            kind: TrackerUnavailable::RateLimited,
            user_message: Some("raw JQL error: field 'xyz' does not exist".to_string()),
        };
        let text = jira_unavailable_text(&c);
        assert_eq!(text, "Jira rate limit — try again later");
        assert!(
            !text.contains("JQL") && !text.contains("xyz"),
            "a raw error body must never surface for Jira: {text}"
        );
    }

    /// 링크는 이슈 키 + 사이트 두 조각으로 굳는다. 사이트를 모르면 None(키만).
    #[test]
    fn jira_link_captures_key_and_site() {
        let with_site = jira_link_for(&jira_issue("PROJ-9"), Some("https://acme.atlassian.net"));
        assert_eq!(with_site.issue, "PROJ-9");
        assert_eq!(with_site.site.as_deref(), Some("https://acme.atlassian.net"));

        let without_site = jira_link_for(&jira_issue("PROJ-9"), None);
        assert_eq!(without_site.issue, "PROJ-9");
        assert_eq!(without_site.site, None);
    }

    /// 사이트 URL 정규화: 끝 슬래시/공백을 벗긴다. 키체인 account가 일관되도록(저장·재로드가
    /// 같은 키를 짚도록) 이 함수가 유일한 정규화 지점이다.
    #[test]
    fn normalize_site_url_strips_trailing_slashes_and_whitespace() {
        assert_eq!(
            normalize_site_url("  https://acme.atlassian.net/  "),
            "https://acme.atlassian.net"
        );
        assert_eq!(
            normalize_site_url("https://acme.atlassian.net///"),
            "https://acme.atlassian.net"
        );
        assert_eq!(
            normalize_site_url("https://acme.atlassian.net"),
            "https://acme.atlassian.net"
        );
    }
}
