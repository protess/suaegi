//! M1 — 스테이징 write-ops: `stage`/`unstage`(single) + `bulk_stage`/`bulk_unstage`.
//!
//! Orca `status.ts`의 `stageFile`(:1882)/`unstageFile`(:1901)/`bulkStageFiles`(:2173)/
//! `bulkUnstageFiles`(:2198) 포팅. 전부 `GitRunner`를 거쳐 그 타임아웃/출력상한/
//! `GIT_TERMINAL_PROMPT=0` 규율을 물려받는다 — raw `Command` 금지. 사용자 전역 config는
//! 절대 미접촉이다(`GitRunner`가 identity/`-c` override를 안 붙인다 — 여기서도 안 붙인다).
//!
//! **`:(literal)` pathspec이 핵심이다.** `a*.txt`처럼 glob 문자를 담거나 `-n`처럼
//! 플래그로 보이는 파일명을 git이 오해하지 않도록 리터럴로 못 박는다(실측: 바 `a*.txt`는
//! `a1.txt`까지 스테이징하지만 `:(literal)a*.txt`는 그 파일 하나만).

use crate::compare::resolve_preserve_symlink;
use crate::runner::{GitError, GitRunner};
use std::path::Path;

/// 한 번의 `git add`/`restore` 호출에 실을 pathspec 최대 개수. 경로가 많으면 argv가
/// `E2BIG`를 치므로 이 크기로 청크한다(Orca `BULK_CHUNK_SIZE`, status.ts:63 = 100).
pub(crate) const BULK_CHUNK_SIZE: usize = 100;

/// WSL 아래 git은 POSIX 경로를 원하지만 호스트 경로는 리터럴로 유지해야 하므로
/// **백슬래시만** 슬래시로 바꾼다(Orca `literalPathspec`, status.ts:2043).
///
/// suaegi는 아직 WSL distro 런타임 옵션을 배선하지 않는다(macOS-first) — 그래서
/// 런타임 경로에서 이 재작성은 항상 꺼져 있고, 규칙 자체는 `literal_pathspec_impl`에
/// 순수 함수로 구현해 win32-style 입력으로 직접 테스트한다. Windows/WSL 런타임에서
/// 이 플래그를 켜는 배선은 follow-up이다(`cfg!(windows)`를 stand-in으로 둔다).
const REWRITE_BACKSLASH: bool = cfg!(windows);

/// `<path>` → `:(literal)<path>`. glob/플래그로 보이는 파일명을 git이 리터럴로 다루게
/// 강제한다. M4 discard도 재사용하므로 `pub(crate)`로 노출한다.
pub(crate) fn literal_pathspec(path: &str) -> String {
    literal_pathspec_impl(path, REWRITE_BACKSLASH)
}

/// `literal_pathspec`의 순수 구현. `rewrite_backslash`가 true면 백슬래시→슬래시
/// (WSL 규칙)를 적용한다. 플랫폼과 무관하게 규칙을 직접 테스트할 수 있게 분리했다.
fn literal_pathspec_impl(path: &str, rewrite_backslash: bool) -> String {
    if rewrite_backslash {
        format!(":(literal){}", path.replace('\\', "/"))
    } else {
        format!(":(literal){path}")
    }
}

/// 한 경로를 스테이징한다 — `git add -- :(literal)<path>`.
pub async fn stage(runner: &GitRunner, worktree: &Path, path: &str) -> Result<(), GitError> {
    let spec = literal_pathspec(path);
    runner.run(worktree, &["add", "--", &spec]).await?;
    Ok(())
}

/// 한 경로를 언스테이징한다 — `git restore --staged -- :(literal)<path>`.
pub async fn unstage(runner: &GitRunner, worktree: &Path, path: &str) -> Result<(), GitError> {
    let spec = literal_pathspec(path);
    runner
        .run(worktree, &["restore", "--staged", "--", &spec])
        .await?;
    Ok(())
}

/// 여러 경로를 청크(`BULK_CHUNK_SIZE`) 단위로 스테이징한다 — 각 청크가 한 번의
/// `git add -- <specs...>` 호출.
///
/// **비-트랜잭션(plan F5).** 청크는 원자적이다: `git add`는 한 pathspec이라도 아무것도
/// 매칭 못 하면 청크 전체를 실패시키고 그 청크의 어떤 경로도 스테이징하지 않는다(실측).
/// 그래서 `Result<(), E>` 대신 **입력 경로별 결과 벡터**를 돌려준다:
/// - 성공한 청크의 경로들 → 각 `Ok(())`.
/// - 실패한 청크의 경로들 → 각 그 청크의 에러(복제)를 `Err`로. 다른 청크는 영향 없다.
///
/// 반환 벡터는 입력과 **같은 순서·같은 길이**다(경로마다 정확히 한 결과).
pub async fn bulk_stage(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
) -> Vec<(String, Result<(), GitError>)> {
    bulk_apply(runner, worktree, paths, &["add", "--"]).await
}

