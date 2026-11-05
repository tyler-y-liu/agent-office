use super::*;
use crate::character::test_support::make_slot;
use crate::layout::Facing;
use crate::pose::{Pose, RouteRig, derive_with_routing};
use pixtuoid_core::state::{ActivityState, GlobalDeskIndex};
use std::time::Duration;

fn team_rooms(l: &SceneLayout) -> usize {
    l.meeting_rooms
        .iter()
        .filter(|r| r.style == RoomStyle::Office && r.trio.is_some())
        .count()
}

fn office() -> SceneLayout {
    let l = SceneLayout::compute(240, 160, None).expect("fits");
    assert!(team_rooms(&l) >= 1, "the fixture needs a team room");
    l
}

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn agent(name: &str, desk: usize, created_ago_ms: u64, parent: Option<AgentId>) -> AgentSlot {
    let id = AgentId::from_transcript_path(&format!("/p/{name}.jsonl"));
    let mut s = make_slot(id, ActivityState::Idle);
    s.desk_index = GlobalDeskIndex(desk);
    s.created_at = t0() - Duration::from_millis(created_ago_ms);
    s.state_started_at = s.created_at;
    s.parent_id = parent;
    s
}

fn wp_of(t: &WanderTarget) -> usize {
    match t.kind {
        WanderKind::Named { wp_idx, .. } => wp_idx,
        WanderKind::Aimless => panic!("a meeting seat is a named seat"),
    }
}

/// A session whose sub-agent has been at it a while sits with it in one team
/// room, at the head of the table, so the team reads as one group led by it.
#[test]
fn a_long_running_sub_agent_meets_its_parent_in_a_team_room() {
    let l = office();
    let parent = agent("lead", 0, 60_000, None);
    let sub = agent("helper", 1, TEAM_AFTER_MS, Some(parent.agent_id));
    let seats = meeting_seats(&[parent.clone(), sub.clone()], t0(), &l);

    let head = wp_of(&seats[&parent.agent_id]);
    let other = wp_of(&seats[&sub.agent_id]);
    assert!(is_team_seat(&l, head) && is_team_seat(&l, other));
    assert_eq!(
        l.waypoints[head].room_id, l.waypoints[other].room_id,
        "one room"
    );
    assert_eq!(l.waypoints[head].kind, WaypointKind::MeetingChair);
    assert_eq!(
        l.waypoints[head].facing,
        Facing::East,
        "the head of the table"
    );
}

/// A sub-agent that has only just started does not pull its parent away from
/// the desk: most finish in seconds, and a walk for that is noise.
#[test]
fn a_brief_sub_agent_calls_no_meeting() {
    let l = office();
    let parent = agent("lead", 0, 60_000, None);
    let sub = agent("helper", 1, TEAM_AFTER_MS - 1, Some(parent.agent_id));
    assert!(meeting_seats(&[parent, sub], t0(), &l).is_empty());
}

/// The team rooms hold one team each, oldest first; a team left over stays at
/// its desks instead of crowding a room it has no seats in.
#[test]
fn teams_take_rooms_oldest_first_and_the_rest_stay_at_their_desks() {
    let l = office();
    let rooms = team_rooms(&l);
    let mut agents = Vec::new();
    for i in 0..=rooms {
        // The last team is the newest.
        let p = agent(&format!("lead{i}"), i * 2, 90_000 - i as u64 * 10_000, None);
        let s = agent(&format!("sub{i}"), i * 2 + 1, 20_000, Some(p.agent_id));
        agents.push(p);
        agents.push(s);
    }
    let seats = meeting_seats(&agents, t0(), &l);
    let room = |i: usize| {
        seats
            .get(&agents[i].agent_id)
            .map(|t| l.waypoints[wp_of(t)].room_id)
    };
    let mut met: Vec<_> = (0..rooms)
        .map(|t| room(2 * t).expect("an older team meets"))
        .collect();
    met.dedup();
    assert_eq!(met.len(), rooms, "each in a room of its own");
    assert_eq!(room(2 * rooms), None, "the newest team has no room left");
    assert_eq!(room(2 * rooms + 1), None);
}

/// An idle agent never takes a team room's seat while another lounge is free,
/// so a team that forms finds its room empty.
#[test]
fn idle_agents_leave_the_team_rooms_free() {
    let l = office();
    let desk = l.home_desks[0];
    for stage in [
        crate::pose::IdleStage::Pantry,
        crate::pose::IdleStage::Rooftop,
    ] {
        for i in 0..200u64 {
            let id = AgentId::from_transcript_path(&format!("/p/idle_{i}.jsonl"));
            let t = crate::pose::resolve_wander_target(
                id,
                0,
                &l,
                desk,
                &crate::pose::SpotClaims::default(),
                stage,
            );
            if let WanderKind::Named { wp_idx, .. } = t.kind {
                assert!(
                    !is_team_seat(&l, wp_idx),
                    "{stage:?} agent {i} took a team seat"
                );
            }
        }
    }
}

