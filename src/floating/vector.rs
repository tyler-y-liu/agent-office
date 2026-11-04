//! Readable text for the window: the system's own faces, anti-aliased at the
//! display's real pixels. The 8-pixel art font is right for the office's
//! signs but leaves Chinese unreadable, so every line a person reads (task
//! labels, badges, the panel) is set here instead.

use std::collections::HashMap;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use anyhow::{Context, Result};
use unicode_width::UnicodeWidthChar;

use super::font::Canvas;

/// Latin and the panel's symbols first, monospaced so the cell grid holds;
/// then a face that has Chinese. Several of each: macOS moves its faces
/// between releases (PingFang already lives in a hashed asset folder).
const MONO: &[&str] = &[
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Monaco.ttf",
];
const CJK: &[&str] = &[
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
];

struct Glyph {
    /// Offset of the bitmap's top-left from the pen at the baseline.
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    alpha: Vec<u8>,
}

pub struct Faces {
    faces: [FontVec; 2],
    /// Rasterising a glyph costs far more than blitting it, and a frame
    /// sets the same few hundred over and over.
    cache: HashMap<(usize, char, u32), Option<Glyph>>,
    /// Cell width and ascent per size, asked for once per cell drawn.
    metrics: HashMap<u32, (u32, i32)>,
}

fn load(path: &str) -> Result<FontVec> {
    let data = std::fs::read(path).with_context(|| format!("read {path}"))?;
    FontVec::try_from_vec_and_index(data, 0).with_context(|| format!("parse {path}"))
}

/// The first of `paths` that loads, and the error naming them all if none.
fn first(paths: &[String]) -> Result<FontVec> {
    paths
        .iter()
        .find_map(|p| load(p).ok())
        .with_context(|| format!("no usable font among {paths:?}"))
}

