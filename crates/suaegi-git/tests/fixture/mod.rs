use std::path::Path;
use std::process::Command;

/// 테스트용 실제 git repo: `git init -b main` + README 커밋 1개.
/// 개발자 머신의 글로벌/시스템 설정(gpg 서명, 훅 템플릿, credential helper)이
/// 테스트를 오염시키지 않도록 env로 완전 격리한다.
pub fn init_repo(dir: &Path) {
    // 빈 글로벌 설정 파일 + 빈 훅 디렉토리 (크로스플랫폼: /dev/null 대신 실제 빈 파일/디렉토리)
    std::fs::write(dir.join(".test-gitconfig"), "").unwrap();
    std::fs::create_dir_all(dir.join(".no-hooks")).unwrap();
    run(dir, &["init", "-b", "main"]);
    run(dir, &["config", "user.email", "t@example.com"]);
    run(dir, &["config", "user.name", "test"]);
    run(dir, &["config", "commit.gpgsign", "false"]);
    run(dir, &["config", "tag.gpgsign", "false"]);
    run(dir, &["config", "core.hooksPath", ".no-hooks"]);
    std::fs::write(dir.join(".gitignore"), ".test-gitconfig\n.no-hooks/\n").unwrap();
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(dir, &["add", "README.md", ".gitignore"]);
    run(dir, &["commit", "-m", "init"]);
}

pub fn run(dir: &Path, args: &[&str]) {
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
        .env("GIT_CONFIG_GLOBAL", dir.join(".test-gitconfig"))
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
