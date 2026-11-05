//! What the desktop apps say you looked at: a result read there is as read as
//! one opened from the office.
//!
//! The Claude app keeps a metadata file per session it hosts, `lastFocusedAt`
//! in it the last time you switched to that session. The format is the app's
//! own, not a published one, so a file that does not read as expected says
//! nothing and the office keeps to its own opens.
//!
//! DeepSeek Harness says nothing of the kind; agent-office's dsh plugin
//! writes which sessions its clients show now ([`crate::dsh::viewing_dir`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pixtuoid_core::AgentId;
use tokio::sync::watch;

/// How often the files are looked at: a switch in the app shows within this.
const EVERY: Duration = Duration::from_secs(2);

/// The apps' bundle identifiers.
#[cfg(target_os = "macos")]
const CLAUDE_ID: &str = "com.anthropic.claudefordesktop";
#[cfg(target_os = "macos")]
const DSH_ID: &str = "com.deepseek.dsh";

/// When you last switched to each session the Claude app hosts.
pub type Focus = HashMap<AgentId, SystemTime>;

/// What the apps say you look at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Looks {
    pub claude: Focus,
    /// The dsh sessions a DeepSeek Harness client shows now.
    pub dsh: Vec<AgentId>,
}

/// The app in front, of the ones that say what you look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Claude,
    Dsh,
    Other,
}

/// Watch what the apps write; empty where they write nothing.
pub fn spawn() -> watch::Receiver<Looks> {
    let (tx, rx) = watch::channel(Looks::default());
    // Only read while DeepSeek Harness is in front, which needs macOS to tell.
    let viewing = crate::dsh::viewing_dir().filter(|_| cfg!(target_os = "macos"));
    let root = sessions_dir();
    if root.is_none() && viewing.is_none() {
        return rx;
    }
    // Plain file reads, kept off the runtime's workers.
    std::thread::spawn(move || {
        let mut reader = Reader::default();
        loop {
            let looks = Looks {
                claude: root.as_deref().map(|r| reader.read(r)).unwrap_or_default(),
                dsh: viewing.as_deref().map(read_viewing).unwrap_or_default(),
            };
            tx.send_if_modified(|old| {
                let changed = *old != looks;
                *old = looks;
                changed
            });
            if tx.is_closed() {
                return;
            }
            std::thread::sleep(EVERY);
        }
    });
    rx
}

fn sessions_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Claude/claude-code-sessions"))
}

/// Mark `seen` with what the apps say you looked at, for the sessions `live`
/// holds. A switch stamps a Claude session only when you switch to it, so one
/// you stay on and watch finish would read unread: while the app is in
/// front, the session switched to last is the one on screen, read as of
/// `now`. A dsh client shows its sessions whether or not you are looking, so
/// they are read only while DeepSeek Harness is in front.
pub fn mark_read(
    seen: &mut HashMap<AgentId, SystemTime>,
    looks: &Looks,
    live: impl Fn(&AgentId) -> bool,
    front: Front,
    now: SystemTime,
) {
    for (&id, &at) in looks.claude.iter().filter(|(id, _)| live(id)) {
        note_latest(seen, id, at);
    }
    let on_screen: Vec<AgentId> = match front {
        Front::Claude => looks
            .claude
            .iter()
            .max_by_key(|(_, at)| **at)
            .map(|(&id, _)| id)
            .into_iter()
            .collect(),
        Front::Dsh => looks.dsh.clone(),
        Front::Other => Vec::new(),
    };
    for id in on_screen.into_iter().filter(|id| live(id)) {
        note_latest(seen, id, now);
    }
}

fn note_latest(map: &mut HashMap<AgentId, SystemTime>, id: AgentId, at: SystemTime) {
    let e = map.entry(id).or_insert(at);
    *e = (*e).max(at);
}

/// Which app is in front. AppKit's, so on the main thread.
#[cfg(target_os = "macos")]
pub fn front() -> Front {
    use objc2_app_kit::NSWorkspace;
    // SAFETY: property reads on the shared workspace, on the main thread.
    let id = unsafe {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.bundleIdentifier())
            .map(|id| id.to_string())
    };
    match id.as_deref() {
        Some(CLAUDE_ID) => Front::Claude,
        Some(DSH_ID) => Front::Dsh,
        _ => Front::Other,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn front() -> Front {
    Front::Other
}

/// The dsh sessions the plugin's files say a client shows, every host's,
/// keyed as the office keys dsh sessions. A file left by a host that has
/// since gone (a crash skips the plugin's cleanup) says nothing: those
/// streams are shut.
fn read_viewing(dir: &Path) -> Vec<AgentId> {
    let alive = |pid| pixtuoid_core::source::pid_start_marker(pid).is_some();
    let mut ids: Vec<AgentId> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|t| viewing(&t, alive))
        .flatten()
        .collect();
    // Sorted, so the same views read the same and wake no one.
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn viewing(text: &str, alive: impl Fn(i32) -> bool) -> Option<Vec<AgentId>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let pid = i32::try_from(v.get("pid")?.as_i64()?).ok()?;
    if !alive(pid) {
        return None;
    }
    Some(
        v.get("sessions")?
            .as_array()?
            .iter()
            .filter_map(|s| s.as_str().filter(|s| !s.is_empty()))
            .map(|s| AgentId::from_parts(crate::dsh::SOURCE, s))
            .collect(),
    )
}

