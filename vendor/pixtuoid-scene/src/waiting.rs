//! agent-office: the line in your office. A session that needs you (a
//! permission prompt left a while, a question, a result you have not read)
//! leaves its desk and takes a seat in your office: prompts first, then
//! questions, then results, each in the order it came. The first sits in the
//! guest chair at your desk, the rest on the sofas, then in the corridor
//! outside the door, so how many need you, and which first, reads off the
//! floor. A prompt answered within seconds moves no one. Dealt with, a
//! session walks back to its desk; read, on to the pantry.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::AgentId;
use pixtuoid_core::state::{ActivityState, AgentSlot};

use crate::layout::{CHARACTER_SPRITE_W, Facing, Point, RoomStyle, SceneLayout, WaypointKind};
use crate::walk::{WanderKind, WanderTarget};

/// How long an agent waits on you at its desk before it lines up.
/// agent-office: public, so the host opens the one in the guest chair.
pub const LINE_UP_AFTER_MS: u64 = 20_000;

/// agent-office: the reason on a session the host lines up for your reply:
/// its turn ended on a question to you. It waits behind a prompt, which
/// holds its session mid-turn where a question has it done.
pub const REPLY_REASON: &str = "agent-office: reply";

/// Whether `slot` waits on your reply rather than on a prompt.
/// agent-office: crate-wide, so its head mark says which.
pub(crate) fn waits_on_reply(slot: &AgentSlot) -> bool {
    matches!(&slot.state, ActivityState::RequiresAction { reason: Some(r) } if &**r == REPLY_REASON)
}

/// agent-office: how long a finished result you have not read waits for you
/// in your office before it gives up and rests in the pantry.
pub const DONE_WAITS_MS: u64 = 15 * 60 * 1000;

/// agent-office: how long a finished session sits at its desk before it lines
/// up with its result: as long as any idle one stays at its desk.
pub const DONE_LINE_UP_AFTER_MS: u64 = crate::pose::IDLE_LEAVE_DESK_MS;

/// agent-office: what a session in the line needs from you. Declared in line
/// order: a prompt holds its session mid-turn, a question leaves it done but
/// stuck, a result just waits to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    /// A permission prompt.
    Approval,
    /// A question to you.
    Reply,
    /// A finished result you have not read.
    Read,
}

/// Columns between neighbours in the line.
const LINE_GAP: u16 = CHARACTER_SPRITE_W + 2;

/// agent-office: what `slot` needs from you, if anything. A finished one
/// needs reading only for [`DONE_WAITS_MS`]; the host backdates a read one
/// past that, so it leaves the line.
pub fn need(slot: &AgentSlot, now: SystemTime) -> Option<Need> {
    if slot.exiting_at.is_some() {
        return None;
    }
    match slot.state {
        ActivityState::RequiresAction { .. } if waits_on_reply(slot) => Some(Need::Reply),
        ActivityState::RequiresAction { .. } => Some(Need::Approval),
        ActivityState::Idle
            if crate::anim::elapsed_ms(now, slot.state_started_at) < DONE_WAITS_MS =>
        {
            Some(Need::Read)
        }
        _ => None,
    }
}

/// Whether `slot` has needed you long enough to line up: a prompt you would
/// answer within seconds moves no one.
/// agent-office: public, so the host opens the one the guest chair holds.
pub fn lines_up(slot: &AgentSlot, now: SystemTime) -> bool {
    let after = match need(slot, now) {
        None => return false,
        Some(Need::Read) => DONE_LINE_UP_AFTER_MS,
        Some(Need::Approval | Need::Reply) => LINE_UP_AFTER_MS,
    };
    crate::anim::elapsed_ms(now, slot.state_started_at) >= after
}

/// agent-office: where `slot` stands in the line, least first: by what it
/// needs, then by when it came. Shared with the host, so the guest chair and
/// the session it opens agree.
pub fn line_key(slot: &AgentSlot, now: SystemTime) -> (Option<Need>, SystemTime, AgentId) {
    (need(slot, now), slot.state_started_at, slot.agent_id)
}

