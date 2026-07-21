//! Task 5 벤치 — 행당 `Paragraph::with_spans` vs 셀당 `fill_text`.
//!
//! 플랜이 이 선택을 **추측으로 정하는 것을 금지**한다. 조사 문서는 행당
//! `with_spans`가 이론적 최선이라고 적었지만, `with_spans`는 우리가 넘긴
//! `Shaping`을 무시하고 `cosmic_text::Shaping::Advanced`를 강제한다
//! (`iced_graphics-0.14.0/src/text/paragraph.rs:161`). 그래서 실제로 재본다.
//!
//! # 두 경로가 프레임마다 실제로 하는 일
//!
//! **A. 행당 `with_spans`** — 매 프레임 새로 만든다. 캐시할 자리가 없다:
//! `Widget::draw`가 `&State`(불변)를 받으므로 그린 `Paragraph`를 위젯 상태에
//! 되돌려 놓을 수 없고, Task 3이 동결한 상태에도 문단 캐시 필드가 없다.
//! → 비용 = `Buffer::new` + `set_rich_text`(Advanced 셰이핑) + `align`, 행마다.
//!
//! **B. 셀당 `fill_text`** — 렌더러가 값을 받아 `Text::Cached`로 레이어에 쌓고
//! (`iced_wgpu-0.14.0/src/layer.rs:113-127`), prepare 시점에 내용 문자열을
//! 해시해 셰이핑 결과를 재사용한다(`iced_graphics/src/text/cache.rs:29-42`).
//! 터미널 셀의 내용은 글자 하나라 캐시 적중률이 사실상 100%다.
//! → 비용 = 셀당 `String` 할당 + 키 해시 + 해시맵 조회. **셰이핑이 아니다.**
//!
//! 그래서 B를 "셀당 `Paragraph::with_text`"로 재면 **틀린 수를 얻는다** —
//! 그건 캐시가 매번 빗나가는 세계의 비용이다. 여기서는 `Cache`를 직접 들고
//! 실제 적중 경로를 잰다.
//!
//! # 이 벤치가 재지 **않는** 것
//!
//! GPU 업로드, 아틀라스 갱신, draw call 제출. 창도 GPU도 없이 도는 벤치이므로
//! CPU 측 프레임 준비 비용만 잰다. A가 draw call 수에서 유리한 것은 여기서
//! 드러나지 않는다 — 다만 아래 수치가 그 차이를 논할 여지가 있는 크기인지를
//! 말해준다.

use std::time::Instant;

use iced::advanced::graphics::text::{cache, font_system, Cache, Paragraph};
use iced::advanced::text::{Alignment, LineHeight, Paragraph as _, Shaping, Span, Text, Wrapping};
use iced::alignment;
use iced::{Color, Font, Pixels, Size};

const TEXT_SIZE: Pixels = Pixels(13.0);
const CELL_W: f32 = 7.8;
const CELL_H: f32 = 16.0;

/// 벤치용 한 행. 실제 셸 출력을 닮게 만든다 — 스타일이 몇 번 바뀌고
/// (`ls --color`처럼) 나머지는 이어진다.
fn row_content(row: usize, cols: usize) -> Vec<(char, Color)> {
    const PALETTE: [Color; 4] = [
        Color::WHITE,
        Color { r: 0.4, g: 0.8, b: 0.4, a: 1.0 },
        Color { r: 0.9, g: 0.7, b: 0.2, a: 1.0 },
        Color { r: 0.4, g: 0.6, b: 1.0, a: 1.0 },
    ];
    // 행마다 다른 글자로 채운다 — 같은 행을 반복하면 캐시가 비현실적으로 잘 맞는다.
    (0..cols)
        .map(|col| {
            let c = char::from(b'!' + ((row * 7 + col * 3) % 90) as u8);
            (c, PALETTE[(col / 9 + row) % PALETTE.len()])
        })
        .collect()
}

/// A: 행마다 `Paragraph::with_spans` 하나. 인접한 동일 색 셀은 한 span으로 묶는다
/// (span을 셀마다 만들면 `set_rich_text`가 셀 수만큼 attrs 전환을 한다).
fn path_with_spans(rows: usize, cols: usize) -> usize {
    let mut made = 0;

    for row in 0..rows {
        let cells = row_content(row, cols);
        let mut spans: Vec<Span<'_, ()>> = Vec::new();
        let mut run = String::new();
        let mut run_color = cells[0].1;

        for &(c, color) in &cells {
            if color != run_color {
                spans.push(Span::new(std::mem::take(&mut run)).color(run_color));
                run_color = color;
            }
            run.push(c);
        }
        spans.push(Span::new(run).color(run_color));

        let paragraph = Paragraph::with_spans::<()>(Text {
            content: &spans,
            bounds: Size::new(cols as f32 * CELL_W, CELL_H),
            size: TEXT_SIZE,
            line_height: LineHeight::Absolute(Pixels(CELL_H)),
            font: Font::MONOSPACE,
            align_x: Alignment::Default,
            align_y: alignment::Vertical::Top,
            // 존중되지 않는다. 이 인자가 무시되는 것이 이 벤치의 존재 이유다.
            shaping: Shaping::Basic,
            wrapping: Wrapping::None,
        });

        made += paragraph.min_bounds().width as usize;
    }

    made
}

