mod fixture;

use suaegi_git::compare::{
    branch_compare, file_diff, file_head_bytes, working_tree_dirty, BranchCompare, ChangeStatus,
    CompareHandle, CompareOutcome, FileDiff, FileSource, BINARY_SNIFF_BYTES,
};
use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::add_worktree;

async fn setup() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "feat", "main", ws.path())
        .await
        .unwrap();
    (repo, ws, created.path)
}

/// 취소하지 않는 호출 — 대부분의 테스트가 보는 것은 `Ready`뿐이다.
async fn ready(r: &GitRunner, wt: &std::path::Path, base: &str) -> BranchCompare {
    match branch_compare(r, wt, base, &CompareHandle::new())
        .await
        .unwrap()
    {
        CompareOutcome::Ready(cmp) => cmp,
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[tokio::test]
async fn compare_reports_committed_changes() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("new.txt"), "new\n").unwrap();
    std::fs::write(wt.join("README.md"), "changed\n").unwrap();
    fixture::run(&wt, &["add", "."]);
    fixture::run(&wt, &["commit", "-m", "change"]);

    let r = GitRunner::new();
    let cmp = ready(&r, &wt, "main").await;
    assert_eq!(cmp.ahead_count, 1);
    let mut paths: Vec<_> = cmp.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    // fixture가 만드는 .test-gitconfig는 untracked로 잡히므로 필터
    let paths: Vec<&str> = paths
        .into_iter()
        .filter(|p| !p.starts_with(".test-"))
        .collect();
    assert_eq!(paths, vec!["README.md", "new.txt"]);
    let readme = cmp.files.iter().find(|f| f.path == "README.md").unwrap();
    assert_eq!(readme.status, ChangeStatus::Modified);
    assert_eq!(readme.additions, Some(1));
    assert_eq!(readme.deletions, Some(1));
}

#[tokio::test]
async fn compare_includes_untracked_files() {
    let (_repo, _ws, wt) = setup().await;
    // add도 commit도 하지 않은 새 파일 — 에이전트 작업 중 가장 흔한 상태
    std::fs::write(wt.join("wip.txt"), "wip\n").unwrap();

    let r = GitRunner::new();
    let cmp = ready(&r, &wt, "main").await;
    assert_eq!(cmp.ahead_count, 0);
    let wip = cmp
        .files
        .iter()
        .find(|f| f.path == "wip.txt")
        .expect("untracked file missing");
    assert_eq!(wip.status, ChangeStatus::Added);
}

#[tokio::test]
async fn no_changes_yields_no_tracked_diffs() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    let cmp = ready(&r, &wt, "main").await;
    assert_eq!(cmp.ahead_count, 0);
    // fixture 부산물(.test-gitconfig 등) 외에는 없어야 한다
    assert!(cmp
        .files
        .iter()
        .all(|f| f.path.starts_with(".test-") || f.path.starts_with(".no-hooks")));
}

#[tokio::test]
async fn compare_reports_renamed_files() {
    let (_repo, _ws, wt) = setup().await;
    // 내용 변경 없이 순수 rename만 수행 — 유사도 100%로 R100 감지를 보장한다.
    fixture::run(&wt, &["mv", "README.md", "renamed.md"]);
    fixture::run(&wt, &["add", "-A"]);
    fixture::run(&wt, &["commit", "-m", "rename"]);

    let r = GitRunner::new();
    let cmp = ready(&r, &wt, "main").await;
    let renamed = cmp
        .files
        .iter()
        .find(|f| f.path == "renamed.md")
        .expect("renamed file missing");
    assert_eq!(
        renamed.status,
        ChangeStatus::Renamed {
            from: "README.md".into()
        }
    );
    assert!(renamed.additions.is_some());
    assert!(renamed.deletions.is_some());
}

