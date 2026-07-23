//! M1 — 스테이징 write-ops: `stage`/`unstage`(single) + `bulk_stage`/`bulk_unstage`.
//!
//! Orca `status.ts`의 `stageFile`(:1882)/`unstageFile`(:1901)/`bulkStageFiles`(:2173)/
//! `bulkUnstageFiles`(:2198) 포팅. 전부 `GitRunner`를 거쳐 그 타임아웃/출력상한/
//! `GIT_TERMINAL_PROMPT=0` 규율을 물려받는다 — raw `Command` 금지. 사용자 전역 config는
//! 절대 미접촉이다(`GitRunner`가 identity/`-c` override를 안 붙인다 — 여기서도 안 붙인다).
//!
//! **`:(literal)` pathspec이 핵심이다.** `a*.txt`처럼 glob 문자를 담거나 `-n`처럼
//! 플래그로 보이는 파일명을 git이 오해하지 않도록 리터럴로 못 박는다(실측: 바 `a*.txt`는
//! `a1.txt`까지 스테이징하지만 `:(literal)a*.txt`는 그 파일 하나만).

use crate::runner::{GitError, GitRunner};
use std::path::Path;

/// 한 번의 `git add`/`restore` 호출에 실을 pathspec 최대 개수. 경로가 많으면 argv가
/// `E2BIG`를 치므로 이 크기로 청크한다(Orca `BULK_CHUNK_SIZE`, status.ts:63 = 100).
pub(crate) const BULK_CHUNK_SIZE: usize = 100;

/// WSL 아래 git은 POSIX 경로를 원하지만 호스트 경로는 리터럴로 유지해야 하므로
/// **백슬래시만** 슬래시로 바꾼다(Orca `literalPathspec`, status.ts:2043).
///
/// suaegi는 아직 WSL distro 런타임 옵션을 배선하지 않는다(macOS-first) — 그래서
/// 런타임 경로에서 이 재작성은 항상 꺼져 있고, 규칙 자체는 `literal_pathspec_impl`에
/// 순수 함수로 구현해 win32-style 입력으로 직접 테스트한다. Windows/WSL 런타임에서
/// 이 플래그를 켜는 배선은 follow-up이다(`cfg!(windows)`를 stand-in으로 둔다).
const REWRITE_BACKSLASH: bool = cfg!(windows);

/// `<path>` → `:(literal)<path>`. glob/플래그로 보이는 파일명을 git이 리터럴로 다루게
/// 강제한다. M4 discard도 재사용하므로 `pub(crate)`로 노출한다.
pub(crate) fn literal_pathspec(path: &str) -> String {
    literal_pathspec_impl(path, REWRITE_BACKSLASH)
}

/// `literal_pathspec`의 순수 구현. `rewrite_backslash`가 true면 백슬래시→슬래시
/// (WSL 규칙)를 적용한다. 플랫폼과 무관하게 규칙을 직접 테스트할 수 있게 분리했다.
fn literal_pathspec_impl(path: &str, rewrite_backslash: bool) -> String {
    if rewrite_backslash {
        format!(":(literal){}", path.replace('\\', "/"))
    } else {
        format!(":(literal){path}")
    }
}

/// 한 경로를 스테이징한다 — `git add -- :(literal)<path>`.
pub async fn stage(runner: &GitRunner, worktree: &Path, path: &str) -> Result<(), GitError> {
    let spec = literal_pathspec(path);
    runner.run(worktree, &["add", "--", &spec]).await?;
    Ok(())
}

/// 한 경로를 언스테이징한다 — `git restore --staged -- :(literal)<path>`.
pub async fn unstage(runner: &GitRunner, worktree: &Path, path: &str) -> Result<(), GitError> {
    let spec = literal_pathspec(path);
    runner
        .run(worktree, &["restore", "--staged", "--", &spec])
        .await?;
    Ok(())
}

/// 여러 경로를 청크(`BULK_CHUNK_SIZE`) 단위로 스테이징한다 — 각 청크가 한 번의
/// `git add -- <specs...>` 호출.
///
/// **비-트랜잭션(plan F5).** 청크는 원자적이다: `git add`는 한 pathspec이라도 아무것도
/// 매칭 못 하면 청크 전체를 실패시키고 그 청크의 어떤 경로도 스테이징하지 않는다(실측).
/// 그래서 `Result<(), E>` 대신 **입력 경로별 결과 벡터**를 돌려준다:
/// - 성공한 청크의 경로들 → 각 `Ok(())`.
/// - 실패한 청크의 경로들 → 각 그 청크의 에러(복제)를 `Err`로. 다른 청크는 영향 없다.
///
/// 반환 벡터는 입력과 **같은 순서·같은 길이**다(경로마다 정확히 한 결과).
pub async fn bulk_stage(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
) -> Vec<(String, Result<(), GitError>)> {
    bulk_apply(runner, worktree, paths, &["add", "--"]).await
}

