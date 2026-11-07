use super::*;
use crate::character::test_support::make_slot;
use crate::layout::{Facing, RoomStyle, WaypointKind};
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
    s.created_at = t0() - Duration::from_secs(3600);
    s.state_started_at = t0() - Duration::from_millis(waiting_ms);
    s
}

fn asking(name: &str, desk: usize, ms: u64) -> AgentSlot {
    let mut s = waiter(name, desk, ms);
    s.state = ActivityState::RequiresAction {
        reason: Some(REPLY_REASON.into()),
    };
    s
}

fn done(name: &str, desk: usize, idle_ms: u64) -> AgentSlot {
    let mut s = waiter(name, desk, idle_ms);
    s.state = ActivityState::Idle;
    s
}

fn boss(l: &SceneLayout) -> Option<usize> {
    l.meeting_rooms.iter().position(|r| r.style == RoomStyle::Boss)
}

/// The seat `id` takes in `line`.
fn seat_of(
    l: &SceneLayout,
    line: &HashMap<AgentId, WanderTarget>,
    id: AgentId,
) -> crate::layout::Waypoint {
    let WanderKind::Named { wp_idx, .. } = line[&id].kind else {
        panic!("a seat for {id:?}: {line:?}")
    };
    l.waypoints[wp_idx]
}

fn is_guest_chair(w: crate::layout::Waypoint) -> bool {
    (w.kind, w.facing) == (WaypointKind::MeetingChair, Facing::East)
}

/// A prompt you answer within seconds moves no one; one left waiting lines up.
/// A finished result lines up as soon as an idle session leaves its desk, so
/// it never first walks off to rest.
#[test]
fn only_a_long_wait_lines_up() {
    assert!(!lines_up(&waiter("quick", 0, LINE_UP_AFTER_MS - 1), t0()));
    assert!(lines_up(&waiter("slow", 0, LINE_UP_AFTER_MS), t0()));
    // A question is a finished turn, its summary back seconds in: it lines
    // up as a result does, or it would walk home and back once summarized.
    assert!(!lines_up(&asking("asked", 0, DONE_LINE_UP_AFTER_MS - 1), t0()));
    assert!(lines_up(&asking("asked", 0, DONE_LINE_UP_AFTER_MS), t0()));
    assert!(lines_up(&done("result", 0, DONE_LINE_UP_AFTER_MS), t0()));
    let mut working = waiter("busy", 0, 60_000);
    working.state = ActivityState::Running { item: None };
    assert!(!lines_up(&working, t0()), "one at work needs nothing");
}

/// The office holds only what needs you: a result lines up once its session
/// would leave its desk and stops once it has waited [`DONE_WAITS_MS`] (or
/// the host backdates it, read); one leaving, or a sub-agent, never does,
/// since `n` opens only sessions.
#[test]
fn a_result_lines_up_only_while_unread_and_fresh() {
    assert!(!lines_up(&done("r", 0, 2_000), t0()), "not at 2 s");
    assert!(lines_up(&done("r", 0, 3_000), t0()), "at 3 s");
    assert!(lines_up(&done("r", 0, DONE_WAITS_MS - 1), t0()));
    assert!(!lines_up(&done("r", 0, DONE_WAITS_MS), t0()), "not at 15 min");
    assert_eq!(need(&done("r", 0, DONE_WAITS_MS), t0()), None);
    let mut leaving = done("leaving", 0, 10_000);
    leaving.exiting_at = Some(t0());
    assert!(!lines_up(&leaving, t0()), "not one on its way out");

    let l = office();
    let parent = done("parent", 0, 10_000);
    let mut sub = done("sub", 1, 10_000);
    sub.parent_id = Some(parent.agent_id);
    let line = waiting_line(&[parent.clone(), sub.clone()], t0(), &l);
    assert!(line.contains_key(&parent.agent_id));
    assert!(!line.contains_key(&sub.agent_id), "a sub-agent stays out");
    let line = waiting_line(std::slice::from_ref(&sub), t0(), &l);
    assert!(
        line.contains_key(&sub.agent_id),
        "one whose parent is not on this floor is a session here"
    );
}

