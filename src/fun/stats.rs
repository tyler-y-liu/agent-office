//! The day's tally and each repository's record, counted off the scene frame
//! by frame: turns finished, times stuck, tokens spent, and the approvals
//! that bring the boss to the door. The record is kept on disk so a bench's
//! stars outlive a restart.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
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
    /// Turns finished today in each hour of the office's clock: the replay's
    /// day as it went.
    #[serde(default)]
    pub hours: [u32; 24],
    /// Turns finished today by each worker, by the name its badge shows.
    #[serde(default)]
    pub by_worker: BTreeMap<String, u32>,
    /// Each worker's longest stuck spell today, in seconds.
    #[serde(default)]
    pub stuck_longest: BTreeMap<String, u64>,
    /// How often each feature was used, ever, by a dotted name
    /// (`picker.open`, `dispatch.relay.qa`): kept here, on this machine
    /// only, to tell what is worth keeping.
    #[serde(default)]
    pub uses: BTreeMap<String, u32>,
}

/// The day's honours the replay hands out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recap {
    /// Who finished the most turns, and how many.
    pub most: Option<(String, u32)>,
    /// Who was stuck longest at a stretch, and for how many seconds.
    pub stuck: Option<(String, u64)>,
}

impl Stats {
    /// Today's honours; a tie goes to the name first in order, so the trophy
    /// does not flicker between them.
    pub fn recap(&self) -> Recap {
        fn top<T: Copy + Ord>(m: &BTreeMap<String, T>) -> Option<(String, T)> {
            m.iter()
                .max_by_key(|(name, n)| (**n, std::cmp::Reverse(*name)))
                .map(|(name, n)| (name.clone(), *n))
        }
        Recap {
            most: top(&self.by_worker),
            stuck: top(&self.stuck_longest).filter(|(_, s)| *s > 0),
        }
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
/// The record is written at most this often; [`Watch::flush`] writes the
/// rest on the way out.
const SAVE_EVERY: Duration = Duration::from_secs(10);

/// Counts the scene's turns as they happen.
#[derive(Debug, Default)]
pub struct Watch {
    seen: HashMap<AgentId, Seen>,
    /// When each agent stuck now got stuck.
    stuck_since: HashMap<AgentId, SystemTime>,
    pub stats: Stats,
    /// Older dirty records awaiting their write, oldest first.
    pending: VecDeque<Stats>,
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
        if (self.unsaved || !self.pending.is_empty())
            && self.saved_at.is_none_or(|t| t.elapsed() >= SAVE_EVERY)
        {
            self.save();
        }
    }

    fn observe_on(&mut self, day: &str, scene: &SceneState, views: &TaskViews, now: SystemTime) {
        let new_day = self.stats.day != day;
        if new_day {
            if self.unsaved {
                self.pending.push_back(self.stats.clone());
            }
            self.stats = Stats {
                day: day.to_owned(),
                repos: std::mem::take(&mut self.stats.repos),
                uses: std::mem::take(&mut self.stats.uses),
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
            if let Some(mut was) = self.seen.insert(*id, now_seen) {
                if new_day && was.stuck && now_seen.stuck {
                    // Today's segment starts at its first observation: the
                    // office may have slept, so yesterday's start is unknown.
                    self.stuck_since.remove(id);
                    was.stuck = false;
                }
                self.count(slot, was, now_seen, now);
            }
        }
        self.seen.retain(|id, _| scene.agents.contains_key(id));
        self.stuck_since
            .retain(|id, _| self.seen.get(id).is_some_and(|s| s.stuck));
    }

    // Marks `unsaved` at each change rather than comparing a copy of the
    // stats, since this runs for every agent on every frame.
    fn count(&mut self, slot: &AgentSlot, was: Seen, is: Seen, now: SystemTime) {
        // A subagent's work is its parent's turn, counted once there.
        if was.state != Doing::Idle && is.state == Doing::Idle && slot.parent_id.is_none() {
            self.unsaved = true;
            self.stats.turns += 1;
            if let Some(repo) = repo_of(slot) {
                *self.stats.repos.entry(repo).or_default() += 1;
            }
            let hour = Clock::now(now).hour as usize;
            self.stats.hours[hour.min(23)] += 1;
            *self
                .stats
                .by_worker
                .entry(String::from(&*slot.label))
                .or_default() += 1;
        }
        if was.state == Doing::Waiting && is.state == Doing::Working {
            self.approved_at = Some(now);
        }
        if !was.stuck && is.stuck {
            self.unsaved = true;
            self.stats.stuck += 1;
        }
        if is.stuck {
            let since = *self.stuck_since.entry(slot.agent_id).or_insert(now);
            let secs = now.duration_since(since).unwrap_or_default().as_secs();
            let longest = self
                .stats
                .stuck_longest
                .entry(String::from(&*slot.label))
                .or_default();
            if secs > *longest {
                *longest = secs;
                self.unsaved = true;
            }
        }
        let spent = is.tokens.saturating_sub(was.tokens);
        if spent > 0 {
            self.stats.tokens += spent;
            self.unsaved = true;
        }
    }

    /// Count one use of `what` ([`Stats::uses`]).
    pub fn tally(&mut self, what: &str) {
        *self.stats.uses.entry(what.to_owned()).or_default() += 1;
        self.unsaved = true;
    }

    fn save(&mut self) {
        self.save_using(crate::atomic::write_private);
    }

    fn save_using(&mut self, mut write: impl FnMut(&Path, &Path, &[u8]) -> std::io::Result<()>) {
        self.saved_at = Some(Instant::now());
        let Some(path) = self.path.clone() else {
            self.pending.clear();
            self.unsaved = false;
            return;
        };
        while self.unsaved || !self.pending.is_empty() {
            let is_pending = !self.pending.is_empty();
            let stats = if is_pending {
                self.pending.front().expect("pending was not empty")
            } else {
                &self.stats
            };
            let saved = (|| -> std::io::Result<()> {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let json = serde_json::to_string_pretty(stats).map_err(std::io::Error::other)?;
                // Through a temporary file, so a crash mid-write keeps the old record.
                let tmp = path.with_extension("json.tmp");
                write(&path, &tmp, json.as_bytes())
            })();
            match saved {
                Ok(()) if is_pending => {
                    self.pending.pop_front();
                }
                Ok(()) => self.unsaved = false,
                // Leave this and every newer record for the next attempt.
                Err(e) => {
                    eprintln!("agent-office: saving stats failed: {e}");
                    return;
                }
            }
        }
    }

    /// Write what changed since the last write, on the way out: counts
    /// since then would otherwise be lost on quit.
    pub fn flush(&mut self) {
        self.flush_using(crate::atomic::write_private);
    }

    fn flush_using(&mut self, write: impl FnMut(&Path, &Path, &[u8]) -> std::io::Result<()>) {
        if self.unsaved || !self.pending.is_empty() {
            self.save_using(write);
        }
    }

    /// Whether a change is waiting for the next write.
    #[cfg(test)]
    fn unsaved(&self) -> bool {
        self.unsaved || !self.pending.is_empty()
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

    fn finish_turn(
        w: &mut Watch,
        scene: &mut SceneState,
        id: AgentId,
        day: &str,
        views: &TaskViews,
        now: SystemTime,
    ) {
        let slot = scene.agents.get_mut(&id).unwrap();
        slot.state = ActivityState::Running { item: None };
        w.observe_on(day, scene, views, now);
        scene.agents.get_mut(&id).unwrap().tokens_used += 10;
        w.observe_on(day, scene, views, now);
        scene.agents.get_mut(&id).unwrap().state = ActivityState::Idle;
        w.observe_on(day, scene, views, now);
    }

    /// A turn counts when a working agent goes idle, to the day and to its
    /// repository's record; the agent already idle when first seen counts
    /// nothing, or every restart would pad the tally.
    /// Feature counts are what tell a feature worth keeping from one
    /// nobody uses: they build up across days, not reset at midnight.
    #[test]
    fn uses_add_up_across_days() {
        let (scene, _) = scene_with(&[]);
        let views = TaskViews::default();
        let now = SystemTime::now();
        let mut w = watch();
        w.observe_on("2026-11-05", &scene, &views, now);
        w.tally("picker.open.relay");
        w.tally("picker.open.relay");
        assert!(w.unsaved());
        w.observe_on("2026-11-06", &scene, &views, now);
        assert_eq!(w.stats.uses.get("picker.open.relay"), Some(&2));
        assert_eq!(w.stats.turns, 0, "the day's tally still starts over");
    }

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

    /// The record is written only when a count moved: frames where nothing
    /// changed must not keep the disk busy, and a finished turn must not
    /// be left unwritten.
    #[test]
    fn only_a_change_marks_the_record_for_writing() {
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        let views = TaskViews::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let mut w = watch();
        scene.agents.get_mut(&ids[0]).unwrap().state = ActivityState::Running { item: None };
        w.observe_on("2026-11-05", &scene, &views, now);
        w.observe_on("2026-11-05", &scene, &views, now);
        assert!(!w.unsaved(), "nothing happened between the two frames");
        scene.agents.get_mut(&ids[0]).unwrap().tokens_used += 10;
        w.observe_on("2026-11-05", &scene, &views, now);
        assert!(w.unsaved(), "tokens were spent");
        w.unsaved = false;
        scene.agents.get_mut(&ids[0]).unwrap().state = ActivityState::Idle;
        w.observe_on("2026-11-05", &scene, &views, now);
        assert!(w.unsaved(), "a turn finished");
    }

    /// A disk failure must not throw away the only dirty marker: after the
    /// path is repaired, quitting via `flush` still has to persist the counts.
    /// A successful flush must then be quiet rather than writing every time.
    #[test]
    fn a_failed_save_stays_dirty_and_flush_writes_after_recovery() {
        let dir =
            std::env::temp_dir().join(format!("agent-office-stats-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the record's parent directory must be makes every real
        // tmp-file open fail without depending on permissions or a fake path.
        let parent = dir.join("record");
        std::fs::write(&parent, "not a directory").unwrap();
        let path = parent.join("stats.json");
        let mut w = Watch {
            path: Some(path.clone()),
            ..watch()
        };
        w.tally("test.recovery");
        w.save();
        assert!(w.unsaved(), "a failed write must stay dirty");
        w.flush();
        assert!(w.unsaved(), "another failed flush must not give up");
        std::fs::remove_file(&parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        w.flush();
        assert!(!w.unsaved(), "the complete recovered write cleared dirty");
        let saved: Stats = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, w.stats);
        let saved_at = w.saved_at;
        w.flush();
        assert_eq!(w.saved_at, saved_at, "a clean flush does not write again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed old-day write must stay ahead of the new day, without pausing
    /// new counts. Otherwise recovering at quit silently replaces the only copy
    /// of the dirty old day with today's record.
    #[test]
    fn rollover_retries_the_failed_old_day_before_the_new_day() {
        let dir = std::env::temp_dir().join(format!(
            "agent-office-stats-rollover-retry-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let parent = dir.join("record");
        std::fs::write(&parent, "not a directory").unwrap();
        let path = parent.join("stats.json");
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        let views = TaskViews::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let mut w = Watch {
            path: Some(path.clone()),
            ..watch()
        };

        finish_turn(&mut w, &mut scene, ids[0], "2026-11-05", &views, now);
        w.tally("test.before_rollover");
        let old = w.stats.clone();
        w.save();
        assert!(w.unsaved());
        assert!(!path.exists(), "the failed write created no record");

        finish_turn(&mut w, &mut scene, ids[0], "2026-11-06", &views, now);
        let current = w.stats.clone();
        assert_eq!(current.turns, 1, "the new day keeps counting");
        assert_eq!(current.repos.get("shop"), Some(&2));
        assert_eq!(current.uses.get("test.before_rollover"), Some(&1));

        std::fs::remove_file(&parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let mut writes: Vec<Stats> = Vec::new();
        w.flush_using(|path, tmp, json| {
            writes.push(serde_json::from_slice(json).unwrap());
            crate::atomic::write_private(path, tmp, json)
        });
        assert_eq!(writes, vec![old, current.clone()]);
        assert!(!w.unsaved());
        let saved: Stats = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, current, "the newest complete record is last on disk");

        w.observe_on("2026-11-06", &scene, &views, now);
        w.flush_using(|_, _, _| -> std::io::Result<()> {
            panic!("a fully recovered, unchanged watch must not write again")
        });
        assert_eq!(
            w.stats, current,
            "observing the same frame counts nothing twice"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Counts made inside the ordinary ten-second debounce window are just as
    /// dirty as counts whose write failed: a midnight rollover must queue them.
    #[test]
    fn debounce_dirty_counts_survive_rollover() {
        let dir = std::env::temp_dir().join(format!(
            "agent-office-stats-rollover-debounce-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stats.json");
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        let views = TaskViews::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let mut w = Watch {
            path: Some(path.clone()),
            ..watch()
        };

        w.saved_at = Some(Instant::now());
        finish_turn(&mut w, &mut scene, ids[0], "2026-11-05", &views, now);
        let old = w.stats.clone();
        // Direct observation is what `observe` does while the recent saved_at
        // suppresses its write; there is deliberately no successful old-day save.
        finish_turn(&mut w, &mut scene, ids[0], "2026-11-06", &views, now);
        let current = w.stats.clone();
        assert!(w.unsaved());

        let mut writes: Vec<Stats> = Vec::new();
        w.flush_using(|path, tmp, json| {
            writes.push(serde_json::from_slice(json).unwrap());
            crate::atomic::write_private(path, tmp, json)
        });
        assert_eq!(writes, vec![old, current.clone()]);
        let saved: Stats = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, current);
        assert_eq!(
            saved.turns, 1,
            "today is counted once, not added to yesterday"
        );
        assert_eq!(saved.repos.get("shop"), Some(&2));

        w.observe_on("2026-11-06", &scene, &views, now);
        w.flush_using(|_, _, _| -> std::io::Result<()> {
            panic!("a clean flush must not write again")
        });
        assert_eq!(w.stats, current);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// More than one day can remain dirty when writes keep failing. Each
    /// rollover must append its own snapshot, never overwrite an older pending day.
    #[test]
    fn repeated_rollovers_preserve_all_earlier_failed_snapshots() {
        let dir = std::env::temp_dir().join(format!(
            "agent-office-stats-rollover-queue-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let parent = dir.join("record");
        std::fs::write(&parent, "not a directory").unwrap();
        let path = parent.join("stats.json");
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        let views = TaskViews::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let mut w = Watch {
            path: Some(path.clone()),
            ..watch()
        };

        finish_turn(&mut w, &mut scene, ids[0], "2026-11-05", &views, now);
        let oldest = w.stats.clone();
        w.save();
        assert!(w.unsaved());
        finish_turn(&mut w, &mut scene, ids[0], "2026-11-06", &views, now);
        let middle = w.stats.clone();
        w.save();
        assert!(
            w.unsaved(),
            "the second failure keeps every pending day dirty"
        );
        finish_turn(&mut w, &mut scene, ids[0], "2026-11-07", &views, now);
        let newest = w.stats.clone();

        std::fs::remove_file(&parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let mut writes: Vec<Stats> = Vec::new();
        w.flush_using(|path, tmp, json| {
            writes.push(serde_json::from_slice(json).unwrap());
            crate::atomic::write_private(path, tmp, json)
        });
        assert_eq!(writes, vec![oldest, middle, newest.clone()]);
        assert!(!w.unsaved());
        let saved: Stats = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, newest);
        assert_eq!(saved.repos.get("shop"), Some(&3));

        w.observe_on("2026-11-07", &scene, &views, now);
        w.flush_using(|_, _, _| -> std::io::Result<()> {
            panic!("draining every day exactly once must leave a clean watch")
        });
        assert_eq!(w.stats, newest);
        let _ = std::fs::remove_dir_all(&dir);
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

    /// A stuck spell counts once however long it lasts; recovering and then
    /// getting stuck again is a new spell.
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
        w.observe_on("d", &scene, &ok, now);
        w.observe_on("d", &scene, &stuck, now);
        w.observe_on("d", &scene, &stuck, now);
        assert_eq!(w.stats.stuck, 2, "only recovery starts another spell");
    }

    /// A spell still stuck at midnight starts one new segment today, timed
    /// from today's first observation. Time before that observation (while
    /// the office may have been asleep) is unknown, not free stuck duration.
    #[test]
    fn stuck_across_midnight_starts_a_new_daily_segment() {
        use crate::tasks::{Stuck, TaskView};
        let (mut scene, ids) = scene_with(&[("a", "/src/shop")]);
        scene.agents.get_mut(&ids[0]).unwrap().state = ActivityState::Running { item: None };
        let at_23_59 = SystemTime::UNIX_EPOCH + Duration::from_secs(23 * 3600 + 59 * 60);
        let mut w = watch();
        let ok = TaskViews::default();
        let stuck: TaskViews = Arc::new(HashMap::from([(
            ids[0],
            TaskView {
                stuck: Some(Stuck::Failing(3)),
                ..TaskView::default()
            },
        )]));
        w.observe_on("2026-11-05", &scene, &ok, at_23_59);
        w.observe_on("2026-11-05", &scene, &stuck, at_23_59);
        assert_eq!(w.stats.stuck, 1, "yesterday's spell still counts once");

        let name = String::from(&*scene.agents[&ids[0]].label);
        w.observe_on(
            "2026-11-06",
            &scene,
            &stuck,
            at_23_59 + Duration::from_secs(6 * 60),
        );
        assert_eq!(
            (w.stats.stuck, w.stats.stuck_longest.get(&name)),
            (1, Some(&0)),
            "00:05 starts today's segment without crediting time before it"
        );
        w.observe_on(
            "2026-11-06",
            &scene,
            &stuck,
            at_23_59 + Duration::from_secs(11 * 60),
        );
        assert_eq!(
            (w.stats.stuck, w.stats.stuck_longest.get(&name)),
            (1, Some(&300)),
            "00:10 extends only today's segment, without counting it again"
        );
    }

    /// The replay's honours come from today's own record: who finished most,
    /// and whose stuck spell ran longest, measured from when it began, not
    /// summed over frames, or a long-open office would inflate every spell.
    #[test]
    fn the_day_names_who_finished_most_and_who_was_stuck_longest() {
        use crate::tasks::{Stuck, TaskView};
        let (mut scene, ids) = scene_with(&[("a", "/src/shop"), ("b", "/src/blog")]);
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let mut w = watch();
        let none = TaskViews::default();
        let label = |scene: &SceneState, i: usize| String::from(&*scene.agents[&ids[i]].label);
        let set =
            |scene: &mut SceneState, i: usize, s| scene.agents.get_mut(&ids[i]).unwrap().state = s;
        let run = ActivityState::Running { item: None };
        set(&mut scene, 0, run.clone());
        set(&mut scene, 1, run.clone());
        w.observe_on("d", &scene, &none, t0);
        // a finishes twice, b once.
        for _ in 0..2 {
            set(&mut scene, 0, ActivityState::Idle);
            w.observe_on("d", &scene, &none, t0);
            set(&mut scene, 0, run.clone());
            w.observe_on("d", &scene, &none, t0);
        }
        set(&mut scene, 1, ActivityState::Idle);
        w.observe_on("d", &scene, &none, t0);
        // b, working again, is stuck for five minutes.
        set(&mut scene, 1, run.clone());
        let stuck: TaskViews = Arc::new(HashMap::from([(
            ids[1],
            TaskView {
                stuck: Some(Stuck::Quiet(5)),
                ..TaskView::default()
            },
        )]));
        for m in 0..=5 {
            w.observe_on("d", &scene, &stuck, t0 + Duration::from_secs(60 * m));
        }
        w.observe_on("d", &scene, &none, t0 + Duration::from_secs(400));
        let recap = w.stats.recap();
        assert_eq!(recap.most, Some((label(&scene, 0), 2)));
        assert_eq!(recap.stuck, Some((label(&scene, 1), 300)));
        assert_eq!(
            w.stats.hours.iter().sum::<u32>(),
            3,
            "each turn in its hour"
        );
    }

    /// A record written before the replay existed still loads, its repository
    /// stars kept: a parse failure would reset every bench to nothing.
    #[test]
    fn an_older_record_still_loads() {
        let old = r#"{"day":"2026-11-04","turns":3,"stuck":1,"tokens":9,"repos":{"shop":12}}"#;
        let stats: Stats = serde_json::from_str(old).expect("loads");
        assert_eq!(stats.repos.get("shop"), Some(&12));
        assert_eq!(stats.recap().most, None);
    }
}