/// 여러 경로를 청크 단위로 언스테이징한다 — `git restore --staged -- <specs...>`.
/// 세맨틱은 `bulk_stage`와 동일하다(per-path 결과 벡터, 청크 원자성).
pub async fn bulk_unstage(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
) -> Vec<(String, Result<(), GitError>)> {
    bulk_apply(runner, worktree, paths, &["restore", "--staged", "--"]).await
}

/// `bulk_stage`/`bulk_unstage`의 공통 청크 실행. `prefix`는 pathspec 앞에 오는 고정
/// argv(`["add", "--"]` 또는 `["restore", "--staged", "--"]`).
async fn bulk_apply(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
    prefix: &[&str],
) -> Vec<(String, Result<(), GitError>)> {
    let mut results = Vec::with_capacity(paths.len());
    for chunk in paths.chunks(BULK_CHUNK_SIZE) {
        // pathspec 문자열은 argv가 참조하는 동안 살아 있어야 한다 — 먼저 소유로 만든다.
        let specs: Vec<String> = chunk.iter().map(|p| literal_pathspec(p)).collect();
        let mut args: Vec<&str> = prefix.to_vec();
        args.extend(specs.iter().map(String::as_str));

        match runner.run(worktree, &args).await {
            Ok(_) => {
                for p in chunk {
                    results.push((p.to_string(), Ok(())));
                }
            }
            // 청크가 원자적으로 실패했다 — 그 청크의 모든 경로에 같은 에러를 준다.
            // `GitError`는 `Io` variant 때문에 `Clone`이 아니라 `duplicate_error`로 복제한다.
            Err(e) => {
                for p in chunk {
                    results.push((p.to_string(), Err(duplicate_error(&e))));
                }
            }
        }
    }
    results
}

/// `GitError`를 값-복제한다. `GitError`는 `Io(std::io::Error)` variant를 담아
/// `#[derive(Clone)]`이 안 되므로, per-path 결과에 같은 에러를 여러 벌 실으려면
/// variant별로 손수 복제한다. `Io`는 kind+메시지를 보존해 새 `io::Error`로 재구성한다.
fn duplicate_error(e: &GitError) -> GitError {
    match e {
        GitError::Io(io) => GitError::Io(std::io::Error::new(io.kind(), io.to_string())),
        GitError::Timeout { args } => GitError::Timeout { args: args.clone() },
        GitError::Failed { args, code, stderr } => GitError::Failed {
            args: args.clone(),
            code: *code,
            stderr: stderr.clone(),
        },
        GitError::Parse { args, detail } => GitError::Parse {
            args: args.clone(),
            detail: detail.clone(),
        },
        GitError::OutputTooLarge { limit } => GitError::OutputTooLarge { limit: *limit },
    }
}

#[cfg(test)]
mod tests {
    use super::{literal_pathspec, literal_pathspec_impl};

    // literal_pathspec: 평범한 경로 → :(literal) 접두.
    #[test]
    fn plain_path_gets_literal_prefix() {
        assert_eq!(literal_pathspec("foo.rs"), ":(literal)foo.rs");
    }

    // glob 문자/선행 대시가 담긴 파일명도 그대로 리터럴로 감싼다 — 접두를 떼면
    // git이 glob/플래그로 오해한다(그게 :(literal)의 존재 이유). 접두를 지우는 mutation은
    // 이 단언과 real-git 라운드트립 테스트를 함께 깬다.
    #[test]
    fn glob_and_dash_names_are_literal_wrapped() {
        assert_eq!(literal_pathspec("a[1].rs"), ":(literal)a[1].rs");
        assert_eq!(literal_pathspec("a*.txt"), ":(literal)a*.txt");
        assert_eq!(literal_pathspec("-n"), ":(literal)-n");
    }

    // WSL 규칙: rewrite_backslash=true면 win32-style 입력의 백슬래시가 슬래시로.
    // macOS 런타임에선 플래그가 꺼져 있어(REWRITE_BACKSLASH=false) 순수 impl을 직접 친다.
    #[test]
    fn wsl_rule_rewrites_backslashes() {
        assert_eq!(
            literal_pathspec_impl(r"src\main\a.rs", true),
            ":(literal)src/main/a.rs"
        );
        // 규칙이 꺼져 있으면 백슬래시를 보존한다.
        assert_eq!(
            literal_pathspec_impl(r"src\main\a.rs", false),
            r":(literal)src\main\a.rs"
        );
    }
}
