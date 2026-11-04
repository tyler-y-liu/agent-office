//! agent-office: the waiting line. An agent that has been waiting on you
//! (a permission prompt, a question) for a while leaves its desk and lines up
//! by the entrance in the order it started waiting, so how many need you, and
//! which first, reads off the floor. A prompt answered within seconds moves
//! no one. Approved, it walks back to its desk.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::AgentId;
use pixtuoid_core::state::{ActivityState, AgentSlot};

use crate::layout::{CHARACTER_SPRITE_W, Point, SceneLayout};
use crate::walk::{WanderKind, WanderTarget};

/// How long an agent waits on you at its desk before it lines up.
pub(crate) const LINE_UP_AFTER_MS: u64 = 20_000;

/// Columns between neighbours in the line.
const LINE_GAP: u16 = CHARACTER_SPRITE_W + 2;

/// Whether `slot` has been waiting on you long enough to line up.
pub(crate) fn lines_up(slot: &AgentSlot, now: SystemTime) -> bool {
    slot.exiting_at.is_none()
        && matches!(slot.state, ActivityState::RequiresAction { .. })
        && crate::anim::elapsed_ms(now, slot.state_started_at) >= LINE_UP_AFTER_MS
}

/// The places in the line, first first: west from the entrance along the
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
    waiting.sort_by_key(|s| (s.state_started_at, s.agent_id));
    let spots = places(layout, waiting.len());
    waiting
        .into_iter()
        .zip(spots)
        .map(|(s, dest)| {
            (
                s.agent_id,
                WanderTarget {
                    dest,
                    kind: WanderKind::Aimless,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
