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
use crate::{desktop, jump, notify, pipeline, tasks};

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

/// The footer's warning while Claude Code's hooks are out: short, since it
/// sits on the row the key hints need.
const HOOKS_OUT: &str = "未接 hooks（agent-office connect）";

/// How long a note on what a key did stays in the footer.
const NOTE_FOR: Duration = Duration::from_secs(5);
/// How long after a first `q` a second one quits: the window stays up all
/// day, and one stray key used to close it.
const QUIT_AGAIN_WITHIN: Duration = Duration::from_secs(2);

/// What a note in the footer reports, which sets its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Under way or done.
    Ok,
    /// Did not happen, or cannot.
    Failed,
}

pub struct Core {
    pub ui: PanelState,
    pub head_labels: bool,
    pub summary_note: String,
    /// Claude Code's hooks are in, as of the start: without them no prompt
    /// is seen, and your office stays empty however many wait.
    hooks: bool,
    /// What the last key did, shown in place of `summary_note` a while.
    note: Option<(String, Tone, Instant)>,
    /// When `q` was pressed once, waiting on a second.
    quit_armed: Option<Instant>,
    pub pipe: pipeline::Pipeline,
    views_rx: tokio::sync::watch::Receiver<tasks::TaskViews>,
    force_tx: tokio::sync::mpsc::UnboundedSender<AgentId>,
    /// The day's tally, the repositories' records and the boss's comings.
    pub watch: Watch,
    outlook_rx: tokio::sync::watch::Receiver<Outlook>,
    /// What the desktop app says you looked at.
    desktop_rx: tokio::sync::watch::Receiver<desktop::Looks>,
    notifier: notify::Notifier,
    banners: bool,
    /// Jumps run off the window's thread (a walk up the process tree with
    /// `ps`, tmux, `open`), their results back here: who, the scene asked
    /// against, and what happened.
    jumps: Jumps,
}

type Jumped = (AgentId, Arc<SceneState>, Result<&'static str, String>);

struct Jumps {
    tx: std::sync::mpsc::Sender<Jumped>,
    rx: std::sync::mpsc::Receiver<Jumped>,
    /// One is out: a second press waits for it rather than race it.
    out: bool,
}

impl Default for Jumps {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx, out: false }
    }
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
    /// The sessions waiting on you ([`panel::waits_on_you`]).
    pub waiting: Vec<AgentId>,
    /// The one to deal with first ([`panel::next_waiting`]).
    pub first_waiting: Option<AgentId>,
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

/// How long a session idles before the office stops drawing it.
const HIDE_IDLE_AFTER: Duration = Duration::from_secs(60 * 60);

/// `scene` without the sessions idle [`HIDE_IDLE_AFTER`] or longer. The
/// desktop app keeps every session it opened alive until it is archived, so
/// the office would otherwise fill with ones you are done with. A hidden
/// session comes back as soon as it gets a new turn; one still waiting on
/// your reply stays, since hiding it would drop the question.
/// Whether `slot` has idled [`HIDE_IDLE_AFTER`] or longer at `now`: the
/// office stops drawing it unless it still waits on your reply.
pub fn idle_long(slot: &pixtuoid_core::AgentSlot, now: SystemTime) -> bool {
    slot.state == pixtuoid_core::ActivityState::Idle
        && slot.exiting_at.is_none()
        && now
            .duration_since(slot.state_started_at)
            .is_ok_and(|d| d >= HIDE_IDLE_AFTER)
}

pub fn without_long_idle(
    scene: &Arc<SceneState>,
    views: &tasks::TaskViews,
    seen: &HashMap<AgentId, SystemTime>,
    now: SystemTime,
) -> Arc<SceneState> {
    let gone: Vec<AgentId> = scene
        .agents
        .values()
        .filter(|s| idle_long(s, now))
        .filter(|s| {
            panel::waits_on_you(s, views.get(&s.agent_id), panel::read_since_idle(s, seen))
                .is_none()
        })
        .map(|s| s.agent_id)
        .collect();
    if gone.is_empty() {
        return Arc::clone(scene);
    }
    let mut kept = (**scene).clone();
    for id in gone {
        kept.agents.remove(&id);
    }
    Arc::new(kept)
}

