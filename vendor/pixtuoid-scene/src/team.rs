//! agent-office: sub-agent teams. A session whose sub-agents have run for a
//! while holds a meeting with them: the session at the head of a meeting
//! room's table, its sub-agents around it, so one glance shows "this one
//! spun up a team". A quick sub-agent does not call a meeting — walking the
//! whole team across the floor for a few seconds' work reads as noise.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::AgentId;
use pixtuoid_core::state::AgentSlot;

use crate::layout::{RoomStyle, SceneLayout, WaypointKind};
use crate::walk::{WanderKind, WanderTarget};

/// How long a sub-agent runs before its parent calls the team into a meeting.
pub(crate) const TEAM_AFTER_MS: u64 = 15_000;

/// Whether `sub` is a sub-agent too brief to draw: it ran (or has run so far)
/// under [`TEAM_AFTER_MS`]. Such a sub-agent is a "+N" on its parent's badge
/// instead of a figure walking in and out within seconds.
pub(crate) fn is_brief_sub(sub: &AgentSlot, now: SystemTime) -> bool {
    sub.parent_id.is_some()
        && crate::anim::elapsed_ms(sub.exiting_at.unwrap_or(now), sub.created_at) < TEAM_AFTER_MS
}

/// The brief sub-agents ([`is_brief_sub`]) of each parent among `agents`.
pub(crate) fn brief_subs<'a>(
    agents: impl IntoIterator<Item = &'a AgentSlot>,
    now: SystemTime,
) -> HashMap<AgentId, usize> {
    let agents: Vec<&AgentSlot> = agents.into_iter().collect();
    let mut out = HashMap::new();
    for sub in &agents {
        if let Some(parent) = sub.parent_id
            && is_brief_sub(sub, now)
            && agents.iter().any(|a| a.agent_id == parent)
        {
            *out.entry(parent).or_insert(0) += 1;
        }
    }
    out
}

/// Whether waypoint `wp_idx` is a seat in a meeting room kept for teams, as
/// opposed to the rooftop's seats, which are a lounge.
pub(crate) fn is_team_seat(layout: &SceneLayout, wp_idx: usize) -> bool {
    layout.waypoints[wp_idx]
        .room_id
        .and_then(|r| layout.meeting_rooms.get(r))
        .is_some_and(|room| room.style != RoomStyle::Rooftop)
}

/// The seats of team room `room`, head first: the chair at the table's west
/// end, then the sofas, then the far chair.
fn seats_head_first(layout: &SceneLayout, room: usize) -> Vec<usize> {
    let mut seats: Vec<usize> = (0..layout.waypoints.len())
        .filter(|&i| layout.waypoints[i].room_id == Some(room))
        .collect();
    let rank = |i: &usize| {
        let wp = &layout.waypoints[*i];
        match wp.kind {
            WaypointKind::MeetingChair if wp.facing == crate::layout::Facing::East => 0,
            WaypointKind::MeetingChair => 2,
            _ => 1,
        }
    };
    seats.sort_by_key(|i| (rank(i), *i));
    seats
}

/// The meeting seat each team member walks to this frame, keyed by agent.
/// Teams take rooms oldest parent first; a team with no room left, and the
/// sub-agents a full room has no seat for, stay at their desks.
pub(crate) fn meeting_seats(
    agents: &[AgentSlot],
    now: SystemTime,
    layout: &SceneLayout,
) -> HashMap<AgentId, WanderTarget> {
    let present = |s: &&AgentSlot| {
        s.exiting_at.is_none()
            && layout
                .home_desk(s.desk_index.single_floor_local())
                .is_some()
    };
    let mut teams: Vec<(&AgentSlot, Vec<&AgentSlot>)> = agents
        .iter()
        .filter(present)
        .filter_map(|parent| {
            let mut subs: Vec<&AgentSlot> = agents
                .iter()
                .filter(present)
                .filter(|s| {
                    s.parent_id == Some(parent.agent_id)
                        && crate::anim::elapsed_ms(now, s.created_at) >= TEAM_AFTER_MS
                })
                .collect();
            subs.sort_by_key(|s| (s.created_at, s.agent_id));
            (!subs.is_empty()).then_some((parent, subs))
        })
        .collect();
    teams.sort_by_key(|(p, _)| (p.created_at, p.agent_id));

    let rooms = (0..layout.meeting_rooms.len())
        .filter(|&r| layout.meeting_rooms[r].style != RoomStyle::Rooftop)
        .map(|r| seats_head_first(layout, r))
        .filter(|seats| !seats.is_empty());

    let mut out = HashMap::new();
    for ((parent, subs), seats) in teams.into_iter().zip(rooms) {
        for (member, wp_idx) in std::iter::once(parent).chain(subs).zip(seats) {
            let Some(desk) = layout.home_desk(member.desk_index.single_floor_local()) else {
                continue;
            };
            let wp = layout.waypoints[wp_idx];
            let dest = layout.approach_point(wp.kind.furniture(), wp.pos, desk.at, wp.facing);
            if dest == wp.pos {
                continue;
            }
            out.insert(
                member.agent_id,
                WanderTarget {
                    dest,
                    kind: WanderKind::Named {
                        wp_idx,
                        kind: wp.kind,
                        seat: crate::layout::seated_foot_cell(wp.kind.furniture(), wp.pos),
                    },
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests;
