//! Each agent's task, as one line: a background loop tails the transcripts of
//! the agents the office shows and keeps their summaries fresh. A dsh
//! session's transcript is the task log agent-office's dsh plugin writes; a
//! Codex session's is its rollout.

pub mod client;
pub mod extract;
pub mod memo;
pub mod trigger;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};
use tokio::sync::{mpsc, watch};

use crate::pipeline::{CC_SOURCE, CODEX_SOURCE, CURSOR_SOURCE, is_shown_source};
use crate::text::truncate_cells;
use client::{OutcomeInput, SUMMARY_CELLS, Summarizer, SummaryInput};
pub use extract::{Cheer, Phase};
use extract::{SubMeta, Tail, Transcript};
use trigger::Trigger;

/// What the panel and the head labels show for one agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskView {
    /// The task line: the model's summary, a subagent's own description, or
    /// the raw prompt cut short when neither is there.
    pub line: Option<String>,
    /// `line` came from the model or a subagent description, not the raw prompt.
    pub summarized: bool,
    /// The request is out.
    pub pending: bool,
    pub prompt: Option<String>,
    pub todo: Option<String>,
    pub recent_tools: Vec<String>,
    pub agent_type: Option<String>,
    /// Its post in the office: a subagent's `office-*` type, else the role
    /// its prompt's marker gave the session.
    pub role: Option<crate::roles::Role>,
    /// Its whole todo list, in order.
    pub todos: Vec<(extract::TodoState, String)>,
    pub phase: Option<Phase>,
    pub stuck: Option<Stuck>,
    /// When the current turn began: the human's last prompt.
    pub turn_started: Option<SystemTime>,
    /// What the turn came to, from the agent's last reply: the model's line,
    /// or the reply cut short when there is none.
    pub outcome: Option<String>,
    /// `outcome` came from the model.
    pub outcome_summarized: bool,
    /// The agent's last reply.
    pub reply: Option<String>,
    /// The last thing it pulled off worth a cheer, and when.
    pub cheer: Option<(Cheer, SystemTime)>,
    /// When the transcript was last written.
    pub written_at: Option<SystemTime>,
}

/// Why an agent looks stuck.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stuck {
    /// This many tool results in a row were errors.
    Failing(u32),
    /// Working, but the transcript has not grown for this many minutes.
    Quiet(u64),
}

impl Stuck {
    pub fn describe(self) -> String {
        match self {
            Stuck::Failing(n) => crate::tr!("连续失败 {n} 次", "{n} failures in a row"),
            Stuck::Quiet(m) => crate::tr!("{m} 分钟没动静", "quiet for {m} min"),
        }
    }
}

/// When a working agent counts as stuck. A repository whose builds or tests
/// keep quiet for longer than these would be flagged all day, so each can be
/// raised from the environment ([`StuckLimits::from_env`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StuckLimits {
    /// This many errors in a row.
    pub fails: u32,
    /// The transcript this quiet.
    pub quiet: Duration,
    /// The same while a call that may run long is out: a build, a subagent.
    pub quiet_long_call: Duration,
}

impl StuckLimits {
    pub const DEFAULT: Self = Self {
        fails: 3,
        quiet: Duration::from_secs(5 * 60),
        quiet_long_call: Duration::from_secs(20 * 60),
    };

    /// The defaults, each overridden by `AGENT_OFFICE_STUCK_FAILS` (errors in
    /// a row), `AGENT_OFFICE_STUCK_QUIET` and `AGENT_OFFICE_STUCK_QUIET_LONG`
    /// (minutes) when set to a whole number above zero. The names of the
    /// ones set but not taken are returned too, for `doctor` to point out.
    pub fn from_env() -> (Self, Vec<&'static str>) {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> (Self, Vec<&'static str>) {
        let mut bad = Vec::new();
        let mut read = |k: &'static str| {
            let v = var(k)?;
            let n = v.trim().parse::<u32>().ok().filter(|n| *n > 0);
            if n.is_none() {
                bad.push(k);
            }
            n
        };
        let d = Self::DEFAULT;
        let minutes = |m: u32| Duration::from_secs(u64::from(m) * 60);
        let limits = Self {
            fails: read("AGENT_OFFICE_STUCK_FAILS").unwrap_or(d.fails),
            quiet: read("AGENT_OFFICE_STUCK_QUIET").map_or(d.quiet, minutes),
            quiet_long_call: read("AGENT_OFFICE_STUCK_QUIET_LONG")
                .map_or(d.quiet_long_call, minutes),
        };
        (limits, bad)
    }
}