/// PingFang, where this macOS keeps it.
fn pingfang() -> Option<String> {
    let root = std::path::Path::new("/System/Library/AssetsV2");
    for family in std::fs::read_dir(root).ok()?.flatten() {
        let Ok(assets) = std::fs::read_dir(family.path()) else {
            continue;
        };
        for asset in assets.flatten() {
            let p = asset.path().join("AssetData/PingFang.ttc");
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    None
}

impl Faces {
    pub fn load() -> Result<Self> {
        let owned = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let mut cjk = owned(CJK);
        cjk.extend(pingfang());
        Ok(Self {
            faces: [first(&owned(MONO))?, first(&cjk)?],
            cache: HashMap::new(),
            metrics: HashMap::new(),
        })
    }

    fn metrics(&mut self, px: u32) -> (u32, i32) {
        let mono = &self.faces[0];
        *self.metrics.entry(px).or_insert_with(|| {
            let f = mono.as_scaled(PxScale::from(px as f32));
            let cell = f.h_advance(f.glyph_id('0')).round().max(1.0) as u32;
            (cell, f.ascent().round() as i32)
        })
    }

    /// A narrow cell's width at `px`: the monospace face's advance.
    pub fn cell_w(&mut self, px: u32) -> u32 {
        self.metrics(px).0
    }

    /// Pixels from a line's top to its baseline at `px`.
    pub fn ascent(&mut self, px: u32) -> i32 {
        self.metrics(px).1
    }

    fn face_for(&self, c: char) -> usize {
        let narrow = c.width() == Some(1);
        if narrow && self.faces[0].glyph_id(c).0 != 0 {
            0
        } else {
            1
        }
    }

    fn glyph(&mut self, c: char, px: u32) -> Option<&Glyph> {
        let face = self.face_for(c);
        let faces = &self.faces;
        self.cache
            .entry((face, c, px))
            .or_insert_with(|| {
                let font = &faces[face];
                let g = font
                    .glyph_id(c)
                    .with_scale_and_position(PxScale::from(px as f32), ab_glyph::point(0.0, 0.0));
                let outlined = font.outline_glyph(g)?;
                let b = outlined.px_bounds();
                let (w, h) = (b.width() as u32, b.height() as u32);
                let mut alpha = vec![0u8; (w * h) as usize];
                outlined.draw(|x, y, v| {
                    if x < w && y < h {
                        alpha[(y * w + x) as usize] = (v * 255.0).round() as u8;
                    }
                });
                Some(Glyph {
                    left: b.min.x as i32,
                    top: b.min.y as i32,
                    w,
                    h,
                    alpha,
                })
            })
            .as_ref()
    }

    /// Cells `text` takes, the grid's unit for width.
    pub fn cells(text: &str) -> u32 {
        text.chars().map(|c| c.width().unwrap_or(0) as u32).sum()
    }

    /// Set `text` on the cell grid from `x`, its baseline at `baseline`: each
    /// character in its own cells, a wide one centred across its two.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        x: i64,
        baseline: i64,
        text: &str,
        color: u32,
        px: u32,
    ) {
        let cell = i64::from(self.cell_w(px));
        let mut pen = x;
        for c in text.chars() {
            let cells = i64::from(c.width().unwrap_or(0) as u32);
            if cells == 0 {
                continue;
            }
            if !c.is_whitespace() {
                let span = cells * cell;
                let ascent = i64::from(self.ascent(px));
                match self.glyph(c, px) {
                    Some(g) => {
                        let gx = if cells == 1 {
                            pen + i64::from(g.left)
                        } else {
                            pen + (span - i64::from(g.w)) / 2
                        };
                        blend(canvas, gx, baseline + i64::from(g.top), g, color);
                    }
                    // Neither face has it (an emoji, a rare symbol): a box,
                    // so the reader sees something is there.
                    None => {
                        let (x0, y0) = (pen + 1, baseline - ascent + 2);
                        let (w, h) = ((span - 2).max(1) as u32, (ascent - 2).max(1) as u32);
                        canvas.fill(x0, y0, w, 1, color);
                        canvas.fill(x0, y0 + i64::from(h) - 1, w, 1, color);
                        canvas.fill(x0, y0, 1, h, color);
                        canvas.fill(x0 + i64::from(w) - 1, y0, 1, h, color);
                    }
                }
            }
            pen += cells * cell;
        }
    }
}

fn blend(canvas: &mut Canvas, x: i64, y: i64, g: &Glyph, color: u32) {
    let (cr, cg, cb) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
    for gy in 0..g.h {
        let py = y + i64::from(gy);
        if py < 0 || py >= i64::from(canvas.h) {
            continue;
        }
        for gx in 0..g.w {
            let px = x + i64::from(gx);
            if px < 0 || px >= i64::from(canvas.w) {
                continue;
            }
            let a = u32::from(g.alpha[(gy * g.w + gx) as usize]);
            if a == 0 {
                continue;
            }
            let i = (py as u32 * canvas.w + px as u32) as usize;
            let d = canvas.px[i];
            let mix = |s: u32, d: u32| (s * a + d * (255 - a)) / 255;
            canvas.px[i] = (mix(cr, (d >> 16) & 0xff) << 16)
                | (mix(cg, (d >> 8) & 0xff) << 8)
                | mix(cb, d & 0xff);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_and_latin_both_ink_and_a_wide_char_takes_two_cells() {
        let Ok(mut faces) = Faces::load() else {
            // Not macOS, or the system faces moved: nothing to check here.
            return;
        };
        let mut c = Canvas::new(200, 40, 0);
        faces.draw(&mut c, 2, 30, "任务 ok", 0x00ff_ffff, 24);
        assert!(
            c.px.iter().filter(|p| **p != 0).count() > 100,
            "glyphs inked"
        );
        assert_eq!(Faces::cells("任务 ok"), 7);
        for ch in "●▸└│↳…↑↓".chars() {
            assert!(faces.glyph(ch, 24).is_some(), "{ch:?} has a glyph");
        }
    }
}
