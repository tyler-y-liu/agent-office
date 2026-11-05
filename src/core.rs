//! What the window acts on, apart from how it draws: the live scene, the task
//! lines, the selection, and what each key does to them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::{AgentId, SceneState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::fun::{self, Clock, hko::Outlook, stats::Watch};
use crate::panel::{self, PanelState};
use crate::{jump, notify, pipeline, tasks, text};

/// An office that has floors to move between.
pub trait Floors {
    fn current_floor(&self) -> usize;
    fn n_floors(&self) -> usize;
    fn navigate(&mut self, floor: usize, now: SystemTime);
}

/// What a key asks for, whichever front end read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Back,
    Down,
    Up,
    Expand,
    Resummarize,
    ToggleLabels,
    NextFloor,
    PrevFloor,
    /// Open the selected agent's session in the desktop app.
    Open,
    /// Open the desktop app's list of sessions waiting on the user.
    OpenWaiting,
    /// Open the session at the head of the waiting line.
    OpenNext,
}

/// The footer's warning while Claude Code's hooks are out.
const HOOKS_OUT: &str = "未接 hooks，看不到等你确认：agent-office connect";

/// How long a note on what a key did stays in the footer.
const NOTE_FOR: Duration = Duration::from_secs(5);

pub struct Core {
    pub ui: PanelState,
    pub head_labels: bool,
    pub summary_note: String,
    /// Claude Code's hooks are in, as of the start: without them no prompt
    /// is seen, and your office stays empty however many wait.
    hooks: bool,
    /// What the last jump did, shown in place of `summary_note` a while.
    note: Option<(String, Instant)>,
    pub pipe: pipeline::Pipeline,
    views_rx: tokio::sync::watch::Receiver<tasks::TaskViews>,
    force_tx: tokio::sync::mpsc::UnboundedSender<AgentId>,
    /// The day's tally, the repositories' records and the boss's comings.
    pub watch: Watch,
    outlook_rx: tokio::sync::watch::Receiver<Outlook>,
    notifier: notify::Notifier,
    banners: bool,
}

/// One frame's worth of state.
pub struct Frame {
    pub scene: Arc<SceneState>,
    /// `scene` as the office draws it ([`as_seen`]).
    pub office: Arc<SceneState>,
    pub views: tasks::TaskViews,
    /// The head label per agent; empty while they are hidden.
    pub labels: HashMap<AgentId, String>,
    /// The office's clock now.
    pub clock: Clock,
    /// Hong Kong's weather, when the user asked for it.
    pub outlook: Outlook,
}

/// `scene` as the office draws it: a session you opened since it went idle
/// ([`panel::read_since_idle`]) has had its result read, so it idles as one
/// past the pantry, out on the rooftop; its idle stretch is drawn as starting
/// [`IDLE_ROOFTOP_MS`](pixtuoid_scene::pose::IDLE_ROOFTOP_MS) ago at least.
/// While `in_pantry` (lunch, or a typhoon closing the rooftop) every idle
/// agent is drawn in the pantry, the cha chaan teng, instead: its idle
/// stretch is drawn a minute short of the rooftop's at most. The panel keeps
/// the real start for its "空闲 Xm". A done session whose turn ended on a
/// question to you waits on you as a prompt does, so it is drawn as one,
/// lining up behind the prompts from when it asked, lunch or not.
pub fn as_seen(
    scene: &Arc<SceneState>,
    views: &tasks::TaskViews,
    seen: &HashMap<AgentId, SystemTime>,
    in_pantry: bool,
    now: SystemTime,
) -> Arc<SceneState> {
    let asking: Vec<AgentId> = scene
        .agents
        .values()
        .filter(|s| {
            matches!(s.state, pixtuoid_core::ActivityState::Idle)
                && panel::waits_on_you(s, views.get(&s.agent_id), panel::read_since_idle(s, seen))
                    == Some(panel::Wait::Reply)
        })
        .map(|s| s.agent_id)
        .collect();
    let rooftop = Duration::from_millis(pixtuoid_scene::pose::IDLE_ROOFTOP_MS);
    let ago = |d: Duration| now.checked_sub(d).unwrap_or(SystemTime::UNIX_EPOCH);
    let (moved, start): (Vec<AgentId>, SystemTime) = if in_pantry {
        let last_pantry = ago(rooftop - PANTRY_MARGIN);
        let ids = scene
            .agents
            .values()
            .filter(|s| s.state == pixtuoid_core::ActivityState::Idle && s.exiting_at.is_none())
            .filter(|s| s.state_started_at < last_pantry)
            // Its wait is timed from when it asked, which the line orders by.
            .filter(|s| !asking.contains(&s.agent_id))
            .map(|s| s.agent_id)
            .collect();
        (ids, last_pantry)
    } else {
        let ids = scene
            .agents
            .values()
            .filter(|s| panel::read_since_idle(s, seen))
            .map(|s| s.agent_id)
            .collect();
        (ids, ago(rooftop))
    };
    if moved.is_empty() && asking.is_empty() {
        return Arc::clone(scene);
    }
    let mut drawn = (**scene).clone();
    for id in moved {
        if let Some(s) = drawn.agents.get_mut(&id) {
            s.state_started_at = if in_pantry {
                start
            } else {
                s.state_started_at.min(start)
            };
        }
    }
    for id in asking {
        if let Some(s) = drawn.agents.get_mut(&id) {
            s.state = pixtuoid_core::ActivityState::RequiresAction {
                reason: Some(pixtuoid_scene::waiting::REPLY_REASON.into()),
            };
        }
    }
    Arc::new(drawn)
}

