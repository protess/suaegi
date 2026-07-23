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

// --- M2: 커밋 (`commit_changes`) ---

/// `commit_changes`의 결과. Orca `commitChanges`의 `{ success, error? }`를 모델링한다
/// (`status.ts:1962-1990`). git이 **돌긴 했으나** 실패한 경우(hook/GPG 거부, empty index)를
/// 담는다 — git을 **아예 못 돌린** 경우(spawn/timeout)는 `commit_changes`가 `GitError`로
/// 돌려주는, 이와 **별개의** 실패다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// 커밋 성공(exit 0).
    Committed,
    /// git이 돌았으나 커밋을 거부/중단했다(non-zero exit). `message`는 채널 우선순위
    /// 규칙(stderr→stdout→generic)으로 고른, 사람이 읽을 사유다.
    Failed { message: String },
}

/// 커밋 실패 시 사람에게 보일 메시지를 고른다 — **stderr → stdout → generic**
/// (Orca `status.ts:1972-1986`). hook/GPG 실패는 stderr로, "nothing to commit"은 stdout으로
/// 나오므로 **비어 있지 않은** stderr를 먼저, 없으면 stdout을, 둘 다 비면 generic fallback.
///
/// **순수 함수 — 이 마일스톤의 핵심 crux.** 우선순위를 뒤집거나(stdout 먼저) 한 채널을
/// 지우는 mutation은 unit 테스트가 잡는다.
fn pick_commit_error(stdout: &str, stderr: &str) -> String {
    if !stderr.is_empty() {
        stderr.to_string()
    } else if !stdout.is_empty() {
        stdout.to_string()
    } else {
        "Commit failed".to_string()
    }
}

/// `(stdout, stderr, exit code)`를 `CommitOutcome`으로 분류하는 순수 함수. exit 0이면
/// `Committed`, 아니면 `Failed`(메시지는 `pick_commit_error`). code가 load-bearing이라
/// "non-zero를 Committed로" 뒤집는 mutation을 empty-index 테스트가 잡는다.
fn classify_commit(stdout: &str, stderr: &str, code: i32) -> CommitOutcome {
    if code == 0 {
        CommitOutcome::Committed
    } else {
        CommitOutcome::Failed {
            message: pick_commit_error(stdout, stderr),
        }
    }
}

/// 스테이징된 변경을 커밋한다 — `git commit -m <message>`.
///
/// **F3/F4 불변식(plan §1):** `-c user.name/user.email`, `commit.gpgsign`, `--no-verify`,
/// 전역 config 접촉 — **어느 것도 하지 않는다.** 실 유저로 그의 repo에 bare 커밋한다
/// (identity override는 서명 제거+가짜 author 회귀, `--no-verify`는 에이전트의 조용한
/// hook 우회다). `message`는 **별개 argv 원소**로 넘겨(절대 shell 보간 없음) 선행 대시나
/// 셸 메타문자가 담긴 메시지도 리터럴로 커밋된다.
///
/// - exit 0 → `Ok(Committed)`.
/// - git이 돌았으나 non-zero(예: empty index는 exit 1 + stdout "nothing to commit") →
///   `Ok(Failed { message })`. 이건 커밋 실패이지 crate 에러가 아니다.
/// - git을 아예 못 돌림(spawn 실패/타임아웃/출력 초과) → `Err(GitError)`.
pub async fn commit_changes(
    runner: &GitRunner,
    worktree: &Path,
    message: &str,
) -> Result<CommitOutcome, GitError> {
    // exit 1은 "돌긴 했으나 실패"의 흔한 코드다("nothing to commit"은 stdout, hook/GPG는
    // stderr). `run_expecting(&[1])`로 exit 1을 에러가 아닌 성공으로 받아 **양쪽 채널과
    // exit code**를 그대로 손에 넣는다 — `GitError::Failed`는 stderr만 담고 stdout·code를
    // 버려 "nothing to commit"을 잃기 때문이다.
    match runner
        .run_expecting(worktree, &["commit", "-m", message], &[1])
        .await
    {
        Ok(out) => Ok(classify_commit(&out.stdout, &out.stderr, out.code)),
        // git이 돌았으나 예상 밖 non-zero(예: 128 fatal). 여전히 "돌고 실패"이므로
        // `GitError`가 아니라 `Failed`로 올린다. `GitError::Failed`는 stdout을 보존하지
        // 않아 메시지는 stderr에서 온다(그 코드들의 사유는 stderr에 실린다).
        Err(GitError::Failed { stderr, code, .. }) => {
            Ok(classify_commit("", &stderr, code.unwrap_or(-1)))
        }
        // git을 못 돌렸다(spawn/timeout/output-too-large) — 진짜 crate 에러.
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_commit, literal_pathspec, literal_pathspec_impl, pick_commit_error, CommitOutcome,
    };

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

    // --- M2 crux: 채널 우선순위 picker (순수) ---

    // stderr가 비어있지 않으면 stderr를 고른다(hook/GPG 실패는 stderr로 온다).
    // mutation "stdout 먼저"는 여기서 "nothing to commit"을 골라 FAIL.
    #[test]
    fn pick_prefers_stderr_when_present() {
        assert_eq!(
            pick_commit_error("nothing to commit", "hook failed"),
            "hook failed"
        );
    }

    // stderr가 비면 stdout으로 폴백한다("nothing to commit"은 stdout으로 온다).
    // mutation "항상 stderr(stdout 드롭)"는 generic으로 떨어져 FAIL.
    #[test]
    fn pick_falls_back_to_stdout_when_stderr_empty() {
        assert_eq!(
            pick_commit_error("nothing to commit", ""),
            "nothing to commit"
        );
    }

    // 둘 다 비면 generic fallback.
    #[test]
    fn pick_generic_when_both_empty() {
        assert_eq!(pick_commit_error("", ""), "Commit failed");
    }

    // --- M2 crux: classify (code가 load-bearing) ---

    // exit 0 → Committed. mutation "code 비교 뒤집기(non-zero→Committed)"는 아래 non-zero
    // 테스트와 empty-index 통합 테스트를 깬다.
    #[test]
    fn classify_zero_is_committed() {
        assert_eq!(classify_commit("out", "err", 0), CommitOutcome::Committed);
    }

    // non-zero → Failed(메시지는 채널 규칙). stderr 우선, 없으면 stdout.
    #[test]
    fn classify_nonzero_is_failed_with_channel_message() {
        assert_eq!(
            classify_commit("nothing to commit", "", 1),
            CommitOutcome::Failed {
                message: "nothing to commit".to_string()
            }
        );
        assert_eq!(
            classify_commit("", "hook failed", 1),
            CommitOutcome::Failed {
                message: "hook failed".to_string()
            }
        );
    }
}
