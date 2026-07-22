use crate::agent::match_agent;
use crate::session::TerminalSession;

/// PTY foreground 프로세스로 판정하는 **에이전트 존재 여부**.
/// 스펙의 working/waiting/done 상태는 Plan 5의 hook 서버가 이 값과 합성해 만든다 —
/// foreground 프로세스만으로는 "작업 중"과 "입력 대기"를 구분할 수 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPresence {
    /// foreground가 에이전트가 아님 (셸 프롬프트 등)
    NoAgent,
    /// foreground가 에이전트다. 값은 [`crate::agent::AgentDef::id`](레지스트리 키).
    /// 프로덕션은 `Agent(_)` 변형 여부만 보고 안쪽 id는 아직 소비하지 않는다(6b UI용).
    Agent(&'static str),
    Exited {
        code: i32,
    },
    /// 판정 불가 — 플랫폼 미지원이거나 프로세스 해석 실패
    Unknown,
}

/// pid → 명령줄 조회. 테스트에서 대체할 수 있도록 trait로 둔다.
pub trait ProcessProbe {
    fn command_line(&self, pid: i32) -> Option<String>;
}

/// `ps -p <pid> -o args=`. `comm`은 macOS에서 전체 경로, Linux에서 15자 절단이라
/// 플랫폼 차이가 커서 전체 명령줄(args)을 쓴다.
#[derive(Debug, Default, Clone, Copy)]
pub struct PsProbe;

impl ProcessProbe for PsProbe {
    #[cfg(unix)]
    fn command_line(&self, pid: i32) -> Option<String> {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "args="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!line.is_empty()).then_some(line)
    }

    #[cfg(not(unix))]
    fn command_line(&self, _pid: i32) -> Option<String> {
        None
    }
}

/// pgid는 폴링 주기 대부분 그대로이므로, 에이전트로 확정된 pgid는 재조회하지 않는다.
/// "에이전트 아님"은 캐시하지 않는다 — 같은 pgid에서 셸이 에이전트를 exec하는
/// 전환을 놓치지 않기 위함.
/// 같은 pgid가 몇 번 연속 조회될 때까지 캐시를 신뢰할지. pgid는 재사용되거나
/// 같은 그룹이 다른 프로그램을 exec할 수 있어 무한 신뢰는 위험하다.
const CACHE_REVALIDATE_AFTER: u32 = 20;

#[derive(Debug, Default)]
pub struct PresenceMonitor {
    cached_agent: Option<(i32, &'static str)>,
    cache_hits: u32,
}

impl PresenceMonitor {
    pub fn probe(&mut self, session: &TerminalSession, probe: &dyn ProcessProbe) -> AgentPresence {
        if let Some(code) = session.exit_code() {
            self.cached_agent = None;
            return AgentPresence::Exited { code };
        }
        if !session.is_running() {
            self.cached_agent = None;
            // session.rs의 리더 스레드는 exit_code를 저장한 *다음* running을
            // false로 저장한다(release 순서, session.rs:140-142). 그러므로
            // 우리가 방금 위에서 !is_running()을 관찰했다는 것은 exit_code
            // 저장도 이미 끝났다는 뜻이다 — 여기서 다시 읽으면 항상 최신
            // 코드를 얻는다. 위쪽의 첫 exit_code() 호출 결과(None)를 그대로
            // 재사용해 -1로 단정하면, 두 저장이 그 두 읽기 "사이"에 모두
            // 끝나버리는 좁은 창에서 실제 코드가 이미 발행됐는데도 -1을
            // 영구적으로 보고하게 된다 — 이 재읽기가 그 경쟁을 없앤다.
            return AgentPresence::Exited {
                code: session.exit_code().unwrap_or(-1),
            };
        }

        #[cfg(unix)]
        {
            let Some(pgid) = session.foreground_pgid() else {
                return AgentPresence::Unknown;
            };
            if let Some((cached_pgid, id)) = self.cached_agent {
                if cached_pgid == pgid && self.cache_hits < CACHE_REVALIDATE_AFTER {
                    self.cache_hits += 1;
                    return AgentPresence::Agent(id);
                }
            }
            // 프로세스를 해석하지 못하면 NoAgent로 단정하지 않는다 —
            // "모른다"와 "에이전트가 아니다"는 다른 상태다
            let Some(line) = probe.command_line(pgid) else {
                return AgentPresence::Unknown;
            };
            match match_agent(&line) {
                Some(def) => {
                    self.cached_agent = Some((pgid, def.id));
                    self.cache_hits = 0;
                    AgentPresence::Agent(def.id)
                }
                None => {
                    self.cached_agent = None;
                    self.cache_hits = 0;
                    AgentPresence::NoAgent
                }
            }
        }
        // Windows에는 controlling terminal의 foreground 프로세스 그룹 개념이 없다
        #[cfg(not(unix))]
        {
            let _ = probe;
            AgentPresence::Unknown
        }
    }
}
