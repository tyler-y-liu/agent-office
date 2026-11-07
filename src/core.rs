//! What the window acts on, apart from how it draws: the live scene, the task
//! lines, the selection, and what each key does to them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::{AgentId, AgentSlot, SceneState};
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
    /// Speak the other language, now and at the next start.
    SwitchLang,
}

/// What `p` dispatches: into which repository, and whose work it carries
/// on, if any. Made when the picker opens, so the pick sends what it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchPlan {
    pub dir: std::path::PathBuf,
    pub backend: crate::roles::Backend,
    pub relay: Option<Relay>,
    /// Whether the picker could switch to the other host. Read once, when
    /// the plan is made: the paint path draws every beat and the answer costs
    /// a walk of the role files.
    pub switchable: bool,
}

#[derive(Clone, Debug)]
/// What the panel's role bar shows: where a click dispatches, in which
/// repository, and every host it offers.
pub struct DispatchBar {
    pub active: crate::roles::Backend,
    pub repo: String,
    /// Each host with whether it could take a dispatch now, in the bar's own
    /// order: one chip per entry, so a host added to [`crate::roles::Backend::ALL`]
    /// appears without another layout.
    pub hosts: Vec<(crate::roles::Backend, bool)>,
}

/// How long the role bar keeps its host: long enough that drawing it costs
/// nothing, short enough that a session moving to the front shows up.
const HOST_CACHE_FOR: Duration = Duration::from_secs(2);

/// The session a dispatch goes from: the selected one, else the one that
/// started its state last — the worker whose repository and work it carries.
fn dispatch_worker(scene: &SceneState, selected: Option<AgentId>) -> Option<&AgentSlot> {
    let selected = selected.filter(|id| scene.agents.contains_key(id));
    match selected {
        Some(id) => scene.agents.get(&id),
        None => scene
            .agents
            .values()
            .filter(|s| !s.unknown_cwd && s.exiting_at.is_none())
            .max_by_key(|s| s.state_started_at),
    }
}

/// The host a session runs in, which a dispatch from it follows by default.
fn host_of(slot: &AgentSlot) -> crate::roles::Backend {
    if &*slot.source == crate::dsh::SOURCE {
        crate::roles::Backend::Dsh
    } else {
        crate::roles::Backend::Claude
    }
}

/// Where a dispatch goes: fresh work belongs to the main checkout, while a
/// relay must stay beside the changes it picks up.
fn dispatch_dir(cwd: &std::path::Path, relay: bool) -> std::path::PathBuf {
    if relay {
        cwd.to_path_buf()
    } else {
        crate::roles::repo_root(cwd)
    }
}

/// A dispatch that picks up a done or stuck session's work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relay {
    pub name: String,
    /// The posts to hand on to, best first.
    pub suggested: Vec<crate::roles::Role>,
    pub task: String,
}

impl DispatchPlan {
    /// The repository's folder name, which the picker heads with.
    pub fn repo(&self) -> String {
        self.dir
            .file_name()
            .map_or_else(|| self.dir.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

/// The footer's warning while Claude Code's hooks are out: short, since it
/// sits on the row the key hints need.
fn hooks_out() -> &'static str {
    crate::i18n::t(
        "未接 hooks（agent-office connect）",
        "hooks off (agent-office connect)",
    )
}

/// A dead input makes the scene stale, so its source and fatal error outrank the
/// ordinary healthy footer. Only the error chain is shown, never a transcript.
pub(crate) fn source_death_note(
    deaths: &[pixtuoid_core::source::manager::SourceDeath],
) -> Option<String> {
    let death = deaths.last()?;
    let source = crate::text::one_line(&death.source);
    let error = crate::text::one_line(&death.error);
    let note = crate::tr!(
        "数据源 {} 已停止：{}",
        "source {} stopped: {}",
        source,
        error
    );
    Some(crate::text::truncate_cells(&note, 120))
}

/// How often an empty office looks again at the hooks and roles.
const SETUP_RECHECK: Duration = Duration::from_secs(3);
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
    /// Claude Code's hooks are in, as of the start: without them no prompt
    /// is seen, and your office stays empty however many wait.
    hooks: bool,
    /// What the last key did, shown in place of the summary note a while.
    note: Option<(String, Tone, Instant)>,
    /// The latest fatal source exit, refreshed every frame. A recent action
    /// note takes precedence briefly; the retained warning returns afterwards.
    source_warning: Option<String>,
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
    /// The real jump by default; tests substitute one that never opens an app.
    open_agent: fn(&SceneState, AgentId) -> Result<&'static str, String>,
    /// Dispatch waits for a desktop app or Host without freezing the event loop.
    dispatches: Dispatches,
    /// When the role bar's host was last worked out, and what it was: the
    /// answer walks the role files, and the bar is drawn every beat.
    host_at: Option<(Instant, Option<DispatchBar>)>,
    /// The host picked from the bar or the picker this run. It outranks the
    /// configured default, so a click does not have to be repeated for every
    /// dispatch: switching once is a decision, not one pick's footnote.
    picked_host: Option<crate::roles::Backend>,
    /// When the empty office last looked again at what is set up.
    setup_checked: Option<Instant>,
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

type Dispatched = (
    crate::roles::Backend,
    crate::roles::Role,
    bool,
    anyhow::Result<String>,
);

struct Dispatches {
    tx: std::sync::mpsc::Sender<Dispatched>,
    rx: std::sync::mpsc::Receiver<Dispatched>,
    /// The host actually in flight: a rejected new plan may name the other one.
    out: Option<crate::roles::Backend>,
}

impl Default for Dispatches {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx, out: None }
    }
}