/// The session a metadata file is for, keyed as the office keys Claude Code
/// sessions, and when you last switched to it.
fn focused(text: &str) -> Option<(AgentId, SystemTime)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let id = v.get("cliSessionId")?.as_str().filter(|s| !s.is_empty())?;
    let ms = v.get("lastFocusedAt")?.as_u64()?;
    Some((
        AgentId::from_parts(crate::pipeline::CC_SOURCE, id),
        SystemTime::UNIX_EPOCH + Duration::from_millis(ms),
    ))
}

/// Reads the files, parsing again only those written since the last read:
/// there is one per session the app ever hosted. Only a file that read is
/// kept: one caught half written is read again next time, not taken as
/// saying nothing until the app next writes it.
#[derive(Default)]
struct Reader {
    cache: HashMap<PathBuf, (SystemTime, (AgentId, SystemTime))>,
}

impl Reader {
    fn read(&mut self, root: &Path) -> Focus {
        let mut next = HashMap::new();
        for (path, written) in session_files(root) {
            let hit = match self.cache.remove(&path) {
                Some((at, hit)) if at == written => Some(hit),
                _ => std::fs::read_to_string(&path)
                    .ok()
                    .as_deref()
                    .and_then(focused),
            };
            if let Some(hit) = hit {
                next.insert(path, (written, hit));
            }
        }
        self.cache = next;
        let mut focus = Focus::new();
        for &(_, (id, at)) in self.cache.values() {
            note_latest(&mut focus, id, at);
        }
        focus
    }
}

