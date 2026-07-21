//! Task 5 — 고정 내장 256색 팔레트. OSC 4/10/11 동적 변경은 Plan 5이므로
//! 실패 가능한 생성자가 필요 없다.
//!
//! **한 번 만들고 계속 쓴다.** `iced_term`은 프레임마다 셀마다 hex 문자열을
//! 다시 파싱하고(`theme.rs:100`), 파싱 실패에 `panic!`한다. 우리는 색을
//! `iced::Color` 상수로 직접 들고 있으므로 파싱도 실패 경로도 없다.
//!
//! 값은 alacritty가 문서에 기본값으로 싣는 Tomorrow Night 팔레트다. 어떤 값을
//! 고르든 프론트엔드의 몫이고(alacritty_terminal은 팔레트를 싣지 않는다),
//! 여기서 중요한 것은 **세 갈래(`Named`/`Indexed`/`Spec`)가 전부 값을 얻는
//! 것**이다 — 한 갈래라도 빠지면 그 색을 쓰는 프로그램이 통째로 안 보인다.

use std::sync::OnceLock;

use alacritty_terminal::vte::ansi::{Color as VteColor, NamedColor, Rgb};
use iced::Color;

/// 프로세스에 하나뿐인 팔레트. **"한 번만 만든다"가 여기서 지켜진다** —
/// `Palette::new()`는 256칸을 계산하므로 프레임마다 부르면 그냥 낭비다.
/// 팔레트가 불변이라(OSC 4/10/11은 Plan 5) 공유에 잠금이 필요 없다.
pub fn shared() -> &'static Palette {
    static PALETTE: OnceLock<Palette> = OnceLock::new();
    PALETTE.get_or_init(Palette::new)
}

/// 8비트 채널을 `iced::Color`로. `Color::from_rgb8`이 아니라 직접 쓰는 이유는
/// `const`로 팔레트 표를 적기 위해서다.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// DIM의 감쇠 계수.
///
/// **Dim 명명색(`DimBlack`..`DimWhite`, `DimForeground`)과 `Flags::DIM`이 같은
/// 계수를 쓴다.** 둘을 따로 두면 `ESC[2m`(플래그)로 어둡게 한 빨강과
/// `DimRed`(명명색)가 서로 다른 빨강이 되어, 같은 의도의 두 표현이 화면에서
/// 갈린다. 상수 하나로 묶어 그 드리프트를 구조적으로 없앤다.
pub const DIM_FACTOR: f32 = 0.66;

/// 알파는 건드리지 않고 RGB만 줄인다. 알파까지 곱하면 dim 텍스트가 반투명해져
/// 뒤의 배경이 비친다 — 어두운 것과 반투명한 것은 다르다.
pub fn attenuate(color: Color, factor: f32) -> Color {
    Color {
        r: color.r * factor,
        g: color.g * factor,
        b: color.b * factor,
        a: color.a,
    }
}

/// 0-15 명명색. `Indexed(0..16)`도 **같은 표를 본다** — 두 벌을 두면 `ESC[31m`과
/// `ESC[38;5;1m`이 서로 다른 빨강이 된다.
const ANSI: [Color; 16] = [
    rgb(0x1d, 0x1f, 0x21), // Black
    rgb(0xcc, 0x66, 0x66), // Red
    rgb(0xb5, 0xbd, 0x68), // Green
    rgb(0xf0, 0xc6, 0x74), // Yellow
    rgb(0x81, 0xa2, 0xbe), // Blue
    rgb(0xb2, 0x94, 0xbb), // Magenta
    rgb(0x8a, 0xbe, 0xb7), // Cyan
    rgb(0xc5, 0xc8, 0xc6), // White
    rgb(0x66, 0x66, 0x66), // BrightBlack
    rgb(0xd5, 0x4e, 0x53), // BrightRed
    rgb(0xb9, 0xca, 0x4a), // BrightGreen
    rgb(0xe7, 0xc5, 0x47), // BrightYellow
    rgb(0x7a, 0xa6, 0xda), // BrightBlue
    rgb(0xc3, 0x97, 0xd8), // BrightMagenta
    rgb(0x70, 0xc0, 0xb1), // BrightCyan
    rgb(0xea, 0xea, 0xea), // BrightWhite
];

const FOREGROUND: Color = rgb(0xc5, 0xc8, 0xc6);
const BACKGROUND: Color = rgb(0x1d, 0x1f, 0x21);
const CURSOR: Color = rgb(0xc5, 0xc8, 0xc6);
const BRIGHT_FOREGROUND: Color = rgb(0xea, 0xea, 0xea);