/// 여러 경로를 청크 단위로 언스테이징한다 — `git restore --staged -- <specs...>`.
/// 세맨틱은 `bulk_stage`와 동일하다(per-path 결과 벡터, 청크 원자성).
pub async fn bulk_unstage(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
) -> Vec<(String, Result<(), GitError>)> {
    bulk_apply(runner, worktree, paths, &["restore", "--staged", "--"]).await
}

/// `bulk_stage`/`bulk_unstage`의 공통 청크 실행. `prefix`는 pathspec 앞에 오는 고정
/// argv(`["add", "--"]` 또는 `["restore", "--staged", "--"]`).
async fn bulk_apply(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
    prefix: &[&str],
) -> Vec<(String, Result<(), GitError>)> {
    let mut results = Vec::with_capacity(paths.len());
    for chunk in paths.chunks(BULK_CHUNK_SIZE) {
        // pathspec 문자열은 argv가 참조하는 동안 살아 있어야 한다 — 먼저 소유로 만든다.
        let specs: Vec<String> = chunk.iter().map(|p| literal_pathspec(p)).collect();
        let mut args: Vec<&str> = prefix.to_vec();
        args.extend(specs.iter().map(String::as_str));

        match runner.run(worktree, &args).await {
            Ok(_) => {
                for p in chunk {
                    results.push((p.to_string(), Ok(())));
                }
            }
            // 청크가 원자적으로 실패했다 — 그 청크의 모든 경로에 같은 에러를 준다.
            // `GitError`는 `Io` variant 때문에 `Clone`이 아니라 `duplicate_error`로 복제한다.
            Err(e) => {
                for p in chunk {
                    results.push((p.to_string(), Err(duplicate_error(&e))));
                }
            }
        }
    }
    results
}

/// `GitError`를 값-복제한다. `GitError`는 `Io(std::io::Error)` variant를 담아
/// `#[derive(Clone)]`이 안 되므로, per-path 결과에 같은 에러를 여러 벌 실으려면
/// variant별로 손수 복제한다. `Io`는 kind+메시지를 보존해 새 `io::Error`로 재구성한다.
fn duplicate_error(e: &GitError) -> GitError {
    match e {
        GitError::Io(io) => GitError::Io(std::io::Error::new(io.kind(), io.to_string())),
        GitError::Timeout { args } => GitError::Timeout { args: args.clone() },
        GitError::Failed { args, code, stderr } => GitError::Failed {
            args: args.clone(),
            code: *code,
            stderr: stderr.clone(),
        },
        GitError::Parse { args, detail } => GitError::Parse {
            args: args.clone(),
            detail: detail.clone(),
        },
        GitError::OutputTooLarge { limit } => GitError::OutputTooLarge { limit: *limit },
    }
}

// --- M2: 커밋 (`commit_changes`) ---

/// `commit_changes`의 결과. Orca `commitChanges`의 `{ success, error? }`를 모델링한다
/// (`status.ts:1962-1990`). git이 **돌긴 했으나** 실패한 경우(hook/GPG 거부, empty index)를
/// 담는다 — git을 **아예 못 돌린** 경우(spawn/timeout)는 `commit_changes`가 `GitError`로
/// 돌려주는, 이와 **별개의** 실패다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// 커밋 성공(exit 0).
    Committed,
    /// git이 돌았으나 커밋을 거부/중단했다(non-zero exit). `message`는 채널 우선순위
    /// 규칙(stderr→stdout→generic)으로 고른, 사람이 읽을 사유다.
    Failed { message: String },
}

/// 커밋 실패 시 사람에게 보일 메시지를 고른다 — **stderr → stdout → generic**
/// (Orca `status.ts:1972-1986`). hook/GPG 실패는 stderr로, "nothing to commit"은 stdout으로
/// 나오므로 **비어 있지 않은** stderr를 먼저, 없으면 stdout을, 둘 다 비면 generic fallback.
///
/// **순수 함수 — 이 마일스톤의 핵심 crux.** 우선순위를 뒤집거나(stdout 먼저) 한 채널을
/// 지우는 mutation은 unit 테스트가 잡는다.
fn pick_commit_error(stdout: &str, stderr: &str) -> String {
    if !stderr.is_empty() {
        stderr.to_string()
    } else if !stdout.is_empty() {
        stdout.to_string()
    } else {
        "Commit failed".to_string()
    }
}

