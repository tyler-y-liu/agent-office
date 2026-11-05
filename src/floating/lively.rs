//! The office's goings-on, painted over the upscaled frame in window pixels:
//! cheers, overtime suppers, stuck moods and the cat that sits with the worst
//! off, the boss at the door, chitchat in Chinese, festival dress, the
//! typhoon signal and the late-night lights. What happens is decided in
//! [`crate::fun`]; this only draws it, beside each figure from its [`Tag`].

use std::time::{Duration, SystemTime};

use pixtuoid_core::AgentId;
use pixtuoid_scene::layout::{CHARACTER_SPRITE_H, CHARACTER_SPRITE_W};

use super::vector::Faces;
use super::{Area, Canvas, DoorSign, Tag, pt_px, tag_pad};
use crate::fun::{Festival, Mood};
use crate::tasks::Cheer;

/// Where the office stands in the window and how big its pixels are.
#[derive(Clone, Copy)]
pub(super) struct Stage {
    /// Window pixels to an art pixel.
    pub u: i64,
    pub scale_factor: f64,
    /// The office's width and height in window pixels.
    pub w: u32,
    pub h: u32,
    /// Where the floor starts under the north wall's windows, in window pixels.
    pub floor_top: i64,
    /// Milliseconds on the wall clock, for the animations.
    pub ms: u64,
}

/// A small sprite's colours, by the letter its rows draw with.
fn ink(c: char) -> Option<u32> {
    Some(match c {
        'w' => 0x00f4_f1e8,
        'r' => 0x00d6_403a,
        'R' => 0x008f_1f1d,
        'y' => 0x00f2_c14e,
        'o' => 0x00f0_8a24,
        'b' => 0x007a_4a2a,
        'B' => 0x00b0_7840,
        'k' => 0x0020_2020,
        'g' => 0x00c8_ccd2,
        'G' => 0x006a_7078,
        'c' => 0x008f_d3ff,
        'n' => 0x004c_af50,
        'N' => 0x002e_7d32,
        'p' => 0x00f1_c27d,
        'h' => 0x002b_2b2b,
        'e' => 0x00e0_8a3c,
        'E' => 0x00a8_5a26,
        's' => 0x00e8_ecf0,
        't' => 0x00c0_392b,
        _ => return None,
    })
}

/// Draw `rows` with its top-left at (`x`, `y`), each letter `u` window pixels square.
fn sprite(canvas: &mut Canvas, x: i64, y: i64, u: i64, rows: &[&str]) {
    for (dy, row) in rows.iter().enumerate() {
        for (dx, c) in row.chars().enumerate() {
            if let Some(col) = ink(c) {
                canvas.fill(
                    x + dx as i64 * u,
                    y + dy as i64 * u,
                    u as u32,
                    u as u32,
                    col,
                );
            }
        }
    }
}

fn sprite_w(rows: &[&str]) -> i64 {
    rows.first().map_or(0, |r| r.chars().count() as i64)
}

const NOODLES: &[&str] = &["wwwwww", "wrrrrw", ".wwww.", ".wrrw.", ".wwww."];
const MILK_TEA: &[&str] = &["g..g", "gBBg", "gBBg", "gBBg", ".gg."];
const BRIEFCASE: &[&str] = &["..bb..", ".b..b.", "bbbbbb", "bbybbb", "bbbbbb"];
const CLOUD: &[&str] = &["...GGG....", ".GGGGGGG..", "GGGGGGGGGG", ".GGGGGGGG."];
const SWEAT: &[&str] = &[".c", "cc", "cc"];
const CAT: &[&str] = &[
    "e.e.....", "eee.....", "kek.....", "eee...e.", ".eEe..e.", ".eeeeee.", ".eeeee..", ".e.e....",
];
const CAT_TAIL_UP: &[&str] = &["......e.", "......e."];
const HANDS_OPEN: &[&str] = &["y...y", "p...p", "p...p"];
const HANDS_SHUT: &[&str] = &["..y..", ".p.p.", ".ppp."];
const BOSS: &[&str] = &[
    ".hhhhhh.", "hhhhhhhh", "hpppppph", "pkkpkkpp", "pppppppp", ".ppkkpp.", "sssttsss", "ssstttss",
];
const LANTERN: &[&str] = &["..y..", ".rrr.", "rrorr", "rrorr", ".rrr.", "..y.."];
const TREE: &[&str] = &[
    "...y...", "...n...", "..nnn..", "..nrn..", ".nnnnn.", ".nynnr.", "nnnnnnn", "...b...",
    "...b...",
];

