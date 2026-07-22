//! 초기 프롬프트를 stdin-after-start 에이전트에 주입하기 위한 **준비 게이트**.
//!
//! argv/flag 에이전트는 스폰 시점에 프롬프트를 argv로 받으므로(`agent::spawn_for_def`)
//! 이 모듈이 필요 없다. 빈 TUI로 뜨는 `StdinAfterStart` 에이전트(19종)는 composer가
//! 준비된 뒤에 PTY로 써넣어야 한다 — 너무 일찍(pre-TUI 스플래시 중) 쓰면 프롬프트가
//! 스플래시에 먹히거나 엉뚱한 곳에 붙는다. **잘못 주입하는 것은 아예 안 하느니만
//! 못하다** — 그래서 게이트는 조건이 확실할 때만 한 번 쏘고, 아니면 조용히 포기한다
//! (사용자가 직접 타이핑하면 된다).
//!
//! Orca `draft-paste-ready-scanner.ts`(:60-93)의 미러다(상수는 Codex 교차검증):
//! 1. **BRACKETED_PASTE 모드가 켜질 때까지 기다린다.** 이 전제가 pre-TUI 스플래시
//!    중 발화를 막는 핵심이다(Codex S3) — 스플래시는 bracketed paste를 켜지 않는다.
//! 2. **그다음 조용한 창을 잰다** — 출력이 [`QUIET_WINDOW`] 동안 멎어야 composer가
//!    안정됐다고 본다. 출력이 다시 오면 창을 리셋한다.
//! 3. **하드 타임아웃**([`HARD_TIMEOUT`]) 안에 조건이 안 서면 포기한다.
//!
//! **시계를 주입 가능하게** 순수 상태기계로 뽑았다: [`PromptGate::poll`]이 관측
//! ([`GateObservation`], `now`/`bracketed_paste`/`generation`)만 받아 결정을 낸다 —
//! 실제 벽시계 sleep 없이 합성 `Instant`와 mode/generation 전이로 타이밍 로직을
//! mutation-검증한다.

use std::time::{Duration, Instant};

/// composer가 안정됐다고 보기까지 출력이 멎어 있어야 하는 시간(Orca 상수).
pub const QUIET_WINDOW: Duration = Duration::from_millis(1500);
/// 게이트가 조건을 포기하기까지의 총 상한(Orca 상수). 이 안에 BRACKETED_PASTE +
/// 조용한 창이 성립하지 않으면 **조용히** 포기한다.
pub const HARD_TIMEOUT: Duration = Duration::from_millis(8000);

/// 한 번의 [`PromptGate::poll`]에 넘기는 세션 관측. 이 셋이 게이트가 보는 세계의
/// 전부다 — 나머지는 상태기계 안에 있다.
#[derive(Debug, Clone, Copy)]
pub struct GateObservation {
    /// 지금 시각. 테스트는 여기에 합성 `Instant`를 넣는다.
    pub now: Instant,
    /// 지금 `BRACKETED_PASTE` 모드인가(composer 준비 전제).
    pub bracketed_paste: bool,
    /// 세션의 출력 generation 카운터. 값이 바뀌면 그새 출력이 더 들어온 것이다.
    pub generation: u64,
}

/// 한 번의 poll이 호출부에 시키는 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    /// 아직 조건이 안 섰다 — 다음 틱에 다시 본다.
    Wait,
    /// 지금 프롬프트를 **한 번** 써넣어라. poll이 이 값을 낸 뒤 게이트는
    /// `Finished`가 되어 다시는 `Inject`를 내지 않는다.
    Inject,
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    /// BRACKETED_PASTE가 켜지길 기다린다.
    WaitingForBracketedPaste,
    /// BRACKETED_PASTE는 봤다 — 이제 출력이 `QUIET_WINDOW` 동안 멎기를 기다린다.
    /// `since`는 마지막으로 출력을 본 시각, `last_generation`은 그때의 카운터.
    Quieting { since: Instant, last_generation: u64 },
    /// 주입했거나 포기했다. 이후 poll은 항상 [`GateAction::Wait`].
    Finished,
}

