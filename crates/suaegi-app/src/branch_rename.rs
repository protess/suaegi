//! Safe first-work branch auto-rename orchestration.

use std::path::{Path, PathBuf};

use suaegi_git::runner::GitRunner;
use suaegi_workname::{
    derive_workspace_display_name, is_auto_generated_creature_branch_name, sanitize_branch_slug,
    strip_configured_branch_prefix, MAX_BRANCH_NAME_WORDS,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchRenameOutcome {
    Renamed {
        old_branch: String,
        new_branch: String,
        display_name: String,
    },
    Settled(String),
    Retry(String),
}

fn branch_leaf(branch: &str) -> &str {
    branch.rsplit('/').next().unwrap_or(branch)
}

fn branch_prefix(branch: &str) -> Option<&str> {
    branch.rsplit_once('/').map(|(prefix, _)| prefix)
}

async fn current_branch(runner: &GitRunner, worktree: &Path) -> Result<String, String> {
    runner
        .run(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .map(|output| output.stdout.trim().to_string())
        .map_err(|error| error.to_string())
}

async fn has_upstream(runner: &GitRunner, worktree: &Path, branch: &str) -> Result<bool, String> {
    let reference = format!("refs/heads/{branch}");
    runner
        .run(
            worktree,
            &["for-each-ref", "--format=%(upstream)", &reference],
        )
        .await
        .map(|output| !output.stdout.trim().is_empty())
        .map_err(|error| error.to_string())
}

async fn branch_exists(runner: &GitRunner, worktree: &Path, branch: &str) -> Result<bool, String> {
    let reference = format!("refs/heads/{branch}");
    runner
        .run_expecting(
            worktree,
            &["show-ref", "--verify", "--quiet", &reference],
            &[1],
        )
        .await
        .map(|output| output.code == 0)
        .map_err(|error| error.to_string())
}

fn apply_prefix(prefix: Option<&str>, leaf: &str) -> String {
    prefix.map_or_else(|| leaf.to_string(), |prefix| format!("{prefix}/{leaf}"))
}

/// Rename only an unpublished Orca creature branch. All checks are repeated
/// immediately before mutation so a concurrent checkout/push wins safely.
pub async fn rename_from_first_work(
    worktree: PathBuf,
    expected_branch: String,
    prompt: String,
    configured_prefix: Option<String>,
    generation: Option<crate::source_control_ai::GenerationRequest>,
) -> BranchRenameOutcome {
    let runner = GitRunner::new();
    let current = match current_branch(&runner, &worktree).await {
        Ok(branch) if !branch.is_empty() && branch != "HEAD" => branch,
        Ok(_) => return BranchRenameOutcome::Retry("no checked-out branch".to_string()),
        Err(error) => return BranchRenameOutcome::Retry(error),
    };
    if current != expected_branch {
        return BranchRenameOutcome::Retry(format!(
            "branch changed before rename ({expected_branch} -> {current})"
        ));
    }
    if !is_auto_generated_creature_branch_name(branch_leaf(&current)) {
        return BranchRenameOutcome::Settled(format!(
            "branch {current:?} is not an auto-generated creature"
        ));
    }
    match has_upstream(&runner, &worktree, &current).await {
        Ok(true) => {
            return BranchRenameOutcome::Settled(format!(
                "branch {current:?} is already published"
            ));
        }
        Ok(false) => {}
        Err(error) => return BranchRenameOutcome::Retry(error),
    }

    let generated_source = if let Some(generation) = generation {
        crate::source_control_ai::generate_branch_name(&worktree, &prompt, generation)
            .await
            .unwrap_or_else(|_| {
                crate::tab_title::derive_generated_tab_title(&prompt)
                    .unwrap_or_else(|| prompt.clone())
            })
    } else {
        crate::tab_title::derive_generated_tab_title(&prompt).unwrap_or_else(|| prompt.clone())
    };
    let generated = sanitize_branch_slug(&generated_source, MAX_BRANCH_NAME_WORDS);
    let slug = strip_configured_branch_prefix(&generated, configured_prefix.as_deref());
    if slug.is_empty() {
        return BranchRenameOutcome::Settled(
            "first prompt produced no safe branch name".to_string(),
        );
    }

    // Keep the branch's actual existing prefix. It is authoritative even when
    // settings changed after workspace creation.
    let prefix = branch_prefix(&current);
    let mut resolved_leaf = slug.clone();
    let mut resolved = apply_prefix(prefix, &resolved_leaf);
    let mut suffix = 2usize;
    loop {
        match branch_exists(&runner, &worktree, &resolved).await {
            Ok(false) => break,
            Ok(true) => {
                resolved_leaf = format!("{slug}-{suffix}");
                resolved = apply_prefix(prefix, &resolved_leaf);
                suffix += 1;
            }
            Err(error) => return BranchRenameOutcome::Retry(error),
        }
    }
    if resolved == current {
        return BranchRenameOutcome::Settled("generated branch is unchanged".to_string());
    }

    let current_now = match current_branch(&runner, &worktree).await {
        Ok(branch) => branch,
        Err(error) => return BranchRenameOutcome::Retry(error),
    };
    if current_now != current {
        return BranchRenameOutcome::Retry(format!(
            "branch changed during generation ({current} -> {current_now})"
        ));
    }
    match has_upstream(&runner, &worktree, &current).await {
        Ok(false) => {}
        Ok(true) => {
            return BranchRenameOutcome::Retry(format!(
                "branch {current:?} was published during generation"
            ));
        }
        Err(error) => return BranchRenameOutcome::Retry(error),
    }

    if let Err(error) = runner.run(&worktree, &["branch", "-m", &resolved]).await {
        return BranchRenameOutcome::Retry(error.to_string());
    }
    BranchRenameOutcome::Renamed {
        old_branch: current,
        new_branch: resolved,
        display_name: derive_workspace_display_name(&prompt, &slug, Some(&resolved_leaf)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn fixture(branch: &str) -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let path = temp.path().to_path_buf();
        let runner = GitRunner::new();
        runner.run(&path, &["init"]).await.unwrap();
        runner
            .run(&path, &["config", "user.email", "test@example.com"])
            .await
            .unwrap();
        runner
            .run(&path, &["config", "user.name", "Test"])
            .await
            .unwrap();
        std::fs::write(path.join("README"), "x").unwrap();
        runner.run(&path, &["add", "README"]).await.unwrap();
        runner.run(&path, &["commit", "-m", "init"]).await.unwrap();
        runner
            .run(&path, &["checkout", "-b", branch])
            .await
            .unwrap();
        (temp, path)
    }

    #[tokio::test]
    async fn fresh_unpublished_creature_branch_is_renamed_with_collision_suffix() {
        let (_temp, path) = fixture("you/Nautilus").await;
        let runner = GitRunner::new();
        runner
            .run(&path, &["branch", "you/fix-the-auth-bug"])
            .await
            .unwrap();

        let outcome = rename_from_first_work(
            path.clone(),
            "you/Nautilus".to_string(),
            "Please fix the auth bug quickly".to_string(),
            Some("you".to_string()),
            None,
        )
        .await;
        assert_eq!(
            outcome,
            BranchRenameOutcome::Renamed {
                old_branch: "you/Nautilus".to_string(),
                new_branch: "you/fix-the-auth-bug-2".to_string(),
                display_name: "Fix the auth bug 2".to_string(),
            }
        );
        assert_eq!(
            current_branch(&runner, &path).await.unwrap(),
            "you/fix-the-auth-bug-2"
        );
    }

    #[tokio::test]
    async fn deliberate_non_creature_branch_is_never_changed() {
        let (_temp, path) = fixture("feature/manual-name").await;
        let outcome = rename_from_first_work(
            path,
            "feature/manual-name".to_string(),
            "fix auth".to_string(),
            None,
            None,
        )
        .await;
        assert!(matches!(outcome, BranchRenameOutcome::Settled(_)));
    }
}
