//! Fusion Pixel 8px (OFL, `assets/`), the face pixtuoid's cutaway sets its
//! world text in, painted one glyph pixel per art pixel so the window's text
//! matches the badges. The file is the one pixtuoid ships: three header
//! bytes (line height, glyph count), the code points, then eight rows a glyph,
//! the high bit leftmost.

use std::sync::OnceLock;

use unicode_width::UnicodeWidthChar;

static FUSION: &[u8] = include_bytes!("../../assets/fusion-pixel-8px.bin");

/// Art pixels a line is tall: the font's box.
pub const LINE_H: u32 = 8;
/// Art pixels a cell is wide: a narrow glyph and its gap.
pub const ADVANCE: u32 = 4;

type Rows = [u8; LINE_H as usize];

fn font() -> &'static (Vec<u16>, &'static [u8]) {
    static FONT: OnceLock<(Vec<u16>, &'static [u8])> = OnceLock::new();
    FONT.get_or_init(|| {
        let n = usize::from(u16::from_le_bytes([FUSION[1], FUSION[2]]));
        let points = FUSION[3..3 + 2 * n]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        (points, &FUSION[3 + 2 * n..])
    })
}

fn fusion(c: char) -> Option<Rows> {
    let (points, glyphs) = font();
    let i = points
        .binary_search(&u16::try_from(u32::from(c)).ok()?)
        .ok()?;
    glyphs.get(i * 8..i * 8 + 8)?.try_into().ok()
}

/// The symbols the panel uses that Fusion Pixel has no glyph for, drawn
/// here on the same 3-pixel cell (`by_hand` in pixtuoid's `cutaway/text.rs`
/// is the model; `…` is its).
fn by_hand(c: char) -> Option<Rows> {
    let rows: [u8; 8] = match c {
        '●' => [0, 0, 0b010, 0b111, 0b111, 0b010, 0, 0],
        '▸' => [0, 0b100, 0b110, 0b111, 0b110, 0b100, 0, 0],
        '…' => [0, 0, 0, 0, 0, 0, 0b101, 0],
        '↳' => [0, 0b100, 0b100, 0b101, 0b111, 0b001, 0, 0],
        '↑' => [0, 0b010, 0b111, 0b010, 0b010, 0b010, 0b010, 0],
        // The icons pixtuoid's board, bubbles and floor indicator carry
        // (`Icon::terminal` in pixtuoid-scene's `display/icon.rs`).
        '▲' => [0, 0, 0b010, 0b010, 0b111, 0b111, 0, 0],
        '△' => [0, 0, 0b010, 0b010, 0b101, 0b111, 0, 0],
        '▼' => [0, 0, 0b111, 0b111, 0b010, 0b010, 0, 0],
        '▽' => [0, 0, 0b111, 0b101, 0b010, 0b010, 0, 0],
        '▾' => [0, 0, 0, 0b111, 0b010, 0, 0, 0],
        '○' => [0, 0, 0b010, 0b101, 0b101, 0b010, 0, 0],
        '◌' => [0, 0, 0b010, 0b001, 0b100, 0b010, 0, 0],
        '◐' => [0, 0, 0b010, 0b111, 0b101, 0b010, 0, 0],
        '★' => [0, 0b010, 0b111, 0b010, 0b101, 0, 0, 0],
        '✓' => [0, 0, 0b001, 0b001, 0b101, 0b010, 0, 0],
        '⚠' => [0, 0b010, 0b010, 0b111, 0b101, 0b111, 0, 0],
        '☕' => [0, 0b010, 0, 0b111, 0b111, 0b110, 0, 0],
        '←' => [0, 0, 0b010, 0b111, 0b010, 0, 0, 0],
        '→' => [0, 0, 0b010, 0b111, 0b010, 0, 0, 0],
        '⋮' => [0, 0b010, 0, 0b010, 0, 0b010, 0, 0],
        '↓' => [0, 0b010, 0b010, 0b010, 0b010, 0b111, 0b010, 0],
        // Box lines fill the whole 4-pixel cell so neighbours join.
        '│' => return Some([0b0100_0000; 8]),
        '└' => {
            return Some([
                0b0100_0000,
                0b0100_0000,
                0b0100_0000,
                0b0100_0000,
                0b0111_1000,
                0,
                0,
                0,
            ]);
        }
        _ => return None,
    };
    Some(rows.map(|r| r << 5))
}

pub fn glyph(c: char) -> Option<Rows> {
    by_hand(c).or_else(|| fusion(c))
}

/// Art pixels `text` advances: four a narrow cell, eight a wide one.
pub fn advance(text: &str) -> u32 {
    text.chars()
        .map(|c| c.width().unwrap_or(0) as u32 * ADVANCE)
        .sum()
}

/// Art pixels from `text`'s first column to its last ink: the advance less
/// the trailing gap.
pub fn width(text: &str) -> u32 {
    advance(text).saturating_sub(1)
}

/// An `0x00RRGGBB` pixel buffer, the format the window takes.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

impl Canvas {
    pub fn new(w: u32, h: u32, fill: u32) -> Self {
        Self {
            w,
            h,
            px: vec![fill; (w * h) as usize],
        }
    }

    /// Make it `w`×`h` and all `fill`, keeping its allocation when it fits.
    pub fn reset(&mut self, w: u32, h: u32, fill: u32) {
        self.w = w;
        self.h = h;
        self.px.clear();
        self.px.resize((w * h) as usize, fill);
    }

    /// `w`×`h` at (`x`, `y`), clipped.
    pub fn fill(&mut self, x: i64, y: i64, w: u32, h: u32, color: u32) {
        let x0 = x.clamp(0, i64::from(self.w)) as u32;
        let y0 = y.clamp(0, i64::from(self.h)) as u32;
        let x1 = (x + i64::from(w)).clamp(0, i64::from(self.w)) as u32;
        let y1 = (y + i64::from(h)).clamp(0, i64::from(self.h)) as u32;
        for row in y0..y1 {
            let at = (row * self.w) as usize;
            self.px[at + x0 as usize..at + x1 as usize].fill(color);
        }
    }

    /// `text` with its top-left at (`x`, `y`), each glyph pixel `scale`
    /// pixels square. A character the font lacks is a box, so a line never
    /// collapses.
    pub fn text(&mut self, x: i64, y: i64, text: &str, color: u32, scale: u32) {
        self.text_or(x, y, text, color, scale, true);
    }

    /// An icon's glyph: nothing where the font lacks it, since a box reads
    /// as a broken lamp rather than a missing letter.
    pub fn icon(&mut self, x: i64, y: i64, text: &str, color: u32) {
        self.text_or(x, y, text, color, 1, false);
    }

    fn text_or(&mut self, x: i64, y: i64, text: &str, color: u32, scale: u32, tofu: bool) {
        let s = i64::from(scale);
        let mut cx = x;
        for c in text.chars() {
            let cells = c.width().unwrap_or(0) as u32;
            if cells == 0 {
                continue;
            }
            match glyph(c) {
                Some(rows) => {
                    for (r, bits) in rows.iter().enumerate() {
                        for k in 0..8i64 {
                            if bits & (0x80 >> k) != 0 {
                                self.fill(cx + k * s, y + r as i64 * s, scale, scale, color);
                            }
                        }
                    }
                }
                None if !tofu => {}
                None => {
                    let w = cells * ADVANCE - 1;
                    self.fill(cx, y + s, w * scale, 6 * scale, color);
                }
            }
            cx += i64::from(cells * ADVANCE) * s;
        }
    }

    /// Copy `src` into this canvas at (`x`, `y`), each pixel `k` pixels
    /// square: the nearest-neighbour upscale pixel art needs.
    pub fn blit_scaled(&mut self, src: &Canvas, x: u32, y: u32, k: u32) {
        let k = k.max(1);
        let cols = src.w.min(self.w.saturating_sub(x) / k);
        let rows = src.h.min(self.h.saturating_sub(y) / k);
        let mut line = vec![0u32; (cols * k) as usize];
        for sy in 0..rows {
            let srow = &src.px[(sy * src.w) as usize..(sy * src.w + cols) as usize];
            for (i, &p) in srow.iter().enumerate() {
                line[i * k as usize..(i + 1) * k as usize].fill(p);
            }
            for dy in 0..k {
                let at = ((y + sy * k + dy) * self.w + x) as usize;
                self.px[at..at + line.len()].copy_from_slice(&line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_face_draws_chinese_and_the_panel_symbols() {
        assert!(glyph('任').is_some_and(|g| g.iter().any(|r| *r != 0)));
        assert!(glyph('A').is_some());
        for c in "●▸…↳↑↓│└（）：▲▼○◐★☕".chars() {
            assert!(glyph(c).is_some(), "{c:?}");
        }
        assert_eq!(advance("任务"), 16);
        assert_eq!(width("ab"), 7);
    }

    #[test]
    fn upscale_repeats_each_pixel_into_a_square() {
        let mut src = Canvas::new(2, 1, 0);
        src.px = vec![1, 2];
        let mut dst = Canvas::new(5, 3, 9);
        dst.blit_scaled(&src, 0, 0, 2);
        assert_eq!(&dst.px[0..5], &[1, 1, 2, 2, 9]);
        assert_eq!(&dst.px[5..10], &[1, 1, 2, 2, 9]);
        assert_eq!(&dst.px[10..15], &[9; 5], "past the scaled rows untouched");
    }
}
