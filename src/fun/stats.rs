//! The day's tally and each repository's record, counted off the scene frame
//! by frame: turns finished, times stuck, tokens spent, and the approvals
//! that bring the boss to the door. The record is kept on disk so a bench's
//! stars outlive a restart.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::state::{ActivityState, project_root};
use pixtuoid_core::{AgentId, AgentSlot, SceneState};
use serde::{Deserialize, Serialize};

use super::Clock;
use crate::tasks::{TaskViews, stuck_at_work};

/// What has been done, today and ever.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// The office's date these counts are for, `YYYY-MM-DD`.
    pub day: String,
    /// Turns finished today.
    pub turns: u32,
    /// Times an agent got stuck today.
    pub stuck: u32,
    /// Tokens spent today.
    pub tokens: u64,
    /// Turns ever finished at each repository, by its directory's name.
    pub repos: BTreeMap<String, u32>,
}

/// A repository's standing, by turns finished there: each tier dresses its
/// bench up a step.
pub fn tier(turns: u32) -> u8 {
    match turns {
        200.. => 3,
        50.. => 2,
        10.. => 1,
        _ => 0,
    }
}

/// The name a repository's bench plate carries: its root directory's.
pub fn repo_of(slot: &AgentSlot) -> Option<String> {
    project_root(&slot.cwd)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
}

/// What an agent was doing last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    state: Doing,
    stuck: bool,
    tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Doing {
    Working,
    Waiting,
    Idle,
}

impl Doing {
    fn of(s: &ActivityState) -> Self {
        match s {
            ActivityState::Running { .. } => Doing::Working,
            ActivityState::RequiresAction { .. } => Doing::Waiting,
            ActivityState::Idle => Doing::Idle,
        }
    }
}

/// How long the boss stays at the door after you approve something.
pub const BOSS_FOR: Duration = Duration::from_secs(4);
/// The record is written at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(10);

/// Counts the scene's turns as they happen.
#[derive(Debug, Default)]
pub struct Watch {
    seen: HashMap<AgentId, Seen>,
    pub stats: Stats,
    /// When you last let a waiting agent go on.
    pub approved_at: Option<SystemTime>,
    path: Option<PathBuf>,
    unsaved: bool,
    saved_at: Option<Instant>,
}

impl Watch {
    /// A watch that keeps its record at `~/.config/agent-office/stats.json`.
    pub fn load() -> Self {
        let path = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config/agent-office/stats.json"));
        let stats = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            stats,
            path,
            ..Self::default()
        }
    }

    /// Fold in the scene as it stands at `now`. An agent seen for the first
    /// time is only noted: what it did before the office saw it is not today's.
    pub fn observe(&mut self, scene: &SceneState, views: &TaskViews, now: SystemTime) {
        let day = Clock::now(now).date();
        self.observe_on(&day, scene, views, now);
        if self.unsaved && self.saved_at.is_none_or(|t| t.elapsed() >= SAVE_EVERY) {
            self.save();
        }
    }

    fn observe_on(&mut self, day: &str, scene: &SceneState, views: &TaskViews, now: SystemTime) {
        if self.stats.day != day {
            self.stats = Stats {
                day: day.to_owned(),
                repos: std::mem::take(&mut self.stats.repos),
                ..Stats::default()
            };
            self.unsaved = true;
        }
        for (id, slot) in &scene.agents {
            let now_seen = Seen {
                state: Doing::of(&slot.state),
                stuck: stuck_at_work(slot, views.get(id)).is_some(),
                tokens: slot.tokens_used,
            };
            if let Some(was) = self.seen.insert(*id, now_seen) {
                self.count(slot, was, now_seen, now);
            }
        }
        self.seen.retain(|id, _| scene.agents.contains_key(id));
    }

    fn count(&mut self, slot: &AgentSlot, was: Seen, is: Seen, now: SystemTime) {
        let before = self.stats.clone();
        // A subagent's work is its parent's turn, counted once there.
        if was.state != Doing::Idle && is.state == Doing::Idle && slot.parent_id.is_none() {
            self.stats.turns += 1;
            if let Some(repo) = repo_of(slot) {
                *self.stats.repos.entry(repo).or_default() += 1;
            }
        }
        if was.state == Doing::Waiting && is.state == Doing::Working {
            self.approved_at = Some(now);
        }
        if !was.stuck && is.stuck {
            self.stats.stuck += 1;
        }
        self.stats.tokens += is.tokens.saturating_sub(was.tokens);
        self.unsaved |= self.stats != before;
    }

    fn save(&mut self) {
        self.saved_at = Some(Instant::now());
        self.unsaved = false;
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.stats) {
            // Through a temporary file, so a crash mid-write keeps the old record.
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    /// Whether the boss is at the door at `now`.
    pub fn boss_at_door(&self, now: SystemTime) -> Option<Duration> {
        let ago = now.duration_since(self.approved_at?).ok()?;
        (ago < BOSS_FOR).then_some(ago)
    }
}