/// stdin-after-start 세션 하나의 주입 준비 상태기계.
#[derive(Debug)]
pub struct PromptGate {
    prompt: String,
    /// 게이트가 실제로 시계를 잰 첫 관측 시각. `start_session_for`가 게이트를
    /// 무장하는 시점과 세션이 실제로 살아나는(첫 poll) 시점 사이에 비동기 간극이
    /// 있으므로, 하드 타임아웃은 **무장 시각이 아니라 첫 poll 시각**부터 잰다 —
    /// 그래야 PTY 스폰 지연이 8초 예산을 갉아먹지 않는다.
    started: Option<Instant>,
    phase: Phase,
}

impl PromptGate {
    pub fn new(prompt: String) -> Self {
        Self {
            prompt,
            started: None,
            phase: Phase::WaitingForBracketedPaste,
        }
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// 관측 하나를 반영하고 결정을 낸다. 순수하다 — I/O도 벽시계도 만지지 않는다.
    pub fn poll(&mut self, obs: GateObservation) -> GateAction {
        // 시계는 **첫 관측**부터 잰다(위 `started` 문서 참고).
        let started = *self.started.get_or_insert(obs.now);

        if matches!(self.phase, Phase::Finished) {
            return GateAction::Wait;
        }

        // 하드 타임아웃: 어느 phase든 상관없이 조용히 포기한다. 첫 poll에서는
        // `started == now`라 0 < HARD_TIMEOUT이므로 걸리지 않는다.
        if obs.now.saturating_duration_since(started) >= HARD_TIMEOUT {
            self.phase = Phase::Finished;
            return GateAction::Wait;
        }

        match &mut self.phase {
            Phase::WaitingForBracketedPaste => {
                if obs.bracketed_paste {
                    // 전제가 섰다 — 조용한 창을 **지금부터** 새로 잰다.
                    self.phase = Phase::Quieting {
                        since: obs.now,
                        last_generation: obs.generation,
                    };
                }
                GateAction::Wait
            }
            Phase::Quieting {
                since,
                last_generation,
            } => {
                if obs.generation != *last_generation {
                    // 그새 출력이 더 들어왔다 — 조용한 창을 리셋한다.
                    *since = obs.now;
                    *last_generation = obs.generation;
                    return GateAction::Wait;
                }
                if obs.now.saturating_duration_since(*since) >= QUIET_WINDOW {
                    self.phase = Phase::Finished;
                    GateAction::Inject
                } else {
                    GateAction::Wait
                }
            }
            Phase::Finished => GateAction::Wait,
        }
    }

    #[cfg(test)]
    fn is_finished(&self) -> bool {
        matches!(self.phase, Phase::Finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(now: Instant, bracketed_paste: bool, generation: u64) -> GateObservation {
        GateObservation {
            now,
            bracketed_paste,
            generation,
        }
    }

    /// **핵심 안전장치**: BRACKETED_PASTE가 켜지기 전에는 절대 주입하지 않는다.
    /// 스플래시가 아무리 오래(단, 하드 타임아웃 안) 조용해도 마찬가지다 — 이
    /// 전제가 pre-TUI 스플래시 중 오발화를 막는다(Codex S3). 전제를 없애는
    /// mutation(예: `Quieting`으로 곧장 진입)이 이 테스트를 깬다.
    #[test]
    fn does_not_inject_during_a_pre_tui_splash_without_bracketed_paste() {
        let t0 = Instant::now();
        let mut gate = PromptGate::new("hello".into());
        // 스플래시: bracketed paste 꺼짐, 출력도 멎어 있음(generation 고정).
        // 조용한 창을 훌쩍 넘겨도(2배) 주입하면 안 된다.
        for ms in [0u64, 500, 1600, 3000] {
            let action = gate.poll(obs(t0 + Duration::from_millis(ms), false, 7));
            assert_eq!(
                action,
                GateAction::Wait,
                "no injection may happen before BRACKETED_PASTE at t={ms}ms"
            );
        }
        assert!(!gate.is_finished(), "still waiting, not given up (within timeout)");
    }

    /// BRACKETED_PASTE를 본 뒤 출력이 `QUIET_WINDOW` 동안 멎으면 **정확히 한 번**
    /// 주입하고 끝난다. 이후 poll은 다시 `Inject`를 내지 않는다.
    #[test]
    fn injects_once_after_bracketed_paste_and_a_quiet_window() {
        let t0 = Instant::now();
        let mut gate = PromptGate::new("do it".into());

        // t=0: 아직 스플래시 출력 중, bracketed paste 켜짐 → Quieting 진입.
        assert_eq!(gate.poll(obs(t0, true, 10)), GateAction::Wait);
        // t=1000ms: 아직 조용한 창(1500ms) 안 참 → 대기.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(1000), true, 10)),
            GateAction::Wait,
            "1000ms of quiet is not yet the 1500ms window"
        );
        // t=1600ms: generation 그대로(출력 없음) & 1500ms 경과 → 주입.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(1600), true, 10)),
            GateAction::Inject,
            "1500ms of no new output after bracketed paste must inject"
        );
        // 한 번만. 다음 poll은 Wait이고 게이트는 끝났다.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(1700), true, 10)),
            GateAction::Wait,
            "the gate must fire exactly once, never twice"
        );
        assert!(gate.is_finished());
    }