/// The limits this process runs with, read once.
fn stuck_limits() -> &'static StuckLimits {
    static LIMITS: std::sync::OnceLock<StuckLimits> = std::sync::OnceLock::new();
    LIMITS.get_or_init(|| StuckLimits::from_env().0)
}
const LONG_CALLS: &[&str] = &[
    "Bash",
    "Task",
    "Agent",
    // dsh's names for the same calls, and its wait on a background job.
    "bash",
    "subagent",
    "subagent_fork",
    "job_output",
    // Codex's: its shells, a running command it feeds, and its wait on agents.
    "exec_command",
    "shell",
    "local_shell",
    "shell_command",
    "write_stdin",
    "wait",
    // Cursor's shell.
    "Shell",
    "shell",
];

fn stuck(t: &Transcript, working: bool, quiet: Duration, limits: &StuckLimits) -> Option<Stuck> {
    if t.fail_streak >= limits.fails {
        return Some(Stuck::Failing(t.fail_streak));
    }
    let limit = if t.awaits_any(LONG_CALLS) {
        limits.quiet_long_call
    } else {
        limits.quiet
    };
    (working && quiet >= limit).then(|| Stuck::Quiet(quiet.as_secs() / 60))
}

/// The stuck verdict that holds for `slot`: only a working agent can be stuck,
/// since one waiting on you or idle is not trying anything.
pub fn stuck_at_work(slot: &pixtuoid_core::AgentSlot, view: Option<&TaskView>) -> Option<Stuck> {
    view.and_then(|v| v.stuck).filter(|_| {
        slot.exiting_at.is_none() && matches!(slot.state, ActivityState::Running { .. })
    })
}

pub type TaskViews = Arc<HashMap<AgentId, TaskView>>;

#[derive(Debug)]
struct Entry {
    tail: Tail,
    transcript: Transcript,
    sub: Option<SubMeta>,
    trigger: Trigger,
    summary: Option<String>,
    /// The outcome line and the reply it was read from.
    outcome: Option<(String, String)>,
    outcome_in_flight: bool,
    /// After a failed outcome request, when to try again.
    outcome_retry_at: Option<Instant>,
    /// Outcome requests failed in a row, for the backoff.
    outcome_failures: u32,
    /// When to read the subagent's meta.json again for a description.
    sub_meta_at: Instant,
    /// When the transcript last gained a line.
    grew_at: Instant,
}

#[derive(Debug)]
struct Locator {
    /// Every transcript under a root, keyed by agent.
    scan: fn(&Path) -> HashMap<AgentId, PathBuf>,
    paths: HashMap<AgentId, PathBuf>,
    /// Ids the last scan did not find, so an agent with no transcript costs
    /// one scan per [`RESCAN_MISSED`], not one per tick.
    missed: HashSet<AgentId>,
    last_scan: Option<Instant>,
}

const RESCAN_MISSED: Duration = Duration::from_secs(30);
/// The least time between two scans, however many sessions are new.
const SCAN_GAP: Duration = Duration::from_secs(1);
/// How often a subagent's meta.json is read again while it has no description.
const SUB_META_EVERY: Duration = Duration::from_secs(5);

impl Locator {
    fn new(scan: fn(&Path) -> HashMap<AgentId, PathBuf>) -> Self {
        Self {
            scan,
            paths: HashMap::new(),
            missed: HashSet::new(),
            last_scan: None,
        }
    }

    fn find(&mut self, id: AgentId, root: &Path, now: Instant) -> Option<PathBuf> {
        if let Some(p) = self.paths.get(&id) {
            return Some(p.clone());
        }
        let stale = self
            .last_scan
            .is_none_or(|t| now.duration_since(t) >= RESCAN_MISSED);
        if self.missed.contains(&id) && !stale {
            return None;
        }
        // Several new sessions in one tick share a scan: one not found yet
        // is looked for again next tick, not marked missed for a while.
        if self
            .last_scan
            .is_some_and(|t| now.duration_since(t) < SCAN_GAP)
        {
            return None;
        }
        self.last_scan = Some(now);
        self.paths = (self.scan)(root);
        self.missed.retain(|m| !self.paths.contains_key(m));
        let found = self.paths.get(&id).cloned();
        if found.is_none() {
            self.missed.insert(id);
        }
        found
    }
}

/// Every transcript under `projects/`, keyed as pixtuoid keys agents:
/// `<session>.jsonl` and `<session>/subagents/agent-<id>.jsonl` by stem.
/// Directory reads only, no per-file stat: there are thousands.
fn scan(root: &Path) -> HashMap<AgentId, PathBuf> {
    let mut out = HashMap::new();
    let mut add = |p: PathBuf| {
        if p.extension().is_some_and(|e| e == "jsonl") {
            let stem = pixtuoid_core::source::claude_code::cc_id_from_path(&p);
            out.insert(AgentId::from_parts(CC_SOURCE, &stem), p);
        }
    };
    let Ok(projects) = std::fs::read_dir(root) else {
        return out;
    };
    for project in projects.flatten() {
        let Ok(items) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for item in items.flatten() {
            if item.file_type().is_ok_and(|t| t.is_dir()) {
                if let Ok(subs) = std::fs::read_dir(item.path().join("subagents")) {
                    subs.flatten().for_each(|s| add(s.path()));
                }
            } else {
                add(item.path());
            }
        }
    }
    out
}

