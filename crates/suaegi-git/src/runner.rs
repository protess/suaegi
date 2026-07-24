use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 한 번의 git 호출이 stdout과 stderr를 **합쳐** 메모리에 담을 수 있는 상한.
/// **바이트다. 문자가 아니다** — `String::len()`이 세는 것과 같은 단위라서
/// "6M자"라고 읽으면 UTF-8 멀티바이트에서 최대 4배까지 어긋난다.
pub const MAX_DIFF_BYTES: usize = 6 * 1024 * 1024;

/// 넘침을 만난 뒤 파이프를 배출할 때의 상한. 킬 직후라 즉시 EOF가 나야 정상이지만
/// 손자 프로세스가 쓰기 끝을 붙들고 있으면 영영 안 닫힌다.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// 죽인 자식을 거두며 기다릴 상한. `reap` 참고.
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

const READ_CHUNK: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// lossy 변환 **전의** 출력. 바이너리 판정(NUL 스니핑)과 파일 내용 읽기는
/// `String::from_utf8_lossy`를 통과하면 안 된다 — 임의의 바이트가 U+FFFD로
/// 뭉개져 원본을 복원할 수 없다.
#[derive(Debug, Clone)]
pub struct GitBytes {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git {args} timed out")]
    Timeout { args: String },
    #[error("git {args} failed (code {code:?}): {stderr}")]
    Failed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("git {args} produced unparseable output: {detail}")]
    Parse { args: String, detail: String },
    #[error("git output exceeded {limit} bytes")]
    OutputTooLarge { limit: usize },
}

#[derive(Debug, Clone, Default)]
pub struct GitRunner;

/// 모든 git 호출에 붙는 credential-prompt 가드 config(F2). `-c key=val`을 subcommand
/// **앞에** 끼우는 argv-순서 버그 클래스를 피하려고 `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/
/// `GIT_CONFIG_VALUE_n` env 프로토콜로 주입한다(Orca `appendGitConfigEnv`,
/// `git-credential-prompt-env.ts:62-79`). credential **helper는 유지**하고 대화형
/// fallback만 끈다 — 캐시된 credential은 계속 동작한다.
const CREDENTIAL_PROMPT_GUARD_CONFIG: &[(&str, &str)] = &[
    ("credential.interactive", "false"),
    ("credential.guiPrompt", "false"),
];

/// 모든 git 호출에 적용하는 비대화형 env(F1). Orca `nonInteractiveGitEnv`처럼 **전역**
/// 정책이다 — 로컬 ops엔 무해(credential 미접촉)하고 원격 ops는 credential 프롬프트로
/// 행 걸리는 대신 **빠르게 실패**한다.
///
/// 순수 함수라 구조 테스트로 mutation 검증할 수 있다. `run_full`이 spawn 전에 이 목록을
/// 그대로 `Command::env`로 흘려 넣는다.
///
/// - `GIT_ASKPASS`/`SSH_ASKPASS`를 비워 askpass 헬퍼가 GUI를 못 띄우게 한다.
/// - `GIT_SSH_COMMAND`에 `BatchMode=yes`로 SSH가 비밀번호 프롬프트 대신 즉시 실패하게 한다.
/// - `GCM_INTERACTIVE=never`로 Git Credential Manager의 자체 GUI를 막는다.
/// - `credential.interactive=false`/`guiPrompt=false`를 GIT_CONFIG env 프로토콜로 주입한다.
///
/// **credential helper·사용자 전역 gitconfig는 절대 건드리지 않는다** — 대화형 UI만 끈다.
pub fn non_interactive_git_env() -> Vec<(String, String)> {
    let mut env = vec![
        // 파서가 항상 영어 출력을 보도록
        ("LC_ALL".to_string(), "C".to_string()),
        // 터미널 인증 프롬프트로 행 걸리지 않도록
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        // askpass 헬퍼가 GUI를 못 띄우게 비운다
        ("GIT_ASKPASS".to_string(), String::new()),
        ("SSH_ASKPASS".to_string(), String::new()),
        // SSH가 프롬프트 대신 즉시 실패하도록
        (
            "GIT_SSH_COMMAND".to_string(),
            "ssh -o BatchMode=yes".to_string(),
        ),
        // GCM은 터미널/askpass 가드를 무시하고 자체 GUI를 열 수 있다
        ("GCM_INTERACTIVE".to_string(), "never".to_string()),
    ];
    // GIT_CONFIG_COUNT/KEY_n/VALUE_n 프로토콜. base=0에서 시작하지만, 미래에 config를
    // 더할 때 count가 합성되도록 슬라이스 길이로 계산한다(고정 상수 대신).
    env.push((
        "GIT_CONFIG_COUNT".to_string(),
        CREDENTIAL_PROMPT_GUARD_CONFIG.len().to_string(),
    ));
    for (i, (key, value)) in CREDENTIAL_PROMPT_GUARD_CONFIG.iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), (*key).to_string()));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), (*value).to_string()));
    }
    env
}