    /// 조용한 창은 **출력이 오면 리셋된다.** 스플래시 애니메이션이 bracketed
    /// paste를 켠 채 계속 그리는 동안에는 주입하지 않고, 진짜로 멎은 뒤에만 쏜다.
    #[test]
    fn new_output_resets_the_quiet_window() {
        let t0 = Instant::now();
        let mut gate = PromptGate::new("go".into());

        assert_eq!(gate.poll(obs(t0, true, 1)), GateAction::Wait);
        // 1400ms 시점에 출력이 하나 더(generation 1→2) — 창 리셋.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(1400), true, 2)),
            GateAction::Wait,
            "output at 1400ms restarts the clock"
        );
        // 리셋 지점(1400ms)에서 1500ms가 안 지났으면(2000ms 시점, 경과 600ms) 아직 대기.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(2000), true, 2)),
            GateAction::Wait,
            "only 600ms since the reset — not yet quiet"
        );
        // 리셋 지점에서 1500ms 경과(2900ms 시점) & 출력 없음 → 주입.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(2900), true, 2)),
            GateAction::Inject,
            "1500ms of quiet measured FROM the reset, not from t0"
        );
    }

    /// 하드 타임아웃 안에 조건이 안 서면 **조용히 포기한다**(주입 없음, 오류 없음).
    /// 여기서 `Inject`가 나오면 스플래시가 8초 넘게 이어질 때 엉뚱한 곳에 프롬프트가
    /// 박힌다 — mis-injection is worse than none.
    #[test]
    fn hard_timeout_gives_up_silently() {
        let t0 = Instant::now();
        let mut gate = PromptGate::new("hello".into());

        // 8초 내내 bracketed paste가 안 켜진다.
        assert_eq!(gate.poll(obs(t0, false, 0)), GateAction::Wait);
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(8000), false, 0)),
            GateAction::Wait,
            "at the hard timeout the gate gives up — it must NOT inject"
        );
        assert!(gate.is_finished(), "the gate is done, not still armed");
        // 이후 아무리 이상적인 관측이 와도 다시 살아나지 않는다.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(9000), true, 5)),
            GateAction::Wait,
            "a finished (gave-up) gate never revives"
        );
    }

    /// 하드 타임아웃은 `Quieting` 중에도 적용된다 — bracketed paste는 켜졌지만
    /// 출력이 8초 내내 끊이지 않고 조용한 창이 한 번도 안 서는 경우.
    #[test]
    fn hard_timeout_applies_even_while_quieting() {
        let t0 = Instant::now();
        let mut gate = PromptGate::new("hello".into());
        assert_eq!(gate.poll(obs(t0, true, 0)), GateAction::Wait);
        // 매번 generation이 바뀌어(출력 계속) 창이 계속 리셋되다 8초 도달.
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(4000), true, 50)),
            GateAction::Wait
        );
        assert_eq!(
            gate.poll(obs(t0 + Duration::from_millis(8000), true, 99)),
            GateAction::Wait,
            "endless output must not defeat the hard timeout"
        );
        assert!(gate.is_finished());
    }
}
