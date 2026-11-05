//! What the window acts on, apart from how it draws: the live scene, the task
//! lines, the selection, and what each key does to them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentId, SceneState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::panel::{self, PanelState};
use crate::{jump, pipeline, tasks, text};

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
}

/// How long a note on what a key did stays in the footer.
const NOTE_FOR: Duration = Duration::from_secs(5);

pub struct Core {
    pub ui: PanelState,
    pub head_labels: bool,
    pub summary_note: String,
    /// What the last jump did, shown in place of `summary_note` a while.
    note: Option<(String, Instant)>,
    pub pipe: pipeline::Pipeline,
    views_rx: tokio::sync::watch::Receiver<tasks::TaskViews>,
    force_tx: tokio::sync::mpsc::UnboundedSender<AgentId>,
}

/// One frame's worth of state.
pub struct Frame {
    pub scene: Arc<SceneState>,
    /// `scene` as the office draws it ([`as_seen`]).
    pub office: Arc<SceneState>,
    pub views: tasks::TaskViews,
    /// The head label per agent; empty while they are hidden.
    pub labels: HashMap<AgentId, String>,
}

/// `scene` as the office draws it: a session you opened since it went idle
/// ([`panel::read_since_idle`]) has had its result read, so it idles as one
/// past the pantry, out on the rooftop; its idle stretch is drawn as starting
/// [`IDLE_ROOFTOP_MS`](pixtuoid_scene::pose::IDLE_ROOFTOP_MS) ago at least.
/// The panel keeps the real start for its "空闲 Xm".
pub fn as_seen(
    scene: &Arc<SceneState>,
    seen: &HashMap<AgentId, SystemTime>,
    now: SystemTime,
) -> Arc<SceneState> {
    let read: Vec<AgentId> = scene
        .agents
        .values()
        .filter(|s| panel::read_since_idle(s, seen))
        .map(|s| s.agent_id)
        .collect();
    if read.is_empty() {
        return Arc::clone(scene);
    }
    let past_pantry = now
        .checked_sub(Duration::from_millis(pixtuoid_scene::pose::IDLE_ROOFTOP_MS))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut drawn = (**scene).clone();
    for id in read {
        if let Some(s) = drawn.agents.get_mut(&id) {
            s.state_started_at = s.state_started_at.min(past_pantry);
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

impl Core {
    /// Start the sources and the task loop on `rt`, the floors seating `caps`.
    pub fn start(rt: &tokio::runtime::Runtime, caps: [usize; MAX_FLOORS]) -> Self {
        let summarizer = tasks::client::Summarizer::from_env();
        let summary_note = match &summarizer {
            Some(s) => format!("摘要 {}", s.model()),
            None => "摘要关闭".to_owned(),
        };
        let socket = crate::hook::socket_path();
        let (pipe, views_rx, force_tx) = rt.block_on(async {
            let pipe = pipeline::spawn(socket, caps);
            let (views_rx, force_tx) = tasks::spawn(pipe.scene_rx.clone(), summarizer);
            (pipe, views_rx, force_tx)
        });
        Self {
            ui: PanelState::default(),
            head_labels: true,
            summary_note,
            note: None,
            pipe,
            views_rx,
            force_tx,
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
        Frame {
            office: as_seen(&scene, &self.ui.seen, now),
            scene,
            views,
            labels,
        }
    }

    /// Select `id` and show the floor it sits on.
    pub fn select(&mut self, id: AgentId, floors: &mut impl Floors) {
        if self.ui.selected == Some(id) {
            self.ui.expanded = !self.ui.expanded;
        } else {
            self.ui.selected = Some(id);
        }
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
                self.follow(floors);
            }
            Action::Expand if self.ui.selected.is_some() => self.ui.expanded = !self.ui.expanded,
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
            Action::Open => {
                let scene = self.scene();
                let done = match self.ui.selected {
                    Some(id) => jump::open_agent(&scene, id).inspect(|()| {
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
            Action::OpenWaiting => self.note_result(jump::open_url(jump::NEEDS_INPUT)),
        }
        false
    }

    fn note_result(&mut self, done: Result<(), String>) {
        let text = done.map_or_else(|e| format!("跳转：{e}"), |()| "已在 Claude 打开".to_owned());
        self.note = Some((text, Instant::now()));
    }

    /// The footer's right-hand note: a fresh jump result, else the summary model.
    fn status_note(&self) -> &str {
        match &self.note {
            Some((text, at)) if at.elapsed() < NOTE_FOR => text,
            _ => &self.summary_note,
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
        assert!(Arc::ptr_eq(&as_seen(&scene, &HashMap::new(), now), &scene));
        let seen = HashMap::from([(ids[0], now - Duration::from_secs(5))]);
        let drawn = as_seen(&scene, &seen, now);
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
