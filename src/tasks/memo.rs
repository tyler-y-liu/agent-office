//! Lines the model already wrote, by the exact request that asked for them,
//! kept on disk: a restart, or a session leaving the office and coming back,
//! would otherwise ask again for every idle session's task and outcome.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The most lines kept; the oldest go first.
const KEEP: usize = 2_000;

#[derive(Debug, Default)]
pub struct Memo {
    path: Option<PathBuf>,
    /// Oldest first.
    order: Vec<String>,
    lines: HashMap<String, String>,
    /// Since when a line has been waiting to be written out.
    unsaved_since: Option<Instant>,
}

/// How long after a new line the memo is written out: a burst of replies
/// (the office opening) costs one write.
const SAVE_AFTER: Duration = Duration::from_secs(5);

/// The key for a request: everything that decides the reply. FNV-1a, not
/// std's hasher, whose output may change between Rust releases and would
/// turn every stored line into a miss.
pub fn key(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        // A separator no text holds, so ["ab", "c"] and ["a", "bc"] differ.
        for b in part.bytes().chain([0xff]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

impl Memo {
    /// The memo at `~/.config/agent-office/summaries.json`.
    pub fn load() -> Self {
        let path = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config/agent-office/summaries.json"));
        let order: Vec<(String, String)> = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut memo = Self {
            path,
            ..Self::default()
        };
        for (k, v) in order {
            memo.put(k, v);
        }
        memo
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.get(key).map(String::as_str)
    }

    /// Keep `line` for `key`; [`Self::save_due`] writes it out.
    pub fn insert(&mut self, key: String, line: String) {
        self.put(key, line);
        self.unsaved_since.get_or_insert_with(Instant::now);
    }

    /// Write the memo out if a line has waited [`SAVE_AFTER`].
    pub fn save_due(&mut self, now: Instant) {
        if self
            .unsaved_since
            .is_some_and(|t| now.duration_since(t) >= SAVE_AFTER)
        {
            self.unsaved_since = None;
            self.save();
        }
    }

    fn put(&mut self, key: String, line: String) {
        if self.lines.insert(key.clone(), line).is_none() {
            self.order.push(key);
        }
        if self.order.len() > KEEP {
            let drop = self.order.len() - KEEP;
            for k in self.order.drain(..drop) {
                self.lines.remove(&k);
            }
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let pairs: Vec<(&String, &String)> = self
            .order
            .iter()
            .filter_map(|k| Some((k, self.lines.get(k)?)))
            .collect();
        if let Ok(json) = serde_json::to_string(&pairs) {
            // Through a temporary file, so a crash mid-write keeps the old
            // memo; 0600 because it holds task titles and questions.
            let tmp = path.with_extension("json.tmp");
            let _ = crate::atomic::write_private(path, &tmp, json.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The memo holds task titles and what sessions are asking you: other
    /// users of the Mac must not read it, even where an older build left it
    /// 0644.
    #[cfg(unix)]
    #[test]
    fn the_saved_memo_is_readable_only_by_its_owner() {
        use crate::atomic::tests::{mode, scratch};
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("memo");
        let path = dir.join("summaries.json");
        let mut m = Memo {
            path: Some(path.clone()),
            ..Memo::default()
        };
        m.insert(key(&["task", "修登录页"]), "修复登录页".into());
        m.save();
        assert_eq!(mode(&path), 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        m.save();
        assert_eq!(mode(&path), 0o600);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("修复登录页")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same request finds its line again, a changed one does not: a
    /// stale line for a new prompt would be worse than asking.
    #[test]
    fn a_line_is_found_only_by_the_request_that_got_it() {
        let mut m = Memo::default();
        m.insert(key(&["task", "修登录页"]), "修复登录页".into());
        assert_eq!(m.get(&key(&["task", "修登录页"])), Some("修复登录页"));
        assert_eq!(m.get(&key(&["task", "修注册页"])), None);
        assert_eq!(m.get(&key(&["outcome", "修登录页"])), None);
        assert_ne!(key(&["ab", "c"]), key(&["a", "bc"]), "parts are kept apart");
        // Pinned: a stored memo must still be found after a rebuild.
        assert_eq!(key(&["a"]), "089bc907b544c769");
    }

    /// The memo stays bounded however long the office runs, dropping the
    /// oldest lines first.
    #[test]
    fn the_oldest_lines_go_once_it_is_full() {
        let mut m = Memo::default();
        for i in 0..KEEP + 5 {
            m.insert(format!("k{i}"), format!("v{i}"));
        }
        assert_eq!(m.lines.len(), KEEP);
        assert_eq!(m.get("k0"), None);
        assert_eq!(
            m.get(&format!("k{}", KEEP + 4)),
            Some(&*format!("v{}", KEEP + 4))
        );
    }
}
