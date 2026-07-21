use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

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
        let pair = pty_system
            .openpty(PtySize {
                rows: spec.rows.max(1),
                cols: spec.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
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

    /// 비블로킹. lifecycle 락을 호출 **전 구간** 잡는다 — try_wait 자체가
    /// 블로킹하지 않으므로 안전하고, 이렇게 해야 "수확됨"과 kill 사이에 틈이 없다.
    ///
    /// `reaping`이 이미 서 있으면(블로킹 `wait()`가 진행 중이거나 이미 끝났으면)
    /// `child` 락에는 **절대 손대지 않고** 바로 `Ok(None)`을 반환한다. `wait()`는
    /// 그 락을 쥔 채로 블로킹하므로, 여기서 그걸 기다리면 `lifecycle`을 쥔 채
    /// 파킹하게 되어 `kill()`의 "즉시 반환" 보장이 깨진다(락 순서 규칙 위반).
    pub fn try_wait(&self) -> Result<Option<i32>, TermError> {
        let mut lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
        if lifecycle.reaping {
            return Ok(None);
        }
        let mut child = self.child.lock().expect("pty child mutex poisoned");
        let status = child.try_wait()?;
        if status.is_some() {
            lifecycle.reaping = true;
            lifecycle.reaped = true;
        }
        Ok(status.map(|status| status.exit_code() as i32))
    }

    /// 블로킹. 리더 스레드가 EOF를 본 뒤 호출한다.
    /// 블로킹 구간에 lifecycle 락을 걸치지 않는 대신, 진입 **전에** `reaping`을
    /// 세워 그 순간부터 kill이 시그널을 보내지 않게 한다 (PID 재사용 방지).
    ///
    /// `child` 락은 블로킹 `child.wait()` 호출을 감싸는 스코프 안에서만 잡고
    /// 반환 즉시 놓는다 — 꼬리의 `reaped` 기록은 그 스코프 **밖**에서, `child`를
    /// 놓은 뒤에 `lifecycle`을 다시 잡아 수행한다. 두 락을 동시에 쥐지 않아야
    /// `try_wait`/`kill`과의 ABBA 역전을 피한다(락 순서 규칙, 위 참고).
    pub fn wait(&self) -> Result<i32, TermError> {
        {
            let mut lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
            lifecycle.reaping = true;
        }
        let status = {
            let mut child = self.child.lock().expect("pty child mutex poisoned");
            child.wait()?
        };
        {
            let mut lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
            lifecycle.reaped = true;
        }
        Ok(status.exit_code() as i32)
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
    pub fn kill(&self) -> Result<(), TermError> {
        let lifecycle = self.lifecycle.lock().expect("lifecycle mutex poisoned");
        if lifecycle.reaping || lifecycle.reaped {
            return Ok(());
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
        Ok(())
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