/// Tokens as the wall board shows them: `950`, `12k`, `3.4M`.
pub fn tokens_short(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use std::sync::Arc;

    fn scene_with(ids: &[(&str, &str)]) -> (SceneState, Vec<AgentId>) {
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let ids = ids
            .iter()
            .map(|(sid, cwd)| {
                let id = AgentId::from_parts("claude-code", sid);
                r.apply(
                    &mut scene,
                    AgentEvent::SessionStart {
                        agent_id: id,
                        source: "claude-code".into(),
                        session_id: (*sid).into(),
                        cwd: (*cwd).into(),
                        parent_id: None,
                    },
                    t0,
                    Transport::Hook,
                );
                id
            })
            .collect();
        (scene, ids)
    }

    fn watch() -> Watch {
        Watch {
            stats: Stats {
                day: "2026-11-05".into(),
                ..Stats::default()
            },
            ..Watch::default()
        }
    }

    /// A turn counts when a working agent goes idle, to the day and to its
    /// repository's record; the agent already idle when first seen counts
    /// nothing, or every restart would pad the tally.
    #[test]
    fn a_finished_turn_counts_for_the_day_and_the_repository() {
        let (mut scene, ids) = scene_with(&[("a", "/src/shop"), ("b", "/src/blog")]);
        let views = TaskViews::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let mut w = watch();
        for id in &ids {
            scene.agents.get_mut(id).unwrap().state = ActivityState::Idle;
        }
        scene.agents.get_mut(&ids[0]).unwrap().state = ActivityState::Running { item: None };
        w.observe_on("2026-11-05", &scene, &views, now);
        assert_eq!(w.stats.turns, 0);
        scene.agents.get_mut(&ids[0]).unwrap().state = ActivityState::Idle;
        w.observe_on("2026-11-05", &scene, &views, now);
        assert_eq!(w.stats.turns, 1);
        assert_eq!(w.stats.repos.get("shop"), Some(&1));
        assert_eq!(w.stats.repos.get("blog"), None, "idle all along");

        // A new day starts the tally over but keeps the record.
        w.observe_on("2026-11-06", &scene, &views, now);
        assert_eq!(w.stats.turns, 0);
        assert_eq!(w.stats.repos.get("shop"), Some(&1));
    }

    /// Letting a waiting agent go on brings the boss to the door for a
    /// moment; an agent that just starts working does not.
    #[test]
    fn an_approval_brings_the_boss_to_the_door_briefly() {
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        let views = TaskViews::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let mut w = watch();
        let set = |scene: &mut SceneState, s| scene.agents.get_mut(&ids[0]).unwrap().state = s;
        set(&mut scene, ActivityState::Idle);
        w.observe_on("d", &scene, &views, now);
        set(&mut scene, ActivityState::Running { item: None });
        w.observe_on("d", &scene, &views, now);
        assert_eq!(w.boss_at_door(now), None, "starting work is no approval");
        set(&mut scene, ActivityState::RequiresAction { reason: None });
        w.observe_on("d", &scene, &views, now);
        set(&mut scene, ActivityState::Running { item: None });
        w.observe_on("d", &scene, &views, now);
        assert_eq!(w.boss_at_door(now), Some(Duration::ZERO));
        assert_eq!(w.boss_at_door(now + BOSS_FOR), None, "and goes back in");
    }

    /// Tokens count what was spent while the office watched, not a session's
    /// whole past.
    #[test]
    fn tokens_count_only_what_is_spent_while_watched() {
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        let views = TaskViews::default();
        let now = SystemTime::UNIX_EPOCH;
        let mut w = watch();
        scene.agents.get_mut(&ids[0]).unwrap().tokens_used = 50_000;
        w.observe_on("d", &scene, &views, now);
        scene.agents.get_mut(&ids[0]).unwrap().tokens_used = 62_000;
        w.observe_on("d", &scene, &views, now);
        assert_eq!(w.stats.tokens, 12_000);
        assert_eq!(tokens_short(w.stats.tokens), "12k");
        assert_eq!(tokens_short(3_450_000), "3.5M");
    }

    /// A stuck spell counts once however long it lasts.
    #[test]
    fn a_stuck_spell_counts_once() {
        use crate::tasks::{Stuck, TaskView};
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        scene.agents.get_mut(&ids[0]).unwrap().state = ActivityState::Running { item: None };
        let now = SystemTime::UNIX_EPOCH;
        let mut w = watch();
        let ok = TaskViews::default();
        let stuck: TaskViews = Arc::new(HashMap::from([(
            ids[0],
            TaskView {
                stuck: Some(Stuck::Failing(3)),
                ..TaskView::default()
            },
        )]));
        w.observe_on("d", &scene, &ok, now);
        for _ in 0..3 {
            w.observe_on("d", &scene, &stuck, now);
        }
        assert_eq!(w.stats.stuck, 1);
    }

    /// Each tier asks for real work: ten turns for the first star.
    #[test]
    fn a_bench_climbs_a_tier_at_ten_fifty_and_two_hundred_turns() {
        assert_eq!(
            [0, 9, 10, 49, 50, 199, 200].map(tier),
            [0, 0, 1, 1, 2, 2, 3]
        );
    }
}