/// Every rollout under Codex's `sessions/YYYY/MM/DD/`, keyed as pixtuoid keys
/// Codex agents: by the UUID that ends `rollout-<time>-<UUID>.jsonl`.
fn scan_codex(root: &Path) -> HashMap<AgentId, PathBuf> {
    fn walk(dir: &Path, depth: u8, out: &mut HashMap<AgentId, PathBuf>) {
        let Ok(items) = std::fs::read_dir(dir) else {
            return;
        };
        for item in items.flatten() {
            let p = item.path();
            if item.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 3 {
                    walk(&p, depth + 1, out);
                }
            } else if let Some(id) = codex_rollout_id(&p) {
                out.insert(AgentId::from_parts(CODEX_SOURCE, id), p);
            }
        }
    }
    let mut out = HashMap::new();
    walk(root, 0, &mut out);
    out
}

/// The session UUID a rollout's file name ends in.
fn codex_rollout_id(path: &Path) -> Option<&str> {
    if path.extension()? != "jsonl" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?.strip_prefix("rollout-")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    let b = id.as_bytes();
    let uuid = b.iter().enumerate().all(|(i, &c)| match i {
        8 | 13 | 18 | 23 => c == b'-',
        _ => c.is_ascii_hexdigit(),
    });
    uuid.then_some(id)
}

/// `$CLAUDE_CONFIG_DIR`, else `~/.claude`, as Claude Code resolves it.
pub fn claude_dir() -> Option<PathBuf> {
    pixtuoid_core::source::claude_code::claude_config_dir()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

pub fn projects_root() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("projects"))
}

/// The task log agent-office keeps for `session_id` (its dsh plugin, its
/// Cursor forwarder), named the way the plugin names it.
pub fn task_log(dir: &Path, session_id: &str) -> PathBuf {
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{safe}.jsonl"))
}

