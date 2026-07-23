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
}