/// A figure's centre column and top row, in window pixels.
fn figure(t: &Tag) -> (i64, i64) {
    (t.badge.x + i64::from(t.badge.w / 2), t.figure_top)
}

fn figure_w(u: i64) -> i64 {
    i64::from(CHARACTER_SPRITE_W) * u
}

fn figure_h(u: i64) -> i64 {
    i64::from(CHARACTER_SPRITE_H) * u
}

/// Late at night only the overtime desks keep a light on: the floor goes
/// dark round a pool of light at each of `lit`. The windows' band is left
/// alone, so the harbour still shines.
pub(super) fn night_lights(canvas: &mut Canvas, s: Stage, tags: &[Tag], lit: &[AgentId]) {
    let pools: Vec<(f64, f64, f64, f64)> = tags
        .iter()
        .filter(|t| lit.contains(&t.agent))
        .map(|t| {
            let (cx, top) = figure(t);
            (
                cx as f64,
                (top + figure_h(s.u) * 3 / 5) as f64,
                (figure_w(s.u) * 3) as f64,
                (figure_h(s.u) * 8 / 5) as f64,
            )
        })
        .collect();
    let (w, h) = (s.w.min(canvas.w), s.h.min(canvas.h));
    let top = s.floor_top.clamp(0, i64::from(h)) as u32;
    // Most of the floor is outside every pool and fully dark: those pixels
    // take a table, the same as `dim(px, 1.0)`, instead of the sums below.
    let full: [[u32; 256]; 3] = std::array::from_fn(|i| {
        let shift = [16, 8, 0][i];
        std::array::from_fn(|c| dim((c as u32) << shift, 1.0))
    });
    let mut near = Vec::with_capacity(pools.len());
    for y in top..h {
        let row = (y * canvas.w) as usize;
        // The pools whose box this row crosses; past them, reach >= 1.
        near.clear();
        near.extend(
            pools
                .iter()
                .filter(|&&(_, cy, _, ry)| (f64::from(y) - cy).abs() < ry)
                .map(|&(cx, _, rx, _)| (cx, rx)),
        );
        for x in 0..w {
            if !near.iter().any(|&(cx, rx)| (f64::from(x) - cx).abs() < rx) {
                let p = &mut canvas.px[row + x as usize];
                *p = full[0][((*p >> 16) & 0xff) as usize]
                    | full[1][((*p >> 8) & 0xff) as usize]
                    | full[2][(*p & 0xff) as usize];
                continue;
            }
            // How deep in the nearest pool: 0 at its heart, 1 at its rim.
            let reach = pools
                .iter()
                .map(|&(cx, cy, rx, ry)| {
                    let (dx, dy) = ((f64::from(x) - cx) / rx, (f64::from(y) - cy) / ry);
                    dx * dx + dy * dy
                })
                .fold(f64::MAX, f64::min);
            let dark = ((reach - 0.55) / 0.45).clamp(0.0, 1.0);
            if dark > 0.0 {
                let p = &mut canvas.px[row + x as usize];
                *p = dim(*p, dark);
            }
        }
    }
}

/// `px` dimmed toward a cool dark by `amount` (0..1).
fn dim(px: u32, amount: f64) -> u32 {
    let ch = |shift: u32, keep: f64| {
        let c = f64::from((px >> shift) & 0xff);
        let k = 1.0 - amount * (1.0 - keep);
        ((c * k).round() as u32).min(255) << shift
    };
    ch(16, 0.38) | ch(8, 0.42) | ch(0, 0.55)
}

/// A cup noodle and a glass of milk tea by an overtime figure, its noodles
/// steaming.
pub(super) fn overtime_supper(canvas: &mut Canvas, s: Stage, t: &Tag) {
    let u = s.u;
    let (cx, top) = figure(t);
    let base = top + figure_h(u) * 3 / 4;
    let x = cx + figure_w(u) * 2 / 3;
    let y = base - 5 * u;
    sprite(canvas, x, y, u, NOODLES);
    sprite(canvas, x + 7 * u, y, u, MILK_TEA);
    // Two wisps of steam rising off the noodles and fading.
    for wisp in 0..2u64 {
        let phase = (s.ms / 220 + wisp * 2) % 4;
        let sx = x + (1 + 3 * wisp as i64) * u + if phase.is_multiple_of(2) { 0 } else { u };
        let sy = y - (1 + phase as i64) * u;
        canvas.fill(sx, sy, u as u32, u as u32, 0x00dd_e2e8);
    }
}