/// The routed path walks a member to its meeting seat and keeps it there while
/// it works, then walks it home once the team breaks up instead of popping it
/// back to its desk.
#[test]
fn a_member_walks_to_the_meeting_and_home_again() {
    let l = office();
    let mut parent = agent("lead", 0, 60_000, None);
    parent.state = ActivityState::Running { item: None };
    let sub = agent("helper", 1, 20_000, Some(parent.agent_id));
    let mut rig = RouteRig::new(crate::pathfind::AStarRouter::new());
    rig.meetings = meeting_seats(&[parent.clone(), sub], t0(), &l);
    let seat = wp_of(&rig.meetings[&parent.agent_id]);

    let mut now = t0();
    let mut sat = false;
    for _ in 0..600 {
        now += Duration::from_millis(100);
        let pose = derive_with_routing(&parent, now, &l, &mut rig.rctx());
        if let Some(Pose::AtWaypoint { wp, .. }) = pose {
            assert_eq!(wp, seat);
            sat = true;
            break;
        }
        assert!(matches!(pose, Some(Pose::Walking { .. })), "{pose:?}");
    }
    assert!(sat, "never reached the meeting seat");
    now += Duration::from_secs(60);
    let pose = derive_with_routing(&parent, now, &l, &mut rig.rctx());
    assert_eq!(
        pose,
        Some(Pose::AtWaypoint {
            wp: seat,
            kind: l.waypoints[seat].kind
        }),
        "working, still in the meeting"
    );

    rig.meetings.clear();
    now += Duration::from_millis(100);
    let pose = derive_with_routing(&parent, now, &l, &mut rig.rctx());
    assert!(
        matches!(pose, Some(Pose::Walking { .. })),
        "walks home: {pose:?}"
    );
    let mut home = false;
    for _ in 0..600 {
        now += Duration::from_millis(100);
        if derive_with_routing(&parent, now, &l, &mut rig.rctx()) == Some(Pose::SeatedTyping) {
            home = true;
            break;
        }
    }
    assert!(home, "never got back to its desk");
}

/// A sub-agent that has run under 15 s is not drawn — a figure walking in
/// and straight back out reads as noise — and stays undrawn on its way out
/// if it finished that fast. Past 15 s it is a figure like any other.
#[test]
fn only_a_brief_sub_agent_goes_undrawn() {
    let parent = agent("lead", 0, 60_000, None);
    let mut sub = agent("helper", 1, TEAM_AFTER_MS - 1, Some(parent.agent_id));
    assert!(is_brief_sub(&sub, t0()));
    assert!(!is_brief_sub(&parent, t0()), "a session is never hidden");

    sub.exiting_at = Some(sub.created_at + Duration::from_millis(5_000));
    assert!(
        is_brief_sub(&sub, t0() + Duration::from_secs(60)),
        "finished fast: still hidden while it leaves"
    );
    let old = agent("veteran", 2, TEAM_AFTER_MS, Some(parent.agent_id));
    assert!(!is_brief_sub(&old, t0()));
}

/// The parent's badge counts the sub-agents that are not drawn, so the work
/// they do is not invisible.
#[test]
fn the_parents_badge_counts_its_brief_sub_agents() {
    let mut parent = agent("lead", 0, 60_000, None);
    parent.label = "repo".into();
    let a = agent("a", 1, 1_000, Some(parent.agent_id));
    let b = agent("b", 2, 2_000, Some(parent.agent_id));
    let long = agent("c", 3, 30_000, Some(parent.agent_id));
    let all = [parent.clone(), a, b, long];
    let names = crate::badge::Namesakes::of(&all).with_brief_subs(&all, t0());
    assert_eq!(names.text(&parent), "repo +2");
}

/// A sub-agent drawn for the first time once it has run 15 s walks in from
/// the door, as every agent does on arriving, rather than popping up seated.
#[test]
fn a_sub_agent_walks_in_when_it_is_first_drawn() {
    let l = office();
    let parent = agent("lead", 0, 60_000, None);
    let mut sub = agent("helper", 1, TEAM_AFTER_MS + 100, Some(parent.agent_id));
    sub.state = ActivityState::Running { item: None };
    let mut rig = RouteRig::new(crate::pathfind::AStarRouter::new());
    let pose = derive_with_routing(&sub, t0(), &l, &mut rig.rctx());
    assert!(matches!(pose, Some(Pose::Walking { .. })), "{pose:?}");
    let at = rig
        .history
        .recent(sub.agent_id, 1_000, t0())
        .expect("drawn walking");
    let door = l.door_threshold;
    let desk = l.home_desks[1].at;
    let dist =
        |p: crate::layout::Point, q: crate::layout::Point| p.x.abs_diff(q.x) + p.y.abs_diff(q.y);
    assert!(dist(at, door) < dist(at, desk), "from the door: {at:?}");
}
