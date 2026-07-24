//! push/pull/fetch의 **순수** 헬퍼(M1): credential 정제, 에러 정규화, push outcome 분류,
//! argv 빌더, upstream 파스. 전부 side-effect 없는 함수라 unit/mutation으로 못 박는다.
//!
//! Orca 포트 출처: `src/shared/git-remote-error.ts`(정제·정규화), `src/main/git/remote.ts`
//! (argv). 실제 git 호출과 전역 env-guard(F1/F2)는 [`crate::runner`]에 있다.
//!
//! M2는 여기에 `fetch`/`pull` **드라이버**(실제 git 호출)를 얹는다 — 로컬 bare remote로
//! AV 가능하다(라이브 auth만 사람눈). pull은 `--ff-only`(F4)라 divergent pull이
//! merge로 워크트리를 stuck시키는 대신 clean하게 abort한다.

use crate::runner::{GitError, GitRunner};
use std::path::Path;

/// 원격 작업 종류. 에러 정규화가 **push에만** 적용해야 하는 힌트(non-fast-forward)를
/// 게이트하는 데 쓴다 — fetch/pull에서 같은 문자열이 나와도 오발화하면 안 된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOp {
    Push,
    Pull,
    Fetch,
}

/// push 시도의 분류된 결과. git exit 0(성공/up-to-date)은 error 경로를 **절대** 거치지
/// 않으므로, 비-0 stderr만으로 거부/인증/네트워크를 가른다.
///
/// **`NonFastForwardRejected`는 절대 `Ok`가 아니다** — 성공으로 오분류하면 PR이 stale
/// commit을 가리키는 워크플로 파괴(대죄)다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// exit 0, 실제로 ref를 갱신함.
    Ok,
    /// exit 0, 원격이 이미 최신("Everything up-to-date").
    UpToDate,
    /// 원격에 우리가 없는 커밋이 있어 거부됨(non-fast-forward).
    NonFastForwardRejected,
    /// credential 미해결(auth 실패).
    AuthFailed,
    /// 네트워크 도달 불가.
    NetworkFailed,
    /// 그 밖의 실패.
    Other,
}

/// pull(`--ff-only`) 시도의 분류된 결과.
///
/// **`NotFastForward`는 clean 실패다** — suaegi엔 충돌해결 UI가 없어 divergent pull은
/// merge로 워크트리를 stuck시키는 대신 loud하게 abort한다(F4). 로컬 git이 fast-forward
/// 불가를 스스로 판정해 **아무것도 건드리지 않고** 멈추므로, 워크트리·HEAD는 미변경으로
/// 남는다(half-merge·MERGE_HEAD·conflict marker 없음). **절대 `Ok`가 아니다** — 성공으로
/// 오분류하면 divergent 상태를 "동기화됨"으로 착각하는 워크플로 파괴다.
///
/// push의 [`PushOutcome::NonFastForwardRejected`](PushOutcome)와 다르다: 저건 **원격이**
/// 우리 push를 거부하는 것이고, 이건 **로컬 git이** ff 불가로 pull을 abort하는 것이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullOutcome {
    /// exit 0, fast-forward로 로컬 ref가 원격까지 전진함.
    Ok,
    /// exit 0, 이미 최신("Already up to date").
    UpToDate,
    /// `--ff-only`가 fast-forward 불가로 거부(diverged). clean 실패, 워크트리 미변경.
    NotFastForward,
}

