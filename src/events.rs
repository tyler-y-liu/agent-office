//! Ledger of `--submit` sessions and the one-line events the office writes
//! about them. Both files live beside the hook socket. They are not rotated:
//! the office truncates the event log the first time it looks, and deleting
//! either file drops what it held.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pixtuoid_core::{AgentSlot, SceneState};
use serde::{Deserialize, Serialize};

use crate::tasks::{TaskView, stuck_at_work};

/// How much of a task the ledger keeps. The file is for a person reading one
/// line per dispatch; the prompt itself can be a whole brief.
const TASK_CELLS: u16 = 160;

const STARTED: &str = "开始了";
const DONE: &str = "做完了";

/// One `--submit` that reached a host. The office emits events only for these
/// session ids, not for subagents they later start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerRow {
    pub session_id: String,
    pub host: String,
    pub role: String,
    pub dir: String,
    pub task: String,
}

impl LedgerRow {
    pub fn new(session_id: &str, host: &str, role: &str, dir: &str, task: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            host: host.to_owned(),
            role: role.to_owned(),
            dir: dir.to_owned(),
            task: task_line(task),
        }
    }
}

/// `task` as one short line: newlines would still be one JSON string, but a
/// person reading the file would see a wall of escaped breaks.
pub fn task_line(task: &str) -> String {
    crate::text::truncate_cells(&crate::text::one_line(task), TASK_CELLS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Started,
    Progress,
    Stuck,
    Done,
}

/// One line of `events.jsonl`. `host`, `role` and `dir` are the ledger's, so
/// the event names the dispatch Hermes made even when the figure's own source
/// or directory differs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeEvent {
    pub kind: Kind,
    pub host: String,
    pub role: String,
    pub dir: String,
    pub session_id: String,
    pub text: String,
}

/// What this process has already emitted, plus the last status and stuck
/// sentences, so a tick that changes nothing writes nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sent {
    sessions: HashMap<String, Tracked>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Tracked {
    started: bool,
    done: bool,
    /// Last `panel::status` sentence. `None` until the session has been seen,
    /// so the first sight is `started` and not also a progress line.
    status: Option<String>,
    /// Last `Stuck::describe`. `None` means it was not stuck.
    stuck: Option<String>,
    /// `(kind, text)` already written. The same pair is not written again.
    sent: HashSet<(Kind, String)>,
}

fn state_dir() -> PathBuf {
    crate::hook::socket_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn ledger_path() -> PathBuf {
    state_dir().join("dispatched.jsonl")
}

pub fn events_path() -> PathBuf {
    state_dir().join("events.jsonl")
}

pub fn cursor_log_path(session_id: &str) -> PathBuf {
    state_dir().join(format!("cursor-{session_id}.log"))
}

/// Drop the event log. A restart has an empty in-memory [`Sent`], so leaving
/// the old lines would mix a previous run's events with a fresh `started`.
pub fn reset_log(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Ok(());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn append_ledger(path: &Path, row: &LedgerRow) -> std::io::Result<()> {
    let line = serde_json::to_string(row).map_err(std::io::Error::other)?;
    append_private(path, &line)
}

pub fn append_events(path: &Path, events: &[OfficeEvent]) -> std::io::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let mut buf = String::new();
    for event in events {
        buf.push_str(&serde_json::to_string(event).map_err(std::io::Error::other)?);
        buf.push('\n');
    }
    append_private(path, &buf)
}

fn append_private(path: &Path, line: &str) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `mode` applies only when the file is created.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    if line.is_empty() {
        return Ok(());
    }
    if line.ends_with('\n') {
        file.write_all(line.as_bytes())
    } else {
        writeln!(file, "{line}")
    }
}

/// Rows in the ledger. A missing file is an empty ledger: the office is up
/// before anyone has dispatched. A line that is not JSON is reported and
/// skipped so one bad line does not hide the sessions around it.
pub fn read_ledger(path: &Path) -> Vec<LedgerRow> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("agent-office: 读派活台账失败：{e}");
            return Vec::new();
        }
    };
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LedgerRow>(line) {
            Ok(row) => rows.push(row),
            Err(e) => eprintln!("agent-office: 派活台账第 {} 行读不了：{e}", i + 1),
        }
    }
    rows
}