/// `(stdout, stderr, exit code)`를 `CommitOutcome`으로 분류하는 순수 함수. exit 0이면
/// `Committed`, 아니면 `Failed`(메시지는 `pick_commit_error`). code가 load-bearing이라
/// "non-zero를 Committed로" 뒤집는 mutation을 empty-index 테스트가 잡는다.
fn classify_commit(stdout: &str, stderr: &str, code: i32) -> CommitOutcome {
    if code == 0 {
        CommitOutcome::Committed
    } else {
        CommitOutcome::Failed {
            message: pick_commit_error(stdout, stderr),
        }
    }
}

/// 스테이징된 변경을 커밋한다 — `git commit -m <message>`.
///
/// **F3/F4 불변식(plan §1):** `-c user.name/user.email`, `commit.gpgsign`, `--no-verify`,
/// 전역 config 접촉 — **어느 것도 하지 않는다.** 실 유저로 그의 repo에 bare 커밋한다
/// (identity override는 서명 제거+가짜 author 회귀, `--no-verify`는 에이전트의 조용한
/// hook 우회다). `message`는 **별개 argv 원소**로 넘겨(절대 shell 보간 없음) 선행 대시나
/// 셸 메타문자가 담긴 메시지도 리터럴로 커밋된다.
///
/// - exit 0 → `Ok(Committed)`.
/// - git이 돌았으나 non-zero(예: empty index는 exit 1 + stdout "nothing to commit") →
///   `Ok(Failed { message })`. 이건 커밋 실패이지 crate 에러가 아니다.
/// - git을 아예 못 돌림(spawn 실패/타임아웃/출력 초과) → `Err(GitError)`.
pub async fn commit_changes(
    runner: &GitRunner,
    worktree: &Path,
    message: &str,
) -> Result<CommitOutcome, GitError> {
    // exit 1은 "돌긴 했으나 실패"의 흔한 코드다("nothing to commit"은 stdout, hook/GPG는
    // stderr). `run_expecting(&[1])`로 exit 1을 에러가 아닌 성공으로 받아 **양쪽 채널과
    // exit code**를 그대로 손에 넣는다 — `GitError::Failed`는 stderr만 담고 stdout·code를
    // 버려 "nothing to commit"을 잃기 때문이다.
    match runner
        .run_expecting(worktree, &["commit", "-m", message], &[1])
        .await
    {
        Ok(out) => Ok(classify_commit(&out.stdout, &out.stderr, out.code)),
        // git이 돌았으나 예상 밖 non-zero(예: 128 fatal). 여전히 "돌고 실패"이므로
        // `GitError`가 아니라 `Failed`로 올린다. Orca는 error 객체로 양쪽 채널을 다
        // 갖지만 `GitError::Failed`는 stdout을 버려 stderr만 남는다 — 이 코드들(예: 128
        // "not a git repository")은 사유가 stderr에 실려 stdout 소실이 무해하다.
        Err(GitError::Failed { stderr, code, .. }) => {
            Ok(classify_commit("", &stderr, code.unwrap_or(-1)))
        }
        // git을 못 돌렸다(spawn/timeout/output-too-large) — 진짜 crate 에러.
        Err(e) => Err(e),
    }
}

// --- M4: discard (`discard`/`bulk_discard`) — 데이터손실/보안 ---

