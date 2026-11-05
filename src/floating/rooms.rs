//! What sets the south rooms apart, painted over the upscaled frame in window
//! pixels as the other goings-on are: the meeting rooms' whiteboard with their
//! subagents' tasks on it, the waiting area's ticket machine showing how many
//! wait on you, and your office's window out on the harbour. Each hangs in the
//! strip under its room's north wall, clear of the sofas.

use pixtuoid_core::{AgentId, SceneState};
use pixtuoid_scene::layout::{RoomStyle, SceneLayout};

use super::vector::Faces;
use super::{Area, Canvas, Tag, pt_px, tag_pad};
use crate::tasks::TaskViews;
use crate::text::truncate_cells;

/// A south room in window pixels, and window pixels to a layout unit.
#[derive(Clone, Copy)]
pub(super) struct Room {
    pub style: RoomStyle,
    pub area: Area,
    /// Window pixels to a layout unit.
    pub l: i64,
}

/// The office's rooms, `d` art pixels a layout unit and `u` window pixels an
/// art pixel.
pub(super) fn rooms_of(layout: &SceneLayout, d: u32, u: u32) -> Vec<Room> {
    let l = i64::from(d * u);
    layout
        .meeting_rooms
        .iter()
        .map(|r| Room {
            style: r.style,
            area: Area {
                x: i64::from(r.bounds.x) * l,
                y: i64::from(r.bounds.y) * l,
                w: u32::from(r.bounds.width) * u32::try_from(l).unwrap_or(1),
                h: u32::from(r.bounds.height) * u32::try_from(l).unwrap_or(1),
            },
            l,
        })
        .collect()
}

/// Layout units the strip under a room's north wall runs down: the sofas
/// stand south of it.
const STRIP_TOP: i64 = 1;
const STRIP_H: i64 = 12;
/// Layout units either side of a room's centre its sofas take, and the gap
/// kept to them.
const SOFA_HALF: i64 = 11;
const SOFA_GAP: i64 = 2;

/// Whether a figure whose badge is `t` stands in `room`.
fn in_room(t: &Tag, room: &Area) -> bool {
    let cx = t.badge.x + i64::from(t.badge.w / 2);
    (room.x..room.x + i64::from(room.w)).contains(&cx)
        && (room.y..room.y + i64::from(room.h)).contains(&t.figure_top)
}

/// What a meeting room's whiteboard says: the task of each subagent in
/// `room`, oldest first, by its summary, else its type, else its name.
pub(super) fn board_lines(
    room: &Area,
    tags: &[Tag],
    scene: &SceneState,
    views: &TaskViews,
) -> Vec<String> {
    let here: Vec<AgentId> = tags
        .iter()
        .filter(|t| in_room(t, room))
        .map(|t| t.agent)
        .collect();
    board_lines_of(&here, scene, views)
}

fn board_lines_of(here: &[AgentId], scene: &SceneState, views: &TaskViews) -> Vec<String> {
    let mut subs: Vec<_> = here
        .iter()
        .filter_map(|id| scene.agents.get(id))
        .filter(|s| s.parent_id.is_some())
        .collect();
    subs.sort_by_key(|s| s.created_at);
    subs.iter()
        .map(|s| {
            let v = views.get(&s.agent_id);
            v.and_then(|v| v.line.clone())
                .or_else(|| v.and_then(|v| v.agent_type.clone()))
                .unwrap_or_else(|| s.label.to_string())
        })
        .collect()
}

const BOARD_FRAME: u32 = 0x00a8_aeb6;
const BOARD_FACE: u32 = 0x00f4_f6f8;
const BOARD_TRAY: u32 = 0x006a_7078;
const MARKER_RED: u32 = 0x00d6_403a;
const MARKER_BLUE: u32 = 0x0024_4a8c;
const MARKER_FAINT: u32 = 0x00c4_cad2;

