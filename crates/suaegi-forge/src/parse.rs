use crate::provider::{ChecksSummary, ReviewState};
use serde::Deserialize;

/// `gh pr create`의 출력에서 PR 번호+URL을 복구한다. **`gh pr create`는 `--json`이 없어**
/// [Codex B2] (어느 버전도 없다 — 영구 CLI 한계), 출력된 URL을 파싱하는 수밖에 없다.
/// 이것이 §3.1 "사람 텍스트 안 긁는다" 규칙에 대한 **의도된 예외**다.
///
/// Orca `parseCreatePRPayload`(`client.ts:1714`)를 미러: `https?://<host>/<owner>/<repo>/pull/<n>`.
/// 어떤 호스트든 매치해 GHES PR URL도 잡는다(#8312). 정규식 없이 손으로 스캔한다.
pub fn parse_created_pr(stdout: &str) -> Option<(u64, String)> {
    for line in stdout.lines() {
        let line = line.trim();
        // 각 라인에서 http(s):// 로 시작하는 토큰을 찾는다(gh는 URL을 한 줄로 출력).
        for token in line.split_whitespace() {
            if let Some(hit) = parse_pr_url(token) {
                return Some(hit);
            }
        }
    }
    None
}

/// 단일 토큰이 `https?://host/owner/repo/pull/<digits>`면 (번호, 정규화된 URL) 반환.
/// Orca 정규식 `https?:\/\/[^\s/]+\/[^\s/]+\/[^\s/]+\/pull\/(\d+)` 등가.
fn parse_pr_url(token: &str) -> Option<(u64, String)> {
    let rest = token
        .strip_prefix("https://")
        .or_else(|| token.strip_prefix("http://"))?;
    let scheme_len = token.len() - rest.len();

    // host / owner / repo / "pull" / <digits> — 정확히 이 세그먼트 순서.
    let mut segs = rest.split('/');
    let host = segs.next().filter(|s| !s.is_empty())?;
    let owner = segs.next().filter(|s| !s.is_empty())?;
    let repo = segs.next().filter(|s| !s.is_empty())?;
    if segs.next()? != "pull" {
        return None;
    }
    let num_seg = segs.next()?;
    // 번호 뒤에 더 붙어 있으면(예: /pull/12/files) 거기서 잘라 숫자만.
    let digits: String = num_seg.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let number: u64 = digits.parse().ok()?;
    if number == 0 {
        return None;
    }
    // 정규화된 URL(쿼리/후행 세그먼트 제거).
    let url = format!(
        "{}{}/{}/{}/pull/{}",
        &token[..scheme_len],
        host,
        owner,
        repo,
        number
    );
    Some((number, url))
}

/// `gh pr view --json owner,name,url`의 owner 형태: `{ "login": ... }`.
#[derive(Debug, Deserialize)]
pub struct GhOwner {
    pub login: String,
}

/// `gh repo view --json owner,name,url` 출력.
#[derive(Debug, Deserialize)]
pub struct GhRepoView {
    pub owner: GhOwner,
    pub name: String,
    pub url: String,
}

impl GhRepoView {
    /// url(`https://host/owner/repo`)에서 호스트를 뽑는다.
    pub fn host(&self) -> String {
        self.url
            .strip_prefix("https://")
            .or_else(|| self.url.strip_prefix("http://"))
            .and_then(|rest| rest.split('/').next())
            .filter(|s| !s.is_empty())
            .unwrap_or("github.com")
            .to_string()
    }
}

/// `gh pr view <sel> --json number,title,state,url,isDraft` 출력.
#[derive(Debug, Deserialize)]
pub struct GhPrView {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
    #[serde(rename = "isDraft", default)]
    pub is_draft: bool,
}

impl GhPrView {
    /// gh의 state(대문자 OPEN/CLOSED/MERGED) + isDraft를 `ReviewState`로.
    pub fn review_state(&self) -> ReviewState {
        match self.state.to_ascii_uppercase().as_str() {
            "MERGED" => ReviewState::Merged,
            "CLOSED" => ReviewState::Closed,
            "OPEN" if self.is_draft => ReviewState::Draft,
            _ => ReviewState::Open,
        }
    }
}