/// A briefcase by a figure packing up for the typhoon.
pub(super) fn packing_up(canvas: &mut Canvas, s: Stage, t: &Tag) {
    let u = s.u;
    let (cx, top) = figure(t);
    sprite(
        canvas,
        cx - figure_w(u) * 2 / 3 - sprite_w(BRIEFCASE) * u,
        top + figure_h(u) - 5 * u,
        u,
        BRIEFCASE,
    );
}

/// How a stuck figure takes it: sweating as it fails, trailing dots as it
/// sits quiet, under its own rain cloud when it has been stuck long.
pub(super) fn mood(canvas: &mut Canvas, s: Stage, t: &Tag, mood: Mood) {
    let u = s.u;
    let (cx, top) = figure(t);
    match mood {
        Mood::Frustrated => {
            let bob = ((s.ms / 300) % 2) as i64 * u;
            sprite(canvas, cx + figure_w(u) / 2, top + 2 * u + bob, u, SWEAT);
            sprite(
                canvas,
                cx - figure_w(u) / 2 - 2 * u,
                top + 3 * u - bob,
                u,
                SWEAT,
            );
        }
        Mood::Slumped => {
            let shown = (s.ms / 500) % 4;
            for i in 0..shown as i64 {
                canvas.fill(
                    cx + figure_w(u) / 2 + i * 2 * u,
                    top + 2 * u,
                    u as u32,
                    u as u32,
                    0x00c8_ccd2,
                );
            }
        }
        Mood::Gloomy => {
            let cw = sprite_w(CLOUD) * u;
            // Beside its badge, so the name still reads; it rains to its head.
            let x = t.badge.x - cw - u;
            let y = t.badge.y;
            sprite(canvas, x, y, u, CLOUD);
            let fall = (s.ms / 120) as i64;
            for (i, col) in [1i64, 4, 7].iter().enumerate() {
                let drop = (fall + i as i64 * 3) % 6;
                canvas.fill(
                    x + col * u,
                    y + (5 + drop) * u,
                    u as u32,
                    (2 * u) as u32,
                    0x008f_d3ff,
                );
            }
        }
    }
}

/// The shop cat sitting by a stuck figure's feet, its tail flicking; where
/// it sits, for a click.
pub(super) fn cat_by(canvas: &mut Canvas, s: Stage, t: &Tag) -> Area {
    let u = s.u;
    let (cx, top) = figure(t);
    let x = cx + figure_w(u) / 2;
    let y = top + figure_h(u) - CAT.len() as i64 * u;
    sprite(canvas, x, y, u, CAT);
    if (s.ms / 700).is_multiple_of(3) {
        sprite(canvas, x, y + 2 * u, u, CAT_TAIL_UP);
    }
    Area {
        x,
        y,
        w: (sprite_w(CAT) * u) as u32,
        h: (CAT.len() as i64 * u) as u32,
    }
}

const ARM: &[&str] = &["p", "p", "p", "B"];

/// An idle figure stretching, `ago` into it: both arms up over its head and
/// pumping, with a yawn's 〜 beside it.
pub(super) fn stretch(canvas: &mut Canvas, s: Stage, t: &Tag, ago: Duration) {
    let u = s.u;
    let (cx, top) = figure(t);
    // Up in the first quarter second, then two slow pumps.
    let reach =
        ((ago.as_millis() / 250).min(1) as i64 + i64::from((ago.as_millis() / 400) % 2 == 1)) * u;
    let half = figure_w(u) / 3;
    let y = top + figure_h(u) / 4 - reach;
    for x in [cx - half - u, cx + half] {
        sprite(canvas, x, y, u, ARM);
        sprite(canvas, x - u, y - 2 * u, u, &["pp.", "pp."][..]);
    }
    for i in 0..3i64 {
        canvas.fill(
            cx + half + 3 * u + i * u,
            top + (i % 2) * u,
            u as u32,
            u as u32,
            0x00c8_ccd2,
        );
    }
}

const CONFETTI: [u32; 6] = [
    0x00f2_c14e,
    0x00d6_403a,
    0x004c_af50,
    0x008f_d3ff,
    0x00f0_8a24,
    0x00e8_7fd0,
];