/// `-C`가 내는 `C` 레코드는 `R`처럼 **경로가 둘**이다. 하나만 소비하면 그 뒤의
/// 모든 레코드가 한 칸씩 밀린다 — 그래서 복사 **뒤에 다른 변경을 하나 더** 둔다.
/// 밀림은 그 뒤 레코드를 봐야만 보인다.
#[tokio::test]
async fn copy_record_consumes_two_paths_and_keeps_later_records_aligned() {
    let (_repo, _ws, wt) = setup().await;
    // `-C`(--find-copies-harder 없이)는 **같은 diff에서 수정된 파일**만 복사원으로
    // 본다. 그래서 README를 수정하면서 그 원본 내용으로 copy.md를 만든다.
    std::fs::write(wt.join("README.md"), "hello\nmore\n").unwrap();
    std::fs::write(wt.join("copy.md"), "hello\n").unwrap();
    // 경로 정렬상 copy.md 뒤에 오는 변경. 밀림의 탐지기다.
    std::fs::write(wt.join("zzz.txt"), "z\n").unwrap();
    fixture::run(&wt, &["add", "-A"]);
    fixture::run(&wt, &["commit", "-m", "copy"]);

    let r = GitRunner::new();
    let cmp = ready(&r, &wt, "main").await;

    let copied = cmp
        .files
        .iter()
        .find(|f| f.path == "copy.md")
        .expect("copy.md missing — `-C` did not produce a C record");
    assert_eq!(
        copied.status,
        ChangeStatus::Copied {
            from: "README.md".into()
        }
    );

    // 밀림 탐지: 파서가 C에서 경로를 하나만 먹으면 zzz.txt는 `Other('c')`가 되고
    // (앞 레코드의 두 번째 경로 "copy.md"의 첫 글자가 상태로 읽힌다) 아예
    // 사라지거나 상태가 어긋난다.
    let last = cmp
        .files
        .iter()
        .find(|f| f.path == "zzz.txt")
        .expect("record after the copy went missing — paths are misaligned");
    assert_eq!(last.status, ChangeStatus::Added);
    // numstat 쪽도 같은 두 경로짜리 모양이다. 여기가 밀리면 `counts`에 to 경로가
    // 안 들어가 (None, None)이 된다 — 그래서 `Some(0)`을 못 박는다.
    assert_eq!(copied.additions, Some(0));
    assert_eq!(copied.deletions, Some(0));

    // 그리고 유령 레코드가 생기지 않았는지: `Other`는 하나도 없어야 한다.
    assert!(
        !cmp.files
            .iter()
            .any(|f| matches!(f.status, ChangeStatus::Other(_))),
        "unexpected Other records: {:?}",
        cmp.files
    );
}

/// 훅 주입 파일은 우리 것이다. 우리 diff 패널에 우리 파일이 뜨면 안 된다.
///
/// **대조군이 둘 있다**: 같은 `.claude/` 아래의 *다른* 파일은 그대로 보여야 하고
/// (필터가 디렉터리 통째로 삼키면 안 된다), 평범한 untracked 파일도 그대로여야
/// 한다 (필터가 untracked 수집 자체를 죽이면 안 된다).
#[tokio::test]
async fn the_injected_settings_file_is_hidden_but_its_neighbours_are_not() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::create_dir_all(wt.join(".claude")).unwrap();
    std::fs::write(wt.join(".claude/settings.local.json"), "{}\n").unwrap();
    std::fs::write(wt.join(".claude/notes.md"), "mine\n").unwrap();
    std::fs::write(wt.join("wip.txt"), "wip\n").unwrap();

    let r = GitRunner::new();
    let cmp = ready(&r, &wt, "main").await;
    let paths: Vec<&str> = cmp.files.iter().map(|f| f.path.as_str()).collect();

    assert!(
        !paths.contains(&".claude/settings.local.json"),
        "our own injected file leaked into the diff: {paths:?}"
    );
    assert!(
        paths.contains(&".claude/notes.md"),
        "the filter swallowed a neighbouring file: {paths:?}"
    );
    assert!(
        paths.contains(&"wip.txt"),
        "the filter broke untracked collection entirely: {paths:?}"
    );
}

