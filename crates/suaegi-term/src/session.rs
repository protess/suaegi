use std::io::{ErrorKind, Read};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Select, Sender, TryRecvError, TrySendError};

use alacritty_terminal::grid::Scroll;

use crate::grid::{GridSize, TerminalGrid, TerminalSnapshot, TitleChange};
use crate::input_types::{
    CopyRequest, CopyTargets, KeyInput, MouseEncodeError, MouseIntent, MouseResult, WriteOutcome,
};
use crate::pty::{KillOutcome, PtySession, PtySpawn, TermError};

const READ_BUFFER_SIZE: usize = 64 * 1024;
/// exit_code를 원자적으로 다루기 위한 "아직 종료 안 됨" 표식
const NO_EXIT: i64 = i64::MIN;
/// UI 입력 큐 상한. 무제한 채널은 자식이 읽기를 멈췄을 때 메모리를 무한히 먹는다.
/// 가득 차면 UI 입력은 버린다 (입력 유실 < 앱 다운).
const WRITE_QUEUE_CAPACITY: usize = 256;
/// 장치 응답 큐 상한. 이전에는 언바운드였다: 자식이 장치 질의(`\033[c` 등)를
/// 계속 쏟아내면서 자기 stdin을 읽지 않으면, 커널 PTY 입력 버퍼가 차서 라이터의
/// 블로킹 `pty.write`가 파킹되고, 그동안 리더는 계속 응답을 큐에 밀어넣어 자식의
/// 출력 속도로 메모리가 무한히 자란다. 여기서 지켜야 할 진짜 불변식은 "리더는
/// 절대 블로킹하지 않는다"이고, 큰 바운드 큐 + 가득 차면 드롭(`try_send`)도 이를
/// 만족한다 — 자기 tty를 읽지 않으면서 질의만 쏟아내는 프로그램은 이미 고장난
/// 것이고, 그 응답을 버리는 게 세션을 죽이는 것보다 낫다. 4096개는 응답 하나당
/// 많아야 수십 바이트인 걸 감안하면 정상적인 시동 핸드셰이크(여러 질의가
/// 몰리는 vim/neovim류)를 넉넉히 흡수하면서도(약 수백 KB) 상한을 유지한다.
const REPLY_QUEUE_CAPACITY: usize = 4096;
/// Drop이 reader/writer 스레드의 조인을 기다리는 상한. `killpg(SIGKILL)`은
/// 자식의 프로세스 그룹까지만 닿는다 — `setsid()`로 그룹을 빠져나갔지만
/// 상속받은 PTY 슬레이브 FD를 닫지 않은 자손이 있으면 리더는 EOF를 영원히
/// 보지 못해 join이 끝나지 않는다. 정상 경로(자식이 그룹 안에서 죽는 흔한
/// 경우)에서는 killpg 직후 리더/라이터가 수십 ms 안에 끝난다 — 기존
/// `dropping_the_session_does_not_block` 류 테스트가 그 사실의 실측 근거다.
/// 2초는 그 정상 경로에 넉넉한 여유를 주면서, 탈출한 자손이 슬레이브를 붙든
/// 드문 경우에도 UI 스레드가 이 시간을 넘겨 얼어붙지 않게 하는 상한이다 —
/// Drop이 UI 스레드에서 도는 Plan 3 이후에는 "순간적으로 느껴짐"이 사용자
/// 관점의 요구사항이다. 넘기면 스레드를 조인하지 않고 분리(detach)한다 —
/// 러스트에는 스레드를 강제 종료할 방법이 없으므로 스레드 하나가 새게(누수)
/// 되지만, 그 자손이 언젠가 끝나면(또는 프로세스 자체가 끝나면) 함께
/// 정리된다. "가끔 스레드 하나가 새는 것"이 "UI가 영원히 멈추는 것"보다
/// 명백히 나은 트레이드오프다.
const JOIN_DEADLINE: Duration = Duration::from_secs(2);