/// How long a task log agent-office writes is kept after its last line.
const TASK_LOG_KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Remove the task logs in `dir` untouched for [`TASK_LOG_KEEP`]: they are
/// only ever appended to, one per session, and a session that quiet is gone.
fn prune_task_logs(dir: &Path, now: SystemTime) {
    let Ok(items) = std::fs::read_dir(dir) else {
        return;
    };
    for item in items.flatten() {
        let p = item.path();
        let old = item
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|d| d >= TASK_LOG_KEEP));
        if old && p.extension().is_some_and(|e| e == "jsonl") {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Spawn the loop; the views it publishes, and where to ask for a re-summary.
pub fn spawn(
    scene_rx: watch::Receiver<Arc<SceneState>>,
    summarizer: Option<Summarizer>,
) -> (watch::Receiver<TaskViews>, mpsc::UnboundedSender<AgentId>) {
    let (views_tx, views_rx) = watch::channel(TaskViews::default());
    let (force_tx, force_rx) = mpsc::unbounded_channel();
    tokio::spawn(run(scene_rx, summarizer, views_tx, force_rx));
    (views_rx, force_tx)
}

async fn run(
    scene_rx: watch::Receiver<Arc<SceneState>>,
    summarizer: Option<Summarizer>,
    views_tx: watch::Sender<TaskViews>,
    mut force_rx: mpsc::UnboundedReceiver<AgentId>,
) {
    let root = projects_root();
    let dsh_dir = crate::dsh::transcripts_dir();
    let cursor_dir = crate::cursor::transcripts_dir();
    let mut entries: HashMap<AgentId, Entry> = HashMap::new();
    let codex_root = pixtuoid_core::source::codex::codex_home().join("sessions");
    let mut locator = Locator::new(scan);
    let mut codex_locator = Locator::new(scan_codex);
    let mut memo = tokio::task::block_in_place(|| {
        for dir in [&dsh_dir, &cursor_dir].into_iter().flatten() {
            prune_task_logs(dir, SystemTime::now());
        }
        memo::Memo::load()
    });
    // Each reply with the memo key of the request it answers.
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<(AgentId, String, Option<String>)>();
    // Each outcome reply with the agent reply it was asked for, and its key.
    let (outcome_tx, mut outcome_rx) =
        mpsc::unbounded_channel::<(AgentId, String, String, Option<String>)>();
    let mut tick = tokio::time::interval(Duration::from_millis(700));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            Some((id, key, out)) = done_rx.recv() => {
                if let Some(line) = &out {
                    memo.insert(key, line.clone());
                }
                if let Some(e) = entries.get_mut(&id) {
                    // A failed refresh keeps the last good line.
                    match out {
                        Some(line) => {
                            e.trigger.finished();
                            e.summary = Some(line);
                        }
                        None => e.trigger.failed(Instant::now()),
                    }
                }
            }
            Some((id, reply, key, out)) = outcome_rx.recv() => {
                if let Some(line) = &out {
                    memo.insert(key, line.clone());
                }
                if let Some(e) = entries.get_mut(&id) {
                    e.outcome_in_flight = false;
                    match out {
                        Some(line) => {
                            e.outcome = Some((reply, line));
                            e.outcome_retry_at = None;
                            e.outcome_failures = 0;
                        }
                        None => {
                            e.outcome_failures += 1;
                            e.outcome_retry_at =
                                Some(Instant::now() + trigger::retry_after(e.outcome_failures));
                        }
                    }
                }
            }
            Some(id) = force_rx.recv() => {
                if let Some(e) = entries.get_mut(&id) {
                    e.trigger.force(Instant::now());
                }
            }
        }
        let scene = scene_rx.borrow().clone();
        let now = Instant::now();
        entries.retain(|id, _| scene.agents.contains_key(id));
        // Transcript reads and directory scans are blocking file I/O.
        tokio::task::block_in_place(|| {
            memo.save_due(now);
            for (id, slot) in &scene.agents {
                if !is_shown_source(&slot.source) {
                    continue;
                }
                if !entries.contains_key(id) {
                    let path = match &*slot.source {
                        crate::dsh::SOURCE => dsh_dir
                            .as_deref()
                            .map(|d| task_log(d, &slot.session_id))
                            .filter(|p| p.is_file()),
                        CURSOR_SOURCE => cursor_dir
                            .as_deref()
                            .map(|d| task_log(d, &slot.session_id))
                            .filter(|p| p.is_file()),
                        CODEX_SOURCE => codex_locator.find(*id, &codex_root, now),
                        _ => root.as_deref().and_then(|r| locator.find(*id, r, now)),
                    };
                    let Some(path) = path else {
                        continue;
                    };
                    let sub = extract::sub_meta(&path);
                    entries.insert(
                        *id,
                        Entry {
                            tail: Tail::new(path),
                            transcript: Transcript::default(),
                            sub,
                            trigger: Trigger::default(),
                            summary: None,
                            outcome: None,
                            outcome_in_flight: false,
                            outcome_retry_at: None,
                            outcome_failures: 0,
                            sub_meta_at: now + SUB_META_EVERY,
                            grew_at: now,
                        },
                    );
                }
                let Some(e) = entries.get_mut(id) else {
                    continue;
                };
                if e.tail.poll(&mut e.transcript).is_ok_and(|grew| grew) {
                    e.grew_at = now;
                }
                if let Some(s) = &summarizer
                    && let Some(reply) = outcome_due(e, slot, SystemTime::now(), now)
                {
                    let input = OutcomeInput {
                        project: project_of(&slot.cwd),
                        task: e.transcript.latest_prompt().map(str::to_owned),
                        reply: reply.clone(),
                    };
                    let key = s.outcome_key(&input);
                    if let Some(line) = memo.get(&key) {
                        e.outcome = Some((reply, line.to_owned()));
                    } else {
                        e.outcome_in_flight = true;
                        let (s, tx, id) = (s.clone(), outcome_tx.clone(), *id);
                        tokio::spawn(async move {
                            let out = s.summarize_outcome(&input).await;
                            debug_log(id, "outcome", &out);
                            let _ = tx.send((id, reply, key, out.ok()));
                        });
                    }
                }
                // Claude Code may write a subagent's meta.json after its
                // transcript; one that has none yet is looked at again
                // now and then, not every tick.
                if e.sub.as_ref().is_none_or(|s| s.description.is_none()) && now >= e.sub_meta_at {
                    e.sub_meta_at = now + SUB_META_EVERY;
                    e.sub = extract::sub_meta(&e.tail.path).or(e.sub.take());
                }
                // A subagent's description is already a summary its parent wrote.
                if e.sub.as_ref().is_some_and(|s| s.description.is_some()) {
                    continue;
                }
                let Some(prompt) = e.transcript.latest_prompt() else {
                    continue;
                };
                let Some(s) = &summarizer else { continue };
                // Long idle, the office does not draw it, so no line is worth
                // asking for until it gets new work; unless it is still drawn
                // for the question it asked you. `r` still asks (`force`).
                let asks = e
                    .outcome
                    .as_ref()
                    .is_some_and(|(_, line)| crate::i18n::is_ask(line));
                if asks || !crate::core::idle_long(slot, SystemTime::now()) {
                    let basis = format!(
                        "{prompt}\u{1f}{}",
                        e.transcript.todo_active.as_deref().unwrap_or("")
                    );
                    e.trigger.observe(&basis, e.transcript.tool_calls, now);
                }
                if e.trigger.take_due(e.transcript.tool_calls, now) {
                    let input = summary_input(&e.transcript, &slot.cwd);
                    let key = s.summary_key(&input);
                    if let Some(line) = memo.get(&key) {
                        e.trigger.finished();
                        e.summary = Some(line.to_owned());
                        continue;
                    }
                    let (s, tx, id) = (s.clone(), done_tx.clone(), *id);
                    tokio::spawn(async move {
                        let out = s.summarize(&input).await;
                        debug_log(id, "task", &out);
                        let _ = tx.send((id, key, out.ok()));
                    });
                }
            }
        });
        let views: HashMap<AgentId, TaskView> = entries
            .iter()
            .map(|(id, e)| {
                let working = scene
                    .agents
                    .get(id)
                    .is_some_and(|s| matches!(s.state, ActivityState::Running { .. }));
                (*id, view(e, working, now))
            })
            .collect();
        views_tx.send_if_modified(|cur| {
            if **cur == views {
                return false;
            }
            *cur = Arc::new(views);
            true
        });
    }
}