/// The whiteboard on a meeting room's north wall, west of its sofas, with
/// `lines` on it under its heading; a few faint strokes when no one meets.
pub(super) fn whiteboard(
    canvas: &mut Canvas,
    faces: &mut Faces,
    scale_factor: f64,
    room: &Room,
    lines: &[String],
) {
    let (a, l) = (room.area, room.l);
    let x0 = a.x + 3 * l;
    let x1 = a.x + i64::from(a.w / 2) - (SOFA_HALF + SOFA_GAP) * l;
    if x1 - x0 < 10 * l {
        return;
    }
    let (y0, h) = (a.y + STRIP_TOP * l, STRIP_H * l);
    let (w, hu) = (
        u32::try_from(x1 - x0).unwrap_or(0),
        u32::try_from(h).unwrap_or(0),
    );
    let t = u32::try_from((l / 3).max(1)).unwrap_or(1);
    canvas.fill(x0, y0, w, hu, BOARD_FRAME);
    canvas.fill(
        x0 + i64::from(t),
        y0 + i64::from(t),
        w - 2 * t,
        hu - 3 * t,
        BOARD_FACE,
    );
    // The marker tray along its foot.
    canvas.fill(
        x0 + l,
        y0 + h - i64::from(t),
        w - 2 * u32::try_from(l).unwrap_or(1),
        t,
        BOARD_TRAY,
    );
    let px = pt_px(8.0, scale_factor);
    let pad = i64::from(tag_pad(scale_factor)) + i64::from(t);
    let line_h = i64::from(px + px / 4);
    let cells = (i64::from(w) - 2 * pad) / i64::from(faces.cell_w(px).max(1));
    let cells = u16::try_from(cells.max(0)).unwrap_or(0);
    let ascent = i64::from(faces.ascent(px));
    let mut y = y0 + pad;
    let room_for = |y: i64| y + line_h <= y0 + h - i64::from(2 * t);
    if room_for(y) {
        faces.draw(canvas, x0 + pad, y + ascent, "议程", MARKER_RED, px);
        y += line_h;
    }
    if lines.is_empty() {
        // Last meeting's half-wiped strokes.
        let stroke_h = u32::try_from((l / 4).max(1)).unwrap_or(1);
        for (i, frac) in [0.7, 0.45, 0.6].iter().enumerate() {
            let sy = y + i64::try_from(i).unwrap_or(0) * line_h + line_h / 2;
            if !room_for(sy - line_h / 2) {
                break;
            }
            let sw = (f64::from(w) * frac) as u32;
            canvas.fill(
                x0 + pad,
                sy,
                sw.saturating_sub(2 * pad as u32),
                stroke_h,
                MARKER_FAINT,
            );
        }
        return;
    }
    for line in lines {
        if !room_for(y) {
            break;
        }
        let text = truncate_cells(&format!("· {line}"), cells);
        faces.draw(canvas, x0 + pad, y + ascent, &text, MARKER_BLUE, px);
        y += line_h;
    }
}

const MACHINE_RED: u32 = 0x00c8_3a32;
const MACHINE_SHADE: u32 = 0x008f_1f1d;
const MACHINE_FACE: u32 = 0x0018_1a1e;
const LED: u32 = 0x00ff_5a3c;
const TICKET: u32 = 0x00f4_f1e8;
const BUTTON: u32 = 0x00f2_c14e;

/// A 3×5 digit, its rows top to bottom as three bits each.
fn digit(d: u8) -> [u8; 5] {
    match d {
        0 => [7, 5, 5, 5, 7],
        1 => [2, 6, 2, 2, 7],
        2 => [7, 1, 7, 4, 7],
        3 => [7, 1, 7, 1, 7],
        4 => [5, 5, 7, 1, 1],
        5 => [7, 4, 7, 1, 7],
        6 => [7, 4, 7, 5, 7],
        7 => [7, 1, 1, 1, 1],
        8 => [7, 5, 7, 5, 7],
        _ => [7, 5, 7, 1, 7],
    }
}

/// The two digits a ticket machine shows for `waiting` sessions in line.
pub(super) fn ticket_digits(waiting: usize) -> [u8; 2] {
    let n = waiting.min(99) as u8;
    [n / 10, n % 10]
}