/// 리더가 바깥 상태기계에 보고하는 중단 사유. **리더는 킬하지 않는다** —
/// `child.wait()`가 `child`를 가변 대여하고 있어 조인 안에서는 킬할 수 없다.
enum RunAbort {
    Io(std::io::Error),
    TooLarge,
}

/// Unix: git이 스폰한 hook/LFS/credential helper까지 함께 죽도록 프로세스 그룹
/// 전체에 SIGKILL. Windows: git 프로세스만 (job object는 post-MVP 한계).
fn kill_process_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// 상한을 **읽는 도중에** 센다. `read_to_end`로 EOF까지 읽고 나서 길이를 보면
/// 검사 시점에 이미 할당이 끝난 뒤라 상한이 아무것도 막지 못한다.
///
/// `budget`은 두 파이프가 공유한다 — 스트림마다 따로 두면 실제 상주 메모리가
/// 상한의 두 배가 된다.
async fn read_capped<R: AsyncRead + Unpin>(
    pipe: Option<&mut R>,
    sink: &mut Vec<u8>,
    budget: &AtomicUsize,
) -> Result<(), RunAbort> {
    let Some(pipe) = pipe else {
        return Ok(());
    };
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let n = pipe.read(&mut chunk).await.map_err(RunAbort::Io)?;
        if n == 0 {
            return Ok(());
        }
        if budget.fetch_add(n, Ordering::Relaxed) + n > MAX_DIFF_BYTES {
            return Err(RunAbort::TooLarge);
        }
        sink.extend_from_slice(&chunk[..n]);
    }
}

/// 남은 바이트를 버리며 읽는다. 담지 않으므로 상한과 무관하다.
async fn drain<R: AsyncRead + Unpin>(pipe: Option<&mut R>) {
    let Some(pipe) = pipe else {
        return;
    };
    let mut chunk = [0u8; READ_CHUNK];
    while let Ok(n) = pipe.read(&mut chunk).await {
        if n == 0 {
            break;
        }
    }
}

/// 자식 stdin에 전부 쓰고 **쓰기 끝을 닫아 EOF를 보낸다.** 읽기(`read_capped`)와
/// **동시에** 돌려야 한다 — 순차로 하면 고전적 교착이다: 자식이 stdout 파이프가
/// 가득 차 write에 블록되면 더 이상 stdin을 읽지 않고, 그러면 우리 `write_all`이
/// 영영 안 끝난다. `run_full`은 이 future를 `read_capped`들과 함께 `try_join!`에
/// 넣어 그 교착을 없앤다.
///
/// **EPIPE(BrokenPipe)는 오류가 아니다.** 자식이 우리 입력을 다 읽기 전에 끝낼 수
/// 있다(예: `check-ignore`가 조기 종료). 그건 자식의 선택이고 진실은 종료 코드에
/// 있으므로, 여기서 죽이지 않고 조용히 멈춘다. 종료 코드 판정은 호출부가 한다.
///
/// 파이프를 **값으로 받아** future가 끝나면 떨어지게 한다 — `shutdown` 뒤 drop까지
/// 겹쳐 fd가 확실히 닫힌다.
async fn write_stdin(pipe: Option<ChildStdin>, data: Option<&[u8]>) -> Result<(), RunAbort> {
    let (Some(mut pipe), Some(data)) = (pipe, data) else {
        return Ok(());
    };
    match pipe.write_all(data).await {
        Ok(()) => {
            // flush + 쓰기 끝 닫기 → git이 EOF를 본다. 여기서도 BrokenPipe는 무해.
            if let Err(e) = pipe.shutdown().await {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(RunAbort::Io(e));
                }
            }
            Ok(())
        }
        // 자식이 먼저 읽기를 멈췄다 — 정상. 나머지는 종료 코드가 말한다.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(RunAbort::Io(e)),
    }
}