/// Confetti bursting over a figure `ago` into its cheer and falling.
pub(super) fn confetti(canvas: &mut Canvas, s: Stage, t: &Tag, ago: Duration) {
    let u = s.u as f64;
    let (cx, top) = figure(t);
    let secs = ago.as_secs_f64();
    for i in 0..18u64 {
        let h = crate::fun::mix(t.agent.raw() ^ i);
        // Out and up in a fan, then down under gravity, in art pixels a second.
        let vx = ((h % 41) as f64 - 20.0) * 0.9;
        let vy = -30.0 - (h >> 8) as f64 % 18.0;
        let x = cx as f64 + vx * secs * u;
        let y = top as f64 + (vy * secs + 32.0 * secs * secs) * u;
        let col = CONFETTI[(h >> 16) as usize % CONFETTI.len()];
        let flip = ((s.ms / 150 + i) % 2) as u32;
        let side = u.max(1.0) as u32;
        canvas.fill(x as i64, y as i64, side + flip * side, side, col);
    }
}

/// What a cheer says on its plate, and the plate's colour.
fn cheer_plate(c: Cheer) -> (&'static str, u32) {
    match c {
        Cheer::TestsPassed => ("测试全过！", 0x002e_8b57),
        Cheer::Shipped => ("已推送！", 0x00b0_3a2e),
        Cheer::Recovered => ("终于过咗！", 0x00b8_860b),
    }
}

/// A plate with `text` at (`x`, `y`) in the readable faces; its area.
fn plate(
    canvas: &mut Canvas,
    faces: &mut Faces,
    s: Stage,
    (x, y): (i64, i64),
    text: &str,
    (bg, fg): (u32, u32),
) -> Area {
    let px = pt_px(10.0, s.scale_factor);
    let pad = tag_pad(s.scale_factor);
    let w = Faces::cells(text) * faces.cell_w(px) + 2 * pad;
    let h = px + 2 * pad;
    canvas.fill(x, y, w, h, bg);
    let base = y + i64::from(pad) + i64::from(faces.ascent(px));
    faces.draw(canvas, x + i64::from(pad), base, text, fg, px);
    Area { x, y, w, h }
}

/// The cheer's plate, east of the figure's badge.
pub(super) fn cheer_label(canvas: &mut Canvas, faces: &mut Faces, s: Stage, t: &Tag, c: Cheer) {
    let (text, bg) = cheer_plate(c);
    let pad = i64::from(tag_pad(s.scale_factor));
    let at = (t.badge.x + i64::from(t.badge.w) + pad, t.badge.y);
    plate(canvas, faces, s, at, text, (bg, 0x00ff_ffff));
}

/// A neighbour clapping over its badge for a comeback.
pub(super) fn clapping(canvas: &mut Canvas, s: Stage, t: &Tag) {
    let u = s.u;
    let hands = if (s.ms / 160).is_multiple_of(2) {
        HANDS_OPEN
    } else {
        HANDS_SHUT
    };
    let x = t.badge.x + i64::from(t.badge.w) + u;
    sprite(
        canvas,
        x,
        t.badge.y + i64::from(t.badge.h) - 3 * u,
        u,
        hands,
    );
}

/// Whether `other` sits near enough `t` to clap for it.
pub(super) fn neighbours(t: &Tag, other: &Tag, u: i64) -> bool {
    let (ax, ay) = figure(t);
    let (bx, by) = figure(other);
    t.agent != other.agent && (ax - bx).abs() < figure_w(u) * 8 && (ay - by).abs() < figure_h(u) * 3
}

/// The boss popping up over your office's sign `ago` after you approved
/// something, with a word for it.
pub(super) fn boss(
    canvas: &mut Canvas,
    faces: &mut Faces,
    s: Stage,
    sign: &DoorSign,
    ago: Duration,
    stay: Duration,
) {
    let u = s.u;
    let pop = Duration::from_millis(250);
    let out = stay.saturating_sub(ago);
    let rise = (ago.min(out).as_secs_f64() / pop.as_secs_f64()).min(1.0);
    let bh = BOSS.len() as i64 * u;
    let shown = (bh as f64 * rise) as i64;
    if shown <= 0 {
        return;
    }
    let x = sign.area.x + i64::from(sign.area.w / 2) - sprite_w(BOSS) * u / 2;
    let y = sign.area.y - shown;
    // Only what has risen over the sign shows.
    let rows = (shown / u).clamp(0, BOSS.len() as i64) as usize;
    sprite(canvas, x, y, u, &BOSS[..rows]);
    if rise >= 1.0 {
        let pad = i64::from(tag_pad(s.scale_factor));
        let at = (x + sprite_w(BOSS) * u + pad, y);
        plate(canvas, faces, s, at, "批咗！", (0x00f4_f1e8, 0x0020_2020));
    }
}

