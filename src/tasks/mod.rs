//! Each agent's task, as one line: a background loop tails the transcripts of
//! the agents the office shows and keeps their summaries fresh.

pub mod client;
pub mod extract;
pub mod trigger;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};
use tokio::sync::{mpsc, watch};

use crate::pipeline::CC_SOURCE;
use crate::text::truncate_cells;
use client::{SUMMARY_CELLS, Summarizer, SummaryInput};
pub use extract::Phase;
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
    pub phase: Option<Phase>,
    pub stuck: Option<Stuck>,
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
            Stuck::Failing(n) => format!("连续失败 {n} 次"),
            Stuck::Quiet(m) => format!("{m} 分钟没动静"),
        }
    }
}

const FAILS_STUCK: u32 = 3;
/// A working agent whose transcript is this quiet is likely stuck.
const QUIET_STUCK: Duration = Duration::from_secs(5 * 60);
/// The same while a call that may run long is out: a build, a subagent.
const QUIET_STUCK_LONG_CALL: Duration = Duration::from_secs(20 * 60);
const LONG_CALLS: &[&str] = &["Bash", "Task", "Agent"];

fn stuck(t: &Transcript, working: bool, quiet: Duration) -> Option<Stuck> {
    if t.fail_streak >= FAILS_STUCK {
        return Some(Stuck::Failing(t.fail_streak));
    }
    let limit = if t.awaits_any(LONG_CALLS) {
        QUIET_STUCK_LONG_CALL
    } else {
        QUIET_STUCK
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
    /// When the transcript last gained a line.
    grew_at: Instant,
}

#[derive(Debug, Default)]
struct Locator {
    paths: HashMap<AgentId, PathBuf>,
    /// Ids the last scan did not find, so an agent with no transcript costs
    /// one scan per [`RESCAN_MISSED`], not one per tick.
    missed: HashSet<AgentId>,
    last_scan: Option<Instant>,
}

const RESCAN_MISSED: Duration = Duration::from_secs(30);

impl Locator {
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
        self.last_scan = Some(now);
        self.paths = scan(root);
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

/// `$CLAUDE_CONFIG_DIR`, else `~/.claude`, as Claude Code resolves it.
pub fn claude_dir() -> Option<PathBuf> {
    pixtuoid_core::source::claude_code::claude_config_dir()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

pub fn projects_root() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("projects"))
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
    let Some(root) = projects_root() else { return };
    let mut entries: HashMap<AgentId, Entry> = HashMap::new();
    let mut locator = Locator::default();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<(AgentId, Option<String>)>();
    let mut tick = tokio::time::interval(Duration::from_millis(700));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            Some((id, out)) = done_rx.recv() => {
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
            for (id, slot) in &scene.agents {
                if &*slot.source != CC_SOURCE {
                    continue;
                }
                if !entries.contains_key(id) {
                    let Some(path) = locator.find(*id, &root, now) else {
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
                // Claude Code may write a subagent's meta.json after its transcript.
                if e.sub.as_ref().is_none_or(|s| s.description.is_none()) {
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
                let basis = format!(
                    "{prompt}\u{1f}{}",
                    e.transcript.todo_active.as_deref().unwrap_or("")
                );
                e.trigger.observe(&basis, e.transcript.tool_calls, now);
                if e.trigger.take_due(e.transcript.tool_calls, now) {
                    let input = summary_input(&e.transcript, &slot.cwd);
                    let (s, tx, id) = (s.clone(), done_tx.clone(), *id);
                    tokio::spawn(async move {
                        let out = s.summarize(&input).await;
                        if let Ok(path) = std::env::var("AGENT_OFFICE_DEBUG") {
                            let line =
                                format!("{id} {:?}\n", out.as_ref().map_err(|e| format!("{e:#}")));
                            let _ = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(path)
                                .and_then(|mut f| {
                                    std::io::Write::write_all(&mut f, line.as_bytes())
                                });
                        }
                        let _ = tx.send((id, out.ok()));
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

fn summary_input(t: &Transcript, cwd: &Path) -> SummaryInput {
    let n = t.prompts.len();
    SummaryInput {
        project: cwd
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
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

fn view(e: &Entry, working: bool, now: Instant) -> TaskView {
    let t = &e.transcript;
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
        phase: t.phase,
        stuck: stuck(t, working, now.duration_since(e.grew_at)),
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

    #[test]
    fn a_quiet_agent_is_stuck_only_while_it_should_be_working() {
        let t = Transcript::default();
        let long = Duration::from_secs(6 * 60);
        assert_eq!(stuck(&t, true, long), Some(Stuck::Quiet(6)));
        // Idle or waiting on the user: silence is expected.
        assert_eq!(stuck(&t, false, long), None);
        assert_eq!(stuck(&t, true, Duration::from_secs(60)), None);
    }

    /// A build or a subagent can keep the transcript quiet for a long time
    /// without anything being wrong.
    #[test]
    fn a_long_call_out_gets_longer_before_it_counts_as_stuck() {
        let t = Transcript {
            outstanding: vec![("b".into(), "Bash".into())],
            ..Transcript::default()
        };
        assert_eq!(stuck(&t, true, Duration::from_secs(6 * 60)), None);
        assert_eq!(
            stuck(&t, true, Duration::from_secs(21 * 60)),
            Some(Stuck::Quiet(21))
        );
    }

    #[test]
    fn errors_in_a_row_are_stuck_even_when_the_transcript_moves() {
        let t = Transcript {
            fail_streak: 3,
            ..Transcript::default()
        };
        assert_eq!(stuck(&t, true, Duration::ZERO), Some(Stuck::Failing(3)));
    }

    #[test]
    fn without_a_summary_the_line_is_the_raw_prompt_marked_unsummarized() {
        let mut e = Entry {
            tail: Tail::new(PathBuf::new()),
            transcript: Transcript::default(),
            sub: None,
            trigger: Trigger::default(),
            summary: None,
            grew_at: Instant::now(),
        };
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
