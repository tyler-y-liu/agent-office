use super::*;
use crate::character::test_support::make_slot;
use crate::pose::{Pose, RouteRig, derive_with_routing};
use pixtuoid_core::state::GlobalDeskIndex;
use std::time::Duration;

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn office() -> SceneLayout {
    SceneLayout::compute(240, 160, None).expect("fits")
}

fn waiter(name: &str, desk: usize, waiting_ms: u64) -> AgentSlot {
    let id = AgentId::from_transcript_path(&format!("/p/{name}.jsonl"));
    let mut s = make_slot(id, ActivityState::RequiresAction { reason: None });
    s.desk_index = GlobalDeskIndex(desk);
    s.created_at = t0() - Duration::from_secs(600);
    s.state_started_at = t0() - Duration::from_millis(waiting_ms);
    s
}

/// A prompt you answer within seconds moves no one; one left waiting lines up.
#[test]
fn only_a_long_wait_lines_up() {
    assert!(!lines_up(&waiter("quick", 0, LINE_UP_AFTER_MS - 1), t0()));
    assert!(lines_up(&waiter("slow", 0, LINE_UP_AFTER_MS), t0()));
    let mut working = waiter("busy", 0, 60_000);
    working.state = ActivityState::Running { item: None };
    assert!(!lines_up(&working, t0()), "only waiting on you lines up");
}

/// The line keeps the order agents started waiting in, first nearest the
/// entrance, each in a place of its own — so which one to answer first reads
/// off the floor.
#[test]
fn the_line_keeps_waiting_order() {
    let l = office();
    let agents = [
        waiter("second", 0, 40_000),
        waiter("first", 1, 90_000),
        waiter("third", 2, 25_000),
    ];
    let line = waiting_line(&agents, t0(), &l);
    let dx = |i: usize| {
        l.door_threshold
            .x
            .abs_diff(line[&agents[i].agent_id].dest.x)
    };
    assert!(
        dx(1) < dx(0) && dx(0) < dx(2),
        "first waiter nearest the entrance"
    );
    let mut spots: Vec<_> = line.values().map(|t| t.dest).collect();
    spots.dedup();
    assert_eq!(spots.len(), 3, "a place each");
}

/// A waiting agent walks to its place and stands there, then once approved
/// walks back to its desk rather than popping there.
#[test]
fn a_waiter_walks_to_the_line_and_back_when_approved() {
    let l = office();
    let mut slot = waiter("asker", 0, LINE_UP_AFTER_MS);
    let mut rig = RouteRig::new(crate::pathfind::AStarRouter::new());
    rig.meetings = waiting_line(std::slice::from_ref(&slot), t0(), &l);
    let place = rig.meetings[&slot.agent_id].dest;

    let mut now = t0();
    let mut arrived = false;
    for _ in 0..600 {
        now += Duration::from_millis(100);
        match derive_with_routing(&slot, now, &l, &mut rig.rctx()) {
            Some(Pose::AimlessAt { dest }) => {
                assert_eq!(dest, place);
                arrived = true;
                break;
            }
            p => assert!(matches!(p, Some(Pose::Walking { .. })), "{p:?}"),
        }
    }
    assert!(arrived, "never reached its place in the line");

    slot.state = ActivityState::Running { item: None };
    slot.state_started_at = now;
    rig.meetings.clear();
    now += Duration::from_millis(100);
    let p = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    assert!(matches!(p, Some(Pose::Walking { .. })), "walks back: {p:?}");
    let mut home = false;
    for _ in 0..600 {
        now += Duration::from_millis(100);
        if derive_with_routing(&slot, now, &l, &mut rig.rctx()) == Some(Pose::SeatedTyping) {
            home = true;
            break;
        }
    }
    assert!(home, "never got back to its desk");
}
