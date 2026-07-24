mod fixture;

use std::path::Path;
use std::process::Command;
use suaegi_git::history::{load_history, RefCategory, COMMIT_FORMAT, DEFAULT_LIMIT, MAX_LIMIT};
use suaegi_git::runner::GitRunner;

/// 격리된 config로 git을 돌리고 stdout(trim)을 돌려준다. `fixture::run`이 반환을
/// 안 줘서 oid 캡처용으로 별도 정의.
fn git_out(dir: &Path, args: &[&str]) -> String {
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
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 파일 하나 쓰고 커밋. 메시지 인자들은 각각 `-m`(git이 빈 줄로 문단 구분).
fn commit(dir: &Path, name: &str, content: &str, msg_parts: &[&str]) {
    std::fs::write(dir.join(name), content).unwrap();
    fixture::run(dir, &["add", name]);
    let mut args = vec!["commit"];
    for m in msg_parts {
        args.push("-m");
        args.push(m);
    }
    fixture::run(dir, &args);
}

fn runner() -> GitRunner {
    GitRunner::new()
}

/// c1(init) → c2 → [feature: c3] / [main: c4] → merge c5. 태그 v1은 c2.
/// 반환: (repo, c2_oid, c3_oid=feature tip, c4_oid=main-work, c5_oid=merge/HEAD)
fn build_merge_repo() -> (tempfile::TempDir, String, String, String, String) {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // c1 = "init"
    commit(p, "a.txt", "a\n", &["second"]); // c2
    let c2 = git_out(p, &["rev-parse", "HEAD"]);
    fixture::run(p, &["tag", "v1"]); // 태그 v1 on c2
    fixture::run(p, &["checkout", "-b", "feature"]);
    commit(p, "b.txt", "b\n", &["feature-work"]); // c3 on feature
    let c3 = git_out(p, &["rev-parse", "HEAD"]);
    fixture::run(p, &["checkout", "main"]);
    commit(p, "c.txt", "c\n", &["main-work"]); // c4 on main
    let c4 = git_out(p, &["rev-parse", "HEAD"]);
    fixture::run(p, &["merge", "--no-ff", "feature", "-m", "merge feature"]); // c5
    let c5 = git_out(p, &["rev-parse", "HEAD"]);
    (repo, c2, c3, c4, c5)
}

// ── keystone + %ct-오프셋(F2) crux ────────────────────────────────────────────
// 병합 커밋의 **정확한 부모 oid**와 tip decoration을 단언한다. `--format`에서 `%ct`를
// 지우면 [5]=P/[6]=decorate 오프셋이 한 칸씩 밀려 parents가 decoration 문자열을,
// decorations가 body를 읽어 이 단언들이 전부 깨진다.
#[tokio::test]
async fn parses_commits_branch_tag_merge_with_exact_parents() {
    let (repo, c2, c3, c4, c5) = build_merge_repo();
    let h = load_history(&runner(), repo.path(), DEFAULT_LIMIT)
        .await
        .unwrap();

    assert_eq!(h.items.len(), 5, "c1..c5 모두 파싱돼야 한다");

    let merge = h.items.iter().find(|c| c.id == c5).expect("merge commit");
    assert_eq!(merge.subject, "merge feature");
    // 부모 순서: first=main(c4), second=feature(c3). %ct 오프셋 shift면 여기가 깨진다.
    assert_eq!(
        merge.parents,
        vec![c4.clone(), c3.clone()],
        "병합 커밋 부모 oid/순서 (%ct 오프셋?)"
    );

    // tip에는 HEAD -> main. decoration 오프셋이 밀리면 이 ref가 안 나온다.
    let refs = &merge.references;
    assert_eq!(refs.len(), 1, "tip 커밋 decoration 개수");
    assert_eq!(refs[0].name, "main");
    assert_eq!(refs[0].category, RefCategory::Branches);
    assert_eq!(refs[0].id, "refs/heads/main");

    // 태그는 c2에.
    let second = h.items.iter().find(|c| c.id == c2).expect("second commit");
    assert_eq!(second.references.len(), 1);
    assert_eq!(second.references[0].name, "v1");
    assert_eq!(second.references[0].category, RefCategory::Tags);

    // feature 브랜치는 c3에.
    let feat = h.items.iter().find(|c| c.id == c3).expect("feature commit");
    assert_eq!(feat.references[0].name, "feature");
    assert_eq!(feat.references[0].category, RefCategory::Branches);

    // 비병합 커밋은 부모 1개(또는 root는 0). c3는 부모 1.
    assert_eq!(feat.parents.len(), 1);
}

/// `--format` 문자열이 byte-for-byte인지 상수 레벨에서 못박는다(우발적 편집 가드).
#[test]
fn commit_format_is_byte_for_byte() {
    assert_eq!(
        COMMIT_FORMAT,
        "%H%n%aN%n%aE%n%at%n%ct%n%P%n%(decorate:prefix=,suffix=,separator=%x1f)%n%B"
    );
}

// ── 다중 커밋 레코드 파싱(count) ──────────────────────────────────────────────
// 선형 3커밋 → 정확히 3개 파싱. NUL-split 레코드 경계가 맞는지 확인한다.
// (F4 leading-`\n` strip 자체의 mutation-kill은 실측 git 2.50.1이 선행 개행을
// 안 붙여 여기선 안 죽는다 → `history.rs`의 `strips_leading_newline_per_record`
// 유닛 테스트가 hermetic 입력으로 그 strip을 못박는다.)
#[tokio::test]
async fn counts_all_commits() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // c1
    commit(p, "a.txt", "a\n", &["c2"]);
    commit(p, "b.txt", "b\n", &["c3"]);

    let h = load_history(&runner(), p, DEFAULT_LIMIT).await.unwrap();
    assert_eq!(h.items.len(), 3, "선행 개행 strip 누락 시 커밋이 drop된다");
    assert!(!h.has_more);
}