/// `handle`이 `deadline` 안에 끝나면 조인해서 패닉을 전파한다(다른 스레드의
/// 패닉을 삼키지 않기 위함). 넘기면 조인을 포기하고 핸들을 버려(분리) 즉시
/// 반환한다 — 이 함수 자체는 절대 `deadline`보다 오래 블로킹하지 않는다.
fn join_with_deadline(handle: JoinHandle<()>, deadline: Duration) {
    let start = Instant::now();
    while !handle.is_finished() {
        if start.elapsed() >= deadline {
            // 조인을 포기하고 분리한다 — drop(handle)은 스레드를 죽이지 않지만
            // (러스트는 스레드 강제 종료 수단이 없다) 더 이상 그 종료를 기다리지
            // 않는다는 뜻이다. 스레드는 언젠가(자손이 슬레이브를 놓으면) 스스로
            // 끝난다.
            drop(handle);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = handle.join();
}

#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub pty: PtySpawn,
    pub scrollback: usize,
}

pub struct TerminalSession {
    pty: Arc<PtySession>,
    grid: Arc<TerminalGrid>,
    generation: Arc<AtomicU64>,
    exit_code: Arc<AtomicI64>,
    running: Arc<AtomicBool>,
    /// Drop에서 닫아 라이터 스레드를 끝낸다
    writes: Mutex<Option<Sender<Vec<u8>>>>,
    reader_thread: Mutex<Option<JoinHandle<()>>>,
    writer_thread: Mutex<Option<JoinHandle<()>>>,
    /// `resize()`가 pty와 grid를 한 쌍으로 갱신하는 동안 다른 resize 호출이
    /// 끼어들지 못하게 막는다. `&self`가 `Sync`라 동시 호출이 가능한데, 이 락이
    /// 없으면 두 resize가 인터리브돼(PTY=A, PTY=B, grid=B, grid=A) pty와 grid가
    /// 서로 다른 크기로 어긋난 채 남을 수 있다.
    resize_lock: Mutex<()>,
}

impl TerminalSession {
    pub fn start(spec: SessionSpec) -> Result<Self, TermError> {
        let size = GridSize {
            rows: spec.pty.rows.max(1) as usize,
            cols: spec.pty.cols.max(1) as usize,
        };
        let (pty, reader) = PtySession::spawn(spec.pty)?;

        let pty = Arc::new(pty);
        let grid = Arc::new(TerminalGrid::new(size, spec.scrollback));
        let generation = Arc::new(AtomicU64::new(0));
        let exit_code = Arc::new(AtomicI64::new(NO_EXIT));
        let running = Arc::new(AtomicBool::new(true));
        // UI 입력은 바운드(유실 허용), 장치 응답은 언바운드 별도 큐(유실 불가).
        // 하나의 큐를 공유하면 UI 입력이 큐를 채운 사이 리더가 응답 송신에서
        // 블로킹돼 PTY 출력 소비가 멈추는 교착이 생긴다.
        let (ui_tx, ui_rx) = crossbeam_channel::bounded::<Vec<u8>>(WRITE_QUEUE_CAPACITY);
        let (reply_tx, reply_rx) = crossbeam_channel::bounded::<Vec<u8>>(REPLY_QUEUE_CAPACITY);

        // 라이터 스레드: PTY write도 블로킹이므로 UI가 직접 부르지 않게 분리한다.
        // std::sync::mpsc 대신 crossbeam_channel을 쓰는 이유: `Select`로 두 큐를
        // 동시에 기다릴 수 있어 폴링 없이(따라서 지연 없이) 응답을 받을 수 있다
        // — 예전에는 20ms 주기로만 깨어나 장치 질의 핸드셰이크(vim/neovim의
        // DA1/DSR/OSC 색상 질의 등)가 눈에 보이는 지연을 겪었다.
        let writer_thread = {
            let writer_pty = Arc::clone(&pty);
            match std::thread::Builder::new()
                .name("suaegi-pty-writer".to_string())
                .spawn(move || {
                    // 한 번만 등록하고 루프 내내 재사용한다 — select는 재등록 없이
                    // 반복 호출할 수 있다.
                    let mut select = Select::new();
                    let reply_idx = select.recv(&reply_rx);
                    let ui_idx = select.recv(&ui_rx);

                    loop {
                        let mut failed = false;
                        // 리더는 reply_tx의 마지막 사본을 들고 있다(아래에서 세션이
                        // 자기 사본을 drop한다) — 그래서 Disconnected는 곧 "리더가
                        // 끝났다 = 자식이 죽었다"는 신호다. UI 송신자가 아직 살아
                        // 있어도(세션이 보관 중이어도) 여기서 끝내야, 자식이 죽은
                        // 세션을 오래 들고 있을 때 이 스레드가 계속 깨어나며 남아
                        // 있는 비용을 없앨 수 있다.
                        let mut reader_gone = false;
                        loop {
                            match reply_rx.try_recv() {
                                Ok(bytes) => {
                                    if writer_pty.write(&bytes).is_err() {
                                        failed = true;
                                        break;
                                    }
                                }
                                Err(TryRecvError::Empty) => break,
                                Err(TryRecvError::Disconnected) => {
                                    reader_gone = true;
                                    break;
                                }
                            }
                        }
                        if failed {
                            break;
                        }
                        if reader_gone {
                            break;
                        }

                        // 매 반복 위에서 응답을 먼저 다 비운 뒤에만 여기 도달하므로,
                        // select가 어느 쪽을 깨우든 응답 우선순위는 유지된다.
                        let oper = select.select();
                        if oper.index() == reply_idx {
                            // 리더가 select 대기 중에 사라졌다면 Err(_) — 다음
                            // 반복의 위쪽 드레인 루프가 Disconnected로 잡아 끝낸다.
                            if let Ok(bytes) = oper.recv(&reply_rx) {
                                if writer_pty.write(&bytes).is_err() {
                                    break;
                                }
                            }
                        } else if oper.index() == ui_idx {
                            match oper.recv(&ui_rx) {
                                Ok(bytes) => {
                                    if writer_pty.write(&bytes).is_err() {
                                        break;
                                    }
                                }
                                // UI 송신자가 사라졌으면(세션 Drop) 남은 응답만
                                // 비우고 끝낸다
                                Err(_) => {
                                    while let Ok(bytes) = reply_rx.try_recv() {
                                        let _ = writer_pty.write(&bytes);
                                    }
                                    break;
                                }
                            }
                        } else {
                            unreachable!("select only registered reply_rx and ui_rx");
                        }
                    }
                }) {
                Ok(handle) => handle,
                Err(e) => {
                    // 자식이 이미 떠 있다 — PtySession의 Drop이 kill+reap을 한다
                    return Err(TermError::ThreadSpawn(e.to_string()));
                }
            }
        };

        // 리더 스레드: 블로킹 read 전용
        let reader_thread = {
            // 클로저로 옮길 클론은 이름을 달리한다 — 섀도잉하면 실패 분기에서
            // 바깥 `pty`를 쓸 수 없게 되어 컴파일 에러가 난다
            let reader_pty = Arc::clone(&pty);
            let grid = Arc::clone(&grid);
            let generation = Arc::clone(&generation);
            let exit_code = Arc::clone(&exit_code);
            let running = Arc::clone(&running);
            // 클론은 이름을 달리해 소유권을 분명히 한다 — 같은 이름으로 섀도잉하면
            // 실패 분기에서 "moved value" 컴파일 에러가 난다
            let reader_reply_tx = reply_tx.clone();
            let mut reader = reader;
            let spawned = std::thread::Builder::new()
                .name("suaegi-pty-reader".to_string())
                .spawn(move || {
                    let mut buf = vec![0u8; READ_BUFFER_SIZE];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) => break, // EOF
                            Ok(n) => {
                                grid.feed(&buf[..n]);
                                // 터미널이 만든 응답을 PTY로 돌려보내지 않으면
                                // 장치 질의를 보낸 프로그램이 영원히 기다린다.
                                // try_send는 절대 블로킹하지 않는다 — 큐가 가득
                                // 찼다면(라이터가 막혀 못 비우는 중) 이 응답은
                                // 버린다. 자기 tty를 읽지 않으면서 질의만
                                // 쏟아내는 프로그램에 대한 유일한 안전한 대응이다.
                                for reply in grid.take_pty_writes() {
                                    match reader_reply_tx.try_send(reply.into_bytes()) {
                                        Ok(()) => {}
                                        Err(TrySendError::Full(_)) => {}
                                        Err(TrySendError::Disconnected(_)) => break,
                                    }
                                }
                                generation.fetch_add(1, Ordering::Release);
                            }
                            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                    // EOF든 읽기 에러든 **수확 전에** 한 번 죽인다. 이 시점의 pgid는
                    // 아직 유효하므로(수확 전) 남은 자손을 안전하게 정리할 수 있고,
                    // 아래 블로킹 wait()가 살아 있는 자식 때문에 멈추지 않는다.
                    // 자식이 이미 스스로 종료했다면 무해한 no-op이다.
                    let _ = reader_pty.kill();
                    // EOF가 자식 종료보다 먼저 올 수 있으므로 블로킹 wait로 확정한다
                    // 순서 의존성: exit_code를 먼저 저장하고 running을 나중에
                    // 저장한다 — PresenceMonitor::probe(presence.rs)가 이 순서에
                    // 기대어 "!is_running()을 봤다면 exit_code도 이미 발행됐다"고
                    // 가정한다. 이 둘의 저장 순서를 바꾸면 그 재읽기가 다시
                    // stale None을 볼 수 있다.
                    if let Ok(code) = reader_pty.wait() {
                        exit_code.store(code as i64, Ordering::Release);
                    }
                    running.store(false, Ordering::Release);
                    generation.fetch_add(1, Ordering::Release);
                });
            match spawned {
                Ok(handle) => handle,
                Err(e) => {
                    // 라이터 스레드와 자식을 정리하고 나간다. 두 송신자를 모두
                    // 떨어뜨려야 라이터 루프가 Disconnected로 끝난다.
                    drop(ui_tx);
                    drop(reply_tx);
                    let _ = pty.kill();
                    let _ = writer_thread.join();
                    return Err(TermError::ThreadSpawn(e.to_string()));
                }
            }
        };

        // reply_tx의 마지막 사본은 리더 스레드가 들고 있다 — 여기서 떨어뜨려야
        // 리더 종료 시 라이터가 Disconnected를 보고 끝날 수 있다
        drop(reply_tx);

        Ok(Self {
            pty,
            grid,
            generation,
            exit_code,
            running,
            writes: Mutex::new(Some(ui_tx)),
            reader_thread: Mutex::new(Some(reader_thread)),
            writer_thread: Mutex::new(Some(writer_thread)),
            resize_lock: Mutex::new(()),
        })
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        self.grid.snapshot()
    }

    /// 논블로킹. 실제 쓰기는 라이터 스레드가 수행한다. 큐가 가득 차면(자식이
    /// 읽기를 멈춘 상태) 이 입력은 버린다 — 무한 버퍼링으로 메모리를 먹는 것보다
    /// 낫다. 반환값으로 유실 여부를 알린다.
    pub fn write(&self, bytes: Vec<u8>) -> bool {
        let writes = self.writes.lock().expect("writes mutex poisoned");
        match writes.as_ref() {
            Some(tx) => tx.try_send(bytes).is_ok(),
            None => false,
        }
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), TermError> {
        // 레이아웃 초기 패스에서 0이 들어올 수 있다 — 퇴화된 그리드를 만들지 않는다
        if rows == 0 || cols == 0 {
            return Ok(());
        }
        // pty.resize와 grid.resize를 한 쌍으로 직렬화한다 — 동시 호출이
        // 인터리브되면 둘이 서로 다른 크기로 어긋난 채 남을 수 있다.
        let _guard = self.resize_lock.lock().expect("resize mutex poisoned");
        self.pty.resize(rows, cols)?;
        self.grid.resize(GridSize {
            rows: rows as usize,
            cols: cols as usize,
        });
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// 테스트 전용 관찰창: 현재 PTY 크기를 grid 크기와 독립적으로 조회한다.
    /// resize()의 pty/grid 원자성을 외부에서 검증하려면 둘을 각자 읽을 방법이
    /// 있어야 한다 — snapshot()은 grid 크기만 준다.
    #[doc(hidden)]
    pub fn pty_size(&self) -> Result<(u16, u16), TermError> {
        self.pty.size()
    }

    /// 테스트 전용 관찰창: 라이터 스레드가 이미 끝났는지. 프로덕션 코드는 이
    /// 값에 의존하지 않는다 — "자식이 죽으면 라이터도 곧 끝난다"를 세션을
    /// 계속 들고 있는 상태에서 외부에서 검증하기 위한 창일 뿐이다.
    #[doc(hidden)]
    pub fn writer_thread_is_finished(&self) -> bool {
        self.writer_thread
            .lock()
            .expect("writer thread mutex poisoned")
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(true)
    }

    /// UI가 락 없이 "다시 그려야 하나"를 판단하는 값. 그리드 변경마다 증가한다.
    ///
    /// **정확한 스냅샷 버전이 아니라 eventual 무효화 신호다.** 카운터는 그리드
    /// 락 **밖에서** 올라간다 — `feed`/`resize`/`scroll_display` 모두 그리드
    /// 호출이 반환한 **뒤에** `fetch_add` 한다. 따라서 generation을 `G`로 읽고
    /// 뜬 스냅샷이 `G+1`짜리 변경까지 이미 담고 있을 수 있다.
    ///
    /// 이 순서가 안전한 방향인 이유: 변경이 **먼저**, 카운터가 **나중**이므로
    /// "`G`를 읽었다"는 곧 "`G`까지의 변경은 이미 그리드에 반영돼 있다"는 뜻이다.
    /// 그래서 스냅샷이 갱신을 **놓치는** 일은 없고, 기껏해야 이미 반영된 변경
    /// 때문에 한 번 더 뜨는 **중복**이 생긴다. 반대 순서(카운터 먼저)였다면
    /// 놓치는 쪽이 되어 화면이 영영 낡은 채로 남는다.
    ///
    /// → 소비자는 이 값을 **단조 staleness 라벨**로만 다뤄야 한다. "generation
    /// `G`인 스냅샷은 정확히 `G`시점의 상태"라고 가정하는 코드를 쓰면 안 된다.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self.exit_code.load(Ordering::Acquire) {
            NO_EXIT => None,
            code => Some(code as i32),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// 안에서는 리더가 EOF를 본 뒤에만 `wait()`를 부르므로 억제(`Suppressed
    /// AfterReap`)가 관찰되더라도 언제나 안전하다 — 그 시점엔 자식이 이미 죽어
    /// 있거나 죽는 중이다. 그래도 반환값 자체는 raw `PtySession::kill()`과
    /// 동일하게 정직해야 하므로 그대로 넘긴다.
    pub fn kill(&self) -> Result<KillOutcome, TermError> {
        self.pty.kill()
    }

    pub fn take_title_changes(&self) -> Vec<TitleChange> {
        self.grid.take_title_changes()
    }

    /// 스크롤백 이동. 화면 내용이 바뀌므로 generation을 올려 UI가 재렌더하게 한다.
    pub fn scroll_display(&self, scroll: Scroll) {
        self.grid.scroll_display(scroll);
        self.generation.fetch_add(1, Ordering::Release);
    }

    // -----------------------------------------------------------------------
    // 입력 — grid가 인코딩하고(락 안), 여기서 큐에 넣는다(락 밖)
    //
    // **락 중첩이 없다.** grid 메서드는 바이트를 돌려주고 이미 락을 놓은 상태다.
    // 앱은 `TerminalGrid`에 닿을 수 없으므로 이 래퍼들이 유일한 입구다.
    // -----------------------------------------------------------------------

    pub fn send_key(&self, input: &KeyInput) -> WriteOutcome {
        match self.grid.encode_key_locked(input) {
            Some(bytes) => self.enqueue(bytes),
            None => WriteOutcome::Suppressed,
        }
    }

    pub fn send_paste(&self, text: &str) -> WriteOutcome {
        self.enqueue(self.grid.encode_paste_locked(text))
    }

    /// 지금 터미널이 `BRACKETED_PASTE` 모드인가. 프롬프트 주입 게이트가 composer
    /// 준비 여부를 판단하는 전제다(값싼 락 한 번).
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.grid.bracketed_paste_enabled()
    }

    /// 초기 프롬프트를 **모드와 무관하게 항상 bracketed paste로 감싸** PTY에
    /// 써넣는다(라이터 스레드가 실제 쓰기를 한다, 논블로킹). 게이트(app)가 이미
    /// `BRACKETED_PASTE` 활성화와 조용한 창을 확인한 뒤에만 부른다 — 그래서
    /// `send_paste`처럼 라이브 모드를 다시 읽지 않고 무조건 감싼다(관측과 쓰기
    /// 사이 모드가 바뀌는 경쟁에서도 주입이 raw로 새지 않게). 종료자는
    /// 페이로드에서 제거된다(`wrap_bracketed_paste`, 프롬프트도 신뢰 불가 입력).
    /// 반환값은 큐 적재 성공 여부 — 유실돼도 사용자가 직접 타이핑하면 되므로
    /// 호출부는 조용히 버린다.
    pub fn inject_bracketed_paste(&self, text: &str) -> bool {
        self.write(crate::encode::wrap_bracketed_paste(text))
    }

    pub fn report_focus(&self, focused: bool) -> WriteOutcome {
        match self.grid.encode_focus_locked(focused) {
            Some(bytes) => self.enqueue(bytes),
            None => WriteOutcome::Suppressed,
        }
    }

    /// `redraw`면 generation을 올린다 — 스냅샷 스케줄링이 generation으로 도는데
    /// (Plan 3) 다시 그리라고만 하면 **옛 스냅샷을 옛 선택으로 다시 그린다.**
    /// 선택 변경을 화면에 반영하려면 새 스냅샷을 찍어야 한다.
    pub fn send_mouse(&self, intent: &MouseIntent) -> Result<MouseResult, MouseEncodeError> {
        let result = self.grid.handle_mouse(intent)?;
        if result.redraw {
            self.generation.fetch_add(1, Ordering::Release);
        }
        let write = match result.bytes {
            Some(bytes) => self.enqueue(bytes),
            None => WriteOutcome::Suppressed,
        };
        Ok(MouseResult {
            write,
            redraw: result.redraw,
            copy: result.copy,
        })
    }

    pub fn extract_selection(&self, epoch: u64) -> Option<String> {
        self.grid.extract_selection(epoch)
    }

    pub fn request_copy(&self, to: CopyTargets) -> Option<CopyRequest> {
        self.grid.request_copy(to)
    }

    /// 빈 바이트열은 **억제다.** 보낼 것이 없는데 `Queued`를 돌려주면 앱이 유실
    /// 피드백 규칙을 잘못된 쪽으로 적용한다.
    fn enqueue(&self, bytes: Vec<u8>) -> WriteOutcome {
        if bytes.is_empty() {
            return WriteOutcome::Suppressed;
        }
        if self.write(bytes) {
            WriteOutcome::Queued
        } else {
            WriteOutcome::Dropped
        }
    }

    #[cfg(unix)]
    pub fn foreground_pgid(&self) -> Option<i32> {
        self.pty.foreground_pgid()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // 세션 객체를 놓으면 자식 프로세스와 두 스레드가 반드시 정리되어야 한다.
        let _ = self.pty.kill();
        // 라이터 채널을 닫아 라이터 루프를 끝낸다
        if let Ok(mut writes) = self.writes.lock() {
            writes.take();
        }
        // unix: killpg(SIGKILL)로 그룹 전체가 죽으므로 보통은 슬레이브가 닫히고
        // 리더가 EOF에 도달한다 — 하지만 `setsid()`로 그룹을 빠져나간 자손이
        // 슬레이브 FD를 들고 있으면 killpg가 닿지 않아 리더가 영원히 블로킹될 수
        // 있다. `join_with_deadline`으로 그 경우에도 Drop이 멈추지 않게 한다
        // (`JOIN_DEADLINE` 주석 참고).
        // Windows: 자손 프로세스를 확실히 죽일 방법이 없어(job object는 post-MVP)
        // 리더가 EOF를 못 볼 수 있으므로 join하지 않고 분리한다.
        // 리더와 라이터 각각에 새로 `JOIN_DEADLINE`을 주면 둘 다 막힌 최악의
        // 경우 UI 스레드가 그 두 배(4초)를 먹는다. 첫 조인 시작 시각을 기준으로
        // 남은 예산을 두 번째 조인에 넘겨, 전체 Drop이 `JOIN_DEADLINE` 하나로
        // 묶이게 한다.
        #[cfg(unix)]
        let join_budget_start = Instant::now();
        #[cfg(unix)]
        if let Ok(mut handle) = self.reader_thread.lock() {
            if let Some(handle) = handle.take() {
                join_with_deadline(handle, JOIN_DEADLINE);
            }
        }
        #[cfg(not(unix))]
        if let Ok(mut handle) = self.reader_thread.lock() {
            let _ = handle.take(); // 분리 (join하면 영원히 멈출 수 있다)
        }
        // 라이터도 같은 이유로 unix에서만 join하되, 리더 조인이 이미 써버린
        // 시간만큼 데드라인을 줄여 총 예산이 `JOIN_DEADLINE`을 넘지 않게 한다.
        // Windows에서는 자손이 의사 콘솔을 붙들고 있으면 write_all이 블로킹된
        // 채로 남을 수 있다.
        #[cfg(unix)]
        if let Ok(mut handle) = self.writer_thread.lock() {
            if let Some(handle) = handle.take() {
                let remaining = JOIN_DEADLINE.saturating_sub(join_budget_start.elapsed());
                join_with_deadline(handle, remaining);
            }
        }
        #[cfg(not(unix))]
        if let Ok(mut handle) = self.writer_thread.lock() {
            let _ = handle.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `join_with_deadline`은 Drop에서 쓰는 그대로다: 스레드가 데드라인을 넘겨도
    /// 살아 있으면 그 자연 종료를 기다리지 않고 반환해야 한다(넘겨받은 데드라인
    /// 근처에서, 스레드가 실제로 끝날 때까지 기다리지 않고). 이 스레드는
    /// 데드라인보다 훨씬 오래(10초) 도는데, 검증 실패 시(=옛 무조건 join
    /// 코드로 되돌리면) 이 테스트 자체가 10초 넘게 멈춰서 실패를 분명히
    /// 드러낸다.
    #[test]
    fn join_with_deadline_detaches_instead_of_waiting_out_a_stuck_thread() {
        let handle = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(10));
        });
        let deadline = Duration::from_millis(200);
        let start = Instant::now();
        join_with_deadline(handle, deadline);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= deadline,
            "returned before the deadline elapsed: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "must not wait for the 10s thread to finish once the deadline passed, took {elapsed:?}"
        );
    }

    /// 스레드가 데드라인 안에 스스로 끝나면 정상적으로 조인해야 한다(=데드라인을
    /// 다 채우고서야 반환하는 게 아니라 스레드 종료 직후 반환) — 위 테스트와
    /// 짝을 이뤄 "빠르면 즉시 반환, 느리면 데드라인에서 분리"라는 계약 전체를
    /// 검증한다.
    #[test]
    fn join_with_deadline_joins_promptly_when_the_thread_finishes_early() {
        let handle = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
        });
        let start = Instant::now();
        join_with_deadline(handle, Duration::from_secs(5));
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "should return shortly after the thread finishes, not wait out the full \
             deadline, took {elapsed:?}"
        );
    }
}
