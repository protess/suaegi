//! 훅 → 앱 계약. **훅 서버는 `AppState`를 절대 만지지 않는다** — 소켓에서 읽은
//! 바이트를 [`HookEvent`]로 정규화해 유계 채널로 밀어 넣을 뿐이고, 상태를 바꾸는
//! 것은 앱의 `update`뿐이다. 이 경계가 정규화(`parse_hook`)와 합성([`reduce`])을
//! 둘 다 순수 함수로 만들고, 그것이 이 플랜의 테스트 가능성의 근거다.
//!
//! 여기 있는 결정들은 전부 조사 문서(`2026-07-21-plan5-hooks-diff-restore.md`)의
//! **실측**에 근거가 있다 — 훅 페이로드는 실제로 `claude`를 돌려 캡처한 것이다.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use suaegi_core::domain::{RepoId, WorktreeId};
use suaegi_term::presence::AgentPresence;

// `SessionState`가 담아야 해서 `suaegi-core`에 산다(의존 방향은 한 방향뿐이다).
// 훅 상관관계 키와 레이아웃 잎이 같은 `WorktreeId`라는 사실이 이 모듈의 전제이므로
// 여기서 재수출해 두 곳을 한 이름으로 읽게 한다.
pub use suaegi_core::domain::{PersistedAxis, PersistedPane};

// ---------------------------------------------------------------------------
// 0.1 pane 정체성
// ---------------------------------------------------------------------------

/// **`WorktreeId`에서 파생한다.** `SessionId`는 실행마다 매기는 카운터라 재시작을
/// 못 넘고, 배열 인덱스는 pane이 닫히면 어긋난다. worktree id는 경로에서 나오므로
/// 앱을 껐다 켜도 같다 — 훅 상관관계와 레이아웃 복원이 **같은 키**를 쓴다.
///
/// 한 worktree에서 세션이 재시작돼도 키가 같다. **이것이 의도다** — 배지는 pane의
/// 속성이지 세션 인스턴스의 속성이 아니다.
///
/// **날것으로 헤더에 실으면 안 된다.** unix 경로는 개행과 임의 바이트를 담을 수
/// 있어 그대로 넣으면 헤더 주입이다. 스폰 시 `SUAEGI_PANE_KEY`에는 base64url
/// (RFC 4648, 패딩 없음)로 **이미 인코딩된 값**을 심고, 서버가 엄격히 디코딩한다.
/// 인코더/디코더는 Task 2·3의 몫이다(이 모듈에는 없다).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaneKey(pub WorktreeId);

/// 세대 판별자. **우리가 발급한다.**
///
/// `PaneKey`만으로는 부족하다: 세션이 교체되면 옛 Claude 프로세스의 훅이 늦게
/// 도착해 새 세션의 배지를 덮을 수 있다(훅이 async라 더 그렇다). worktree를 지웠다
/// 같은 경로에 다시 만들어도 같다.
///
/// **Claude의 `session_id`로는 못 막는다.** "첫 이벤트의 id를 묶는다"는 규칙은
/// 그 첫 이벤트가 **옛 세션의 늦은 훅일 때** 옛 세션을 새 세대에 묶어버린다.
/// 스폰 시점에 이미 알고 있는 값이어야 "첫 이벤트를 믿는" 창이 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpawnNonce(pub u64);

impl SpawnNonce {
    /// 프로세스 전역 단조 증가. **재시작을 넘어 유일할 필요는 없다** — 비교
    /// 대상이 항상 "지금 이 프로세스가 방금 심은 값"이고, 프로세스가 죽으면
    /// 그 프로세스로 오던 훅도 갈 곳을 잃기 때문이다.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// 앱이 스폰 직전에 만들어 env(`SUAEGI_PANE_KEY`/`SUAEGI_SPAWN_NONCE`)로 심는 짝.
/// 훅 스크립트가 둘을 헤더로 되돌려 보내고, **앱은 `expected`와 다른 nonce의
/// 이벤트를 버린다.** 세션 교체 시 nonce를 새로 발급하고 배지를
/// [`BadgeState::Unknown`]으로 리셋한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneBinding {
    pub key: PaneKey,
    pub expected: SpawnNonce,
}

// ---------------------------------------------------------------------------
// 0.2 훅 인입 타입
// ---------------------------------------------------------------------------

