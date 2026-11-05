//! agent-office: what a working agent's screen shows, set by the host from the
//! phase its panel card names, so a desk reads as what its sitter is at
//! without its label: scrolling code, a test run's progress bar and its green
//! flash, a browser page, a terminal's cursor. A screen with no show set keeps
//! pixtuoid's own lit glass and scanline.

use std::collections::HashMap;
use std::sync::RwLock;

use pixtuoid_core::AgentId;
use pixtuoid_core::sprite::{Frame, Rgb};

/// What a screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeskShow {
    /// Writing code: green lines scrolling up.
    Code,
    /// Running the tests: a bar filling.
    Test,
    /// The tests just came back green: the glass flashes green.
    Passed,
    /// Reading up: a browser page.
    Browse,
    /// Running a command: a terminal, its cursor blinking.
    Terminal,
}

static SHOWS: RwLock<Option<HashMap<AgentId, DeskShow>>> = RwLock::new(None);

/// Show `shows` on their agents' screens from the next frame on; every other
/// screen goes back to its plain lit glass.
pub fn set_desk_shows(shows: HashMap<AgentId, DeskShow>) {
    if let Ok(mut s) = SHOWS.write() {
        *s = Some(shows);
    }
}

/// What `agent`'s screen shows, if the host set it.
pub(crate) fn desk_show(agent: AgentId) -> Option<DeskShow> {
    SHOWS.read().ok()?.as_ref()?.get(&agent).copied()
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// How long one step of a show's animation holds.
pub(crate) const STEP_MS: u64 = 250;
/// The steps a show cycles through, so a screen's art is cached once per step.
pub(crate) const STEPS: u8 = 12;

/// The step a show is on at `ms`, a desk at column `x` offset from its
/// neighbours so a bench of coders doesn't scroll in lockstep.
pub(crate) fn step_at(ms: u64, x: u16) -> u8 {
    ((ms / STEP_MS + u64::from(x) * 5) % u64::from(STEPS)) as u8
}

/// The shortest glass, in art pixels, a show is drawn on: below it the classic
/// art's two-row screen keeps its scanline.
const MIN_GLASS_H: u16 = 6;

const CODE_BG: Rgb = rgb(0x0e, 0x22, 0x18);
const CODE_INK: Rgb = rgb(0x4c, 0xd8, 0x6c);
const CODE_KEYWORD: Rgb = rgb(0xb8, 0xf5, 0xc4);
const TEST_BG: Rgb = rgb(0x18, 0x20, 0x30);
const TEST_TRACK: Rgb = rgb(0x3a, 0x44, 0x58);
const TEST_FILL: Rgb = rgb(0xf2, 0xc1, 0x4e);
const TEST_DOT: Rgb = rgb(0x8a, 0x94, 0xa8);
const PASS_BRIGHT: Rgb = rgb(0x5c, 0xe0, 0x6a);
const PASS_DEEP: Rgb = rgb(0x23, 0x8a, 0x3a);
const PAGE_BG: Rgb = rgb(0xe8, 0xec, 0xf0);
const PAGE_BAR: Rgb = rgb(0x9a, 0xa3, 0xad);
const PAGE_LINK: Rgb = rgb(0x3d, 0x7b, 0xe0);
const PAGE_TEXT: Rgb = rgb(0x8a, 0x92, 0x9c);
const PAGE_IMAGE: Rgb = rgb(0xf0, 0xa8, 0x5c);
const TERM_BG: Rgb = rgb(0x0f, 0x11, 0x14);
const TERM_TEXT: Rgb = rgb(0x9a, 0xc8, 0x9e);
const TERM_CURSOR: Rgb = rgb(0xe6, 0xe6, 0xe6);

/// A line of code's length and indent, in glass pixels, for source line `n`:
/// a fixed-looking file, the same lines coming round as it scrolls.
fn code_line(n: u16, w: u16) -> (u16, u16) {
    let h = u32::from(n).wrapping_mul(2_654_435_761) >> 7;
    let indent = [0, 2, 4, 2][(h % 4) as usize];
    let len = (w / 3 + (h >> 3) as u16 % (w / 2).max(1)).min(w.saturating_sub(indent + 1));
    (indent, len)
}

/// `lit` with `show` drawn on its glass, the pixels where `glass` says the
/// screen keys drew; `None` where the glass is too small to show anything.
pub(crate) fn paint(
    lit: &Frame,
    glass: &dyn Fn(u16, u16) -> bool,
    show: DeskShow,
    step: u8,
) -> Option<Frame> {
    let (w, h) = (lit.width(), lit.height());
    let mut bbox: Option<(u16, u16, u16, u16)> = None;
    for y in 0..h {
        for x in 0..w {
            if glass(x, y) {
                let (x0, y0, x1, y1) = bbox.unwrap_or((x, y, x, y));
                bbox = Some((x0.min(x), y0.min(y), x1.max(x), y1.max(y)));
            }
        }
    }
    let (x0, y0, x1, y1) = bbox?;
    let (gw, gh) = (x1 - x0 + 1, y1 - y0 + 1);
    if gh < MIN_GLASS_H {
        return None;
    }
    let step16 = u16::from(step);
    // The colour of glass pixel (gx, gy), counted from the glass's top-left.
    let ink = |gx: u16, gy: u16| -> Rgb {
        match show {
            DeskShow::Code => {
                // Every other row a line, scrolling up one line each step.
                if gy % 2 == 1 {
                    return CODE_BG;
                }
                let (indent, len) = code_line(gy / 2 + step16, gw);
                if (1 + indent..1 + indent + len).contains(&gx) {
                    // A keyword at the start of the line.
                    if gx < 1 + indent + 3 {
                        CODE_KEYWORD
                    } else {
                        CODE_INK
                    }
                } else {
                    CODE_BG
                }
            }
            DeskShow::Test => {
                // The bar near the top, where a sitter's head never covers it.
                let (bar_top, bar_h) = (1, 3);
                if (bar_top..bar_top + bar_h).contains(&gy) && (1..gw - 1).contains(&gx) {
                    let filled = 1 + (gw - 2) * (step16 + 1) / u16::from(STEPS);
                    let edge =
                        gy == bar_top || gy == bar_top + bar_h - 1 || gx == 1 || gx == gw - 2;
                    return if gx < filled && !edge {
                        TEST_FILL
                    } else if edge {
                        TEST_TRACK
                    } else {
                        TEST_BG
                    };
                }
                // Test names ticking off under it, a dot each.
                let row = gy.saturating_sub(bar_top + bar_h + 1);
                if gy > bar_top + bar_h && row % 2 == 0 && gx % 3 == 1 && gx < gw - 1 {
                    let done = (row / 2) * (gw / 3) + gx / 3 < step16 * 2;
                    return if done { PASS_BRIGHT } else { TEST_DOT };
                }
                TEST_BG
            }
            DeskShow::Passed => {
                // Flashing between two greens, a tick drawn in the brighter.
                let (on, off) = if step % 2 == 0 {
                    (PASS_BRIGHT, PASS_DEEP)
                } else {
                    (PASS_DEEP, PASS_BRIGHT)
                };
                let (cx, cy) = (gw / 2, gh / 3 + 2);
                let tick = (gx + 2 == cx && gy + 1 == cy)
                    || (gx + 1 == cx && gy == cy)
                    || (gx >= cx && gx - cx < 4 && gy + (gx - cx) + 1 == cy);
                if tick { off } else { on }
            }
            DeskShow::Browse => {
                if gy < 2 {
                    // The address bar, a tab on its left.
                    return if gy == 0 && gx < gw / 3 {
                        PAGE_LINK
                    } else {
                        PAGE_BAR
                    };
                }
                let body = gy - 3;
                if gy == 2 {
                    return PAGE_BG;
                }
                if body == 0 && (2..gw * 2 / 3).contains(&gx) {
                    return PAGE_LINK;
                }
                // An image on the right, text beside it.
                if (gw * 2 / 3 + 1..gw - 1).contains(&gx) && (2..6).contains(&body) {
                    return PAGE_IMAGE;
                }
                if body >= 2 && body % 2 == 0 && (2..gw * 2 / 3 - 1 - (body % 3) * 2).contains(&gx)
                {
                    return PAGE_TEXT;
                }
                PAGE_BG
            }
            DeskShow::Terminal => {
                // Two lines of output, then the prompt with its cursor.
                let line = gy / 2;
                if gy % 2 == 1 {
                    return TERM_BG;
                }
                let len = match line {
                    0 => gw * 2 / 3,
                    1 => gw / 2,
                    2 => 3,
                    _ => 0,
                };
                if (1..1 + len).contains(&gx) {
                    return TERM_TEXT;
                }
                let blink = step % 4 < 2;
                if line == 2 && blink && (5..7).contains(&gx) {
                    return TERM_CURSOR;
                }
                TERM_BG
            }
        }
    };
    let pixels = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            if glass(x, y) {
                Some(ink(x - x0, y - y0))
            } else {
                lit.get(x, y).copied().flatten()
            }
        })
        .collect();
    Some(Frame::from_pixels(w, h, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glass_frame(w: u16, h: u16) -> Frame {
        Frame::from_pixels(
            w,
            h,
            vec![Some(rgb(1, 2, 3)); usize::from(w) * usize::from(h)],
        )
    }

    fn colours(f: &Frame) -> std::collections::HashSet<Rgb> {
        (0..f.height())
            .flat_map(|y| (0..f.width()).map(move |x| (x, y)))
            .filter_map(|(x, y)| f.get(x, y).copied().flatten())
            .collect()
    }

    /// The point of a show is telling the phases apart at a glance: no two
    /// draw the same picture on a bench's glass.
    #[test]
    fn every_show_draws_its_own_picture() {
        let lit = glass_frame(28, 12);
        let all = [
            DeskShow::Code,
            DeskShow::Test,
            DeskShow::Passed,
            DeskShow::Browse,
            DeskShow::Terminal,
        ];
        let drawn: Vec<Frame> = all
            .iter()
            .map(|&s| paint(&lit, &|_, _| true, s, 3).expect("drawn"))
            .collect();
        for i in 0..drawn.len() {
            for j in i + 1..drawn.len() {
                assert_ne!(
                    colours(&drawn[i]),
                    colours(&drawn[j]),
                    "{:?} and {:?} look alike",
                    all[i],
                    all[j]
                );
            }
        }
    }

    /// A show draws only on the glass: the monitor's casing and the desk
    /// round it keep their art.
    #[test]
    fn a_show_keeps_off_everything_but_the_glass() {
        let lit = glass_frame(10, 10);
        let glass = |x: u16, y: u16| (2..8).contains(&x) && (1..9).contains(&y);
        let f = paint(&lit, &glass, DeskShow::Browse, 0).expect("drawn");
        for (x, y) in [(0, 0), (1, 5), (9, 9), (5, 0), (8, 3)] {
            assert_eq!(f.get(x, y).copied().flatten(), Some(rgb(1, 2, 3)));
        }
    }

    /// Code scrolls and a test bar fills, so a screen reads as live work
    /// rather than a still picture.
    #[test]
    fn code_scrolls_and_the_test_bar_fills() {
        let lit = glass_frame(28, 12);
        let all = |_, _| true;
        let px = |step| {
            let f = paint(&lit, &all, DeskShow::Code, step).expect("drawn");
            (0..12)
                .flat_map(|y| (0..28).map(move |x| (x, y)))
                .map(|(x, y)| f.get(x, y).copied().flatten())
                .collect::<Vec<_>>()
        };
        assert_ne!(px(0), px(1));
        let fill = |step| {
            let f = paint(&lit, &all, DeskShow::Test, step).expect("drawn");
            (0..28)
                .filter(|&x| f.get(x, 2).copied().flatten() == Some(TEST_FILL))
                .count()
        };
        assert!(fill(STEPS - 1) > fill(0));
    }

    /// The classic art's two-row glass is too small for a picture: it keeps
    /// pixtuoid's scanline.
    #[test]
    fn a_tiny_glass_shows_nothing() {
        assert!(paint(&glass_frame(6, 2), &|_, _| true, DeskShow::Code, 0).is_none());
    }
}