// ---- 분류: 셋 다 merge-base의 exit 1로 뭉개져 있었다 ----

#[tokio::test]
async fn unresolvable_base_ref_is_invalid_base() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    let outcome = branch_compare(&r, &wt, "no-such-branch", &CompareHandle::new())
        .await
        .unwrap();
    assert_eq!(outcome, CompareOutcome::InvalidBase);
}

#[tokio::test]
async fn unborn_head_is_distinguished_from_invalid_base() {
    // worktree가 아니라 커밋이 하나도 없는 저장소. HEAD가 아직 없다.
    let repo = tempfile::tempdir().unwrap();
    fixture::run(repo.path(), &["init", "-b", "main"]);
    fixture::run(repo.path(), &["config", "user.email", "t@example.com"]);
    fixture::run(repo.path(), &["config", "user.name", "test"]);
    // base ref는 풀려야 한다 — 안 그러면 InvalidBase에서 먼저 걸려 이 테스트가
    // 검사하려는 두 번째 프로브에 도달하지 못한다. 별도 커밋으로 ref를 만든다.
    let other = tempfile::tempdir().unwrap();
    fixture::init_repo(other.path());
    fixture::run(
        repo.path(),
        &["fetch", other.path().to_str().unwrap(), "main:base"],
    );

    let r = GitRunner::new();
    let outcome = branch_compare(&r, repo.path(), "base", &CompareHandle::new())
        .await
        .unwrap();
    assert_eq!(outcome, CompareOutcome::UnbornHead);
}

#[tokio::test]
async fn orphan_branch_has_no_merge_base() {
    let (repo, _ws, _wt) = setup().await;
    // 고아 브랜치: main과 공통 조상이 전혀 없다. 실측으로 merge-base가 exit 1.
    fixture::run(repo.path(), &["checkout", "--orphan", "lonely"]);
    fixture::run(repo.path(), &["rm", "-rf", "--cached", "."]);
    std::fs::write(repo.path().join("only.txt"), "only\n").unwrap();
    fixture::run(repo.path(), &["add", "only.txt"]);
    fixture::run(repo.path(), &["commit", "-m", "orphan"]);

    let r = GitRunner::new();
    let outcome = branch_compare(&r, repo.path(), "main", &CompareHandle::new())
        .await
        .unwrap();
    assert_eq!(outcome, CompareOutcome::NoMergeBase);
}

/// 취소는 **오류가 아니다.** 그리고 대조군이 중요하다 — 취소가 없으면 같은 입력이
/// `InvalidBase`라는 **다른 구체적인 값**을 낸다. 그래야 "취소가 첫 git 호출을
/// 아예 시작하지 않았다"가 관찰된다.
#[tokio::test]
async fn cancel_stops_before_the_first_call() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();

    let cancel = CompareHandle::new();
    cancel.stop_after_current_call();
    let stopped = branch_compare(&r, &wt, "no-such-branch", &cancel)
        .await
        .unwrap();
    assert_eq!(stopped, CompareOutcome::Cancelled);

    // 대조군: 취소하지 않으면 분류 프로브가 실제로 돌아 InvalidBase가 나온다.
    let ran = branch_compare(&r, &wt, "no-such-branch", &CompareHandle::new())
        .await
        .unwrap();
    assert_eq!(ran, CompareOutcome::InvalidBase);
}

// ---- file_diff ----

#[tokio::test]
async fn file_diff_returns_unified_patch() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("README.md"), "changed\n").unwrap();
    fixture::run(&wt, &["add", "."]);
    fixture::run(&wt, &["commit", "-m", "change"]);

    let r = GitRunner::new();
    let diff = file_diff(&r, &wt, "main", "README.md", &ChangeStatus::Modified)
        .await
        .unwrap();
    let FileDiff::Patch(patch) = diff else {
        panic!("expected Patch, got {diff:?}");
    };
    assert!(patch.contains("-hello"));
    assert!(patch.contains("+changed"));
}

