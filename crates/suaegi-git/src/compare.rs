use crate::refname::validate_user_ref;
use crate::runner::{GitError, GitRunner};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    Other(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCompare {
    pub merge_base: String,
    pub ahead_count: u32,
    pub files: Vec<ChangedFile>,
}

async fn merge_base(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
) -> Result<String, GitError> {
    validate_user_ref(base_ref)?;
    Ok(runner
        .run(worktree_path, &["merge-base", "HEAD", base_ref])
        .await?
        .stdout
        .trim()
        .to_string())
}

/// numstat 카운트 파싱: "-"(바이너리)만 None. 그 외 비숫자는 손상 출력이므로 Parse.
fn parse_count(token: &str, args: &str) -> Result<Option<u32>, GitError> {
    if token == "-" {
        return Ok(None);
    }
    token.parse::<u32>().map(Some).map_err(|e| GitError::Parse {
        args: args.to_string(),
        detail: format!("bad numstat count {token:?}: {e}"),
    })
}

/// merge-base 대비 **working tree** diff. `<mb>..HEAD`가 아니라 `<mb>`를 단독
/// 인자로 주면 커밋된 변경과 미커밋 변경이 모두 잡힌다. untracked 파일은 diff에
/// 안 잡히므로 `status --porcelain -z`에서 별도 수집해 Added로 합류시킨다.
pub async fn branch_compare(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
) -> Result<BranchCompare, GitError> {
    let mb = merge_base(runner, worktree_path, base_ref).await?;
    let ahead_args = format!("{mb}..HEAD");
    let ahead_out = runner
        .run(worktree_path, &["rev-list", "--count", &ahead_args])
        .await?;
    let ahead = ahead_out
        .stdout
        .trim()
        .parse::<u32>()
        .map_err(|e| GitError::Parse {
            args: format!("rev-list --count {ahead_args}"),
            detail: format!("{e}: {:?}", ahead_out.stdout),
        })?;

    // -z: 레코드가 NUL 구분이라 특수문자 경로 안전. -M: rename 감지 (Orca와 동일)
    let name_status = runner
        .run(worktree_path, &["diff", "--name-status", "-z", "-M", &mb])
        .await?;
    let numstat = runner
        .run(worktree_path, &["diff", "--numstat", "-z", "-M", &mb])
        .await?;

    // numstat -z 레코드: "adds\tdels\tpath" 또는 rename 시 "adds\tdels\t" 뒤에
    // from, to가 각각 별도 NUL 레코드로 이어진다.
    let numstat_args = format!("diff --numstat -z -M {mb}");
    let mut counts: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::new();
    let mut numstat_records = numstat.stdout.split('\0');
    while let Some(record) = numstat_records.next() {
        if record.is_empty() {
            continue;
        }
        // splitn(3): 파일명에 탭이 있어도 세 번째 조각(경로)이 절단되지 않게
        let mut parts = record.splitn(3, '\t');
        let (Some(a), Some(d)) = (parts.next(), parts.next()) else {
            return Err(GitError::Parse {
                args: numstat_args.clone(),
                detail: format!("truncated record {record:?}"),
            });
        };
        let adds = parse_count(a, &numstat_args)?;
        let dels = parse_count(d, &numstat_args)?;
        match parts.next() {
            Some(path) if !path.is_empty() => {
                counts.insert(path.to_string(), (adds, dels));
            }
            _ => {
                // rename: from, to가 이어지는 별도 레코드
                let _from = numstat_records.next();
                let to = numstat_records.next().ok_or_else(|| GitError::Parse {
                    args: numstat_args.clone(),
                    detail: "rename record missing target path".to_string(),
                })?;
                counts.insert(to.to_string(), (adds, dels));
            }
        }
    }

    // name-status -z 레코드: "X\0path\0" 또는 rename "R100\0from\0to\0"
    let ns_args = format!("diff --name-status -z -M {mb}");
    let mut files = Vec::new();
    let mut records = name_status.stdout.split('\0');
    while let Some(code) = records.next() {
        if code.is_empty() {
            continue;
        }
        let status_char = code.chars().next().unwrap_or('?');
        let (status, path) = match status_char {
            'R' => {
                let from = records.next().ok_or_else(|| GitError::Parse {
                    args: ns_args.clone(),
                    detail: "rename record missing source path".to_string(),
                })?;
                let to = records.next().ok_or_else(|| GitError::Parse {
                    args: ns_args.clone(),
                    detail: "rename record missing target path".to_string(),
                })?;
                (
                    ChangeStatus::Renamed {
                        from: from.to_string(),
                    },
                    to.to_string(),
                )
            }
            c => {
                let path = records.next().ok_or_else(|| GitError::Parse {
                    args: ns_args.clone(),
                    detail: format!("record {code:?} missing path"),
                })?;
                let status = match c {
                    'A' => ChangeStatus::Added,
                    'M' => ChangeStatus::Modified,
                    'D' => ChangeStatus::Deleted,
                    other => ChangeStatus::Other(other),
                };
                (status, path.to_string())
            }
        };
        let (additions, deletions) = counts.get(&path).copied().unwrap_or((None, None));
        files.push(ChangedFile {
            path,
            status,
            additions,
            deletions,
        });
    }

    // untracked 파일 수집: status --porcelain -z에서 "?? path" 레코드
    let status_out = runner
        .run(
            worktree_path,
            &["status", "--porcelain", "-z", "--untracked-files=all"],
        )
        .await?;
    for record in status_out.stdout.split('\0') {
        if let Some(path) = record.strip_prefix("?? ") {
            files.push(ChangedFile {
                path: path.to_string(),
                status: ChangeStatus::Added,
                additions: None,
                deletions: None,
            });
        }
    }

    Ok(BranchCompare {
        merge_base: mb,
        ahead_count: ahead,
        files,
    })
}

pub async fn file_diff(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
    file: &str,
) -> Result<String, GitError> {
    let mb = merge_base(runner, worktree_path, base_ref).await?;
    let patch = runner
        .run(worktree_path, &["diff", "-M", &mb, "--", file])
        .await?
        .stdout;
    if !patch.is_empty() {
        return Ok(patch);
    }
    // 빈 diff는 "변경 없는 tracked 파일"일 수도 있다. 실제 untracked("??")일 때만
    // --no-index로 합성 (차이 있으면 exit 1). 아니면 빈 patch가 정답.
    let status = runner
        .run(worktree_path, &["status", "--porcelain", "-z", "--", file])
        .await?;
    let is_untracked = status
        .stdout
        .split('\0')
        .any(|r| r.strip_prefix("?? ").is_some_and(|p| p == file));
    if !is_untracked {
        return Ok(patch);
    }
    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let out = runner
        .run_expecting(
            worktree_path,
            &["diff", "--no-index", "--", null_device, file],
            &[1],
        )
        .await?;
    Ok(out.stdout)
}

pub async fn working_tree_dirty(
    runner: &GitRunner,
    worktree_path: &Path,
) -> Result<bool, GitError> {
    let out = runner
        .run(worktree_path, &["status", "--porcelain"])
        .await?;
    Ok(!out.stdout.trim().is_empty())
}
