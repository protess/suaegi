//! 스크립트 fake `gh`를 PATH에 얹는 테스트 하네스(플랜 §5, Codex #Q3). suaegi-git의
//! `init_repo`가 tempdir에 **실제 git**을 돌리는 것과 같은 정신으로, 여기선 **바이너리를
//! 스크립트**한다: 각 테스트가 자기 tempdir에 정해진 stdout/stderr/exit를 내는 실행
//! 가능한 `gh` sh 스크립트를 만들고, 테스트 프로세스의 PATH 앞에 그 dir을 얹는다. 이걸로
//! 출력 파싱·exit code 분류·None/Unavailable/Found 분기를 트레잇 추상화 없이 실 단위로 본다.
//!
//! **PATH는 프로세스 전역이라** 병렬 테스트가 서로의 PATH를 덮어쓸 수 있다 — [`env_lock`]으로
//! fake-gh 테스트를 직렬화하고, PATH를 세우는 동안만 잡는다.
//!
//! 각 테스트 바이너리는 이 픽스처의 일부만 쓰므로, 안 쓰는 헬퍼의 dead_code 경고를 끈다
//! (suaegi-git의 공유 픽스처와 같은 관행).
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

/// PATH를 만지는 fake-gh 테스트 직렬화용 전역 락. (`std::env::set_var`는 프로세스 전역.)
static ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// gh 서브커맨드 한 매칭에 대한 canned 응답.
struct Rule {
    /// args를 공백으로 이은 문자열의 **접두사** 매칭(예: "repo view", "pr view", "--version").
    prefix: String,
    stdout: String,
    stderr: String,
    exit: i32,
}

/// 스크립트 fake gh 빌더. 규칙을 등록한 순서대로 매칭한다(먼저 등록한 게 우선).
pub struct FakeGh {
    dir: TempDir,
    rules: Vec<Rule>,
}

impl FakeGh {
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

    /// preflight를 통과시키는 기본 규칙(버전 2.40, auth OK)을 얹는다.
    pub fn with_ready_preflight(self) -> Self {
        self.rule("--version", "gh version 2.40.0 (2024-01-01)\n", "", 0)
            .rule("auth status", "", "Logged in to github.com\n", 0)
    }

    /// 스크립트 파일들을 쓰고, PATH 앞에 이 dir을 얹는다. 반환 가드가 drop되면 PATH를
    /// 되돌린다. **호출 전 [`env_lock`]을 잡아야 한다.**
    pub fn activate(&self) -> PathGuard {
        let mut script = String::new();
        script.push_str("#!/bin/sh\n");
        // DIR을 리터럴로 박는다 — PATH 조회 시 $0가 "gh"로만 넘어와 dirname이 안 통한다.
        script.push_str(&format!("DIR='{}'\n", self.dir.path().display()));
        script.push_str("args=\"$*\"\n");
        script.push_str("case \"$args\" in\n");
        for (i, r) in self.rules.iter().enumerate() {
            std::fs::write(self.dir.path().join(format!("r{i}.out")), &r.stdout).unwrap();
            std::fs::write(self.dir.path().join(format!("r{i}.err")), &r.stderr).unwrap();
            // 리터럴 접두사는 따옴표로(글롭 죽임), 뒤 * 만 살린다.
            script.push_str(&format!("  \"{}\"*)\n", r.prefix));
            script.push_str(&format!("    cat \"$DIR/r{i}.out\"\n"));
            script.push_str(&format!("    cat \"$DIR/r{i}.err\" >&2\n"));
            script.push_str(&format!("    exit {}\n", r.exit));
            script.push_str("    ;;\n");
        }
        script.push_str("  *)\n");
        script.push_str("    printf 'fake gh: unexpected args: %s\\n' \"$args\" >&2\n");
        script.push_str("    exit 97\n");
        script.push_str("    ;;\n");
        script.push_str("esac\n");

        let gh_path = self.dir.path().join("gh");
        std::fs::write(&gh_path, script).unwrap();
        make_executable(&gh_path);

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

/// 실제 git repo(eligibility의 `@{u}` 체크용). `init_repo`(suaegi-git 픽스처) 정신으로
/// env 격리. `with_upstream=true`면 bare 원격을 만들어 브랜치를 push해 tracking ref를 세운다.
pub fn init_repo_with_branch(dir: &Path, branch: &str, with_upstream: bool) {
    std::fs::write(dir.join(".test-gitconfig"), "").unwrap();
    git(dir, &["init", "-b", branch]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-m", "init"]);

    if with_upstream {
        let remote = dir.join("..").join(format!(
            "{}-remote.git",
            dir.file_name().unwrap().to_string_lossy()
        ));
        git(dir, &["init", "--bare", remote.to_str().unwrap()]);
        git(dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(dir, &["push", "-u", "origin", branch]);
    }
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

/// gh가 **아예 없는** PATH(빈 dir 하나)를 세운다 — `is_gh_not_found`(ENOENT) 경로 테스트용.
/// 반환 가드가 살아 있는 동안 유효하며 drop 시 PATH를 되돌린다. [`env_lock`]을 잡고 쓴다.
pub fn activate_no_gh() -> NoGhGuard {
    let dir = tempfile::tempdir().expect("tempdir");
    let original = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", dir.path());
    NoGhGuard {
        _dir: dir,
        original,
    }
}

pub struct NoGhGuard {
    _dir: TempDir,
    original: String,
}

impl Drop for NoGhGuard {
    fn drop(&mut self) {
        std::env::set_var("PATH", &self.original);
    }
}