/// How far short of the rooftop's an idle stretch held in the pantry is
/// drawn: enough that a frame's lag never tips it over.
const PANTRY_MARGIN: Duration = Duration::from_secs(60);

impl Core {
    /// Start the sources and the task loop on `rt`, the floors seated for an
    /// office `buf_w`×`buf_h`.
    pub fn start(rt: &tokio::runtime::Runtime, buf_w: u16, buf_h: u16) -> Self {
        let summarizer = tasks::client::Summarizer::from_env();
        let summarizer_on = summarizer.is_some();
        let summary_note = match &summarizer {
            // The model's name is long and `doctor` already shows it.
            Some(_) => "摘要 ✓".to_owned(),
            None => "摘要关闭".to_owned(),
        };
        let socket = crate::hook::socket_path();
        let (pipe, views_rx, force_tx, outlook_rx) = rt.block_on(async {
            let pipe = pipeline::spawn(socket, buf_w, buf_h);
            let (views_rx, force_tx) = tasks::spawn(pipe.scene_rx.clone(), summarizer);
            (pipe, views_rx, force_tx, fun::hko::spawn())
        });
        Self {
            ui: PanelState {
                summaries: summarizer_on,
                ..PanelState::default()
            },
            head_labels: true,
            summary_note,
            hooks: crate::hook::installed(),
            note: None,
            quit_armed: None,
            pipe,
            views_rx,
            force_tx,
            watch: Watch::load(),
            outlook_rx,
            desktop_rx: desktop::spawn(),
            notifier: notify::Notifier::default(),
            banners: notify::banners_on(),
            jumps: Jumps::default(),
        }
    }

    /// Every live session, as the transcripts date their idle stretches.
    fn live(&self, views: &tasks::TaskViews) -> Arc<SceneState> {
        idle_since_written(&self.pipe.scene_rx.borrow().clone(), views)
    }

    /// The sessions the window shows: [`without_long_idle`] of the live ones.
    pub fn scene(&self) -> Arc<SceneState> {
        let views = self.views_rx.borrow().clone();
        without_long_idle(&self.live(&views), &views, &self.ui.seen, SystemTime::now())
    }

    /// The state to draw now; drops a selection whose agent left.
    pub fn frame(&mut self) -> Frame {
        self.land_jumps();
        let views = self.views_rx.borrow().clone();
        let live = self.live(&views);
        let now = SystemTime::now();
        let scene = without_long_idle(&live, &views, &self.ui.seen, now);
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
        // A hidden session is still live: what you read and its tally carry over.
        self.ui.seen.retain(|id, _| live.agents.contains_key(id));
        let waiting = panel::waiting(&scene, &views, &order, &self.ui.seen);
        let first_waiting = panel::next_waiting(&scene, &views, now, &self.ui.seen);
        self.watch.observe(&live, &views, now);
        let clock = Clock::now(now);
        let outlook = *self.outlook_rx.borrow();
        // A typhoon closes the rooftop; at lunch everyone is down for a meal.
        let in_pantry = clock.lunch() || outlook.typhoon();
        self.ui.in_pantry = in_pantry;
        Frame {
            office: as_seen(&scene, &views, &self.ui.seen, in_pantry, now),
            scene,
            views,
            labels,
            clock,
            outlook,
            waiting,
            first_waiting,
        }
    }