/// The first one waiting on you sits in the guest chair at your desk and the
/// rest take the sofas in the order they started waiting, north sofa west to
/// east, then the south one — so which one to answer first reads off the
/// floor.
#[test]
fn the_line_keeps_waiting_order() {
    let l = office();
    let agents = [
        waiter("second", 0, 40_000),
        waiter("first", 1, 90_000),
        waiter("third", 2, 25_000),
    ];
    let line = waiting_line(&agents, t0(), &l);
    let seat = |i: usize| seat_of(&l, &line, agents[i].agent_id);
    let chair = seat(1);
    assert_eq!(chair.room_id, boss(&l), "the first sits at your desk");
    assert!(is_guest_chair(chair), "{chair:?}");
    let (s2, s3) = (seat(0), seat(2));
    assert_eq!((s2.room_id, s3.room_id), (boss(&l), boss(&l)));
    assert_eq!(
        (s2.kind, s3.kind),
        (WaypointKind::MeetingSofa, WaypointKind::MeetingSofa)
    );
    assert!(
        (s2.pos.y, s2.pos.x) < (s3.pos.y, s3.pos.x),
        "the earlier waiter first along the sofas"
    );
}

/// agent-office: a prompt holds its session mid-turn, while a question to
/// you comes from one whose turn is done; so a prompt takes the guest chair
/// ahead of a question asked earlier, and questions keep their own order.
#[test]
fn a_prompt_goes_ahead_of_a_question() {
    let l = office();
    let agents = [
        asking("old question", 0, 300_000),
        waiter("new prompt", 1, 25_000),
        asking("new question", 2, 60_000),
    ];
    let line = waiting_line(&agents, t0(), &l);
    let seat = |i: usize| seat_of(&l, &line, agents[i].agent_id);
    assert!(is_guest_chair(seat(1)), "the prompt sits at your desk");
    let (a, b) = (seat(0).pos, seat(2).pos);
    assert!(
        (a.y, a.x) < (b.y, b.x),
        "the older question first along the sofas"
    );
}

/// agent-office: a blocked session must not be overtaken by a result, which
/// only waits to be read: prompts, then questions, then results, however
/// long the result has waited.
#[test]
fn a_result_to_read_queues_behind_approvals_and_questions() {
    let l = office();
    let agents = [
        done("old result", 0, 600_000),
        asking("question", 1, 30_000),
        waiter("prompt", 2, 21_000),
    ];
    let line = waiting_line(&agents, t0(), &l);
    let seat = |i: usize| seat_of(&l, &line, agents[i].agent_id);
    assert!(is_guest_chair(seat(2)), "the prompt in the guest chair");
    let (q, r) = (seat(1), seat(0));
    assert_eq!(
        (q.kind, r.kind),
        (WaypointKind::MeetingSofa, WaypointKind::MeetingSofa)
    );
    assert!(
        (q.pos.y, q.pos.x) < (r.pos.y, r.pos.x),
        "the question before the result"
    );
    let mut keys: Vec<_> = agents.iter().map(|s| line_key(s, t0())).collect();
    keys.sort();
    assert_eq!(
        keys.iter().map(|k| k.0).collect::<Vec<_>>(),
        [Some(Need::Approval), Some(Need::Reply), Some(Need::Read)]
    );
}

/// agent-office: the guest chair means "the one `n` opens" only if the line
/// never reaches the chair across the desk, yours: every seat it takes is in
/// your office, and none faces the desk from the east.
#[test]
fn your_own_chair_stays_empty() {
    let l = office();
    let seats = seats(&l);
    assert_eq!(seats.len(), 7, "the guest chair and two sofas of three");
    assert!(is_guest_chair(l.waypoints[seats[0]]));
    for &i in &seats {
        let w = l.waypoints[i];
        assert_eq!(w.room_id, boss(&l), "{w:?}");
        assert!(
            !(w.kind == WaypointKind::MeetingChair && w.facing == Facing::West),
            "your own chair taken: {w:?}"
        );
    }
    let agents: Vec<AgentSlot> = (0..12)
        .map(|i| waiter(&format!("w{i}"), i, 30_000 + i as u64 * 1000))
        .collect();
    let line = waiting_line(&agents, t0(), &l);
    for t in line.values() {
        if let WanderKind::Named { wp_idx, .. } = t.kind {
            let w = l.waypoints[wp_idx];
            assert!(!(w.kind == WaypointKind::MeetingChair && w.facing == Facing::West));
        }
    }
}