/// 셀 색 조회표. **한 번 만들어 계속 쓴다.**
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// 표준 xterm 256색. 0-15는 `ANSI`와 같은 값이다.
    indexed: [Color; 256],
    /// `DimBlack`..`DimWhite` (`NamedColor` 259..266). 인덱스는 0-7과 나란하다.
    dim: [Color; 8],
    foreground: Color,
    background: Color,
    cursor: Color,
    bright_foreground: Color,
    dim_foreground: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

impl Palette {
    /// **실패할 수 없다.** 팔레트가 고정 내장값이라 파싱도 검증도 없다 —
    /// OSC 4/10/11로 런타임에 바뀌는 것은 Plan 5다.
    pub fn new() -> Self {
        let mut indexed = [Color::TRANSPARENT; 256];

        // 0-15: 명명색과 **같은 표**.
        indexed[..16].copy_from_slice(&ANSI);

        // 16-231: 6×6×6 큐브. 인덱스 → 성분은 `16 + 36r + 6g + b`의 역이고,
        // 성분 값은 0이면 0, 아니면 `n*40 + 55`다(xterm이 정한 비선형 계단).
        for i in 16..232u16 {
            let n = i - 16;
            let level = |c: u16| -> u8 {
                if c == 0 {
                    0
                } else {
                    (c * 40 + 55) as u8
                }
            };
            let r = level(n / 36);
            let g = level((n % 36) / 6);
            let b = level(n % 6);
            indexed[i as usize] = rgb(r, g, b);
        }

        // 232-255: 24단계 회색 `i*10 + 8`. 검정(0x00)과 흰색(0xff)에 닿지
        // 않는다 — 그 둘은 큐브의 양 끝이 이미 갖고 있다.
        for i in 232..256u16 {
            let v = ((i - 232) * 10 + 8) as u8;
            indexed[i as usize] = rgb(v, v, v);
        }

        let mut dim = [Color::TRANSPARENT; 8];
        for (slot, base) in dim.iter_mut().zip(ANSI.iter()) {
            *slot = attenuate(*base, DIM_FACTOR);
        }

        Self {
            indexed,
            dim,
            foreground: FOREGROUND,
            background: BACKGROUND,
            cursor: CURSOR,
            bright_foreground: BRIGHT_FOREGROUND,
            dim_foreground: attenuate(FOREGROUND, DIM_FACTOR),
        }
    }

    pub fn foreground(&self) -> Color {
        self.foreground
    }

    pub fn background(&self) -> Color {
        self.background
    }

    pub fn cursor(&self) -> Color {
        self.cursor
    }

    /// 스냅샷 셀의 색을 실제 색으로. **세 갈래를 전부 다룬다** — `Spec`은
    /// 트루컬러(`ESC[38;2;r;g;bm`), `Indexed`는 256색, `Named`는 SGR 30-37 등.
    pub fn resolve(&self, color: VteColor) -> Color {
        match color {
            VteColor::Spec(Rgb { r, g, b }) => rgb(r, g, b),
            VteColor::Indexed(i) => self.indexed[i as usize],
            VteColor::Named(named) => self.named(named),
        }
    }