    /// Tell the OS who waits on you: a banner per new wait, the Dock's badge.
    /// Run on its own clock, not per frame, since a covered window draws
    /// none and that is when word matters most.
    pub fn watch_waits(&mut self, focused: bool) {
        self.land_jumps();
        // Here, not per frame: it runs on the main thread, as AppKit's
        // front-app check needs, and while the window is covered too.
        let live = self.pipe.scene_rx.borrow().clone();
        desktop::mark_read(
            &mut self.ui.seen,
            &self.desktop_rx.borrow(),
            |id| live.agents.contains_key(id),
            desktop::front(),
            SystemTime::now(),
        );
        let views = self.views_rx.borrow().clone();
        // Also counted here: a covered window draws no frames, and a turn
        // that starts and ends while it is covered still counts.
        self.watch
            .observe(&self.live(&views), &views, SystemTime::now());
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
            Action::Quit => {
                if quits(&mut self.quit_armed, Instant::now()) {
                    return true;
                }
                self.note("再按一次 q 退出", Tone::Ok);
            }
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
                None => self.note("现在没有会话等你", Tone::Failed),
            },
            Action::Open => match self.ui.selected {
                Some(_) if self.jumps.out => {}
                Some(id) => {
                    self.jumps.out = true;
                    self.note("正在打开…", Tone::Ok);
                    let scene = self.scene();
                    let tx = self.jumps.tx.clone();
                    std::thread::spawn(move || {
                        let done = jump::open_agent(&scene, id);
                        let _ = tx.send((id, scene, done));
                    });
                }
                None => self.note("先选中一个 agent", Tone::Failed),
            },
            Action::OpenWaiting => {
                self.note_result(jump::open_url(jump::NEEDS_INPUT).map(|()| "已在 Claude 打开"))
            }
        }
        false
    }

    /// Take in the jumps that have finished since the last look.
    fn land_jumps(&mut self) {
        while let Ok((id, scene, done)) = self.jumps.rx.try_recv() {
            self.jumps.out = false;
            if done.is_ok() {
                // The session opened is the root's: all of it is read.
                let now = SystemTime::now();
                let mut at = Some(id);
                // Bounded: a parent cycle in corrupt state must not spin.
                for _ in 0..8 {
                    let Some(a) = at else { break };
                    self.ui.seen.insert(a, now);
                    at = scene.agents.get(&a).and_then(|s| s.parent_id);
                }
            }
            self.note_result(done);
        }
    }

    /// A jump's outcome, a failure saying it was the jump that failed.
    fn note_result(&mut self, done: Result<&str, String>) {
        match done {
            Ok(text) => self.note(text, Tone::Ok),
            Err(e) => self.note(&format!("跳转失败：{e}"), Tone::Failed),
        }
    }

    fn note(&mut self, text: &str, tone: Tone) {
        self.note = Some((text.to_owned(), tone, Instant::now()));
    }

    /// The footer's right-hand note: a fresh note on what a key did, else the
    /// summary model, after a warning while the hooks are out.
    fn status_note(&self) -> (String, Option<Tone>) {
        match &self.note {
            Some((text, tone, at)) if at.elapsed() < NOTE_FOR => (text.clone(), Some(*tone)),
            _ if !self.hooks => (format!("{HOOKS_OUT} · {}", self.summary_note), None),
            _ => (self.summary_note.clone(), None),
        }
    }
}

/// The key hints and the floor and summary status along the bottom, after
/// how many sessions wait on you when any do. The status keeps its room;
/// the hints that do not fit beside it are left out whole, the least needed
/// first, rather than cut off mid-word with the last ones lost.
pub fn footer(
    buf: &mut Buffer,
    area: Rect,
    floors: &impl Floors,
    core: &Core,
    waiting: usize,
    extra: &[(&str, &str)],
) {
    let dim = Style::default().fg(Color::Rgb(0x8a, 0x8f, 0x98));
    let key = Style::default().fg(Color::Rgb(0xe6, 0xe6, 0xe6));
    let labels = if core.head_labels {
        "隐藏任务"
    } else {
        "显示任务"
    };
    // In the order shown; kept by `rank`, the lower the more needed.
    let base = [
        ("↑↓", "选中", 0),
        ("Enter", "展开", 3),
        ("o", "打开会话", 0),
        ("w", "等你的", 4),
        ("n", "处理下一个", 1),
        ("r", "重新总结", 7),
        ("b", labels, 6),
    ];
    let floors_hint = (floors.n_floors() > 1).then_some(("[ ]", "换楼层", 5));
    let hints: Vec<(&str, &str, u8)> = base
        .into_iter()
        .chain(floors_hint)
        .chain(extra.iter().map(|&(k, w)| (k, w, 8)))
        .chain([("q", "退出", 2)])
        .collect();
    let mut right = Vec::new();
    if waiting > 0 {
        let wait = Style::default().fg(panel::WAIT);
        right.push(Span::styled(format!("● {waiting} 个等你  "), wait));
    }
    // The floor only where there is more than one to be on.
    if floors.n_floors() > 1 {
        right.push(Span::styled(
            format!("{}/{} 层  ", floors.current_floor() + 1, floors.n_floors()),
            dim,
        ));
    }
    let (note, tone) = core.status_note();
    let note_style = match tone {
        Some(Tone::Ok) => Style::default().fg(Color::Rgb(0x9e, 0xce, 0x6a)),
        Some(Tone::Failed) => Style::default().fg(Color::Rgb(0xf7, 0x76, 0x8e)),
        None => dim,
    };
    right.push(Span::styled(format!("{note} "), note_style));
    let status = Line::from(right);
    let room = area.width.saturating_sub(
        u16::try_from(status.width())
            .unwrap_or(u16::MAX)
            .saturating_add(1),
    );
    // Each key on a keycap, its action beside it.
    let cap = key.bg(Color::Rgb(0x3b, 0x40, 0x4c));
    let mut spans = Vec::new();
    for (k, what, _) in &fit_hints(hints, room) {
        spans.push(Span::styled(format!(" {k} "), cap));
        spans.push(Span::styled(format!(" {what}  "), dim));
    }
    set_hints_and_status(buf, area, &Line::from(spans), &status);
}