/// `scene` with each idle session's idle stretch starting no later than its
/// transcript's last write. A session found already open when agent-office
/// starts gets a fresh slot that reads idle since launch, however long ago it
/// last did anything; the transcript knows. While it runs, the last write
/// trails the turn's end, so a live session is left as it is.
pub fn idle_since_written(scene: &Arc<SceneState>, views: &tasks::TaskViews) -> Arc<SceneState> {
    let stale: Vec<(AgentId, SystemTime)> = scene
        .agents
        .values()
        .filter(|s| matches!(s.state, pixtuoid_core::ActivityState::Idle))
        .filter_map(|s| {
            let at = views.get(&s.agent_id)?.written_at?;
            (at < s.state_started_at).then_some((s.agent_id, at))
        })
        .collect();
    if stale.is_empty() {
        return Arc::clone(scene);
    }
    let mut fixed = (**scene).clone();
    for (id, at) in stale {
        if let Some(s) = fixed.agents.get_mut(&id) {
            s.state_started_at = at;
        }
    }
    Arc::new(fixed)
}

/// How far short of the rooftop's an idle stretch held in the pantry is
/// drawn: enough that a frame's lag never tips it over.
const PANTRY_MARGIN: Duration = Duration::from_secs(60);

impl Core {
    /// Start the sources and the task loop on `rt`, the floors seated for an
    /// office `buf_w`×`buf_h`.
    pub fn start(rt: &tokio::runtime::Runtime, buf_w: u16, buf_h: u16) -> Self {
        let summarizer = tasks::client::Summarizer::from_env();
        let summary_note = match &summarizer {
            Some(s) => format!("摘要 {}", s.model()),
            None => "摘要关闭".to_owned(),
        };
        let socket = crate::hook::socket_path();
        let (pipe, views_rx, force_tx, outlook_rx) = rt.block_on(async {
            let pipe = pipeline::spawn(socket, buf_w, buf_h);
            let (views_rx, force_tx) = tasks::spawn(pipe.scene_rx.clone(), summarizer);
            (pipe, views_rx, force_tx, fun::hko::spawn())
        });
        Self {
            ui: PanelState::default(),
            head_labels: true,
            summary_note,
            hooks: crate::hook::installed(),
            note: None,
            pipe,
            views_rx,
            force_tx,
            watch: Watch::load(),
            outlook_rx,
            notifier: notify::Notifier::default(),
            banners: notify::banners_on(),
        }
    }

    pub fn scene(&self) -> Arc<SceneState> {
        self.pipe.scene_rx.borrow().clone()
    }

    /// The state to draw now; drops a selection whose agent left.
    pub fn frame(&mut self) -> Frame {
        let views = self.views_rx.borrow().clone();
        let scene = idle_since_written(&self.scene(), &views);
        let now = SystemTime::now();
        let order = panel::order(&scene, &views, now, &self.ui.seen);
        if self
            .ui
            .selected
            .is_some_and(|id| !order.iter().any(|(o, _)| *o == id))
        {
            self.ui.selected = None;
            self.ui.expanded = false;
        }
        let labels = if self.head_labels {
            views
                .iter()
                .filter_map(|(id, v)| Some((*id, v.line.clone()?)))
                .collect()
        } else {
            HashMap::new()
        };
        self.ui.seen.retain(|id, _| scene.agents.contains_key(id));
        self.watch.observe(&scene, &views, now);
        let clock = Clock::now(now);
        let outlook = *self.outlook_rx.borrow();
        // A typhoon closes the rooftop; at lunch everyone is down for a meal.
        let in_pantry = clock.lunch() || outlook.typhoon();
        Frame {
            office: as_seen(&scene, &views, &self.ui.seen, in_pantry, now),
            scene,
            views,
            labels,
            clock,
            outlook,
        }
    }