/// The ticket machine at the waiting area's north-east corner, east of its
/// sofas: its display shows how many wait on you, a ticket out of its slot
/// while anyone does.
pub(super) fn ticket_machine(canvas: &mut Canvas, room: &Room, waiting: usize, ms: u64) {
    let (a, l) = (room.area, room.l);
    let east = a.x + i64::from(a.w);
    let sofa_east = a.x + i64::from(a.w / 2) + SOFA_HALF * l;
    let w = 6 * l;
    let x = east - w - 2 * l;
    if x < sofa_east + l {
        return;
    }
    let y = a.y + (STRIP_TOP + 1) * l;
    let h = STRIP_H * l;
    let (wu, hu, lu) = (w as u32, h as u32, l as u32);
    canvas.fill(x, y, wu, hu, MACHINE_RED);
    canvas.fill(x + w - l / 2, y, lu / 2, hu, MACHINE_SHADE);
    // The display, its number lit.
    let (dx, dy, dw, dh) = (x + l / 2, y + l, w - 3 * l / 2, 3 * l);
    canvas.fill(dx, dy, dw as u32, dh as u32, MACHINE_FACE);
    let p = (dw * 4 / 5 / 7).min(dh * 4 / 5 / 5).max(1);
    let (gx, gy) = (dx + (dw - 7 * p) / 2, dy + (dh - 5 * p) / 2);
    for (i, d) in ticket_digits(waiting).into_iter().enumerate() {
        for (r, bits) in digit(d).iter().enumerate() {
            for c in 0..3 {
                if bits >> (2 - c) & 1 == 1 {
                    let cx = gx + (i64::try_from(i).unwrap_or(0) * 4 + c) * p;
                    canvas.fill(
                        cx,
                        gy + i64::try_from(r).unwrap_or(0) * p,
                        p as u32,
                        p as u32,
                        LED,
                    );
                }
            }
        }
    }
    // The button, and the slot with a ticket out of it, pulsing.
    canvas.fill(x + l, y + 5 * l, lu, lu, BUTTON);
    let slot_y = y + 7 * l;
    canvas.fill(
        x + l / 2,
        slot_y,
        (w - 3 * l / 2) as u32,
        (l / 3).max(1) as u32,
        MACHINE_FACE,
    );
    if waiting > 0 {
        let out = if (ms / 600).is_multiple_of(2) {
            2 * l
        } else {
            3 * l / 2
        };
        canvas.fill(
            x + l,
            slot_y,
            (w - 5 * l / 2).max(1) as u32,
            out as u32,
            TICKET,
        );
    }
}

/// The sky, the far shore and the sea through your office's window at
/// `hour`: (sky top, sky bottom, hills, sea, wave).
fn harbour_at(hour: u32) -> (u32, u32, u32, u32, u32) {
    match hour {
        7..17 => (
            0x007e_b8f0,
            0x00cf_e6fb,
            0x005c_7a8c,
            0x002f_6fa8,
            0x00b8_daf4,
        ),
        17..19 | 6 => (
            0x006a_5a9a,
            0x00f2_a86a,
            0x004a_4058,
            0x003a_4f7a,
            0x00f6_c48a,
        ),
        _ => (
            0x000a_1430,
            0x001e_2c58,
            0x0018_2034,
            0x000e_1c38,
            0x0046_6aa0,
        ),
    }
}

const WINDOW_FRAME: u32 = 0x004a_2e1c;
const SAIL: u32 = 0x00c0_392b;
const HULL: u32 = 0x003a_2a20;
const STAR: u32 = 0x00e8_ecf0;
const SUN: u32 = 0x00ff_f2c0;