/// 정규화된 훅 이벤트. 서버가 `parse_hook(pane, nonce, body)`로 만들고, 이 타입은
/// **`iced`도 소켓도 모른다** — 그래서 표 테스트가 가능하다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    pub pane_key: PaneKey,
    /// 세대 판별. **우리가 발급한 값**이다.
    pub spawn_nonce: SpawnNonce,
    /// 진단·툴팁용. **판별에 쓰지 않는다**(위 [`SpawnNonce`]의 이유).
    pub claude_session_id: String,
    pub event: HookEventName,
    pub tool_name: Option<String>,
    /// `Some` = 서브에이전트, `None` = 리드. 실측: 리드 이벤트는 `agent_id`를
    /// 아예 갖지 않고 서브에이전트만 `agent_id`/`agent_type`을 둘 다 갖는다.
    pub agent_id: Option<String>,
    /// **`None`은 "비지 않음"으로 취급한다**(보수적). `Stop`에는 실측으로 항상
    /// 존재하지만 `StopFailure` 페이로드는 미캡처다 — 없다고 `Done`을 찍으면
    /// 백그라운드 서브에이전트가 도는 중에 끝난 것으로 보인다. 반대 실수는
    /// 배지가 조금 늦게 도는 것뿐이다.
    pub background_tasks_empty: Option<bool>,
    /// 서브에이전트 완료가 주입하는 합성 프롬프트(`<task-notification>` XML)인가.
    /// 참이면 `UserPromptSubmit`을 **무시한다** — 아니면 사람이 치지도 않은
    /// 프롬프트로 배지가 working이 된다.
    pub prompt_is_task_notification: bool,
}

/// 등록하는 훅 이벤트. **`Notification`은 없다** — `PermissionRequest`보다 6초
/// 늦게 오므로 배지 신호로 쓸 값이 없다(실측).
///
/// **`StopFailure`가 필수다**: API/모델 오류 시 Claude가 정상 `Stop`을 건너뛴다.
/// 없으면 pane이 영원히 도는 스피너로 남는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEventName {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionRequest,
    Stop,
    StopFailure,
    /// **무시한다.** 유령 이벤트가 온다 — 본 적 없는 `agent_id`에 `agent_type: ""`.
    /// Task 호출이 전혀 없던 순수 Bash 실행에서도 하나 발화했다(실측).
    SubagentStop,
    /// **무시한다.** 종료 판정은 presence 폴링의 `Exited`/`NoAgent`가 권위다 —
    /// 훅은 async라 프로세스 사망 시 아예 안 올 수도 있고, 폴링만이 그걸 본다.
    SessionEnd,
}

// ---------------------------------------------------------------------------
// 0.3 배지 리듀서
// ---------------------------------------------------------------------------

/// 사용자에게 보이는 상태. **`Unknown`을 `Working`과 시각적으로 구별한다** —
/// 모른다 ≠ 바쁘다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BadgeState {
    Working,
    Waiting,
    Done,
    Unknown,
}

/// **훅이 만들 수 있는 상태는 셋뿐이다.** `Unknown`은 훅에서 오지 않고 리듀서가
/// 만든다 — [`BadgeState`]를 그대로 쓰면 결정표에 정의되지 않은 행
/// (`hook == Unknown`)이 생긴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookState {
    Working,
    Waiting,
    Done,
}

/// `Working` 훅 상태가 이 나이를 넘으면 [`BadgeState::Unknown`]으로 떨어진다.
/// **`Waiting`에는 적용하지 않는다** — 답 없는 `AskUserQuestion`은 몇 시간이고
/// 정당하게 `Waiting`이다. 오래돼서 의심스러운 것은 `Working`뿐이다.
pub const HOOK_STALE_AFTER: Duration = Duration::from_secs(90);

/// `NoAgent`를 몇 번 연속으로 봐야 `Done`으로 확정하는가. `presence.rs`가 셸이
/// exec하는 동안 포그라운드를 잠깐 쥐는 전이를 이미 문서화해뒀다 — 한 틱에
/// 반응하면 배지가 깜빡인다. 750ms 티어에서 ~2.25s.
pub const NO_AGENT_CONFIRMATIONS: u8 = 3;

/// [`reduce`]의 입력 전부. **구조체로 묶은 이유**는 이것이 결정표를 그대로 표
/// 테스트로 옮길 수 있는 유일한 형태이기 때문이다 — 인자 다섯을 늘어놓으면
/// 표의 행과 호출부가 눈으로 대응되지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeInput {
    pub presence: AgentPresence,
    /// 마지막으로 관측한 훅 상태와 그 시각. `None` = 훅을 하나도 못 봤다.
    pub hook: Option<(HookState, Instant)>,
    /// `NoAgent` streak가 [`NO_AGENT_CONFIRMATIONS`] 미만일 때 유지할 값.
    pub previous: BadgeState,
    /// `Agent(_)`나 `Exited`를 보면 0으로 리셋한다.
    pub no_agent_streak: u8,
    pub now: Instant,
}