// ── credential 정제 ──────────────────────────────────────────────────────────
//
// Orca `git-remote-error.ts:12-13`의 두 scheme-scoped 정규식을 그대로 옮긴다(외부 dep
// 없이 손으로). git stderr는 흔히 원격 URL 전체를 박아 넣고 거기에 credential이 섞인다:
//
//   1) USERPASS: `([a-z][a-z0-9+.-]*://)[^\s/@:]+:[^\s/@]+@`  → `$1` (모든 scheme)
//   2) HTTPS_TOKEN: `(https?://)[^\s/@:]+@`                    → `$1` (http(s)만)
//
// `user:pass@`는 어느 scheme든 credential이지만, lone `user@`는 http(s)에서만 credential
// (토큰 PAT `https://ghp_xxx@host`). `ssh://git@host`의 `git`은 SSH 원격이 요구하는
// 로그인이라 지우면 URL이 깨져 **어느 원격이 실패했는지**가 사라진다 → 보존.
// 두 패스를 **순서대로** 돌리는 것이 self-heal의 핵심: 패스1이 `user:pass@`를 지운 뒤
// 남은 `residual@`을 패스2가 http(s)에서 마저 거둔다(literal-`@` 케이스, 아래 테스트).

/// userinfo(username, http-token) 종결 문자: 공백/`/`/`@`/`:`. (`[^\s/@:]`)
fn is_userinfo_delim(c: char) -> bool {
    c.is_whitespace() || c == '/' || c == '@' || c == ':'
}

/// password 종결 문자: 공백/`/`/`@`. **`:`는 포함 가능** — password는 콜론을 담을 수 있다.
/// (`[^\s/@]`)
fn is_password_delim(c: char) -> bool {
    c.is_whitespace() || c == '/' || c == '@'
}

/// 패스1: `scheme://user:pass@`를 `scheme://`로. i에서 매칭되면 (userinfo 시작 인덱스,
/// `@` **다음** 인덱스)를 돌려준다. 못 하면 None.
fn match_userpass_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let n = chars.len();
    let mut j = i;
    // scheme: [a-z] 그다음 [a-z0-9+.-]*  (대소문자 무시)
    if j >= n || !chars[j].is_ascii_alphabetic() {
        return None;
    }
    j += 1;
    while j < n && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '+' | '.' | '-')) {
        j += 1;
    }
    // 리터럴 `://`
    if !(j + 2 < n && chars[j] == ':' && chars[j + 1] == '/' && chars[j + 2] == '/') {
        return None;
    }
    j += 3;
    let userinfo_start = j; // 캡처 그룹 $1은 여기(`://` 직후)까지
                            // username: [^\s/@:]+ (한 개 이상)
    let user_start = j;
    while j < n && !is_userinfo_delim(chars[j]) {
        j += 1;
    }
    if j == user_start {
        return None;
    }
    // 리터럴 `:`
    if !(j < n && chars[j] == ':') {
        return None;
    }
    j += 1;
    // password: [^\s/@]+ (한 개 이상)
    let pass_start = j;
    while j < n && !is_password_delim(chars[j]) {
        j += 1;
    }
    if j == pass_start {
        return None;
    }
    // 리터럴 `@`
    if !(j < n && chars[j] == '@') {
        return None;
    }
    j += 1; // `@` 소비
    Some((userinfo_start, j))
}

/// 패스2: `https?://user@`를 `https?://`로(http(s) 전용, colon 없는 lone userinfo).
fn match_https_token_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let n = chars.len();
    let mut j = i;
    const LIT: [char; 4] = ['h', 't', 't', 'p'];
    for (k, expected) in LIT.iter().enumerate() {
        if i + k >= n || chars[i + k].to_ascii_lowercase() != *expected {
            return None;
        }
    }
    j += 4;
    // 선택적 `s`
    if j < n && chars[j].to_ascii_lowercase() == 's' {
        j += 1;
    }
    // 리터럴 `://`
    if !(j + 2 < n && chars[j] == ':' && chars[j + 1] == '/' && chars[j + 2] == '/') {
        return None;
    }
    j += 3;
    let userinfo_start = j;
    let start = j;
    while j < n && !is_userinfo_delim(chars[j]) {
        j += 1;
    }
    if j == start {
        return None;
    }
    if !(j < n && chars[j] == '@') {
        return None;
    }
    j += 1;
    Some((userinfo_start, j))
}