/// The seats the line takes, first first: the guest chair at your desk (the
/// chair facing it from the west), then the sofas, north before south, west
/// to east. The chair facing the desk from the east is yours and stays empty.
fn seats(layout: &SceneLayout) -> Vec<usize> {
    let Some(office) = layout
        .meeting_rooms
        .iter()
        .position(|r| r.style == RoomStyle::Boss)
    else {
        return Vec::new();
    };
    let mut seats: Vec<usize> = (0..layout.waypoints.len())
        .filter(|&i| {
            let w = layout.waypoints[i];
            w.room_id == Some(office)
                && matches!(
                    w.kind,
                    WaypointKind::MeetingChair | WaypointKind::MeetingSofa
                )
                && !(w.kind == WaypointKind::MeetingChair && w.facing == Facing::West)
        })
        .collect();
    seats.sort_by_key(|&i| {
        let w = layout.waypoints[i];
        (w.kind != WaypointKind::MeetingChair, w.pos.y, w.pos.x)
    });
    seats
}

/// Where those the seats run out for stand: in the corridor outside your
/// office, west from its door, the doorway left clear for those going in;
/// each a walkable cell the floor reaches.
fn places(layout: &SceneLayout, n: usize) -> Vec<Point> {
    let start = layout
        .meeting_rooms
        .iter()
        .find(|r| r.style == RoomStyle::Boss)
        .map_or(layout.door_threshold, |r| {
            let b = r.bounds;
            Point {
                x: b.x + crate::layout::pct(b.width, crate::layout::OFFICE_DOOR_PCT),
                y: b.y.saturating_sub(2),
            }
        });
    let mut out: Vec<Point> = Vec::with_capacity(n);
    let mut step = 1u16;
    while out.len() < n {
        let Some(x) = start.x.checked_sub(step * LINE_GAP) else {
            break;
        };
        step += 1;
        let p = crate::pathfind::snap_point_where(&layout.walkable, Point { x, y: start.y }, |p| {
            layout.reachable.reaches(p)
        });
        if let Some(p) = p
            && !out.contains(&p)
        {
            out.push(p);
        }
    }
    out
}

/// Each lined-up agent's place in the line this frame, keyed by agent.
/// agent-office: sessions only, not sub-agents, as the host opens only
/// sessions; a sub-agent is a sub-agent of one on this floor.
pub(crate) fn waiting_line(
    agents: &[AgentSlot],
    now: SystemTime,
    layout: &SceneLayout,
) -> HashMap<AgentId, WanderTarget> {
    let mut waiting: Vec<&AgentSlot> = agents
        .iter()
        .filter(|s| lines_up(s, now))
        .filter(|s| {
            s.parent_id
                .is_none_or(|p| !agents.iter().any(|a| a.agent_id == p))
        })
        .filter(|s| {
            layout
                .home_desk(s.desk_index.single_floor_local())
                .is_some()
        })
        .collect();
    waiting.sort_by_key(|s| line_key(s, now));
    let mut seats = seats(layout).into_iter();
    let mut out = HashMap::new();
    let mut standing = Vec::new();
    for s in waiting {
        let desk = layout
            .home_desk(s.desk_index.single_floor_local())
            .expect("filtered on its desk");
        match seats
            .by_ref()
            .find_map(|i| crate::team::seat_target(layout, i, desk))
        {
            Some(t) => {
                out.insert(s.agent_id, t);
            }
            None => standing.push(s.agent_id),
        }
    }
    let spots = places(layout, standing.len());
    for (id, dest) in standing.into_iter().zip(spots) {
        out.insert(
            id,
            WanderTarget {
                dest,
                kind: WanderKind::Aimless,
            },
        );
    }
    out
}

#[cfg(test)]
mod tests;
