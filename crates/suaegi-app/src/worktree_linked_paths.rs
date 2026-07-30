use std::path::{Component, Path, PathBuf};

pub fn normalize_relative_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw
        .trim()
        .trim_start_matches(['/', '\\'])
        .replace('\\', "/");
    if trimmed.is_empty()
        || trimmed.as_bytes().get(1) == Some(&b':')
        || Path::new(&trimmed)
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

pub async fn materialize(
    primary_path: PathBuf,
    worktree_path: PathBuf,
    paths: Vec<String>,
) -> Vec<String> {
    materialize_with_mode(primary_path, worktree_path, paths, true).await
}

/// Resolve `orca.yaml` shared directories to existing gitignored directories.
/// A malformed config or Git failure never blocks worktree creation.
pub async fn resolve_shared_directories(primary_path: PathBuf) -> Vec<String> {
    let configured = match crate::repo_hooks::load_shared_directories(&primary_path) {
        Ok(configured) => configured,
        Err(error) => {
            eprintln!("worktree shared directories: {error}");
            return Vec::new();
        }
    };
    let existing = configured
        .into_iter()
        .filter(|relative| {
            std::fs::metadata(primary_path.join(relative)).is_ok_and(|metadata| metadata.is_dir())
        })
        .collect::<Vec<_>>();
    let refs = existing.iter().map(String::as_str).collect::<Vec<_>>();
    let ignored = match suaegi_git::status::check_ignored(
        &suaegi_git::runner::GitRunner::new(),
        &primary_path,
        &refs,
    )
    .await
    {
        Ok(ignored) => ignored,
        Err(error) => {
            eprintln!("worktree shared directories: {error}");
            return Vec::new();
        }
    };
    let mut resolved = existing
        .into_iter()
        .filter(|relative| ignored.contains(relative))
        .collect::<Vec<_>>();
    resolved.sort();
    resolved
}

/// Shared directories always remain symlinks, even on APFS: one installation
/// must be visible to every worktree.
pub async fn materialize_shared(
    primary_path: PathBuf,
    worktree_path: PathBuf,
    paths: Vec<String>,
) -> Vec<String> {
    materialize_with_mode(primary_path, worktree_path, paths, false).await
}

async fn materialize_with_mode(
    primary_path: PathBuf,
    worktree_path: PathBuf,
    paths: Vec<String>,
    allow_apfs_clone: bool,
) -> Vec<String> {
    tokio::task::spawn_blocking(move || {
        materialize_blocking(&primary_path, &worktree_path, &paths, allow_apfs_clone)
    })
    .await
    .unwrap_or_else(|error| vec![format!("Shared-path worker failed: {error}")])
}

pub async fn existing_symlinks(worktree_path: PathBuf, paths: Vec<String>) -> Vec<String> {
    tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .filter_map(|raw| {
                let relative = normalize_relative_path(&raw)?;
                std::fs::symlink_metadata(worktree_path.join(&relative))
                    .ok()
                    .filter(|metadata| metadata.file_type().is_symlink())
                    .map(|_| relative.to_string_lossy().to_string())
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

pub async fn remove_symlinks(worktree_path: PathBuf, paths: Vec<String>) -> Vec<String> {
    tokio::task::spawn_blocking(move || {
        let mut warnings = Vec::new();
        for raw in paths {
            let Some(relative) = normalize_relative_path(&raw) else {
                continue;
            };
            let target = worktree_path.join(&relative);
            match std::fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    if let Err(error) = std::fs::remove_file(&target) {
                        warnings.push(format!("Could not remove {}: {error}", target.display()));
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    warnings.push(format!("Could not inspect {}: {error}", target.display()));
                }
            }
        }
        warnings
    })
    .await
    .unwrap_or_else(|error| vec![format!("Shared-path cleanup worker failed: {error}")])
}

fn materialize_blocking(
    primary: &Path,
    worktree: &Path,
    paths: &[String],
    allow_apfs_clone: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for raw in paths {
        let Some(relative) = normalize_relative_path(raw) else {
            warnings.push(format!("Skipped unsafe shared path: {raw}"));
            continue;
        };
        let source = primary.join(&relative);
        let target = worktree.join(&relative);
        let source_link_metadata = match std::fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warnings.push(format!("Could not inspect {}: {error}", source.display()));
                continue;
            }
        };
        let source_metadata = match std::fs::metadata(&source) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if std::fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        if let Some(parent) = target.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                warnings.push(format!("Could not create {}: {error}", parent.display()));
                continue;
            }
        }

        #[cfg(target_os = "macos")]
        if allow_apfs_clone && !source_link_metadata.file_type().is_symlink() {
            match clone_on_same_apfs_volume(&source, &target, &source_metadata) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => warnings.push(format!(
                    "APFS clone-copy unavailable for {}: {error}",
                    target.display()
                )),
            }
            // A failed directory clone may have published some content. Never
            // replace or merge a partial target with a symlink.
            if std::fs::symlink_metadata(&target).is_ok() {
                warnings.push(format!(
                    "APFS clone-copy was incomplete for {}; left it for review",
                    target.display()
                ));
                continue;
            }
        }

        #[cfg(unix)]
        {
            if let Err(error) = std::os::unix::fs::symlink(&source, &target) {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    warnings.push(format!(
                        "Could not link {} to {}: {error}",
                        source.display(),
                        target.display()
                    ));
                }
            }
        }
        #[cfg(not(unix))]
        {
            let result = if source_metadata.is_dir() {
                std::os::windows::fs::symlink_dir(&source, &target)
            } else {
                std::os::windows::fs::symlink_file(&source, &target)
            };
            if let Err(error) = result {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    warnings.push(format!(
                        "Could not link {} to {}: {error}",
                        source.display(),
                        target.display()
                    ));
                }
            }
        }
    }
    warnings
}