/// A chitchat bubble with `line` over a figure, above its highest tag; left
/// out where it would cover another's. Its area, when drawn.
pub(super) fn chitchat(
    canvas: &mut Canvas,
    faces: &mut Faces,
    s: Stage,
    t: &Tag,
    line: &str,
    taken: &[Area],
) -> Option<Area> {
    let px = pt_px(10.0, s.scale_factor);
    let pad = tag_pad(s.scale_factor);
    let w = Faces::cells(line) * faces.cell_w(px) + 2 * pad;
    let h = px + 2 * pad;
    let over = t.label.unwrap_or(t.badge);
    let cx = t.badge.x + i64::from(t.badge.w / 2);
    let tail = s.u.max(2);
    let area = (1..=3)
        .map(|tier| Area {
            x: (cx - i64::from(w / 2)).max(0),
            y: over.y - (i64::from(h) + tail) * tier,
            w,
            h,
        })
        .find(|a| a.y >= 0 && !taken.iter().any(|o| o.meets(a)))?;
    const PAPER: u32 = 0x00f4_f1e8;
    canvas.fill(area.x, area.y, area.w, area.h, PAPER);
    // The tail, pointing down at its speaker.
    let tx = cx.clamp(area.x + tail, area.x + i64::from(area.w) - 2 * tail);
    canvas.fill(
        tx,
        area.y + i64::from(area.h),
        tail as u32,
        tail as u32,
        PAPER,
    );
    let base = area.y + i64::from(pad) + i64::from(faces.ascent(px));
    faces.draw(canvas, area.x + i64::from(pad), base, line, 0x0020_2020, px);
    Some(area)
}

/// The cat's word in a bubble over the point it was clicked at.
pub(super) fn meow(
    canvas: &mut Canvas,
    faces: &mut Faces,
    s: Stage,
    (x, y): (i64, i64),
    line: &str,
) {
    let px = pt_px(10.0, s.scale_factor);
    let pad = tag_pad(s.scale_factor);
    let w = Faces::cells(line) * faces.cell_w(px) + 2 * pad;
    let h = px + 2 * pad;
    let tail = s.u.max(2);
    let ax = (x - i64::from(w / 2)).max(0);
    let ay = (y - i64::from(h) - 3 * tail).max(0);
    const PAPER: u32 = 0x00f4_f1e8;
    canvas.fill(ax, ay, w, h, PAPER);
    canvas.fill(x, ay + i64::from(h), tail as u32, tail as u32, PAPER);
    let base = ay + i64::from(pad) + i64::from(faces.ascent(px));
    faces.draw(canvas, ax + i64::from(pad), base, line, 0x0020_2020, px);
}

/// How long the evening board's replay of the day runs.
pub(super) const REPLAY_FOR: Duration = Duration::from_secs(10);
/// The part of it the day's turns take to fill in, hour by hour; the
/// honours follow.
const REPLAY_DAY: Duration = Duration::from_secs(6);

const TROPHY: &[&str] = &[
    "yyyyyyy", "yyyyyyy", "y.yyy.y", "yy.y.yy", "..yyy..", "...y...", "..yyy..", ".ooooo.",
];