/// 한 타깃을 discard한 결과. discard는 **삭제/복원**이라 "무엇을 했는가"를 호출부가
/// 알아야 UI가 정확히 보고할 수 있다(그냥 `()`면 no-op과 실제 삭제를 구별 못 한다).
///
/// **경로 안전 거부(traversal/`..`/absolute/null-byte/중간-심링크 escape)는 이 enum이
/// 아니라 `Err(GitError)`로 나간다** — 그건 "아무것도 안 했고, 위험해서 거부했다"이지
/// 성공 결과가 아니다. 존재하지 않는 타깃만 `NothingToDiscard`(멱등 성공)다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscardOutcome {
    /// tracked 경로를 HEAD 내용으로 되돌렸다 — `git restore --worktree --source=HEAD`.
    /// **워크트리만** 바꾼다(인덱스 미접촉). staged-add(HEAD에 없음)에 대해선 이 git
    /// (2.50.1)이 워크트리 파일을 **제거**하고 인덱스의 staged 항목은 남긴다(= 워크트리를
    /// HEAD 상태로 맞춤). Codex/플랜은 "restore가 실패한다"고 봤으나 실측은 exit 0 —
    /// **의도적으로 이 결과(RestoredTracked)로 흡수**하고, restore가 non-zero인 git
    /// 버전에서는 `Err(GitError)`로 표면화된다(조용한 미검 경로 없음).
    RestoredTracked,
    /// untracked 경로(파일/디렉터리/심링크)를 제거했다 — `git clean -ffdx`(ignored 포함).
    /// leaf 심링크는 **링크 자체**만 지운다(타깃 미추적).
    RemovedUntracked,
    /// 타깃이 존재하지 않는다 — 멱등 no-op 성공. 지울 게 없다(NotFound). Orca처럼
    /// nearest-existing-parent까지 walk-up하지 않는다(delete엔 불필요).
    NothingToDiscard,
}

/// `resolve_preserve_symlink` 검증 결과의 3분기. NotFound(멱등 no-op)와 거부(위험)를
/// 갈라야 하는데 둘 다 `Err(io)`라 kind로 구별한다.
enum Validation {
    /// 워크트리 안의 실존 경로(leaf 심링크 포함 — 링크 자체가 대상). discard 가능.
    Valid,
    /// NotFound — 지울 게 없다. 멱등 성공.
    NothingToDiscard,
    /// traversal/`..`/absolute/null-byte/중간-심링크 escape 등. **아무것도 지우지 않고 거부.**
    Rejected(std::io::Error),
}

/// discard 타깃을 워크트리 경계 안으로 검증한다 — 기존 primitive(`compare.rs`
/// `resolve_preserve_symlink`, `ExistingOnly`)를 **재사용**한다(Orca `git-discard-path-safety.ts`
/// realpath-lexical 포팅 금지 — 컴포넌트별 walk가 중간 심링크를 위치 무관 무조건 거부해
/// strictly stronger). leaf 심링크는 `Resolved::Symlink`로 un-follow 반환 = "링크 자체 삭제"
/// 라 **허용**한다. NotFound만 멱등 no-op, 그 외 에러는 전부 거부.
fn validate_target(worktree: &Path, path: &str) -> Validation {
    match resolve_preserve_symlink(worktree, path) {
        Ok(_) => Validation::Valid,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Validation::NothingToDiscard,
        Err(e) => Validation::Rejected(e),
    }
}

/// tracked 프로브 — `git ls-files --error-unmatch -- :(literal)<path>`. exit 0=tracked,
/// 1=untracked. 디렉터리 pathspec도 하위에 tracked 파일이 하나라도 있으면 exit 0.
async fn probe_tracked(runner: &GitRunner, worktree: &Path, path: &str) -> Result<bool, GitError> {
    let spec = literal_pathspec(path);
    let out = runner
        .run_expecting(
            worktree,
            &["ls-files", "--error-unmatch", "--", &spec],
            &[1],
        )
        .await?;
    Ok(out.code == 0)
}

