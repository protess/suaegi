//! M3-lite — 충돌 진행상태 프로브 + merge/rebase abort.
//!
//! `status.rs`가 **어떤 경로가** 충돌인지(kind)를 답한다면, 여기는 **저장소가 지금 어떤
//! 충돌 연산 중인지**(merge/rebase/cherry-pick)를 git 디렉터리의 마커 파일로 답한다 —
//! Orca `detectConflictOperation`/`resolveGitDir`(status.ts:923-986) 포팅.
//!
//! git 서브프로세스를 쓰지 않는 **순수 real-fs 프로브**다(abort 둘만 `GitRunner` 경유).
//! linked-worktree에서는 `<worktree>/.git`이 디렉터리가 아니라 `gitdir: <path>` 포인터를
//! 담은 **파일**이라, 마커를 찾기 전에 진짜 git 디렉터리를 먼저 해석해야 한다.

use crate::runner::{GitError, GitRunner};
use std::io;
use std::path::{Path, PathBuf};

/// 지금 진행 중인 충돌 연산의 종류. `<git-dir>`의 마커 파일/디렉터리로 판별한다
/// (Orca `GitConflictOperation`, status.ts:923-952).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictOperation {
    /// `MERGE_HEAD` 존재 → merge 진행 중.
    Merge,
    /// `rebase-merge/` 또는 `rebase-apply/` 디렉터리 존재 → rebase 진행 중.
    Rebase,
    /// `CHERRY_PICK_HEAD` 존재 → cherry-pick 진행 중.
    CherryPick,
    /// 위 마커가 하나도 없음(진행 중인 충돌 연산 없음, 또는 판별 불가).
    Unknown,
}

/// `<worktree>/.git`을 실제 git 디렉터리로 해석한다.
///
/// 일반 체크아웃은 `.git`이 **디렉터리**라 그대로 git 디렉터리다. linked worktree
/// (`git worktree add`)는 `.git`이 `gitdir: <path>` 한 줄을 담은 **파일**이고, 그
/// 포인터가 `<main>/.git/worktrees/<name>`을 가리킨다(Orca status.ts:972-986). 이
/// 간접을 따라가지 않으면 merge/rebase 마커를 엉뚱한 곳에서 찾게 된다.
///
/// - `.git`이 없음(ENOENT) → `<worktree>/.git`을 그대로 돌려준다(비-repo에서도 마커
///   프로브가 전부 false → `Unknown`으로 안전하게 떨어진다, Orca의 catch→dotGitPath 동치).
/// - 포인터를 못 읽거나 `gitdir:` 라인이 없음 → 마찬가지로 `.git` 경로를 돌려준다.
pub fn resolve_git_dir(worktree: &Path) -> io::Result<PathBuf> {
    let dot_git = worktree.join(".git");
    let meta = match std::fs::metadata(&dot_git) {
        Ok(m) => m,
        // 존재하지 않으면 그대로 돌려준다 — 마커 프로브가 알아서 false를 낸다.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(dot_git),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        return Ok(dot_git);
    }
    // `.git`이 파일이다 = linked worktree. `gitdir:` 포인터를 따라간다.
    let contents = std::fs::read_to_string(&dot_git)?;
    match parse_gitdir_pointer(&contents) {
        // 절대 경로면 그대로, 상대 경로면 worktree 기준으로 해석(Orca path.resolve 동치).
        Some(ptr) => {
            let p = Path::new(ptr);
            if p.is_absolute() {
                Ok(p.to_path_buf())
            } else {
                Ok(worktree.join(p))
            }
        }
        None => Ok(dot_git),
    }
}

/// `.git` 파일 내용에서 `gitdir: <path>` 포인터를 뽑는다(순수). 첫 `gitdir:` 라인의
/// 값을 trim해 돌려준다. 없으면 `None`(Orca 정규식 `/^gitdir:\s*(.+)\s*$/m` 동치).
fn parse_gitdir_pointer(contents: &str) -> Option<&str> {
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("gitdir:") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// 진행 중인 충돌 연산을 판별한다. `<git-dir>`의 마커를 **정해진 우선순위**로 본다
/// (Orca status.ts:942-951):
///
/// 1. `MERGE_HEAD`(파일) → `Merge`
/// 2. `rebase-merge/` **또는** `rebase-apply/`(디렉터리) → `Rebase`
/// 3. `CHERRY_PICK_HEAD`(파일) → `CherryPick`
/// 4. 아무것도 없음 → `Unknown`
///
/// **우선순위가 crux다.** 예컨대 abort된 merge가 남긴 `CHERRY_PICK_HEAD`보다 `MERGE_HEAD`가
/// 먼저 검사돼야 한다 — 순서를 뒤집는 mutation은 두 마커가 모두 있는 테스트에서 FAIL한다.
pub fn detect_conflict_operation(worktree: &Path) -> io::Result<ConflictOperation> {
    let git_dir = resolve_git_dir(worktree)?;
    // Orca와 동일하게 존재 여부만 본다(존재 = 진행 중). exists()는 접근 오류를 false로
    // 접지만 Orca의 catch→'unknown'과 결과가 같다(마커 없음 → Unknown).
    if git_dir.join("MERGE_HEAD").exists() {
        return Ok(ConflictOperation::Merge);
    }
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        return Ok(ConflictOperation::Rebase);
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Ok(ConflictOperation::CherryPick);
    }
    Ok(ConflictOperation::Unknown)
}

/// 진행 중인 merge를 중단한다 — `git merge --abort`(Orca `abortMerge`, status.ts:954).
/// `GitRunner`를 거쳐 타임아웃/출력상한/`GIT_TERMINAL_PROMPT=0` 규율을 물려받는다.
pub async fn abort_merge(runner: &GitRunner, worktree: &Path) -> Result<(), GitError> {
    runner.run(worktree, &["merge", "--abort"]).await?;
    Ok(())
}

/// 진행 중인 rebase를 중단한다 — `git rebase --abort`(Orca `abortRebase`, status.ts:963).
pub async fn abort_rebase(runner: &GitRunner, worktree: &Path) -> Result<(), GitError> {
    runner.run(worktree, &["rebase", "--abort"]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_gitdir_pointer;

    // 포인터 파싱 순수 테스트. linked worktree `.git` 파일의 실제 형태:
    // "gitdir: /abs/path/.git/worktrees/name\n".
    #[test]
    fn parses_absolute_gitdir_pointer() {
        let contents = "gitdir: /home/u/repo/.git/worktrees/fix\n";
        assert_eq!(
            parse_gitdir_pointer(contents),
            Some("/home/u/repo/.git/worktrees/fix")
        );
    }

    // 상대 경로 포인터도 값 그대로 뽑는다(해석은 resolve_git_dir이 worktree 기준으로).
    #[test]
    fn parses_relative_gitdir_pointer_and_trims() {
        assert_eq!(
            parse_gitdir_pointer("gitdir: ../.git/worktrees/fix  \n"),
            Some("../.git/worktrees/fix")
        );
    }

    // `gitdir:` 라인이 없으면 None(일반 repo의 `.git`은 디렉터리라 여기 안 온다).
    #[test]
    fn no_pointer_returns_none() {
        assert_eq!(parse_gitdir_pointer("not a pointer file\n"), None);
        assert_eq!(parse_gitdir_pointer(""), None);
    }
}