/// The day replayed on a card over the office, `ago` into its ten seconds:
/// the turns finishing hour by hour on a bar for each hour, then who
/// finished most, with a trophy, and who was stuck longest.
pub(super) fn replay(
    canvas: &mut Canvas,
    faces: &mut Faces,
    s: Stage,
    stats: &crate::fun::stats::Stats,
    ago: Duration,
) {
    let px = pt_px(11.0, s.scale_factor);
    let pad = i64::from(tag_pad(s.scale_factor)) * 3;
    let line_h = i64::from(px) + pad;
    let (ow, oh) = (i64::from(s.w), i64::from(s.h));
    let w = (ow * 3 / 5).clamp(200.min(ow), ow);
    let h = (line_h * 7 + pad * 2).min(oh);
    let (x, y) = ((ow - w) / 2, ((oh - h) / 2).max(0));
    canvas.fill(x - 2, y - 2, (w + 4) as u32, (h + 4) as u32, 0x00ff_4fa3);
    canvas.fill(x, y, w as u32, h as u32, 0x0016_1220);
    let text = |canvas: &mut Canvas, faces: &mut Faces, row: i64, line: &str, ink: u32| {
        let base = y + pad + row * line_h + i64::from(faces.ascent(px));
        faces.draw(canvas, x + pad, base, line, ink, px);
    };
    text(canvas, faces, 0, "今日回顾", 0x00ff_4fa3);

    // The day so far, its hours filling in one after another.
    let t = (ago.as_secs_f64() / REPLAY_DAY.as_secs_f64()).min(1.0);
    let hours_shown = (t * 24.0).ceil() as usize;
    let done: u32 = stats.hours.iter().take(hours_shown).sum();
    let clock = format!(
        "{:02}:00  完成 {done} 轮",
        hours_shown.min(24).saturating_sub(1)
    );
    text(canvas, faces, 1, &clock, 0x00e6_e6e6);
    let chart_top = y + pad + 2 * line_h;
    let chart_h = line_h * 2;
    let peak = stats.hours.iter().copied().max().unwrap_or(0).max(1);
    let bar_w = ((w - 2 * pad) / 24).max(1);
    for (i, &n) in stats.hours.iter().enumerate().take(hours_shown) {
        let bh = (chart_h * i64::from(n) / i64::from(peak)).max(i64::from(n > 0) * 2);
        let bx = x + pad + i as i64 * bar_w;
        canvas.fill(
            bx,
            chart_top + chart_h - bh,
            (bar_w - 1).max(1) as u32,
            bh as u32,
            0x0063_e6be,
        );
    }
    canvas.fill(
        x + pad,
        chart_top + chart_h,
        (w - 2 * pad) as u32,
        1,
        0x0060_6070,
    );

    if ago < REPLAY_DAY {
        return;
    }
    let recap = stats.recap();
    let u = s.u.max(1);
    let row = 5;
    let most = match &recap.most {
        Some((name, n)) => format!("最多：{} {n} 轮", pixtuoid_scene::badge::shown_label(name)),
        None => "今日未有人完成".to_owned(),
    };
    // The trophy drops in and bounces where the winner's line ends.
    let since = (ago - REPLAY_DAY).as_secs_f64();
    let bounce = ((since * 6.0).sin().abs() * (1.0 - since / 4.0).max(0.0) * 4.0) as i64 * u;
    let tx = x + w - pad - sprite_w(TROPHY) * u;
    let ty = y + pad + row * line_h - TROPHY.len() as i64 * u / 2 - bounce;
    text(canvas, faces, row, &most, 0x00f2_c14e);
    if recap.most.is_some() {
        sprite(canvas, tx, ty, u, TROPHY);
    }
    if ago >= REPLAY_DAY + Duration::from_millis(1_200) {
        let stuck = match &recap.stuck {
            Some((name, secs)) => format!(
                "卡最久：{} {} 分钟",
                pixtuoid_scene::badge::shown_label(name),
                secs.div_ceil(60)
            ),
            None => "今日冇人卡住".to_owned(),
        };
        text(canvas, faces, row + 1, &stuck, 0x00ff_8a8a);
    }
}

