//! M3-lite 통합 테스트: 실제 git으로 진짜 충돌/linked-worktree를 tempdir에서 만들어
//! `working_tree_status`의 `ConflictKind`, `detect_conflict_operation`, `resolve_git_dir`을
//! 고정한다(모킹 금지 — 저장소 규율). fixture가 개발자 전역 설정을 격리한다.

mod fixture;

use std::path::Path;
use std::process::Command;

use suaegi_git::conflict::{detect_conflict_operation, resolve_git_dir, ConflictOperation};
use suaegi_git::runner::GitRunner;
use suaegi_git::status::{working_tree_status, ConflictKind, FileStatus};

/// fixture::run과 같은 격리 env로 git을 돌리되 **실패를 허용**한다(merge conflict는
/// exit 1). fixture::run은 성공을 단언하므로 충돌을 일으키는 호출엔 이걸 쓴다.
fn run_allow_fail(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", dir.join(".test-gitconfig"))
        .output()
        .expect("spawn git")
}

/// 격리된 실제 repo(main + README 커밋 1개).
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    dir
}

// --- 충돌 kind 왕복(real git): 양쪽 수정 → BothModified ---
#[tokio::test]
async fn both_modified_conflict_round_trips() {
    let dir = repo();
    let p = dir.path();
    // 공통 조상에 file.txt를 심는다.
    std::fs::write(p.join("file.txt"), "base\n").unwrap();
    fixture::run(p, &["add", "file.txt"]);
    fixture::run(p, &["commit", "-m", "seed file"]);

    // them 브랜치: file.txt 수정.
    fixture::run(p, &["checkout", "-b", "them"]);
    std::fs::write(p.join("file.txt"), "theirs\n").unwrap();
    fixture::run(p, &["commit", "-am", "theirs"]);

    // us(main): file.txt 다르게 수정.
    fixture::run(p, &["checkout", "main"]);
    std::fs::write(p.join("file.txt"), "ours\n").unwrap();
    fixture::run(p, &["commit", "-am", "ours"]);

    // merge → 충돌.
    let out = run_allow_fail(p, &["merge", "them"]);
    assert!(!out.status.success(), "merge가 충돌 없이 성공해버렸다");

    let r = GitRunner::new();
    let map = working_tree_status(&r, p).await.unwrap();
    assert_eq!(
        map.get("file.txt"),
        Some(&FileStatus::Conflicted(ConflictKind::BothModified)),
        "UU 충돌이 BothModified로 안 잡혔다: {map:?}"
    );
}

// --- 충돌 kind 왕복(real git): 양쪽 추가 → BothAdded ---
#[tokio::test]
async fn both_added_conflict_round_trips() {
    let dir = repo();
    let p = dir.path();
    // 조상엔 new.txt가 없다. 두 브랜치가 서로 다른 내용으로 같은 이름을 추가한다.
    fixture::run(p, &["checkout", "-b", "them"]);
    std::fs::write(p.join("new.txt"), "theirs\n").unwrap();
    fixture::run(p, &["add", "new.txt"]);
    fixture::run(p, &["commit", "-m", "theirs adds new"]);

    fixture::run(p, &["checkout", "main"]);
    std::fs::write(p.join("new.txt"), "ours\n").unwrap();
    fixture::run(p, &["add", "new.txt"]);
    fixture::run(p, &["commit", "-m", "ours adds new"]);

    let out = run_allow_fail(p, &["merge", "them"]);
    assert!(!out.status.success(), "merge가 충돌 없이 성공해버렸다");

    let r = GitRunner::new();
    let map = working_tree_status(&r, p).await.unwrap();
    assert_eq!(
        map.get("new.txt"),
        Some(&FileStatus::Conflicted(ConflictKind::BothAdded)),
        "AA 충돌이 BothAdded로 안 잡혔다: {map:?}"
    );
}

// --- operation 프로브: 진행 중 merge → Merge ---
#[tokio::test]
async fn detects_merge_in_progress() {
    let dir = repo();
    let p = dir.path();
    std::fs::write(p.join("file.txt"), "base\n").unwrap();
    fixture::run(p, &["add", "file.txt"]);
    fixture::run(p, &["commit", "-m", "seed"]);
    fixture::run(p, &["checkout", "-b", "them"]);
    std::fs::write(p.join("file.txt"), "theirs\n").unwrap();
    fixture::run(p, &["commit", "-am", "theirs"]);
    fixture::run(p, &["checkout", "main"]);
    std::fs::write(p.join("file.txt"), "ours\n").unwrap();
    fixture::run(p, &["commit", "-am", "ours"]);
    run_allow_fail(p, &["merge", "them"]);

    assert_eq!(
        detect_conflict_operation(p).unwrap(),
        ConflictOperation::Merge,
        "MERGE_HEAD가 있는데 Merge로 안 잡혔다"
    );
}