/// 한 경로의 워크트리 변경을 discard한다.
///
/// 순서(최소-안전 스펙): (1) `validate_target`로 경계 검증 — NotFound→멱등 no-op,
/// 거부→`Err`(아무것도 안 지움); (2) tracked 프로브; (3) tracked→`restore --worktree
/// --source=HEAD`(**`--worktree`만** — bare로 떨궈도 git는 --worktree 기본이라 같으나,
/// `--staged`가 붙으면 인덱스까지 리셋되므로 명시); (4) untracked→**clean 직전 1회
/// 재검증**(TOCTOU: tracked-restore 단계의 `.gitattributes` smudge/clean 필터 부작용
/// 가능성) 후 `git clean -ffdx`.
///
/// 마지막 재검증~`clean` exec 사이의 좁은 TOCTOU는 남는다 — `fs.rs:219` 쓰기 staleness와
/// 같은 자세로 **명시하고 수용**한다(락 안 검). 그 창에서 경로가 심링크로 바뀌어도
/// `:(literal)` pathspec-bounded `clean`은 심링크 부모로 내려가지 않아 blast가 제한된다.
pub async fn discard(
    runner: &GitRunner,
    worktree: &Path,
    path: &str,
) -> Result<DiscardOutcome, GitError> {
    match validate_target(worktree, path) {
        Validation::Valid => {}
        Validation::NothingToDiscard => return Ok(DiscardOutcome::NothingToDiscard),
        Validation::Rejected(e) => return Err(GitError::Io(e)),
    }

    if probe_tracked(runner, worktree, path).await? {
        let spec = literal_pathspec(path);
        // `--worktree`만: 인덱스는 건드리지 않는다. `--staged`를 더하면 staged 변경까지
        // 날아간다(데이터 손실). restore가 non-zero면 `run`이 `Err`로 표면화한다.
        runner
            .run(
                worktree,
                &["restore", "--worktree", "--source=HEAD", "--", &spec],
            )
            .await?;
        return Ok(DiscardOutcome::RestoredTracked);
    }

    // untracked: clean 직전 재검증(TOCTOU). NotFound면 그새 사라진 것 → 멱등 성공.
    match validate_target(worktree, path) {
        Validation::Valid => {}
        Validation::NothingToDiscard => return Ok(DiscardOutcome::NothingToDiscard),
        Validation::Rejected(e) => return Err(GitError::Io(e)),
    }
    let spec = literal_pathspec(path);
    // `-ffdx`: force + 디렉터리 + **ignored 포함**(Q4 의도적). `:(literal)`로 blast 제한.
    runner
        .run(worktree, &["clean", "-ffdx", "--", &spec])
        .await?;
    Ok(DiscardOutcome::RemovedUntracked)
}

/// `git ls-files -z -- <specs...>`를 청크 배치로 돌려 tracked 파일 경로들을 모은다.
/// bulk 파티션용 — untracked/ignored spec은 출력에 아무것도 안 실으므로 tracked만 남는다.
/// **빈 입력이면 git을 부르지 않는다**(specs 없는 `ls-files`는 워크트리 전체를 낸다).
async fn list_tracked(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
) -> Result<Vec<String>, GitError> {
    let mut tracked = Vec::new();
    for chunk in paths.chunks(BULK_CHUNK_SIZE) {
        let specs: Vec<String> = chunk.iter().map(|p| literal_pathspec(p)).collect();
        let mut args: Vec<&str> = vec!["ls-files", "-z", "--"];
        args.extend(specs.iter().map(String::as_str));
        let out = runner.run(worktree, &args).await?;
        for t in out.stdout.split('\0') {
            if !t.is_empty() {
                tracked.push(t.to_string());
            }
        }
    }
    Ok(tracked)
}

/// 백슬래시→`/` 정규화 + 후행 슬래시 제거(Orca `normalizeGitPathForCompare`, status.ts:2039).
fn normalize_git_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// bulk 파티션 술어: `input`이 tracked인가. `tracked_paths`는 `git ls-files -z`가 낸 실제
/// tracked 파일 경로들. **디렉터리**를 discard 대상으로 주면 ls-files는 그 하위 파일들을
/// 내므로 input 자체는 목록에 없다 — 그래서 정확히 같거나 `input/`로 시작하는 tracked
/// 경로가 하나라도 있으면 tracked로 본다(Orca `isTrackedPathSpec`, status.ts:2049).
///
/// **순수 함수 — 접두 매칭이 crux.** `input/`(슬래시 포함)이 아니라 `input`으로 시작만
/// 보면 `dir`이 `dirother/...`를 tracked로 오판한다. mutation을 unit 테스트가 잡는다.
fn is_tracked_pathspec(input: &str, tracked_paths: &[String]) -> bool {
    let normalized = normalize_git_path(input);
    let prefix = format!("{normalized}/");
    tracked_paths.iter().any(|t| {
        let nt = normalize_git_path(t);
        nt == normalized || nt.starts_with(&prefix)
    })
}

/// `(원본 index, path)` 목록을 청크(`BULK_CHUNK_SIZE`)로 한 git 명령에 실어 돌리고,
/// **청크 원자성**을 per-path 결과로 편다(M1 `bulk_apply`와 같은 자세): 청크가 성공하면
/// 그 경로들 전부 `on_ok`, 실패하면 전부 그 청크의 에러(복제). 결과는 원본 index 자리에 쓴다.
async fn run_chunked(
    runner: &GitRunner,
    worktree: &Path,
    items: &[(usize, &str)],
    prefix: &[&str],
    on_ok: DiscardOutcome,
    results: &mut [Option<Result<DiscardOutcome, GitError>>],
) {
    for chunk in items.chunks(BULK_CHUNK_SIZE) {
        let specs: Vec<String> = chunk.iter().map(|(_, p)| literal_pathspec(p)).collect();
        let mut args: Vec<&str> = prefix.to_vec();
        args.extend(specs.iter().map(String::as_str));
        match runner.run(worktree, &args).await {
            Ok(_) => {
                for (idx, _) in chunk {
                    results[*idx] = Some(Ok(on_ok.clone()));
                }
            }
            Err(e) => {
                for (idx, _) in chunk {
                    results[*idx] = Some(Err(duplicate_error(&e)));
                }
            }
        }
    }
}