/// A host's own result for DSH, whose text includes where the session went;
/// Claude's GUI note keeps its relay/fresh wording in either language.
fn dispatch_success(
    backend: crate::roles::Backend,
    role: crate::roles::Role,
    relay: bool,
    message: String,
) -> String {
    match backend {
        crate::roles::Backend::Dsh => message,
        crate::roles::Backend::Claude if relay => crate::tr!(
            "已在 Claude 开好「{}」接力会话，看一眼就发送",
            "opened a {} relay in Claude: check it and send",
            role.name()
        ),
        crate::roles::Backend::Claude => crate::tr!(
            "已在 Claude 开好「{}」的新会话，写上任务发送",
            "opened a new {} session in Claude: write the task and send",
            role.name()
        ),
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
/// ([`panel::read_since_idle`]) has had its result read, so it leaves the
/// line in your office for the pantry; its idle stretch is drawn as starting
/// [`DONE_WAITS_MS`](pixtuoid_scene::waiting::DONE_WAITS_MS) ago at least.
/// While `in_pantry` (lunch, or a typhoon closing the rooftop) every idle
/// agent resting is drawn in the pantry, the cha chaan teng, rather than on
/// the rooftop: its idle stretch is drawn a minute short of the rooftop's at
/// most; the line in your office stays as it is. The panel keeps the real
/// start for its "空闲 Xm". A done session whose turn ended on a question to
/// you waits on you as a prompt does, so it is drawn as one, lining up behind
/// the prompts from when it asked, lunch or not.
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
    // One never given a task has no result either: the guest chair and `n`
    // must not point at a session with nothing to look at.
    let read: Vec<AgentId> = scene
        .agents
        .values()
        .filter(|s| {
            panel::read_since_idle(s, seen)
                || (panel::never_ran(s, views.get(&s.agent_id)) && !asking.contains(&s.agent_id))
        })
        .map(|s| s.agent_id)
        .collect();
    let last_pantry = ago(rooftop - PANTRY_MARGIN);
    // After the read ones are moved, so one just read is down for lunch too
    // rather than back in your office.
    let resting_up = |s: &AgentSlot| {
        in_pantry
            && s.state == pixtuoid_core::ActivityState::Idle
            && s.exiting_at.is_none()
            && s.state_started_at < last_pantry
            // Its wait is timed from when it asked, which the line orders by.
            && !asking.contains(&s.agent_id)
    };
    if read.is_empty() && asking.is_empty() && !scene.agents.values().any(resting_up) {
        return Arc::clone(scene);
    }
    let mut drawn = (**scene).clone();
    let out_of_line = ago(Duration::from_millis(pixtuoid_scene::waiting::DONE_WAITS_MS));
    for id in read {
        if let Some(s) = drawn.agents.get_mut(&id) {
            s.state_started_at = s.state_started_at.min(out_of_line);
        }
    }
    for s in drawn.agents.values_mut() {
        if resting_up(s) {
            s.state_started_at = last_pantry;
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

/// Who sits in the guest chair of your office on `floor` of `office`, the
/// scene as the office draws it: the line of that floor alone.
pub fn guest_of(office: &SceneState, floor: usize, now: SystemTime) -> Option<AgentId> {
    panel::guest_chair(&pixtuoid_scene::floor::project_floor_scene(office, floor), now)
}

/// `scene` with each subagent named after its parent and its post: Claude
/// Code labels every one "subagent", so a PM's helpers all read
/// "subag…·1", "subag…·2", saying neither whose they are nor what they do.
pub fn named_subagents(scene: &Arc<SceneState>, views: &tasks::TaskViews) -> Arc<SceneState> {
    let names: Vec<(AgentId, String)> = scene
        .agents
        .values()
        .filter(|s| &*s.label == "subagent")
        .filter_map(|s| {
            let parent = scene.agents.get(s.parent_id.as_ref()?)?;
            let view = views.get(&s.agent_id);
            let what = match view.and_then(|v| v.role) {
                Some(r) => r.name().to_owned(),
                None => view.and_then(|v| v.agent_type.clone())?,
            };
            let of = pixtuoid_scene::badge::shown_label(&parent.label);
            Some((s.agent_id, format!("{of}›{what}")))
        })
        .collect();
    if names.is_empty() {
        return Arc::clone(scene);
    }
    let mut named = (**scene).clone();
    for (id, name) in names {
        if let Some(s) = named.agents.get_mut(&id) {
            s.label = pixtuoid_core::state::SlotLabel::renamed(name);
        }
    }
    Arc::new(named)
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

/// `scene` with each idle session that still has a subagent at work drawn
/// working. A session that backgrounds its subagents ends its turn while
/// they run, so the wire reads it idle; it is not done until they report
/// back and it picks up again. One whose turn ended on a question to you
/// still waits on you, and is left as it is.
pub fn working_through_helpers(
    scene: &Arc<SceneState>,
    views: &tasks::TaskViews,
    seen: &HashMap<AgentId, SystemTime>,
) -> Arc<SceneState> {
    let mut busy: Vec<AgentId> = Vec::new();
    for s in scene.agents.values() {
        let at_work = s.exiting_at.is_none()
            && !matches!(s.state, pixtuoid_core::ActivityState::Idle);
        if !at_work {
            continue;
        }
        let mut at = s.parent_id;
        // Bounded: a corrupt parent cycle must not hang the frame.
        for _ in 0..scene.agents.len() {
            let Some(p) = at.and_then(|p| scene.agents.get(&p)) else { break };
            if p.state == pixtuoid_core::ActivityState::Idle
                && p.exiting_at.is_none()
                && panel::waits_on_you(p, views.get(&p.agent_id), panel::read_since_idle(p, seen))
                    .is_none()
                && !busy.contains(&p.agent_id)
            {
                busy.push(p.agent_id);
            }
            at = p.parent_id;
        }
    }
    if busy.is_empty() {
        return Arc::clone(scene);
    }
    let mut drawn = (**scene).clone();
    for id in busy {
        if let Some(s) = drawn.agents.get_mut(&id) {
            s.state = pixtuoid_core::ActivityState::Running {
                item: Some(pixtuoid_core::state::ToolUse {
                    tool_use_id: None,
                    detail: Some(panel::ON_HELPERS.into()),
                    kind: pixtuoid_core::state::ToolKind::Task,
                }),
            };
        }
    }
    Arc::new(drawn)
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
    // The reducer still holds their desks, so the ones shown sat upstairs
    // past them; seat those as if the hidden were not there.
    kept.settle_floors();
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
        let socket = crate::hook::socket_path();
        let (pipe, views_rx, force_tx, outlook_rx) = rt.block_on(async {
            let pipe = pipeline::spawn(socket, buf_w, buf_h);
            let (views_rx, force_tx) = tasks::spawn(pipe.scene_rx.clone(), summarizer);
            (pipe, views_rx, force_tx, fun::hko::spawn())
        });
        Self {
            ui: PanelState {
                summaries: summarizer_on,
                hooks: crate::hook::installed(),
                roles: crate::roles::doctor_line().0,
                ..PanelState::default()
            },
            head_labels: true,
            hooks: crate::hook::installed(),
            note: None,
            source_warning: None,
            quit_armed: None,
            pipe,
            views_rx,
            force_tx,
            watch: Watch::load(),
            outlook_rx,
            desktop_rx: desktop::spawn(),
            setup_checked: None,
            notifier: notify::Notifier::default(),
            banners: notify::banners_on(),
            jumps: Jumps::default(),
            open_agent: jump::open_agent,
            dispatches: Dispatches::default(),
            host_at: None,
            picked_host: None,
        }
    }

    /// Every live session, as the transcripts date their idle stretches.
    fn live(&self, views: &tasks::TaskViews) -> Arc<SceneState> {
        named_subagents(
            &working_through_helpers(
                &idle_since_written(&self.pipe.scene_rx.borrow().clone(), views),
                views,
                &self.ui.seen,
            ),
            views,
        )
    }

    /// The sessions the window shows: [`without_long_idle`] of the live ones.
    pub fn scene(&self) -> Arc<SceneState> {
        let views = self.views_rx.borrow().clone();
        without_long_idle(&self.live(&views), &views, &self.ui.seen, SystemTime::now())
    }

    /// The state to draw now; drops a selection whose agent left.
    pub fn frame(&mut self) -> Frame {
        self.source_warning = source_death_note(&self.pipe.deaths_rx.borrow());
        self.land_jumps();
        self.land_dispatches();
        let views = self.views_rx.borrow().clone();
        let live = self.live(&views);
        let now = SystemTime::now();
        let scene = without_long_idle(&live, &views, &self.ui.seen, now);
        // An empty office lists what is left to set up: you run those steps
        // with it open, so it looks again rather than trust the start.
        if scene.agents.is_empty()
            && self
                .setup_checked
                .is_none_or(|t| t.elapsed() >= SETUP_RECHECK)
        {
            self.setup_checked = Some(Instant::now());
            self.ui.hooks = crate::hook::installed();
            self.hooks = self.ui.hooks;
            self.ui.roles = crate::roles::doctor_line().0;
        }
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
        self.watch.observe(&live, &views, now);
        let clock = Clock::now(now);
        let outlook = *self.outlook_rx.borrow();
        let office = as_seen(&scene, &views, &self.ui.seen, self.in_pantry(now), now);
        let first_waiting = panel::next_waiting(&scene, &views, now, &self.ui.seen);
        Frame {
            office,
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

    /// The session to deal with next ([`panel::next_waiting`]), if any
    /// needs you.
    pub fn next_waiting(&self) -> Option<AgentId> {
        let views = self.views_rx.borrow().clone();
        panel::next_waiting(&self.scene(), &views, SystemTime::now(), &self.ui.seen)
    }

    /// Who sits in the guest chair of your office on `floor`: that floor's
    /// own line, as its office seats it, where `n` goes by the whole
    /// building's.
    pub fn guest_on(&self, floor: usize) -> Option<AgentId> {
        let views = self.views_rx.borrow().clone();
        let now = SystemTime::now();
        let office = as_seen(&self.scene(), &views, &self.ui.seen, self.in_pantry(now), now);
        guest_of(&office, floor, now)
    }

    /// Open whoever sits in the guest chair on the floor shown; false with
    /// the chair empty.
    pub fn open_guest(&mut self, floors: &mut impl Floors) -> bool {
        let Some(id) = self.guest_on(floors.current_floor()) else {
            return false;
        };
        self.ui.selected = Some(id);
        self.act(Action::Open, floors);
        true
    }

    /// Whether the office's resting are all in the pantry: a typhoon closes
    /// the rooftop; at lunch everyone is down for a meal.
    fn in_pantry(&self, now: SystemTime) -> bool {
        Clock::now(now).lunch() || self.outlook_rx.borrow().typhoon()
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
                self.note(
                    crate::i18n::t("再按一次 q 退出", "press q again to quit"),
                    Tone::Ok,
                );
            }
            Action::Back if self.ui.expanded => self.ui.expanded = false,
            Action::Back => self.ui.selected = None,
            Action::Down | Action::Up => {
                let delta = if action == Action::Down { 1 } else { -1 };
                let views = self.views_rx.borrow().clone();
                let (scene, now) = (self.scene(), SystemTime::now());
                let order = panel::order(&scene, &views, now, &self.ui.seen);
                // Through the rows as shown: a folded card is not stepped onto.
                let (shown, _) = panel::fold(
                    &order,
                    &scene,
                    &views,
                    now,
                    &self.ui.seen,
                    self.ui.selected,
                    self.ui.expanded,
                );
                self.ui.selected = panel::step(&shown, self.ui.selected, delta);
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
            Action::SwitchLang => {
                let (now, saved) = crate::i18n::switch();
                let note = match (saved, crate::i18n::pinned_by_env()) {
                    (Err(e), _) => crate::tr!(
                        "已换成中文，但没存下来，下次启动不算：{e}",
                        "Switched to English, but could not save it for the next start: {e}"
                    ),
                    // The environment wins over the file at the next start.
                    (Ok(()), Some(env)) if env != now => crate::i18n::t(
                        "已换成中文；环境变量 AGENT_OFFICE_LANG 会在下次启动时改回去",
                        "Switched to English; AGENT_OFFICE_LANG in the environment will undo it at the next start",
                    )
                    .to_owned(),
                    (Ok(()), _) => crate::i18n::t("已换成中文", "Switched to English").to_owned(),
                };
                self.note(&note, Tone::Ok);
            }
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
                None => self.note(
                    crate::i18n::t("你的办公室里没人", "nobody is in your office"),
                    Tone::Failed,
                ),
            },
            Action::Open => match self.ui.selected {
                Some(_) if self.jumps.out => {}
                Some(id) => {
                    self.jumps.out = true;
                    self.note(crate::i18n::t("正在打开…", "opening…"), Tone::Ok);
                    let scene = self.scene();
                    let tx = self.jumps.tx.clone();
                    let open = self.open_agent;
                    std::thread::spawn(move || {
                        let done = open(&scene, id);
                        let _ = tx.send((id, scene, done));
                    });
                }
                None => self.note(
                    crate::i18n::t("先选中一个 agent", "select an agent first"),
                    Tone::Failed,
                ),
            },
            Action::OpenWaiting => self.note_result(jump::open_desktop(jump::NEEDS_INPUT)),
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

    /// Both hosts wait off-thread; only their short outcomes land in a frame.
    fn land_dispatches(&mut self) {
        while let Ok((backend, role, relay, done)) = self.dispatches.rx.try_recv() {
            self.dispatches.out = None;
            let kind = if relay { "relay" } else { "fresh" };
            self.watch.tally(&match &done {
                Ok(_) => format!("dispatch.{kind}.{}", role.post().id),
                Err(_) => "dispatch.failed".to_owned(),
            });
            match done {
                Ok(message) => {
                    self.note(&dispatch_success(backend, role, relay, message), Tone::Ok)
                }
                Err(e) => self.note(
                    &crate::tr!("派活失败：{}", "dispatch failed: {}", e),
                    Tone::Failed,
                ),
            }
        }
    }

    /// A jump's outcome, a failure saying it was the jump that failed.
    fn note_result(&mut self, done: Result<&str, String>) {
        match done {
            Ok(text) => self.note(text, Tone::Ok),
            Err(e) => self.note(
                &crate::tr!("跳转失败：{e}", "could not jump: {e}"),
                Tone::Failed,
            ),
        }
    }

    /// What `p` would dispatch now: into the selected session's repository,
    /// carrying its work on when it is done or stuck; with none selected,
    /// a fresh session where the latest activity was. `None`, with a note
    /// saying why, when there is nowhere to send it.
    pub fn plan_dispatch(&mut self) -> Option<DispatchPlan> {
        let scene = self.scene();
        let views = self.views_rx.borrow().clone();
        let now = SystemTime::now();
        // A selection whose session just left is no selection.
        let selected = self.ui.selected.filter(|id| scene.agents.contains_key(id));
        let Some(slot) = dispatch_worker(&scene, selected) else {
            self.watch.tally("dispatch.blocked.empty");
            self.note(
                crate::i18n::t(
                    "办公室里还没人：在终端用 agent-office dispatch 派活",
                    "no one in the office yet: dispatch from a terminal with agent-office dispatch",
                ),
                Tone::Failed,
            );
            return None;
        };
        if slot.unknown_cwd {
            self.watch.tally("dispatch.blocked.no_dir");
            self.note(
                crate::i18n::t(
                    "不知道它在哪个目录，换一个小人",
                    "its folder is unknown: pick another worker",
                ),
                Tone::Failed,
            );
            return None;
        }
        let relay = selected.and_then(|_| {
            let view = views.get(&slot.agent_id);
            let read = panel::read_since_idle(slot, &self.ui.seen);
            let after = match panel::Group::of(slot, view, now, read) {
                panel::Group::Done => crate::roles::After::Done,
                panel::Group::Stuck => crate::roles::After::Stuck,
                _ => return None,
            };
            let name = pixtuoid_scene::badge::shown_names(scene.agents.values())
                .remove(&slot.agent_id)
                .unwrap_or_else(|| slot.label.to_string());
            let v = view.cloned().unwrap_or_default();
            let stuck = tasks::stuck_at_work(slot, view).map(tasks::Stuck::describe);
            let task = crate::roles::relay_task(
                &name,
                after,
                v.prompt.as_deref().or(v.line.as_deref()),
                v.reply.as_deref().or(v.outcome.as_deref()),
                stuck.as_deref(),
                &slot.cwd,
            );
            Some(Relay {
                suggested: crate::roles::next_posts(after, v.role),
                name,
                task,
            })
        });
        // A fresh session goes to the repository the worker is seated with,
        // never into a worktree under its owner; a relay goes where the work
        // it carries on is, since a worktree's changes are not in the main
        // checkout.
        let dir = dispatch_dir(&slot.cwd, relay.is_some());
        self.watch.tally(if relay.is_some() {
            "picker.open.relay"
        } else {
            "picker.open.fresh"
        });
        // The worker's own host unless AGENT_OFFICE_DISPATCH_HOST names one,
        // and `h` in the picker can still switch it before a role is picked.
        let backend = self.dispatch_backend(host_of(slot));
        let switchable = crate::roles::host_ready(backend.other());
        Some(DispatchPlan {
            dir,
            backend,
            relay,
            switchable,
        })
    }

    /// What the role bar shows: the host a click dispatches into, and whether
    /// the other one could take a dispatch too. `None` hides the bar: one
    /// that only ever answers "not set up" would be furniture. The cache above
    /// bounds `repo_root`'s git lookup to one miss per 2s, never one per frame.
    pub fn dispatch_bar(&mut self) -> Option<DispatchBar> {
        if let Some((at, bar)) = &self.host_at
            && at.elapsed() < HOST_CACHE_FOR
        {
            return bar.clone();
        }
        let bar = self
            .worker_host()
            .and_then(|(following, dir)| {
                let host = self.dispatch_backend(following);
                crate::roles::host_ready(host).then(|| {
                    let repo = dir.file_name().map_or_else(
                        || dir.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    DispatchBar {
                        active: host,
                        repo,
                        // Every host the bar shows is asked whether it could
                        // take a dispatch: an unready one is still listed, so
                        // clicking it can say what it is missing.
                        hosts: crate::roles::Backend::ALL
                            .iter()
                            .map(|host| (*host, crate::roles::host_ready(*host)))
                            .collect(),
                    }
                })
            })
            .filter(|bar| !bar.hosts.is_empty());
        self.host_at = Some((Instant::now(), bar.clone()));
        bar
    }

    /// The host a dispatch goes to: the one picked this run, else the
    /// configured default, else the worker's own — and one that could not
    /// take a dispatch never wins, so a stale pick cannot break `p`.
    fn dispatch_backend(&self, following: crate::roles::Backend) -> crate::roles::Backend {
        match self.picked_host {
            Some(host) if crate::roles::host_ready(host) => host,
            _ => crate::roles::dispatch_host(following),
        }
    }

    /// Dispatch into `host` from now on, if it could take it; otherwise say
    /// what it is missing, since that is how a host that was never set up
    /// gets found out.
    pub fn choose_dispatch_host(&mut self, host: crate::roles::Backend) {
        let current = self
            .dispatch_bar()
            .map_or(crate::roles::Backend::Claude, |bar| bar.active);
        if host == current {
            return;
        }
        match crate::roles::host_missing(host) {
            None => {
                self.picked_host = Some(host);
                // The bar and the next plan both read this cache.
                self.host_at = None;
                self.watch.tally("dispatch.host.switched");
            }
            Some(why) => self.note(&why, Tone::Failed),
        }
    }

    /// The dispatch's worker — the selected one, else the one that moved
    /// last — with the host it runs in and the repository a fresh session
    /// would open in. A relay goes to its own worktree, which the picker says
    /// separately; the bar names the repository either way.
    fn worker_host(&self) -> Option<(crate::roles::Backend, std::path::PathBuf)> {
        let scene = self.scene();
        let slot = dispatch_worker(&scene, self.ui.selected)?;
        let dir = dispatch_dir(&slot.cwd, false);
        Some((host_of(slot), dir))
    }

    /// Switch the picker's host to the other one, when that host could take
    /// the dispatch; otherwise say what it is missing, since `h` is the only
    /// way to learn a host is not set up without losing the pick.
    pub fn toggle_dispatch_host(&mut self, plan: &mut DispatchPlan) {
        self.choose_dispatch_host(plan.backend.other());
        // A refused switch leaves the plan as it was.
        if let Some(bar) = self.dispatch_bar() {
            let host = bar.active;
            plan.backend = host;
            plan.switchable = crate::roles::host_ready(host.other());
        }
    }

    /// Send `plan` to `role`: a relay with its task written, else a fresh
    /// session for you to give one.
    pub fn dispatch(&mut self, plan: &DispatchPlan, role: crate::roles::Role) {
        self.dispatch_with(plan, role, crate::roles::dispatch);
    }

    /// The injectable runner keeps dispatch timing and its landing testable
    /// without opening a real desktop session.
    fn dispatch_with(
        &mut self,
        plan: &DispatchPlan,
        role: crate::roles::Role,
        dispatch: impl FnOnce(
            crate::roles::Backend,
            crate::roles::Role,
            &std::path::Path,
            &str,
            bool,
        ) -> anyhow::Result<String>
        + Send
        + 'static,
    ) {
        let backend = plan.backend;
        if let Some(in_flight) = self.dispatches.out {
            self.note(
                &crate::tr!(
                    "上一次 {} 派活还没完成，等它出结果再派",
                    "wait for the current {} dispatch before starting another",
                    in_flight.short()
                ),
                Tone::Failed,
            );
            return;
        }
        self.dispatches.out = Some(backend);
        self.note(
            match backend {
                crate::roles::Backend::Dsh => crate::i18n::t(
                    "正在让 DSH 创建角色会话…",
                    "asking DSH to create the role session…",
                ),
                crate::roles::Backend::Claude => crate::i18n::t(
                    "正在让 Claude 打开会话…",
                    "asking Claude to open the session…",
                ),
            },
            Tone::Ok,
        );
        let tx = self.dispatches.tx.clone();
        let dir = plan.dir.clone();
        let task = plan
            .relay
            .as_ref()
            .map_or("", |r| r.task.as_str())
            .to_owned();
        let relay = plan.relay.is_some();
        std::thread::spawn(move || {
            let done = dispatch(backend, role, &dir, &task, false);
            let _ = tx.send((backend, role, relay, done));
        });
    }

    fn note(&mut self, text: &str, tone: Tone) {
        self.note = Some((text.to_owned(), tone, Instant::now()));
    }

    /// The footer's right-hand note: a fresh note on what a key did, else a
    /// fatal source warning, then the summary after a warning if hooks are out.
    fn status_note(&self) -> (String, Option<Tone>) {
        match &self.note {
            Some((text, tone, at)) if at.elapsed() < NOTE_FOR => (text.clone(), Some(*tone)),
            _ => {
                if let Some(warning) = &self.source_warning {
                    (warning.clone(), Some(Tone::Failed))
                } else if !self.hooks {
                    (format!("{} · {}", hooks_out(), self.summary_note()), None)
                } else {
                    (self.summary_note().to_owned(), None)
                }
            }
        }
    }

    /// Asked each frame, not kept: the language can change while running.
    fn summary_note(&self) -> &'static str {
        if self.ui.summaries {
            // The model's name is long and `doctor` already shows it.
            crate::i18n::t("摘要 ✓", "summary ✓")
        } else {
            // Not switched off: no AGENT_OFFICE_API_* to summarize with.
            crate::i18n::t("摘要未配置", "summary not set up")
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
    empty: bool,
    extra: &[(&str, &str)],
) {
    let dim = Style::default().fg(Color::Rgb(0x8a, 0x8f, 0x98));
    let key = Style::default().fg(Color::Rgb(0xe6, 0xe6, 0xe6));
    let hints = key_hints(core.head_labels, floors.n_floors(), empty, extra);
    let mut right = Vec::new();
    if waiting > 0 {
        let wait = Style::default().fg(panel::WAIT);
        right.push(Span::styled(
            crate::tr!("● {waiting} 个等你  ", "● {waiting} waiting  "),
            wait,
        ));
    }
    // The floor only where there is more than one to be on.
    if floors.n_floors() > 1 {
        right.push(Span::styled(
            crate::tr!(
                "{}/{} 层  ",
                "floor {}/{}  ",
                floors.current_floor() + 1,
                floors.n_floors()
            ),
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
    for (k, what, _) in fit_hints(hints, room) {
        spans.extend(hint_spans(k, what, cap, dim));
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

/// One hint as drawn: the key on a keycap, its action beside it. A hint
/// with no key is a pointer gesture told in words: on a keycap it read as
/// a key to press.
fn hint_spans(k: &str, what: &str, cap: Style, dim: Style) -> Vec<Span<'static>> {
    if k.is_empty() {
        return vec![Span::styled(format!("{what}  "), dim)];
    }
    vec![
        Span::styled(format!(" {k} "), cap),
        Span::styled(format!(" {what}  "), dim),
    ]
}

/// The footer's key hints (key, what it does, rank) in the order shown.
fn key_hints<'a>(
    head_labels: bool,
    n_floors: usize,
    empty: bool,
    extra: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str, u8)> {
    let labels = if head_labels {
        crate::i18n::t("隐藏任务", "hide tasks")
    } else {
        crate::i18n::t("显示任务", "show tasks")
    };
    // In the order shown; kept by `rank`, the lower the more needed.
    let base = [
        ("↑↓", crate::i18n::t("选中", "select"), 0),
        ("Enter", crate::i18n::t("展开", "expand"), 3),
        ("o", crate::i18n::t("打开会话", "open"), 0),
        ("w", crate::i18n::t("等你的", "waiting"), 4),
        ("n", crate::i18n::t("处理下一个", "next waiting"), 1),
        ("p", crate::i18n::t("派活", "dispatch"), 5),
        ("r", crate::i18n::t("重新总结", "resummarize"), 7),
        ("b", labels, 6),
    ];
    let floors_hint =
        (n_floors > 1).then_some(("[ ]", crate::i18n::t("换楼层", "floor"), 5));
    // Named in the language it switches to, so a reader who cannot read
    // the current one still finds it.
    let lang = ("l", crate::i18n::t("English", "中文"), 4);
    // With no one in the office, the keys act on no one: only switching
    // the language the setup steps are in, and quitting.
    base
        .into_iter()
        .filter(|_| !empty)
        .chain(floors_hint.filter(|_| !empty))
        .chain(extra.iter().filter(|_| !empty).map(|&(k, w)| (k, w, 8)))
        .chain([lang, ("q", crate::i18n::t("退出", "quit"), 2)])
        .collect()
}

/// The key hints (key, what it does, rank) that fit `room` cells, in their
/// order: the highest rank, the least needed, left out first, each whole.
fn fit_hints<'a>(mut hints: Vec<(&'a str, &'a str, u8)>, room: u16) -> Vec<(&'a str, &'a str, u8)> {
    let width = |&(k, what, _): &(&str, &str, u8)| {
        hint_spans(k, what, Style::default(), Style::default())
            .iter()
            .map(|s| crate::text::cells(&s.content))
            .sum::<u16>()
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

    /// Every Claude Code subagent is labelled "subagent": a PM's helpers
    /// read "subag…·1", "subag…·2", saying neither whose they are nor what
    /// they do. Named after the parent and the post, they say both.
    #[test]
    fn a_subagent_is_named_after_its_parent_and_post() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let mut start = |sid: &str, parent: Option<AgentId>| {
            let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
            r.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: pipeline::CC_SOURCE.into(),
                    session_id: sid.into(),
                    cwd: "/work/shop-app".into(),
                    parent_id: parent,
                },
                t0,
                Transport::Hook,
            );
            id
        };
        let pm = start("pm", None);
        let fe = start("fe", Some(pm));
        let explore = start("ex", Some(pm));
        let plain = start("plain", Some(pm));
        for id in [fe, explore, plain] {
            scene.agents.get_mut(&id).unwrap().label =
                pixtuoid_core::state::SlotLabel::renamed("subagent");
        }
        let views: tasks::TaskViews = Arc::new(HashMap::from([
            (
                fe,
                tasks::TaskView {
                    role: Some(crate::roles::Role::Frontend),
                    ..tasks::TaskView::default()
                },
            ),
            (
                explore,
                tasks::TaskView {
                    agent_type: Some("Explore".into()),
                    ..tasks::TaskView::default()
                },
            ),
        ]));
        let named = named_subagents(&Arc::new(scene), &views);
        assert_eq!(&*named.agents[&fe].label, "shop-app›前端");
        assert_eq!(&*named.agents[&explore].label, "shop-app›Explore");
        // Nothing known of it yet: left as it was rather than guessed.
        assert_eq!(&*named.agents[&plain].label, "subagent");
        assert!(named.agents[&pm].label.ends_with("shop-app"));
    }
    use super::*;
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};

    /// A view of a session that has run a turn: it has a prompt.
    fn ran() -> tasks::TaskView {
        tasks::TaskView {
            prompt: Some("修登录页".into()),
            ..tasks::TaskView::default()
        }
    }

    /// Views of `ids`, each having run a turn.
    fn all_ran(ids: &[AgentId]) -> tasks::TaskViews {
        Arc::new(ids.iter().map(|&id| (id, ran())).collect())
    }

    #[derive(Default)]
    struct TestFloors {
        current: usize,
        count: usize,
    }

    impl Floors for TestFloors {
        fn current_floor(&self) -> usize {
            self.current
        }

        fn n_floors(&self) -> usize {
            self.count.max(1)
        }

        fn navigate(&mut self, floor: usize, _now: SystemTime) {
            self.current = floor;
        }
    }

    /// A Core whose scene and task channels are in memory. The short-lived
    /// runtime provides pipeline-owned layout state and is shut down before
    /// the assertions; no app or GUI starts.
    fn core_with_scene(scene: SceneState) -> Core {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "agent-office-core-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let socket = root.join("hook.sock");
        let spawned = rt.block_on(async { pipeline::spawn(socket, 100, 40) });
        rt.shutdown_background();
        let _ = std::fs::remove_dir_all(&root);
        let pipeline::Pipeline {
            floor_caps,
            deaths_rx,
            ..
        } = spawned;
        let (scene_tx, scene_rx) = tokio::sync::watch::channel(Arc::new(scene));
        drop(scene_tx);
        let pipe = pipeline::Pipeline {
            scene_rx,
            floor_caps,
            deaths_rx,
        };
        let (_views_tx, views_rx) =
            tokio::sync::watch::channel(tasks::TaskViews::default());
        let (force_tx, _force_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_outlook_tx, outlook_rx) = tokio::sync::watch::channel(Outlook::default());
        let (_desktop_tx, desktop_rx) =
            tokio::sync::watch::channel(desktop::Looks::default());
        Core {
            ui: PanelState::default(),
            head_labels: true,
            hooks: true,
            note: None,
            source_warning: None,
            quit_armed: None,
            pipe,
            views_rx,
            force_tx,
            watch: Watch::default(),
            outlook_rx,
            desktop_rx,
            notifier: notify::Notifier::default(),
            banners: false,
            jumps: Jumps::default(),
            open_agent: jump::open_agent,
            dispatches: Dispatches::default(),
            host_at: None,
            picked_host: None,
            setup_checked: None,
        }
    }

    fn land_until_done(core: &mut Core) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while core.dispatches.out.is_some() && Instant::now() < deadline {
            core.land_dispatches();
            std::thread::sleep(Duration::from_millis(5));
        }
        core.land_dispatches();
        assert!(
            core.dispatches.out.is_none(),
            "dispatch result did not land"
        );
    }

    fn fake_open_agent(_scene: &SceneState, _id: AgentId) -> Result<&'static str, String> {
        Ok("test open; no GUI")
    }

    /// A failed dispatch can mean the result is unknown, so retrying blindly is
    /// unsafe. That immediate instruction must not disappear behind a source
    /// death; once its ordinary display window ends, the stale input stays
    /// visible and its retained health state remains intact.
    #[test]
    fn recent_action_note_precedes_a_source_warning_until_it_expires() {
        let mut core = core_with_scene(SceneState::uniform(8));
        let warning = "数据源 claude-code 已停止：listener exploded";
        core.source_warning = Some(warning.into());
        core.note("派活失败：结果未知，先检查勿重试", Tone::Failed);
        assert_eq!(
            core.status_note(),
            (
                "派活失败：结果未知，先检查勿重试".into(),
                Some(Tone::Failed)
            )
        );
        assert_eq!(core.source_warning.as_deref(), Some(warning));

        let (_, _, at) = core.note.as_mut().unwrap();
        *at = Instant::now()
            .checked_sub(NOTE_FOR + Duration::from_millis(1))
            .unwrap();
        assert_eq!(core.status_note(), (warning.into(), Some(Tone::Failed)));
        assert_eq!(core.source_warning.as_deref(), Some(warning));
    }

    /// A waiting child is already included in waiting counts and alerts. Both
    /// OpenNext and notification refocus must select that same child, and the
    /// jump must resolve it to the root session exactly as open_agent does.
    #[test]
    fn open_next_and_notification_refocus_reach_a_waiting_subagent() {
        let now = SystemTime::now();
        let started = now - Duration::from_secs(120);
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let mut start = |sid: &str, parent: Option<AgentId>| {
            let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
            reducer.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: pipeline::CC_SOURCE.into(),
                    session_id: sid.into(),
                    cwd: "/repo".into(),
                    parent_id: parent,
                },
                started,
                Transport::Hook,
            );
            id
        };
        let parent = start("parent", None);
        let child = start("agent-child", Some(parent));
        scene.agents.get_mut(&parent).unwrap().state =
            pixtuoid_core::ActivityState::Running { item: None };
        let waiting = scene.agents.get_mut(&child).unwrap();
        waiting.state = pixtuoid_core::ActivityState::RequiresAction { reason: None };
        waiting.state_started_at = now - Duration::from_secs(60);
        let views = tasks::TaskViews::default();
        let seen = HashMap::new();
        let rows = panel::order(&scene, &views, now, &seen);
        assert_eq!(panel::waiting(&scene, &views, &rows, &seen), [child]);

        let mut core = core_with_scene(scene.clone());
        core.open_agent = fake_open_agent;
        assert_eq!(core.next_waiting(), Some(child));
        let mut floors = TestFloors::default();
        core.act(Action::OpenNext, &mut floors);
        assert_eq!(core.ui.selected, Some(child));
        let (opened, opened_scene, done) = core
            .jumps
            .rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the injected opener ran without a GUI");
        assert!(done.is_ok());
        assert_eq!(opened, child);
        assert_eq!(
            jump::root_agent(&opened_scene, opened).unwrap().agent_id,
            parent,
            "OpenNext passed a child whose jump opens the root session"
        );

        // Prime before the wait began so the next tick is an actual unseen
        // banner, not merely an initial waiting session.
        let mut primed = scene.clone();
        primed.agents.get_mut(&child).unwrap().state =
            pixtuoid_core::ActivityState::Running { item: None };
        core.notifier.tick(&primed, &views, &seen, started, false);
        let tick = core.notifier.tick(&scene, &views, &seen, now, false);
        assert_eq!(tick.badge, Some(1));
        assert_eq!(tick.alerts.len(), 1);
        assert_eq!(tick.alerts[0].agent, child);
        assert!(core.notifier.unseen);
        core.ui.selected = None;
        core.focus_gained(&mut floors);
        assert_eq!(core.ui.selected, Some(child));
        assert!(!core.notifier.unseen);
    }

    /// Async landing must not flatten Claude's fresh and relay instructions or
    /// freeze them in one language. DSH already returns its richer route note,
    /// which is preserved verbatim.
    #[test]
    fn dispatch_success_notes_keep_host_kind_and_language() {
        use crate::i18n::{Lang, with};
        for (relay, zh, en) in [
            (
                false,
                "已在 Claude 开好「后端」的新会话，写上任务发送",
                "opened a new Backend session in Claude: write the task and send",
            ),
            (
                true,
                "已在 Claude 开好「后端」接力会话，看一眼就发送",
                "opened a Backend relay in Claude: check it and send",
            ),
        ] {
            assert_eq!(
                with(Lang::Zh, || {
                    dispatch_success(
                        crate::roles::Backend::Claude,
                        crate::roles::Role::Backend,
                        relay,
                        "CLI text stays out of the GUI note".into(),
                    )
                }),
                zh
            );
            assert_eq!(
                with(Lang::En, || {
                    dispatch_success(
                        crate::roles::Backend::Claude,
                        crate::roles::Role::Backend,
                        relay,
                        "CLI text stays out of the GUI note".into(),
                    )
                }),
                en
            );
        }
        assert_eq!(
            dispatch_success(
                crate::roles::Backend::Dsh,
                crate::roles::Role::Qa,
                true,
                "created and submitted a QA relay (shop · abc123)".into(),
            ),
            "created and submitted a QA relay (shop · abc123)"
        );
    }

    /// Claude's checked URL open may take seconds. It must happen off the UI
    /// thread: another action works while it is out, a repeat does not start a
    /// competing dispatch, and its eventual success and failure both land.
    #[test]
    fn claude_dispatch_stays_asynchronous_and_lands_its_outcome() {
        let mut core = core_with_scene(SceneState::uniform(8));
        let mut floors = TestFloors::default();
        let plan = DispatchPlan {
            dir: std::env::temp_dir(),
            backend: crate::roles::Backend::Claude,
            relay: None,
            switchable: false,
        };
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let releaser = std::thread::spawn(move || {
            // Bounds the fail-before run too: the old synchronous call returns
            // here and fails the elapsed assertion instead of hanging a test.
            std::thread::sleep(Duration::from_millis(600));
            let _ = release_tx.send(());
        });
        let started = Instant::now();
        core.dispatch_with(&plan, crate::roles::Role::Backend, move |backend, _, _, _, _| {
            assert_eq!(backend, crate::roles::Backend::Claude);
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok("the CLI keeps its own success text".into())
        });
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "dispatch blocked the UI thread for {:?}",
            started.elapsed()
        );
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        core.act(Action::ToggleLabels, &mut floors);
        assert!(!core.head_labels, "an action is handled while dispatch is out");
        assert_eq!(core.dispatches.out, Some(crate::roles::Backend::Claude));

        use std::sync::atomic::{AtomicUsize, Ordering};
        let duplicates = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&duplicates);
        core.dispatch_with(&plan, crate::roles::Role::Qa, move |_, _, _, _, _| {
            counted.fetch_add(1, Ordering::Relaxed);
            Ok("must not run".into())
        });
        land_until_done(&mut core);
        releaser.join().unwrap();
        assert_eq!(
            duplicates.load(Ordering::Relaxed),
            0,
            "one dispatch at a time"
        );
        assert_eq!(
            core.watch.stats.uses.get("dispatch.fresh.backend"),
            Some(&1)
        );
        let (note, tone, _) = core.note.as_ref().unwrap();
        assert_eq!(note, "已在 Claude 开好「后端」的新会话，写上任务发送");
        assert_eq!(*tone, Tone::Ok);

        core.dispatch_with(&plan, crate::roles::Role::Qa, |_, _, _, _, _| {
            Err(anyhow::anyhow!("injected failure"))
        });
        land_until_done(&mut core);
        let (note, tone, _) = core.note.as_ref().unwrap();
        assert_eq!(note, "派活失败：injected failure");
        assert_eq!(*tone, Tone::Failed);
        assert_eq!(core.watch.stats.uses.get("dispatch.failed"), Some(&1));
    }

    /// The rejection belongs to the host already working, not to the new plan
    /// it refused. Exercise both directions without a real desktop app, then
    /// cross the async landing before proving the other host can now run.
    fn assert_busy_dispatch_names_in_flight_host(in_flight: crate::roles::Backend) {
        use crate::i18n::{Lang, with};
        use crate::roles::Backend;

        let rejected = in_flight.other();
        let plan = |backend| DispatchPlan {
            dir: std::env::temp_dir(),
            backend,
            relay: None,
            switchable: true,
        };
        let mut core = core_with_scene(SceneState::uniform(8));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        core.dispatch_with(
            &plan(in_flight),
            crate::roles::Role::Backend,
            move |backend, _, _, _, _| {
                assert_eq!(backend, in_flight);
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("in-flight dispatch landed".into())
            },
        );
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        core.dispatch_with(&plan(rejected), crate::roles::Role::Qa, |_, _, _, _, _| {
            panic!("a competing dispatch must not run")
        });
        let (note, tone, _) = core.note.as_ref().unwrap();
        let lost = match in_flight {
            Backend::Dsh => "上一次 DSH 派活还没完成，等它出结果再派",
            Backend::Claude => "上一次 Claude 派活还没完成，等它出结果再派",
        };
        assert_eq!(note, lost);
        assert_eq!(*tone, Tone::Failed);

        with(Lang::En, || {
            core.dispatch_with(&plan(rejected), crate::roles::Role::Qa, |_, _, _, _, _| {
                panic!("a competing dispatch must not run")
            });
            let (note, tone, _) = core.note.as_ref().unwrap();
            assert_eq!(
                note,
                &format!(
                    "wait for the current {} dispatch before starting another",
                    in_flight.short()
                )
            );
            assert_eq!(*tone, Tone::Failed);
        });

        release_tx.send(()).unwrap();
        land_until_done(&mut core);
        assert_eq!(
            core.watch.stats.uses.get("dispatch.fresh.backend"),
            Some(&1),
            "the original dispatch still lands and is counted"
        );
        with(Lang::En, || {
            core.dispatch_with(
                &plan(rejected),
                crate::roles::Role::Qa,
                move |backend, _, _, _, _| {
                    assert_eq!(backend, rejected);
                    Err(anyhow::anyhow!("next dispatch failed"))
                },
            );
            land_until_done(&mut core);
            let (note, tone, _) = core.note.as_ref().unwrap();
            assert_eq!(note, "dispatch failed: next dispatch failed");
            assert_eq!(*tone, Tone::Failed);
        });
        assert!(core.dispatches.out.is_none());
        assert_eq!(core.watch.stats.uses.get("dispatch.failed"), Some(&1));
    }

    #[test]
    fn dsh_in_flight_rejection_does_not_blame_claude() {
        assert_busy_dispatch_names_in_flight_host(crate::roles::Backend::Dsh);
    }

    #[test]
    fn claude_in_flight_rejection_does_not_blame_dsh() {
        assert_busy_dispatch_names_in_flight_host(crate::roles::Backend::Claude);
    }

    /// Two fresh sessions must not edit one ordinary git worktree at once, but
    /// a relay belongs beside that worktree's uncommitted changes. Exercise the
    /// real git layout: pixtuoid's seat helper only knows `.claude/worktrees`.
    #[test]
    fn fresh_dispatch_uses_main_checkout_while_relay_keeps_ordinary_worktree() {
        let root = std::env::temp_dir().join(format!(
            "agent-office-core-worktree-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&main, &["init", "-q"]);
        git(
            &main,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
        let worktree = root.join("wt");
        git(
            &main,
            &["worktree", "add", "-q", worktree.to_str().unwrap()],
        );
        let main = main.canonicalize().unwrap();
        assert_eq!(
            pixtuoid_core::state::project_root(&worktree),
            worktree,
            "the regression really is an ordinary worktree, not a pixtuoid seat"
        );
        assert_eq!(dispatch_dir(&worktree, false), main, "fresh");
        assert_eq!(dispatch_dir(&worktree, true), worktree, "relay");
        let _ = std::fs::remove_dir_all(&root);
    }

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

    /// The language key is the one way to switch while running, so it must
    /// be offered even to an empty office, whose setup steps are the words
    /// most worth reading, and be named in the language it switches to: a
    /// reader stuck in the wrong one cannot read a label in it.
    #[test]
    fn the_language_key_is_offered_in_the_language_it_switches_to() {
        use crate::i18n::{Lang, with};
        let lang_hint = |l| {
            with(l, || {
                key_hints(true, 1, true, &[])
                    .into_iter()
                    .find(|h| h.0 == "l")
                    .map(|h| h.1)
            })
        };
        assert_eq!(lang_hint(Lang::Zh), Some("English"));
        assert_eq!(lang_hint(Lang::En), Some("中文"));
        let busy = with(Lang::Zh, || key_hints(true, 1, false, &[]));
        assert!(busy.iter().any(|h| h.0 == "l"), "and in a busy one");
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

    /// "点门牌" sat on a keycap like `t` and `q` beside it, so it read as a key
    /// to press, and pressing it did nothing. A hint with no key is plain
    /// words, measured as drawn so the row still fits.
    #[test]
    fn a_pointer_hint_is_not_drawn_as_a_key() {
        let cap = Style::default().bg(Color::Rgb(1, 2, 3));
        let spans = hint_spans("", "点门牌看说明", cap, Style::default());
        assert!(spans.iter().all(|s| s.style.bg != cap.bg), "no keycap");
        assert_eq!(spans[0].content, "点门牌看说明  ");
        let keyed = hint_spans("t", "置顶", cap, Style::default());
        assert_eq!(keyed[0].style.bg, cap.bg, "a real key keeps its cap");
        let w = crate::text::cells("点门牌看说明  ");
        assert_eq!(fit_hints(vec![("", "点门牌看说明", 8)], w).len(), 1);
        assert!(fit_hints(vec![("", "点门牌看说明", 8)], w - 1).is_empty());
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
    /// line in your office to rest in the pantry, not up to the rooftop: it
    /// draws that one's idle stretch as past the line's wait but short of the
    /// rooftop's, and leaves every other session, and the scene the panel
    /// reads, alone, so your office holds only what you have not seen.
    #[test]
    fn a_read_session_leaves_the_office_line() {
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
        let views = all_ran(&ids);
        assert!(Arc::ptr_eq(
            &as_seen(&scene, &views, &HashMap::new(), false, now),
            &scene
        ));
        let seen = HashMap::from([(ids[0], now - Duration::from_secs(5))]);
        let drawn = as_seen(&scene, &views, &seen, false, now);
        let idle_for =
            |s: &SceneState, id| now.duration_since(s.agents[id].state_started_at).unwrap();
        let read_for = idle_for(&drawn, &ids[0]).as_millis();
        assert!(
            read_for >= u128::from(pixtuoid_scene::waiting::DONE_WAITS_MS),
            "out of the line"
        );
        assert!(
            read_for < u128::from(pixtuoid_scene::pose::IDLE_ROOFTOP_MS),
            "resting in the pantry, not on the rooftop"
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
            ..ran()
        };
        let views: tasks::TaskViews =
            Arc::new([(ids[0], ran()), (ids[1], asked)].into_iter().collect());
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

    /// At lunch, or with a typhoon closing the rooftop, every idle agent
    /// resting is drawn in the cha chaan teng, even one read and one idle for
    /// an hour, rather than back in your office; one just gone idle keeps its
    /// own stretch and still lines up with its result, and a working one is
    /// left at its desk.
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
        let idle_since = [3600, 120, 30, 3600];
        for (id, secs) in ids.iter().zip(idle_since) {
            let s = scene.agents.get_mut(id).unwrap();
            s.state = pixtuoid_core::ActivityState::Idle;
            s.state_started_at = now - Duration::from_secs(secs);
        }
        scene.agents.get_mut(&ids[3]).unwrap().state =
            pixtuoid_core::ActivityState::Running { item: None };
        let scene = Arc::new(scene);
        let seen = HashMap::from([(ids[1], now - Duration::from_secs(5))]);
        let drawn = as_seen(&scene, &all_ran(&ids), &seen, true, now);
        let idle_ms = |id: &AgentId| {
            now.duration_since(drawn.agents[id].state_started_at)
                .unwrap()
                .as_millis() as u64
        };
        let rooftop = pixtuoid_scene::pose::IDLE_ROOFTOP_MS;
        let line = pixtuoid_scene::waiting::DONE_WAITS_MS;
        assert!(
            (line..rooftop).contains(&idle_ms(&ids[0])),
            "an hour idle, still in the pantry"
        );
        assert!(
            (line..rooftop).contains(&idle_ms(&ids[1])),
            "read, but down for lunch"
        );
        assert_eq!(idle_ms(&ids[2]), 30_000, "just idle, left as it is");
        assert!(
            pixtuoid_scene::waiting::lines_up(&drawn.agents[&ids[2]], now),
            "just idle, it still lines up with its result"
        );
        assert_eq!(idle_ms(&ids[3]), 3_600_000, "working, left at its desk");
    }

    /// The guest chair and `n` agree: both go by the office's own line on
    /// the scene as drawn. A result you have not read sits there while a
    /// prompt is fresh; once the prompt has waited long enough to walk over,
    /// it takes the chair, and `n` opens it.
    #[test]
    fn the_guest_chair_is_who_n_opens() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let [result, prompt] = ["result", "prompt"].map(|sid| {
            let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
            r.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: pipeline::CC_SOURCE.into(),
                    session_id: sid.into(),
                    cwd: "/repo".into(),
                    parent_id: None,
                },
                t0,
                Transport::Hook,
            );
            id
        });
        let now = t0 + Duration::from_secs(600);
        let s = scene.agents.get_mut(&result).unwrap();
        s.state = pixtuoid_core::ActivityState::Idle;
        s.state_started_at = now - Duration::from_secs(60);
        let s = scene.agents.get_mut(&prompt).unwrap();
        s.state = pixtuoid_core::ActivityState::RequiresAction { reason: None };
        s.state_started_at = now - Duration::from_secs(5);
        let scene = Arc::new(scene);
        let views = all_ran(&[result, prompt]);
        // Who the office really seats in the guest chair.
        let layout = pixtuoid_scene::layout::SceneLayout::compute(240, 160, None).expect("fits");
        let in_chair = |drawn: &SceneState, now| {
            let agents: Vec<AgentSlot> = drawn.agents.values().cloned().collect();
            pixtuoid_scene::waiting::guest_chair(&agents, now, &layout)
        };
        for (later, first) in [(0, result), (20, prompt)] {
            let now = now + Duration::from_secs(later);
            let drawn = as_seen(&scene, &views, &HashMap::new(), false, now);
            assert_eq!(panel::guest_chair(&drawn, now), Some(first), "+{later}s");
            assert_eq!(in_chair(&drawn, now), Some(first), "+{later}s");
            // With no sub-agent waiting, `n` opens whoever the chair holds.
            assert_eq!(
                panel::next_waiting(&scene, &views, now, &HashMap::new()),
                Some(first),
                "+{later}s"
            );
        }
    }

    /// Your office's sign on a floor opens whoever sits in that floor's
    /// guest chair, which `n`, going by the whole building, may not: an
    /// earlier prompt upstairs takes `n`, not the chair you clicked.
    #[test]
    fn each_floors_sign_opens_its_own_guest_chair() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut caps = [0; pixtuoid_core::state::MAX_FLOORS];
        caps[0] = 1;
        caps[1] = 1;
        let mut scene = SceneState::new(caps);
        let mut r = Reducer::new();
        let [down, up] = ["down", "up"].map(|sid| {
            let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
            r.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: pipeline::CC_SOURCE.into(),
                    session_id: sid.into(),
                    cwd: "/repo".into(),
                    parent_id: None,
                },
                t0,
                Transport::Hook,
            );
            id
        });
        assert_eq!((scene.agents[&down].floor_idx, scene.agents[&up].floor_idx), (0, 1));
        let now = t0 + Duration::from_secs(600);
        let s = scene.agents.get_mut(&down).unwrap();
        s.state = pixtuoid_core::ActivityState::Idle;
        s.state_started_at = now - Duration::from_secs(60);
        let s = scene.agents.get_mut(&up).unwrap();
        s.state = pixtuoid_core::ActivityState::RequiresAction { reason: None };
        s.state_started_at = now - Duration::from_secs(60);
        let (scene, views) = (Arc::new(scene), all_ran(&[down, up]));
        let drawn = as_seen(&scene, &views, &HashMap::new(), false, now);
        assert_eq!(
            panel::next_waiting(&scene, &views, now, &HashMap::new()),
            Some(up),
            "n: the prompt upstairs"
        );
        assert_eq!(guest_of(&drawn, 0, now), Some(down), "this floor's chair");
        assert_eq!(guest_of(&drawn, 1, now), Some(up));
    }

    /// A session opened and never given a task has no result: the guest
    /// chair and `n` must not point at one with nothing to look at, so it
    /// rests and the panel files it idle. One dispatched in a role and
    /// waiting for its task still waits on you.
    #[test]
    fn a_session_never_given_a_task_does_not_queue() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let [fresh, dispatched] = ["fresh", "dispatched"].map(|sid| {
            let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
            r.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: pipeline::CC_SOURCE.into(),
                    session_id: sid.into(),
                    cwd: "/repo".into(),
                    parent_id: None,
                },
                t0,
                Transport::Hook,
            );
            id
        });
        let now = t0 + Duration::from_secs(60);
        for id in [fresh, dispatched] {
            let s = scene.agents.get_mut(&id).unwrap();
            s.state = pixtuoid_core::ActivityState::Idle;
            s.state_started_at = t0;
        }
        let scene = Arc::new(scene);
        let none = tasks::TaskViews::default();
        let drawn = as_seen(&scene, &none, &HashMap::new(), false, now);
        assert_eq!(
            pixtuoid_scene::waiting::need(&drawn.agents[&fresh], now),
            None,
            "nothing to read"
        );
        assert_eq!(panel::next_waiting(&scene, &none, now, &HashMap::new()), None);
        let group = |id: AgentId, views: &tasks::TaskViews| {
            panel::Group::of(&scene.agents[&id], views.get(&id), now, false)
        };
        assert_eq!(group(fresh, &none), panel::Group::Idle);

        let waiting_for_task = tasks::TaskView {
            needs_task: true,
            ..tasks::TaskView::default()
        };
        let views: tasks::TaskViews =
            Arc::new(HashMap::from([(dispatched, waiting_for_task)]));
        assert_eq!(
            panel::next_waiting(&scene, &views, now, &HashMap::new()),
            Some(dispatched)
        );
        assert_eq!(group(dispatched, &views), panel::Group::Waiting);
        assert_eq!(group(fresh, &views), panel::Group::Idle);
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

    /// A session that backgrounds its subagents ends its turn while they
    /// run: drawn by its own state it read 「闲着」 and walked off to the
    /// rooftop while its helpers still worked. It works until they are done;
    /// one that asked you something still waits on you.
    #[test]
    fn a_session_with_a_helper_at_work_is_working() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(16);
        let mut r = Reducer::new();
        let mut start = |sid: &str, parent: Option<AgentId>| {
            let id = AgentId::from_parts(pipeline::CC_SOURCE, sid);
            r.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: pipeline::CC_SOURCE.into(),
                    session_id: sid.into(),
                    cwd: "/repo".into(),
                    parent_id: parent,
                },
                now,
                Transport::Hook,
            );
            id
        };
        let lead = start("lead", None);
        let helper = start("agent-helper", Some(lead));
        let deep = start("deep", None);
        let mid = start("agent-mid", Some(deep));
        let leaf = start("agent-leaf", Some(mid));
        let asked = start("asked", None);
        let asked_helper = start("agent-asked", Some(asked));
        let done = start("done", None);
        let done_helper = start("agent-done", Some(done));
        let running = pixtuoid_core::ActivityState::Running { item: None };
        for id in [helper, leaf, asked_helper] {
            scene.agents.get_mut(&id).unwrap().state = running.clone();
        }
        for id in [lead, deep, mid, asked, done, done_helper] {
            scene.agents.get_mut(&id).unwrap().state = pixtuoid_core::ActivityState::Idle;
        }
        let views: tasks::TaskViews = Arc::new(HashMap::from([
            (
                asked,
                tasks::TaskView {
                    outcome: Some("问：先合哪个？".into()),
                    outcome_summarized: true,
                    ..Default::default()
                },
            ),
            (done, ran()),
        ]));
        let seen = HashMap::from([(lead, now)]);
        let drawn = working_through_helpers(&Arc::new(scene), &views, &seen);
        let group = |id: AgentId| {
            let s = &drawn.agents[&id];
            panel::Group::of(s, views.get(&id), now, panel::read_since_idle(s, &seen))
        };
        assert_eq!(group(lead), panel::Group::Working, "its helper still runs, though you read it");
        assert_eq!(group(deep), panel::Group::Working, "a grandchild at work counts");
        assert_eq!(group(mid), panel::Group::Working);
        assert_eq!(group(asked), panel::Group::Waiting, "a question to you still waits");
        assert_eq!(group(done), panel::Group::Done, "helpers done: done");
        let chip = |id: AgentId| {
            panel::status(&drawn.agents[&id], views.get(&id), false, now).0
        };
        assert!(chip(lead).contains("等子代理"), "not thinking: {}", chip(lead));
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

    /// A hidden session still holds its desk in the reducer, so with the
    /// ground floor full of sessions you were done with, the ones you can see
    /// were seated upstairs: three floors for a dozen people, and no way to
    /// tell why. The office as drawn seats only those it draws.
    #[test]
    fn hidden_sessions_do_not_push_the_shown_ones_upstairs() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(5 * 3600);
        let mut scene = SceneState::uniform(2);
        let mut r = Reducer::new();
        let ids: Vec<AgentId> = ["old-1", "old-2", "new-1", "new-2"]
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
        for id in &ids[..2] {
            let s = scene.agents.get_mut(id).unwrap();
            s.state = pixtuoid_core::ActivityState::Idle;
            s.state_started_at = now - Duration::from_secs(2 * 3600);
        }
        for id in &ids[2..] {
            scene.agents.get_mut(id).unwrap().state =
                pixtuoid_core::ActivityState::Running { item: None };
        }
        assert_eq!(scene.agents[&ids[2]].floor_idx, 1, "the reducer seats it upstairs");

        let shown = without_long_idle(&Arc::new(scene), &tasks::TaskViews::default(), &HashMap::new(), now);
        for id in &ids[2..] {
            let s = &shown.agents[id];
            assert_eq!(s.floor_idx, 0, "{id:?} is drawn on the ground floor");
            assert_eq!(shown.floor_of(s.desk_index), 0);
        }
        assert_ne!(shown.agents[&ids[2]].desk_index, shown.agents[&ids[3]].desk_index);
        assert_eq!(shown.floors_in_use(), 1, "one floor is all it needs");
    }
}