/// 죽인 자식을 거둔다. **기다림에 상한을 둔다** — 킬이 통하지 않는 경우(Windows의
/// 손자, 저지 불가 상태의 자식)에 `wait()`는 영영 돌아오지 않고, 그러면 이 함수를
/// 부른 git 호출이 **자기 타임아웃보다 오래** 매달린다. mutation으로 실측한 것도
/// 정확히 이 모양이었다: 킬을 지우니 테스트가 실패하는 대신 300초를 매달렸다.
///
/// 상한을 넘기면 `Child`를 그대로 떨군다. `kill_on_drop(true)`이 걸려 있어
/// tokio의 백그라운드 리퍼가 이어서 거둔다 — 좀비로 남기는 것보다 낫고,
/// 무한정 매달리는 것보다는 훨씬 낫다.
async fn reap(child: &mut Child) {
    let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
}

impl GitRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, &[], None)
            .await
            .map(lossy)
    }

    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, timeout, &[], None)
            .await
            .map(lossy)
    }

    pub async fn run_expecting(
        &self,
        cwd: &Path,
        args: &[&str],
        extra_ok_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes, None)
            .await
            .map(lossy)
    }

    /// lossy 변환을 하지 않는 경로. 파일 내용을 바이트로 봐야 하는 호출자용.
    pub async fn run_bytes(
        &self,
        cwd: &Path,
        args: &[&str],
        extra_ok_codes: &[i32],
    ) -> Result<GitBytes, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes, None)
            .await
    }

    /// git의 stdin에 `stdin` 바이트를 먹인다. `git check-ignore --stdin`처럼
    /// **positional 인자로는 경로 수 폭발/인자 길이 한계에 걸리는** 호출용
    /// (`check-ignored-paths.ts:18-30`).
    ///
    /// 쓰기와 읽기를 **동시에** 돌려 교착을 피한다(`write_stdin` 참고). 출력 상한·
    /// 타임아웃·프로세스 트리 킬은 다른 경로와 똑같이 적용된다. `extra_ok_codes`는
    /// `run_expecting`과 같은 의미다 — check-ignore의 **exit 1("무시된 것 없음")을
    /// 오류가 아닌 성공으로** 받으려면 `&[1]`을 넘긴다.
    pub async fn run_with_stdin(
        &self,
        cwd: &Path,
        args: &[&str],
        stdin: &[u8],
        extra_ok_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes, Some(stdin))
            .await
            .map(lossy)
    }

    async fn run_full(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
        extra_ok_codes: &[i32],
        stdin: Option<&[u8]>,
    ) -> Result<GitBytes, GitError> {
        let args_str = args.join(" ");
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(cwd);
        // 비대화형 env 가드(F1/F2)를 **모든** 호출에 전역 적용한다. 로컬 ops엔 무해,
        // 원격 ops는 credential 프롬프트로 행 걸리는 대신 빠르게 실패한다.
        for (key, value) in non_interactive_git_env() {
            cmd.env(key, value);
        }
        cmd
            // stdin을 줄 때만 파이프한다. 없으면 기존 그대로 null — 인증 프롬프트로
            // 행 걸리지 않게 하는 불변식을 유지한다.
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0); // 타임아웃 시 그룹 전체 킬 가능하게

        let mut child = cmd.spawn()?;
        let stdin_pipe = child.stdin.take();
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let budget = AtomicUsize::new(0);

        let waited = tokio::time::timeout(timeout, async {
            // stdin 쓰기와 stdout/stderr 읽기를 **모두 동시에** 돌린다 — 어느 하나를
            // 순차로 하면 반대쪽 파이프가 가득 차 자식이 블록되는 교착이 난다
            // (stdin write vs stdout read 교착 포함, `write_stdin` 참고).
            let read_out = read_capped(stdout_pipe.as_mut(), &mut out, &budget);
            let read_err = read_capped(stderr_pipe.as_mut(), &mut err, &budget);
            let write_in = write_stdin(stdin_pipe, stdin);
            let (status, _, _, _) = tokio::try_join!(
                async { child.wait().await.map_err(RunAbort::Io) },
                read_out,
                read_err,
                write_in
            )?;
            Ok::<_, RunAbort>(status)
        })
        .await;

        let status = match waited {
            Err(_) => {
                kill_process_tree(&mut child);
                reap(&mut child).await;
                return Err(GitError::Timeout { args: args_str });
            }
            Ok(Ok(status)) => status,
            Ok(Err(abort)) => {
                // 여기 왔다는 것은 `try_join!`이 나머지 future를 떨궜다는 뜻이고,
                // 그래서 `child.wait()`의 가변 대여가 **이제야** 끝났다. 리더 안에서
                // 킬할 수 없었던 이유가 이것이다.
                //
                // `kill_on_drop(true)`은 종료를 요청할 뿐 수확이 아니다 — 명시적으로
                // 죽이고, 배출하고, `wait()`으로 거둬야 좀비가 남지 않는다.
                //
                // **배출을 지우지 마라. 테스트로는 못 지키는 자리다.**
                // mutation으로 확인했다: 이 배출을 지워도 테스트는 전부 통과한다.
                // 킬이 먹히는 정상 경로에서는 아무도 파이프를 붙들지 않고,
                // `child.wait()`은 이미 죽은 직계 자식만 거두면 되기 때문이다.
                //
                // 그러나 `reap`의 주석이 적어 놓았듯 **킬이 안 먹힐 수 있다.**
                // 그때 자식은 살아 있고, 우리가 읽기를 멈춘 파이프는 가득 차 있고,
                // 자식은 write에 블록돼 **영영 끝나지 못한다** — `reap`의 5초가
                // 그대로 날아간다. 배출은 정확히 그 경우를 위한 보험이다.
                // 킬이 실패할 수 있다는 사실 자체가 실측으로 나온 것이라
                // (`reap` 참고), 이 경로는 "도달 불가"가 아니라 "테스트로 만들기
                // 어려울 뿐"이다.
                kill_process_tree(&mut child);
                let _ = tokio::time::timeout(DRAIN_TIMEOUT, async {
                    tokio::join!(drain(stdout_pipe.as_mut()), drain(stderr_pipe.as_mut()));
                })
                .await;
                reap(&mut child).await;
                return Err(match abort {
                    RunAbort::TooLarge => GitError::OutputTooLarge {
                        limit: MAX_DIFF_BYTES,
                    },
                    RunAbort::Io(e) => GitError::Io(e),
                });
            }
        };

        let code = status.code().unwrap_or(-1);
        if !status.success() && !extra_ok_codes.contains(&code) {
            return Err(GitError::Failed {
                args: args_str,
                code: status.code(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            });
        }
        Ok(GitBytes {
            stdout: out,
            stderr: err,
            code,
        })
    }
}

