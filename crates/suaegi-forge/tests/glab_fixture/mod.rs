//! 스크립트 fake `glab`을 PATH에 얹는 테스트 하네스. github 테스트의 `FakeGh` 픽스처를
//! 그대로 미러한다: 각 테스트가 자기 tempdir에 정해진 stdout/stderr/exit를 내는 실행 가능한
//! `glab` sh 스크립트를 만들고, 테스트 프로세스의 PATH 앞에 그 dir을 얹는다. 이걸로 출력
//! 파싱·exit code 분류·None/Unavailable/Found 분기를 트레잇 추상화 없이 실 단위로 본다.
//!
//! **PATH는 프로세스 전역이라** 병렬 테스트가 서로의 PATH를 덮어쓸 수 있다 — [`env_lock`]으로
//! 직렬화하고, PATH를 세우는 동안만 잡는다.
//!
//! resolve_repository는 glab이 아니라 **git origin 원격**을 파싱하므로, 실제 git repo를
//! tempdir에 세우는 [`init_gitlab_repo`]도 함께 제공한다.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

/// PATH를 만지는 fake-glab 테스트 직렬화용 전역 락.
static ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Rule {
    prefix: String,
    stdout: String,
    stderr: String,
    exit: i32,
}

/// 스크립트 fake glab 빌더. 규칙을 등록한 순서대로 매칭한다(먼저 등록한 게 우선).
pub struct FakeGlab {
    dir: TempDir,
    rules: Vec<Rule>,
}

impl FakeGlab {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
            rules: Vec::new(),
        }
    }

    /// `prefix`로 시작하는 args에 대해 (stdout, stderr, exit)를 낸다.
    pub fn rule(mut self, prefix: &str, stdout: &str, stderr: &str, exit: i32) -> Self {
        self.rules.push(Rule {
            prefix: prefix.to_string(),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit,
        });
        self
    }

    /// preflight를 통과시키는 기본 규칙(버전 1.36, auth OK)을 얹는다.
    pub fn with_ready_preflight(self) -> Self {
        self.rule("--version", "glab version 1.36.0 (2024-05-01)\n", "", 0)
            .rule("auth status", "", "gitlab.com\n  Logged in\n", 0)
    }

    /// 스크립트 파일들을 쓰고, PATH 앞에 이 dir을 얹는다. 반환 가드가 drop되면 PATH를
    /// 되돌린다. **호출 전 [`env_lock`]을 잡아야 한다.**
    pub fn activate(&self) -> PathGuard {
        let mut script = String::new();
        script.push_str("#!/bin/sh\n");
        script.push_str(&format!("DIR='{}'\n", self.dir.path().display()));
        script.push_str("args=\"$*\"\n");
        script.push_str("case \"$args\" in\n");
        for (i, r) in self.rules.iter().enumerate() {
            std::fs::write(self.dir.path().join(format!("r{i}.out")), &r.stdout).unwrap();
            std::fs::write(self.dir.path().join(format!("r{i}.err")), &r.stderr).unwrap();
            script.push_str(&format!("  \"{}\"*)\n", r.prefix));
            script.push_str(&format!("    cat \"$DIR/r{i}.out\"\n"));
            script.push_str(&format!("    cat \"$DIR/r{i}.err\" >&2\n"));
            script.push_str(&format!("    exit {}\n", r.exit));
            script.push_str("    ;;\n");
        }
        script.push_str("  *)\n");
        script.push_str("    printf 'fake glab: unexpected args: %s\\n' \"$args\" >&2\n");
        script.push_str("    exit 97\n");
        script.push_str("    ;;\n");
        script.push_str("esac\n");

        let glab_path = self.dir.path().join("glab");
        std::fs::write(&glab_path, script).unwrap();
        make_executable(&glab_path);

        let original = std::env::var("PATH").unwrap_or_default();
        let new = format!("{}:{}", self.dir.path().display(), original);
        std::env::set_var("PATH", &new);
        PathGuard { original }
    }
}

/// drop 시 PATH를 원래대로 되돌린다.
pub struct PathGuard {
    original: String,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        std::env::set_var("PATH", &self.original);
    }
}

#[cfg(unix)]
fn make_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(p).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(p, perm).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_p: &Path) {}

/// tempdir에 실제 git repo를 세우고 `origin`을 주어진 URL로 건다. resolve_repository가
/// git origin을 파싱하므로 필요하다.
pub fn init_gitlab_repo(dir: &Path, origin_url: &str) {
    std::fs::write(dir.join(".test-gitconfig"), "").unwrap();
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "test"]);
    git(dir, &["remote", "add", "origin", origin_url]);
}

pub fn git(dir: &Path, args: &[&str]) {
    let cfg = dir.join(".test-gitconfig");
    if !cfg.exists() {
        let _ = std::fs::write(&cfg, "");
    }
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cfg)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// glab이 **아예 없는** PATH(빈 dir 하나)를 세운다 — `is_glab_not_found`(ENOENT) 경로용.
pub fn activate_no_glab() -> NoGlabGuard {
    let dir = tempfile::tempdir().expect("tempdir");
    let original = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", dir.path());
    NoGlabGuard {
        _dir: dir,
        original,
    }
}

pub struct NoGlabGuard {
    _dir: TempDir,
    original: String,
}

impl Drop for NoGlabGuard {
    fn drop(&mut self) {
        std::env::set_var("PATH", &self.original);
    }
}
