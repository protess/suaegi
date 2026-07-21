//! Task 5 — 스냅샷 → 드로우. 셀 스타일 결정(`resolve_cell`)을 드로우에서
//! 분리하고, 배경 → 커서 → 텍스트 3패스로 그린다.
//!
//! **왜 스타일 결정을 분리하는가.** 반환이 `(fg, bg)`면 `Flags::HIDDEN`을
//! 표현할 수 없다 — HIDDEN은 **글자만** 숨기고 배경은 남긴다. 그러면 플래그
//! 처리가 "색을 정하는 곳"과 "그릴지 정하는 곳" 둘로 갈리고, 둘 중 하나만
//! 고치는 버그가 생긴다. `ResolvedCell::draw_glyph`가 그 갈림을 없앤다.
//!
//! **픽셀 결과는 헤드리스로 검증할 수 없다.** `()` 렌더러의 드로우 메서드는
//! 전부 빈 몸통이라 무엇을 그렸는지 남지 않는다(`iced_core/src/renderer/null.rs`).
//! 아래 테스트는 전부 순수 함수(`resolve_cell`, `cell_selected`,
//! `glyph_span`)에 대한 것이고, **실제 화면은 사람 눈이 필요하다.**

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use iced::advanced::layout::Layout;
use iced::advanced::renderer::Quad;
use iced::advanced::{mouse, renderer, text};
use iced::alignment;
use iced::{Border, Color, Font, Pixels, Point, Rectangle, Shadow, Size, Theme};

use suaegi_term::grid::{SnapshotCell, TerminalSnapshot, ViewportSelection};

use crate::terminal::palette::{self, Palette};
use crate::terminal::state::{CellMetrics, State};
use crate::terminal::Terminal;

// ---------------------------------------------------------------------------
// 셀 스타일 결정 — 순수
// ---------------------------------------------------------------------------

/// 밑줄 다섯 종류. `Flags`가 비트 다섯 개로 나르는 것을 한 값으로 좁힌다 —
/// 여러 개가 동시에 켜질 수 있는데 그릴 수 있는 것은 하나뿐이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineKind {
    Single,
    Double,
    Curl,
    Dotted,
    Dashed,
}

/// 한 셀을 그리는 데 필요한 것 전부. `draw_glyph`가 따로 있는 이유는 위 모듈
/// 문서 참고.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedCell {
    pub fg: Color,
    pub bg: Color,
    /// `false`여도 **배경은 그린다.** HIDDEN과 wide char spacer가 여기 걸린다.
    pub draw_glyph: bool,
    pub underline: Option<UnderlineKind>,
    pub strikeout: bool,
    pub bold: bool,
    pub italic: bool,
}

/// 셀 하나의 색과 장식을 확정한다.
///
/// # 합성 순서 (이 순서가 계약이다)
///
/// 1. **기본 색** — 팔레트에서 `cell.fg` / `cell.bg`를 뽑는다.
/// 2. **DIM 감쇠** — `DIM` 또는 `DIM_BOLD`면 **전경만** 어둡게 한다. 배경까지
///    줄이면 어두운 배경 위의 dim 텍스트가 배경째 흐려져 대비가 되레 커진다.
///    감쇠를 **교환보다 먼저** 하는 이유: INVERSE된 셀에서 dim은 "글자가
///    흐리다"가 아니라 "그 셀의 전경 성분이 흐리다"는 뜻이고, 전경 성분은
///    교환 전의 `fg`다.
/// 3. **INVERSE 교환** — `fg`↔`bg`.
/// 4. **선택 교환** — 선택 영역이면 `fg`↔`bg`.
/// 5. **커서 교환** — 커서가 덮는 칸이면 `fg`↔`bg`.
///
/// **교환은 짝수 번이면 상쇄된다.** 선택 영역 안의 INVERSE 셀이 원래 색으로
/// 돌아오는 것은 버그가 아니라 이 규칙의 결과다 — 그래야 "선택은 반전이다"가
/// 이미 반전된 셀에도 일관되게 적용된다.
///
/// 6. **HIDDEN / spacer** — `draw_glyph`를 내린다. 색에는 손대지 않는다.
///
/// `under_cursor`는 **커서가 칸을 실제로 덮을 때만** `true`다(`Block`).
/// `Beam`/`Underline`/`HollowBlock`은 글자를 가리지 않으므로 교환하면 멀쩡한
/// 셀이 통째로 반전된 것처럼 보인다. 호출자가 그 판단을 한다 — 이 함수는
/// 커서 모양을 모른다.
pub fn resolve_cell(
    cell: &SnapshotCell,
    p: &Palette,
    selected: bool,
    under_cursor: bool,
) -> ResolvedCell {
    let flags = cell.flags;

    // 1. 기본 색
    let mut fg = p.resolve(cell.fg);
    let mut bg = p.resolve(cell.bg);

    // 2. DIM 감쇠 (DIM_BOLD는 DIM 비트를 포함한다 — `intersects`가 맞다)
    if flags.intersects(Flags::DIM | Flags::DIM_BOLD) {
        fg = palette::attenuate(fg, palette::DIM_FACTOR);
    }

    // 3~5. 교환 셋. 홀수 번이면 반전, 짝수 번이면 상쇄된다.
    let swaps =
        u32::from(flags.contains(Flags::INVERSE)) + u32::from(selected) + u32::from(under_cursor);
    if swaps % 2 == 1 {
        std::mem::swap(&mut fg, &mut bg);
    }

    // 6. 글리프를 그릴 것인가. **배경과는 무관하다.**
    //
    // spacer 셀은 앞선 wide char가 이미 그 자리에 글자를 그렸으므로 글리프를
    // 억제한다. 배경까지 억제하면 줄바꿈 경계의 `LEADING_WIDE_CHAR_SPACER`가
    // 배경을 잃어 선택 영역에 구멍이 뚫린다.
    let draw_glyph = !flags
        .intersects(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);

    ResolvedCell {
        fg,
        bg,
        draw_glyph,
        underline: underline_kind(flags),
        strikeout: flags.contains(Flags::STRIKEOUT),
        // `BOLD_ITALIC`은 `BOLD | ITALIC`이고 `DIM_BOLD`는 `DIM | BOLD`다 —
        // 셋 다 `contains(BOLD)`로 걸린다.
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
    }
}