/// Whether a `q` at `now` quits: the second within [`QUIT_AGAIN_WITHIN`] of
/// the first; else it arms `armed` for the next.
fn quits(armed: &mut Option<Instant>, now: Instant) -> bool {
    if armed
        .take()
        .is_some_and(|at| now.duration_since(at) < QUIT_AGAIN_WITHIN)
    {
        return true;
    }
    *armed = Some(now);
    false
}

/// The key hints (key, what it does, rank) that fit `room` cells, in their
/// order: the highest rank, the least needed, left out first, each whole.
fn fit_hints<'a>(mut hints: Vec<(&'a str, &'a str, u8)>, room: u16) -> Vec<(&'a str, &'a str, u8)> {
    let width = |(k, what, _): &(&str, &str, u8)| {
        crate::text::cells(&format!(" {k} ")) + crate::text::cells(&format!(" {what}  "))
    };
    while hints.iter().map(width).sum::<u16>() > room {
        let Some(worst) = hints
            .iter()
            .enumerate()
            .max_by_key(|(i, h)| (h.2, *i))
            .map(|(i, _)| i)
        else {
            break;
        };
        hints.remove(worst);
    }
    hints
}

/// `hints` along `area`'s row and `status` flush right on it, the hints cut
/// short a cell before the status; the hints alone when the status does
/// not fit.
fn set_hints_and_status(buf: &mut Buffer, area: Rect, hints: &Line, status: &Line) {
    let w = u16::try_from(status.width()).unwrap_or(u16::MAX);
    if area.width > w {
        let room = area.width - w;
        buf.set_line(area.x, area.y, hints, room.saturating_sub(1));
        buf.set_line(area.x + room, area.y, status, w);
    } else {
        buf.set_line(area.x, area.y, hints, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};

    /// The footer's key hints run longer than a narrow window: drawn across
    /// the whole row, they were overwritten mid-word by the status, which
    /// left both unreadable. The hints give way and the status reads whole.
    #[test]
    fn long_hints_stop_short_of_the_status() {
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        let hints = Line::from("t 取消置顶  q 退出  b 隐藏头顶  r 重新总结");
        let status = Line::from("● 2 个等你  F1/1  摘要 ✓ ");
        set_hints_and_status(&mut buf, area, &hints, &status);
        let row: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_owned())
            .collect();
        let w = u16::try_from(status.width()).expect("fits");
        let tail: String = (area.width - w..area.width)
            .map(|x| buf[(x, 0)].symbol().to_owned())
            .collect();
        assert_eq!(tail.replace(' ', ""), "●2个等你F1/1摘要✓", "row: {row:?}");
        assert_eq!(
            buf[(area.width - w - 1, 0)].symbol(),
            " ",
            "a gap before it"
        );
        assert_eq!(buf[(0, 0)].symbol(), "t", "the hints still start the row");
    }

    /// A narrow window cut the hints mid-word and lost `q 退出` first; now
    /// the least needed go whole, and opening, the next wait and quitting
    /// stay.
    #[test]
    fn a_narrow_footer_drops_whole_hints_least_needed_first() {
        let hints = vec![
            ("↑↓", "选中", 0),
            ("Enter", "展开", 3),
            ("o", "打开会话", 0),
            ("n", "处理下一个", 1),
            ("r", "重新总结", 7),
            ("q", "退出", 2),
        ];
        let all = fit_hints(hints.clone(), 200);
        assert_eq!(all.len(), hints.len(), "all fit in a wide row");
        let few = fit_hints(hints, 52);
        let keys: Vec<&str> = few.iter().map(|h| h.0).collect();
        assert_eq!(keys, ["↑↓", "o", "n", "q"]);
    }

    /// One stray `q` closed a window kept up all day; it takes a second one
    /// soon after.
    #[test]
    fn q_quits_only_when_pressed_twice_quickly() {
        let t = Instant::now();
        let mut armed = None;
        assert!(!quits(&mut armed, t), "the first arms");
        assert!(quits(&mut armed, t + Duration::from_millis(500)));
        let mut armed = None;
        assert!(!quits(&mut armed, t));
        assert!(
            !quits(&mut armed, t + QUIT_AGAIN_WITHIN + Duration::from_millis(1)),
            "too late: it arms again"
        );
        assert!(quits(
            &mut armed,
            t + QUIT_AGAIN_WITHIN + Duration::from_secs(1)
        ));
    }

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
        assert_eq!(
            started(&ids[1]),
            launch,
            "a later write keeps the slot's clock"
        );
        assert_eq!(started(&ids[2]), launch, "a working session is not idle");
        assert!(Arc::ptr_eq(
            &idle_since_written(&scene, &Arc::new(HashMap::new())),
            &scene
        ));
    }

    /// The desktop app keeps every session alive until it is archived, so
    /// one you finished an hour ago must leave the office, and come back the
    /// moment it gets new work. A long-idle session still waiting on your
    /// reply stays: hiding it would hide the question.
    #[test]
    fn a_long_idle_session_is_hidden_until_it_works_again() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(5 * 3600);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let ids: Vec<AgentId> = ["done", "recent", "asking", "busy"]
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
        let idle_for = [2 * 3600, 30 * 60, 2 * 3600];
        for (id, secs) in ids.iter().zip(idle_for) {
            let s = scene.agents.get_mut(id).unwrap();
            s.state = pixtuoid_core::ActivityState::Idle;
            s.state_started_at = now - Duration::from_secs(secs);
        }
        let busy = scene.agents.get_mut(&ids[3]).unwrap();
        busy.state = pixtuoid_core::ActivityState::Running { item: None };
        busy.state_started_at = now - Duration::from_secs(3 * 3600);
        let views: tasks::TaskViews = Arc::new(HashMap::from([(
            ids[2],
            tasks::TaskView {
                outcome: Some("问：要合并吗？".into()),
                outcome_summarized: true,
                ..Default::default()
            },
        )]));
        let seen = HashMap::new();
        let shown = without_long_idle(&Arc::new(scene.clone()), &views, &seen, now);
        assert!(
            !shown.agents.contains_key(&ids[0]),
            "idle two hours: hidden"
        );
        assert!(
            shown.agents.contains_key(&ids[1]),
            "idle half an hour: still in"
        );
        assert!(
            shown.agents.contains_key(&ids[2]),
            "its question waits on you"
        );
        assert!(
            shown.agents.contains_key(&ids[3]),
            "a long turn is still work"
        );

        // Once you read its question, it is done with you and leaves too.
        let read = HashMap::from([(ids[2], now)]);
        let shown = without_long_idle(&Arc::new(scene.clone()), &views, &read, now);
        assert!(!shown.agents.contains_key(&ids[2]), "read question: hidden");

        // A new turn, as the reducer records one, brings it back.
        let s = scene.agents.get_mut(&ids[0]).unwrap();
        s.state = pixtuoid_core::ActivityState::Running { item: None };
        s.state_started_at = now;
        let shown = without_long_idle(&Arc::new(scene), &views, &seen, now);
        assert!(
            shown.agents.contains_key(&ids[0]),
            "new work: back in the office"
        );
    }
}
