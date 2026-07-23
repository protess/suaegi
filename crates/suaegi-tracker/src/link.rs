//! 워크트리 → 링크된 Linear 이슈 해석(§1.3). **순수 함수** — I/O 없음. 링크 없으면 `None`
//! (N3에서 에이전트에게 "링크된 이슈 없음" 힌트). N3의 `resolveCurrentIssue`가 이걸 재사용한다.

use suaegi_core::domain::Worktree;

/// 워크트리에 링크된 Linear 이슈. 세 조각(§1.3): 식별자 + 워크스페이스/조직 좌표(딥링크·재연결).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedLinearIssue {
    /// 이슈 식별자(예: `ENG-123`).
    pub issue: String,
    /// 다중 워크스페이스 구분(organization id).
    pub workspace_id: Option<String>,
    /// `linear.app/{url_key}/...` 딥링크·재연결 식별자.
    pub organization_url_key: Option<String>,
}

/// 워크트리의 링크 필드를 읽어 [`LinkedLinearIssue`]로. `linked_linear_issue`가 없으면 `None`.
pub fn resolve_current_issue(worktree: &Worktree) -> Option<LinkedLinearIssue> {
    let issue = worktree.linked_linear_issue.clone()?;
    Some(LinkedLinearIssue {
        issue,
        workspace_id: worktree.linked_linear_issue_workspace_id.clone(),
        organization_url_key: worktree.linked_linear_issue_organization_url_key.clone(),
    })
}

/// 워크트리에 링크된 Jira 이슈(§2). Linear보다 **단순한 두 조각**: 이슈 키 + 어느 사이트냐
/// (Linear의 워크스페이스 id/url_key 좌표 대신). 사이트는 딥링크(`{site}/browse/{key}`)와
/// 다중-사이트 구분에 쓴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedJiraIssue {
    /// 이슈 키(예: `PROJ-123`).
    pub issue: String,
    /// 어느 Jira 사이트(연결)의 이슈인지 — 정규화된 `site_url`. 아직 모르면 `None`(키만).
    pub site: Option<String>,
}

/// 워크트리의 Jira 링크 필드를 읽어 [`LinkedJiraIssue`]로. `linked_jira_issue`가 없으면 `None`
/// ([`resolve_current_issue`]의 Jira 짝 — N3 에이전트 힌트가 재사용한다).
pub fn resolve_current_jira_issue(worktree: &Worktree) -> Option<LinkedJiraIssue> {
    let issue = worktree.linked_jira_issue.clone()?;
    Some(LinkedJiraIssue {
        issue,
        site: worktree.linked_jira_site.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_core::domain::{RepoId, WorktreeId};

    fn base_worktree() -> Worktree {
        Worktree {
            id: WorktreeId("/tmp/ws/demo/x".into()),
            repo_id: RepoId("/tmp/demo".into()),
            path: "/tmp/ws/demo/x".into(),
            branch: "x".into(),
            display_name: "x".into(),
            created_with_agent: None,
            created_at_unix_ms: 0,
            linked_github_pr: None,
            linked_linear_issue: None,
            linked_linear_issue_workspace_id: None,
            linked_linear_issue_organization_url_key: None,
            linked_jira_issue: None,
            linked_jira_site: None,
        }
    }

    #[test]
    fn no_link_resolves_to_none() {
        assert_eq!(resolve_current_issue(&base_worktree()), None);
    }

    #[test]
    fn link_resolves_to_all_three_pieces() {
        let mut wt = base_worktree();
        wt.linked_linear_issue = Some("ENG-123".into());
        wt.linked_linear_issue_workspace_id = Some("org-1".into());
        wt.linked_linear_issue_organization_url_key = Some("acme".into());
        assert_eq!(
            resolve_current_issue(&wt),
            Some(LinkedLinearIssue {
                issue: "ENG-123".into(),
                workspace_id: Some("org-1".into()),
                organization_url_key: Some("acme".into()),
            })
        );
    }

    /// 식별자만 있고 좌표가 없어도 링크로 해석된다(좌표는 옵션).
    #[test]
    fn identifier_only_still_resolves() {
        let mut wt = base_worktree();
        wt.linked_linear_issue = Some("ENG-9".into());
        let got = resolve_current_issue(&wt).expect("should resolve");
        assert_eq!(got.issue, "ENG-9");
        assert_eq!(got.workspace_id, None);
        assert_eq!(got.organization_url_key, None);
    }

    #[test]
    fn no_jira_link_resolves_to_none() {
        assert_eq!(resolve_current_jira_issue(&base_worktree()), None);
    }

    #[test]
    fn jira_link_resolves_to_key_and_site() {
        let mut wt = base_worktree();
        wt.linked_jira_issue = Some("PROJ-123".into());
        wt.linked_jira_site = Some("https://acme.atlassian.net".into());
        assert_eq!(
            resolve_current_jira_issue(&wt),
            Some(LinkedJiraIssue {
                issue: "PROJ-123".into(),
                site: Some("https://acme.atlassian.net".into()),
            })
        );
    }

    /// 키만 있고 사이트를 몰라도 링크로 해석된다(사이트는 옵션).
    #[test]
    fn jira_key_only_still_resolves() {
        let mut wt = base_worktree();
        wt.linked_jira_issue = Some("PROJ-9".into());
        let got = resolve_current_jira_issue(&wt).expect("should resolve");
        assert_eq!(got.issue, "PROJ-9");
        assert_eq!(got.site, None);
    }

    /// **N2 링크는 N1 링크와 독립이다** — Jira만 링크된 워크트리는 Linear resolver에 `None`을,
    /// 그 반대도 성립한다(한 provider의 링크가 다른 provider로 새지 않는다).
    #[test]
    fn jira_and_linear_links_are_independent() {
        let mut wt = base_worktree();
        wt.linked_jira_issue = Some("PROJ-1".into());
        assert!(
            resolve_current_issue(&wt).is_none(),
            "a Jira-only link must not resolve as a Linear link"
        );
        assert!(resolve_current_jira_issue(&wt).is_some());
    }
}
