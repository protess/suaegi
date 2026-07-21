use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

#[derive(Debug, thiserror::Error)]
pub enum TermError {
    #[error("pty: {0}")]
    Pty(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to spawn thread: {0}")]
    ThreadSpawn(String),
}

#[derive(Debug, Clone)]
pub struct PtySpawn {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

/// PTY 리더. 블로킹 API이므로 전용 스레드로 이동시켜 사용한다.
/// 세션에 보관하지 않는 이유: `Box<dyn Read + Send>`는 `Sync`가 아니라
/// 세션이 `Arc`로 공유될 수 없게 만든다.
pub struct PtyReader(Box<dyn Read + Send>);

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

/// PTY와 자식 프로세스의 핸들. 모든 내부 자원이 `Mutex` 뒤에 있어
/// `Send + Sync`이며 `Arc`로 스레드 간 공유할 수 있다.
pub struct PtySession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// `wait`/`try_wait` **전용** 락. `kill`은 이 락을 절대 잡지 않는다 —
    /// 리더 스레드가 블로킹 `wait()` 안에서 이 락을 쥐고 있는 동안 Drop이
    /// `kill()`에서 멈추면 "Drop은 블로킹하지 않는다"는 보장이 깨진다.
    child: Mutex<Box<dyn Child + Send + Sync>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// 스폰 시점에 고정한 프로세스 그룹 ID (= 직속 자식의 PID). 나중에 자식을
    /// 조회하지 않고 이 값을 쓴다 — 락 회피 + 값 불변.
    #[cfg(unix)]
    pgid: i32,
    /// 수확 상태. 시그널 전송과 수확을 **상호 배제**하기 위한 짧은 락으로,
    /// 블로킹 호출을 걸친 채로는 절대 잡지 않는다.
    lifecycle: Mutex<Lifecycle>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Lifecycle {
    /// 수확을 시작했다(블로킹 wait 진입 직전에 세움). 이 시점부터는 PID가
    /// 언제든 재사용될 수 있으므로 시그널을 보내지 않는다.
    reaping: bool,
    reaped: bool,
    /// 알려진 종료 코드. `wait()`든 `try_wait()`든 먼저 수확한 쪽이 채운다.
    /// `reaped`만으로는 "이미 수확됨"만 알 수 있을 뿐 코드 자체를 나중 호출자에게
    /// 전달할 수 없었다 — 그래서 `try_wait`가 한 번 `None`을 넘어가면 영원히
    /// `None`만 돌려주는 버그가 있었다(수확 여부와 "그 결과가 뭐였는지"를
    /// 구분하지 못함). 이 필드가 그 결과를 보관해 `try_wait`가 언제 불려도
    /// 알려진 코드를 정직하게 돌려주게 한다.
    exit_code: Option<i32>,
}

/// `PtySession::kill()`이 실제로 무엇을 했는지. `Ok(())` 하나로는 "시그널을
/// 보냈다"와 "수확이 이미 시작돼 아무것도 안 보냈다"를 구분할 수 없어, 자식이
/// 아직 멀쩡히 살아 있는데도(예: 다른 스레드가 `wait()`에 파킹된 채 자연
/// 종료를 기다리는 중) 호출자에게는 kill이 성공한 것처럼 보이는 문제가 있었다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    /// 시그널을 실제로 보냈다.
    Signalled,
    /// 수확이 이미 시작됐거나 끝나서 시그널을 보내지 않았다(PID/PGID 재사용
    /// 위험 때문 — `kill()` 문서 참고). **자식이 죽었다는 뜻이 아니다**: 이미
    /// 파킹된 `wait()`가 있다면 그 자연 종료를 계속 기다릴 뿐이다.
    SuppressedAfterReap,
}

/// `openpty` 재시도 횟수(최초 시도 포함).
///
/// 실측(아래 주석 참고)에서 실패한 호출은 **전부** 두 번째 시도에서 성공했고
/// 세 번째를 필요로 한 경우는 한 번도 없었다. 4는 그 위에 얹은 여유분이다.
const OPENPTY_ATTEMPTS: u32 = 4;

/// 재시도 사이 대기. 첫 재시도는 **즉시**(0ms) — 실측상 그것으로 충분하다.
/// 이후 시도만 조금씩 물러선다(부하가 훨씬 심한 상황을 위한 보험).
fn openpty_backoff(attempt: u32) -> Duration {
    match attempt {
        1 => Duration::ZERO,
        n => Duration::from_millis(u64::from(n) * 2),
    }
}

/// `openpty`를 유한 횟수 재시도한다.
///
/// **왜 필요한가**: macOS(Darwin)의 `openpty(3)`는 동시 호출에 안전하지 않다.
/// 여러 스레드 **또는 여러 프로세스**가 동시에 부르면 간헐적으로 실패하며,
/// 그때 `errno`조차 유효한 값이 아니다(관측값 `-6` — 실패 경로가 errno를 제대로
/// 세우지 않는다는 뜻). 프로젝트 코드가 전혀 없는 순수 C 프로그램으로 재현했다:
/// 스레드 14개 × 400회 배리어 동기 호출에서 5600회 중 55회가 첫 시도에 실패했고,
/// **55회 전부 두 번째 시도에서 성공**했다(3번째 시도가 필요한 경우 0회).
/// 단일 스레드 프로세스 14개를 동시에 돌려도 실패가 나오므로 이 경쟁은
/// **프로세스를 넘나든다** — 프로세스 내부 뮤텍스로는 막을 수 없고, 재시도가
/// 유일하게 통하는 수단이다.
///
/// 이건 타임아웃을 늘려 문제를 덮는 것이 아니라, **일시적이고 재시도 가능한
/// OS 오류**를 재시도하는 것이다(위 실측이 일시성을 보여준다). 실패가 진짜로
/// 지속되면(예: ptmx 고갈) 몇 밀리초 안에 시도를 소진하고 마지막 오류를 그대로
/// 올려보낸다 — 삼키지 않는다.
///
/// `openpty`는 실패 시 fd를 남기지 않으므로 재시도에 부작용이 없다. errno를
/// 신뢰할 수 없어 오류 종류로 구분하지 않고 일괄 재시도한다.
fn open_pty_retrying<T, E>(
    mut attempt: impl FnMut() -> Result<T, E>,
    mut sleep: impl FnMut(Duration),
) -> Result<T, E> {
    let mut last = match attempt() {
        Ok(pair) => return Ok(pair),
        Err(e) => e,
    };
    for n in 1..OPENPTY_ATTEMPTS {
        let backoff = openpty_backoff(n);
        if !backoff.is_zero() {
            sleep(backoff);
        }
        match attempt() {
            Ok(pair) => return Ok(pair),
            Err(e) => last = e,
        }
    }
    Err(last)
}

impl PtySession {
    // 락 순서 규칙 (wait/try_wait/kill 전체에 적용): `lifecycle`을 쥔 채로
    // `child` 락을 **기다리지** 않는다. (`try_wait`는 둘을 잠깐 동시에 잡지만,
    // `reaping`을 먼저 확인하므로 그 시점엔 `child`를 붙들 수 있는 스레드가
    // 없어 대기가 발생하지 않는다.) 대기가 겹치지 않으면 ABBA 역전이 성립하지
    // 않는다. `wait()`는
    // `child.wait()`처럼 블로킹 호출을 거는 동안 `lifecycle`을 들고 있지
    // 않아야 하고(진입 전에 `reaping`만 세우고 놓는다), `child` 락을 요구하는
    // 코드는 `lifecycle`을 쥔 채로 그 락을 기다려서는 안 된다 — `try_wait`가
    // `reaping`을 먼저 확인해 이를 지킨다(수확이 진행 중이면 `child`에
    // 손대지 않고 바로 반환). 이 규칙을 깨는 변경은 `wait`/`try_wait`/`kill`
    // 사이에 데드락을 재도입한다.

