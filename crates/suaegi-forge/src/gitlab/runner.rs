use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

/// `GlabRunner`는 `GhRunner`(`crates/suaegi-forge/src/runner.rs`)를 그대로 미러한다 —
/// `Command::new("glab")`, 같은 타임아웃/킬/거둠 규율. 멈춘 glab이 UI를 막으면 안 되기
/// 때문이다(7a와 동일). GhRunner의 비공개 `run_full`을 재사용할 수 없어 구조를 복제한다 —
/// 의도된 중복이다(GhRunner가 GitRunner를 복제한 것과 같은 정신).
///
/// gh와의 유일한 차이는 **프롬프트 억제 방식**이다: gh는 `GH_PROMPT_DISABLED=1`을 쓰지만
/// glab에는 대응 env가 없다. glab은 non-tty(stdin null)에서 프롬프트를 건너뛰며, 확인이
/// 필요한 쓰기(create/merge)는 호출부가 `--yes`를 붙인다(Orca `client.ts`가 그렇게 한다).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// glab 생성/머지(`mr create`/`mr merge`)는 네트워크 왕복이라 좀 더 넉넉하게(Orca도 60초).
pub const CREATE_TIMEOUT: Duration = Duration::from_secs(60);

/// 한 번의 glab 호출이 stdout+stderr를 합쳐 담을 수 있는 바이트 상한. GhRunner와 같은
/// 규율로 폭주를 막는다.
pub const MAX_OUTPUT_BYTES: usize = 6 * 1024 * 1024;

const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);
const READ_CHUNK: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub struct GlabOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum GlabError {
    #[error("glab io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("glab {args} timed out")]
    Timeout { args: String },
    #[error("glab {args} failed (code {code:?}): {stderr}")]
    Failed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("glab output exceeded {limit} bytes")]
    OutputTooLarge { limit: usize },
}

impl GlabError {
    /// glab 자체를 실행조차 못한 경우(PATH에 glab 없음). classify가 이걸로 NotInstalled를
    /// 구분한다 — stderr 문자열이 아니라 spawn의 구조화된 ENOENT를 본다.
    pub fn is_glab_not_found(&self) -> bool {
        matches!(self, GlabError::Io(e) if e.kind() == std::io::ErrorKind::NotFound)
    }
}

#[derive(Debug, Clone, Default)]
pub struct GlabRunner;

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

impl GlabRunner {
    pub fn new() -> Self {
        Self
    }

    /// 성공(exit 0)만 Ok. 비-0은 `GlabError::Failed`(stderr 포함) — 호출자가
    /// "no MR" 같은 예상 실패를 stderr로 분류한다.
    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<GlabOutput, GlabError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, &[]).await
    }

    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<GlabOutput, GlabError> {
        self.run_full(cwd, args, timeout, &[]).await
    }

    /// `extra_ok_codes`에 든 exit code는 성공처럼 Ok로 돌려준다(`glab auth status`가
    /// 미인증에 비-0을 내는 것 등). GhRunner의 `run_expecting` 미러.
    pub async fn run_expecting(
        &self,
        cwd: &Path,
        args: &[&str],
        extra_ok_codes: &[i32],
    ) -> Result<GlabOutput, GlabError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes)
            .await
    }

    async fn run_full(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
        extra_ok_codes: &[i32],
    ) -> Result<GlabOutput, GlabError> {
        let args_str = args.join(" ");
        // glab을 PATH로 해석한다(절대경로 하드코딩 금지) — 테스트가 PATH 앞에 얹는
        // 스크립트 fake glab이 그대로 잡히도록.
        let mut cmd = Command::new("glab");
        cmd.args(args)
            .current_dir(cwd)
            // stderr 분류가 영어 로케일에 의존하므로 glab에도 이어야 한다(§3.3, 7a와 동일).
            .env("LC_ALL", "C")
            // glab은 GH_PROMPT_DISABLED에 대응하는 env가 없다. non-tty(stdin null)에서
            // 프롬프트를 건너뛰고, 확인이 필요한 쓰기는 호출부가 `--yes`를 붙인다.
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
                return Err(GlabError::Timeout { args: args_str });
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
                    RunAbort::TooLarge => GlabError::OutputTooLarge {
                        limit: MAX_OUTPUT_BYTES,
                    },
                    RunAbort::Io(e) => GlabError::Io(e),
                });
            }
        };

        let code = status.code().unwrap_or(-1);
        if !status.success() && !extra_ok_codes.contains(&code) {
            return Err(GlabError::Failed {
                args: args_str,
                code: status.code(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            });
        }
        Ok(GlabOutput {
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            code,
        })
    }
}
