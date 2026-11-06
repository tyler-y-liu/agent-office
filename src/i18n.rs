//! The language the office speaks: Simplified Chinese or English.
//!
//! Picked at start: `AGENT_OFFICE_LANG` (environment, else
//! `~/.config/agent-office/env`), else the macOS UI language, else the POSIX
//! locale. Anything not Chinese gets English, which more people read.
//! [`switch`] changes it while running and saves it for the next start.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

/// 0 until picked, then [`code`] of the language.
static LANG: AtomicU8 = AtomicU8::new(0);

fn code(l: Lang) -> u8 {
    match l {
        Lang::Zh => 1,
        Lang::En => 2,
    }
}

thread_local! {
    /// A test's language, so tests of either language run side by side
    /// without racing on the process-wide pick.
    static FORCED: Cell<Option<Lang>> = const { Cell::new(None) };
}

pub fn lang() -> Lang {
    if let Some(l) = FORCED.with(Cell::get) {
        return l;
    }
    // Tests assert on the Chinese the office was written in, whatever the
    // locale of the machine running them.
    if cfg!(test) {
        return Lang::Zh;
    }
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Zh,
        2 => Lang::En,
        _ => {
            let l = detect();
            LANG.store(code(l), Ordering::Relaxed);
            l
        }
    }
}

/// Speak the other language from now on, and save it to the env file so
/// the next start speaks it too. The switch holds even when saving fails.
pub fn switch() -> (Lang, std::io::Result<()>) {
    let next = match lang() {
        Lang::Zh => Lang::En,
        Lang::En => Lang::Zh,
    };
    LANG.store(code(next), Ordering::Relaxed);
    (next, save(next))
}

/// `AGENT_OFFICE_LANG` in the environment, which wins over the env file at
/// the next start.
pub fn pinned_by_env() -> Option<Lang> {
    env_lang().and_then(|v| parse(&v))
}

/// Set and not empty: an exported empty value must not hide the env file.
fn env_lang() -> Option<String> {
    std::env::var("AGENT_OFFICE_LANG")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

fn save(l: Lang) -> std::io::Result<()> {
    let home = std::env::var_os("HOME").ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    let path = std::path::PathBuf::from(home).join(crate::tasks::client::ENV_FILE);
    // Through a symlink (a dotfiles repo, say) to the file it names, so the
    // rename below replaces that file and not the link.
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let old = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let value = match l {
        Lang::Zh => "zh",
        Lang::En => "en",
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written aside and renamed in: the file holds the summary API key, which
    // a crash mid-write must not lose. A new file is 0600 so the key is not
    // exposed; an existing one keeps the mode its owner gave it.
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(&tmp)?;
    if let Ok(m) = std::fs::metadata(&path) {
        f.set_permissions(m.permissions())?;
    }
    std::io::Write::write_all(
        &mut f,
        with_var(&old, "AGENT_OFFICE_LANG", value).as_bytes(),
    )?;
    std::fs::rename(&tmp, &path)
}

/// `file` with `key` set to `value`: its line rewritten where it has one,
/// else one added, and every other line as it was.
fn with_var(file: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut out: Vec<String> = file
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let (export, rest) = match t.strip_prefix("export ") {
                Some(r) => ("export ", r),
                None => ("", t),
            };
            let ours =
                !t.starts_with('#') && rest.split_once('=').is_some_and(|(k, _)| k.trim() == key);
            if ours {
                found = true;
                format!("{export}{key}={value}")
            } else {
                l.to_owned()
            }
        })
        .collect();
    if !found {
        out.push(format!("{key}={value}"));
    }
    out.join("\n") + "\n"
}

pub fn is_zh() -> bool {
    lang() == Lang::Zh
}

/// `zh` or `en`, by the office's language.
pub fn t(zh: &'static str, en: &'static str) -> &'static str {
    match lang() {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

/// `format!` of `zh` or of `en`, by the office's language; both get the
/// same arguments.
#[macro_export]
macro_rules! tr {
    ($zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::is_zh() {
            format!($zh $(, $arg)*)
        } else {
            format!($en $(, $arg)*)
        }
    };
}

/// Run `f` speaking `lang` on this thread.
#[cfg(test)]
pub fn with<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    let before = FORCED.with(|c| c.replace(Some(lang)));
    let r = f();
    FORCED.with(|c| c.set(before));
    r
}

