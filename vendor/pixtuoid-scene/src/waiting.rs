//! agent-office: the waiting line. An agent that has been waiting on you
//! (a permission prompt, a question) for a while leaves its desk and takes a
//! seat by your office in the order it started waiting: the first in the
//! guest chair at your desk, the rest in the waiting area outside, so how
//! many need you, and which first, reads off the floor. A prompt answered
//! within seconds moves no one. Approved, it walks back to its desk.

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
fn waits_on_reply(slot: &AgentSlot) -> bool {
    matches!(&slot.state, ActivityState::RequiresAction { reason: Some(r) } if &**r == REPLY_REASON)
}

/// Columns between neighbours in the line.
const LINE_GAP: u16 = CHARACTER_SPRITE_W + 2;

/// Whether `slot` has been waiting on you long enough to line up.
pub(crate) fn lines_up(slot: &AgentSlot, now: SystemTime) -> bool {
    slot.exiting_at.is_none()
        && matches!(slot.state, ActivityState::RequiresAction { .. })
        && crate::anim::elapsed_ms(now, slot.state_started_at) >= LINE_UP_AFTER_MS
}

/// The seats the line takes, first first: the guest chair at your desk (the
/// chair facing it from the west), then the waiting area's, nearest your
/// office first.
fn seats(layout: &SceneLayout) -> Vec<usize> {
    let room = |style| layout.meeting_rooms.iter().position(|r| r.style == style);
    let in_room = |r: Option<usize>| {
        (0..layout.waypoints.len())
            .filter(move |&i| r.is_some() && layout.waypoints[i].room_id == r)
    };
    let guest = in_room(room(RoomStyle::Boss)).find(|&i| {
        let w = layout.waypoints[i];
        w.kind == WaypointKind::MeetingChair && w.facing == Facing::East
    });
    let mut waiting: Vec<usize> = in_room(room(RoomStyle::Waiting)).collect();
    // Nearest your office, east, first; north before south on a sofa.
    waiting.sort_by_key(|&i| {
        let p = layout.waypoints[i].pos;
        (std::cmp::Reverse(p.x), p.y)
    });
    guest.into_iter().chain(waiting).collect()
}

/// Where those the seats run out for stand: west from the entrance along the
/// north wall, each a walkable cell the floor reaches.
fn places(layout: &SceneLayout, n: usize) -> Vec<Point> {
    let start = layout.door_threshold;
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
pub(crate) fn waiting_line(
    agents: &[AgentSlot],
    now: SystemTime,
    layout: &SceneLayout,
) -> HashMap<AgentId, WanderTarget> {
    let mut waiting: Vec<&AgentSlot> = agents
        .iter()
        .filter(|s| lines_up(s, now))
        .filter(|s| {
            layout
                .home_desk(s.desk_index.single_floor_local())
                .is_some()
        })
        .collect();
    waiting.sort_by_key(|s| (waits_on_reply(s), s.state_started_at, s.agent_id));
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