/// 밑줄 비트 다섯 개 중 하나를 고른다. **구체적인 것이 이긴다** — `UNDERCURL`과
/// `UNDERLINE`이 같이 켜져 있으면 curl이 의도다(TUI가 undercurl을 켜면서 폴백용
/// 밑줄을 같이 세우는 일이 있다).
fn underline_kind(flags: Flags) -> Option<UnderlineKind> {
    if flags.contains(Flags::UNDERCURL) {
        Some(UnderlineKind::Curl)
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Some(UnderlineKind::Dotted)
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Some(UnderlineKind::Dashed)
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Some(UnderlineKind::Double)
    } else if flags.contains(Flags::UNDERLINE) {
        Some(UnderlineKind::Single)
    } else {
        None
    }
}

/// 스냅샷의 선택 영역이 이 칸을 포함하는가. 좌표는 **뷰포트 기준이고 양 끝을
/// 포함한다**(`ViewportSelection` 문서).
///
/// 선형과 블록이 다르다: 블록은 직사각형이라 **모든 행에 같은 열 범위**가
/// 걸리고, 선형은 첫 행이 `start.1`부터, 마지막 행이 `end.1`까지다.
pub fn cell_selected(sel: &ViewportSelection, row: usize, col: usize) -> bool {
    let (top, bottom) = (sel.start.0, sel.end.0);
    if row < top || row > bottom {
        return false;
    }

    if sel.is_block {
        let (left, right) = if sel.start.1 <= sel.end.1 {
            (sel.start.1, sel.end.1)
        } else {
            (sel.end.1, sel.start.1)
        };
        return col >= left && col <= right;
    }

    // 선형. 한 행짜리 선택은 두 조건이 **동시에** 걸린다.
    if row == top && col < sel.start.1 {
        return false;
    }
    if row == bottom && col > sel.end.1 {
        return false;
    }
    true
}

/// 글리프가 차지하는 칸 수와, 그리드 오른쪽 끝에서 잘린 실제 폭.
///
/// `WIDE_CHAR`는 **두 칸**이다. 마지막 열에 걸리면 두 번째 칸이 그리드 밖이므로
/// 폭을 한 칸으로 **클리핑**한다 — 클리핑하지 않으면 글리프가 위젯 경계를 넘어
/// 옆 pane을 침범한다.
///
/// `iced_term`은 wide char를 통째로 빠뜨렸다 — CJK가 한 칸에 뭉갠다.
pub fn glyph_span(flags: Flags, col: usize, cols: usize) -> (usize, usize) {
    let wanted = if flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    };
    let available = cols.saturating_sub(col);
    (wanted, wanted.min(available))
}

// ---------------------------------------------------------------------------
// 드로우
// ---------------------------------------------------------------------------

/// 커서 선/빔의 두께와 밑줄 두께. 셀 높이에 비례시키되 1px 아래로는 내리지
/// 않는다 — 0.5px 선은 렌더러가 반투명한 회색으로 뭉갠다.
fn stroke(cell_height: f32) -> f32 {
    (cell_height / 12.0).round().max(1.0)
}

