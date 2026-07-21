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
    /// **`Stop`에서만 `None`을 "비지 않음"으로 취급한다**(보수적). 백그라운드
    /// 서브에이전트가 도는 중에 `Done`을 찍으면 배지가 done↔working으로 깜빡인다
    /// (실측 §1.6.6: 한 턴에 `Stop`이 두 번 오고, 첫 번째 뒤에 도구 호출이 8개 더 왔다).
    ///
    /// **`StopFailure`에는 이 필드가 구조적으로 없다**(실측 §1.6.2 — `session_crons`,
    /// `permission_mode`도 없는 훨씬 얇은 페이로드다). 가끔 빠지는 게 아니라 아예
    /// 없으므로 보수적 기본값을 적용하면 `StopFailure`가 **영원히 `Done`이 될 수 없고**,
    /// 무한 스피너를 막으려고 등록한 이벤트가 그 원인이 된다. 그래서 리듀서가
    /// [`HookEventName`]으로 분기한다 — 이 필드만 보고 판단하지 않는다.
    pub background_tasks_empty: Option<bool>,
    // **`prompt_is_task_notification`이 없는 것은 의도다.** 서브에이전트 완료가
    // 합성 `UserPromptSubmit`을 주입하지만(조사 §1.6.5에 접두사까지 실측돼 있다),
    // 이 플랜은 그걸 거르지 않는다 — 배지 결과가 같고(리드가 서브에이전트 결과를
    // 처리하는 중이니 working이 맞다) 프롬프트 텍스트를 표시하지도 않는다.
    // 파싱만 하고 안 쓰는 필드는 썩고, 있으면 "쓰라고 만든 것"으로 오해된다.
    // 툴팁이 생겨서 필요해지면 §1.6.5의 접두사로 bool 하나를 다시 넣으면 된다.
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
///
/// **측정된 API 재시도 창(~210초)보다 길게 잡는다.** 오류 시 Claude는 빨리 실패하지
/// 않고 백오프로 재시도하며(실측: 화면에 "attempt 7/10", 그동안 훅이 하나도 오지
/// 않는다), `StopFailure`가 t+210s에야 도착한다. 90초로 두면 **정상 재시도 중에**
/// 배지가 `Unknown`으로 튀는데 그때 에이전트는 실제로 일하는 중이다 — 긴 침묵의
/// 흔한 원인이 재시도라는 것이 이 값의 근거다.
pub const HOOK_STALE_AFTER: Duration = Duration::from_secs(240);

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
/// | `Agent(_)` | `Working`, [`HOOK_STALE_AFTER`] 이내 | `Working` |
/// | `Agent(_)` | `Working`, [`HOOK_STALE_AFTER`] 초과 | `Unknown` |
/// | `Agent(_)` | `Done`, 나이 무관 | `Done` |
/// | `Agent(_)` | 없음 | `Unknown` |
/// | `Unknown` | 있음 | 훅 그대로 (나이 규칙 동일) |
/// | `Unknown` | 없음 | `Unknown` — **`Done`을 합성하지 않는다** |
///
/// `Exited`가 최우선인 것이 **영구히 멈춘 `Working` 배지를 막는 유일한 규칙**이다:
/// 크래시한 에이전트는 `Stop`을 내지 않으므로 훅만으로는 영원히 `Working`이다.
///
/// **`Agent(_)`와 `Unknown`이 같은 팔인 것은 표를 그대로 옮긴 결과다** — 위 표의
/// 마지막 두 행이 `Agent(_)`의 대응 행과 글자 그대로 같다. presence가 "에이전트가
/// 있다"인지 "모르겠다"인지는 **훅이 있을 때는 결론을 바꾸지 않는다**: 어느 쪽이든
/// 훅이 가장 최근의 사실이다. 둘을 가르는 것은 폴링이 `NoAgent`나 `Exited`를
/// **확정했을 때**뿐이고, 그 둘은 위에서 이미 걸러진다.
pub fn reduce(input: &BadgeInput) -> BadgeState {
    match input.presence {
        // 최우선. 크래시한 에이전트는 `Stop`을 내지 않으므로 이 행이 없으면
        // 배지가 영원히 `Working`에 멈춘다.
        AgentPresence::Exited { .. } => BadgeState::Done,
        AgentPresence::NoAgent => {
            if input.no_agent_streak >= NO_AGENT_CONFIRMATIONS {
                BadgeState::Done
            } else {
                // 셸이 exec하는 동안 포그라운드를 잠깐 쥐는 전이다. 한 틱에
                // 반응하면 배지가 깜빡인다.
                input.previous
            }
        }
        AgentPresence::Agent(_) | AgentPresence::Unknown => match input.hook {
            // **`Done`을 합성하지 않는다.** 훅을 못 봤다는 것은 끝났다는 뜻이
            // 아니다 — 신뢰 대화상자 대기 중이면 `SessionStart`조차 오지 않는다.
            None => BadgeState::Unknown,
            // **나이를 보지 않는다.** 답 없는 권한 프롬프트는 몇 시간이고
            // 정당하게 `Waiting`이다.
            Some((HookState::Waiting, _)) => BadgeState::Waiting,
            Some((HookState::Done, _)) => BadgeState::Done,
            Some((HookState::Working, at)) => {
                // 오래돼서 의심스러운 것은 `Working`뿐이다. `saturating_`인
                // 이유는 시계가 뒤로 간 입력(테스트가 만드는)에서 패닉하지
                // 않기 위해서다.
                if input.now.saturating_duration_since(at) > HOOK_STALE_AFTER {
                    BadgeState::Unknown
                } else {
                    BadgeState::Working
                }
            }
        },
    }
}