/// 여러 경로의 워크트리 변경을 discard한다. **비-트랜잭션**(plan F5): per-path 결과 벡터를
/// 입력과 같은 순서·길이로 돌려준다.
///
/// (1) **모든 경로를 먼저 검증**한다 — 거부는 `Err`로 그 자리에 기록되고 **git 명령에
/// 절대 실리지 않는다**(escape가 다른 경로의 discard를 막지도, 바깥 타깃을 지우지도 않게
/// 하는 핵심 불변식); NotFound는 `NothingToDiscard`. (2) 유효 경로를 `list_tracked`로
/// tracked/untracked 파티션. (3) untracked는 **clean 직전 재검증**(TOCTOU) 후 청크
/// `clean -ffdx`, tracked는 청크 `restore --worktree --source=HEAD`. 각 청크는 원자적이다.
pub async fn bulk_discard(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&str],
) -> Vec<(String, Result<DiscardOutcome, GitError>)> {
    // 1. 전부 선-검증. 거부/NotFound는 즉시 확정, 유효 경로의 원본 index만 모은다.
    let mut results: Vec<Option<Result<DiscardOutcome, GitError>>> =
        Vec::with_capacity(paths.len());
    let mut valid_idx: Vec<usize> = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        match validate_target(worktree, p) {
            Validation::Valid => {
                results.push(None);
                valid_idx.push(i);
            }
            Validation::NothingToDiscard => {
                results.push(Some(Ok(DiscardOutcome::NothingToDiscard)))
            }
            Validation::Rejected(e) => results.push(Some(Err(GitError::Io(e)))),
        }
    }

    // 2. 유효 경로만 tracked/untracked 파티션(`git ls-files -z`). ls-files 자체가 실패하면
    //    유효 경로 전부에 그 에러를 실어 반환(git을 못 돌린 상황).
    let valid_paths: Vec<&str> = valid_idx.iter().map(|&i| paths[i]).collect();
    let tracked_paths = match list_tracked(runner, worktree, &valid_paths).await {
        Ok(t) => t,
        Err(e) => {
            for &i in &valid_idx {
                results[i] = Some(Err(duplicate_error(&e)));
            }
            return finalize_bulk(paths, results);
        }
    };

    // 3a. untracked: clean 직전 재검증(TOCTOU). 실패한 경로는 청크에서 제외하고 즉시 확정 —
    //     **검증 못 통과한 경로는 clean 명령에 절대 안 실린다.**
    let mut untracked_items: Vec<(usize, &str)> = Vec::new();
    let mut tracked_items: Vec<(usize, &str)> = Vec::new();
    for &i in &valid_idx {
        let p = paths[i];
        if is_tracked_pathspec(p, &tracked_paths) {
            tracked_items.push((i, p));
        } else {
            match validate_target(worktree, p) {
                Validation::Valid => untracked_items.push((i, p)),
                Validation::NothingToDiscard => {
                    results[i] = Some(Ok(DiscardOutcome::NothingToDiscard))
                }
                Validation::Rejected(e) => results[i] = Some(Err(GitError::Io(e))),
            }
        }
    }

    // 3b. 청크 실행. tracked는 재검증 안 한다(restore는 rm이 아니라 pathspec-bounded 복원이라
    //     심링크 부모로 내려가지 않고, plan TOCTOU 재검증은 `clean` 직전만 요구).
    run_chunked(
        runner,
        worktree,
        &tracked_items,
        &["restore", "--worktree", "--source=HEAD", "--"],
        DiscardOutcome::RestoredTracked,
        &mut results,
    )
    .await;
    run_chunked(
        runner,
        worktree,
        &untracked_items,
        &["clean", "-ffdx", "--"],
        DiscardOutcome::RemovedUntracked,
        &mut results,
    )
    .await;

    finalize_bulk(paths, results)
}