    /// Tell the OS who waits on you: a banner per new wait, the Dock's badge.
    /// Run on its own clock, not per frame, since a covered window draws
    /// none and that is when word matters most.
    pub fn watch_waits(&mut self, focused: bool) {
        let views = self.views_rx.borrow().clone();
        let tick = self.notifier.tick(
            &self.scene(),
            &views,
            &self.ui.seen,
            SystemTime::now(),
            focused,
        );
        if self.banners {
            tick.alerts.iter().for_each(notify::post);
        }
        if let Some(n) = tick.badge {
            notify::set_badge(n);
        }
    }

    /// The window came back in front: after a banner, select who waits
    /// first, so `o` or Enter goes straight to it.
    pub fn focus_gained(&mut self, floors: &mut impl Floors) {
        if std::mem::take(&mut self.notifier.unseen)
            && let Some(id) = self.next_waiting()
        {
            self.ui.selected = Some(id);
            self.ui.reveal = true;
            self.follow(floors);
        }
    }

    /// The session at the head of the waiting line, if any waits on you.
    pub fn next_waiting(&self) -> Option<AgentId> {
        let views = self.views_rx.borrow().clone();
        panel::next_waiting(&self.scene(), &views, SystemTime::now(), &self.ui.seen)
    }

    /// Select `id` and show the floor it sits on.
    pub fn select(&mut self, id: AgentId, floors: &mut impl Floors) {
        if self.ui.selected == Some(id) {
            self.ui.expanded = !self.ui.expanded;
        } else {
            self.ui.selected = Some(id);
        }
        self.ui.reveal = true;
        self.follow(floors);
    }

    fn follow(&self, floors: &mut impl Floors) {
        let scene = self.scene();
        if let Some(slot) = self.ui.selected.and_then(|id| scene.agents.get(&id))
            && slot.floor_idx != floors.current_floor()
        {
            floors.navigate(slot.floor_idx, SystemTime::now());
        }
    }

    /// Whether `action` quits.
    pub fn act(&mut self, action: Action, floors: &mut impl Floors) -> bool {
        match action {
            Action::Quit => return true,
            Action::Back if self.ui.expanded => self.ui.expanded = false,
            Action::Back => self.ui.selected = None,
            Action::Down | Action::Up => {
                let delta = if action == Action::Down { 1 } else { -1 };
                let views = self.views_rx.borrow().clone();
                let order = panel::order(&self.scene(), &views, SystemTime::now(), &self.ui.seen);
                self.ui.selected = panel::step(&order, self.ui.selected, delta);
                self.ui.reveal = true;
                self.follow(floors);
            }
            Action::Expand if self.ui.selected.is_some() => {
                self.ui.expanded = !self.ui.expanded;
                self.ui.reveal = true;
            }
            Action::Expand => {}
            Action::Resummarize => {
                if let Some(id) = self.ui.selected {
                    let _ = self.force_tx.send(id);
                }
            }
            Action::ToggleLabels => self.head_labels = !self.head_labels,
            Action::NextFloor => {
                let last = floors.n_floors().saturating_sub(1);
                let next = (floors.current_floor() + 1).min(last);
                floors.navigate(next, SystemTime::now());
            }
            Action::PrevFloor => {
                let prev = floors.current_floor().saturating_sub(1);
                floors.navigate(prev, SystemTime::now());
            }
            Action::OpenNext => match self.next_waiting() {
                Some(id) => {
                    self.ui.selected = Some(id);
                    self.follow(floors);
                    return self.act(Action::Open, floors);
                }
                None => self.note_result(Err("现在没有会话等你".to_owned())),
            },
            Action::Open => {
                let scene = self.scene();
                let done = match self.ui.selected {
                    Some(id) => jump::open_agent(&scene, id).inspect(|_| {
                        // The session opened is the root's: all of it is read.
                        let now = SystemTime::now();
                        let mut at = Some(id);
                        // Bounded: a parent cycle in corrupt state must not spin.
                        for _ in 0..8 {
                            let Some(a) = at else { break };
                            self.ui.seen.insert(a, now);
                            at = scene.agents.get(&a).and_then(|s| s.parent_id);
                        }
                    }),
                    None => Err("先选中一个 agent".to_owned()),
                };
                self.note_result(done);
            }
            Action::OpenWaiting => {
                self.note_result(jump::open_url(jump::NEEDS_INPUT).map(|()| "已在 Claude 打开"))
            }
        }
        false
    }