/// 훅 이벤트 하나가 저장된 훅 상태에 하는 일. **세 결과를 구별하는 것이 요점이다** —
/// `Option<HookState>`로 두면 "무시한다"와 "지운다"가 같은 `None`으로 뭉개지고,
/// 그러면 `SessionStart`가 옛 세션의 `Working`을 그대로 물려받는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// 저장된 상태를 **그대로 둔다**. 유령 이벤트와 권위 없는 이벤트가 여기 온다.
    Ignore,
    /// 저장된 상태를 **지운다**(배지는 `Unknown`으로). 새 세션이 붙었고 아직
    /// 아무 일도 일어나지 않았다.
    Reset,
    Set(HookState),
}

/// 이벤트 → 훅 상태. **전부 실측이다**(조사 §1.4, §1.6) — 여기서 다시 추론하지 않는다.
///
/// | 이벤트 | 결과 | 근거 |
/// |---|---|---|
/// | `SessionStart` | `Reset` | 바인딩만. 아직 아무 일도 안 일어났다 |
/// | `UserPromptSubmit` | `Set(Working)` | `<task-notification>` 필터는 넣지 않는다(아래) |
/// | `PreToolUse`/`PostToolUse`/`PostToolUseFailure` | `Set(Working)` | |
/// | `PermissionRequest` | `Set(Waiting)` | `AskUserQuestion` 특수 처리 없음 — 자동 허용이 아니다 |
/// | `Stop` + background 빔 | `Set(Done)` | |
/// | `Stop` + background 안 빔 | `Set(Working)` | 서브에이전트가 도는 중이다 |
/// | `StopFailure` | `Set(Done)` **무조건** | 이 이벤트엔 `background_tasks`가 **구조적으로 없다** |
/// | `SubagentStop`/`SessionEnd` | `Ignore` | |
///
/// **`StopFailure`가 `background_tasks`를 보지 않는 이유**가 이 함수에서 가장
/// 틀리기 쉬운 곳이다. 페이로드에 그 필드가 **아예 없고**(`session_crons`,
/// `permission_mode`도 없다 — `Stop`보다 훨씬 얇다), [`HookEvent`]의 보수적 규칙은
/// `None`을 "비지 않음"으로 읽는다. 둘을 합치면 `StopFailure`는 **영원히 `Done`이
/// 될 수 없고** pane이 계속 돈다 — 이 이벤트를 등록한 이유가 정확히 그 무한
/// 스피너를 막는 것이었는데 그 자체가 원인이 된다. 필드가 가끔 빠지는 게 아니라
/// 구조적으로 없으므로 보수적 기본값이 **이 이벤트에는 틀린다.**
///
/// **`SessionEnd`를 무시하는 이유**: 종료 판정은 presence 폴링의 `Exited`/`NoAgent`가
/// 권위다. 훅은 async라 프로세스가 죽으면 아예 안 올 수도 있고, 폴링만이 그걸 본다.
///
/// **`SubagentStop`을 무시하는 이유**: 유령 이벤트가 온다 — `agent_type: ""`에 스폰을
/// 본 적 없는 `agent_id`. Task 호출이 전혀 없던 순수 Bash 실행에서도 하나 발화했고,
/// 독립적으로 두 번 관측됐다.
pub fn hook_outcome(event: &HookEvent) -> HookOutcome {
    match event.event {
        HookEventName::SessionStart => HookOutcome::Reset,
        HookEventName::UserPromptSubmit
        | HookEventName::PreToolUse
        | HookEventName::PostToolUse
        | HookEventName::PostToolUseFailure => HookOutcome::Set(HookState::Working),
        HookEventName::PermissionRequest => HookOutcome::Set(HookState::Waiting),
        HookEventName::Stop => match event.background_tasks_empty {
            Some(true) => HookOutcome::Set(HookState::Done),
            // **`None`은 "비지 않음"이다**(보수적). 없다고 done을 찍으면
            // 백그라운드 서브에이전트가 도는 중에 끝난 것으로 보인다.
            // `Working`으로 **덮어쓰는** 것이지 무시하는 것이 아니다 — 그래야
            // 나이가 갱신돼, 오래 도는 서브에이전트가 `Unknown`으로 새지 않는다.
            Some(false) | None => HookOutcome::Set(HookState::Working),
        },
        HookEventName::StopFailure => HookOutcome::Set(HookState::Done),
        HookEventName::SubagentStop | HookEventName::SessionEnd => HookOutcome::Ignore,
    }
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

    /// 처음부터 열려 있는 게이트. **부팅을 거치지 않는 경로**(테스트,
    /// `AppState::default()`)용이다 — 거기서는 하이드레이션할 것이 없고, 닫힌
    /// 게이트를 기본값으로 두면 그 경로의 저장이 영원히 막힌다.
    ///
    /// `Default`가 이걸 하지 **않는** 것이 의도다: 게이트의 기본 자세는 닫힘이고,
    /// 여는 것은 언제나 명시적이어야 한다.
    pub fn opened() -> Self {
        Self {
            pending_repos: HashSet::new(),
            sessions_resolved: true,
            layout_built: true,
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
    use suaegi_term::agent::AgentKind;

    // ---- 0.3 배지 리듀서: 결정표를 그대로 옮긴다 ----

    /// 훅 나이를 값으로 만든다. `now - age`가 아니라 `at + age`인 것은 `Instant`가
    /// 프로세스 시작 이전으로 내려가면 플랫폼에 따라 패닉하기 때문이다.
    fn at_age(hook: Option<HookState>, age: Duration) -> (Option<(HookState, Instant)>, Instant) {
        let at = Instant::now();
        (hook.map(|h| (h, at)), at + age)
    }

    fn input(
        presence: AgentPresence,
        hook: Option<HookState>,
        age: Duration,
        previous: BadgeState,
        no_agent_streak: u8,
    ) -> BadgeInput {
        let (hook, now) = at_age(hook, age);
        BadgeInput {
            presence,
            hook,
            previous,
            no_agent_streak,
            now,
        }
    }

    const FRESH: Duration = Duration::from_secs(1);
    /// [`HOOK_STALE_AFTER`]보다 확실히 크다. 상수를 바꿔도 따라간다.
    const STALE: Duration = Duration::from_secs(HOOK_STALE_AFTER.as_secs() * 2);

    /// 훅 상태 × 나이의 **전 조합**. `presence`마다 이 표를 그대로 돌린다.
    fn hook_table() -> Vec<(Option<HookState>, Duration, &'static str)> {
        vec![
            (None, FRESH, "no hook at all"),
            (Some(HookState::Working), FRESH, "working, fresh"),
            (Some(HookState::Working), STALE, "working, stale"),
            (Some(HookState::Waiting), FRESH, "waiting, fresh"),
            (Some(HookState::Waiting), STALE, "waiting, stale"),
            (Some(HookState::Done), FRESH, "done, fresh"),
            (Some(HookState::Done), STALE, "done, stale"),
        ]
    }

    /// **`Exited`가 최우선이다.** 이 행이 영구히 멈춘 `Working` 배지를 막는 유일한
    /// 규칙이다 — 크래시한 에이전트는 `Stop`을 내지 않으므로 훅만으로는 영원히
    /// `Working`이다. 훅 상태·나이·`previous`가 무엇이든 이겨야 한다.
    #[test]
    fn exited_beats_every_other_signal() {
        for code in [0, 1, 127, -1] {
            for (hook, age, label) in hook_table() {
                for previous in [
                    BadgeState::Working,
                    BadgeState::Waiting,
                    BadgeState::Done,
                    BadgeState::Unknown,
                ] {
                    // streak가 커도 `Exited`가 먼저 걸려야 한다.
                    for streak in [0, NO_AGENT_CONFIRMATIONS + 1] {
                        let got = reduce(&input(
                            AgentPresence::Exited { code },
                            hook,
                            age,
                            previous,
                            streak,
                        ));
                        assert_eq!(
                            got,
                            BadgeState::Done,
                            "Exited{{code:{code}}} must win over ({label}, previous={previous:?}, \
                             streak={streak}) — a crashed agent emits no Stop, so this row is the \
                             only thing preventing a permanently stuck badge"
                        );
                    }
                }
            }
        }
    }

    /// `Agent(_)`의 훅 표 전체. 값을 **직접 적는다** — 기대값을 함수로 계산하면
    /// 리듀서를 테스트 안에서 다시 구현하는 꼴이라 아무것도 검사하지 못한다.
    #[test]
    fn the_agent_present_rows_match_the_decision_table() {
        let expected = [
            (None, FRESH, BadgeState::Unknown),
            (Some(HookState::Working), FRESH, BadgeState::Working),
            (Some(HookState::Working), STALE, BadgeState::Unknown),
            (Some(HookState::Waiting), FRESH, BadgeState::Waiting),
            // 나이 무관.
            (Some(HookState::Waiting), STALE, BadgeState::Waiting),
            (Some(HookState::Done), FRESH, BadgeState::Done),
            (Some(HookState::Done), STALE, BadgeState::Done),
        ];
        for (hook, age, want) in expected {
            let got = reduce(&input(
                AgentPresence::Agent(AgentKind::Claude),
                hook,
                age,
                // `previous`는 이 행들에 영향을 주면 안 된다 — 일부러 답과 다른
                // 값을 넣어 새어 들어오면 드러나게 한다.
                BadgeState::Waiting,
                0,
            ));
            assert_eq!(got, want, "Agent(_) + {hook:?} @ {age:?} must be {want:?}");
        }
    }

    /// **`Unknown` presence는 `Agent(_)`와 같은 결론을 낸다.** 표의 마지막 두 행이
    /// 대응 행과 글자 그대로 같다 — 훅이 있으면 그것이 가장 최근의 사실이고,
    /// presence의 불확실성은 결론을 바꾸지 않는다. 두 팔이 갈라지면 여기서 잡힌다.
    #[test]
    fn unknown_presence_resolves_exactly_like_a_present_agent() {
        for (hook, age, label) in hook_table() {
            let with_agent = reduce(&input(
                AgentPresence::Agent(AgentKind::Claude),
                hook,
                age,
                BadgeState::Waiting,
                0,
            ));
            let with_unknown =
                reduce(&input(AgentPresence::Unknown, hook, age, BadgeState::Waiting, 0));
            assert_eq!(
                with_unknown, with_agent,
                "Unknown presence must resolve like Agent(_) for {label}"
            );
        }
        // 대조군: 훅이 없을 때 `Done`을 합성하지 않는다는 것을 직접 고정한다.
        // 위의 동치 단언만으로는 둘 다 틀린 값이어도 통과한다.
        assert_eq!(
            reduce(&input(AgentPresence::Unknown, None, FRESH, BadgeState::Done, 0)),
            BadgeState::Unknown,
            "with no hook and no presence we know nothing — synthesizing Done here would \
             mark a session finished that may be waiting on the trust dialog"
        );
    }

    /// **`Waiting`은 절대 감쇠하지 않는다.** 답 없는 권한 프롬프트는 몇 시간이고
    /// 정당하게 `Waiting`이다. 오래돼서 의심스러운 것은 `Working`뿐이다.
    #[test]
    fn waiting_never_decays_however_old_it_gets() {
        for age in [
            FRESH,
            HOOK_STALE_AFTER,
            HOOK_STALE_AFTER * 10,
            Duration::from_secs(60 * 60 * 8),
        ] {
            assert_eq!(
                reduce(&input(
                    AgentPresence::Agent(AgentKind::Claude),
                    Some(HookState::Waiting),
                    age,
                    BadgeState::Unknown,
                    0
                )),
                BadgeState::Waiting,
                "a permission prompt unanswered for {age:?} is still legitimately Waiting"
            );
            // 대조군: 같은 나이의 `Working`은 실제로 감쇠한다 — 위 단언이
            // "나이가 아무 데도 안 쓰인다"로 설명되면 안 된다.
            if age > HOOK_STALE_AFTER {
                assert_eq!(
                    reduce(&input(
                        AgentPresence::Agent(AgentKind::Claude),
                        Some(HookState::Working),
                        age,
                        BadgeState::Unknown,
                        0
                    )),
                    BadgeState::Unknown,
                    "control: Working at {age:?} DOES decay, so the age is genuinely consulted"
                );
            }
        }
    }

    /// 감쇠 경계는 **초과**에서 일어난다. 정확히 `HOOK_STALE_AFTER`는 아직 `Working`이다.
    ///
    /// 이 상수는 **측정된 API 재시도 창(~210초)보다 길어야 한다**: 오류 시 Claude는
    /// 백오프로 재시도하며 그동안 훅이 하나도 오지 않고 `StopFailure`가 t+210s에야
    /// 온다. 짧게 잡으면 **정상 재시도 중에** 배지가 `Unknown`으로 튄다.
    #[test]
    fn the_working_staleness_boundary_is_exclusive() {
        let just_inside = reduce(&input(
            AgentPresence::Agent(AgentKind::Claude),
            Some(HookState::Working),
            HOOK_STALE_AFTER,
            BadgeState::Unknown,
            0,
        ));
        assert_eq!(
            just_inside,
            BadgeState::Working,
            "exactly at the threshold is still Working — the rule is 'older than', not \
             'at least'"
        );
        let just_outside = reduce(&input(
            AgentPresence::Agent(AgentKind::Claude),
            Some(HookState::Working),
            HOOK_STALE_AFTER + Duration::from_millis(1),
            BadgeState::Unknown,
            0,
        ));
        assert_eq!(just_outside, BadgeState::Unknown, "one millisecond over decays");

        assert!(
            HOOK_STALE_AFTER > Duration::from_secs(210),
            "HOOK_STALE_AFTER must exceed the measured API retry window (StopFailure arrived \
             at t+210s with total hook silence before it) — otherwise the badge flips to \
             Unknown while the agent is legitimately retrying"
        );
    }

    /// `NoAgent` streak 경계. 셸이 exec하는 동안 포그라운드를 잠깐 쥐는 전이라
    /// 한 틱에 반응하면 배지가 깜빡인다.
    #[test]
    fn no_agent_holds_the_previous_badge_until_the_streak_is_confirmed() {
        assert_eq!(
            NO_AGENT_CONFIRMATIONS, 3,
            "the boundary cases below are written for 3 confirmations"
        );
        for previous in [
            BadgeState::Working,
            BadgeState::Waiting,
            BadgeState::Done,
            BadgeState::Unknown,
        ] {
            for streak in [0, 1, 2] {
                assert_eq!(
                    reduce(&input(AgentPresence::NoAgent, None, FRESH, previous, streak)),
                    previous,
                    "streak {streak} is below the threshold, so the badge must hold at \
                     {previous:?} rather than flicker"
                );
            }
            // 대조군: 경계에 닿으면 확정된다.
            for streak in [3, 4, u8::MAX] {
                assert_eq!(
                    reduce(&input(AgentPresence::NoAgent, None, FRESH, previous, streak)),
                    BadgeState::Done,
                    "control: streak {streak} has confirmed the agent is gone"
                );
            }
        }
    }

    /// `NoAgent`가 확정되기 전에는 **훅조차 무시하고** `previous`를 든다.
    /// 이 팔이 훅을 보기 시작하면 위 테스트가 `previous == 훅 결과`인 경우에만
    /// 통과하므로, 서로 다른 값으로 갈라 고정한다.
    #[test]
    fn an_unconfirmed_no_agent_ignores_the_hook_entirely() {
        let got = reduce(&input(
            AgentPresence::NoAgent,
            Some(HookState::Done),
            FRESH,
            BadgeState::Working,
            1,
        ));
        assert_eq!(
            got,
            BadgeState::Working,
            "below the streak threshold the previous badge wins even over a Done hook — \
             this arm must not consult the hook at all"
        );
    }

    // ---- 이벤트 → 훅 상태 매핑 (실측) ----

    fn event(name: HookEventName, background_tasks_empty: Option<bool>) -> HookEvent {
        HookEvent {
            pane_key: PaneKey(WorktreeId("/tmp/wt".into())),
            spawn_nonce: SpawnNonce(1),
            claude_session_id: "sid".into(),
            event: name,
            tool_name: None,
            agent_id: None,
            background_tasks_empty,
        }
    }

    #[test]
    fn the_event_mapping_matches_the_measured_table() {
        use HookEventName as E;
        let table = [
            (E::SessionStart, None, HookOutcome::Reset),
            (E::UserPromptSubmit, None, HookOutcome::Set(HookState::Working)),
            (E::PreToolUse, None, HookOutcome::Set(HookState::Working)),
            (E::PostToolUse, None, HookOutcome::Set(HookState::Working)),
            (
                E::PostToolUseFailure,
                None,
                HookOutcome::Set(HookState::Working),
            ),
            (
                E::PermissionRequest,
                None,
                HookOutcome::Set(HookState::Waiting),
            ),
            (E::SubagentStop, None, HookOutcome::Ignore),
            (E::SessionEnd, None, HookOutcome::Ignore),
        ];
        for (name, background, want) in table {
            assert_eq!(
                hook_outcome(&event(name, background)),
                want,
                "{name:?} must map to {want:?}"
            );
        }
    }

    /// **`Stop`은 "끝났다"가 아니다.** Agent 도구는 기본이 백그라운드 실행이라
    /// 서브에이전트가 도는 중에 `Stop`이 먼저 온다(실측: 한 턴에 `Stop`이 두 번).
    /// ①에서 done을 찍으면 배지가 done으로 갔다가 뒤따르는 도구 호출 8개에 다시
    /// working으로 돌아온다.
    #[test]
    fn stop_only_means_done_when_no_background_task_is_running() {
        assert_eq!(
            hook_outcome(&event(HookEventName::Stop, Some(true))),
            HookOutcome::Set(HookState::Done),
            "the final Stop carries an empty background_tasks and IS done"
        );
        assert_eq!(
            hook_outcome(&event(HookEventName::Stop, Some(false))),
            HookOutcome::Set(HookState::Working),
            "a Stop while a subagent is still running must stay working, or the badge \
             flickers done->working across the subagent's remaining tool calls"
        );
        assert_eq!(
            hook_outcome(&event(HookEventName::Stop, None)),
            HookOutcome::Set(HookState::Working),
            "absent background_tasks is treated as NOT empty (conservative) — the wrong \
             way round marks a running agent finished"
        );
    }

    /// **`StopFailure`는 `background_tasks`를 보지 않는다.** 페이로드에 그 필드가
    /// 구조적으로 없고, 보수적 규칙(`None` = 비지 않음)을 적용하면 이 이벤트는
    /// **영원히 done이 될 수 없다** — 등록한 이유가 그 무한 스피너를 막는 것인데
    /// 그 자체가 원인이 된다.
    #[test]
    fn stop_failure_is_done_unconditionally() {
        for background in [None, Some(false), Some(true)] {
            assert_eq!(
                hook_outcome(&event(HookEventName::StopFailure, background)),
                HookOutcome::Set(HookState::Done),
                "StopFailure must reach done regardless of background_tasks ({background:?}) — \
                 the field is structurally absent from this payload, so the conservative \
                 default is wrong here and would strand the pane on a spinner forever"
            );
        }
        // 대조군: 같은 `background` 값으로 `Stop`은 갈린다 — 위가 "무조건 done"이
        // 아니라 "이 이벤트만 무조건"임을 고정한다.
        assert_eq!(
            hook_outcome(&event(HookEventName::Stop, None)),
            HookOutcome::Set(HookState::Working),
            "control: Stop with the same absent field does NOT reach done"
        );
    }

    /// `SessionStart`는 `Ignore`가 아니라 `Reset`이다. 둘을 `None` 하나로 뭉개면
    /// 새 세션이 옛 세션의 `Working`을 그대로 물려받아, 아무 일도 안 하는 pane이
    /// 도는 스피너로 보인다.
    #[test]
    fn session_start_clears_the_badge_rather_than_leaving_it_alone() {
        assert_eq!(
            hook_outcome(&event(HookEventName::SessionStart, None)),
            HookOutcome::Reset
        );
        assert_ne!(
            hook_outcome(&event(HookEventName::SessionStart, None)),
            HookOutcome::Ignore,
            "Reset and Ignore must not be the same outcome — a fresh session inheriting the \
             previous session's Working badge is exactly the bug this distinction prevents"
        );
    }

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
