//! 플랜 0.8의 두 벤치. **`#[ignore]`다** — 시간을 재는 것이라 CI에서 돌리면
//! 머신 부하에 따라 흔들린다. 수동으로 돌린다:
//!
//! ```text
//! cargo test -p suaegi-term --test latency_bench --release -- --ignored --nocapture
//! ```
//!
//! 재는 것: **"락이 짧다"는 보장이 아니라 가정이다.** `feed()`가 최대 64KiB
//! 청크를 락 쥔 채 파싱하므로, UI 스레드의 intent 메서드가 그 뒤에 밀릴 수 있다.
//! 경합이 눈에 보이면 intent 처리를 세션당 직렬 워커로 옮겨야 한다 — 그 판단의
//! 근거를 추측이 아니라 실측으로 만든다.
//!
//! **렌더 벤치나 추출 벤치로 입력 지연 판단을 대신하지 않는다** — 재는 것이 다르다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::index::Side;
use suaegi_term::grid::{GridSize, TerminalGrid};
use suaegi_term::input_types::{
    ClickKind, KeyInput, KeyLocation, Mods, MouseAction, MouseIntent, NamedKey, TermKey,
    ViewportHit,
};

const READ_CHUNK: usize = 64 * 1024;

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn report(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let total: Duration = samples.iter().sum();
    println!(
        "{label}: n={} mean={:?} p50={:?} p95={:?} p99={:?} max={:?}",
        samples.len(),
        total / samples.len() as u32,
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        percentile(&samples, 0.99),
        samples.last().copied().unwrap_or_default(),
    );
}

fn arrow_up() -> KeyInput {
    KeyInput {
        key: TermKey::Named(NamedKey::ArrowUp),
        physical_latin: None,
        location: KeyLocation::Standard,
        mods: Mods::default(),
        text: None,
        repeat: false,
    }
}

fn press_at(row: usize, col: usize) -> MouseIntent {
    MouseIntent {
        action: MouseAction::Press(suaegi_term::input_types::TermMouseButton::Left),
        hit: ViewportHit {
            row,
            col,
            side: Side::Left,
        },
        held: Some(suaegi_term::input_types::TermMouseButton::Left),
        mods: Mods::default(),
        click: ClickKind::Single,
        force_local: false,
    }
}

/// 리더가 **최대 크기 청크**를 쉬지 않고 먹이는 동안 UI 스레드가 intent
/// 메서드를 반복 호출한다. 그 지연 분포가 이 벤치의 산출물이다.
#[test]
#[ignore = "타이밍 측정 — 수동으로 --ignored --nocapture로 돌린다"]
fn input_latency_while_the_reader_feeds_max_size_chunks() {
    let grid = Arc::new(TerminalGrid::new(GridSize { rows: 50, cols: 200 }, 10_000));
    let stop = Arc::new(AtomicBool::new(false));

    // 파서를 실제로 일하게 만드는 청크 — 개행과 SGR이 섞인 진짜 출력에 가깝다.
    let chunk: Vec<u8> = {
        let unit = b"\x1b[31mhello \x1b[0mworld 0123456789 abcdefghij\r\n";
        unit.iter().copied().cycle().take(READ_CHUNK).collect()
    };

    let feeder = {
        let grid = Arc::clone(&grid);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut chunks = 0u64;
            while !stop.load(Ordering::Relaxed) {
                grid.feed(&chunk);
                chunks += 1;
            }
            chunks
        })
    };

    // 워밍업 — 첫 호출의 할당을 표본에 넣지 않는다.
    for _ in 0..50 {
        let _ = grid.encode_key_locked(&arrow_up());
    }

    let key = arrow_up();
    let mut key_samples = Vec::with_capacity(2_000);
    let mut mouse_samples = Vec::with_capacity(2_000);
    for i in 0..2_000 {
        let t = Instant::now();
        let _ = grid.encode_key_locked(&key);
        key_samples.push(t.elapsed());

        let t = Instant::now();
        let _ = grid.handle_mouse(&press_at(i % 50, i % 200));
        mouse_samples.push(t.elapsed());
    }

    stop.store(true, Ordering::Relaxed);
    let chunks = feeder.join().expect("feeder thread");

    println!("--- input latency under a saturating reader ---");
    println!("reader fed {chunks} chunks of {READ_CHUNK} bytes while sampling");
    report("encode_key_locked", key_samples);
    report("handle_mouse", mouse_samples);
}

/// 대조군. 위 숫자가 큰지 작은지는 경합이 **없을 때**와 견줘야만 말할 수 있다.
#[test]
#[ignore = "타이밍 측정 — 수동으로 --ignored --nocapture로 돌린다"]
fn input_latency_with_an_idle_reader() {
    let grid = TerminalGrid::new(GridSize { rows: 50, cols: 200 }, 10_000);
    let key = arrow_up();
    for _ in 0..50 {
        let _ = grid.encode_key_locked(&key);
    }

    let mut key_samples = Vec::with_capacity(2_000);
    let mut mouse_samples = Vec::with_capacity(2_000);
    for i in 0..2_000 {
        let t = Instant::now();
        let _ = grid.encode_key_locked(&key);
        key_samples.push(t.elapsed());

        let t = Instant::now();
        let _ = grid.handle_mouse(&press_at(i % 50, i % 200));
        mouse_samples.push(t.elapsed());
    }

    println!("--- input latency with no competing reader (control) ---");
    report("encode_key_locked", key_samples);
    report("handle_mouse", mouse_samples);
}

/// **최대 크기 스크롤백 전체**를 선택한 채 추출한다. `selection_to_string()`이
/// 선택 범위 전체를 훑으므로 여기가 UI 스레드에 있으면 안 되는 이유다.
#[test]
#[ignore = "타이밍 측정 — 수동으로 --ignored --nocapture로 돌린다"]
fn extraction_latency_over_a_full_scrollback() {
    let rows = 50;
    let cols = 200;
    let scrollback = 10_000;
    let grid = TerminalGrid::new(GridSize { rows, cols }, scrollback);

    // 스크롤백을 실제로 채운다 — 빈 줄이면 훑을 것이 없어 벤치가 거짓말을 한다.
    let line: String = "x".repeat(cols);
    for _ in 0..(scrollback + rows) {
        grid.feed(format!("{line}\r\n").as_bytes());
    }
    let history = grid.snapshot().history_size;
    assert!(
        history >= scrollback - 1,
        "the scrollback did not actually fill: {history}"
    );

    // 히스토리 맨 위에서 화면 맨 아래까지 한 번에 끈다.
    grid.scroll_display(alacritty_terminal::grid::Scroll::Top);
    let left = Some(suaegi_term::input_types::TermMouseButton::Left);
    grid.handle_mouse(&press_at(0, 0)).expect("press routes");
    grid.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
    grid.handle_mouse(&MouseIntent {
        action: MouseAction::Release(suaegi_term::input_types::TermMouseButton::Left),
        hit: ViewportHit {
            row: rows - 1,
            col: cols - 1,
            side: Side::Right,
        },
        held: left,
        mods: Mods::default(),
        click: ClickKind::Single,
        force_local: false,
    })
    .expect("release routes");

    let epoch = grid.selection_epoch();
    let extracted = grid.extract_selection(epoch).expect("a selection exists");
    println!("--- extraction latency ---");
    println!(
        "scrollback={scrollback} rows history={history} extracted {} bytes",
        extracted.len()
    );

    let mut samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let t = Instant::now();
        let text = grid.extract_selection(epoch);
        samples.push(t.elapsed());
        assert!(text.is_some(), "the epoch must stay valid across samples");
    }
    report("extract_selection", samples);
}