// ── unborn HEAD → 빈 History, 에러 아님 ───────────────────────────────────────
#[tokio::test]
async fn unborn_head_returns_empty_not_error() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    std::fs::write(p.join(".test-gitconfig"), "").unwrap();
    fixture::run(p, &["init", "-b", "main"]); // 커밋 0개

    let h = load_history(&runner(), p, DEFAULT_LIMIT)
        .await
        .expect("unborn은 Ok(empty)여야 한다");
    assert!(h.items.is_empty());
    assert!(h.current_ref.is_none());
    assert!(!h.has_more);
    assert!(!h.has_incoming);
    assert!(!h.has_outgoing);
    assert_eq!(h.limit, DEFAULT_LIMIT);
}

// ── detached HEAD → short-hash/Commits 폴백, 에러 아님 ─────────────────────────
#[tokio::test]
async fn detached_head_falls_back_to_short_hash() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // c1
    commit(p, "a.txt", "a\n", &["c2"]);
    let c2 = git_out(p, &["rev-parse", "HEAD"]);
    fixture::run(p, &["checkout", &c2]); // detached

    let h = load_history(&runner(), p, DEFAULT_LIMIT)
        .await
        .expect("detached는 Ok여야 한다");
    let cur = h.current_ref.expect("detached도 current_ref는 있다");
    assert_eq!(cur.category, RefCategory::Commits, "detached 폴백 카테고리");
    assert_eq!(cur.name, &c2[..7], "detached는 short-hash 이름");
    assert_eq!(cur.id, c2);
    // 로그 자체는 정상적으로 나온다.
    assert_eq!(h.items.len(), 2);
}

// ── limit clamp + hasMore crux ────────────────────────────────────────────────
#[tokio::test]
async fn limit_clamp_and_has_more() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // c1
    for i in 2..=5 {
        commit(p, "a.txt", &format!("v{i}\n"), &[&format!("c{i}")]); // c2..c5, 총 5
    }

    // limit 3 → 정확히 3개 + 더 있음.
    let h = load_history(&runner(), p, 3).await.unwrap();
    assert_eq!(h.items.len(), 3);
    assert!(h.has_more, "5 > 3");
    assert_eq!(h.limit, 3);

    // limit 0 → 1로 clamp.
    let h0 = load_history(&runner(), p, 0).await.unwrap();
    assert_eq!(h0.limit, 1);
    assert_eq!(h0.items.len(), 1);
    assert!(h0.has_more);

    // limit 50 → 전부, 더 없음.
    let hall = load_history(&runner(), p, 50).await.unwrap();
    assert_eq!(hall.items.len(), 5);
    assert!(!hall.has_more);

    // limit 999 → MAX_LIMIT로 clamp(상한 초과 방어).
    let hmax = load_history(&runner(), p, 999).await.unwrap();
    assert_eq!(hmax.limit, MAX_LIMIT);
}

