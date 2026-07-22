use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

/// `GhRunner`는 suaegi-git의 `GitRunner`(`crates/suaegi-git/src/runner.rs`)를 그대로
/// 미러한다 — `Command::new("gh")`, 같은 타임아웃/킬/거둠 규율. 멈춘 gh가 UI를
/// 막으면 안 되기 때문이다(플랜 §3.1, 조사 §4.4). 별도 크레이트라 GitRunner의
/// 비공개 `run_full`을 재사용할 수 없어 구조를 복제한다 — 의도된 중복이다.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// gh 생성(`pr create`)은 네트워크 왕복이라 좀 더 넉넉하게(Orca도 60초).
pub const CREATE_TIMEOUT: Duration = Duration::from_secs(60);

/// 한 번의 gh 호출이 stdout+stderr를 합쳐 담을 수 있는 바이트 상한. gh --json
/// 출력은 작지만, GitRunner와 같은 규율로 폭주를 막는다.
pub const MAX_OUTPUT_BYTES: usize = 6 * 1024 * 1024;

const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);
const READ_CHUNK: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub struct GhOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum GhError {
    #[error("gh io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("gh {args} timed out")]
    Timeout { args: String },
    #[error("gh {args} failed (code {code:?}): {stderr}")]
    Failed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("gh output exceeded {limit} bytes")]
    OutputTooLarge { limit: usize },
}

impl GhError {
    /// gh 자체를 실행조차 못한 경우(PATH에 gh 없음). classify가 이걸로 NotInstalled를
    /// 구분한다 — stderr 문자열이 아니라 spawn의 구조화된 ENOENT를 본다.
    pub fn is_gh_not_found(&self) -> bool {
        matches!(self, GhError::Io(e) if e.kind() == std::io::ErrorKind::NotFound)
    }
}

#[derive(Debug, Clone, Default)]
pub struct GhRunner;

enum RunAbort {
    Io(std::io::Error),
    TooLarge,
}

fn kill_process_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

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
        if budget.fetch_add(n, Ordering::Relaxed) + n > MAX_OUTPUT_BYTES {
            return Err(RunAbort::TooLarge);
        }
        sink.extend_from_slice(&chunk[..n]);
    }
}

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

async fn reap(child: &mut Child) {
    let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
}

impl GhRunner {
    pub fn new() -> Self {
        Self
    }

    /// 성공(exit 0)만 Ok. 비-0은 `GhError::Failed`(stderr 포함) — 호출자가
    /// "no PR" 같은 예상 실패를 stderr로 분류한다.
    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<GhOutput, GhError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, &[]).await
    }

    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<GhOutput, GhError> {
        self.run_full(cwd, args, timeout, &[]).await
    }

    /// `extra_ok_codes`에 든 exit code는 성공처럼 Ok로 돌려준다(`gh pr checks`가
    /// 실패 체크에 대해 비-0를 내는 것 등). GitRunner의 `run_expecting` 미러.
    pub async fn run_expecting(
        &self,
        cwd: &Path,
        args: &[&str],
        extra_ok_codes: &[i32],
    ) -> Result<GhOutput, GhError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes)
            .await
    }

    async fn run_full(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
        extra_ok_codes: &[i32],
    ) -> Result<GhOutput, GhError> {
        let args_str = args.join(" ");
        // gh를 PATH로 해석한다(절대경로 하드코딩 금지) — 테스트가 PATH 앞에 얹는
        // 스크립트 fake gh가 그대로 잡히도록(플랜 §5).
        let mut cmd = Command::new("gh");
        cmd.args(args)
            .current_dir(cwd)
            // stderr 분류(§3.3)가 영어 로케일에 의존하므로 gh에도 이어야 한다.
            .env("LC_ALL", "C")
            // gh가 어떤 write op에서도 대화형 프롬프트로 행 걸리지 않도록[Codex S4].
            .env("GH_PROMPT_DISABLED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn()?;
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let budget = AtomicUsize::new(0);

        let waited = tokio::time::timeout(timeout, async {
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
                return Err(GhError::Timeout { args: args_str });
            }
            Ok(Ok(status)) => status,
            Ok(Err(abort)) => {
                kill_process_tree(&mut child);
                let _ = tokio::time::timeout(DRAIN_TIMEOUT, async {
                    tokio::join!(drain(stdout_pipe.as_mut()), drain(stderr_pipe.as_mut()));
                })
                .await;
                reap(&mut child).await;
                return Err(match abort {
                    RunAbort::TooLarge => GhError::OutputTooLarge {
                        limit: MAX_OUTPUT_BYTES,
                    },
                    RunAbort::Io(e) => GhError::Io(e),
                });
            }
        };

        let code = status.code().unwrap_or(-1);
        if !status.success() && !extra_ok_codes.contains(&code) {
            return Err(GhError::Failed {
                args: args_str,
                code: status.code(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            });
        }
        Ok(GhOutput {
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            code,
        })
    }
}