/// `AGENT_OFFICE_DEBUG`: append each summary result to that file.
fn debug_log(id: AgentId, kind: &str, out: &anyhow::Result<String>) {
    let Ok(path) = std::env::var("AGENT_OFFICE_DEBUG") else {
        return;
    };
    let line = format!(
        "{id} {kind} {:?}\n",
        out.as_ref().map_err(|e| format!("{e:#}"))
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

fn project_of(cwd: &Path) -> String {
    cwd.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The reply to ask an outcome for now, if any: the agent has stopped (idle
/// a moment, so the turn's last lines are in) and its last reply has no
/// outcome yet, none is out, and no failed one is waiting to be retried.
fn outcome_due(
    e: &Entry,
    slot: &pixtuoid_core::AgentSlot,
    wall: SystemTime,
    now: Instant,
) -> Option<String> {
    let reply = e.transcript.last_reply.as_ref()?;
    let settled = wall
        .duration_since(slot.state_started_at)
        .is_ok_and(|d| d >= trigger::DEBOUNCE);
    let due = matches!(slot.state, ActivityState::Idle)
        && slot.exiting_at.is_none()
        && settled
        && !e.outcome_in_flight
        && e.outcome.as_ref().is_none_or(|(r, _)| r != reply)
        && e.outcome_retry_at.is_none_or(|t| now >= t);
    due.then(|| reply.clone())
}

fn summary_input(t: &Transcript, cwd: &Path) -> SummaryInput {
    let n = t.prompts.len();
    SummaryInput {
        project: project_of(cwd),
        latest_prompt: t.latest_prompt().unwrap_or_default().to_owned(),
        earlier_prompts: t
            .prompts
            .iter()
            .take(n.saturating_sub(1))
            .cloned()
            .collect(),
        todo: t.todo_active.clone(),
        recent_tools: t.recent_tools.iter().cloned().collect(),
    }
}

/// A subagent's type is what it was started as; a marker in the prompt a PM
/// wrote it is only text, so the type wins.
fn role_of(e: &Entry) -> Option<crate::roles::Role> {
    let typed = e.sub.as_ref().and_then(|s| s.agent_type.as_deref());
    typed
        .and_then(crate::roles::Role::of_agent_type)
        .or(e.transcript.role)
}

fn view(e: &Entry, working: bool, now: Instant) -> TaskView {
    let t = &e.transcript;
    // An outcome read from an earlier reply describes an earlier turn.
    let outcome = t.last_reply.as_ref().map(|r| match &e.outcome {
        Some((of, line)) if of == r => (line.clone(), true),
        _ => (truncate_cells(r, SUMMARY_CELLS), false),
    });
    let desc = e.sub.as_ref().and_then(|s| s.description.clone());
    let (line, summarized) = match (desc, &e.summary) {
        (Some(d), _) => (Some(d), true),
        (None, Some(s)) => (Some(s.clone()), true),
        (None, None) => (
            t.latest_prompt().map(|p| truncate_cells(p, SUMMARY_CELLS)),
            false,
        ),
    };
    TaskView {
        line,
        summarized,
        pending: e.trigger.in_flight(),
        prompt: t.latest_prompt().map(str::to_owned),
        todo: t.todo_active.clone(),
        recent_tools: t.recent_tools.iter().cloned().collect(),
        agent_type: e.sub.as_ref().and_then(|s| s.agent_type.clone()),
        role: role_of(e),
        todos: t.todos.clone(),
        phase: t.phase,
        stuck: stuck(t, working, now.duration_since(e.grew_at), stuck_limits()),
        turn_started: t.turn_started,
        outcome: outcome.as_ref().map(|(o, _)| o.clone()),
        outcome_summarized: outcome.is_some_and(|(_, s)| s),
        reply: t.last_reply.clone(),
        cheer: t.cheer,
        written_at: e.tail.modified,
    }
}

/// How long ago `t` was, as the panel shows it.
pub fn ago(t: SystemTime) -> String {
    let s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcripts_are_keyed_the_way_pixtuoid_keys_agents() {
        let root = std::env::temp_dir().join(format!("agent-office-scan-{}", std::process::id()));
        let sess = root.join("-repo");
        std::fs::create_dir_all(sess.join("abc/subagents")).unwrap();
        std::fs::write(sess.join("abc.jsonl"), "").unwrap();
        std::fs::write(sess.join("abc/subagents/agent-a1.jsonl"), "").unwrap();
        std::fs::write(
            sess.join("abc/subagents/agent-a1.meta.json"),
            r#"{"description":"调研 X 仓库","agentType":"Explore"}"#,
        )
        .unwrap();
        let found = scan(&root);
        let sub = AgentId::from_parts(CC_SOURCE, "agent-a1");
        assert!(found.contains_key(&AgentId::from_parts(CC_SOURCE, "abc")));
        assert!(found.contains_key(&sub));
        let meta = extract::sub_meta(&found[&sub]).unwrap();
        assert_eq!(meta.description.as_deref(), Some("调研 X 仓库"));
        std::fs::remove_dir_all(root).ok();
    }

    /// pixtuoid keys a Codex agent by the UUID its rollout's name ends in,
    /// so the reader must key it the same, or a Codex figure never gets its
    /// task line; anything else under `sessions/` is not a rollout.
    #[test]
    fn codex_rollouts_are_keyed_by_their_session_uuid() {
        let root =
            std::env::temp_dir().join(format!("agent-office-codex-scan-{}", std::process::id()));
        let day = root.join("2026/11/05");
        std::fs::create_dir_all(&day).unwrap();
        let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
        let rollout = day.join(format!("rollout-2026-11-05T09-30-15-{uuid}.jsonl"));
        std::fs::write(&rollout, "").unwrap();
        std::fs::write(day.join("history.jsonl"), "").unwrap();
        std::fs::write(day.join("rollout-2026-11-05T09-30-15-notauuid.jsonl"), "").unwrap();
        let found = scan_codex(&root);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[&AgentId::from_parts(CODEX_SOURCE, uuid)], rollout);
        std::fs::remove_dir_all(root).ok();
    }

    /// The plugin and the reader must agree on the file name, or a dsh
    /// session never gets a task line.
    #[test]
    fn a_dsh_log_is_named_the_way_the_plugin_names_it() {
        let dir = Path::new("/t");
        assert_eq!(
            task_log(dir, "session-209513f0-7740"),
            Path::new("/t/session-209513f0-7740.jsonl")
        );
        // The plugin's `replace(/[^A-Za-z0-9._-]/gu, "_")`: per code point,
        // as `chars()` is, so a non-BMP character is one `_` on both sides.
        assert_eq!(task_log(dir, "../a/b c🙂"), Path::new("/t/.._a_b_c_.jsonl"));
        let plugin = include_str!("../dsh_plugin.mjs");
        assert!(plugin.contains(r#"sessionId.replace(/[^A-Za-z0-9._-]/gu, "_") + ".jsonl""#));
    }

    #[test]
    fn a_quiet_agent_is_stuck_only_while_it_should_be_working() {
        let t = Transcript::default();
        let long = Duration::from_secs(6 * 60);
        assert_eq!(stuck(&t, true, long, &D), Some(Stuck::Quiet(6)));
        // Idle or waiting on the user: silence is expected.
        assert_eq!(stuck(&t, false, long, &D), None);
        assert_eq!(stuck(&t, true, Duration::from_secs(60), &D), None);
    }

    /// A build or a subagent can keep the transcript quiet for a long time
    /// without anything being wrong.
    #[test]
    fn a_long_call_out_gets_longer_before_it_counts_as_stuck() {
        let t = Transcript {
            outstanding: vec![("b".into(), "Bash".into())],
            ..Transcript::default()
        };
        assert_eq!(stuck(&t, true, Duration::from_secs(6 * 60), &D), None);
        assert_eq!(
            stuck(&t, true, Duration::from_secs(21 * 60), &D),
            Some(Stuck::Quiet(21))
        );
    }

    const D: StuckLimits = StuckLimits::DEFAULT;

    /// A repository whose test suite runs half an hour must be able to say
    /// so, or every run of it is flagged stuck.
    #[test]
    fn raised_limits_hold_off_the_stuck_flag() {
        let vars = |k: &str| match k {
            "AGENT_OFFICE_STUCK_QUIET" => Some("10".to_owned()),
            "AGENT_OFFICE_STUCK_QUIET_LONG" => Some(" 45 ".to_owned()),
            "AGENT_OFFICE_STUCK_FAILS" => Some("5".to_owned()),
            _ => None,
        };
        let (limits, bad) = StuckLimits::from_vars(vars);
        assert!(bad.is_empty());
        let quiet = Transcript::default();
        assert_eq!(
            stuck(&quiet, true, Duration::from_secs(6 * 60), &limits),
            None
        );
        let building = Transcript {
            outstanding: vec![("b".into(), "Bash".into())],
            ..Transcript::default()
        };
        assert_eq!(
            stuck(&building, true, Duration::from_secs(30 * 60), &limits),
            None
        );
        assert_eq!(
            stuck(&building, true, Duration::from_secs(46 * 60), &limits),
            Some(Stuck::Quiet(46))
        );
        let failing = Transcript {
            fail_streak: 4,
            ..Transcript::default()
        };
        assert_eq!(stuck(&failing, true, Duration::ZERO, &limits), None);
    }

    /// A typo must not silently turn the flag off (0) or leave the user
    /// guessing: the default holds and the name is reported.
    #[test]
    fn a_limit_that_is_not_a_positive_number_keeps_the_default_and_is_reported() {
        let vars = |k: &str| match k {
            "AGENT_OFFICE_STUCK_QUIET" => Some("0".to_owned()),
            "AGENT_OFFICE_STUCK_FAILS" => Some("three".to_owned()),
            _ => None,
        };
        let (limits, bad) = StuckLimits::from_vars(vars);
        assert_eq!(limits, D);
        assert_eq!(
            bad,
            ["AGENT_OFFICE_STUCK_FAILS", "AGENT_OFFICE_STUCK_QUIET"]
        );
    }

    #[test]
    fn errors_in_a_row_are_stuck_even_when_the_transcript_moves() {
        let t = Transcript {
            fail_streak: 3,
            ..Transcript::default()
        };
        assert_eq!(stuck(&t, true, Duration::ZERO, &D), Some(Stuck::Failing(3)));
    }

    fn entry() -> Entry {
        Entry {
            tail: Tail::new(PathBuf::new()),
            transcript: Transcript::default(),
            sub: None,
            trigger: Trigger::default(),
            summary: None,
            outcome: None,
            outcome_in_flight: false,
            outcome_retry_at: None,
            outcome_failures: 0,
            sub_meta_at: Instant::now(),
            grew_at: Instant::now(),
        }
    }

    /// A PM's prompt to a subagent may well quote a marker; the subagent is
    /// what it was started as. And Claude Code's own Explore is no post.
    #[test]
    fn an_office_type_outranks_a_marker_and_a_builtin_is_no_role() {
        use crate::roles::Role;
        let mut e = entry();
        e.transcript.role = Some(Role::Pm);
        let sub = |t: &str| {
            Some(extract::SubMeta {
                agent_type: Some(t.into()),
                ..Default::default()
            })
        };
        e.sub = sub("office-qa");
        assert_eq!(view(&e, true, Instant::now()).role, Some(Role::Qa));
        e.sub = sub("Explore");
        e.transcript.role = None;
        let v = view(&e, true, Instant::now());
        assert_eq!((v.role, v.agent_type.as_deref()), (None, Some("Explore")));
        e.sub = None;
        e.transcript.role = Some(Role::Ux);
        assert_eq!(view(&e, true, Instant::now()).role, Some(Role::Ux));
    }

    /// Old task logs go, recent ones and anything that is not a task log
    /// stay: the directory is only ever appended to.
    #[test]
    fn only_task_logs_a_month_quiet_are_removed() {
        let dir = std::env::temp_dir().join(format!("agent-office-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("s.jsonl");
        let other = dir.join("notes.txt");
        std::fs::write(&log, "{}\n").unwrap();
        std::fs::write(&other, "x").unwrap();
        prune_task_logs(&dir, SystemTime::now());
        assert!(log.exists(), "a fresh log stays");
        prune_task_logs(&dir, SystemTime::now() + TASK_LOG_KEEP);
        assert!(!log.exists(), "a month quiet goes");
        assert!(other.exists(), "not a task log");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    static SCANS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn counting_scan(_: &Path) -> HashMap<AgentId, PathBuf> {
        SCANS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        HashMap::new()
    }

    /// Dozens of sessions showing up at once (the office opening) cost one
    /// directory scan, not one each: the projects tree holds thousands of
    /// transcripts. One not found is still looked for again shortly.
    #[test]
    fn new_sessions_in_one_tick_share_a_scan() {
        let mut l = Locator::new(counting_scan);
        let now = Instant::now();
        let root = Path::new("/nowhere");
        for i in 0..20 {
            let id = AgentId::from_parts(CC_SOURCE, &format!("s{i}"));
            assert_eq!(l.find(id, root, now), None);
        }
        assert_eq!(SCANS.load(std::sync::atomic::Ordering::SeqCst), 1);
        let late = AgentId::from_parts(CC_SOURCE, "s19");
        l.find(late, root, now + SCAN_GAP);
        assert_eq!(
            SCANS.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one the first scan never looked for gets its own a moment later"
        );
    }

    fn idle_slot(since: SystemTime) -> pixtuoid_core::AgentSlot {
        let mut scene = SceneState::uniform(8);
        let id = AgentId::from_parts(CC_SOURCE, "s");
        pixtuoid_core::Reducer::new().apply(
            &mut scene,
            pixtuoid_core::AgentEvent::SessionStart {
                agent_id: id,
                source: CC_SOURCE.into(),
                session_id: "s".into(),
                cwd: "/repo".into(),
                parent_id: None,
            },
            since,
            pixtuoid_core::Transport::Hook,
        );
        let mut slot = scene.agents.remove(&id).unwrap();
        slot.state = ActivityState::Idle;
        slot.state_started_at = since;
        slot
    }

    /// One request per finished turn: asked once the agent has stopped and
    /// its last lines are in, never while it works or for a reply already
    /// read, and a failed one waits before it is tried again.
    #[test]
    fn an_outcome_is_asked_once_per_reply_after_the_agent_stops() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = Instant::now();
        let mut e = entry();
        let mut slot = idle_slot(t0);
        let settled = t0 + trigger::DEBOUNCE;
        assert_eq!(outcome_due(&e, &slot, settled, now), None, "no reply yet");
        e.transcript.last_reply = Some("已修好".into());
        assert_eq!(
            outcome_due(&e, &slot, t0 + Duration::from_secs(1), now),
            None,
            "its last lines may still be landing"
        );
        assert_eq!(
            outcome_due(&e, &slot, settled, now).as_deref(),
            Some("已修好")
        );
        slot.state = ActivityState::Running { item: None };
        assert_eq!(outcome_due(&e, &slot, settled, now), None, "still working");
        slot.state = ActivityState::Idle;
        e.outcome_in_flight = true;
        assert_eq!(
            outcome_due(&e, &slot, settled, now),
            None,
            "one out already"
        );
        e.outcome_in_flight = false;
        e.outcome = Some(("已修好".into(), "登录页已修好".into()));
        assert_eq!(outcome_due(&e, &slot, settled, now), None, "already read");
        e.transcript.last_reply = Some("要删旧接口吗？".into());
        assert!(
            outcome_due(&e, &slot, settled, now).is_some(),
            "a new turn's reply"
        );
        e.outcome_retry_at = Some(now + Duration::from_secs(30));
        assert_eq!(
            outcome_due(&e, &slot, settled, now),
            None,
            "waits after a failure"
        );
        assert!(outcome_due(&e, &slot, settled, now + Duration::from_secs(30)).is_some());
    }

    /// An outcome read from an earlier reply describes an earlier turn: the
    /// card falls back to the new reply rather than show a stale result.
    #[test]
    fn an_outcome_shows_only_for_the_reply_it_was_read_from() {
        let mut e = entry();
        e.transcript.last_reply = Some("已修好，补了测试".into());
        e.outcome = Some(("已修好，补了测试".into(), "登录页已修好".into()));
        let v = view(&e, false, Instant::now());
        assert_eq!(v.outcome.as_deref(), Some("登录页已修好"));
        assert!(v.outcome_summarized);
        e.transcript.last_reply = Some("要删旧接口吗？".into());
        let v = view(&e, false, Instant::now());
        assert_eq!(v.outcome.as_deref(), Some("要删旧接口吗？"));
        assert!(!v.outcome_summarized);
        e.transcript.last_reply = None;
        assert_eq!(view(&e, false, Instant::now()).outcome, None);
    }

    #[test]
    fn without_a_summary_the_line_is_the_raw_prompt_marked_unsummarized() {
        let mut e = entry();
        e.transcript
            .prompts
            .push_back("请帮我把登录页的报错修好然后补一个回归测试再部署".into());
        let v = view(&e, false, Instant::now());
        assert!(!v.summarized);
        assert!(crate::text::cells(v.line.as_deref().unwrap()) <= SUMMARY_CELLS);
        e.summary = Some("修复登录页报错".into());
        assert_eq!(
            view(&e, false, Instant::now()).line.as_deref(),
            Some("修复登录页报错")
        );
        assert!(view(&e, false, Instant::now()).summarized);
    }
}