/// 훅과 폴링을 하나의 배지로 합성한다. **전 조합 결정표**(0.3):
///
/// | presence | 훅 상태 | 결과 |
/// |---|---|---|
/// | `Exited{code}` | 무엇이든 | `Done` (코드≠0이면 오류 표시) — **최우선** |
/// | `NoAgent`, streak < 3 | 무엇이든 | `previous` 유지 (셸 exec 중 포그라운드 전이) |
/// | `NoAgent`, streak ≥ 3 | 무엇이든 | `Done` |
/// | `Agent(_)` | `Waiting` (나이 무관) | `Waiting` — **절대 감쇠시키지 않는다** |
/// | `Agent(_)` | `Working`, 90초 이내 | `Working` |
/// | `Agent(_)` | `Working`, 90초 초과 | `Unknown` |
/// | `Agent(_)` | `Done`, 나이 무관 | `Done` |
/// | `Agent(_)` | 없음 | `Unknown` |
/// | `Unknown` | 있음 | 훅 그대로 (나이 규칙 동일) |
/// | `Unknown` | 없음 | `Unknown` — **`Done`을 합성하지 않는다** |
///
/// `Exited`가 최우선인 것이 **영구히 멈춘 `Working` 배지를 막는 유일한 규칙**이다:
/// 크래시한 에이전트는 `Stop`을 내지 않으므로 훅만으로는 영원히 `Working`이다.
pub fn reduce(input: &BadgeInput) -> BadgeState {
    let _ = input;
    todo!("Task 3: 위 결정표를 그대로 구현한다 (표 테스트도 표를 그대로 옮긴다)")
}

// ---------------------------------------------------------------------------
// 0.4 서버 → 앱 전달
// ---------------------------------------------------------------------------

/// 훅 채널 용량. **정책은 drop-newest다, drop-oldest가 아니다** —
/// `futures::channel::mpsc`는 가득 차면 새 전송을 거절하거나 재우고, 보내는 쪽이
/// 가장 오래된 것을 꺼낼 수 없다. 실제 용량도 `buffer + 송신자 수`이지 정확히
/// 이 값이 아니다. 무계로 두면 훅 폭주에 앱이 OOM 난다.
///
/// **대가를 정직하게 적는다**: "다음 이벤트나 폴링이 곧 고친다"는 항상 참이
/// 아니다. 큐가 `Working`들로 차 있는 동안 마지막 `PermissionRequest`나 `Stop`이
/// 버려지면 — 폴링은 계속 `Agent`를 보고, 잃어버린 `Waiting`은 재구성할 수 없고,
/// 잃어버린 `Done`은 90초 뒤에야 `Unknown`이 되며, 후속 훅이 아예 없을 수도 있다.
/// → **버린 개수를 카운터로 노출한다**(Task 2). pane별 합치기는 다음 개선이다.
pub const HOOK_QUEUE: usize = 256;

// ---------------------------------------------------------------------------
// 0.7 하이드레이션 게이트
// ---------------------------------------------------------------------------

/// 부팅 진행의 종단 사건. **`ReposListed`는 성공·실패 양쪽에서 정확히 한 번**
/// 발행된다 — `Authoritative`든 `Degraded`든 그 repo를 `pending`에서 뺀다.
/// **낡은 응답은 빼지 않는다**: 요청 시 발급한 `OpId`로 대조해 현재 요청의 것만
/// [`Hydration::apply`]에 넘긴다(중복 응답이 카운터를 두 번 깎으면 게이트가
/// 일찍 열린다). 그 대조는 호출부의 책임이고 여기서는 하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HydrationStep {
    ReposListed(RepoId),
    SessionsResolved,
    LayoutBuilt,
}

/// 부팅이 끝날 때까지 `persist()`를 막는 게이트.
///
/// **왜 필요한가**: 부팅 중간 단계가 실패하면 부분 변경된 상태가 디스크에 덮인다.
/// Orca가 정확히 이걸로 사용자 탭을 날렸다(이슈 #1158). 우리에겐 지금 대응물이
/// 전혀 없다 — `boot()`이 끝나는 순간부터 `persist()`가 호출 가능하다.
///
/// **저하된 완료도 완료다**: repo 조회가 실패해도 그 repo는 `pending`에서 빠진다.
/// 게이트가 영원히 닫혀 있으면 사용자가 아무것도 저장할 수 없다.
///
/// 게이트가 닫힌 동안의 사용자 편집은 **메모리에 남고 게이트가 열릴 때 저장된다**
/// — 거부하지 않는다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hydration {
    pending_repos: HashSet<RepoId>,
    sessions_resolved: bool,
    layout_built: bool,
}