/// With the seats taken, the rest still line up, standing in the corridor
/// outside your office west of its door, earlier nearer the door, rather
/// than go uncounted or block the doorway.
#[test]
fn those_past_the_seats_stand_in_line() {
    let l = office();
    let agents: Vec<AgentSlot> = (0..12)
        .map(|i| waiter(&format!("w{i}"), i, 30_000 + (12 - i) as u64 * 1000))
        .collect();
    let line = waiting_line(&agents, t0(), &l);
    assert_eq!(line.len(), agents.len(), "everyone waiting has a place");
    let room = l.meeting_rooms[boss(&l).expect("an office")].bounds;
    let door_x = room.x + crate::layout::pct(room.width, crate::layout::OFFICE_DOOR_PCT);
    let standing: Vec<Point> = agents
        .iter()
        .filter(|s| line[&s.agent_id].kind == WanderKind::Aimless)
        .map(|s| line[&s.agent_id].dest)
        .collect();
    assert_eq!(standing.len(), agents.len() - 7, "the seats run out at seven");
    for p in &standing {
        assert!(
            (l.corridor.y..room.y).contains(&p.y),
            "in the corridor, not in a room: {p:?} {room:?}"
        );
        assert!(p.x < door_x, "west of the door: {p:?} door {door_x}");
    }
    assert!(
        standing.windows(2).all(|w| w[0].x > w[1].x),
        "the earlier nearer the door: {standing:?}"
    );
}