    fn note_result(&mut self, done: Result<&str, String>) {
        let text = done.map_or_else(|e| format!("跳转：{e}"), str::to_owned);
        self.note = Some((text, Instant::now()));
    }

    /// The footer's right-hand note: a fresh jump result, else the summary
    /// model, after a warning while the hooks are out.
    fn status_note(&self) -> String {
        match &self.note {
            Some((text, at)) if at.elapsed() < NOTE_FOR => text.clone(),
            _ if !self.hooks => format!("{HOOKS_OUT} · {}", self.summary_note),
            _ => self.summary_note.clone(),
        }
    }
}

/// The key hints and the floor and summary status along the bottom.
pub fn footer(
    buf: &mut Buffer,
    area: Rect,
    floors: &impl Floors,
    core: &Core,
    extra: &[(&str, &str)],
) {
    let dim = Style::default().fg(Color::Rgb(0x8a, 0x8f, 0x98));
    let key = Style::default().fg(Color::Rgb(0xe6, 0xe6, 0xe6));
    let labels = if core.head_labels {
        "隐藏头顶"
    } else {
        "显示头顶"
    };
    let mut spans = Vec::new();
    let base = [
        ("↑↓", "选中"),
        ("Enter", "展开"),
        ("o", "打开会话"),
        ("w", "等你的"),
        ("n", "处理下一个"),
        ("r", "重新总结"),
        ("b", labels),
        ("[ ]", "换楼层"),
    ];
    // Each key on a keycap, its action beside it.
    let cap = key.bg(Color::Rgb(0x3b, 0x40, 0x4c));
    for (k, what) in base.iter().chain(extra).chain(&[("q", "退出")]) {
        spans.push(Span::styled(format!(" {k} "), cap));
        spans.push(Span::styled(format!(" {what}  "), dim));
    }
    let right = format!(
        "F{}/{}  {} ",
        floors.current_floor() + 1,
        floors.n_floors().max(1),
        core.status_note()
    );
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
    let w = text::cells(&right);
    if area.width > w {
        buf.set_line(
            area.x + area.width - w,
            area.y,
            &Line::from(Span::styled(right, dim)),
            w,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};

    /// The office walks a session you opened since it went idle out of the
    /// pantry: it draws that one's idle stretch as past the pantry's, and
    /// leaves every other session, and the scene the panel reads, alone.
    #[test]
    fn the_office_draws_a_read_session_past_the_pantry() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(600);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let ids: Vec<AgentId> = ["a", "b"]
            .iter()
            .map(|sid| {
                let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
                r.apply(
                    &mut scene,
                    AgentEvent::SessionStart {
                        agent_id: id,
                        source: pipeline::CC_SOURCE.into(),
                        session_id: (*sid).into(),
                        cwd: "/repo".into(),
                        parent_id: None,
                    },
                    t0,
                    Transport::Hook,
                );
                id
            })
            .collect();
        for id in &ids {
            let s = scene.agents.get_mut(id).unwrap();
            s.state = pixtuoid_core::ActivityState::Idle;
            s.state_started_at = now - Duration::from_secs(60);
        }
        let scene = Arc::new(scene);
        let views = tasks::TaskViews::default();
        assert!(Arc::ptr_eq(
            &as_seen(&scene, &views, &HashMap::new(), false, now),
            &scene
        ));
        let seen = HashMap::from([(ids[0], now - Duration::from_secs(5))]);
        let drawn = as_seen(&scene, &views, &seen, false, now);
        let idle_for =
            |s: &SceneState, id| now.duration_since(s.agents[id].state_started_at).unwrap();
        assert!(
            idle_for(&drawn, &ids[0]).as_millis()
                >= u128::from(pixtuoid_scene::pose::IDLE_ROOFTOP_MS),
            "past the pantry"
        );
        assert_eq!(
            idle_for(&drawn, &ids[1]),
            Duration::from_secs(60),
            "unread left alone"
        );
        assert_eq!(
            idle_for(&scene, &ids[0]),
            Duration::from_secs(60),
            "the panel's scene untouched"
        );

        // A done session asking you a question lines up for your office as
        // a reply, from when it asked: drawn idle it would sit in the pantry
        // as though nothing needed you. Opened, its question was seen.
        let asked = tasks::TaskView {
            outcome: Some("问：要不要删旧接口".into()),
            outcome_summarized: true,
            ..tasks::TaskView::default()
        };
        let views: tasks::TaskViews = Arc::new([(ids[1], asked)].into_iter().collect());
        for lunch in [false, true] {
            let drawn = as_seen(&scene, &views, &seen, lunch, now);
            assert_eq!(
                drawn.agents[&ids[1]].state,
                pixtuoid_core::ActivityState::RequiresAction {
                    reason: Some(pixtuoid_scene::waiting::REPLY_REASON.into())
                },
                "lunch: {lunch}"
            );
            assert_eq!(
                idle_for(&drawn, &ids[1]),
                Duration::from_secs(60),
                "waits from when it asked, lunch: {lunch}"
            );
        }
        let read = HashMap::from([(ids[1], now - Duration::from_secs(5))]);
        let drawn = as_seen(&scene, &views, &read, false, now);
        assert_eq!(
            drawn.agents[&ids[1]].state,
            pixtuoid_core::ActivityState::Idle
        );
    }

    /// At lunch, or with a typhoon closing the rooftop, every idle agent is
    /// drawn in the cha chaan teng, even one read and one idle for an hour;
    /// one just gone idle keeps its own stretch, and a working one is left
    /// at its desk.
    #[test]
    fn lunch_brings_every_idle_agent_down_to_the_cha_chaan_teng() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(7200);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let ids: Vec<AgentId> = ["long", "read", "fresh", "busy"]
            .iter()
            .map(|sid| {
                let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
                r.apply(
                    &mut scene,
                    AgentEvent::SessionStart {
                        agent_id: id,
                        source: pipeline::CC_SOURCE.into(),
                        session_id: (*sid).into(),
                        cwd: "/repo".into(),
                        parent_id: None,
                    },
                    t0,
                    Transport::Hook,
                );
                id
            })
            .collect();
        let idle_since = [3600, 1200, 30, 3600];
        for (id, secs) in ids.iter().zip(idle_since) {
            let s = scene.agents.get_mut(id).unwrap();
            s.state = pixtuoid_core::ActivityState::Idle;
            s.state_started_at = now - Duration::from_secs(secs);
        }
        scene.agents.get_mut(&ids[3]).unwrap().state =
            pixtuoid_core::ActivityState::Running { item: None };
        let scene = Arc::new(scene);
        let seen = HashMap::from([(ids[1], now - Duration::from_secs(5))]);
        let drawn = as_seen(&scene, &tasks::TaskViews::default(), &seen, true, now);
        let idle_ms = |id: &AgentId| {
            now.duration_since(drawn.agents[id].state_started_at)
                .unwrap()
                .as_millis() as u64
        };
        let rooftop = pixtuoid_scene::pose::IDLE_ROOFTOP_MS;
        assert!(
            idle_ms(&ids[0]) < rooftop,
            "an hour idle, still in the pantry"
        );
        assert!(idle_ms(&ids[1]) < rooftop, "read, but down for lunch");
        assert_eq!(idle_ms(&ids[2]), 30_000, "just idle, left as it is");
        assert_eq!(idle_ms(&ids[3]), 3_600_000, "working, left at its desk");
    }

    /// A session already open when agent-office starts last worked hours
    /// ago, but its slot is new: without the transcript's word it would read
    /// "空闲 8m" and sit in the pantry as if it had just finished. One that
    /// went idle after its last write keeps its own clock.
    #[test]
    fn an_idle_session_found_open_counts_from_its_last_write() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let launch = t0 + Duration::from_secs(18 * 3600);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let ids: Vec<AgentId> = ["old", "fresh", "busy"]
            .iter()
            .map(|sid| {
                let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
                r.apply(
                    &mut scene,
                    AgentEvent::SessionStart {
                        agent_id: id,
                        source: pipeline::CC_SOURCE.into(),
                        session_id: (*sid).into(),
                        cwd: "/repo".into(),
                        parent_id: None,
                    },
                    launch,
                    Transport::Jsonl,
                );
                id
            })
            .collect();
        scene.agents.get_mut(&ids[2]).unwrap().state =
            pixtuoid_core::ActivityState::Running { item: None };
        let written = |at| tasks::TaskView {
            written_at: Some(at),
            ..Default::default()
        };
        let views: tasks::TaskViews = Arc::new(HashMap::from([
            (ids[0], written(t0)),
            (ids[1], written(launch + Duration::from_secs(2))),
            (ids[2], written(t0)),
        ]));
        let scene = Arc::new(scene);
        let fixed = idle_since_written(&scene, &views);
        let started = |id| fixed.agents[id].state_started_at;
        assert_eq!(started(&ids[0]), t0, "idle since its last write");
        assert_eq!(started(&ids[1]), launch, "a later write keeps the slot's clock");
        assert_eq!(started(&ids[2]), launch, "a working session is not idle");
        assert!(Arc::ptr_eq(
            &idle_since_written(&scene, &Arc::new(HashMap::new())),
            &scene
        ));
    }
}