impl Hydration {
    /// 부팅 시 조회를 낸 repo 전부로 시작한다. **repo가 하나도 없으면**
    /// `pending_repos`가 처음부터 비어 나머지 둘만 기다린다.
    pub fn new(repos: impl IntoIterator<Item = RepoId>) -> Self {
        Self {
            pending_repos: repos.into_iter().collect(),
            sessions_resolved: false,
            layout_built: false,
        }
    }

    /// 같은 단계가 두 번 와도 안전하다(`HashSet::remove`와 `= true` 둘 다 멱등).
    /// 낡은 응답을 걸러내는 것은 호출부의 `OpId` 대조다.
    pub fn apply(&mut self, step: &HydrationStep) {
        match step {
            HydrationStep::ReposListed(repo) => {
                self.pending_repos.remove(repo);
            }
            HydrationStep::SessionsResolved => self.sessions_resolved = true,
            HydrationStep::LayoutBuilt => self.layout_built = true,
        }
    }

    /// 셋 다 끝나야 참. 열린 뒤로는 다시 닫히지 않는다.
    pub fn is_open(&self) -> bool {
        self.pending_repos.is_empty() && self.sessions_resolved && self.layout_built
    }
}

// ---------------------------------------------------------------------------
// Task 5 — 레이아웃 저장 디바운스
// ---------------------------------------------------------------------------

/// 리사이즈 메시지가 멎고 이만큼 지나면 저장한다.
///
/// **"드래그 종료 시"라는 이벤트는 iced에 없다.** `pane_grid::ResizeEvent`는
/// `split`과 `ratio`만 나르고 단계 표시가 없어서 `on_resize`가 드래그 **중에도**
/// 계속 발화한다. 있지도 않은 단계 이벤트를 찾지 말고 디바운스로 간다.
pub const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_nonce_strictly_increases() {
        let a = SpawnNonce::next();
        let b = SpawnNonce::next();
        assert!(
            b > a,
            "nonce가 증가하지 않으면 세대 판별이 무너진다: {a:?} -> {b:?}"
        );
    }

    /// **"셋 중 둘만"을 세 조합 전부** 확인한다. 순서대로 하나씩 쌓아 올리기만
    /// 하면 게이트가 실제로는 두 조건만 보고 있어도 통과한다 — 마지막에 적용한
    /// 단계가 어차피 마지막까지 거짓이기 때문이다. 빠뜨릴 단계를 바꿔가며 봐야
    /// 조건 하나를 잃은 구현이 죽는다.
    #[test]
    fn hydration_needs_all_three_steps() {
        let repo = RepoId("/tmp/demo".into());
        let all = [
            HydrationStep::ReposListed(repo.clone()),
            HydrationStep::SessionsResolved,
            HydrationStep::LayoutBuilt,
        ];

        for missing in 0..all.len() {
            let mut h = Hydration::new([repo.clone()]);
            for (i, step) in all.iter().enumerate() {
                if i != missing {
                    h.apply(step);
                }
            }
            assert!(
                !h.is_open(),
                "{:?}가 아직 안 왔는데 게이트가 열렸다",
                all[missing]
            );
            h.apply(&all[missing]);
            assert!(h.is_open(), "셋 다 끝났는데 게이트가 닫혀 있다");
        }
    }

    #[test]
    fn hydration_waits_for_every_repo() {
        let a = RepoId("/tmp/a".into());
        let b = RepoId("/tmp/b".into());
        let mut h = Hydration::new([a.clone(), b.clone()]);
        h.apply(&HydrationStep::SessionsResolved);
        h.apply(&HydrationStep::LayoutBuilt);

        // 같은 repo가 두 번 와도 다른 repo의 자리를 대신 깎으면 안 된다.
        h.apply(&HydrationStep::ReposListed(a.clone()));
        h.apply(&HydrationStep::ReposListed(a));
        assert!(!h.is_open(), "repo 하나가 아직 안 왔는데 게이트가 열렸다");

        h.apply(&HydrationStep::ReposListed(b));
        assert!(h.is_open());
    }
}