fn detect() -> Lang {
    let file = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(crate::tasks::client::ENV_FILE))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let chosen = env_lang()
        .or_else(|| crate::tasks::client::env_file_var(&file, "AGENT_OFFICE_LANG"))
        .and_then(|v| parse(&v));
    chosen
        .or_else(apple_language)
        .or_else(posix_locale)
        .unwrap_or(Lang::En)
}

/// `zh`, `zh-CN`, `zh_Hans`, `en_US.UTF-8`… as a language; `C` and the like
/// say nothing.
fn parse(tag: &str) -> Option<Lang> {
    let tag = tag
        .trim()
        .trim_matches(['"', '(', ')', ',', ' '])
        .to_ascii_lowercase();
    if tag.starts_with("zh") {
        Some(Lang::Zh)
    } else if tag.starts_with("en") {
        Some(Lang::En)
    } else if tag.is_empty() || tag == "c" || tag == "posix" || tag.starts_with("c.") {
        None
    } else {
        // Another language: English is the nearer fallback for its reader.
        Some(Lang::En)
    }
}

/// The first of the user's preferred UI languages: the Terminal sets `LANG`
/// from the region, which a Chinese reader in another region would not want.
fn apple_language() -> Option<Lang> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().find_map(|l| {
        let l = l.trim().trim_matches(['"', ',', ' ']);
        (!l.is_empty() && l != "(" && l != ")")
            .then(|| parse(l))
            .flatten()
    })
}

fn posix_locale() -> Option<Lang> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .and_then(|v| parse(&v))
}

/// Marks an outcome that asks the developer something, as the summary model
/// writes it in either language.
const ASK_MARKS: [&str; 3] = ["问：", "Q: ", "Q:"];

/// `line` without its question mark prefix, if it has one. A line kept from
/// a run in the other language still reads as a question.
pub fn strip_ask(line: &str) -> Option<&str> {
    ASK_MARKS.iter().find_map(|m| line.strip_prefix(m))
}

pub fn is_ask(line: &str) -> bool {
    strip_ask(line).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A locale must pick the language its reader reads: a Chinese Mac
    /// showing English, or an English one Chinese, is the bug this fixes.
    #[test]
    fn locale_tags_pick_their_language() {
        for zh in ["zh", "zh-Hans-CN", "zh_CN.UTF-8", "ZH_tw", "\"zh-Hant\","] {
            assert_eq!(parse(zh), Some(Lang::Zh), "{zh}");
        }
        for en in ["en", "en-US", "en_GB.UTF-8", "fr_FR.UTF-8"] {
            assert_eq!(parse(en), Some(Lang::En), "{en}");
        }
        // `C` says nothing about the reader, so the next source decides.
        for none in ["", "C", "C.UTF-8", "POSIX"] {
            assert_eq!(parse(none), None, "{none}");
        }
    }

    /// The language switch rewrites the env file that also holds the summary
    /// API key: the key and comments must survive, and the next start must
    /// read the new language, not a stale line left above it.
    #[test]
    fn saving_the_language_keeps_the_rest_of_the_env_file() {
        let file = "# summary\nAGENT_OFFICE_API_KEY=sk-x\nexport AGENT_OFFICE_LANG=\"zh\"\n# AGENT_OFFICE_LANG=fr\n";
        let out = with_var(file, "AGENT_OFFICE_LANG", "en");
        assert_eq!(
            out,
            "# summary\nAGENT_OFFICE_API_KEY=sk-x\nexport AGENT_OFFICE_LANG=en\n# AGENT_OFFICE_LANG=fr\n"
        );
        let read = crate::tasks::client::env_file_var(&out, "AGENT_OFFICE_LANG");
        assert_eq!(read.as_deref().and_then(parse), Some(Lang::En));

        let added = with_var("AGENT_OFFICE_API_KEY=sk-x", "AGENT_OFFICE_LANG", "zh");
        assert_eq!(added, "AGENT_OFFICE_API_KEY=sk-x\nAGENT_OFFICE_LANG=zh\n");
        assert_eq!(
            with_var("", "AGENT_OFFICE_LANG", "zh"),
            "AGENT_OFFICE_LANG=zh\n"
        );
    }

    /// A question is what lines a session up as waiting on you; one written
    /// in the other language (a memo from before a switch) must still count.
    #[test]
    fn a_question_reads_in_either_language() {
        assert_eq!(strip_ask("问：要合并吗"), Some("要合并吗"));
        assert_eq!(strip_ask("Q: merge it?"), Some("merge it?"));
        assert_eq!(strip_ask("Fixed the login page"), None);
    }
}