#[cfg(target_os = "macos")]
fn clone_on_same_apfs_volume(
    source: &Path,
    target: &Path,
    source_metadata: &std::fs::Metadata,
) -> std::io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    let Some(target_parent) = target.parent() else {
        return Ok(false);
    };
    let source_volume = darwin_volume(source)?;
    let target_volume = darwin_volume(target_parent)?;
    if source_volume != target_volume || source_volume.1 != "APFS" {
        return Ok(false);
    }

    if source_metadata.is_dir() {
        match std::fs::create_dir(target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(true),
            Err(error) => return Err(error),
        }
        let status = std::process::Command::new("/bin/cp")
            .args(["-n", "-c", "-R"])
            .arg(source)
            .arg(target_parent)
            .status();
        match status {
            Ok(status) if status.success() => {
                let mode = source_metadata.permissions().mode();
                std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode))?;
                Ok(true)
            }
            Ok(status) => {
                let _ = std::fs::remove_dir(target);
                Err(std::io::Error::other(format!(
                    "/bin/cp exited with {status}"
                )))
            }
            Err(error) => {
                let _ = std::fs::remove_dir(target);
                Err(error)
            }
        }
    } else {
        let temp_name = format!(
            ".suaegi-apfs-clone-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        );
        let temp = target_parent.join(temp_name);
        let result = (|| {
            let status = std::process::Command::new("/bin/cp")
                .arg("-c")
                .arg(source)
                .arg(&temp)
                .status()?;
            if !status.success() {
                return Err(std::io::Error::other(format!(
                    "/bin/cp exited with {status}"
                )));
            }
            match std::fs::hard_link(&temp, target) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(true),
                Err(error) => Err(error),
            }
        })();
        let _ = std::fs::remove_file(temp);
        result
    }
}

#[cfg(target_os = "macos")]
fn darwin_volume(path: &Path) -> std::io::Result<(String, String)> {
    let output = std::process::Command::new("/bin/df")
        .arg("-P")
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other("/bin/df failed"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let device = stdout
        .lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| std::io::Error::other("could not resolve filesystem device"))?;
    let output = std::process::Command::new("/usr/sbin/diskutil")
        .args(["info", "-plist", device])
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other("/usr/sbin/diskutil failed"));
    }
    let plist = String::from_utf8_lossy(&output.stdout);
    let marker = "<key>FilesystemName</key>";
    let filesystem = plist
        .split_once(marker)
        .and_then(|(_, suffix)| suffix.split_once("<string>"))
        .and_then(|(_, suffix)| suffix.split_once("</string>"))
        .map(|(name, _)| name.to_string())
        .unwrap_or_default();
    Ok((device.to_string(), filesystem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_drive_paths_are_rejected() {
        assert_eq!(normalize_relative_path("../secret"), None);
        assert_eq!(normalize_relative_path(r"apps\\..\\secret"), None);
        assert_eq!(normalize_relative_path(r"C:\\secret"), None);
        assert_eq!(
            normalize_relative_path("/apps/web/.env"),
            Some(PathBuf::from("apps/web/.env"))
        );
    }

    #[tokio::test]
    async fn missing_sources_skip_and_existing_targets_are_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(primary.join(".env"), "source").unwrap();
        std::fs::write(worktree.join(".env"), "target").unwrap();
        let warnings = materialize(
            primary,
            worktree.clone(),
            vec![".env".into(), "missing".into()],
        )
        .await;
        assert!(warnings.is_empty());
        assert_eq!(
            std::fs::read_to_string(worktree.join(".env")).unwrap(),
            "target"
        );
    }

    #[tokio::test]
    async fn created_links_are_detected_and_removed_without_touching_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(primary.join(".env"), "source").unwrap();
        std::fs::write(worktree.join("keep"), "regular").unwrap();
        std::os::unix::fs::symlink(primary.join(".env"), worktree.join(".env")).unwrap();

        assert_eq!(
            existing_symlinks(
                worktree.clone(),
                vec![".env".into(), "keep".into(), "missing".into()]
            )
            .await,
            vec![".env"]
        );
        assert!(
            remove_symlinks(worktree.clone(), vec![".env".into(), "keep".into()])
                .await
                .is_empty()
        );
        assert!(!worktree.join(".env").exists());
        assert_eq!(
            std::fs::read_to_string(worktree.join("keep")).unwrap(),
            "regular"
        );
    }

    #[tokio::test]
    async fn yaml_shared_directories_require_existing_ignored_directories() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary");
        std::fs::create_dir_all(primary.join("node_modules")).unwrap();
        std::fs::create_dir_all(primary.join("tracked-cache")).unwrap();
        std::fs::write(primary.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(
            primary.join("orca.yaml"),
            "worktree:\n  sharedDirectories:\n    - tracked-cache\n    - missing\n    - node_modules\n",
        )
        .unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&primary)
            .status()
            .unwrap();
        assert!(init.success());

        assert_eq!(
            resolve_shared_directories(primary).await,
            vec!["node_modules"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn yaml_shared_directories_are_symlinked_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(primary.join("node_modules")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();

        assert!(
            materialize_shared(primary, worktree.clone(), vec!["node_modules".into()])
                .await
                .is_empty()
        );
        assert!(std::fs::symlink_metadata(worktree.join("node_modules"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
