//! 단일 커밋의 부모 대비 diff. Orca `commitCompare`
//! (`git-handler-commit-diff-ops.ts:15-122`)의 이식.
//!
//! **M0에서 추출한 파서(`parse_numstat_z`/`parse_name_status_z`)를 재사용**한다 —
//! commit..commit diff와 `diff-tree --root`의 `-z` 출력이 working-tree diff와
//! 동일한 형태이기 때문이다.
//!
//! **파일별 blob diff는 담지 않는다(lean).** Orca도 `commitCompare`는 name-status +
//! numstat 카운트(`entries`)만 내고, 파일 내용 diff는 UI가 파일을 펼칠 때 별도
//! lazy 핸들러(`commitDiffEntry` `:124-`)로 가져온다. suaegi는 그 lazy 경로에 기존
//! `compare::file_diff`/`file_head_bytes`를 그대로 쓸 수 있으므로, 여기서 미리
//! 모든 파일의 patch를 만들지 않는다.

use crate::compare::{parse_name_status_z, parse_numstat_z, ChangedFile};
use crate::quick_open::is_lower_hex;
use crate::runner::{GitError, GitRunner};
use std::path::Path;

/// 단일 커밋의 변경 요약. Orca `commitCompare`의 `{summary, entries}`를 lean하게
/// 이식했다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDiff {
    /// 대상 커밋 oid. 입력을 그대로 담는다 — 이미 40/64 hex로 검증됐다.
    pub commit: String,
    /// 첫 부모 oid. **root commit이면 `None`**(→ `diff-tree --root` 분기를 탄 것).
    /// 이후 파일별 lazy diff가 `parent`를 base로 쓴다(Orca `commitDiffEntry`가
    /// `parentOid`를 받는 것과 같다).
    pub parent: Option<String>,
    /// 변경 파일 목록(name-status + numstat 카운트). 빈 커밋이면 빈 목록이다 —
    /// **오류가 아니다**.
    pub files: Vec<ChangedFile>,
}

/// commit ref가 **정확히** 40자(SHA-1) 또는 64자(SHA-256) 소문자 hex인가.
///
/// **exact-alternation이다 — `{40,64}` 범위가 아니다.** 범위로 두면 41..63자도
/// 통과해 "full object id" 보장을 잃는다. 이 가드가 git argv에 닿기 전에 서서
/// (1) leading-`-` 옵션 주입과 (2) `HEAD~1`/`a..b` 같은 ref-expression 모호성을
/// 원천 차단한다.
///
/// Orca `assertFullGitObjectId`(`git-handler-commit-diff-ops.ts:7-13`,
/// `/^(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})$/`)와 동치이되 소문자로 좁혔다.
/// hex 판정은 `quick_open::is_lower_hex`를 재사용한다.
fn is_full_object_id(commit: &str) -> bool {
    let len = commit.len();
    if len != 40 && len != 64 {
        return false;
    }
    commit.bytes().all(is_lower_hex)
}

/// 커밋 하나의 부모 대비 변경을 계산한다.
///
/// - **argv 가드**: `commit`이 40/64 hex가 아니면 git을 **한 번도 부르지 않고**
///   즉시 `Err`. (위 `is_full_object_id` 참고.)
/// - **first-parent 판별**: `rev-list --parents -n 1 <commit>`의 2번째 whitespace
///   필드가 첫 부모다(Orca `parseGitRevListFirstParentOid`). 필드가 하나뿐이면
///   부모 없음(root commit) → `None`.
/// - **root commit**: 하드코딩 empty-tree 해시가 **아니라** `diff-tree --root`로
///   git이 empty tree와 비교하게 한다(`git-handler-commit-diff-ops.ts:70-104`).
/// - **존재하지 않는 커밋**: `rev-list`가 `fatal`로 실패 → `GitError`가 그대로
///   올라간다(진짜 실패를 빈 결과로 뭉개지 않는다). **빈 커밋**(부모와 차이 없음)은
///   빈 `files`이지 오류가 아니다.
pub async fn commit_show(
    runner: &GitRunner,
    worktree_path: &Path,
    commit: &str,
) -> Result<CommitDiff, GitError> {
    if !is_full_object_id(commit) {
        return Err(GitError::Parse {
            args: "commit_show ref validation".to_string(),
            detail: format!(
                "commit must be a full 40- or 64-char lowercase hex object id: {commit:?}"
            ),
        });
    }

    // 첫 부모 oid: `--parents` 출력의 2번째 필드. root commit이면 필드가 하나뿐이라
    // `nth(1)`이 None. (nonexistent commit이면 rev-list가 실패해 여기서 Err.)
    let parents_out = runner
        .run(worktree_path, &["rev-list", "--parents", "-n", "1", commit])
        .await?;
    let parent = parents_out
        .stdout
        .split_whitespace()
        .nth(1)
        .map(|s| s.to_string());

    // -z: 특수문자 경로 안전(NUL 구분). -M: rename, -C: copy 감지. root면 부모 트리가
    // 없어 diff-tree --root로 empty tree와 비교한다(-r로 재귀, 하위 파일까지).
    let (name_status, numstat) = match &parent {
        Some(p) => {
            let ns = runner
                .run(
                    worktree_path,
                    &["diff", "--name-status", "-z", "-M", "-C", p, commit],
                )
                .await?;
            let num = runner
                .run(
                    worktree_path,
                    &["diff", "--numstat", "-z", "-M", "-C", p, commit],
                )
                .await?;
            (ns, num)
        }
        None => {
            let ns = runner
                .run(
                    worktree_path,
                    &[
                        "diff-tree",
                        "--root",
                        "--no-commit-id",
                        "--name-status",
                        "-r",
                        "-z",
                        "-M",
                        "-C",
                        commit,
                    ],
                )
                .await?;
            let num = runner
                .run(
                    worktree_path,
                    &[
                        "diff-tree",
                        "--root",
                        "--no-commit-id",
                        "--numstat",
                        "-r",
                        "-z",
                        "-M",
                        "-C",
                        commit,
                    ],
                )
                .await?;
            (ns, num)
        }
    };

    // M0 추출 파서 재사용: numstat 카운트를 먼저 판 뒤 name-status에 경로로 조인.
    let counts = parse_numstat_z(&numstat.stdout)?;
    let files = parse_name_status_z(&name_status.stdout, &counts)?;

    Ok(CommitDiff {
        commit: commit.to_string(),
        parent,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::is_full_object_id;

    #[test]
    fn full_object_id_is_exact_40_or_64_lowercase_hex() {
        // 정확히 40, 64는 통과.
        assert!(is_full_object_id(&"a".repeat(40)));
        assert!(is_full_object_id(&"0123456789abcdef".repeat(4))); // 64
                                                                   // 범위 사이(41..63)는 거부 — `{40,64}`가 아니라 exact-alternation임을 고정.
        assert!(!is_full_object_id(&"a".repeat(41)));
        assert!(!is_full_object_id(&"a".repeat(50)));
        assert!(!is_full_object_id(&"a".repeat(63)));
        // 39/65도 거부.
        assert!(!is_full_object_id(&"a".repeat(39)));
        assert!(!is_full_object_id(&"a".repeat(65)));
        // 대문자·비-hex·옵션류·ref 표현식 거부.
        assert!(!is_full_object_id(&"A".repeat(40)));
        assert!(!is_full_object_id(&"g".repeat(40)));
        assert!(!is_full_object_id("HEAD~1"));
        assert!(!is_full_object_id("-foo"));
        assert!(!is_full_object_id(""));
    }
}