#[tokio::test]
async fn file_diff_synthesizes_patch_for_untracked() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("wip.txt"), "wip\n").unwrap();
    let r = GitRunner::new();
    let diff = file_diff(&r, &wt, "main", "wip.txt", &ChangeStatus::Added)
        .await
        .unwrap();
    let FileDiff::Patch(patch) = diff else {
        panic!("expected Patch, got {diff:?}");
    };
    assert!(patch.contains("+wip"));
}

/// NUL 스니핑. lossy `String`을 통과했다면 이 판정은 불가능하다.
#[tokio::test]
async fn file_diff_reports_binary_via_nul_sniffing() {
    let (_repo, _ws, wt) = setup().await;
    // 앞부분에 NUL이 있는 파일. git도 이걸 바이너리로 본다.
    std::fs::write(wt.join("blob.bin"), b"\x89PNG\r\n\x1a\n\x00\x00\x00rest").unwrap();
    let r = GitRunner::new();
    let diff = file_diff(&r, &wt, "main", "blob.bin", &ChangeStatus::Added)
        .await
        .unwrap();
    assert_eq!(diff, FileDiff::Binary);
}

/// 대조군: NUL이 없으면 같은 경로가 patch로 나온다. 위 테스트가 "무조건 Binary"를
/// 잡아내지 못하는 것을 막는다.
#[tokio::test]
async fn file_without_nul_is_not_binary() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("blob.bin"), "no nul here\n").unwrap();
    let r = GitRunner::new();
    let diff = file_diff(&r, &wt, "main", "blob.bin", &ChangeStatus::Added)
        .await
        .unwrap();
    let FileDiff::Patch(patch) = diff else {
        panic!("expected Patch, got {diff:?}");
    };
    assert!(patch.contains("+no nul here"));
}

/// `Other(c)`는 **추측하지 않는다.** git을 한 번도 부르지 않고 바로 돌려준다.
#[tokio::test]
async fn other_status_is_non_renderable() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    let diff = file_diff(&r, &wt, "main", "whatever.txt", &ChangeStatus::Other('T'))
        .await
        .unwrap();
    assert_eq!(diff, FileDiff::NonRenderable('T'));
}

/// 삭제된 파일은 working tree에 없다 — `Revision(merge_base)`로 봐야 한다.
/// `WorkingTree`로 봤다면 여기서 NotFound가 나 테스트가 죽는다.
#[tokio::test]
async fn deleted_file_is_sniffed_from_the_merge_base() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::remove_file(wt.join("README.md")).unwrap();
    fixture::run(&wt, &["add", "-A"]);
    fixture::run(&wt, &["commit", "-m", "delete"]);

    let r = GitRunner::new();
    let diff = file_diff(&r, &wt, "main", "README.md", &ChangeStatus::Deleted)
        .await
        .unwrap();
    let FileDiff::Patch(patch) = diff else {
        panic!("expected Patch, got {diff:?}");
    };
    assert!(patch.contains("-hello"));
}

// ---- file_head_bytes ----

#[tokio::test]
async fn head_bytes_reads_only_the_cap() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("big.txt"), "x".repeat(20_000)).unwrap();
    let r = GitRunner::new();
    let head = file_head_bytes(
        &r,
        &wt,
        FileSource::WorkingTree,
        "big.txt",
        BINARY_SNIFF_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(head.len(), BINARY_SNIFF_BYTES);
}

/// `Revision`도 `cap`을 지켜야 한다. `git show`는 앞부분만 달라고 할 수 없어 **파일
/// 전체**를 내므로, 우리가 자르지 않으면 삭제된 큰 파일 하나가 NUL 스니핑 한 번에
/// 통째로 메모리에 남는다. 프로덕션에서 흔한 입력이다 — 8KB 넘는 삭제 파일.
#[tokio::test]
async fn head_bytes_from_revision_also_honors_the_cap() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("big.txt"), "x".repeat(20_000)).unwrap();
    fixture::run(&wt, &["add", "-A"]);
    fixture::run(&wt, &["commit", "-m", "big"]);

    let r = GitRunner::new();
    let head = file_head_bytes(
        &r,
        &wt,
        FileSource::Revision("HEAD".into()),
        "big.txt",
        BINARY_SNIFF_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(head.len(), BINARY_SNIFF_BYTES);
}