/// New events for the ledger's sessions. `read` is false: this is whether the
/// work entered [`crate::panel::Group::Done`], not whether someone opened the
/// card. The updated [`Sent`] is returned with the events so the next tick
/// can see a status that was recorded without being written.
pub fn observe(
    scene: &SceneState,
    views: &HashMap<pixtuoid_core::AgentId, TaskView>,
    ledger: &[LedgerRow],
    sent: &Sent,
    now: SystemTime,
) -> (Vec<OfficeEvent>, Sent) {
    let mut sent = sent.clone();
    let mut out = Vec::new();
    for row in ledger {
        let Some(slot) = session_slot(scene, &row.session_id) else {
            continue;
        };
        let view = views.get(&slot.agent_id);
        let tracked = sent.sessions.entry(row.session_id.clone()).or_default();
        if !tracked.started {
            push(&mut out, tracked, row, Kind::Started, STARTED);
            tracked.started = true;
        }
        let (status, _) = crate::panel::status(slot, view, false, now);
        if tracked.status.as_ref() != Some(&status) {
            // The first sentence is the baseline. A progress line is a change
            // after that, including 「等你确认」.
            if tracked.status.is_some() {
                push(&mut out, tracked, row, Kind::Progress, &status);
            }
            tracked.status = Some(status);
        }
        let stuck_now = stuck_at_work(slot, view).map(|s| s.describe());
        if let Some(text) = &stuck_now
            && tracked.stuck.as_ref() != Some(text)
        {
            push(&mut out, tracked, row, Kind::Stuck, text);
        }
        tracked.stuck = stuck_now;
        let entered_done = tracked_done(slot, view, now);
        if entered_done && !tracked.done {
            push(&mut out, tracked, row, Kind::Done, DONE);
            tracked.done = true;
        }
    }
    (out, sent)
}

fn tracked_done(slot: &AgentSlot, view: Option<&TaskView>, now: SystemTime) -> bool {
    slot.exiting_at.is_some()
        || crate::panel::Group::of(slot, view, now, false) == crate::panel::Group::Done
}

/// The dispatched session, not a subagent that happened to reuse its id.
fn session_slot<'a>(scene: &'a SceneState, session_id: &str) -> Option<&'a AgentSlot> {
    let mut found: Option<&AgentSlot> = None;
    for slot in scene.agents.values() {
        if slot.session_id.as_ref() != session_id {
            continue;
        }
        match found {
            Some(prev) if prev.parent_id.is_none() => {}
            _ => found = Some(slot),
        }
    }
    found
}