/// A waiting agent walks to its seat and sits, then once approved walks back
/// to its desk rather than popping there.
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
            Some(Pose::AtWaypoint { .. }) => {
                assert_eq!(rig.walks[&slot.agent_id].wander.target.dest, place);
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

/// agent-office: a result you read in your office walks from its seat
/// straight to the pantry, frame by frame, with no detour to its desk and no
/// jump: walking home first would read as starting work again.
#[test]
fn a_result_read_in_your_office_walks_to_the_pantry_not_home() {
    let l = office();
    let mut slot = done("reader", 0, DONE_LINE_UP_AFTER_MS);
    let mut rig = RouteRig::new(crate::pathfind::AStarRouter::new());
    let mut now = t0();
    let mut seated = None;
    for _ in 0..600 {
        now += Duration::from_millis(100);
        rig.meetings = waiting_line(std::slice::from_ref(&slot), now, &l);
        if let Some(Pose::AtWaypoint { wp, .. }) =
            derive_with_routing(&slot, now, &l, &mut rig.rctx())
        {
            seated = Some(wp);
            break;
        }
    }
    let seat = seated.expect("never sat down in your office");
    assert!(crate::team::in_your_office(&l, seat));

    // Read: the host backdates it past the line's wait, so it leaves the line.
    slot.state_started_at = now - Duration::from_millis(DONE_WAITS_MS);
    let mut last = l.waypoints[seat].pos;
    let mut walked = false;
    let mut rested = None;
    for _ in 0..1200 {
        now += Duration::from_millis(100);
        rig.meetings = waiting_line(std::slice::from_ref(&slot), now, &l);
        assert!(rig.meetings.is_empty(), "read, it is out of the line");
        let pose = derive_with_routing(&slot, now, &l, &mut rig.rctx());
        assert!(
            rig.walks[&slot.agent_id].lifted.is_none(),
            "set down to walk home first"
        );
        match pose {
            Some(Pose::Walking { segment, .. }) => {
                let at = segment.at();
                let step = last.x.abs_diff(at.x).max(last.y.abs_diff(at.y));
                assert!(step <= 6, "jumped {step} from {last:?} to {at:?}");
                last = at;
                walked = true;
            }
            Some(Pose::AtWaypoint { wp, kind }) if walked => {
                rested = Some(kind);
                assert!(!crate::team::in_your_office(&l, wp));
                break;
            }
            Some(Pose::AtWaypoint { wp, .. }) => assert_eq!(wp, seat, "still seated"),
            p => panic!("neither walking nor seated in a lounge: {p:?}"),
        }
    }
    assert!(
        matches!(rested, Some(WaypointKind::Pantry | WaypointKind::Island)),
        "ends in the pantry: {rested:?}"
    );
}

/// Walk `slot` into the line until it sits; its seat.
fn seat_in_line(
    slot: &AgentSlot,
    l: &SceneLayout,
    rig: &mut RouteRig<crate::pathfind::AStarRouter>,
    now: &mut SystemTime,
) -> usize {
    for _ in 0..600 {
        *now += Duration::from_millis(100);
        rig.meetings = waiting_line(std::slice::from_ref(slot), *now, l);
        if let Some(Pose::AtWaypoint { wp, .. }) = derive_with_routing(slot, *now, l, &mut rig.rctx())
        {
            return wp;
        }
    }
    panic!("never sat down in the line");
}

/// Frames on, `slot` still sits at `seat` and was never set down to walk
/// home: what it needs from you never let it go.
fn stays_seated(
    slot: &AgentSlot,
    l: &SceneLayout,
    rig: &mut RouteRig<crate::pathfind::AStarRouter>,
    mut now: SystemTime,
    seat: usize,
    frames: usize,
) {
    for i in 0..frames {
        now += Duration::from_millis(100);
        rig.meetings = waiting_line(std::slice::from_ref(slot), now, l);
        let pose = derive_with_routing(slot, now, l, &mut rig.rctx());
        assert!(rig.walks[&slot.agent_id].lifted.is_none(), "set down, frame {i}");
        assert!(
            matches!(pose, Some(Pose::AtWaypoint { wp, .. }) if wp == seat),
            "left its seat, frame {i}: {pose:?}"
        );
    }
}

/// agent-office: a result whose summary comes back a question changes what
/// it needs, not whether: it keeps its seat frame by frame rather than walk
/// home and back, so the line does not churn as summaries land.
#[test]
fn a_result_that_turns_out_a_question_keeps_its_seat() {
    let l = office();
    let mut slot = done("asker", 0, DONE_LINE_UP_AFTER_MS);
    let mut rig = RouteRig::new(crate::pathfind::AStarRouter::new());
    let mut now = t0();
    let seat = seat_in_line(&slot, &l, &mut rig, &mut now);
    slot.state = ActivityState::RequiresAction {
        reason: Some(REPLY_REASON.into()),
    };
    assert!(lines_up(&slot, now + Duration::from_millis(100)), "still in line");
    stays_seated(&slot, &l, &mut rig, now, seat, 100);
}

/// agent-office: a prompt turned down ends the turn, so the session needs
/// you again as a result a few seconds on; it keeps its seat in between
/// rather than walk home and straight back.
#[test]
fn a_prompt_turned_down_keeps_its_seat() {
    let l = office();
    let mut slot = waiter("refused", 0, LINE_UP_AFTER_MS);
    let mut rig = RouteRig::new(crate::pathfind::AStarRouter::new());
    let mut now = t0();
    let seat = seat_in_line(&slot, &l, &mut rig, &mut now);
    slot.state = ActivityState::Idle;
    slot.state_started_at = now;
    assert!(!lines_up(&slot, now + Duration::from_millis(100)), "out of the line a moment");
    stays_seated(&slot, &l, &mut rig, now, seat, 60);
}

/// agent-office: only a seat in your office lets one out straight to rest;
/// a team's meeting breaking up still walks its members home first, as it
/// always has.
#[test]
fn a_meeting_let_out_still_walks_home_first() {
    let l = office();
    let slot = done("member", 0, DONE_WAITS_MS + 60_000);
    let desk = l.home_desk(slot.desk_index.single_floor_local()).expect("desk");
    let seat = (0..l.waypoints.len())
        .filter(|&i| crate::team::is_team_seat(&l, i))
        .find_map(|i| crate::team::seat_target(&l, i, desk))
        .expect("a meeting seat");
    let mut rig = RouteRig::new(crate::pathfind::AStarRouter::new());
    rig.meetings.insert(slot.agent_id, seat);
    let mut now = t0();
    let mut sat = false;
    for _ in 0..600 {
        now += Duration::from_millis(100);
        if matches!(
            derive_with_routing(&slot, now, &l, &mut rig.rctx()),
            Some(Pose::AtWaypoint { .. })
        ) {
            sat = true;
            break;
        }
    }
    assert!(sat, "never sat down at the meeting");
    rig.meetings.clear();
    now += Duration::from_millis(100);
    derive_with_routing(&slot, now, &l, &mut rig.rctx());
    assert!(
        rig.walks[&slot.agent_id].lifted.is_some(),
        "set down to walk home, as before"
    );
}