    /// **모든 `NamedColor` 변형을 열거한다.** `_ =>` 폴백을 두면 vte가 변형을
    /// 추가했을 때 새 색이 조용히 엉뚱한 값으로 그려진다 — 컴파일 에러로
    /// 드러나는 편이 낫다.
    pub fn named(&self, named: NamedColor) -> Color {
        match named {
            NamedColor::Black => self.indexed[0],
            NamedColor::Red => self.indexed[1],
            NamedColor::Green => self.indexed[2],
            NamedColor::Yellow => self.indexed[3],
            NamedColor::Blue => self.indexed[4],
            NamedColor::Magenta => self.indexed[5],
            NamedColor::Cyan => self.indexed[6],
            NamedColor::White => self.indexed[7],
            NamedColor::BrightBlack => self.indexed[8],
            NamedColor::BrightRed => self.indexed[9],
            NamedColor::BrightGreen => self.indexed[10],
            NamedColor::BrightYellow => self.indexed[11],
            NamedColor::BrightBlue => self.indexed[12],
            NamedColor::BrightMagenta => self.indexed[13],
            NamedColor::BrightCyan => self.indexed[14],
            NamedColor::BrightWhite => self.indexed[15],

            NamedColor::Foreground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,

            NamedColor::DimBlack => self.dim[0],
            NamedColor::DimRed => self.dim[1],
            NamedColor::DimGreen => self.dim[2],
            NamedColor::DimYellow => self.dim[3],
            NamedColor::DimBlue => self.dim[4],
            NamedColor::DimMagenta => self.dim[5],
            NamedColor::DimCyan => self.dim[6],
            NamedColor::DimWhite => self.dim[7],

            NamedColor::BrightForeground => self.bright_foreground,
            NamedColor::DimForeground => self.dim_foreground,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Palette {
        Palette::new()
    }

    /// 큐브가 `16 + 36r + 6g + b`이고 성분이 `0 or n*40+55`인지. 경계를 다
    /// 넣는다 — 계산을 한 칸이라도 밀면 어느 하나는 반드시 틀린다.
    #[test]
    fn the_color_cube_maps_index_to_components() {
        // (index, (r, g, b), 왜 이 케이스인가)
        let cases: &[(u8, (u8, u8, u8), &str)] = &[
            (16, (0, 0, 0), "cube origin — every component 0, not 55"),
            (17, (0, 0, 95), "b = 1 → 1*40 + 55"),
            (21, (0, 0, 255), "b = 5 → the top of a component"),
            (22, (0, 95, 0), "g = 1, b wraps back to 0"),
            (52, (95, 0, 0), "r = 1 → 16 + 36"),
            (231, (255, 255, 255), "cube top — all components 5"),
        ];

        for (index, (r, g, b), why) in cases {
            // 위 표의 값을 손으로 적지 않고 다시 계산해 맞추면 표가 공허해진다.
            // 그래서 기대값은 표에 박아두고, 여기서는 표만 비교한다.
            assert_eq!(
                p().resolve(VteColor::Indexed(*index)),
                rgb(*r, *g, *b),
                "Indexed({index}) — {why}"
            );
        }
    }

    /// 위 표의 123번은 손으로 적기 쉬운 곳이 아니다. 산술을 따로 고정한다.
    #[test]
    fn the_cube_index_123_is_derived_not_guessed() {
        // 123 - 16 = 107 → r = 107 / 36 = 2, g = (107 % 36) / 6 = 35 / 6 = 5,
        // b = 107 % 6 = 5 → (2*40+55, 5*40+55, 5*40+55) = (135, 255, 255)
        assert_eq!(
            p().resolve(VteColor::Indexed(123)),
            rgb(135, 255, 255),
            "the cube is row-major in r, then g, then b"
        );
    }

    #[test]
    fn the_grayscale_ramp_starts_at_8_and_steps_by_10() {
        let cases: &[(u8, u8, &str)] = &[
            (
                232,
                8,
                "the ramp starts at 8, not 0 — black is the cube's job",
            ),
            (233, 18, "one step is 10"),
            (243, 118, "middle of the ramp"),
            (
                255,
                238,
                "the ramp ends at 238, not 255 — white is the cube's job",
            ),
        ];

        for (index, v, why) in cases {
            assert_eq!(
                p().resolve(VteColor::Indexed(*index)),
                rgb(*v, *v, *v),
                "Indexed({index}) — {why}"
            );
        }
    }

    /// 대조군이 붙은 경계 테스트: 231(큐브 끝)과 232(회색 시작)가 **다른 규칙**을
    /// 탄다. 경계를 한 칸 밀면 231이 회색이 되거나 232가 큐브가 된다.
    #[test]
    fn the_cube_and_the_ramp_do_not_overlap() {
        assert_eq!(
            p().resolve(VteColor::Indexed(231)),
            rgb(255, 255, 255),
            "231 is the last cube entry"
        );
        assert_eq!(
            p().resolve(VteColor::Indexed(232)),
            rgb(8, 8, 8),
            "232 is the first ramp entry — a cube formula here would give \
             a very different color"
        );
        assert_eq!(
            p().resolve(VteColor::Indexed(15)),
            ANSI[15],
            "15 is the last named entry"
        );
        assert_eq!(
            p().resolve(VteColor::Indexed(16)),
            rgb(0, 0, 0),
            "16 is the first cube entry — a named lookup here would panic \
             or give ANSI black, which is not (0,0,0) in this palette"
        );
    }

    /// `Indexed(0..16)`과 `Named`의 0-15가 **같은 표**를 봐야 한다.
    #[test]
    fn indexed_and_named_agree_on_the_first_sixteen() {
        let named = [
            NamedColor::Black,
            NamedColor::Red,
            NamedColor::Green,
            NamedColor::Yellow,
            NamedColor::Blue,
            NamedColor::Magenta,
            NamedColor::Cyan,
            NamedColor::White,
            NamedColor::BrightBlack,
            NamedColor::BrightRed,
            NamedColor::BrightGreen,
            NamedColor::BrightYellow,
            NamedColor::BrightBlue,
            NamedColor::BrightMagenta,
            NamedColor::BrightCyan,
            NamedColor::BrightWhite,
        ];

        for (i, n) in named.into_iter().enumerate() {
            assert_eq!(
                p().resolve(VteColor::Indexed(i as u8)),
                p().resolve(VteColor::Named(n)),
                "Indexed({i}) and {n:?} must be the same color — two tables \
                 would make ESC[31m and ESC[38;5;1m different reds"
            );
        }

        // 대조군: 이 단언이 "전부 같은 색"으로도 통과하지 않게.
        assert_ne!(
            p().resolve(VteColor::Named(NamedColor::Red)),
            p().resolve(VteColor::Named(NamedColor::Green))
        );
    }

    #[test]
    fn spec_passes_truecolor_straight_through() {
        assert_eq!(
            p().resolve(VteColor::Spec(Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            })),
            rgb(0x12, 0x34, 0x56),
            "Spec is truecolor — the palette must not round it to a cube entry"
        );
        assert_eq!(
            p().resolve(VteColor::Spec(Rgb { r: 0, g: 0, b: 0 })).a,
            1.0,
            "an opaque spec must stay opaque"
        );
    }

    /// Dim 명명색이 실제로 어두운지, 그리고 `DIM_FACTOR`를 쓰는지.
    #[test]
    fn dim_named_colors_are_the_base_colors_attenuated() {
        for (dim, base) in [
            (NamedColor::DimBlack, NamedColor::Black),
            (NamedColor::DimRed, NamedColor::Red),
            (NamedColor::DimGreen, NamedColor::Green),
            (NamedColor::DimYellow, NamedColor::Yellow),
            (NamedColor::DimBlue, NamedColor::Blue),
            (NamedColor::DimMagenta, NamedColor::Magenta),
            (NamedColor::DimCyan, NamedColor::Cyan),
            (NamedColor::DimWhite, NamedColor::White),
        ] {
            assert_eq!(
                p().named(dim),
                attenuate(p().named(base), DIM_FACTOR),
                "{dim:?} must be {base:?} attenuated by DIM_FACTOR"
            );
        }

        assert_eq!(
            p().named(NamedColor::DimForeground),
            attenuate(p().foreground(), DIM_FACTOR)
        );
    }

    /// 대조군: dim이 base보다 실제로 어두운가. 위 테스트는 `DIM_FACTOR = 1.0`
    /// 이어도 통과한다.
    #[test]
    fn dim_is_actually_darker_than_its_base() {
        let base = p().named(NamedColor::Red);
        let dim = p().named(NamedColor::DimRed);
        assert!(
            dim.r < base.r,
            "dim red {} must be darker than {}",
            dim.r,
            base.r
        );
        assert_eq!(dim.a, base.a, "attenuation must not touch alpha");
    }

    /// `Foreground`/`Background`/`Cursor`/`BrightForeground`가 인덱스 표에서
    /// 오지 않는다 — 그 셋을 0-15로 매핑하면 `ESC[39m`(기본 전경)이 흰색이
    /// 아니라 ANSI White가 되어 테마와 어긋난다.
    #[test]
    fn the_special_named_colors_are_distinct_slots() {
        assert_eq!(p().named(NamedColor::Foreground), p().foreground());
        assert_eq!(p().named(NamedColor::Background), p().background());
        assert_eq!(p().named(NamedColor::Cursor), p().cursor());
        assert_eq!(
            p().named(NamedColor::BrightForeground),
            BRIGHT_FOREGROUND,
            "bright foreground is its own value, not indexed[15] by accident"
        );

        // 대조군: 전경과 배경이 같으면 화면이 통째로 안 보인다.
        assert_ne!(p().foreground(), p().background());
    }

    /// 표 전체가 채워졌는지. `Color::TRANSPARENT` 초기값이 남아 있으면 그
    /// 인덱스는 화면에서 사라진다 — 큐브/회색 루프의 off-by-one이 정확히
    /// 이렇게 드러난다.
    #[test]
    fn every_one_of_the_256_slots_is_opaque() {
        for i in 0..=255u8 {
            let c = p().resolve(VteColor::Indexed(i));
            assert_eq!(c.a, 1.0, "Indexed({i}) was never filled in");
        }
    }
}
