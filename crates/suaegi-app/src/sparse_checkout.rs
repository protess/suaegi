use std::path::{Component, Path};

use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::CreatedWorktree;

pub fn normalize_directories(value: &str) -> Result<Vec<String>, String> {
    let mut directories = Vec::new();
    for raw in value.split([',', '\n']) {
        let normalized = raw.trim().trim_end_matches('/').replace('\\', "/");
        if normalized.is_empty() {
            continue;
        }
        let path = Path::new(&normalized);
        if normalized == "."
            || normalized.as_bytes().get(1) == Some(&b':')
            || path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
        {
            return Err(
                "Use repo-relative directories, not root, absolute paths, or parent segments."
                    .to_string(),
            );
        }
        if !directories.contains(&normalized) {
            directories.push(normalized);
        }
    }
    if directories.is_empty() {
        return Err("Add at least one directory.".to_string());
    }
    Ok(directories)
}

pub async fn apply(created: &CreatedWorktree, directories: &[String]) -> Result<(), String> {
    if directories.is_empty() {
        return Ok(());
    }
    let runner = GitRunner::new();
    let mut args = vec![
        "sparse-checkout".to_string(),
        "set".to_string(),
        "--cone".to_string(),
        "--".to_string(),
    ];
    args.extend(directories.iter().cloned());
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    runner
        .run(&created.path, &refs)
        .await
        .map_err(|error| format!("Could not enable sparse checkout: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_are_normalized_deduplicated_and_guarded() {
        assert_eq!(
            normalize_directories("apps/web/\napps\\web\npackages/core").unwrap(),
            vec!["apps/web", "packages/core"]
        );
        for value in [".", "../secret", "/root", r"C:\root", "apps/../secret"] {
            assert!(normalize_directories(value).is_err(), "{value}");
        }
    }
}