/// `gh pr checks <sel> --json bucket` 원소.
#[derive(Debug, Deserialize)]
pub struct GhCheck {
    #[serde(default)]
    pub bucket: String,
}

/// 체크 bucket 배열을 passing/failing/pending 카운트로 요약한다. gh의 bucket은
/// pass|fail|pending|skipping|cancel. skipping은 비-차단이라 세지 않는다.
pub fn summarize_checks(checks: &[GhCheck]) -> ChecksSummary {
    let mut s = ChecksSummary::default();
    for c in checks {
        match c.bucket.to_ascii_lowercase().as_str() {
            "pass" => s.passing += 1,
            "fail" | "cancel" => s.failing += 1,
            "pending" => s.pending += 1,
            // "skipping" 및 알 수 없는 값은 세지 않는다(보수적).
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_github_com_pr_url() {
        let out = "https://github.com/acme/widget/pull/123\n";
        assert_eq!(
            parse_created_pr(out),
            Some((123, "https://github.com/acme/widget/pull/123".to_string()))
        );
    }

    /// **PR 번호 파싱 회귀 방어**: 이 파싱이 깨지면 생성한 PR을 worktree에 연결할 수 없다.
    #[test]
    fn parses_a_ghes_host_and_ignores_surrounding_text() {
        // gh는 안내 텍스트 뒤에 URL을 낸다. GHES 호스트도 잡아야 한다(#8312).
        let out = "\nCreating pull request for feat into main in acme/widget\n\
                   https://ghe.corp.example/acme/widget/pull/7\n";
        assert_eq!(
            parse_created_pr(out),
            Some((7, "https://ghe.corp.example/acme/widget/pull/7".to_string()))
        );
    }

    #[test]
    fn trailing_path_segments_are_trimmed_to_the_number() {
        let out = "https://github.com/acme/widget/pull/42/files";
        assert_eq!(
            parse_created_pr(out),
            Some((42, "https://github.com/acme/widget/pull/42".to_string()))
        );
    }

    #[test]
    fn non_pr_output_yields_none() {
        assert_eq!(parse_created_pr("https://github.com/acme/widget/issues/9"), None);
        assert_eq!(parse_created_pr("some warning without a url"), None);
        assert_eq!(parse_created_pr("https://github.com/acme/widget/pull/0"), None);
    }

    #[test]
    fn repo_view_host_is_read_from_url() {
        let rv = GhRepoView {
            owner: GhOwner {
                login: "acme".into(),
            },
            name: "widget".into(),
            url: "https://ghe.corp.example/acme/widget".into(),
        };
        assert_eq!(rv.host(), "ghe.corp.example");
    }

    #[test]
    fn draft_and_state_mapping() {
        let mk = |state: &str, draft: bool| GhPrView {
            number: 1,
            title: "t".into(),
            state: state.into(),
            url: "u".into(),
            is_draft: draft,
        };
        assert_eq!(mk("OPEN", false).review_state(), ReviewState::Open);
        assert_eq!(mk("OPEN", true).review_state(), ReviewState::Draft);
        assert_eq!(mk("MERGED", false).review_state(), ReviewState::Merged);
        assert_eq!(mk("CLOSED", false).review_state(), ReviewState::Closed);
        // merged인데 isDraft=true여도 merged가 이긴다.
        assert_eq!(mk("MERGED", true).review_state(), ReviewState::Merged);
    }

    #[test]
    fn checks_summary_buckets() {
        let checks = vec![
            GhCheck { bucket: "pass".into() },
            GhCheck { bucket: "PASS".into() },
            GhCheck { bucket: "fail".into() },
            GhCheck { bucket: "cancel".into() },
            GhCheck { bucket: "pending".into() },
            GhCheck { bucket: "skipping".into() },
        ];
        let s = summarize_checks(&checks);
        assert_eq!(s.passing, 2);
        assert_eq!(s.failing, 2); // fail + cancel
        assert_eq!(s.pending, 1);
    }
}
