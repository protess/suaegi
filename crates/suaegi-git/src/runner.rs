use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git {args} timed out")]
    Timeout { args: String },
    #[error("git {args} failed (code {code:?}): {stderr}")]
    Failed { args: String, code: Option<i32>, stderr: String },
    #[error("git {args} produced unparseable output: {detail}")]
    Parse { args: String, detail: String },
}

#[derive(Debug, Clone, Default)]
pub struct GitRunner;

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

impl GitRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, &[]).await
    }

    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, timeout, &[]).await
    }

    pub async fn run_expecting(
        &self,
        cwd: &Path,
        args: &[&str],
        extra_ok_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes).await
    }

    async fn run_full(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
        extra_ok_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
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

        let waited = tokio::time::timeout(timeout, async {
            // stdout/stderr 동시 드레인 — 순차로 읽으면 반대쪽 파이프가 가득 차
            // 자식이 블록되는 교착 가능
            let read_out = async {
                if let Some(s) = stdout_pipe.as_mut() {
                    s.read_to_end(&mut out).await?;
                }
                Ok::<_, std::io::Error>(())
            };
            let read_err = async {
                if let Some(s) = stderr_pipe.as_mut() {
                    s.read_to_end(&mut err).await?;
                }
                Ok::<_, std::io::Error>(())
            };
            let (status, _, _) = tokio::try_join!(child.wait(), read_out, read_err)?;
            Ok::<_, std::io::Error>(status)
        })
        .await;

        let status = match waited {
            Err(_) => {
                kill_process_tree(&mut child);
                let _ = child.wait().await; // 좀비 회수
                return Err(GitError::Timeout { args: args_str });
            }
            Ok(result) => result?,
        };

        let stdout = String::from_utf8_lossy(&out).into_owned();
        let stderr = String::from_utf8_lossy(&err).into_owned();
        let code = status.code().unwrap_or(-1);
        if !status.success() && !extra_ok_codes.contains(&code) {
            return Err(GitError::Failed {
                args: args_str,
                code: status.code(),
                stderr,
            });
        }
        Ok(GitOutput { stdout, stderr, code })
    }
}