/// Your office's window on its north wall, between its door sign `avoid` and
/// its bookshelf: the harbour at `hour`, a junk's red sail crossing it.
pub(super) fn sea_window(
    canvas: &mut Canvas,
    room: &Room,
    avoid: Option<&Area>,
    hour: u32,
    ms: u64,
) {
    let (a, l) = (room.area, room.l);
    let mut x0 = a.x + i64::from(a.w / 2) - SOFA_HALF * l;
    if let Some(s) = avoid {
        x0 = x0.max(s.x + i64::from(s.w) + l);
    }
    // The bookshelf stands in the east end.
    let x1 = a.x + i64::from(a.w) - 11 * l;
    if x1 - x0 < 10 * l {
        return;
    }
    let (y0, h) = (a.y + STRIP_TOP * l, STRIP_H * l - l);
    let t = (l / 2).max(1);
    let (w, hu) = ((x1 - x0) as u32, h as u32);
    canvas.fill(x0, y0, w, hu, WINDOW_FRAME);
    let (gx, gy, gw, gh) = (x0 + t, y0 + t, x1 - x0 - 2 * t, h - 2 * t);
    let (sky_top, sky_low, hills, sea, wave) = harbour_at(hour);
    let horizon = gh * 11 / 20;
    // The sky in bands, darker up top.
    let bands = 4;
    for b in 0..bands {
        let mix = |c0: u32, c1: u32, s: u32| {
            let (p, q) = ((c0 >> s) & 0xff, (c1 >> s) & 0xff);
            ((p * (bands - 1 - b) + q * b) / (bands - 1)) << s
        };
        let c = mix(sky_top, sky_low, 16) | mix(sky_top, sky_low, 8) | mix(sky_top, sky_low, 0);
        let by = gy + horizon * i64::from(b) / i64::from(bands);
        let bh = horizon / i64::from(bands) + 1;
        canvas.fill(gx, by, gw as u32, bh as u32, c);
    }
    let night = !(6..19).contains(&hour);
    if night {
        for i in 0..6i64 {
            let sx = gx + (i * 37 + 11) % gw.max(1);
            let sy = gy + (i * 23 + 5) % (horizon * 2 / 3).max(1);
            canvas.fill(sx, sy, (l / 4).max(1) as u32, (l / 4).max(1) as u32, STAR);
        }
    } else {
        canvas.fill(gx + gw * 3 / 4, gy + horizon / 4, l as u32, l as u32, SUN);
    }
    // The far shore's hills, a ridge along the horizon.
    let ridge = l.max(2);
    for x in 0..gw {
        let rise = ((x * 7 / gw.max(1)) % 3 + (x * 3 / gw.max(1)) % 2) * ridge / 3;
        canvas.fill(
            gx + x,
            gy + horizon - rise - ridge / 2,
            1,
            (rise + ridge / 2) as u32,
            hills,
        );
    }
    canvas.fill(gx, gy + horizon, gw as u32, (gh - horizon) as u32, sea);
    // Wave glints drifting east.
    let drift = (ms / 120) as i64;
    let glint = (l / 4).max(1);
    for row in 0..3i64 {
        let ry = gy + horizon + (gh - horizon) * (row + 1) / 4;
        let mut x = ((drift + row * 13) % (6 * l)) - 6 * l;
        while x < gw {
            if x >= 0 {
                canvas.fill(
                    gx + x,
                    ry,
                    (2 * l).min(gw - x).max(1) as u32,
                    glint as u32,
                    wave,
                );
            }
            x += 6 * l;
        }
    }
    // A junk crossing west to east, its red sail over a dark hull.
    let span = gw + 6 * l;
    let bx = gx - 3 * l + ((ms / 90) as i64 % span.max(1));
    let by = gy + horizon + (gh - horizon) / 3;
    for (dx, dy, bw, bh, c) in [
        (l, -2 * l, 2 * l, 2 * l, SAIL),
        (0, 0, 4 * l, l / 2 + 1, HULL),
    ] {
        let (sx, ex) = ((bx + dx).max(gx), (bx + dx + bw).min(gx + gw));
        if ex > sx {
            canvas.fill(sx, by + dy, (ex - sx) as u32, bh as u32, c);
        }
    }
    // The mullion and transom.
    canvas.fill(gx + gw / 2 - t / 2, gy, t as u32, gh as u32, WINDOW_FRAME);
    canvas.fill(
        gx,
        gy + gh / 3,
        gw as u32,
        (t / 2).max(1) as u32,
        WINDOW_FRAME,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::CC_SOURCE;
    use crate::tasks::TaskView;
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    /// The whiteboard tells you what a meeting is about: each subagent's
    /// task, oldest first, and never the parent's own, which its card has.
    #[test]
    fn the_board_lists_the_subagents_tasks() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let mut start = |sid: &str, parent: Option<AgentId>, at| {
            let id = AgentId::from_parts(CC_SOURCE, sid);
            r.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: CC_SOURCE.into(),
                    session_id: sid.into(),
                    cwd: PathBuf::from("/repo"),
                    parent_id: parent,
                },
                at,
                Transport::Hook,
            );
            id
        };
        let p = start("p", None, t0);
        let late = start("agent-late", Some(p), t0 + Duration::from_secs(9));
        let early = start("agent-early", Some(p), t0 + Duration::from_secs(1));
        let mut views = std::collections::HashMap::new();
        views.insert(
            late,
            TaskView {
                line: Some("查登录 bug".into()),
                ..TaskView::default()
            },
        );
        views.insert(
            early,
            TaskView {
                agent_type: Some("Explore".into()),
                ..TaskView::default()
            },
        );
        views.insert(
            p,
            TaskView {
                line: Some("父会话的活".into()),
                ..TaskView::default()
            },
        );
        assert_eq!(
            board_lines_of(&[late, p, early], &scene, &std::sync::Arc::new(views)),
            vec!["Explore".to_owned(), "查登录 bug".to_owned()]
        );
    }

    /// The machine shows the length of the line in two digits, and a line
    /// past 99 still reads as full rather than wrapping round to a short one.
    #[test]
    fn the_ticket_machine_shows_the_line_in_two_digits() {
        assert_eq!(ticket_digits(0), [0, 0]);
        assert_eq!(ticket_digits(7), [0, 7]);
        assert_eq!(ticket_digits(42), [4, 2]);
        assert_eq!(ticket_digits(130), [9, 9]);
    }
}