/// The office dressed for `festival`: lanterns, spring couplets and
/// firecrackers, or Christmas lights and a tree.
pub(super) fn festival(
    canvas: &mut Canvas,
    faces: &mut Faces,
    s: Stage,
    festival: Festival,
    signs: &[DoorSign],
) {
    let u = s.u;
    let top = s.floor_top;
    match festival {
        Festival::Christmas => {
            // A string of lights under the windows, blinking in turn.
            let step = 6 * u;
            canvas.fill(0, top + u, s.w, u as u32, 0x0024_5a2a);
            for (i, x) in (0..i64::from(s.w)).step_by(step as usize).enumerate() {
                let col = [0x00d6_403a, 0x004c_af50, 0x00f2_c14e, 0x008f_d3ff][i % 4];
                let on = !(i as u64 + s.ms / 500).is_multiple_of(3);
                if on {
                    canvas.fill(x + 2 * u, top + 2 * u, (2 * u) as u32, (2 * u) as u32, col);
                }
            }
            if let Some(o) = signs.iter().find(|d| d.text == "你的办公室") {
                let th = TREE.len() as i64 * u;
                sprite(
                    canvas,
                    o.area.x - sprite_w(TREE) * u - u,
                    o.area.y + i64::from(o.area.h) - th,
                    u,
                    TREE,
                );
            }
        }
        Festival::MidAutumn => {
            // Lanterns along the windows, swaying a little.
            let n = (i64::from(s.w) / (40 * u)).max(3);
            for i in 0..n {
                let x = i64::from(s.w) * (2 * i + 1) / (2 * n);
                let sway = if (s.ms / 600 + i as u64).is_multiple_of(2) {
                    0
                } else {
                    u
                };
                canvas.fill(x + 2 * u, top, u as u32, (3 * u) as u32, 0x0060_4020);
                sprite(canvas, x + sway, top + 3 * u, u, LANTERN);
            }
        }
        Festival::LunarNewYear => {
            // A 福 on red by every door, and firecrackers at the windows' ends.
            for d in signs {
                let r = i64::from(d.area.h) * 3 / 4;
                let (cx, cy) = (d.area.x - r - u, d.area.y + i64::from(d.area.h / 2));
                for dy in -r..=r {
                    let half = r - dy.abs();
                    canvas.fill(cx - half, cy + dy, (2 * half + 1) as u32, 1, 0x00c6_282b);
                }
                let px = (r as u32).max(8);
                let base = cy + i64::from(faces.ascent(px)) / 2;
                faces.draw(canvas, cx - i64::from(px / 2), base, "福", 0x00f2_c14e, px);
            }
            for x in [2 * u, i64::from(s.w) - 5 * u] {
                canvas.fill(x + u, top, u as u32, (14 * u) as u32, 0x00f2_c14e);
                for i in 0..6 {
                    let side = if i % 2 == 0 { -u } else { u };
                    canvas.fill(
                        x + side,
                        top + u + i * 2 * u,
                        (2 * u) as u32,
                        (2 * u) as u32,
                        0x00d6_403a,
                    );
                }
            }
        }
    }
}

/// The tropical cyclone signal up, on a placard by the windows: its symbol
/// as the Observatory hoists it, and its name.
pub(super) fn typhoon_signal(canvas: &mut Canvas, faces: &mut Faces, s: Stage, signal: u8) {
    let symbol: &[&str] = match signal {
        1 => &[
            "kkkkkkk", "...k...", "...k...", "...k...", "...k...", "...k...", "...k...",
        ],
        3 => &[
            "...k...", "...k...", "...k...", "...k...", "...k...", "...k...", "kkkkkkk",
        ],
        9 => &[
            "kkkkkkk", ".kkkkk.", "..kkk..", "...k...", "..kkk..", ".kkkkk.", "kkkkkkk",
        ],
        10 => &[
            "...k...", "...k...", "...k...", "kkkkkkk", "...k...", "...k...", "...k...",
        ],
        _ => &[
            "...k...", "..kkk..", "..kkk..", ".kkkkk.", ".kkkkk.", "kkkkkkk", "kkkkkkk",
        ],
    };
    let name = match signal {
        1 => "一号风球",
        3 => "三号风球",
        8 => "八号风球",
        9 => "九号风球",
        _ => "十号风球",
    };
    let u = s.u;
    let px = pt_px(10.0, s.scale_factor);
    let pad = i64::from(tag_pad(s.scale_factor));
    let tw = i64::from(Faces::cells(name) * faces.cell_w(px));
    let sym = 7 * u;
    let w = sym + tw + 4 * pad;
    let h = sym.max(i64::from(px)) + 2 * pad;
    let x = i64::from(s.w) - w - 4 * u;
    let y = s.floor_top + 4 * u;
    let bg = if signal >= 8 {
        0x00f2_c14e
    } else {
        0x00f4_f1e8
    };
    canvas.fill(x, y, w as u32, h as u32, bg);
    sprite(canvas, x + pad, y + (h - sym) / 2, u, symbol);
    let base = y + (h - i64::from(px)) / 2 + i64::from(faces.ascent(px));
    faces.draw(canvas, x + sym + 3 * pad, base, name, 0x0020_2020, px);
}

/// The wall board's two lines with the day's tally, in Chinese like the
/// rest of the office: the window sets them in its readable faces, the art
/// font's 8px English having been all but unreadable.
pub(super) fn tally_lines(stats: &crate::fun::stats::Stats) -> [String; 2] {
    [
        format!("今日完成 {} 轮", stats.turns),
        format!(
            "卡住 {} · {} token",
            stats.stuck,
            crate::fun::stats::tokens_short(stats.tokens)
        ),
    ]
}