fn lossy(bytes: GitBytes) -> GitOutput {
    GitOutput {
        stdout: String::from_utf8_lossy(&bytes.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&bytes.stderr).into_owned(),
        code: bytes.code,
    }
}

#[cfg(test)]
mod env_guard_tests {
    use super::*;

    fn find<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// 전역 비대화형 가드가 4개 프롬프트-억제 var를 모두 세팅하는지(F1). 어느 하나를
    /// 지우는 mutation은 그 var의 단언에서 실패한다.
    #[test]
    fn guard_sets_all_prompt_suppressors() {
        let env = non_interactive_git_env();
        assert_eq!(find(&env, "LC_ALL"), Some("C"));
        assert_eq!(find(&env, "GIT_TERMINAL_PROMPT"), Some("0"));
        // askpass 헬퍼는 **비워야** GUI를 못 띄운다 — 없으면(None) mutation.
        assert_eq!(find(&env, "GIT_ASKPASS"), Some(""));
        assert_eq!(find(&env, "SSH_ASKPASS"), Some(""));
        assert_eq!(find(&env, "GIT_SSH_COMMAND"), Some("ssh -o BatchMode=yes"));
        assert_eq!(find(&env, "GCM_INTERACTIVE"), Some("never"));
    }

    /// GIT_CONFIG env 프로토콜(F2)이 정합적인지: COUNT == 쌍 개수이고 각 KEY_n/VALUE_n이
    /// credential.interactive=false / guiPrompt=false를 담는다. COUNT를 어긋내거나 키를
    /// 바꾸는 mutation은 여기서 실패하고, 실제 git도 count 불일치로 config를 거부한다.
    #[test]
    fn guard_config_env_protocol_is_consistent() {
        let env = non_interactive_git_env();
        // 고정 상수가 아니라 슬라이스 길이로 계산돼야 확장 시 합성된다.
        assert_eq!(find(&env, "GIT_CONFIG_COUNT"), Some("2"));
        assert_eq!(
            find(&env, "GIT_CONFIG_KEY_0"),
            Some("credential.interactive")
        );
        assert_eq!(find(&env, "GIT_CONFIG_VALUE_0"), Some("false"));
        assert_eq!(find(&env, "GIT_CONFIG_KEY_1"), Some("credential.guiPrompt"));
        assert_eq!(find(&env, "GIT_CONFIG_VALUE_1"), Some("false"));
        // credential.helper는 절대 세팅하지 않는다 — 캐시된 credential 유지.
        assert!(
            !env.iter().any(|(k, _)| k == "GIT_CONFIG_KEY_2"),
            "M1은 정확히 2쌍이어야 한다 (dangling index는 git이 거부)"
        );
    }
}