/// 한 패스를 문자열 전체에 global 적용한다(정규식 `/g`). 각 위치에서 `matcher`를 시도해
/// 매칭이면 캡처 그룹(scheme prefix)만 남기고 credential을 건너뛴다.
fn scrub_pass(input: &str, matcher: fn(&[char], usize) -> Option<(usize, usize)>) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < n {
        if let Some((keep_end, after)) = matcher(&chars, i) {
            // scheme prefix(`scheme://`)만 남기고 userinfo..`@`는 버린다.
            out.extend(&chars[i..keep_end]);
            i = after;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// git stderr에서 URL에 박힌 credential을 제거한다(Orca `stripCredentialsFromMessage`).
/// 두 패스 순차: (1) 모든 scheme의 `user:pass@`, (2) http(s)의 lone `user@`.
/// SSH `git@host`(scp-form)와 `ssh://git@host`는 **보존**된다(scheme-anchored).
pub fn strip_credentials_from_message(message: &str) -> String {
    let pass1 = scrub_pass(message, match_userpass_at);
    scrub_pass(&pass1, match_https_token_at)
}

// ── 에러 정규화 ──────────────────────────────────────────────────────────────

/// 마지막 non-empty 라인만 뽑는다(Orca `extractTailLine`). full blob은 로컬 경로/환경을
/// 흘릴 수 있어 UI에 올리지 않는다. 호출부가 이미 credential-scrub한 텍스트를 넘긴다.
fn extract_tail_line(message: &str) -> String {
    for line in message.lines().rev() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    message.to_string()
}

/// git 원격 에러 메시지를 사용자용으로 정규화한다(Orca `normalizeGitErrorMessage`).
///
/// **credential를 맨 앞에서 무조건 제거**한 뒤 분기한다 — 이후 어떤 분기(미래 리팩터로
/// `raw`의 부분문자열을 반환하더라도)도 이미 정제된 텍스트로 동작한다.
///
/// `non-fast-forward`/`fetch first` 힌트는 **push에만** 붙인다: fetch(원격 force-push 후
/// tracking ref 갱신)·pull(pull.ff=only)에서도 같은 문자열이 나므로, 사용자가 실제로
/// **push** 중일 때만 "pull or sync first"가 말이 된다.
pub fn normalize_git_error_message(message: &str, operation: RemoteOp) -> String {
    let raw = strip_credentials_from_message(message);

    if operation == RemoteOp::Push
        && (raw.contains("non-fast-forward") || raw.contains("fetch first"))
    {
        return "Push rejected: remote has newer commits (non-fast-forward). \
                Please pull or sync first."
            .to_string();
    }

    if raw.contains("could not read Username") || raw.contains("Authentication failed") {
        return "Authentication failed. Check your remote credentials.".to_string();
    }

    if raw.contains("Could not resolve host") || raw.contains("Network is unreachable") {
        return "Network error. Check your connection.".to_string();
    }

    if raw.contains("no tracking information") || raw.contains("no upstream") {
        return "Branch has no upstream. Publish the branch first.".to_string();
    }

    if raw.contains("Your local changes to the following files would be overwritten")
        || raw.contains("Your local changes would be overwritten")
    {
        return "Pull would overwrite local changes. \
                Commit, stash, or discard them before pulling."
            .to_string();
    }

    if raw.contains("untracked working tree files would be overwritten") {
        return "Pull would overwrite untracked files. \
                Move, remove, or add them before pulling."
            .to_string();
    }

    // Fallthrough: 마지막 stderr 라인만. `raw`는 위에서 이미 credential-scrub됨.
    extract_tail_line(&raw)
}

/// push 종료 코드+stderr를 [`PushOutcome`]으로 분류한다.
///
/// exit 0은 성공("Everything up-to-date"면 up-to-date). 비-0은 **non-fast-forward를 가장
/// 먼저** 판정한다 — `NonFastForwardRejected`는 어떤 경우에도 `Ok`가 되면 안 된다.
pub fn classify_push_outcome(exit_code: i32, stderr: &str) -> PushOutcome {
    // git exit 0 = 성공. push 성공 시 stderr에 "Everything up-to-date"를 낸다.
    if exit_code == 0 {
        if stderr.contains("Everything up-to-date") {
            return PushOutcome::UpToDate;
        }
        return PushOutcome::Ok;
    }
    // 비-0. non-fast-forward는 **절대 성공이 아니다**(대죄). 가장 먼저 판정.
    if stderr.contains("non-fast-forward")
        || stderr.contains("fetch first")
        || stderr.contains("[rejected]")
    {
        return PushOutcome::NonFastForwardRejected;
    }
    if stderr.contains("could not read Username") || stderr.contains("Authentication failed") {
        return PushOutcome::AuthFailed;
    }
    if stderr.contains("Could not resolve host") || stderr.contains("Network is unreachable") {
        return PushOutcome::NetworkFailed;
    }
    PushOutcome::Other
}

// ── argv 빌더 ────────────────────────────────────────────────────────────────

/// `git push [--set-upstream] origin HEAD:<branch>`. 단일 refspec, **`--force` 없음**
/// (force-with-lease는 M4). Orca `remote.ts:202-207`의 explicit-target push를 옮긴 것.
/// Orca는 항상 `--set-upstream`을 붙이지만 suaegi는 최초 publish 때만 붙이도록 파라미터화.
pub fn push_args(branch: &str, set_upstream: bool) -> Vec<String> {
    let mut args = vec!["push".to_string()];
    if set_upstream {
        args.push("--set-upstream".to_string());
    }
    args.push("origin".to_string());
    args.push(format!("HEAD:{branch}"));
    args
}

/// `git pull --ff-only`. **의도적 divergence(F4)**: Orca 기본 `git pull`은 merge지만,
/// suaegi엔 충돌해결 UI가 없어 merge-on-pull 충돌이 워크트리를 stuck시킨다. ff-only는
/// loud/clean 실패(stuck 상태 없음). Orca `gitFastForward`(`remote.ts:271`)의 argv와 동형.
pub fn pull_args() -> Vec<String> {
    vec!["pull".to_string(), "--ff-only".to_string()]
}

/// `git fetch origin`. Orca `gitFetch`(`remote.ts:310`)는 `--prune`을 붙이지만, M1은
/// remote-tracking ref를 지우지 않도록 prune을 생략한다(원한다면 후속에서 opt-in).
pub fn fetch_args() -> Vec<String> {
    vec!["fetch".to_string(), "origin".to_string()]
}

// ── fetch/pull 드라이버(M2, 실제 git 호출) ───────────────────────────────────

/// `git pull`이 낸 "이미 최신" 메시지 판정. 신형("Already up to date.")과 구형 git의
/// ("Already up-to-date.")를 모두 수용한다. git은 이 문구를 stdout에 낸다.
fn is_already_up_to_date(stdout: &str) -> bool {
    let lower = stdout.to_ascii_lowercase();
    lower.contains("already up to date") || lower.contains("already up-to-date")
}

/// `git pull --ff-only`이 **diverged로 fast-forward 불가** 거부됐는지 stderr로 판정한다(F4).
///
/// 이건 **clean 실패**다 — 로컬 git이 ff 불가를 스스로 판정하고 워크트리를 건드리지 않고
/// abort한다(half-merge·MERGE_HEAD·conflict marker 없음). unrelated-histories 거부
/// ("refusing to merge unrelated histories")나 no-remote/네트워크 실패는 여기 해당하지
/// 않는다 — 그건 진짜 에러로 표면화돼야 한다.
///
/// 문구-결합(version-coupled)이지만 **fail-safe**다: 미래 git이 이 문구를 바꾸면 diverged
/// pull이 `NotFastForward` 대신 `Err`로 떨어질 뿐이다 — git이 여전히 거부하므로 워크트리는
/// 미변경이고 false success도 없다. 최악이라도 분류가 덜 세분화될 뿐 데이터 안전은 유지된다.
pub fn is_ff_only_rejected(stderr: &str) -> bool {
    stderr.contains("Not possible to fast-forward")
}

/// `git fetch origin`을 실행한다([`fetch_args`]). 성공 = exit 0.
///
/// fetch는 remote-tracking ref(`origin/<branch>`)만 갱신하고 **워크트리·HEAD는 건드리지
/// 않는** 안전한 read op다. 실패(no remote/네트워크)는 **삼키지 않고** `Err`로 표면화한다
/// — [`normalize_git_error_message`]로 credential을 정제하고 fetch 문맥 메시지를 붙인다
/// (push 전용 non-ff 힌트는 붙지 않는다). transient 실패가 조용한 no-op가 되면 안 된다.
pub async fn fetch(runner: &GitRunner, worktree: &Path) -> Result<(), GitError> {
    let owned = fetch_args();
    let argv: Vec<&str> = owned.iter().map(String::as_str).collect();
    match runner.run(worktree, &argv).await {
        Ok(_) => Ok(()),
        Err(GitError::Failed { args, code, stderr }) => Err(GitError::Failed {
            args,
            code,
            stderr: normalize_git_error_message(&stderr, RemoteOp::Fetch),
        }),
        Err(other) => Err(other),
    }
}

/// `git pull --ff-only`을 실행하고([`pull_args`]) 결과를 [`PullOutcome`]으로 분류한다(F4).
///
/// - exit 0 + "Already up to date" → [`PullOutcome::UpToDate`].
/// - exit 0 그 외 → [`PullOutcome::Ok`](fast-forward로 HEAD 전진).
/// - `--ff-only`가 ff 불가로 abort → [`PullOutcome::NotFastForward`] (**clean 실패**,
///   워크트리·HEAD 미변경. 절대 `Ok`가 아니다).
/// - 그 밖의 실패(no remote/네트워크/overwrite 등) → `Err` — [`normalize_git_error_message`]로
///   credential 정제 + pull 문맥 메시지(push 전용 non-ff 힌트는 **안** 붙는다).
///
/// **transient≠false-negative**: 원격 도달 실패를 절대 "up to date"로 삼키지 않는다 —
/// 그건 `Err`다. divergent는 [`PullOutcome::NotFastForward`]로, 역시 `Ok`가 아니다.
///
/// `git pull`은 그 자체가 fetch + integrate라 별도 fetch 선행이 필요 없다(Orca `gitPull`도
/// 동일).
pub async fn pull(runner: &GitRunner, worktree: &Path) -> Result<PullOutcome, GitError> {
    let owned = pull_args();
    let argv: Vec<&str> = owned.iter().map(String::as_str).collect();
    match runner.run(worktree, &argv).await {
        Ok(out) => {
            if is_already_up_to_date(&out.stdout) {
                Ok(PullOutcome::UpToDate)
            } else {
                Ok(PullOutcome::Ok)
            }
        }
        // 비-0 exit. `--ff-only`가 diverged로 abort한 건 clean 실패(워크트리 미변경)라
        // `NotFastForward` **값**으로 돌린다 — 진짜 에러(`Err`)와 구분된다.
        Err(GitError::Failed { args, code, stderr }) => {
            if is_ff_only_rejected(&stderr) {
                Ok(PullOutcome::NotFastForward)
            } else {
                Err(GitError::Failed {
                    args,
                    code,
                    stderr: normalize_git_error_message(&stderr, RemoteOp::Pull),
                })
            }
        }
        Err(other) => Err(other),
    }
}

// ── upstream 파스 ────────────────────────────────────────────────────────────

/// `git rev-parse --abbrev-ref --symbolic-full-name @{upstream}`의 stdout을 파스한다.
/// upstream이 있으면 `Some("origin/main")`, 비어 있으면 `None`. **에러가 아니다** —
/// upstream 없음은 예상된 상태고, 호출부가 [`is_no_upstream_error`]로 stderr를 가른다.
pub fn parse_upstream(rev_parse_stdout: &str) -> Option<String> {
    let trimmed = rev_parse_stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// "upstream 없음"만 골라내는 stderr 판정(Orca `isNoUpstreamError`). `fatal:` 프리픽스
/// **와** 알려진 문구가 함께여야 참 — 그래야 corrupt-repo/unborn-HEAD/ambiguous-ref 같은
/// 진짜 실패를 "upstream 없음"으로 삼켜 숨기지 않는다.
pub fn is_no_upstream_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    // /(^|\n)fatal:/i — 줄 맨 앞의 fatal:.
    let has_fatal = lower.starts_with("fatal:") || lower.contains("\nfatal:");
    if !has_fatal {
        return false;
    }
    const PHRASES: &[&str] = &[
        "no upstream configured",
        "no tracking information",
        "head does not point",
        "needed a single revision",
        "ambiguous argument 'head@{u}'",
    ];
    PHRASES.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 정제(SECURITY crux) ─────────────────────────────────────────────────

    #[test]
    fn strips_https_token_credential() {
        // lone user@ (토큰 PAT)는 http(s)에서 credential → 제거.
        assert_eq!(
            strip_credentials_from_message("fatal: unable to access 'https://ghp_TOKEN@host/x'"),
            "fatal: unable to access 'https://host/x'"
        );
    }

    #[test]
    fn strips_userpass_credential() {
        assert_eq!(
            strip_credentials_from_message("remote: https://user:pass@host/repo.git failed"),
            "remote: https://host/repo.git failed"
        );
    }

    /// SSH scp-form `git@github.com:...`은 **보존**돼야 한다. 이게 깨지면 어느 원격이
    /// 실패했는지 사라진다. Mutation: lone user@를 모든 scheme에서 지우면 이 테스트 FAIL.
    #[test]
    fn preserves_ssh_scp_form() {
        let msg = "fatal: Could not read from remote git@github.com:foo/bar.git";
        assert_eq!(strip_credentials_from_message(msg), msg);
    }

    /// `ssh://git@host/...`도 보존(scheme이 http(s)가 아니고 userinfo에 colon 없음).
    #[test]
    fn preserves_ssh_url_form() {
        let msg = "fatal: unable to access 'ssh://git@host/x'";
        assert_eq!(strip_credentials_from_message(msg), msg);
    }

    /// literal `@`를 담은 credential `https://u:p@ss@host` → 완전 redaction. 두 패스
    /// 순서 self-heal 전용: 패스1이 `u:p@`를 지우고 남은 `ss@`를 패스2가 거둔다.
    /// **순서를 뒤집거나 한 패스를 지우면 credential 잔여가 새어 이 테스트 FAIL.**
    #[test]
    fn literal_at_in_credential_fully_redacted() {
        let out = strip_credentials_from_message("error: https://u:p@ss@host/repo");
        assert_eq!(out, "error: https://host/repo");
        assert!(!out.contains("ss@"), "credential 잔여가 샜다: {out}");
        assert!(!out.contains("u:p"), "userinfo 잔여가 샜다: {out}");
    }

    #[test]
    fn strips_credential_mid_message() {
        let out = strip_credentials_from_message(
            "line1\nremote fetch from https://tok@example.com/a.git rejected\nline3",
        );
        assert!(out.contains("https://example.com/a.git"));
        assert!(!out.contains("tok@"), "token이 샜다: {out}");
    }

    #[test]
    fn userpass_stripped_on_non_http_scheme() {
        // user:pass@는 **모든** scheme에서 제거(여기선 git+ssh://).
        assert_eq!(
            strip_credentials_from_message("git+ssh://user:secret@host/x"),
            "git+ssh://host/x"
        );
    }

    #[test]
    fn no_credential_passes_through_unchanged() {
        let msg = "fatal: repository not found";
        assert_eq!(strip_credentials_from_message(msg), msg);
    }

    // ── 정규화(비-ff는 push에만) ─────────────────────────────────────────────

    /// non-ff 힌트는 **push에만**. Mutation: gate를 지우면 pull/fetch에서도 오발화.
    #[test]
    fn non_ff_hint_only_on_push() {
        let stderr = "! [rejected] main -> main (non-fast-forward)\nfetch first";
        let push = normalize_git_error_message(stderr, RemoteOp::Push);
        assert!(
            push.contains("Push rejected"),
            "push엔 힌트가 붙어야: {push}"
        );

        // pull/fetch에는 붙지 않고 tail-line으로 떨어진다.
        let pull = normalize_git_error_message(stderr, RemoteOp::Pull);
        assert!(!pull.contains("Push rejected"), "pull에 오발화: {pull}");
        let fetch = normalize_git_error_message(stderr, RemoteOp::Fetch);
        assert!(!fetch.contains("Push rejected"), "fetch에 오발화: {fetch}");
    }

    /// 정규화는 credential를 맨 앞에서 무조건 제거한다. Mutation: strip 호출을 지우면
    /// tail-line에 token이 새어 이 테스트 FAIL.
    #[test]
    fn normalize_scrubs_credentials_first() {
        let out = normalize_git_error_message(
            "fatal: unable to access 'https://ghp_LEAK@host/x': the requested URL returned error: 403",
            RemoteOp::Push,
        );
        assert!(
            !out.contains("ghp_LEAK"),
            "credential이 정규화 출력에 샜다: {out}"
        );
    }

    #[test]
    fn normalize_auth_failure() {
        let out = normalize_git_error_message(
            "fatal: Authentication failed for 'https://host/x'",
            RemoteOp::Push,
        );
        assert_eq!(out, "Authentication failed. Check your remote credentials.");
    }

    #[test]
    fn normalize_network_failure() {
        let out = normalize_git_error_message(
            "fatal: Could not resolve host: github.com",
            RemoteOp::Fetch,
        );
        assert_eq!(out, "Network error. Check your connection.");
    }

    #[test]
    fn normalize_no_upstream() {
        let out = normalize_git_error_message(
            "fatal: no upstream configured for branch 'feature'",
            RemoteOp::Push,
        );
        assert_eq!(out, "Branch has no upstream. Publish the branch first.");
    }

    #[test]
    fn normalize_fallthrough_is_tail_line() {
        let out = normalize_git_error_message(
            "Command failed: git push\nfirst line\nfatal: the real reason",
            RemoteOp::Push,
        );
        assert_eq!(out, "fatal: the real reason");
    }

    // ── outcome 분류(비-ff ≠ 성공, WORKFLOW crux) ────────────────────────────

    /// **대죄 방지**: non-fast-forward-rejected는 절대 Ok가 아니다. Mutation: 이걸 Ok로
    /// 매핑하면 FAIL.
    #[test]
    fn non_ff_rejected_is_never_ok() {
        let stderr = "! [rejected]        main -> main (non-fast-forward)\n\
                      error: failed to push some refs to 'origin'\n\
                      hint: Updates were rejected because the tip of your current branch is behind";
        let outcome = classify_push_outcome(1, stderr);
        assert_eq!(outcome, PushOutcome::NonFastForwardRejected);
        assert_ne!(outcome, PushOutcome::Ok, "non-ff가 성공으로 오분류됐다");
        assert_ne!(outcome, PushOutcome::UpToDate);
    }

    #[test]
    fn fetch_first_is_non_ff() {
        let outcome = classify_push_outcome(1, "hint: Updates were rejected\nfetch first");
        assert_eq!(outcome, PushOutcome::NonFastForwardRejected);
    }

    #[test]
    fn exit_zero_is_ok() {
        assert_eq!(
            classify_push_outcome(0, "To github.com:foo/bar.git"),
            PushOutcome::Ok
        );
    }

    #[test]
    fn exit_zero_up_to_date() {
        assert_eq!(
            classify_push_outcome(0, "Everything up-to-date"),
            PushOutcome::UpToDate
        );
    }

    #[test]
    fn classify_auth_failed() {
        assert_eq!(
            classify_push_outcome(128, "fatal: Authentication failed for 'https://host/x'"),
            PushOutcome::AuthFailed
        );
    }

    #[test]
    fn classify_network_failed() {
        assert_eq!(
            classify_push_outcome(128, "fatal: Could not resolve host: github.com"),
            PushOutcome::NetworkFailed
        );
    }

    #[test]
    fn classify_other() {
        assert_eq!(
            classify_push_outcome(1, "error: something unexpected"),
            PushOutcome::Other
        );
    }

    // ── argv 빌더 ────────────────────────────────────────────────────────────

    /// push는 단일 refspec HEAD:<branch>, --force 없음. Mutation: --force를 넣으면 FAIL.
    #[test]
    fn push_args_no_upstream() {
        assert_eq!(
            push_args("feature/x", false),
            vec!["push", "origin", "HEAD:feature/x"]
        );
    }

    #[test]
    fn push_args_with_set_upstream() {
        assert_eq!(
            push_args("main", true),
            vec!["push", "--set-upstream", "origin", "HEAD:main"]
        );
    }

    #[test]
    fn push_args_never_forces() {
        let args = push_args("main", true);
        assert!(!args.iter().any(|a| a == "--force"), "force 금지(M4까지)");
        assert!(
            !args.iter().any(|a| a == "--force-with-lease"),
            "force-with-lease는 M4"
        );
    }

    /// pull은 --ff-only(F4). Mutation: --ff-only를 지우면 FAIL.
    #[test]
    fn pull_args_is_ff_only() {
        assert_eq!(pull_args(), vec!["pull", "--ff-only"]);
    }

    #[test]
    fn fetch_args_is_origin() {
        assert_eq!(fetch_args(), vec!["fetch", "origin"]);
    }

    // ── pull outcome 분류 헬퍼(F4) ───────────────────────────────────────────

    /// ff-only 거부 stderr → NotFastForward로 분류할 신호. Mutation: 판정을 false
    /// 상수로 바꾸면 divergent pull이 `Err`로 새어 이 단언 + AV 테스트가 FAIL.
    #[test]
    fn ff_only_rejection_detected() {
        assert!(is_ff_only_rejected(
            "fatal: Not possible to fast-forward, aborting."
        ));
    }

    /// unrelated-histories 거부는 ff-only 거부가 **아니다** — 진짜 에러로 표면화돼야 한다.
    /// 이걸 NotFastForward로 삼키면 실패 원인이 뭉개진다.
    #[test]
    fn unrelated_histories_is_not_ff_rejection() {
        assert!(!is_ff_only_rejected(
            "fatal: refusing to merge unrelated histories"
        ));
        assert!(!is_ff_only_rejected(
            "fatal: Could not resolve host: github.com"
        ));
    }

    #[test]
    fn already_up_to_date_both_spellings() {
        assert!(is_already_up_to_date("Already up to date."));
        assert!(is_already_up_to_date("Already up-to-date."));
        assert!(!is_already_up_to_date(
            "Updating a1b2c3..d4e5f6\nFast-forward"
        ));
    }

    // ── upstream 파스 ────────────────────────────────────────────────────────

    #[test]
    fn parse_upstream_some() {
        assert_eq!(
            parse_upstream("origin/main\n"),
            Some("origin/main".to_string())
        );
    }

    #[test]
    fn parse_upstream_none_when_blank() {
        assert_eq!(parse_upstream("   \n"), None);
        assert_eq!(parse_upstream(""), None);
    }

    #[test]
    fn no_upstream_error_detected() {
        assert!(is_no_upstream_error(
            "fatal: no upstream configured for branch 'x'"
        ));
        assert!(is_no_upstream_error(
            "fatal: ambiguous argument 'HEAD@{u}': unknown revision"
        ));
    }

    /// fatal: 프리픽스 없이 문구만 있으면 upstream-없음으로 삼키지 않는다 — 진짜 실패
    /// (corrupt/hook stdout 등)를 숨기지 않기 위함.
    #[test]
    fn no_upstream_requires_fatal_prefix() {
        assert!(!is_no_upstream_error(
            "hint: no upstream configured, just FYI"
        ));
        assert!(!is_no_upstream_error("fatal: not a git repository"));
    }
}