// --- operation 프로브: rebase-merge/ 디렉터리 → Rebase ---
#[tokio::test]
async fn detects_rebase_from_marker_dir() {
    let dir = repo();
    let p = dir.path();
    let git_dir = resolve_git_dir(p).unwrap();
    std::fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
    assert_eq!(
        detect_conflict_operation(p).unwrap(),
        ConflictOperation::Rebase
    );
}

// --- operation 프로브: rebase-apply/ 도 Rebase ---
#[tokio::test]
async fn detects_rebase_from_apply_dir() {
    let dir = repo();
    let p = dir.path();
    let git_dir = resolve_git_dir(p).unwrap();
    std::fs::create_dir_all(git_dir.join("rebase-apply")).unwrap();
    assert_eq!(
        detect_conflict_operation(p).unwrap(),
        ConflictOperation::Rebase
    );
}

// --- operation 프로브: CHERRY_PICK_HEAD → CherryPick ---
#[tokio::test]
async fn detects_cherry_pick_from_marker_file() {
    let dir = repo();
    let p = dir.path();
    let git_dir = resolve_git_dir(p).unwrap();
    std::fs::write(git_dir.join("CHERRY_PICK_HEAD"), "deadbeef\n").unwrap();
    assert_eq!(
        detect_conflict_operation(p).unwrap(),
        ConflictOperation::CherryPick
    );
}

// --- operation 프로브: 깨끗한 repo → Unknown ---
#[tokio::test]
async fn clean_repo_is_unknown() {
    let dir = repo();
    assert_eq!(
        detect_conflict_operation(dir.path()).unwrap(),
        ConflictOperation::Unknown
    );
}

// --- crux(precedence mutation): MERGE_HEAD + CHERRY_PICK_HEAD 동시 → Merge ---
// MERGE_HEAD를 CHERRY_PICK_HEAD보다 먼저 검사해야 한다. 순서를 뒤집는 mutation은
// 두 마커가 모두 있는 이 테스트에서 CherryPick을 내며 FAIL한다.
#[tokio::test]
async fn merge_head_wins_over_cherry_pick_marker() {
    let dir = repo();
    let p = dir.path();
    let git_dir = resolve_git_dir(p).unwrap();
    std::fs::write(git_dir.join("MERGE_HEAD"), "deadbeef\n").unwrap();
    std::fs::write(git_dir.join("CHERRY_PICK_HEAD"), "cafebabe\n").unwrap();
    assert_eq!(
        detect_conflict_operation(p).unwrap(),
        ConflictOperation::Merge,
        "MERGE_HEAD가 CHERRY_PICK_HEAD보다 우선해야 한다"
    );
}

// --- crux(precedence mutation, synthetic): MERGE_HEAD + rebase-merge/ 동시 → Merge ---
// 실제 git은 merge와 rebase 마커를 공존시키지 않지만, merge arm이 rebase arm보다 먼저
// 검사돼야 한다는 **방어 분기**를 미래 리팩터 대비 pin한다. git init 없이 `.git`
// 디렉터리에 두 마커를 직접 만든다(resolve_git_dir이 `.git` 디렉터리를 그대로 쓴다).
// merge/rebase arm 순서를 뒤집으면(Rebase 먼저) 이 테스트가 Rebase를 내며 FAIL한다.
#[tokio::test]
async fn merge_head_wins_over_rebase_marker() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let git_dir = p.join(".git");
    std::fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
    std::fs::write(git_dir.join("MERGE_HEAD"), "deadbeef\n").unwrap();
    assert_eq!(
        detect_conflict_operation(p).unwrap(),
        ConflictOperation::Merge,
        "MERGE_HEAD가 rebase-merge/보다 우선해야 한다"
    );
}

// --- crux(relative-pointer mutation, synthetic): 상대 gitdir: 포인터를 worktree 기준 해석 ---
// `git worktree add`는 절대 포인터를 쓰므로 상대 분기(`worktree.join(p)`)와 빈-포인터
// 가드가 실제 git 경로로는 미탐이다. 상대 포인터를 담은 `.git` **파일**을 직접 만들어
// pin한다. else 분기를 `Ok(p.to_path_buf())`(join 생략)로 바꾸면 resolved가 상대 경로가
// 되어 worktree.join(...)와 달라 이 단언이 FAIL한다.
#[tokio::test]
async fn resolve_git_dir_joins_relative_pointer_to_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    // `.git`을 파일로 만들어 linked-worktree 간접을 흉내낸다(디렉터리가 아님).
    std::fs::write(p.join(".git"), "gitdir: relative/sub/path\n").unwrap();
    let resolved = resolve_git_dir(p).unwrap();
    assert_eq!(
        resolved,
        p.join("relative/sub/path"),
        "상대 포인터가 worktree 기준으로 해석되지 않았다: {resolved:?}"
    );
}