/// B: 셀당 `fill_text`가 실제로 치르는 비용 — `String` 할당 + 캐시 조회.
/// `Cache`를 프레임 밖에 두는 것이 핵심이다. 렌더러의 캐시가 프레임을 건너
/// 살아 있기 때문이다(`Cache::trim`이 최근 사용분을 유지한다).
fn path_fill_text(cache: &mut Cache, rows: usize, cols: usize) -> usize {
    let mut font_system = font_system().write().expect("write font system");
    let mut hit = 0;

    for row in 0..rows {
        for (c, _color) in row_content(row, cols) {
            // `fill_text`가 `Text<String>`을 **값으로** 받는다 — 셀마다 할당이다
            // (`iced_core/src/text.rs:368-374`).
            let content = c.to_string();

            let (_key, entry) = cache.allocate(
                font_system.raw(),
                cache::Key {
                    content: &content,
                    size: TEXT_SIZE.0,
                    line_height: CELL_H,
                    font: Font::MONOSPACE,
                    bounds: Size::new(CELL_W, CELL_H),
                    shaping: Shaping::Basic,
                    align_x: Alignment::Default,
                },
            );
            hit += entry.min_bounds.width as usize;
        }
    }

    hit
}

/// C. 드로우가 렌더러를 부르기 **전에** 하는 일 — 셀마다 `resolve_cell` 한 번.
/// damage 추적을 도입할지 판단하려면 "전체를 다시 결정하는 비용"을 알아야 한다.
fn path_resolve(rows: usize, cols: usize) -> usize {
    use alacritty_terminal::term::cell::Flags;
    use alacritty_terminal::vte::ansi::{Color as VteColor, NamedColor, Rgb};
    use suaegi_app::terminal::palette;
    use suaegi_app::terminal::render::resolve_cell;
    use suaegi_term::grid::SnapshotCell;

    let p = palette::shared();
    let cells: Vec<SnapshotCell> = (0..cols)
        .map(|col| SnapshotCell {
            c: 'x',
            combining: Vec::new(),
            fg: if col % 3 == 0 {
                VteColor::Named(NamedColor::Foreground)
            } else {
                VteColor::Spec(Rgb {
                    r: col as u8,
                    g: 0,
                    b: 0,
                })
            },
            bg: VteColor::Indexed(col as u8),
            flags: if col % 7 == 0 {
                Flags::BOLD | Flags::UNDERLINE
            } else {
                Flags::empty()
            },
        })
        .collect();

    let mut n = 0;
    for row in 0..rows {
        for (col, cell) in cells.iter().enumerate() {
            let r = resolve_cell(cell, p, (row + col) % 11 == 0, false);
            n += usize::from(r.draw_glyph);
        }
    }
    n
}

/// D. `follow-ups.md` 6번 — 스냅샷 셀 복사 비용. 그리드 락 안에서 뷰포트를
/// 통째로 복사하는 것이 실제로 문제인지 재본다. `clone`이 그 복사와 같은 일을
/// 한다(행 `Vec` + 셀당 `combining: Vec<char>`).
fn path_snapshot_clone(snapshot: &suaegi_term::grid::TerminalSnapshot) -> usize {
    let copy = snapshot.clone();
    copy.rows.len()
}

fn time<T>(label: &str, iters: u32, mut f: impl FnMut() -> T) {
    // 워밍업 — 폰트 로딩과 캐시 채우기를 측정에서 뺀다.
    let _ = f();

    let start = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(f());
    }
    let per_frame = start.elapsed() / iters;
    println!("{label:<44} {per_frame:>12.2?} / frame");
}

fn main() {
    for (rows, cols) in [(24usize, 80usize), (50, 200)] {
        println!("\n--- {rows}x{cols} = {} cells ---", rows * cols);

        time(&format!("A. with_spans (per row, rebuilt)  {rows}x{cols}"), 20, || {
            path_with_spans(rows, cols)
        });

        let mut cache = Cache::new();
        time(&format!("B. fill_text  (per cell, cached)  {rows}x{cols}"), 20, || {
            let n = path_fill_text(&mut cache, rows, cols);
            cache.trim();
            n
        });

        time(&format!("C. resolve_cell over the grid     {rows}x{cols}"), 200, || {
            path_resolve(rows, cols)
        });

        let grid = suaegi_term::grid::TerminalGrid::new(
            suaegi_term::grid::GridSize { rows, cols },
            10_000,
        );
        // 빈 그리드를 재면 `combining`이 전부 비어 있어 낙관적인 수가 나온다.
        // 화면을 실제 글자로 채운 뒤 잰다.
        for row in 0..rows {
            grid.feed(format!("line {row} ").repeat(cols / 8).as_bytes());
            grid.feed(b"\r\n");
        }
        let snapshot = grid.snapshot();
        time(&format!("D. TerminalSnapshot::clone        {rows}x{cols}"), 200, || {
            path_snapshot_clone(&snapshot)
        });
    }
}
