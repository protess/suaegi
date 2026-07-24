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
    // **개발자의 전역 무시 규칙을 차단한다.** `GitRunner`는 `GIT_CONFIG_GLOBAL`을
    // 세우지 않으므로(실 앱에서 그러면 안 된다) 테스트 중에도 개발자 기계의
    // `$XDG_CONFIG_HOME/git/ignore`를 그대로 읽는다. 그리고 그 파일은
    // `GIT_CONFIG_GLOBAL`로 막히지 않는다 — `core.excludesFile`은 config가
    // 아니라 별도 기본 경로이기 때문이다.
    //
    // 실제로 이것 때문에 테스트 하나가 공허했다: 이 기계의 전역 ignore에
    // `**/.claude/settings.local.json`이 있어서, 우리 필터를 통째로 지워도
    // 그 파일이 애초에 목록에 안 나와 테스트가 통과했다.
    run(dir, &["config", "core.excludesFile", "/dev/null"]);
    std::fs::write(dir.join(".gitignore"), ".test-gitconfig\n.no-hooks/\n").unwrap();
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(dir, &["add", "README.md", ".gitignore"]);
    run(dir, &["commit", "-m", "init"]);
}

/// 로컬 **bare** remote(네트워크 없이 AV). `dir`에 `git init --bare -b main`.
/// clone/push/fetch/pull이 이걸 origin으로 왕복한다. 라이브 github auth만 사람눈이고,
/// 로컬 bare 왕복은 자율 검증 가능하다.
// M2 fetch/pull AV 하네스 전용 — 이 fixture를 include하는 다른 test 바이너리는 안 쓰므로
// dead_code를 허용한다(공유 fixture를 부분집합만 쓰는 표준 idiom).
#[allow(dead_code)]
pub fn init_bare_remote(dir: &Path) {
    run(dir, &["init", "--bare", "-b", "main"]);
}

/// bare remote를 `dest`로 clone하고 격리 identity/서명끄기/훅차단을 **로컬** config로 건다.
///
/// **로컬 config가 유일한 신뢰 격리다.** 실제 드라이버(`fetch`/`pull`)는 `GitRunner`를
/// 거치는데, `GitRunner`는 (실 앱 정책상) `GIT_CONFIG_GLOBAL`을 세우지 않아 개발자 기계의
/// 전역 config를 읽는다. 그래서 clone마다 로컬 config로 identity/`commit.gpgsign=false`/
/// `pull.rebase=false`를 못 박아 개발자 전역 설정이 테스트를 오염시키지 못하게 한다.
#[allow(dead_code)]
pub fn clone_from(bare: &Path, dest: &Path) {
    let parent = dest.parent().expect("dest has parent");
    let name = dest.file_name().unwrap().to_str().unwrap();
    run(parent, &["clone", bare.to_str().unwrap(), name]);
    run(dest, &["config", "user.email", "t@example.com"]);
    run(dest, &["config", "user.name", "test"]);
    run(dest, &["config", "commit.gpgsign", "false"]);
    run(dest, &["config", "tag.gpgsign", "false"]);
    run(dest, &["config", "core.hooksPath", ".no-hooks"]);
    // pull 전략을 명시적으로 고정 — 개발자 전역 `pull.rebase`/`pull.ff`가 어떻든
    // divergent pull이 결정적으로 동작하게 한다. drop-`--ff-only` mutation을 드러내는
    // merge 경로도 여기 설정 위에서 재현된다.
    run(dest, &["config", "pull.rebase", "false"]);
    std::fs::create_dir_all(dest.join(".no-hooks")).unwrap();
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