// 빈/공백-only `gitdir:` 포인터는 fallback(`<wt>/.git`)으로 간다 — `if !trimmed.is_empty()`
// 가드를 pin한다(가드를 지우면 빈 포인터가 `worktree.join("")` = worktree 자체로 새어
// 엉뚱한 경로가 된다).
#[tokio::test]
async fn resolve_git_dir_empty_pointer_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join(".git"), "gitdir:   \n").unwrap();
    let resolved = resolve_git_dir(p).unwrap();
    assert_eq!(
        resolved,
        p.join(".git"),
        "빈 포인터가 fallback으로 안 갔다: {resolved:?}"
    );
}

// --- crux(pointer mutation): linked worktree의 .git 파일 포인터를 따라간다 ---
// `git worktree add`가 만든 linked worktree는 `.git`이 파일(gitdir: 포인터)이다.
// resolve_git_dir이 포인터를 무시하고 `<wt>/.git`을 그대로 쓰면(파일인데 디렉터리처럼),
// 해석 경로가 실제 git 디렉터리(`<main>/.git/worktrees/<name>`)와 달라 이 단언이 FAIL한다.
#[tokio::test]
async fn resolve_git_dir_follows_linked_worktree_pointer() {
    let repo = repo();
    let ws = tempfile::tempdir().unwrap();
    let wt_path = ws.path().join("linked");
    fixture::run(
        repo.path(),
        &["worktree", "add", wt_path.to_str().unwrap(), "-b", "linked"],
    );

    // linked worktree의 `.git`은 파일이어야 한다(디렉터리가 아니다).
    let dot_git = wt_path.join(".git");
    assert!(
        dot_git.is_file(),
        ".git가 파일이 아니다(linked worktree 아님?)"
    );

    let resolved = resolve_git_dir(&wt_path).unwrap();
    // 포인터를 blind하게 무시하면 resolved == <wt>/.git(파일)이 된다.
    assert_ne!(
        resolved, dot_git,
        "포인터를 안 따라가고 .git 파일을 그대로 썼다"
    );
    // 진짜 git 디렉터리는 존재하는 **디렉터리**이고 worktrees/ 아래를 가리킨다.
    assert!(
        resolved.is_dir(),
        "해석된 git 디렉터리가 디렉터리가 아니다: {resolved:?}"
    );
    assert!(
        resolved.to_string_lossy().contains("worktrees"),
        "linked worktree git 디렉터리가 worktrees/ 아래를 안 가리킨다: {resolved:?}"
    );
    // 그리고 그 디렉터리에서 HEAD 파일을 볼 수 있어야 한다(실재 확인).
    assert!(
        resolved.join("HEAD").exists(),
        "해석된 git 디렉터리에 HEAD가 없다"
    );
}

// --- operation 프로브: linked worktree에서의 merge도 포인터 너머 마커를 본다 ---
// linked worktree 안에서 진행 중인 merge의 MERGE_HEAD는 `<main>/.git/worktrees/<name>`에
// 생긴다. resolve_git_dir이 포인터를 따라가야만 detect가 이를 본다.
#[tokio::test]
async fn detects_merge_in_linked_worktree() {
    let repo = repo();
    let rp = repo.path();
    // main repo에 충돌 소스를 만든다.
    std::fs::write(rp.join("file.txt"), "base\n").unwrap();
    fixture::run(rp, &["add", "file.txt"]);
    fixture::run(rp, &["commit", "-m", "seed"]);
    fixture::run(rp, &["branch", "them"]);
    // them 브랜치에 커밋.
    fixture::run(rp, &["checkout", "them"]);
    std::fs::write(rp.join("file.txt"), "theirs\n").unwrap();
    fixture::run(rp, &["commit", "-am", "theirs"]);
    fixture::run(rp, &["checkout", "main"]);
    std::fs::write(rp.join("file.txt"), "ours\n").unwrap();
    fixture::run(rp, &["commit", "-am", "ours"]);

    // linked worktree를 main에서 만들고 그 안에서 them을 merge → 충돌.
    let ws = tempfile::tempdir().unwrap();
    let wt_path = ws.path().join("linked");
    fixture::run(
        rp,
        &[
            "worktree",
            "add",
            "-b",
            "mergehere",
            wt_path.to_str().unwrap(),
            "main",
        ],
    );
    let out = run_allow_fail(&wt_path, &["merge", "them"]);
    assert!(
        !out.status.success(),
        "linked worktree merge가 충돌 없이 성공했다"
    );

    assert_eq!(
        detect_conflict_operation(&wt_path).unwrap(),
        ConflictOperation::Merge,
        "linked worktree의 MERGE_HEAD를 포인터 너머에서 못 봤다"
    );
}