/// `<account>/<org>/local_<id>.json` under `root`, with when each was written.
fn session_files(root: &Path) -> Vec<(PathBuf, SystemTime)> {
    let entries = |p: &Path| {
        std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
    };
    let mut out = Vec::new();
    for account in entries(root) {
        for org in entries(&account) {
            for f in entries(&org) {
                let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("local_")
                    && name.ends_with(".json")
                    && let Ok(written) = f.metadata().and_then(|m| m.modified())
                {
                    out.push((f, written));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    fn cc(id: &str) -> AgentId {
        AgentId::from_parts(crate::pipeline::CC_SOURCE, id)
    }

    /// The file names the session by Claude Code's own id, the one its
    /// transcript and the office's figure go by; anything else in it, or a
    /// file half written, says nothing rather than mark a session read.
    #[test]
    fn a_session_file_reads_as_the_session_and_its_last_switch() {
        let text = r#"{"sessionId":"local_x","cliSessionId":"abc","lastFocusedAt":1800000000500}"#;
        assert_eq!(
            focused(text),
            Some((cc("abc"), at(0) + Duration::from_millis(500)))
        );
        assert_eq!(
            focused(r#"{"cliSessionId":"abc"}"#),
            None,
            "never switched to"
        );
        assert_eq!(
            focused(r#"{"lastFocusedAt":1}"#),
            None,
            "no CLI session yet"
        );
        assert_eq!(focused(r#"{"cliSessionId":"abc","lastFo"#), None);
    }

    /// A switch after it finished is a read; one before is not, since what
    /// it went on to do came after you looked. Sessions not in the office
    /// stay out of `seen`.
    #[test]
    fn a_switch_marks_read_what_was_there_to_read() {
        let focus = Focus::from([(cc("a"), at(100)), (cc("b"), at(50)), (cc("gone"), at(10))]);
        let mut seen = HashMap::from([(cc("b"), at(80))]);
        let live = |id: &AgentId| *id != cc("gone");
        mark_read(&mut seen, &claude(focus), live, Front::Other, at(200));
        assert_eq!(seen[&cc("a")], at(100));
        assert_eq!(
            seen[&cc("b")],
            at(80),
            "an open from the office later still counts"
        );
        assert!(!seen.contains_key(&cc("gone")));
    }

    /// You stay on a session and watch it finish: no switch stamps it, but
    /// with the app in front it is the one on screen. The others it hosts,
    /// out of view, are not.
    #[test]
    fn the_session_on_screen_reads_as_read_while_the_app_is_in_front() {
        let looks = claude(Focus::from([(cc("on"), at(100)), (cc("off"), at(50))]));
        let mut seen = HashMap::new();
        mark_read(&mut seen, &looks, |_| true, Front::Claude, at(200));
        assert_eq!(seen[&cc("on")], at(200));
        assert_eq!(seen[&cc("off")], at(50));
        for front in [Front::Other, Front::Dsh] {
            let mut away = HashMap::new();
            mark_read(&mut away, &looks, |_| true, front, at(200));
            assert_eq!(away[&cc("on")], at(100), "the app behind: only the switch");
        }
    }

    fn claude(focus: Focus) -> Looks {
        Looks {
            claude: focus,
            dsh: Vec::new(),
        }
    }

    fn dsh(id: &str) -> AgentId {
        AgentId::from_parts(crate::dsh::SOURCE, id)
    }

    /// A dsh client keeps its session on screen with the app behind another,
    /// so only with DeepSeek Harness in front is what it shows read: a reply
    /// that lands while you are in another app still waits for you.
    #[test]
    fn a_dsh_session_on_screen_reads_as_read_only_with_the_app_in_front() {
        let looks = Looks {
            claude: Focus::new(),
            dsh: vec![dsh("on"), dsh("gone")],
        };
        let live = |id: &AgentId| *id != dsh("gone");
        let mut seen = HashMap::new();
        mark_read(&mut seen, &looks, live, Front::Dsh, at(200));
        assert_eq!(seen, HashMap::from([(dsh("on"), at(200))]));
        for front in [Front::Other, Front::Claude] {
            let mut away = HashMap::new();
            mark_read(&mut away, &looks, live, front, at(200));
            assert!(away.is_empty(), "DeepSeek Harness behind: {front:?}");
        }
    }

    /// The plugin's file names sessions by dsh's own id, the one the office's
    /// dsh figures go by. One left by a host that is gone says nothing, or a
    /// crash would leave its last session read for good.
    #[test]
    fn the_viewing_file_counts_only_while_its_host_runs() {
        let text = r#"{"pid":42,"sessions":["session-a","",7]}"#;
        assert_eq!(viewing(text, |p| p == 42), Some(vec![dsh("session-a")]));
        assert_eq!(viewing(text, |_| false), None, "its host is gone");
        assert_eq!(viewing(r#"{"sessions":["session-a"]}"#, |_| true), None);
        assert_eq!(viewing(r#"{"pid":42,"sess"#, |_| true), None);
    }

    /// The app and a CLI can share one DSH_HOME, each host with its own
    /// file: one's views must not hide the other's.
    #[test]
    fn every_hosts_views_count() {
        let dir = std::env::temp_dir().join(format!("ao-viewing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        let host = |pid: u32, ids: &str| format!(r#"{{"pid":{pid},"sessions":[{ids}]}}"#);
        std::fs::write(dir.join("a.json"), host(me, r#""s1","s2""#)).unwrap();
        std::fs::write(dir.join("b.json"), host(me, r#""s2","s3""#)).unwrap();
        std::fs::write(dir.join("b.json.tmp"), host(me, r#""half""#)).unwrap();
        let mut want = vec![dsh("s1"), dsh("s2"), dsh("s3")];
        want.sort_unstable();
        assert_eq!(read_viewing(&dir), want);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A session's switch shows once its file is rewritten; a file caught
    /// half written is read again, even if the finished write leaves the
    /// same mtime, rather than dropping that switch until the next one.
    #[test]
    fn the_reader_follows_the_files_the_app_writes() {
        let root = std::env::temp_dir().join(format!("ao-desktop-{}", std::process::id()));
        let org = root.join("acct/org");
        std::fs::create_dir_all(&org).unwrap();
        let file = org.join("local_1.json");
        // The mtime set by hand: the cache keys on it, and a filesystem's
        // own resolution may be too coarse to tell two writes apart.
        let write = |text: &str, mtime: SystemTime| {
            std::fs::write(&file, text).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(mtime)
                .unwrap();
        };
        let json = |ms: u64| format!(r#"{{"cliSessionId":"s","lastFocusedAt":{ms}}}"#);
        std::fs::write(org.join("deleted_2"), "{}").unwrap();
        let mut r = Reader::default();
        write(r#"{"cliSessionId":"s","lastFo"#, at(1));
        assert_eq!(r.read(&root), Focus::new(), "half written");
        write(&json(1_800_000_000_000), at(1));
        assert_eq!(r.read(&root), Focus::from([(cc("s"), at(0))]));
        write(&json(1_800_000_060_000), at(61));
        assert_eq!(r.read(&root), Focus::from([(cc("s"), at(60))]));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
