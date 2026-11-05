//! What the Claude desktop app says you looked at. It keeps a metadata file
//! per session it hosts, `lastFocusedAt` in it the last time you switched to
//! that session: a result read there is as read as one opened from the office.
//! The format is the app's own, not a published one, so a file that does not
//! read as expected says nothing and the office keeps to its own opens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pixtuoid_core::AgentId;
use tokio::sync::watch;

/// How often the files are looked at: a switch in the app shows within this.
const EVERY: Duration = Duration::from_secs(2);

/// The desktop app's bundle identifier.
#[cfg(target_os = "macos")]
const APP_ID: &str = "com.anthropic.claudefordesktop";

/// When you last switched to each session the app hosts.
pub type Focus = HashMap<AgentId, SystemTime>;

/// Watch the app's session files; empty where there are none.
pub fn spawn() -> watch::Receiver<Focus> {
    let (tx, rx) = watch::channel(Focus::new());
    let Some(root) = sessions_dir() else {
        return rx;
    };
    // Plain file reads, kept off the runtime's workers.
    std::thread::spawn(move || {
        let mut reader = Reader::default();
        loop {
            let focus = reader.read(&root);
            tx.send_if_modified(|old| {
                let changed = *old != focus;
                *old = focus;
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

/// Mark `seen` with what the app says you looked at, for the sessions `live`
/// holds. A switch stamps a session only when you switch to it, so one you
/// stay on and watch finish would read unread: while the app is in front,
/// the session switched to last is the one on screen, read as of `now`.
pub fn mark_read(
    seen: &mut HashMap<AgentId, SystemTime>,
    focus: &Focus,
    live: impl Fn(&AgentId) -> bool,
    app_in_front: bool,
    now: SystemTime,
) {
    for (&id, &at) in focus.iter().filter(|(id, _)| live(id)) {
        note_latest(seen, id, at);
    }
    if app_in_front
        && let Some((&id, _)) = focus.iter().max_by_key(|(_, at)| **at)
        && live(&id)
    {
        note_latest(seen, id, now);
    }
}

fn note_latest(map: &mut HashMap<AgentId, SystemTime>, id: AgentId, at: SystemTime) {
    let e = map.entry(id).or_insert(at);
    *e = (*e).max(at);
}

/// Whether the desktop app is the front app. AppKit's, so on the main thread.
#[cfg(target_os = "macos")]
pub fn app_in_front() -> bool {
    use objc2_app_kit::NSWorkspace;
    // SAFETY: property reads on the shared workspace, on the main thread.
    unsafe {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.bundleIdentifier())
            .is_some_and(|id| id.to_string() == APP_ID)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn app_in_front() -> bool {
    false
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
        mark_read(&mut seen, &focus, live, false, at(200));
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
        let focus = Focus::from([(cc("on"), at(100)), (cc("off"), at(50))]);
        let mut seen = HashMap::new();
        mark_read(&mut seen, &focus, |_| true, true, at(200));
        assert_eq!(seen[&cc("on")], at(200));
        assert_eq!(seen[&cc("off")], at(50));
        let mut away = HashMap::new();
        mark_read(&mut away, &focus, |_| true, false, at(200));
        assert_eq!(away[&cc("on")], at(100), "the app behind: only the switch");
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
