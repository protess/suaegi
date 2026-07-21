use suaegi_term::grid::{GridSize, TerminalGrid, TitleChange, TITLE_CHANGES_CAPACITY};

#[test]
fn plain_text_lands_in_the_grid() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    grid.feed(b"hello");
    assert_eq!(grid.snapshot().row_text(0), "hello");
}

#[test]
fn newline_advances_to_the_next_row() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    grid.feed(b"first\r\nsecond");
    let snap = grid.snapshot();
    assert_eq!(snap.row_text(0), "first");
    assert_eq!(snap.row_text(1), "second");
}

#[test]
fn ansi_clear_screen_is_interpreted_not_printed() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    grid.feed(b"garbage");
    grid.feed(b"\x1b[2J\x1b[H");
    grid.feed(b"clean");
    assert_eq!(grid.snapshot().row_text(0), "clean");
}

#[test]
fn utf8_split_across_chunks_is_reassembled() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    let bytes = "한".as_bytes(); // 3바이트
    assert_eq!(bytes.len(), 3);
    // 한 문자 중간에서 쪼갠다 — 파서가 상태를 유지해야 복원된다
    grid.feed(&bytes[..1]);
    grid.feed(&bytes[1..]);
    assert!(grid.snapshot().row_text(0).starts_with('한'));
}

#[test]
fn cursor_position_tracks_output() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    grid.feed(b"abc");
    let cursor = grid.snapshot().cursor.expect("cursor should be on screen");
    assert_eq!((cursor.row, cursor.col), (0, 3));
}

#[test]
fn combining_characters_are_preserved() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    // e + combining acute accent
    grid.feed("e\u{0301}".as_bytes());
    let snap = grid.snapshot();
    let cell = &snap.rows[0][0];
    assert_eq!(cell.c, 'e');
    assert!(
        cell.combining.contains(&'\u{0301}'),
        "combining mark must survive the snapshot"
    );
}

#[test]
fn scrolled_view_shows_history_not_the_live_screen() {
    let grid = TerminalGrid::new(GridSize { rows: 3, cols: 20 }, 50);
    for i in 0..10 {
        grid.feed(format!("line{i}\r\n").as_bytes());
    }
    let live = grid.snapshot();
    assert_eq!(live.display_offset, 0);
    grid.scroll_display(5); // 위로 5줄
    let scrolled = grid.snapshot();
    assert_eq!(scrolled.display_offset, 5);
    assert_ne!(
        scrolled.row_text(0),
        live.row_text(0),
        "scrolled snapshot must show history, not the live screen"
    );
    // 좌표 변환을 빠뜨리면 히스토리 행이 통째로 버려져 빈 화면이 된다 —
    // 위 assert_ne만으로는 그 버그가 통과하므로 실제 내용까지 확인한다
    assert!(
        scrolled.row_text(0).starts_with("line"),
        "scrolled row must contain history text, got {:?}",
        scrolled.row_text(0)
    );
}

#[test]
fn device_status_query_produces_a_pty_write() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    grid.feed(b"\x1b[c"); // DA1
    let writes = grid.take_pty_writes();
    assert!(!writes.is_empty(), "terminal must answer device queries");
    assert!(grid.take_pty_writes().is_empty(), "take must drain");
}

#[test]
fn title_set_and_reset_are_distinguishable() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    grid.feed(b"\x1b]0;my-title\x07");
    // 빈 타이틀은 리셋으로 보고되어야 한다 — Set("")과 구분되지 않으면
    // UI가 이전 타이틀을 지울 수 없다
    grid.feed(b"\x1b]0;\x07");
    let changes = grid.take_title_changes();
    assert_eq!(
        changes,
        vec![TitleChange::Set("my-title".to_string()), TitleChange::Reset],
        "empty title must surface as Reset, not Set(\"\")"
    );
    assert!(grid.take_title_changes().is_empty(), "take must drain");
}

/// `take_title_changes`를 아무도 부르지 않으면(UI가 폴링을 놓쳤거나 죽었으면)
/// 타이틀 이스케이프를 반복하는 자식이 벡터를 무한정 키울 수 있다 — 상한이
/// 없으면 이 테스트는 `TITLE_CHANGES_CAPACITY`의 몇 배를 흘려보낸 뒤에도
/// 그만큼(또는 그 이상)이 그대로 쌓여 있는 것으로 실패해야 한다.
#[test]
fn title_changes_are_capped_and_keep_the_most_recent() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    let flood = TITLE_CHANGES_CAPACITY * 4;
    for i in 0..flood {
        grid.feed(format!("\x1b]0;title-{i}\x07").as_bytes());
    }
    let changes = grid.take_title_changes();
    assert_eq!(
        changes.len(),
        TITLE_CHANGES_CAPACITY,
        "title_changes must be capped at TITLE_CHANGES_CAPACITY, not grow with the flood"
    );
    // 가장 오래된 건 버려지고 최신 항목들이 남아야 한다
    assert_eq!(
        changes.last(),
        Some(&TitleChange::Set(format!("title-{}", flood - 1))),
        "the most recent title change must be preserved"
    );
    let oldest_kept = flood - TITLE_CHANGES_CAPACITY;
    assert_eq!(
        changes.first(),
        Some(&TitleChange::Set(format!("title-{oldest_kept}"))),
        "the oldest surviving entry must be exactly capacity entries behind the newest"
    );
}

#[test]
fn resize_changes_snapshot_dimensions() {
    let grid = TerminalGrid::new(GridSize { rows: 10, cols: 40 }, 100);
    grid.resize(GridSize { rows: 20, cols: 60 });
    let snap = grid.snapshot();
    assert_eq!(snap.size.rows, 20);
    assert_eq!(snap.size.cols, 60);
    assert_eq!(snap.rows.len(), 20, "row count must match reported size");
    assert!(snap.rows.iter().all(|r| r.len() == 60));
}

#[test]
fn snapshot_rows_always_match_reported_size() {
    // 렌더러가 size를 믿고 인덱싱하므로 둘은 항상 일관돼야 한다
    let grid = TerminalGrid::new(GridSize { rows: 5, cols: 10 }, 100);
    for size in [(3, 8), (12, 30), (5, 10)] {
        grid.resize(GridSize {
            rows: size.0,
            cols: size.1,
        });
        let snap = grid.snapshot();
        assert_eq!(snap.rows.len(), snap.size.rows);
        assert!(snap.rows.iter().all(|r| r.len() == snap.size.cols));
    }
}

#[test]
fn scrollback_retains_lines_beyond_the_viewport() {
    let grid = TerminalGrid::new(GridSize { rows: 3, cols: 20 }, 50);
    for i in 0..10 {
        grid.feed(format!("line{i}\r\n").as_bytes());
    }
    assert!(
        grid.snapshot().history_size > 0,
        "scrollback should retain lines"
    );
}