/// `Quad`를 만든다. **`snap`을 명시하는 것이 이 헬퍼의 존재 이유다** —
/// `Quad`의 `Default`가 `snap: cfg!(feature = "crisp")`이라 기본값을 쓰면
/// 셀 격자의 정렬이 **우리 크레이트가 켜지도 않은 feature**에 달린다
/// (`iced_core/src/renderer.rs:90-101`). 배경 런 사이에 1px 틈이 생기거나
/// 안 생기는 것이 그 feature로 갈리면 재현 불가능한 버그가 된다.
fn quad(bounds: Rectangle) -> Quad {
    Quad {
        bounds,
        border: Border::default(),
        shadow: Shadow::default(),
        // 셀 경계는 정수 픽셀에 붙어야 런 사이에 틈이 생기지 않는다.
        snap: true,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw<Renderer>(
    terminal: &Terminal<'_>,
    state: &State,
    renderer: &mut Renderer,
    _theme: &Theme,
    _style: &renderer::Style,
    layout: Layout<'_>,
    _cursor: mouse::Cursor,
    viewport: &Rectangle,
) where
    Renderer: text::Renderer<Font = Font>,
{
    let bounds = layout.bounds();

    // 클리핑 API가 따로 없다 — `with_layer`가 클리핑이다
    // (`iced_core/src/renderer.rs:22-28`). 화면 밖이면 아예 하지 않는다.
    let Some(clip) = bounds.intersection(viewport) else {
        return;
    };

    let p = palette::shared();

    renderer.with_layer(clip, |renderer| {
        // 메트릭이 없으면(측정 실패) 셀을 어디에 놓을지 모른다. 그래도 배경은
        // 칠한다 — 부모의 색이 비치는 것보다 낫다.
        renderer.fill_quad(quad(bounds), p.background());

        let Some(metrics) = state.metrics else {
            return;
        };

        // 폰트와 글자 크기는 **이번 프레임의 설정**이라 `Terminal`에 있다
        // (`State`는 Task 3에서 동결됐고 그 목록에 없다). `render`는
        // `crate::terminal`의 자식 모듈이라 비공개 필드를 볼 수 있다.
        draw_grid(
            terminal.snapshot(),
            state,
            renderer,
            p,
            bounds,
            clip,
            metrics,
            terminal.font,
            terminal.text_size,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_grid<Renderer>(
    snapshot: &TerminalSnapshot,
    state: &State,
    renderer: &mut Renderer,
    p: &Palette,
    bounds: Rectangle,
    clip: Rectangle,
    metrics: CellMetrics,
    font: Font,
    text_size: Pixels,
) where
    Renderer: text::Renderer<Font = Font>,
{
    let (cw, ch) = (metrics.width(), metrics.height());
    let cols = snapshot.size.cols;

    // 커서가 **칸을 덮는** 경우에만 글리프를 반전한다. 언포커스는 `HollowBlock`
    // 이므로 덮지 않는다.
    //
    // `iced_term`은 이 반전을 `APP_CURSOR`로 게이팅한다(`view.rs:577`) —
    // APP_CURSOR는 커서 **키** 모드(화살표가 `ESC O A`냐 `ESC [ A`냐)이고
    // 커서 렌더링과 아무 관계가 없다. 그래서 일반 모드에서 커서 아래 글자가
    // 사라진다.
    let shape = cursor_shape(snapshot, state.focused);
    let covering_cursor = match shape {
        Some(CursorShape::Block) => snapshot.cursor.map(|c| (c.row, c.col)),
        _ => None,
    };

    // 셀당 한 번만 결정하고 세 패스가 같은 결과를 쓴다. 패스마다 다시
    // 결정하면 세 패스가 **다른 색을 볼 수 있는** 경로가 생긴다.
    //
    // **`snapshot.size.cols`가 행 길이의 권위다.** `cells.iter()`를 그대로
    // 돌면 어떤 행이 짧을 때 이후 행 전체의 인덱스가 한 칸씩 밀린다 — 색이
    // 대각선으로 번지는 버그이고, 짧은 행이 나올 일이 없다는 것에 기대는 대신
    // 구조적으로 막는다.
    let blank = SnapshotCell {
        c: ' ',
        combining: Vec::new(),
        fg: alacritty_terminal::vte::ansi::Color::Named(
            alacritty_terminal::vte::ansi::NamedColor::Foreground,
        ),
        bg: alacritty_terminal::vte::ansi::Color::Named(
            alacritty_terminal::vte::ansi::NamedColor::Background,
        ),
        flags: Flags::empty(),
    };
    let mut resolved: Vec<ResolvedCell> = Vec::with_capacity(snapshot.rows.len() * cols);
    for (row, cells) in snapshot.rows.iter().enumerate() {
        for col in 0..cols {
            let cell = cells.get(col).unwrap_or(&blank);
            let selected = snapshot
                .selection
                .as_ref()
                .is_some_and(|sel| cell_selected(sel, row, col));
            let under_cursor = covering_cursor == Some((row, col));
            resolved.push(resolve_cell(cell, p, selected, under_cursor));
        }
    }

    // --- 패스 1: 배경 전부 --------------------------------------------------
    //
    // **모든 보이는 슬롯이 자기 배경을 받는다** — spacer도 포함이다. 같은 색
    // 수평 런으로 묶어 quad 수를 줄인다. 기본 배경은 이미 위에서 한 장으로
    // 칠했으므로 건너뛴다.
    for row in 0..snapshot.rows.len() {
        let line = &resolved[row * cols..(row + 1) * cols];
        let mut col = 0;
        while col < cols {
            let bg = line[col].bg;
            let mut end = col + 1;
            while end < cols && line[end].bg == bg {
                end += 1;
            }
            if bg != p.background() {
                renderer.fill_quad(
                    quad(Rectangle {
                        x: bounds.x + col as f32 * cw,
                        y: bounds.y + row as f32 * ch,
                        width: (end - col) as f32 * cw,
                        height: ch,
                    }),
                    bg,
                );
            }
            col = end;
        }
    }

    // --- 패스 2: 커서 -------------------------------------------------------
    //
    // 배경 **뒤**, 텍스트 **앞**. `iced_term`은 배경 런을 커서보다 늦게 flush해
    // 커서를 덮어버린다.
    if let (Some(shape), Some(cursor)) = (shape, snapshot.cursor) {
        // 커서가 wide char 위에 있으면 두 칸을 덮는다.
        let wide = snapshot
            .rows
            .get(cursor.row)
            .and_then(|r| r.get(cursor.col))
            .is_some_and(|c| c.flags.contains(Flags::WIDE_CHAR));
        let (_, span) = glyph_span(
            if wide {
                Flags::WIDE_CHAR
            } else {
                Flags::empty()
            },
            cursor.col,
            cols,
        );

        let cell = Rectangle {
            x: bounds.x + cursor.col as f32 * cw,
            y: bounds.y + cursor.row as f32 * ch,
            width: span as f32 * cw,
            height: ch,
        };
        draw_cursor(renderer, shape, cell, p.cursor(), stroke(ch));
    }

    // --- 패스 3: 텍스트 -----------------------------------------------------
    for (row, cells) in snapshot.rows.iter().enumerate() {
        for col in 0..cols {
            let cell = cells.get(col).unwrap_or(&blank);
            let r = resolved[row * cols + col];
            let origin = Point::new(bounds.x + col as f32 * cw, bounds.y + row as f32 * ch);
            let (_, span) = glyph_span(cell.flags, col, cols);
            let span_w = span as f32 * cw;

            // 장식은 글리프가 없어도 그린다 — 공백에 밑줄을 긋는 것은 정상이다.
            if let Some(kind) = r.underline {
                draw_underline(renderer, kind, origin, span_w, ch, r.fg);
            }
            if r.strikeout {
                renderer.fill_quad(
                    quad(Rectangle {
                        x: origin.x,
                        y: origin.y + (ch * 0.5).round(),
                        width: span_w,
                        height: stroke(ch),
                    }),
                    r.fg,
                );
            }

            if !r.draw_glyph || (cell.c == ' ' && cell.combining.is_empty()) {
                continue;
            }

            // 결합 문자를 기준 문자에 붙인다. 붙이지 않으면 악센트가 사라진다.
            let mut content = String::with_capacity(1 + cell.combining.len());
            content.push(cell.c);
            content.extend(cell.combining.iter().copied());

            // wide 글리프는 두 칸 **중앙**에 놓는다. `Alignment::Center`는
            // 렌더러에서 "position.x가 중앙"으로 해석된다
            // (`iced_wgpu-0.14.0/src/text.rs:567-577`).
            let (align_x, x) = if span > 1 {
                (text::Alignment::Center, origin.x + span_w / 2.0)
            } else {
                (text::Alignment::Default, origin.x)
            };

            // 마지막 열에 걸린 wide char는 여기서 잘린다. `span_w`가 이미 한
            // 칸으로 줄어 있으므로 clip이 정확히 그리드 안쪽이다.
            let cell_clip = Rectangle {
                x: origin.x,
                y: origin.y,
                width: span_w,
                height: ch,
            };
            let Some(cell_clip) = cell_clip.intersection(&clip) else {
                continue;
            };

            // `Text`에 `Default`가 없다 — 9개 필드 전부 명시한다.
            renderer.fill_text(
                text::Text {
                    content,
                    bounds: Size::new(span_w, ch),
                    size: text_size,
                    // 행 피치를 메트릭과 **같은 값**으로 못 박는다. 위젯이 든
                    // `LineHeight`를 그대로 넘기면 `to_absolute`가 여기서 다시
                    // 계산되어, 반올림 하나로 글자가 행마다 어긋날 수 있다.
                    line_height: text::LineHeight::Absolute(Pixels(ch)),
                    font: glyph_font(font, r.bold, r.italic),
                    align_x,
                    align_y: alignment::Vertical::Top,
                    // CJK·이모지·결합 문자가 `Basic`에서 깨진다. 셀 내용은
                    // 글자 하나라 `Advanced`여도 셰이핑 캐시가 거의 항상
                    // 맞는다(`iced_graphics/src/text/cache.rs:29-42`).
                    shaping: text::Shaping::Advanced,
                    wrapping: text::Wrapping::None,
                },
                Point::new(x, origin.y),
                r.fg,
                cell_clip,
            );
        }
    }
}

/// 이번 프레임에 그릴 커서 모양. `None`이면 그리지 않는다.
///
/// **언포커스면 `HollowBlock`이다.** alacritty가 해주지 않는다 —
/// `Term::is_focused`는 공개 필드이고 `RenderableCursor`는 그것을 보지 않는다.
/// 렌더러가 직접 해야 한다.
///
/// `CursorShape::Hidden`은 `!SHOW_CURSOR`가 우리에게 도달하는 경로다
/// (`alacritty_terminal/src/term/mod.rs:2380`). 언포커스보다 **먼저** 본다 —
/// 숨긴 커서는 포커스를 잃어도 숨긴 것이다.
fn cursor_shape(snapshot: &TerminalSnapshot, focused: bool) -> Option<CursorShape> {
    let shape = snapshot.cursor?.shape;
    match shape {
        CursorShape::Hidden => None,
        _ if !focused => Some(CursorShape::HollowBlock),
        other => Some(other),
    }
}

fn draw_cursor<Renderer: renderer::Renderer>(
    renderer: &mut Renderer,
    shape: CursorShape,
    cell: Rectangle,
    color: Color,
    stroke: f32,
) {
    match shape {
        // 여기 오지 않는다 — `cursor_shape`가 `None`으로 걸렀다.
        CursorShape::Hidden => {}
        CursorShape::Block => renderer.fill_quad(quad(cell), color),
        CursorShape::HollowBlock => {
            let mut q = quad(cell);
            q.border = Border {
                color,
                width: stroke,
                radius: 0.0.into(),
            };
            // 테두리만 남기려면 채움이 투명해야 한다. `Border`는 quad **안쪽**에
            // 그려지므로 셀 밖으로 새지 않는다.
            renderer.fill_quad(q, Color::TRANSPARENT);
        }
        CursorShape::Underline => renderer.fill_quad(
            quad(Rectangle {
                y: cell.y + cell.height - stroke,
                height: stroke,
                ..cell
            }),
            color,
        ),
        CursorShape::Beam => renderer.fill_quad(
            quad(Rectangle {
                width: stroke,
                ..cell
            }),
            color,
        ),
    }
}

/// 밑줄. **`Curl`/`Dotted`/`Dashed`는 근사다** — quad 말고는 프리미티브가 없어
/// 진짜 곡선을 그릴 수 없다(canvas 레이어를 끌어오면 3패스 순서가 깨진다).
/// 서로 **구별되게** 그리는 것이 목표다: undercurl은 맞춤법 오류를, dotted는
/// 다른 뜻을 나르므로 다 같은 실선으로 뭉개면 정보가 사라진다.
fn draw_underline<Renderer: renderer::Renderer>(
    renderer: &mut Renderer,
    kind: UnderlineKind,
    origin: Point,
    width: f32,
    cell_height: f32,
    color: Color,
) {
    let t = stroke(cell_height);
    // 글리프 하단과 겹치지 않게 두께만큼 띄운다.
    let y = origin.y + cell_height - t * 2.0;

    let mut line = |x: f32, w: f32, y: f32| {
        renderer.fill_quad(
            quad(Rectangle {
                x,
                y,
                width: w,
                height: t,
            }),
            color,
        );
    };

    match kind {
        UnderlineKind::Single => line(origin.x, width, y),
        UnderlineKind::Double => {
            line(origin.x, width, y - t);
            line(origin.x, width, y + t);
        }
        UnderlineKind::Dotted => {
            let mut x = origin.x;
            while x < origin.x + width {
                line(x, t, y);
                x += t * 2.0;
            }
        }
        UnderlineKind::Dashed => {
            let dash = (width / 3.0).max(t);
            let mut x = origin.x;
            while x < origin.x + width {
                line(x, dash.min(origin.x + width - x), y);
                x += dash * 2.0;
            }
        }
        UnderlineKind::Curl => {
            // 톱니로 근사한다 — 한 칸에 두 번 오르내린다.
            let step = (width / 4.0).max(t);
            let mut x = origin.x;
            let mut up = true;
            while x < origin.x + width {
                line(
                    x,
                    step.min(origin.x + width - x),
                    if up { y - t } else { y + t },
                );
                x += step;
                up = !up;
            }
        }
    }
}

/// BOLD/ITALIC을 폰트에 반영한다. `Font`는 `Copy`라 매 셀 만들어도 할당이 없다.
fn glyph_font(base: Font, bold: bool, italic: bool) -> Font {
    Font {
        weight: if bold {
            iced::font::Weight::Bold
        } else {
            base.weight
        },
        style: if italic {
            iced::font::Style::Italic
        } else {
            base.style
        },
        ..base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alacritty_terminal::vte::ansi::{Color as VteColor, NamedColor, Rgb};

    fn p() -> Palette {
        Palette::new()
    }

    /// 팔레트에 없는 색 두 개를 골라 쓴다 — 전경/배경을 쓰면 교환이 일어났는지
    /// 아닌지가 "둘 다 회색"이라 눈에 안 띈다.
    const FG: VteColor = VteColor::Spec(Rgb {
        r: 0xff,
        g: 0x00,
        b: 0x00,
    });
    const BG: VteColor = VteColor::Spec(Rgb {
        r: 0x00,
        g: 0x00,
        b: 0xff,
    });

    fn cell(flags: Flags) -> SnapshotCell {
        SnapshotCell {
            c: 'x',
            combining: Vec::new(),
            fg: FG,
            bg: BG,
            flags,
        }
    }

    fn red() -> Color {
        p().resolve(FG)
    }

    fn blue() -> Color {
        p().resolve(BG)
    }

    // ------------------------------------------------------------ 기본과 교환

    #[test]
    fn a_plain_cell_keeps_its_colors_and_draws_its_glyph() {
        assert_eq!(
            resolve_cell(&cell(Flags::empty()), &p(), false, false),
            ResolvedCell {
                fg: red(),
                bg: blue(),
                draw_glyph: true,
                underline: None,
                strikeout: false,
                bold: false,
                italic: false,
            }
        );
    }

    /// 교환 세 갈래와 그 **합성**. 짝수 번이면 상쇄된다는 계약이 여기 있다.
    #[test]
    fn swaps_compose_and_an_even_number_cancels() {
        // (flags, selected, under_cursor, 교환되는가, 왜)
        let cases: &[(Flags, bool, bool, bool, &str)] = &[
            (Flags::empty(), false, false, false, "no swap at all"),
            (Flags::INVERSE, false, false, true, "INVERSE alone"),
            (Flags::empty(), true, false, true, "selection alone"),
            (Flags::empty(), false, true, true, "cursor alone"),
            (
                Flags::INVERSE,
                true,
                false,
                false,
                "INVERSE + selection = two swaps, which cancel",
            ),
            (
                Flags::INVERSE,
                false,
                true,
                false,
                "INVERSE + cursor = two swaps, which cancel",
            ),
            (
                Flags::empty(),
                true,
                true,
                false,
                "selection + cursor = two swaps, which cancel",
            ),
            (
                Flags::INVERSE,
                true,
                true,
                true,
                "all three = three swaps, which is one swap",
            ),
        ];

        for (flags, selected, under_cursor, swapped, why) in cases {
            let r = resolve_cell(&cell(*flags), &p(), *selected, *under_cursor);
            let expected = if *swapped {
                (blue(), red())
            } else {
                (red(), blue())
            };
            assert_eq!((r.fg, r.bg), expected, "{why}");
        }
    }

    // ---------------------------------------------------------------- DIM

    #[test]
    fn dim_attenuates_the_foreground_only() {
        let r = resolve_cell(&cell(Flags::DIM), &p(), false, false);
        assert_eq!(r.fg, palette::attenuate(red(), palette::DIM_FACTOR));
        assert_eq!(r.bg, blue(), "DIM must not touch the background");

        // 대조군: 감쇠가 실제로 색을 바꿨는가.
        assert_ne!(r.fg, red());
    }

    #[test]
    fn dim_bold_attenuates_too_and_is_still_bold() {
        // `DIM_BOLD = DIM | BOLD`라 비트가 겹친다 — `contains(DIM)`으로 보면
        // 놓치지 않지만, `== DIM`으로 보면 놓친다.
        let r = resolve_cell(&cell(Flags::DIM_BOLD), &p(), false, false);
        assert_eq!(r.fg, palette::attenuate(red(), palette::DIM_FACTOR));
        assert!(r.bold, "DIM_BOLD is bold as well as dim");
    }

    /// 감쇠가 **교환보다 먼저**라는 계약. 순서를 뒤집으면 배경이 감쇠된다.
    #[test]
    fn dim_is_applied_before_the_inverse_swap() {
        let r = resolve_cell(&cell(Flags::DIM | Flags::INVERSE), &p(), false, false);
        assert_eq!(
            (r.fg, r.bg),
            (blue(), palette::attenuate(red(), palette::DIM_FACTOR)),
            "the attenuated foreground must end up in the background slot; \
             attenuating after the swap would dim the blue instead"
        );
    }

    // -------------------------------------------------------------- HIDDEN

    #[test]
    fn hidden_suppresses_the_glyph_but_keeps_the_background() {
        let r = resolve_cell(&cell(Flags::HIDDEN), &p(), false, false);
        assert!(!r.draw_glyph, "HIDDEN hides the glyph");
        assert_eq!(
            r.bg,
            blue(),
            "HIDDEN must keep its background — a (fg, bg) return could not \
             express this, which is why draw_glyph exists"
        );
    }

    #[test]
    fn hidden_still_swaps_when_selected() {
        // 숨긴 글자의 셀도 선택 표시는 나야 한다 — 안 그러면 선택 영역에
        // 구멍이 뚫린다.
        let r = resolve_cell(&cell(Flags::HIDDEN), &p(), true, false);
        assert_eq!(r.bg, red(), "a hidden cell still participates in selection");
        assert!(!r.draw_glyph);
    }

    // ------------------------------------------------------ wide char spacer

    /// **배경 슬롯과 글리프 억제를 따로 단언한다.** 하나만 보는 테스트는
    /// 다른 쪽이 깨져도 통과한다.
    #[test]
    fn spacers_suppress_the_glyph_and_keep_the_background() {
        for flags in [Flags::WIDE_CHAR_SPACER, Flags::LEADING_WIDE_CHAR_SPACER] {
            let r = resolve_cell(&cell(flags), &p(), false, false);
            assert!(!r.draw_glyph, "{flags:?} must not draw a glyph");
            assert_eq!(
                r.bg,
                blue(),
                "{flags:?} must still get its own background — dropping it \
                 loses the background of a LEADING_WIDE_CHAR_SPACER at a wrap"
            );
        }
    }

    #[test]
    fn a_selected_spacer_gets_the_selected_background() {
        // 줄바꿈 경계의 spacer가 선택 안에 있을 때. 배경을 억제하면 여기가
        // 정확히 구멍이 된다.
        let r = resolve_cell(&cell(Flags::LEADING_WIDE_CHAR_SPACER), &p(), true, false);
        assert_eq!(r.bg, red(), "the spacer must show the selection");
        assert!(!r.draw_glyph);
    }

    #[test]
    fn a_wide_char_itself_draws_its_glyph() {
        // 대조군: 위 테스트가 "wide 관련 비트면 다 억제"로 통과하지 않게.
        let r = resolve_cell(&cell(Flags::WIDE_CHAR), &p(), false, false);
        assert!(r.draw_glyph, "WIDE_CHAR is the cell that draws the glyph");
    }

    // -------------------------------------------------------------- 장식

    #[test]
    fn every_underline_flag_maps_to_its_kind() {
        let cases: &[(Flags, Option<UnderlineKind>, &str)] = &[
            (Flags::empty(), None, "no underline"),
            (Flags::UNDERLINE, Some(UnderlineKind::Single), "single"),
            (
                Flags::DOUBLE_UNDERLINE,
                Some(UnderlineKind::Double),
                "double",
            ),
            (Flags::UNDERCURL, Some(UnderlineKind::Curl), "curl"),
            (
                Flags::DOTTED_UNDERLINE,
                Some(UnderlineKind::Dotted),
                "dotted",
            ),
            (
                Flags::DASHED_UNDERLINE,
                Some(UnderlineKind::Dashed),
                "dashed",
            ),
            (
                Flags::UNDERCURL | Flags::UNDERLINE,
                Some(UnderlineKind::Curl),
                "the more specific kind wins over a plain UNDERLINE fallback",
            ),
            (
                Flags::ALL_UNDERLINES,
                Some(UnderlineKind::Curl),
                "ALL_UNDERLINES is a mask, not a kind — it must not be None",
            ),
        ];

        for (flags, expected, why) in cases {
            assert_eq!(
                resolve_cell(&cell(*flags), &p(), false, false).underline,
                *expected,
                "{why}"
            );
        }
    }

    #[test]
    fn strikeout_bold_and_italic_are_carried_through() {
        let r = resolve_cell(
            &cell(Flags::STRIKEOUT | Flags::BOLD_ITALIC),
            &p(),
            false,
            false,
        );
        assert!(r.strikeout);
        assert!(r.bold, "BOLD_ITALIC = BOLD | ITALIC");
        assert!(r.italic);

        // 대조군.
        let plain = resolve_cell(&cell(Flags::empty()), &p(), false, false);
        assert!(!plain.strikeout && !plain.bold && !plain.italic);
    }

    // ---------------------------------------------------- 색 갈래 세 개

    #[test]
    fn all_three_color_arms_reach_the_palette() {
        let arms = [
            (VteColor::Named(NamedColor::Red), p().named(NamedColor::Red)),
            (VteColor::Indexed(196), p().resolve(VteColor::Indexed(196))),
            (
                VteColor::Spec(Rgb { r: 1, g: 2, b: 3 }),
                Color::from_rgb8(1, 2, 3),
            ),
        ];

        for (arm, expected) in arms {
            let c = SnapshotCell {
                c: 'x',
                combining: Vec::new(),
                fg: arm,
                bg: VteColor::Named(NamedColor::Background),
                flags: Flags::empty(),
            };
            assert_eq!(
                resolve_cell(&c, &p(), false, false).fg,
                expected,
                "{arm:?} must resolve through the palette"
            );
        }
    }

    // ------------------------------------------------------------- 선택 판정

    fn linear(start: (usize, usize), end: (usize, usize)) -> ViewportSelection {
        ViewportSelection {
            start,
            end,
            is_block: false,
        }
    }

    fn block(start: (usize, usize), end: (usize, usize)) -> ViewportSelection {
        ViewportSelection {
            start,
            end,
            is_block: true,
        }
    }

    #[test]
    fn a_linear_selection_runs_to_the_end_of_each_middle_row() {
        let sel = linear((1, 5), (3, 2));
        // (row, col, 포함?, 왜)
        let cases: &[(usize, usize, bool, &str)] = &[
            (0, 9, false, "above the selection"),
            (1, 4, false, "first row, before the start column"),
            (
                1,
                5,
                true,
                "first row, exactly the start column (inclusive)",
            ),
            (1, 99, true, "first row runs to the end of the line"),
            (2, 0, true, "a middle row is selected from column 0"),
            (2, 99, true, "...to the last column"),
            (3, 2, true, "last row, exactly the end column (inclusive)"),
            (3, 3, false, "last row, past the end column"),
            (4, 0, false, "below the selection"),
        ];

        for (row, col, expected, why) in cases {
            assert_eq!(cell_selected(&sel, *row, *col), *expected, "{why}");
        }
    }

    #[test]
    fn a_single_row_linear_selection_applies_both_bounds() {
        // 한 행짜리 선택은 start와 end 조건이 **동시에** 걸린다. 조건 하나만
        // 보는 구현은 위 테스트를 통과하고 여기서 깨진다.
        let sel = linear((2, 4), (2, 6));
        for (col, expected) in [(3, false), (4, true), (5, true), (6, true), (7, false)] {
            assert_eq!(cell_selected(&sel, 2, col), expected, "column {col}");
        }
    }

    #[test]
    fn a_block_selection_is_a_rectangle_on_every_row() {
        let sel = block((1, 5), (3, 8));
        for row in 1..=3 {
            assert!(!cell_selected(&sel, row, 4), "row {row} col 4 is outside");
            assert!(
                cell_selected(&sel, row, 5),
                "row {row} col 5 is the left edge"
            );
            assert!(
                cell_selected(&sel, row, 8),
                "row {row} col 8 is the right edge"
            );
            assert!(!cell_selected(&sel, row, 9), "row {row} col 9 is outside");
        }
        assert!(!cell_selected(&sel, 0, 6), "above the block");
        assert!(!cell_selected(&sel, 4, 6), "below the block");

        // 대조군: 같은 좌표가 선형이면 다르게 나온다. 이게 없으면 `is_block`을
        // 무시하는 구현도 통과한다.
        assert!(
            cell_selected(&linear((1, 5), (3, 8)), 2, 40),
            "a linear selection covers the whole middle row"
        );
        assert!(!cell_selected(&sel, 2, 40), "a block selection does not");
    }

    #[test]
    fn a_block_selection_normalizes_a_right_to_left_drag() {
        // 오른쪽에서 왼쪽으로 끌면 start.1 > end.1이다. 정규화하지 않으면
        // 아무것도 선택되지 않는다.
        let sel = block((1, 8), (3, 5));
        assert!(cell_selected(&sel, 2, 6));
        assert!(!cell_selected(&sel, 2, 4));
        assert!(!cell_selected(&sel, 2, 9));
    }

    // ------------------------------------------------------------ 글리프 폭

    #[test]
    fn a_wide_char_takes_two_columns_and_clips_at_the_last_one() {
        /// (플래그, 열, 그리드 폭, (원하는 칸 수, 실제 칸 수), 왜 이 케이스인가)
        type Case = (Flags, usize, usize, (usize, usize), &'static str);

        let cases: &[Case] = &[
            (
                Flags::empty(),
                0,
                80,
                (1, 1),
                "a narrow glyph is one column",
            ),
            (Flags::empty(), 79, 80, (1, 1), "...even at the last column"),
            (Flags::WIDE_CHAR, 0, 80, (2, 2), "a wide glyph wants two"),
            (Flags::WIDE_CHAR, 78, 80, (2, 2), "two still fit at col 78"),
            (
                Flags::WIDE_CHAR,
                79,
                80,
                (2, 1),
                "at the last column it still WANTS two but is clipped to one — \
                 without the clip the glyph spills past the widget bounds",
            ),
        ];

        for (flags, col, cols, expected, why) in cases {
            assert_eq!(glyph_span(*flags, *col, *cols), *expected, "{why}");
        }
    }

    // ------------------------------------------------------------- 커서 모양

    fn snapshot_with_cursor(shape: CursorShape) -> TerminalSnapshot {
        use suaegi_term::grid::{GridSize, SnapshotCursor};

        TerminalSnapshot {
            rows: vec![vec![cell(Flags::empty())]],
            size: GridSize { rows: 1, cols: 1 },
            cursor: Some(SnapshotCursor {
                row: 0,
                col: 0,
                shape,
                blinking: false,
            }),
            display_offset: 0,
            history_size: 0,
            mode: alacritty_terminal::term::TermMode::default(),
            selection: None,
        }
    }

    #[test]
    fn all_five_cursor_shapes_are_handled() {
        let focused = true;
        for shape in [
            CursorShape::Block,
            CursorShape::Underline,
            CursorShape::Beam,
            CursorShape::HollowBlock,
        ] {
            assert_eq!(
                cursor_shape(&snapshot_with_cursor(shape), focused),
                Some(shape),
                "{shape:?} must survive when focused"
            );
        }
        assert_eq!(
            cursor_shape(&snapshot_with_cursor(CursorShape::Hidden), focused),
            None,
            "Hidden draws nothing — this is how !SHOW_CURSOR reaches the renderer"
        );
    }

    #[test]
    fn an_unfocused_terminal_draws_a_hollow_block() {
        for shape in [
            CursorShape::Block,
            CursorShape::Underline,
            CursorShape::Beam,
            CursorShape::HollowBlock,
        ] {
            assert_eq!(
                cursor_shape(&snapshot_with_cursor(shape), false),
                Some(CursorShape::HollowBlock),
                "alacritty does not apply this — the renderer must ({shape:?})"
            );
        }
    }

    #[test]
    fn a_hidden_cursor_stays_hidden_when_unfocused() {
        // 대조군 딸린 경계: "언포커스면 무조건 HollowBlock"으로 짜면 숨긴
        // 커서가 언포커스 시 되살아난다.
        assert_eq!(
            cursor_shape(&snapshot_with_cursor(CursorShape::Hidden), false),
            None
        );
    }

    #[test]
    fn no_cursor_in_the_viewport_means_no_cursor_shape() {
        let mut snapshot = snapshot_with_cursor(CursorShape::Block);
        snapshot.cursor = None;
        assert_eq!(cursor_shape(&snapshot, true), None);
    }

    // ---------------------------------------------------------------- 폰트

    #[test]
    fn bold_and_italic_reach_the_font() {
        let base = Font::MONOSPACE;
        assert_eq!(glyph_font(base, false, false), base, "control: unchanged");
        assert_eq!(
            glyph_font(base, true, false).weight,
            iced::font::Weight::Bold
        );
        assert_eq!(
            glyph_font(base, false, true).style,
            iced::font::Style::Italic
        );

        let both = glyph_font(base, true, true);
        assert_eq!(both.weight, iced::font::Weight::Bold);
        assert_eq!(both.style, iced::font::Style::Italic);
        assert_eq!(both.family, base.family, "the family must not change");
    }
}
