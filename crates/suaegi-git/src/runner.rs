use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

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
        self.run_full(cwd, args, DEFAULT_TIMEOUT, &[])
            .await
            .map(lossy)
    }

    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, timeout, &[]).await.map(lossy)
    }

    pub async fn run_expecting(
        &self,
        cwd: &Path,
        args: &[&str],
        extra_ok_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes)
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
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes)
            .await
    }

    async fn run_full(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
        extra_ok_codes: &[i32],
    ) -> Result<GitBytes, GitError> {
        let args_str = args.join(" ");
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(cwd)
            // 파서가 항상 영어 출력을 보도록; 인증 프롬프트로 행 걸리지 않도록
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0); // 타임아웃 시 그룹 전체 킬 가능하게

        let mut child = cmd.spawn()?;
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let budget = AtomicUsize::new(0);

        let waited = tokio::time::timeout(timeout, async {
            // stdout/stderr 동시 드레인 — 순차로 읽으면 반대쪽 파이프가 가득 차
            // 자식이 블록되는 교착 가능
            let read_out = read_capped(stdout_pipe.as_mut(), &mut out, &budget);
            let read_err = read_capped(stderr_pipe.as_mut(), &mut err, &budget);
            let (status, _, _) = tokio::try_join!(
                async { child.wait().await.map_err(RunAbort::Io) },
                read_out,
                read_err
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