/// Milliseconds since the epoch at `now`, the animations' clock.
pub(super) fn ms_of(now: SystemTime) -> u64 {
    now.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Late-night dimming darkens the floor and spares a working desk's pool
    /// of light and the windows' band, or the harbour would go dark with it.
    #[test]
    fn the_night_dims_the_floor_but_not_the_lit_desk_or_the_windows() {
        let mut c = Canvas::new(200, 120, 0x0080_8080);
        let s = Stage {
            u: 2,
            scale_factor: 1.0,
            w: 200,
            h: 120,
            floor_top: 30,
            ms: 0,
        };
        let id = AgentId::from_parts("claude-code", "a");
        let tag = Tag {
            agent: id,
            badge: Area {
                x: 40,
                y: 40,
                w: 20,
                h: 8,
            },
            figure_top: 50,
            label: None,
            plate: 0,
            marker: 0,
            marker_rgb: pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
            name: String::new(),
            ink: 0,
            task: None,
            full: true,
        };
        night_lights(&mut c, s, &[tag], &[id]);
        let at = |x: u32, y: u32| c.px[(y * c.w + x) as usize];
        assert_eq!(at(50, 64), 0x0080_8080, "the lit desk keeps its light");
        assert_eq!(at(10, 10), 0x0080_8080, "the windows keep theirs");
        assert!(
            at(180, 110) < 0x0060_6060,
            "the rest of the floor goes dark"
        );
    }

    /// The fast path for the dark floor must light the night exactly as
    /// working it out pixel by pixel does, on every colour and round two
    /// overlapping pools: it is a speed-up, not a new look.
    #[test]
    fn the_dark_floor_shortcut_matches_the_pixel_by_pixel_light() {
        fn by_pixel(canvas: &mut Canvas, s: Stage, pools: &[(f64, f64, f64, f64)]) {
            let (w, h) = (s.w.min(canvas.w), s.h.min(canvas.h));
            for y in s.floor_top as u32..h {
                for x in 0..w {
                    let reach = pools
                        .iter()
                        .map(|&(cx, cy, rx, ry)| {
                            let (dx, dy) = ((f64::from(x) - cx) / rx, (f64::from(y) - cy) / ry);
                            dx * dx + dy * dy
                        })
                        .fold(f64::MAX, f64::min);
                    let dark = ((reach - 0.55) / 0.45).clamp(0.0, 1.0);
                    if dark > 0.0 {
                        let p = &mut canvas.px[(y * canvas.w + x) as usize];
                        *p = dim(*p, dark);
                    }
                }
            }
        }
        let s = Stage {
            u: 2,
            scale_factor: 1.0,
            w: 220,
            h: 140,
            floor_top: 20,
            ms: 0,
        };
        let tag = |n: &str, x: i64, top: i64| Tag {
            agent: AgentId::from_parts("claude-code", n),
            badge: Area {
                x,
                y: top - 10,
                w: 20,
                h: 8,
            },
            figure_top: top,
            label: None,
            plate: 0,
            marker: 0,
            marker_rgb: pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
            name: String::new(),
            ink: 0,
            task: None,
            full: true,
        };
        let tags = [tag("a", 40, 50), tag("b", 60, 60), tag("c", 150, 90)];
        let lit: Vec<AgentId> = tags[..2].iter().map(|t| t.agent).collect();
        let mut fast = Canvas::new(220, 140, 0);
        for (i, p) in fast.px.iter_mut().enumerate() {
            *p = (i as u32).wrapping_mul(2_654_435_761) & 0x00ff_ffff;
        }
        let mut slow = Canvas::new(220, 140, 0);
        slow.px.clone_from(&fast.px);
        night_lights(&mut fast, s, &tags, &lit);
        let pools: Vec<_> = tags[..2]
            .iter()
            .map(|t| {
                let (cx, top) = figure(t);
                (
                    cx as f64,
                    (top + figure_h(s.u) * 3 / 5) as f64,
                    (figure_w(s.u) * 3) as f64,
                    (figure_h(s.u) * 8 / 5) as f64,
                )
            })
            .collect();
        by_pixel(&mut slow, s, &pools);
        assert!(fast.px == slow.px);
    }

    /// The board's tally says what was done in the office's language, short
    /// enough for the board's line.
    #[test]
    fn the_tally_reads_turns_stuck_and_tokens() {
        let stats = crate::fun::stats::Stats {
            turns: 12,
            stuck: 2,
            tokens: 1_234_567,
            ..Default::default()
        };
        assert_eq!(
            tally_lines(&stats),
            ["今日完成 12 轮", "卡住 2 · 1.2M token"]
        );
        assert!(
            tally_lines(&stats)
                .iter()
                .all(|l| crate::text::cells(l) <= 20)
        );
    }
}
