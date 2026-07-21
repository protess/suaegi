//! `Drop for TerminalSession`은 최대 2초 UI 스레드를 멈춘다(session.rs 참고).
//! `Arc` 하나를 워커로 옮기는 것만으로는 부족하다 — 구독·프레즌스 폴링이 든
//! 클론이 나중에 떨어지면 마지막 파괴는 여전히 그 클론을 놓은 스레드에서
//! 일어난다. Reaper는 은퇴한 세션들을 **대기 목록**으로 들고, 각 항목의
//! `Arc::strong_count`가 1이 될 때까지(= 다른 클론이 모두 사라질 때까지)
//! 주기적으로 스캔하다가, 1이 된 항목부터 이 스레드에서 떨어뜨린다.
//!
//! 단일 대기(맨 앞 항목 하나가 끝나길 블로킹하며 기다리는 방식)로 만들면 안
//! 된다 — 구독 클론을 오래 쥐고 있는 세션 하나가 뒤에 닫힌 모든 세션의 정리를
//! 막는다(head-of-line blocking). 그래서 매 주기 목록 전체를 스캔한다.

use std::any::Any;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use suaegi_term::session::TerminalSession;

/// pending 목록을 다시 스캔하는 주기. Arc는 "마지막 참조가 사라졌다"는 알림을
/// 주지 않으므로(Weak::upgrade로 흉내낼 수도 있지만 여기선 strong_count 폴링이
/// 더 단순하다) 주기적으로 깨어나 확인하는 수밖에 없다. 짧을수록 반응이
/// 빠르지만 그만큼 자주 깨어난다 — 테스트의 10초 타임아웃에 넉넉히 여유를
/// 두면서도 실사용에서 체감 지연이 없는 값으로 20ms를 택했다.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 은퇴 대상 하나. `Arc<dyn Any + Send + Sync>`로 받는 이유는 테스트가 실제
/// `TerminalSession`이 아니라 가벼운 센티널(`DropSentinel`)을 은퇴시켜 Reaper의
/// 스케줄링 자체(HOL 방지)를 `TerminalSession`의 실제 생성 비용 없이 검증하기
/// 위함이다 — Reaper는 자신이 들고 있는 값의 구체 타입을 알 필요가 없다.
struct Retiree {
    handle: Arc<dyn Any + Send + Sync>,
    /// 이 항목이 실제로 떨어진(= 이 스레드가 마지막 클론을 drop한) 직후 호출된다.
    /// 호출 시점이 스레드 경계를 넘지 않으므로, 콜백 안에서 읽은
    /// `std::thread::current().id()`는 소멸자가 실행된 스레드 그 자체를 증언한다.
    on_reaped: Option<Box<dyn FnOnce() + Send>>,
}

pub struct Reaper {
    tx: Sender<Retiree>,
}

impl Reaper {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Retiree>();
        std::thread::Builder::new()
            .name("suaegi-reaper".to_string())
            .spawn(move || Self::run(rx))
            .expect("failed to spawn reaper thread");
        Self { tx }
    }

    /// 프로덕션 경로: `SessionStore`가 슬롯을 꺼내면서 세션의 `Arc`를 넘긴다.
    pub fn retire(&self, session: Arc<TerminalSession>) {
        self.send(session, None);
    }

    /// `SessionStore`가 "언제 어느 스레드에서 실제로 떨어졌는지"를 관측하려고
    /// 쓰는 내부용 경로. `SessionId`별 기록은 호출자(`SessionStore`)가 콜백
    /// 안에서 채운다 — Reaper 자신은 `SessionId`를 모른다.
    pub(crate) fn retire_with_callback(
        &self,
        session: Arc<TerminalSession>,
        on_reaped: impl FnOnce() + Send + 'static,
    ) {
        self.send(session, Some(Box::new(on_reaped)));
    }

    /// 테스트 전용: `TerminalSession`을 실제로 스폰하는 비용 없이 Reaper의
    /// 스케줄링 규칙(HOL 방지)만 검증하기 위해 임의의 `Arc<T>`를 은퇴시킨다.
    #[doc(hidden)]
    pub fn retire_for_test<T: Any + Send + Sync>(&self, value: Arc<T>) {
        self.send(value, None);
    }

    fn send<T: Any + Send + Sync>(
        &self,
        value: Arc<T>,
        on_reaped: Option<Box<dyn FnOnce() + Send>>,
    ) {
        let handle: Arc<dyn Any + Send + Sync> = value;
        // 워커가 죽어 있으면(패닉) 보낼 곳이 없다 — 세션 하나가 새는 것보다는
        // 낫지만, 이 경로는 정상 동작에서는 절대 밟지 않아야 한다.
        let _ = self.tx.send(Retiree { handle, on_reaped });
    }

    /// 단일 대기가 아니라 **매 주기 pending 전체를 스캔**한다. 앞쪽 항목이
    /// strong_count > 1로 오래 남아 있어도, 뒤이어 들어온 다른 항목이 먼저
    /// strong_count == 1이 되면 그 항목부터 즉시 떨어뜨린다 — 그래야 세션 하나가
    /// 뒤에 닫힌 모든 세션의 정리를 막는 head-of-line blocking이 생기지 않는다.
    fn run(rx: Receiver<Retiree>) {
        let mut pending: Vec<Retiree> = Vec::new();
        loop {
            // 새로 들어온 항목을 블로킹 없이 모두 흡수한다.
            let mut disconnected = false;
            loop {
                match rx.try_recv() {
                    Ok(item) => pending.push(item),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            // strong_count == 1인 항목부터 떨어뜨린다. 인덱스를 앞에서부터 훑되
            // 제거된 자리는 다시 검사하지 않도록 증가를 건너뛴다 — 이 순회는
            // "먼저 준비된 것부터"가 아니라 "준비된 건 전부" 훑는다는 점이
            // head-of-line 방지의 핵심이다.
            let mut i = 0;
            while i < pending.len() {
                if Arc::strong_count(&pending[i].handle) == 1 {
                    let item = pending.remove(i);
                    drop(item.handle);
                    if let Some(on_reaped) = item.on_reaped {
                        on_reaped();
                    }
                } else {
                    i += 1;
                }
            }

            if disconnected && pending.is_empty() {
                return; // 스토어가 사라졌고 처리할 것도 없다 — 조용히 끝낸다
            }

            // 아직 남은 항목이 있으면(또는 채널이 살아 있으면) 잠시 뒤 다시 스캔한다.
            // `recv_timeout`으로 새 항목이 들어오면 즉시 깨어나되, 최소 폴링 주기는
            // 유지한다.
            if pending.is_empty() && !disconnected {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(item) => pending.push(item),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::thread::ThreadId;
    use std::time::Instant;

    struct DropSentinel(Arc<Mutex<Option<ThreadId>>>);
    impl Drop for DropSentinel {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = Some(std::thread::current().id());
        }
    }

    fn wait_until<F: FnMut() -> bool>(t: Duration, mut f: F) -> bool {
        let deadline = Instant::now() + t;
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_session_with_no_extra_clones_is_reaped_promptly() {
        let reaper = Reaper::spawn();
        let where_dropped = Arc::new(Mutex::new(None));
        let sentinel = Arc::new(DropSentinel(where_dropped.clone()));
        let caller = std::thread::current().id();

        reaper.retire_for_test(sentinel);

        assert!(wait_until(Duration::from_secs(10), || {
            where_dropped.lock().unwrap().is_some()
        }));
        assert_ne!(where_dropped.lock().unwrap().unwrap(), caller);
    }
}