    pub fn spawn(spec: PtySpawn) -> Result<(Self, PtyReader), TermError> {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows: spec.rows.max(1),
            cols: spec.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        // Darwin의 openpty는 동시 호출에서 간헐적으로 실패한다 —
        // `open_pty_retrying` 문서 참고.
        let pair = open_pty_retrying(|| pty_system.openpty(size), std::thread::sleep)
            .map_err(|e| TermError::Pty(e.to_string()))?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        // 기본값 먼저, 호출자 env가 나중에 와서 덮어쓴다
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Suaegi");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| TermError::Pty(e.to_string()))?;

        // 스폰 직후 실패 경로의 정리. killer(SIGHUP)만으로는 이를 무시하는 자식이
        // 남아 아래 wait()가 영원히 멈출 수 있으므로 unix에서는 그룹 SIGKILL을 쓴다.
        fn abort_child(child: &mut Box<dyn Child + Send + Sync>) {
            #[cfg(unix)]
            {
                if let Some(pid) = child.process_id() {
                    unsafe {
                        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
            }
            let _ = child.clone_killer().kill();
            let _ = child.wait();
        }

        // 여기서 실패하면 자식이 이미 떠 있다 — 죽이고 reap한 뒤 에러를 낸다
        let reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                abort_child(&mut child);
                return Err(TermError::Pty(e.to_string()));
            }
        };
        // take_writer는 한 번만 유효하므로 여기서 소유한다
        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                abort_child(&mut child);
                return Err(TermError::Pty(e.to_string()));
            }
        };
        let killer = child.clone_killer();
        // 그룹 ID를 지금 고정한다 — portable-pty는 자식을 자체 세션/그룹으로 띄우므로
        // pgid == 직속 자식의 PID다
        #[cfg(unix)]
        let pgid = child.process_id().unwrap_or(0) as i32;