/// per-path 결과를 입력 순서로 조립한다. 모든 index가 채워졌어야 한다(유효 경로는 청크
/// 실행 또는 TOCTOU 제외에서, 나머지는 선-검증에서).
fn finalize_bulk(
    paths: &[&str],
    results: Vec<Option<Result<DiscardOutcome, GitError>>>,
) -> Vec<(String, Result<DiscardOutcome, GitError>)> {
    results
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            (
                paths[i].to_string(),
                r.expect("every path must have an outcome"),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        classify_commit, is_tracked_pathspec, literal_pathspec, literal_pathspec_impl,
        pick_commit_error, validate_target, CommitOutcome, Validation,
    };

    // literal_pathspec: 평범한 경로 → :(literal) 접두.
    #[test]
    fn plain_path_gets_literal_prefix() {
        assert_eq!(literal_pathspec("foo.rs"), ":(literal)foo.rs");
    }

    // glob 문자/선행 대시가 담긴 파일명도 그대로 리터럴로 감싼다 — 접두를 떼면
    // git이 glob/플래그로 오해한다(그게 :(literal)의 존재 이유). 접두를 지우는 mutation은
    // 이 단언과 real-git 라운드트립 테스트를 함께 깬다.
    #[test]
    fn glob_and_dash_names_are_literal_wrapped() {
        assert_eq!(literal_pathspec("a[1].rs"), ":(literal)a[1].rs");
        assert_eq!(literal_pathspec("a*.txt"), ":(literal)a*.txt");
        assert_eq!(literal_pathspec("-n"), ":(literal)-n");
    }

    // WSL 규칙: rewrite_backslash=true면 win32-style 입력의 백슬래시가 슬래시로.
    // macOS 런타임에선 플래그가 꺼져 있어(REWRITE_BACKSLASH=false) 순수 impl을 직접 친다.
    #[test]
    fn wsl_rule_rewrites_backslashes() {
        assert_eq!(
            literal_pathspec_impl(r"src\main\a.rs", true),
            ":(literal)src/main/a.rs"
        );
        // 규칙이 꺼져 있으면 백슬래시를 보존한다.
        assert_eq!(
            literal_pathspec_impl(r"src\main\a.rs", false),
            r":(literal)src\main\a.rs"
        );
    }

    // --- M2 crux: 채널 우선순위 picker (순수) ---

    // stderr가 비어있지 않으면 stderr를 고른다(hook/GPG 실패는 stderr로 온다).
    // mutation "stdout 먼저"는 여기서 "nothing to commit"을 골라 FAIL.
    #[test]
    fn pick_prefers_stderr_when_present() {
        assert_eq!(
            pick_commit_error("nothing to commit", "hook failed"),
            "hook failed"
        );
    }

    // stderr가 비면 stdout으로 폴백한다("nothing to commit"은 stdout으로 온다).
    // mutation "항상 stderr(stdout 드롭)"는 generic으로 떨어져 FAIL.
    #[test]
    fn pick_falls_back_to_stdout_when_stderr_empty() {
        assert_eq!(
            pick_commit_error("nothing to commit", ""),
            "nothing to commit"
        );
    }

    // 둘 다 비면 generic fallback.
    #[test]
    fn pick_generic_when_both_empty() {
        assert_eq!(pick_commit_error("", ""), "Commit failed");
    }

    // --- M2 crux: classify (code가 load-bearing) ---

    // exit 0 → Committed. mutation "code 비교 뒤집기(non-zero→Committed)"는 아래 non-zero
    // 테스트와 empty-index 통합 테스트를 깬다.
    #[test]
    fn classify_zero_is_committed() {
        assert_eq!(classify_commit("out", "err", 0), CommitOutcome::Committed);
    }

    // non-zero → Failed(메시지는 채널 규칙). stderr 우선, 없으면 stdout.
    #[test]
    fn classify_nonzero_is_failed_with_channel_message() {
        assert_eq!(
            classify_commit("nothing to commit", "", 1),
            CommitOutcome::Failed {
                message: "nothing to commit".to_string()
            }
        );
        assert_eq!(
            classify_commit("", "hook failed", 1),
            CommitOutcome::Failed {
                message: "hook failed".to_string()
            }
        );
    }

    // --- M4 crux: bulk 파티션 술어(순수) ---

    // 정확히 같은 경로는 tracked.
    #[test]
    fn tracked_pathspec_exact_match() {
        assert!(is_tracked_pathspec("a.txt", &["a.txt".to_string()]));
    }

    // 디렉터리 input은 하위 tracked 파일이 있으면 tracked(ls-files가 하위 파일을 냄).
    #[test]
    fn tracked_pathspec_directory_prefix() {
        assert!(is_tracked_pathspec(
            "dir",
            &["dir/a.txt".to_string(), "dir/sub/b.txt".to_string()]
        ));
        // 후행 슬래시가 붙은 input도 정규화되어 매칭.
        assert!(is_tracked_pathspec("dir/", &["dir/a.txt".to_string()]));
    }

    // **`input/`(슬래시 경계)로만 접두 매칭한다.** `dir`이 `dirother/...`를 tracked로
    // 오판하면 안 된다 — startsWith(normalized+"/")를 startsWith(normalized)로 바꾸는
    // mutation을 이 단언이 죽인다.
    #[test]
    fn tracked_pathspec_prefix_requires_slash_boundary() {
        assert!(!is_tracked_pathspec("dir", &["dirother/a.txt".to_string()]));
    }

    // 어떤 tracked 경로와도 안 겹치면 untracked.
    #[test]
    fn tracked_pathspec_no_match_is_untracked() {
        assert!(!is_tracked_pathspec(
            "u.txt",
            &["a.txt".to_string(), "dir/b.txt".to_string()]
        ));
        // 빈 tracked 목록도 untracked.
        assert!(!is_tracked_pathspec("a.txt", &[]));
    }

    // --- M4 SECURITY: `validate_target` 게이트 단독 pin (git 백스톱 무관) ---
    //
    // 통합 테스트의 일부 escape(절대경로/`..`/null-byte)는 git·OS 백스톱에도 가려
    // suaegi 첫 게이트를 단독으로 pin하지 못한다(hollow-by-backstop 리스크). 여기서는
    // **git을 개입시키지 않고** tempdir+실제 fs만으로 `validate_target`을 직접 친다 —
    // `resolve_preserve_symlink`의 거부 arm을 `Valid`로 바꾸는 게이트-우회 mutation을
    // 이 유닛 테스트들이 git과 무관하게 죽인다.

    // 어휘 escape는 전부 Rejected(NothingToDiscard도 Valid도 아님). syscall 전에 거부되므로
    // worktree는 빈 tempdir이면 충분하다.
    #[test]
    fn validate_target_rejects_lexical_escapes() {
        let wt = tempfile::tempdir().unwrap();
        for bad in ["../outside", "a/../../b", "/etc/passwd", ".", "", "a\0b"] {
            assert!(
                matches!(validate_target(wt.path(), bad), Validation::Rejected(_)),
                "{bad:?}는 Rejected여야 한다(게이트-우회 mutation이면 Valid로 새어 FAIL)"
            );
        }
    }

    // 존재하지 않는 leaf → NothingToDiscard(멱등). Rejected 아님.
    #[test]
    fn validate_target_notfound_is_nothing_to_discard() {
        let wt = tempfile::tempdir().unwrap();
        assert!(matches!(
            validate_target(wt.path(), "does-not-exist"),
            Validation::NothingToDiscard
        ));
    }

    // worktree 안 실존 정상 파일 → Valid.
    #[test]
    fn validate_target_existing_file_is_valid() {
        let wt = tempfile::tempdir().unwrap();
        std::fs::write(wt.path().join("real.txt"), b"x").unwrap();
        assert!(matches!(
            validate_target(wt.path(), "real.txt"),
            Validation::Valid
        ));
    }

    // **핵심 pin(git 백스톱 없음):** worktree 안 `link -> 바깥` 심링크 부모를 지나는
    // `link/victim`은 `Validation::Rejected`여야 한다. git은 이걸 못 잡으므로(어휘적으로
    // repo 안) 오직 suaegi의 컴포넌트별 walk만이 방어한다 — 거부 arm을 `Valid`로 바꾸는
    // mutation을 이 단언이 단독으로 죽인다.
    #[cfg(unix)]
    #[test]
    fn validate_target_rejects_symlink_parent_escape() {
        use std::os::unix::fs::symlink;
        let wt = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("victim"), b"x").unwrap();
        symlink(outside.path(), wt.path().join("link")).unwrap();
        assert!(
            matches!(
                validate_target(wt.path(), "link/victim"),
                Validation::Rejected(_)
            ),
            "심링크 부모 escape는 Rejected여야 한다(Valid/NothingToDiscard면 게이트-우회 mutation)"
        );
    }
}
