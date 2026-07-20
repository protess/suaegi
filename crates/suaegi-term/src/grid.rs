use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub rows: usize,
    pub cols: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotCell {
    pub c: char,
    /// 결합 문자(zero-width). `c`만 그리면 결합 문자가 있는 텍스트가 깨진다.
    pub combining: Vec<char>,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
}

/// 락 없이 렌더링할 수 있는 뷰포트 사본. 스크롤백 전체를 복사하지 않는다 —
/// 출력마다 수 MB를 복사하면 이 프로젝트의 존재 이유(성능)와 충돌한다.
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    pub rows: Vec<Vec<SnapshotCell>>,
    pub size: GridSize,
    /// 커서가 표시 중인 뷰포트 안에 있을 때만 Some (스크롤백을 올려보면 None)
    pub cursor: Option<SnapshotCursor>,
    pub display_offset: usize,
    pub history_size: usize,
}

impl TerminalSnapshot {
    pub fn row_text(&self, row: usize) -> String {
        match self.rows.get(row) {
            Some(cells) => cells
                .iter()
                .map(|c| c.c)
                .collect::<String>()
                .trim_end()
                .to_string(),
            None => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TitleChange {
    Set(String),
    Reset,
}

/// 터미널의 부수 효과를 모으는 공유 상태. `send_event`가 `&self`라서 내부는 Mutex.
#[derive(Debug, Default)]
struct GridEventState {
    pty_writes: Mutex<Vec<String>>,
    title_changes: Mutex<Vec<TitleChange>>,
}

/// 로컬 뉴타입. `impl EventListener for Arc<..>`는 외래 트레이트 + 외래 타입이라
/// 고아 규칙에 걸린다 — 우리 타입에 구현하고 Clone으로 상태를 공유한다.
/// Term은 프록시 접근자를 제공하지 않으므로 TerminalGrid도 같은 클론을 보관한다.
#[derive(Debug, Clone, Default)]
pub struct GridEventProxy(Arc<GridEventState>);

impl EventListener for GridEventProxy {
    fn send_event(&self, event: Event) {
        match event {
            // 장치 질의 응답 등 — PTY로 되돌려 쓰지 않으면 질의한 프로그램이 멈춘다
            Event::PtyWrite(text) => {
                self.0.pty_writes.lock().expect("pty write mutex").push(text);
            }
            // 빈 타이틀은 리셋이다 — Set("")로 두면 UI가 이전 타이틀을 지울지
            // 빈 문자열을 표시할지 구분할 수 없다
            Event::Title(title) => {
                let change = if title.is_empty() {
                    TitleChange::Reset
                } else {
                    TitleChange::Set(title)
                };
                self.0.title_changes.lock().expect("title mutex").push(change);
            }
            Event::ResetTitle => {
                self.0
                    .title_changes
                    .lock()
                    .expect("title mutex")
                    .push(TitleChange::Reset);
            }
            _ => {}
        }
    }
}

pub struct TerminalGrid {
    term: FairMutex<Term<GridEventProxy>>,
    parser: Mutex<Processor>,
    proxy: GridEventProxy,
}

impl TerminalGrid {
    pub fn new(size: GridSize, scrollback: usize) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let proxy = GridEventProxy::default();
        // 스크롤백은 생성 시 고정된다 — 바꾸려면 Term을 새로 만들어야 한다
        let term = Term::new(config, &size, proxy.clone());
        Self {
            term: FairMutex::new(term),
            parser: Mutex::new(Processor::new()),
            proxy,
        }
    }

    /// PTY에서 읽은 바이트를 그리드에 반영한다. 부분 UTF-8은 파서가 유지하므로
    /// 청크 경계를 호출자가 맞출 필요는 없다.
    pub fn feed(&self, bytes: &[u8]) {
        let mut term = self.term.lock();
        let mut parser = self.parser.lock().expect("parser mutex");
        parser.advance(&mut *term, bytes);
    }

    pub fn resize(&self, size: GridSize) {
        let mut term = self.term.lock();
        term.resize(size);
    }

    /// 표시 중인 뷰포트의 스냅샷. 모든 값을 **같은 락 안에서** 읽어 size와 rows가
    /// 어긋나(리사이즈 경합) 렌더링 중 인덱스를 초과하는 일이 없게 한다.
    ///
    /// 그리드를 `Line(0..screen_lines)`로 직접 인덱싱하지 않고 `display_iter`를
    /// 쓰는 이유: 사용자가 스크롤백을 올려본 상태(display_offset > 0)에서 전자는
    /// 항상 최신 화면을 복사해 화면에 보이는 것과 다른 내용을 렌더링하게 된다.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let term = self.term.lock();
        let rows_len = term.grid().screen_lines();
        let cols_len = term.grid().columns();
        let history_size = term.grid().history_size();

        let content = term.renderable_content();
        let display_offset = content.display_offset;
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;

        let mut rows: Vec<Vec<SnapshotCell>> = vec![Vec::with_capacity(cols_len); rows_len];
        // display_iter가 내는 point.line은 **그리드 좌표**다: 스크롤백을 올려보면
        // 히스토리 줄이 음수 Line으로 나온다. 음수를 버리면 화면이 빈 채로 그려지므로
        // display_offset을 더해 0..rows_len의 뷰포트 좌표로 옮긴다.
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + display_offset as i32;
            if row < 0 || row as usize >= rows_len {
                continue;
            }
            let cell = indexed.cell;
            rows[row as usize].push(SnapshotCell {
                c: cell.c,
                // zerowidth()는 Cell의 메서드다 (CellExtra가 아니라)
                combining: cell.zerowidth().unwrap_or_default().to_vec(),
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags,
            });
        }
        // 행 길이를 cols_len으로 맞춰 렌더러가 균일하게 인덱싱할 수 있게 한다
        let blank = SnapshotCell {
            c: ' ',
            combining: Vec::new(),
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        };
        for row in rows.iter_mut() {
            row.resize(cols_len, blank.clone());
        }

        let cursor = {
            // 커서 좌표도 같은 그리드 좌표계다 — 동일하게 뷰포트 좌표로 옮긴다
            let r = cursor_point.line.0 + display_offset as i32;
            let c = cursor_point.column.0;
            if r >= 0 && (r as usize) < rows_len && c < cols_len {
                Some(SnapshotCursor {
                    row: r as usize,
                    col: c,
                    shape: cursor_shape,
                })
            } else {
                // 스크롤백을 올려보는 중이면 커서가 화면 밖일 수 있다
                None
            }
        };

        TerminalSnapshot {
            rows,
            size: GridSize {
                rows: rows_len,
                cols: cols_len,
            },
            cursor,
            display_offset,
            history_size,
        }
    }

    /// 스크롤백 이동. 스냅샷이 표시 좌표계를 쓰므로 즉시 반영된다.
    pub fn scroll_display(&self, lines: i32) {
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Delta(lines));
    }

    /// 터미널이 생성한 PTY 응답을 비우고 반환한다. 호출자는 반드시 PTY로 써야 한다.
    pub fn take_pty_writes(&self) -> Vec<String> {
        let mut writes = self.proxy.0.pty_writes.lock().expect("pty write mutex");
        std::mem::take(&mut *writes)
    }

    pub fn take_title_changes(&self) -> Vec<TitleChange> {
        let mut changes = self.proxy.0.title_changes.lock().expect("title mutex");
        std::mem::take(&mut *changes)
    }
}