        // slave를 붙들고 있으면 자식이 죽어도 리더가 EOF를 보지 못한다
        drop(pair.slave);

        Ok((
            Self {
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                child: Mutex::new(child),
                killer: Mutex::new(killer),
                #[cfg(unix)]
                pgid,
                lifecycle: Mutex::new(Lifecycle::default()),
            },
            PtyReader(reader),
        ))
    }

    /// 블로킹. 전용 writer 스레드에서만 호출한다.
    pub fn write(&self, bytes: &[u8]) -> Result<(), TermError> {
        let mut writer = self.writer.lock().expect("pty writer mutex poisoned");
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), TermError> {
        let master = self.master.lock().expect("pty master mutex poisoned");
        master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TermError::Pty(e.to_string()))
    }

    /// 현재 PTY 크기 (rows, cols). 주로 테스트에서 PTY와 grid 크기가
    /// 일치하는지 독립적으로 확인하는 용도.
    #[doc(hidden)]
    pub fn size(&self) -> Result<(u16, u16), TermError> {
        let master = self.master.lock().expect("pty master mutex poisoned");
        let size = master
            .get_size()
            .map_err(|e| TermError::Pty(e.to_string()))?;
        Ok((size.rows, size.cols))
    }

    /// 비블로킹. lifecycle 락을 호출 **전 구간** 잡는다 — try_wait 자체가
    /// 블로킹하지 않으므로 안전하고, 이렇게 해야 "수확됨"과 kill 사이에 틈이 없다.
    ///
    /// **계약**: 종료 코드가 알려지면(이 호출이 직접 수확했든, `wait()`가 다른
    /// 스레드에서 먼저 수확했든) 이후 몇 번을 불러도 항상 `Ok(Some(code))`를
    /// 돌려준다 — fire-once가 아니라 멱등이다. `Ok(None)`은 오직 "아직 종료
    /// 안 했다"만을 뜻한다("이미 어딘가에서 수확됐는데 코드를 모른다"는 상태는
    /// 없다).
    ///
    /// `reaping`은 서 있는데 코드가 아직 없는 경우(다른 스레드의 `wait()`가
    /// `reaping`만 세우고 아직 블로킹 `child.wait()`를 끝내지 못한 극히 짧은
    /// 창)에는 `child` 락에 **절대 손대지 않고** `Ok(None)`을 반환한다. `wait()`는
    /// 그 락을 쥔 채로 블로킹하므로, 여기서 그걸 기다리면 `lifecycle`을 쥔 채
    /// 파킹하게 되어 `kill()`의 "즉시 반환" 보장이 깨진다(락 순서 규칙 위반).
    /// 그 창을 지나면 `wait()`가 `exit_code`를 채우므로 다음 호출부터는 코드가
    /// 보인다.
    pub fn try_wait(&self) -> Result<Option<i32>, TermError> {
        let mut lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
        if let Some(code) = lifecycle.exit_code {
            return Ok(Some(code));
        }
        if lifecycle.reaping {
            return Ok(None);
        }
        let mut child = self.child.lock().expect("pty child mutex poisoned");
        let status = child.try_wait()?;
        if let Some(status) = status {
            let code = status.exit_code() as i32;
            lifecycle.reaping = true;
            lifecycle.reaped = true;
            lifecycle.exit_code = Some(code);
            return Ok(Some(code));
        }
        Ok(None)
    }

    /// 블로킹. 리더 스레드가 EOF를 본 뒤 호출한다.
    /// 블로킹 구간에 lifecycle 락을 걸치지 않는 대신, 진입 **전에** `reaping`을
    /// 세워 그 순간부터 kill이 시그널을 보내지 않게 한다 (PID 재사용 방지).
    ///
    /// 이미 알려진 코드가 있으면(다른 스레드가 먼저 수확했으면) `child` 락을
    /// 다시 잡지 않고 바로 그 값을 돌려준다 — portable-pty의 unix `Child`가
    /// `std::process::Child`라 재호출이 원래도 안전하긴 하지만(캐시된 상태를
    /// 돌려줌), 굳이 `child` 락을 다시 다툴 이유가 없다.
    ///
    /// `child` 락은 블로킹 `child.wait()` 호출을 감싸는 스코프 안에서만 잡고
    /// 반환 즉시 놓는다 — 꼬리의 `reaped`/`exit_code` 기록은 그 스코프 **밖**에서,
    /// `child`를 놓은 뒤에 `lifecycle`을 다시 잡아 수행한다. 두 락을 동시에 쥐지
    /// 않아야 `try_wait`/`kill`과의 ABBA 역전을 피한다(락 순서 규칙, 위 참고).
    pub fn wait(&self) -> Result<i32, TermError> {
        {
            let mut lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
            if let Some(code) = lifecycle.exit_code {
                return Ok(code);
            }
            lifecycle.reaping = true;
        }
        let status = {
            let mut child = self.child.lock().expect("pty child mutex poisoned");
            child.wait()?
        };
        let code = status.exit_code() as i32;
        {
            let mut lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
            lifecycle.reaped = true;
            lifecycle.exit_code = Some(code);
        }
        Ok(code)
    }

    /// unix: 자식의 프로세스 그룹 전체에 SIGKILL. portable-pty의 killer는 직속
    /// 자식에게 SIGHUP만 보내므로, 이를 무시하는 프로세스가 PTY를 붙들면 리더가
    /// EOF를 보지 못하고 상위의 join이 영원히 멈춘다. SIGKILL은 무시할 수 없다.
    ///
    /// **child 락을 잡지 않는다** — 리더 스레드가 블로킹 `wait()` 안에서 그 락을
    /// 쥐고 있어도 이 함수는 즉시 반환해야 Drop이 멈추지 않는다.
    ///
    /// 수확이 시작된 뒤에는 **아무 시그널도 보내지 않는다**: PID/PGID가 재사용돼
    /// 무관한 프로세스를 죽일 수 있기 때문이다. 시그널 전송은 lifecycle 락 안에서
    /// 이루어지므로 "수확 시작"과 "시그널 전송" 사이에 틈이 없다.
    ///
    /// 대신 수확 **전에** 반드시 한 번 kill이 불리도록 호출 순서를 설계했다
    /// (리더 스레드는 EOF/에러 어느 경로든 `wait()` 직전에 `kill()`을 부른다).
    /// 그래도 PTY 디스크립터를 닫고 살아남은 자손은 놓칠 수 있다 — 재사용된 PID를
    /// 죽일 위험보다 낫다고 판단한 트레이드오프다.
    ///
    /// 반환값은 `Ok`이 곧 "자식이 죽는다"는 뜻이 아님을 드러낸다:
    /// `TerminalSession`(리더가 EOF 뒤에만 `wait()`를 부름) 안에서는 억제가
    /// 항상 안전하지만, 이 raw API를 직접 쓰면서 어떤 스레드가 **살아 있는**
    /// 자식에 대해 이미 `wait()`에 파킹돼 있는 상태로 `kill()`을 부르면,
    /// `Ok(SuppressedAfterReap)`가 돌아오는데도 자식은 그 `wait()`가 자연
    /// 종료를 기다리는 동안 계속 살아 있다 — 호출자는 이 값을 보고 "시그널이
    /// 안 갔다, 자식이 곧 죽는다고 가정하지 마라"를 알 수 있어야 한다.
    pub fn kill(&self) -> Result<KillOutcome, TermError> {
        let lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
        if lifecycle.reaping || lifecycle.reaped {
            return Ok(KillOutcome::SuppressedAfterReap);
        }
        #[cfg(unix)]
        {
            // 그룹 전체에 SIGKILL — killer(SIGHUP)를 무시하는 자손까지 확실히 종료
            if self.pgid > 0 {
                unsafe {
                    libc::killpg(self.pgid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
        let mut killer = self.killer.lock().expect("pty killer mutex poisoned");
        killer.kill()?;
        drop(lifecycle);
        Ok(KillOutcome::Signalled)
    }

    /// PTY의 foreground 프로세스 그룹. portable-pty가 tcgetpgrp를 안전 래핑한 것.
    /// Windows에는 등가 개념이 없어 메서드 자체가 존재하지 않는다.
    #[cfg(unix)]
    pub fn foreground_pgid(&self) -> Option<i32> {
        let master = self.master.lock().expect("pty master mutex poisoned");
        master.process_group_leader()
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // 좀비를 남기지 않는다 — 세션 래퍼 없이 이 타입만 쓰는 경로도 마찬가지.
        // Drop은 모든 Arc 참조가 사라진 뒤(= 리더/라이터 스레드 종료 후)에만
        // 실행되므로 여기서 child 락을 기다릴 상대가 없다.
        let _ = self.kill();
        let already_reaped = self
            .lifecycle
            .lock()
            .map(|lifecycle| lifecycle.reaped)
            .unwrap_or(false);
        if !already_reaped {
            let _ = self.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 실측된 실제 케이스: 첫 시도가 실패하고 **두 번째**가 성공한다
    /// (Darwin openpty 경쟁의 관측된 형태 그대로 — 55/55가 이랬다).
    /// 재시도가 없다면 이 호출은 Err로 끝난다.
    #[test]
    fn a_failure_on_the_first_attempt_is_retried_and_succeeds() {
        let calls = RefCell::new(0);
        let result: Result<&str, &str> = open_pty_retrying(
            || {
                *calls.borrow_mut() += 1;
                if *calls.borrow() == 1 {
                    Err("transient")
                } else {
                    Ok("pty")
                }
            },
            |_| {},
        );
        assert_eq!(result, Ok("pty"));
        assert_eq!(*calls.borrow(), 2, "should have retried exactly once");
    }

    /// 재시도 예산 안이라면 연속 실패도 회복한다.
    #[test]
    fn failures_up_to_the_budget_are_retried() {
        let calls = RefCell::new(0);
        let result: Result<&str, &str> = open_pty_retrying(
            || {
                *calls.borrow_mut() += 1;
                if *calls.borrow() < OPENPTY_ATTEMPTS as i32 {
                    Err("transient")
                } else {
                    Ok("pty")
                }
            },
            |_| {},
        );
        assert_eq!(result, Ok("pty"));
        assert_eq!(*calls.borrow(), OPENPTY_ATTEMPTS as i32);
    }

    /// 지속되는 실패는 삼키지 않는다: 예산만큼만 시도하고 **마지막** 오류를
    /// 그대로 올려보낸다. 무한 재시도가 되면 이 테스트는 영영 끝나지 않는다.
    #[test]
    fn a_persistent_failure_gives_up_and_reports_the_last_error() {
        let calls = RefCell::new(0);
        let result: Result<&str, String> = open_pty_retrying(
            || {
                *calls.borrow_mut() += 1;
                Err(format!("failure #{}", calls.borrow()))
            },
            |_| {},
        );
        assert_eq!(
            *calls.borrow(),
            OPENPTY_ATTEMPTS as i32,
            "must attempt exactly OPENPTY_ATTEMPTS times, no more and no fewer"
        );
        assert_eq!(
            result,
            Err(format!("failure #{}", OPENPTY_ATTEMPTS)),
            "the error surfaced must be the last one, not the first"
        );
    }

    /// 성공하는 호출은 재시도도 대기도 하지 않는다 — 정상 경로에 비용을
    /// 얹지 않는다는 것이 이 수정의 전제다.
    #[test]
    fn the_happy_path_does_not_sleep_or_retry() {
        let calls = RefCell::new(0);
        let sleeps = RefCell::new(0);
        let result: Result<&str, &str> = open_pty_retrying(
            || {
                *calls.borrow_mut() += 1;
                Ok("pty")
            },
            |_| *sleeps.borrow_mut() += 1,
        );
        assert_eq!(result, Ok("pty"));
        assert_eq!(*calls.borrow(), 1);
        assert_eq!(*sleeps.borrow(), 0);
    }

    /// 첫 재시도는 즉시 이뤄져야 한다(실측상 그것으로 충분하고, 대기는
    /// 순수한 손해다). 그 뒤 시도만 물러선다.
    #[test]
    fn the_first_retry_is_immediate_and_later_ones_back_off() {
        let waits = RefCell::new(Vec::new());
        let calls = RefCell::new(0);
        let _: Result<&str, &str> = open_pty_retrying(
            || {
                *calls.borrow_mut() += 1;
                Err("transient")
            },
            |d| waits.borrow_mut().push(d),
        );
        // 첫 재시도 전에는 sleep이 호출되지 않는다(ZERO는 건너뛴다).
        let waits = waits.borrow();
        assert!(
            waits.iter().all(|d| !d.is_zero()),
            "zero backoffs must not reach the sleeper at all, got {waits:?}"
        );
        assert_eq!(
            waits.len(),
            OPENPTY_ATTEMPTS.saturating_sub(2) as usize,
            "only retries after the first should sleep, got {waits:?}"
        );
        assert!(
            waits.windows(2).all(|w| w[1] > w[0]),
            "backoff must increase, got {waits:?}"
        );
    }
}