#[tokio::test]
async fn head_bytes_from_revision_reads_committed_content() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    let head = file_head_bytes(
        &r,
        &wt,
        FileSource::Revision("HEAD".into()),
        "README.md",
        BINARY_SNIFF_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(head, b"hello\n");
}

/// `Revision`도 lossy `String`을 통과하면 안 된다. 유효하지 않은 UTF-8을 커밋해
/// 두고 원본 바이트가 그대로 오는지 본다 — 통과했다면 U+FFFD로 뭉개져 온다.
#[tokio::test]
async fn head_bytes_from_revision_survives_invalid_utf8() {
    let (_repo, _ws, wt) = setup().await;
    let raw = b"\xff\xfe\x00\x01binary";
    std::fs::write(wt.join("blob.bin"), raw).unwrap();
    fixture::run(&wt, &["add", "-A"]);
    fixture::run(&wt, &["commit", "-m", "binary"]);

    let r = GitRunner::new();
    let head = file_head_bytes(
        &r,
        &wt,
        FileSource::Revision("HEAD".into()),
        "blob.bin",
        BINARY_SNIFF_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(head, raw);
}

/// 6MB 상한을 넘는 patch는 **오류가 아니라 상태**다. `Err`로 올리면 UI가 진짜
/// 오류와 구별하지 못하고 배너를 띄운다. 프로덕션에서 도달 가능한 입력이다 —
/// 에이전트가 만든 큰 생성 파일 하나면 된다.
#[tokio::test]
async fn oversized_patch_becomes_too_large_not_an_error() {
    let (_repo, _ws, wt) = setup().await;
    // NUL이 없어야 바이너리로 빠지지 않고 patch 경로로 간다.
    let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n";
    std::fs::write(wt.join("huge.txt"), line.repeat(110_000)).unwrap();

    let r = GitRunner::new();
    let diff = file_diff(&r, &wt, "main", "huge.txt", &ChangeStatus::Added)
        .await
        .unwrap();
    assert_eq!(
        diff,
        FileDiff::TooLarge {
            limit: suaegi_git::runner::MAX_DIFF_BYTES
        }
    );
}

/// 같은 규칙이 **삭제된** 큰 파일에도 걸린다. 그쪽은 `git show`가 파일 전체를
/// 내므로 patch가 아니라 **스니핑 단계**에서 상한에 걸린다 — 매핑을 빠뜨리면
/// 삭제된 큰 파일만 오류 배너를 띄우는 비대칭이 생긴다.
#[tokio::test]
async fn oversized_deleted_file_becomes_too_large_not_an_error() {
    // **merge base에 이미 있어야** 삭제로 보인다. worktree를 만든 뒤에 커밋하면
    // base 대비 "추가됐다 삭제됨" = diff에 아예 안 잡히는 도달 불가 상태다.
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n";
    std::fs::write(repo.path().join("huge.txt"), line.repeat(110_000)).unwrap();
    fixture::run(repo.path(), &["add", "huge.txt"]);
    fixture::run(repo.path(), &["commit", "-m", "huge"]);

    let r = GitRunner::new();
    let wt = add_worktree(&r, repo.path(), "feat", "main", ws.path())
        .await
        .unwrap()
        .path;
    std::fs::remove_file(wt.join("huge.txt")).unwrap();
    fixture::run(&wt, &["add", "-A"]);
    fixture::run(&wt, &["commit", "-m", "delete huge"]);

    let diff = file_diff(&r, &wt, "main", "huge.txt", &ChangeStatus::Deleted)
        .await
        .unwrap();
    assert_eq!(
        diff,
        FileDiff::TooLarge {
            limit: suaegi_git::runner::MAX_DIFF_BYTES
        }
    );
}

/// HEAD도 base도 둘 다 풀리지 않을 때 **무엇이 이기는가.** 플랜이 base를 먼저
/// 보라고 못 박았고, 그 순서는 이 입력에서만 보인다.
#[tokio::test]
async fn invalid_base_is_checked_before_unborn_head() {
    let repo = tempfile::tempdir().unwrap();
    fixture::run(repo.path(), &["init", "-b", "main"]);
    let r = GitRunner::new();
    let outcome = branch_compare(&r, repo.path(), "no-such-branch", &CompareHandle::new())
        .await
        .unwrap();
    assert_eq!(outcome, CompareOutcome::InvalidBase);
}

/// 심볼릭 링크를 **따라가지 않는다.** git은 링크를 링크 *내용*으로 다루고,
/// 따라가면 worktree 밖 파일을 읽어 사용자에게 보여주게 된다.
#[cfg(unix)]
#[tokio::test]
async fn symlink_is_read_as_its_target_path_not_followed() {
    let (_repo, _ws, wt) = setup().await;
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "SECRET-CONTENT\n").unwrap();
    std::os::unix::fs::symlink(&secret, wt.join("link.txt")).unwrap();

    let r = GitRunner::new();
    let head = file_head_bytes(
        &r,
        &wt,
        FileSource::WorkingTree,
        "link.txt",
        BINARY_SNIFF_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(head, secret.as_os_str().as_encoded_bytes());
    assert!(
        !head.windows(6).any(|w| w == b"SECRET"),
        "followed the symlink and read the target's contents"
    );
}

/// 중간 컴포넌트가 링크여도 안 된다. **마지막 컴포넌트에만 `symlink_metadata`를
/// 부르는 것은 부족하다** — 그 호출은 중간 링크를 이미 따라간 뒤다.
#[cfg(unix)]
#[tokio::test]
async fn intermediate_symlink_component_is_rejected() {
    let (_repo, _ws, wt) = setup().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "SECRET-CONTENT\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), wt.join("linkdir")).unwrap();

    let r = GitRunner::new();
    let err = file_head_bytes(
        &r,
        &wt,
        FileSource::WorkingTree,
        "linkdir/secret.txt",
        BINARY_SNIFF_BYTES,
    )
    .await
    .unwrap_err();
    assert!(
        format!("{err}").contains("traverses a symlink"),
        "unexpected error: {err}"
    );

    // 대조군: 진짜 디렉터리였다면 읽힌다. 위 거절이 "하위 경로는 전부 실패"라는
    // 뭉툭한 규칙이 아님을 고정한다.
    std::fs::create_dir(wt.join("realdir")).unwrap();
    std::fs::write(wt.join("realdir/ok.txt"), "OK\n").unwrap();
    let head = file_head_bytes(
        &r,
        &wt,
        FileSource::WorkingTree,
        "realdir/ok.txt",
        BINARY_SNIFF_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(head, b"OK\n");
}

#[tokio::test]
async fn paths_escaping_the_worktree_are_rejected() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    for path in ["../README.md", "/etc/passwd", "a/../../b", ""] {
        let err = file_head_bytes(&r, &wt, FileSource::WorkingTree, path, BINARY_SNIFF_BYTES)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, suaegi_git::runner::GitError::Io(e)
                if e.kind() == std::io::ErrorKind::InvalidInput),
            "path {path:?} was not rejected: {err}"
        );
    }
}

#[tokio::test]
async fn dirty_detection() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    // fixture 부산물이 untracked로 존재하므로 이 테스트는 tracked 변경으로 판별
    std::fs::write(wt.join("README.md"), "dirty\n").unwrap();
    assert!(working_tree_dirty(&r, &wt).await.unwrap());
}