fn push(
    out: &mut Vec<OfficeEvent>,
    tracked: &mut Tracked,
    row: &LedgerRow,
    kind: Kind,
    text: &str,
) {
    if !tracked.sent.insert((kind, text.to_owned())) {
        return;
    }
    out.push(OfficeEvent {
        kind,
        host: row.host.clone(),
        role: row.role.clone(),
        dir: row.dir.clone(),
        session_id: row.session_id.clone(),
        text: text.to_owned(),
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pixtuoid_core::source::{AgentEvent, Transport};
    use pixtuoid_core::{ActivityState, AgentId, Reducer};

    use super::*;
    use crate::pipeline::CC_SOURCE;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agent-office-events-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn add(
        scene: &mut SceneState,
        reducer: &mut Reducer,
        sid: &str,
        parent: Option<AgentId>,
    ) -> AgentId {
        let id = AgentId::from_parts(CC_SOURCE, sid);
        reducer.apply(
            scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: CC_SOURCE.into(),
                session_id: sid.into(),
                cwd: "/repo".into(),
                parent_id: parent,
            },
            at(),
            Transport::Hook,
        );
        id
    }

    fn row(sid: &str) -> LedgerRow {
        LedgerRow::new(sid, "dsh", "backend", "/abs", "写接口")
    }

    fn kinds(events: &[OfficeEvent]) -> Vec<Kind> {
        events.iter().map(|e| e.kind).collect()
    }

    /// Dispatch and the office are different processes. This file is the only
    /// thing that joins them: a dropped field, or a task that breaks the line,
    /// would start work the office never reports.
    #[test]
    fn the_ledger_is_one_json_object_per_dispatch_and_reads_back() {
        let dir = temp_dir("ledger");
        let path = dir.join("dispatched.jsonl");
        assert!(
            read_ledger(&path).is_empty(),
            "no ledger yet is no sessions, not an error"
        );
        let row = LedgerRow::new("sid-1", "claude", "backend", "/abs", "第一行\n第二行");
        assert_eq!(row.task, "第一行 第二行");
        assert!(!row.task.contains('\n'));
        let long = task_line(&"a".repeat(500));
        assert!(long.ends_with('…') && crate::text::cells(&long) <= TASK_CELLS);
        append_ledger(&path, &row).unwrap();
        append_ledger(
            &path,
            &LedgerRow::new("sid-2", "dsh", "qa", "/abs", "跑测试"),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2, "one dispatch, one line");
        let got = read_ledger(&path);
        assert_eq!(got[0].session_id, "sid-1");
        assert_eq!(got[0].host, "claude");
        assert_eq!(got[0].role, "backend");
        assert_eq!(got[0].dir, "/abs");
        assert_eq!(got[0].task, "第一行 第二行");
        assert_eq!(got[1].session_id, "sid-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A subagent the session starts has its own id and is not in the ledger.
    /// Emitting for it would tell Hermes a second worker had started.
    #[test]
    fn a_session_outside_the_ledger_emits_nothing_including_its_subagent() {
        let now = at();
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let parent = add(&mut scene, &mut reducer, "parent", None);
        let child = add(&mut scene, &mut reducer, "child", Some(parent));
        let other = add(&mut scene, &mut reducer, "other", None);
        for id in [parent, child, other] {
            scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
        }
        let views = HashMap::new();
        let (events, _) = observe(&scene, &views, &[row("parent")], &Sent::default(), now);
        assert_eq!(
            events
                .iter()
                .map(|e| e.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["parent"]
        );
        assert!(events.iter().all(|e| e.kind == Kind::Started));
    }

    /// The first time the office sees the session is the start signal. The
    /// status it already has is the baseline, not a progress line Hermes
    /// would treat as news from before the start.
    #[test]
    fn the_first_sight_of_a_ledger_session_is_started() {
        let now = at();
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let id = add(&mut scene, &mut reducer, "sid", None);
        scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
        let (events, sent) = observe(
            &scene,
            &HashMap::new(),
            &[row("sid")],
            &Sent::default(),
            now,
        );
        assert_eq!(kinds(&events), vec![Kind::Started]);
        assert_eq!(events[0].text, "开始了");
        assert_eq!(events[0].host, "dsh");
        assert_eq!(events[0].role, "backend");
        assert_eq!(events[0].dir, "/abs");
        assert_eq!(events[0].session_id, "sid");
        let wire = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(wire["kind"], "started");
        let (again, _) = observe(&scene, &HashMap::new(), &[row("sid")], &sent, now);
        assert!(again.is_empty(), "the same sight is not another start");
    }

    /// Hermes pings on progress. The sentence is the panel's own, including
    /// 「等你确认」, and the same sentence twice would ping twice for one wait.
    #[test]
    fn a_changed_status_sentence_is_progress_and_is_not_repeated() {
        let now = at();
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let id = add(&mut scene, &mut reducer, "sid", None);
        scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
        let (events, sent) = observe(
            &scene,
            &HashMap::new(),
            &[row("sid")],
            &Sent::default(),
            now,
        );
        assert_eq!(kinds(&events), vec![Kind::Started]);

        scene.agents.get_mut(&id).unwrap().state = ActivityState::RequiresAction { reason: None };
        let (status, _) = crate::panel::status(scene.agents.get(&id).unwrap(), None, false, now);
        assert!(
            status.contains("等你确认"),
            "the fixture must be the approval sentence, got {status}"
        );
        let (events, sent) = observe(&scene, &HashMap::new(), &[row("sid")], &sent, now);
        assert_eq!(kinds(&events), vec![Kind::Progress]);
        assert_eq!(events[0].text, status);

        let (events, sent) = observe(&scene, &HashMap::new(), &[row("sid")], &sent, now);
        assert!(events.is_empty(), "the same sentence is not progress again");

        scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
        let (events, sent) = observe(&scene, &HashMap::new(), &[row("sid")], &sent, now);
        assert_eq!(kinds(&events), vec![Kind::Progress]);
        scene.agents.get_mut(&id).unwrap().state = ActivityState::RequiresAction { reason: None };
        let (events, _) = observe(&scene, &HashMap::new(), &[row("sid")], &sent, now);
        assert!(
            events.is_empty(),
            "「等你确认」 was already sent; a return to it is not a new wait"
        );
    }

    /// Stuck is the verdict the card already shows. A new sentence here would
    /// be a second definition of stuck.
    #[test]
    fn stuck_at_work_emits_the_existing_stuck_sentence() {
        let now = at();
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let id = add(&mut scene, &mut reducer, "sid", None);
        scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
        let (events, sent) = observe(
            &scene,
            &HashMap::new(),
            &[row("sid")],
            &Sent::default(),
            now,
        );
        assert_eq!(kinds(&events), vec![Kind::Started]);

        let view = TaskView {
            stuck: Some(crate::tasks::Stuck::Failing(3)),
            ..TaskView::default()
        };
        let views = HashMap::from([(id, view.clone())]);
        assert!(stuck_at_work(scene.agents.get(&id).unwrap(), Some(&view)).is_some());
        let (events, sent) = observe(&scene, &views, &[row("sid")], &sent, now);
        let stuck: Vec<_> = events.iter().filter(|e| e.kind == Kind::Stuck).collect();
        assert_eq!(
            stuck.len(),
            1,
            "stuck_at_work becoming Some is one stuck: {events:?}"
        );
        assert_eq!(stuck[0].text, crate::tasks::Stuck::Failing(3).describe());

        let (events, _) = observe(&scene, &views, &[row("sid")], &sent, now);
        assert!(
            events.is_empty(),
            "the same stuck sentence is not sent again"
        );
    }

    /// Done is the first moment the work is finished, not the end of the 15
    /// minutes `Group::Done` keeps the card up, and not a second time when
    /// the figure then leaves. An idle session that never ran is not done.
    #[test]
    fn entering_done_or_exiting_emits_done_once() {
        let now = at();
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let idle = add(&mut scene, &mut reducer, "idle", None);
        scene.agents.get_mut(&idle).unwrap().state = ActivityState::Idle;
        scene.agents.get_mut(&idle).unwrap().state_started_at = now;
        let (events, sent) = observe(
            &scene,
            &HashMap::new(),
            &[row("idle")],
            &Sent::default(),
            now,
        );
        assert_eq!(
            kinds(&events),
            vec![Kind::Started],
            "idle without a prompt never ran, so it is not done"
        );

        let view = TaskView {
            prompt: Some("写接口".into()),
            ..TaskView::default()
        };
        let views = HashMap::from([(idle, view.clone())]);
        assert_eq!(
            crate::panel::Group::of(scene.agents.get(&idle).unwrap(), Some(&view), now, false),
            crate::panel::Group::Done,
            "the fixture must be inside the done window at once, not after it"
        );
        let (events, sent) = observe(&scene, &views, &[row("idle")], &sent, now);
        let done: Vec<_> = events.iter().filter(|e| e.kind == Kind::Done).collect();
        assert_eq!(
            done.len(),
            1,
            "entering Group::Done is one done: {events:?}"
        );
        assert_eq!(done[0].text, "做完了");
        let (events, _) = observe(&scene, &views, &[row("idle")], &sent, now);
        assert!(
            events.iter().all(|e| e.kind != Kind::Done),
            "done is once: {events:?}"
        );

        let leaving = add(&mut scene, &mut reducer, "leaving", None);
        let slot = scene.agents.get_mut(&leaving).unwrap();
        slot.state = ActivityState::Running { item: None };
        slot.exiting_at = Some(now);
        assert_ne!(
            crate::panel::Group::of(scene.agents.get(&leaving).unwrap(), None, now, false),
            crate::panel::Group::Done,
            "exiting while it still runs is not Group::Done; the event is the other arm"
        );
        let (events, sent) = observe(
            &scene,
            &HashMap::new(),
            &[row("leaving")],
            &Sent::default(),
            now,
        );
        assert_eq!(kinds(&events), vec![Kind::Started, Kind::Done]);
        assert_eq!(events[1].text, "做完了");
        let (events, _) = observe(&scene, &HashMap::new(), &[row("leaving")], &sent, now);
        assert!(events.iter().all(|e| e.kind != Kind::Done));
    }

    /// Hermes reads every kind, not only `started`. A progress, stuck or done
    /// line that dropped the ledger's host, role or directory would attach the
    /// update to the wrong dispatch.
    #[test]
    fn every_kind_names_the_dispatch_it_came_from() {
        let now = at();
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let id = add(&mut scene, &mut reducer, "sid", None);
        scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
        let ledger = [row("sid")];
        let (events, sent) = observe(&scene, &HashMap::new(), &ledger, &Sent::default(), now);
        assert_eq!(kinds(&events), vec![Kind::Started]);
        assert_names_dispatch(&events[0], "sid");

        scene.agents.get_mut(&id).unwrap().state = ActivityState::RequiresAction { reason: None };
        let (events, sent) = observe(&scene, &HashMap::new(), &ledger, &sent, now);
        let progress: Vec<_> = events.iter().filter(|e| e.kind == Kind::Progress).collect();
        assert_eq!(progress.len(), 1, "{events:?}");
        assert_names_dispatch(progress[0], "sid");
        assert!(
            progress[0].text.contains("等你确认"),
            "{}",
            progress[0].text
        );

        scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
        let view = TaskView {
            stuck: Some(crate::tasks::Stuck::Failing(3)),
            ..TaskView::default()
        };
        let (events, _) = observe(&scene, &HashMap::from([(id, view)]), &ledger, &sent, now);
        let stuck: Vec<_> = events.iter().filter(|e| e.kind == Kind::Stuck).collect();
        assert_eq!(stuck.len(), 1, "{events:?}");
        assert_names_dispatch(stuck[0], "sid");
        assert_eq!(stuck[0].text, crate::tasks::Stuck::Failing(3).describe());

        let idle = add(&mut scene, &mut reducer, "idle", None);
        let slot = scene.agents.get_mut(&idle).unwrap();
        slot.state = ActivityState::Idle;
        slot.state_started_at = now;
        let (events, sent) = observe(
            &scene,
            &HashMap::new(),
            &[row("idle")],
            &Sent::default(),
            now,
        );
        assert_eq!(kinds(&events), vec![Kind::Started]);
        let done_view = TaskView {
            prompt: Some("写接口".into()),
            ..TaskView::default()
        };
        let (events, _) = observe(
            &scene,
            &HashMap::from([(idle, done_view)]),
            &[row("idle")],
            &sent,
            now,
        );
        let done: Vec<_> = events.iter().filter(|e| e.kind == Kind::Done).collect();
        assert_eq!(done.len(), 1, "{events:?}");
        assert_names_dispatch(done[0], "idle");
        assert_eq!(done[0].text, "做完了");
    }

    fn assert_names_dispatch(event: &OfficeEvent, session_id: &str) {
        let wire = serde_json::to_value(event).unwrap();
        assert_eq!(wire["host"], "dsh");
        assert_eq!(wire["role"], "backend");
        assert_eq!(wire["dir"], "/abs");
        assert_eq!(wire["session_id"], session_id);
        assert_eq!(wire["text"], event.text);
        assert!(!event.text.is_empty());
        assert_eq!(wire["kind"], serde_json::to_value(event.kind).unwrap());
        for key in ["kind", "host", "role", "dir", "session_id", "text"] {
            assert!(wire.get(key).is_some(), "missing {key}: {wire}");
        }
    }

    /// The office and Hermes are different processes. The jsonl file is the
    /// only copy of an event: a missing break would glue two dispatches into
    /// one line Hermes cannot parse.
    #[test]
    fn appended_events_are_one_json_object_per_line() {
        let dir = temp_dir("events-file");
        let path = dir.join("events.jsonl");
        let events = [
            OfficeEvent {
                kind: Kind::Started,
                host: "claude".into(),
                role: "backend".into(),
                dir: "/abs".into(),
                session_id: "sid".into(),
                text: "开始了".into(),
            },
            OfficeEvent {
                kind: Kind::Done,
                host: "claude".into(),
                role: "backend".into(),
                dir: "/abs".into(),
                session_id: "sid".into(),
                text: "做完了".into(),
            },
        ];
        append_events(&path, &events).unwrap();
        append_events(&path, &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        for (line, event) in text.lines().zip(events.iter()) {
            let got: OfficeEvent = serde_json::from_str(line).unwrap();
            assert_eq!(&got, event);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