// ── decoration category + %x1f split(F3) crux ─────────────────────────────────
// 한 커밋에 HEAD/main + 태그 + 리모트 ref를 모두 얹어 decoration 필드가 x1f로
// 여러 조각이 되게 한다. split 문자를 comma로 바꾸면 통째로 한 조각이 돼 ref가
// 1개(이름도 오염)로 파싱돼 실패한다.
#[tokio::test]
async fn decoration_categories_and_x1f_split() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // c1 = 유일 커밋
    let c1 = git_out(p, &["rev-parse", "HEAD"]);
    fixture::run(p, &["tag", "v1"]);
    fixture::run(p, &["update-ref", "refs/remotes/origin/main", &c1]);

    let h = load_history(&runner(), p, DEFAULT_LIMIT).await.unwrap();
    assert_eq!(h.items.len(), 1);
    let refs = &h.items[0].references;
    // 3개(main/origin-main/v1)로 파싱 = x1f split이 살아있다는 증거.
    assert_eq!(refs.len(), 3, "x1f split이 comma면 1개로 뭉개진다");
    // category 정렬: heads < remotes < tags.
    assert_eq!(refs[0].name, "main");
    assert_eq!(refs[0].category, RefCategory::Branches);
    assert_eq!(refs[1].name, "origin/main");
    assert_eq!(refs[1].category, RefCategory::RemoteBranches);
    assert_eq!(refs[1].id, "refs/remotes/origin/main");
    assert_eq!(refs[2].name, "v1");
    assert_eq!(refs[2].category, RefCategory::Tags);
}

// ── body 여러 줄 crux ─────────────────────────────────────────────────────────
#[tokio::test]
async fn parses_multiline_body() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p);
    // subject + 본문 2줄. `-m subject -m "l1\nl2"` → "subject\n\nl1\nl2".
    commit(
        p,
        "a.txt",
        "a\n",
        &["subject line", "body line 1\nbody line 2"],
    );

    let h = load_history(&runner(), p, DEFAULT_LIMIT).await.unwrap();
    let top = &h.items[0];
    assert_eq!(top.subject, "subject line");
    assert_eq!(
        top.body, "subject line\n\nbody line 1\nbody line 2",
        "body는 여러 줄 원본 전체(후행 개행 1개만 제거)"
    );
}

// ── has_incoming/has_outgoing (merge-base 불린) ───────────────────────────────
#[tokio::test]
async fn no_upstream_means_both_false() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p);
    commit(p, "a.txt", "a\n", &["c2"]);

    let h = load_history(&runner(), p, DEFAULT_LIMIT).await.unwrap();
    assert!(h.remote_ref.is_none());
    assert!(!h.has_incoming);
    assert!(!h.has_outgoing);
    assert!(h.merge_base.is_none());
}

#[tokio::test]
async fn local_ahead_is_outgoing() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // c1
    let c1 = git_out(p, &["rev-parse", "HEAD"]);
    commit(p, "a.txt", "a\n", &["c2"]); // c2 = HEAD
                                        // 리모트는 c1(뒤처짐), 업스트림 설정.
    fixture::run(p, &["update-ref", "refs/remotes/origin/main", &c1]);
    // 실제 clone에는 항상 있는 fetch refspec — 없으면 %(upstream)이 비어 나온다.
    fixture::run(
        p,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    fixture::run(p, &["config", "branch.main.remote", "origin"]);
    fixture::run(p, &["config", "branch.main.merge", "refs/heads/main"]);

    let h = load_history(&runner(), p, DEFAULT_LIMIT).await.unwrap();
    let rr = h.remote_ref.as_ref().expect("업스트림 해석돼야 한다");
    assert_eq!(rr.name, "origin/main");
    assert_eq!(rr.category, RefCategory::RemoteBranches);
    assert_eq!(h.merge_base.as_deref(), Some(c1.as_str()));
    assert!(h.has_outgoing, "로컬이 앞서면 outgoing");
    assert!(!h.has_incoming);
}

#[tokio::test]
async fn remote_ahead_is_incoming() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // c1
    commit(p, "a.txt", "a\n", &["c2"]); // c2
    let c2 = git_out(p, &["rev-parse", "HEAD"]);
    commit(p, "b.txt", "b\n", &["c3"]); // c3
    let c3 = git_out(p, &["rev-parse", "HEAD"]);
    // 리모트는 c3(앞섬), 로컬은 c2로 되감기.
    fixture::run(p, &["update-ref", "refs/remotes/origin/main", &c3]);
    fixture::run(p, &["reset", "--hard", &c2]);
    fixture::run(
        p,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    fixture::run(p, &["config", "branch.main.remote", "origin"]);
    fixture::run(p, &["config", "branch.main.merge", "refs/heads/main"]);

    let h = load_history(&runner(), p, DEFAULT_LIMIT).await.unwrap();
    assert_eq!(h.merge_base.as_deref(), Some(c2.as_str()));
    assert!(h.has_incoming, "리모트가 앞서면 incoming");
    assert!(!h.has_outgoing);
}
