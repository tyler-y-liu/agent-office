//! Cursor (`cursor-agent`): hook-only. `connect cursor` points every event of
//! Cursor's user-global `hooks.json` at `agent-office hook --source cursor`,
//! which forwards the payload the way pixtuoid's Cursor decoder reads it and
//! keeps a per-session task log the task reader tails: Cursor's own
//! transcript has the conversation but not the tool calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::pipeline::CURSOR_SOURCE;

/// The events pixtuoid's Cursor decoder reads (the prompt opens a turn), and
/// the agent's reply, which only the task log keeps.
const EVENTS: [&str; 8] = [
    "sessionStart",
    "beforeSubmitPrompt",
    "afterAgentResponse",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "stop",
    "sessionEnd",
];
pub const PROMPT_EVENT: &str = "beforeSubmitPrompt";
pub const REPLY_EVENT: &str = "afterAgentResponse";

/// The tool input fields the task reader shows or classifies by. Everything
/// else (file contents, edit strings, subagent prompts) stays out of the log.
const KEEP_ARGS: [&str; 8] = [
    "file_path",
    "path",
    "command",
    "description",
    "pattern",
    "query",
    "url",
    "todos",
];

/// Cursor's config dir as Cursor resolves it: `CURSOR_CONFIG_DIR`, else on
/// Linux a set `XDG_CONFIG_HOME`'s `cursor`, else `~/.cursor`.
fn config_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = env("CURSOR_CONFIG_DIR") {
        return Some(d);
    }
    if cfg!(all(unix, not(target_os = "macos")))
        && let Some(x) = env("XDG_CONFIG_HOME")
    {
        return Some(x.join("cursor"));
    }
    env("HOME").map(|h| h.join(".cursor"))
}

fn hooks_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("hooks.json"))
}

/// Where Cursor reads user-level agents from, beside its hooks: office roles
/// installed with `roles install --agent cursor` go here.
pub fn agents_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("agents"))
}

/// Where `cursor-agent` is: on PATH, else where its installer and Homebrew
/// put it.
pub fn bin() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        // A relative entry (`.`) would resolve inside the repository the
        // dispatch cd's into, running the repository's own `cursor-agent`.
        .filter(|d| d.is_absolute())
        .map(|d| d.join("cursor-agent"));
    let known = home
        .map(|h| h.join(".local/bin/cursor-agent"))
        .into_iter()
        .chain(
            [
                "/opt/homebrew/bin/cursor-agent",
                "/usr/local/bin/cursor-agent",
            ]
            .map(PathBuf::from),
        );
    on_path.chain(known).find(|p| p.is_file())
}

/// Arguments that start `cursor-agent` on a dispatched task in the directory
/// it was spawned in. Cursor has no office `--agent`, so the role card is in
/// `prompt`. This does not open the app.
pub fn submit_args(session_id: &str, prompt: &str) -> Vec<String> {
    vec![
        "--print".into(),
        "--trust".into(),
        "--force".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--new-session-id".into(),
        session_id.into(),
        prompt.into(),
    ]
}

/// `s` as one POSIX shell word: inside single quotes nothing is special but
/// the quote itself, which closes, is escaped and reopens.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Whether the Cursor app (the IDE) is installed where `open -a Cursor`
/// finds it: a fresh dispatch can open it with the prompt filled in.
pub fn app_installed() -> bool {
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications/Cursor.app"));
    std::iter::once(PathBuf::from("/Applications/Cursor.app"))
        .chain(home)
        .any(|p| p.is_dir())
}

// What the Cursor app's prompt link takes, as read from the `cursor-deeplink`
// extension of Cursor 3.29.9 (dist/main.js). A later Cursor may change any of
// it; a prompt it refuses only shows "Invalid text" there, while the office
// would believe it dispatched, so the rules here err on refusing.

/// Cursor refuses a prompt when `21 + encodeURIComponent(text).length` is
/// over this.
const APP_LINK_MAX: usize = 10_000;
const APP_LINK_OVERHEAD: usize = 21;
/// Cursor's "secret" patterns, each a run of parts matched in order within
/// one line (`a.*b`, case aside). `read.*\.env.*print` is left out: any
/// `.env` is already refused on its own. Cursor also decodes every word that
/// looks like base64 and checks that too; that layer is not repeated here
/// and no test covers it, so a role prompt with a word that happened to
/// decode to one of these would still be sent, and refused by Cursor.
const APP_REFUSED: [&[&str]; 7] = [
    &["cat", ".ssh/id_rsa"],
    &["upload", "passwords.txt"],
    &["dump", "token"],
    &["dump", "password"],
    &["exfiltrate", "credential"],
    &["leak", "secret"],
    &["how", "extract", "credential"],
];

/// Format characters (Unicode `Cf`) and private use, which Cursor's
/// character check refuses; with every control but tab and the line breaks
/// (`char::is_control`), that is all of `\p{C}` std can tell apart.
fn format_char(c: char) -> bool {
    matches!(u32::from(c),
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2
        | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F
        | 0xE000..=0xF8FF | 0xF0000..=0x10FFFF)
}

/// Whether `parts` occur in this order within one line of `lower`, as
/// Cursor's `a.*b` does (its `.` stops at line breaks).
fn in_one_line(lower: &str, parts: &[&str]) -> bool {
    lower
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .any(|line| {
            let mut rest = line;
            parts.iter().all(|p| match rest.find(p) {
                Some(i) => {
                    rest = &rest[i + p.len()..];
                    true
                }
                None => false,
            })
        })
}

/// What in `prompt` Cursor refuses outright.
fn app_refused(prompt: &str) -> Option<String> {
    if prompt.trim().is_empty() {
        return Some(crate::i18n::t("空的", "empty").into());
    }
    if prompt
        .chars()
        .any(|c| (c.is_control() && !matches!(c, '\t' | '\n' | '\r')) || format_char(c))
    {
        return Some(crate::i18n::t("格式字符或控制字符", "format or control characters").into());
    }
    let lower = prompt.to_lowercase();
    // `/\.env(\b|\W)/` without the `u` flag: `.env` not followed by an
    // ASCII word character.
    let env = lower.match_indices(".env").any(|(i, _)| {
        !lower
            .as_bytes()
            .get(i + 4)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
    });
    if env {
        return Some(".env".into());
    }
    APP_REFUSED
        .iter()
        .find(|parts| in_one_line(&lower, parts))
        .map(|parts| parts.join(".*"))
}

/// The link that opens a chat with `prompt` filled in, in whichever Cursor
/// window is in front, or why Cursor would not take `prompt` as it is.
pub fn app_link(prompt: &str) -> Result<String, String> {
    let text = crate::roles::encode(prompt);
    let len = APP_LINK_OVERHEAD + text.len();
    if len > APP_LINK_MAX {
        return Err(crate::tr!(
            "prompt 太长（编码后 {len}，Cursor 链接最多 {APP_LINK_MAX}）",
            "the prompt is too long for a Cursor link ({len} encoded, at most {APP_LINK_MAX})"
        ));
    }
    if let Some(what) = app_refused(prompt) {
        return Err(crate::tr!(
            "prompt 里有 Cursor 会拒收的内容（{what}）",
            "the prompt has content Cursor refuses ({what})"
        ));
    }
    // Cursor decodes the query once whole, cuts it at every `&name=`, turns
    // `+` into spaces and decodes again: encoded twice, the first pass leaves
    // no `&` or `+` to act on and the second gives the prompt back, so a
    // relay that quotes `C++` or `x&y=1` arrives as written. No
    // `workspace=`: the Agents window, where the link goes, ignores it.
    Ok(format!(
        "cursor://anysphere.cursor-deeplink/prompt?text={}",
        crate::roles::encode(&text)
    ))
}

/// Whether the Cursor app is running: a cold start needs far longer before
/// its window takes a link.
fn app_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", "Cursor"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The Agents window's title (Cursor 3.29.9; re-check after an upgrade).
const AGENTS_WINDOW_TITLE: &str = "Cursor Agents";

/// The link that opens Cursor's Agents window, or brings it to the front.
const AGENTS_WINDOW_LINK: &str = "cursor://anysphere.cursor-deeplink/glass";

// Cursor 3.29.9 runs the app's own agents in its extension host (`ps` shows
// it retitled, "Cursor Helper (Plugin): extension-host …", no path) with a
// worker CLI it keeps under its globalStorage. A `cursor-agent` started from
// a terminal, Cursor's own included, is another install that runs its hooks
// itself, and its chats are unknown to the Agents window ("We can't find
// this agent").
const APP_WORKER: &str = "/anysphere.cursor-agent-worker/";
const APP_HELPER: &str = "Cursor Helper";

/// Whether the process that ran a Cursor hook, `exe` as `ps` names it, is
/// the Cursor app's, so its chat is in the Agents window rather than in a
/// terminal.
pub fn run_by_app(exe: &str) -> bool {
    exe.contains(APP_WORKER)
        || exe
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with(APP_HELPER))
}

/// The link that shows chat `id` in Cursor's Agents window: activating the
/// app alone brings back whichever window was last used, often an editor.
pub fn agent_link(id: &str) -> String {
    format!(
        "cursor://anysphere.cursor-deeplink/agent?id={}",
        crate::roles::encode(id)
    )
}

/// Whether a prompt link sent now would land in the Agents window, given
/// Cursor's on-screen window titles front first (`None`: no title yet).
/// Cursor 3.29.9 hands a deeplink to its focused window, else its last
/// active one, if that window has loaded; an editor window in front would
/// take the chat into its side panel instead.
fn agents_in_front(titles: &[Option<String>]) -> bool {
    matches!(titles.first(), Some(Some(t)) if t.starts_with(AGENTS_WINDOW_TITLE))
}

// Waits measured by hand on Cursor 3.29.9, not guarantees.
/// How often Cursor's windows are looked at while waiting.
const APP_POLL: std::time::Duration = std::time::Duration::from_millis(200);
/// How long a running Cursor may take to bring the Agents window up in
/// front; a cold one also has to start, and may restore other windows.
const APP_WARM_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);
const APP_COLD_LIMIT: std::time::Duration = std::time::Duration::from_secs(40);
/// How long the Agents window must stay in front before the link goes: a
/// window that just came up loads its page (the link's handler) a moment
/// after its title shows, and a window already open must show it holds the
/// front after being asked to, not before it.
const APP_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

/// The wait for the Agents window, poll by poll.
struct Waiting {
    start: std::time::Instant,
    limit: std::time::Duration,
    polls: u32,
    /// Since when the Agents window has been in front without a break.
    since: Option<std::time::Instant>,
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    Send,
    Timeout,
}

/// One poll of the wait. A wrong call here must come out as not sending,
/// never as sending: a link an editor window takes opens the chat in its
/// side panel, while not sending only fails the dispatch with a reason. So
/// the first poll never counts (it may show the windows as they were before
/// the window was asked for), and the Agents window must hold the front for
/// [`APP_SETTLE`] with the clock restarting whenever anything else is in
/// front.
fn step(w: &mut Waiting, titles: &[Option<String>], now: std::time::Instant) -> Step {
    let first = w.polls == 0;
    w.polls += 1;
    if !first && agents_in_front(titles) {
        if now.duration_since(*w.since.get_or_insert(now)) >= APP_SETTLE {
            return Step::Send;
        }
    } else {
        w.since = None;
    }
    if now.duration_since(w.start) >= w.limit {
        Step::Timeout
    } else {
        Step::Wait
    }
}

/// Bring Cursor's Agents window to the front, wait until it holds it, then
/// send `link` to it: the chat it creates is prefilled, for the user to
/// pick the repository and send. When the landing cannot be vouched for,
/// nothing is sent and the error says why.
pub fn open_in_app(link: &str) -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "macos"),
        crate::i18n::t(
            "只有 macOS 能替你打开 Cursor",
            "only macOS can open Cursor for you"
        )
    );
    // Asking for the permission would pop a system prompt mid-dispatch;
    // the reason says where to grant it instead.
    anyhow::ensure!(
        front::titles().is_some(),
        crate::i18n::t(
            "agent-office 没有屏幕录制权限，看不到 Cursor 的窗口标题，没法确认 prompt 会落在 Agents 窗口（系统设置 → 隐私与安全性 → 屏幕录制，把 agent-office 加进去）",
            "agent-office has no Screen Recording permission, so it cannot see Cursor's window titles to make sure the prompt lands in the Agents window (System Settings → Privacy & Security → Screen Recording: add agent-office)",
        )
    );
    let limit = if app_running() {
        APP_WARM_LIMIT
    } else {
        APP_COLD_LIMIT
    };
    run_open(std::process::Command::new("open").arg(AGENTS_WINDOW_LINK)).map_err(|why| {
        anyhow::anyhow!(crate::tr!(
            "没能打开 Cursor 的 Agents 窗口（{why}）",
            "could not open Cursor's Agents window ({why})"
        ))
    })?;
    let mut w = Waiting {
        start: std::time::Instant::now(),
        limit,
        polls: 0,
        since: None,
    };
    // The window can still change between the last look and the link: this
    // narrows the race to a few milliseconds, it does not close it.
    loop {
        let titles = front::titles().unwrap_or_default();
        match step(&mut w, &titles, std::time::Instant::now()) {
            Step::Send => break,
            Step::Wait => std::thread::sleep(APP_POLL),
            Step::Timeout => anyhow::bail!(crate::tr!(
                "等了 {} 秒，Cursor 的 Agents 窗口没到最前，没法确认 prompt 会落在那里",
                "after {} s Cursor's Agents window was not in front, so the prompt could land elsewhere",
                limit.as_secs()
            )),
        }
    }
    run_open(std::process::Command::new("open").arg(link)).map_err(|why| {
        anyhow::anyhow!(crate::tr!(
            "Cursor 没接住链接（{why}）",
            "Cursor did not take the link ({why})"
        ))
    })
}

/// Run an `open` that should hand off at once: `Err` with why when it fails
/// within a few seconds. Its output is dropped, as it may echo the prompt.
fn run_open(cmd: &mut std::process::Command) -> Result<(), String> {
    let opened = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|child| crate::jump::exit_within(child, std::time::Duration::from_secs(3)));
    match opened {
        // Still running: Cursor took it and is starting up.
        Ok(None) => Ok(()),
        Ok(Some(s)) if s.success() => Ok(()),
        Ok(Some(s)) => Err(s.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Cursor's windows as the window server lists them.
#[cfg(target_os = "macos")]
mod front {
    use std::ffi::{c_char, c_void};

    type Ref = *const c_void;

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Ref;
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRectMakeWithDictionaryRepresentation(dict: Ref, rect: *mut Rect) -> bool;
        static kCGWindowOwnerName: Ref;
        static kCGWindowName: Ref;
        static kCGWindowLayer: Ref;
        static kCGWindowBounds: Ref;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: Ref) -> isize;
        fn CFArrayGetValueAtIndex(array: Ref, i: isize) -> Ref;
        fn CFDictionaryGetValue(dict: Ref, key: Ref) -> Ref;
        fn CFStringGetCString(s: Ref, buf: *mut c_char, len: isize, encoding: u32) -> u8;
        fn CFNumberGetValue(n: Ref, kind: isize, out: *mut c_void) -> u8;
        fn CFGetTypeID(r: Ref) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFRelease(r: Ref);
    }

    const ON_SCREEN_ONLY: u32 = 1 << 0;
    const EXCLUDE_DESKTOP: u32 = 1 << 4;
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;
    /// Smaller ones are tooltips and popups, never what a link lands in.
    const MIN_SIDE: f64 = 200.0;

    unsafe fn string(r: Ref) -> Option<String> {
        if r.is_null() || unsafe { CFGetTypeID(r) != CFStringGetTypeID() } {
            return None;
        }
        let mut buf = vec![0 as c_char; 4096];
        (unsafe { CFStringGetCString(r, buf.as_mut_ptr(), buf.len() as isize, UTF8) } != 0).then(
            || {
                unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            },
        )
    }

    unsafe fn number(r: Ref) -> Option<i64> {
        if r.is_null() || unsafe { CFGetTypeID(r) != CFNumberGetTypeID() } {
            return None;
        }
        let mut n = 0i64;
        (unsafe { CFNumberGetValue(r, SINT64, (&raw mut n).cast()) } != 0).then_some(n)
    }

    /// The titles of Cursor's on-screen windows, front first (`None` for
    /// one with no title yet); `None` when titles cannot be seen at all:
    /// without Screen Recording permission macOS hides them.
    pub fn titles() -> Option<Vec<Option<String>>> {
        // SAFETY: CoreGraphics returns a CFArray of CFDictionaries we own
        // (Copy rule) and release; values read from them are borrowed and
        // type-checked before use.
        unsafe {
            if !CGPreflightScreenCaptureAccess() {
                return None;
            }
            let list = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
            if list.is_null() {
                return Some(Vec::new());
            }
            let mut out = Vec::new();
            for i in 0..CFArrayGetCount(list) {
                let w = CFArrayGetValueAtIndex(list, i);
                if string(CFDictionaryGetValue(w, kCGWindowOwnerName)).as_deref() != Some("Cursor")
                    || number(CFDictionaryGetValue(w, kCGWindowLayer)) != Some(0)
                {
                    continue;
                }
                let mut r = Rect::default();
                let bounds = CFDictionaryGetValue(w, kCGWindowBounds);
                if bounds.is_null()
                    || !CGRectMakeWithDictionaryRepresentation(bounds, &raw mut r)
                    || r.w < MIN_SIDE
                    || r.h < MIN_SIDE
                {
                    continue;
                }
                let title =
                    string(CFDictionaryGetValue(w, kCGWindowName)).filter(|t| !t.is_empty());
                out.push(title);
            }
            CFRelease(list);
            Some(out)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod front {
    pub fn titles() -> Option<Vec<Option<String>>> {
        None
    }
}

/// Where the forwarder writes one `<session>.jsonl` task log per session.
pub fn transcripts_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config/agent-office/cursor/transcripts"))
}

/// Cursor's on-disk projects, where a subagent's transcript lives under
/// `agent-transcripts/<parent>/subagents/<id>.jsonl`.
fn projects_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cursor/projects"))
}

/// How long a subagent index stays. A child that appears just after a walk
/// waits this out; an ended turn is re-read from the file on each sweep.
const SUBAGENT_INDEX_FOR: Duration = Duration::from_secs(2);

/// Parent session id and transcript path for one Cursor subagent.
#[derive(Clone)]
struct SubagentFile {
    parent_session: String,
    path: PathBuf,
}

/// Subagent transcripts last seen on disk. Rebuilt when it goes stale or a
/// live session is not in it yet: hooks often run before Cursor has written
/// the nested path, so the child is seated as its own root.
pub struct SubagentIndex {
    at: Option<Instant>,
    files: HashMap<String, SubagentFile>,
}

impl Default for SubagentIndex {
    fn default() -> Self {
        Self {
            at: None,
            files: HashMap::new(),
        }
    }
}

impl SubagentIndex {
    /// Link each Cursor subagent the index can see, and walk out one whose
    /// transcript has already ended. A lost `stop` otherwise leaves it
    /// Running, and five quiet minutes put it in 卡住 with nothing to answer.
    pub fn seat(
        &mut self,
        reducer: &mut pixtuoid_core::Reducer,
        scene: &mut pixtuoid_core::SceneState,
        now: SystemTime,
    ) {
        let Some(root) = projects_dir() else {
            return;
        };
        self.seat_under(reducer, scene, &root, now);
    }

    fn seat_under(
        &mut self,
        reducer: &mut pixtuoid_core::Reducer,
        scene: &mut pixtuoid_core::SceneState,
        projects: &Path,
        now: SystemTime,
    ) {
        if !scene.agents.values().any(needs_subagent_seat) {
            return;
        }
        // A root session is never in this index, so "missing" cannot be the
        // refresh: it would walk the projects tree on every sweep. A child
        // that appears just after a walk waits out the interval.
        if self.at.is_none_or(|t| t.elapsed() >= SUBAGENT_INDEX_FOR) {
            self.files = index_subagents(projects);
            self.at = Some(Instant::now());
        }
        apply_seats(reducer, scene, &self.files, now);
    }
}

fn needs_subagent_seat(slot: &pixtuoid_core::AgentSlot) -> bool {
    slot.exiting_at.is_none()
        && slot.source.as_ref() == CURSOR_SOURCE
        && (slot.parent_id.is_none()
            || matches!(
                slot.state,
                pixtuoid_core::state::ActivityState::Running { .. }
            ))
}

/// Every `<parent>/subagents/<id>.jsonl` under `projects`. A newer file wins
/// when the same id was written twice.
fn index_subagents(projects: &Path) -> HashMap<String, SubagentFile> {
    let mut out = HashMap::new();
    let Ok(projects) = std::fs::read_dir(projects) else {
        return out;
    };
    for project in projects.flatten() {
        let Ok(parents) = std::fs::read_dir(project.path().join("agent-transcripts")) else {
            continue;
        };
        for parent in parents.flatten() {
            let Ok(kind) = parent.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let Ok(parent_session) = parent.file_name().into_string() else {
                continue;
            };
            let Ok(kids) = std::fs::read_dir(parent.path().join("subagents")) else {
                continue;
            };
            for kid in kids.flatten() {
                let path = kid.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let newer = out
                    .get(id)
                    .is_none_or(|have: &SubagentFile| mtime(&path) > mtime(&have.path));
                if newer {
                    out.insert(
                        id.to_string(),
                        SubagentFile {
                            parent_session: parent_session.clone(),
                            path,
                        },
                    );
                }
            }
        }
    }
    out
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// `Some(aborted)` when `path`'s last record is `turn_ended`.
fn turn_ended(path: &Path) -> Option<bool> {
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut file = std::io::BufReader::new(file);
    let start = len.saturating_sub(8192);
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start)).ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut file, &mut text).ok()?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    if start > 0 {
        // The first line is a record cut in half by the seek.
        lines.next();
    }
    let last = lines.last()?;
    let v: Value = serde_json::from_str(last).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("turn_ended") {
        return None;
    }
    let status = v.get("status").and_then(Value::as_str).unwrap_or("");
    Some(matches!(
        status,
        "error" | "aborted" | "cancelled" | "canceled" | "failure"
    ))
}

fn apply_seats(
    reducer: &mut pixtuoid_core::Reducer,
    scene: &mut pixtuoid_core::SceneState,
    files: &HashMap<String, SubagentFile>,
    now: SystemTime,
) {
    use pixtuoid_core::{AgentEvent, AgentId, Transport};

    let due: Vec<(AgentId, String, PathBuf, bool)> = scene
        .agents
        .values()
        .filter(|slot| needs_subagent_seat(slot))
        .filter_map(|slot| {
            let file = files.get(slot.session_id.as_ref())?;
            Some((
                slot.agent_id,
                file.parent_session.clone(),
                file.path.clone(),
                slot.parent_id.is_none(),
            ))
        })
        .collect();
    for (id, parent_session, path, unlinked) in due {
        if unlinked {
            let cwd = scene
                .agents
                .get(&id)
                .map(|s| s.cwd.as_ref().to_path_buf())
                .unwrap_or_default();
            let session_id = scene
                .agents
                .get(&id)
                .map(|s| s.session_id.to_string())
                .unwrap_or_default();
            reducer.apply(
                scene,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: CURSOR_SOURCE.to_string(),
                    session_id,
                    cwd,
                    parent_id: Some(AgentId::from_parts(CURSOR_SOURCE, &parent_session)),
                },
                now,
                Transport::Hook,
            );
        }
        let Some(aborted) = turn_ended(&path) else {
            continue;
        };
        if scene
            .agents
            .get(&id)
            .is_some_and(|s| s.exiting_at.is_some())
        {
            continue;
        }
        reducer.apply(
            scene,
            AgentEvent::TurnEnd {
                agent_id: id,
                turn: None,
                aborted,
            },
            now,
            Transport::Hook,
        );
    }
}

fn is_ours(command: &str) -> bool {
    command.contains("agent-office")
        && command.ends_with(&format!(" hook --source {CURSOR_SOURCE}"))
}

/// Drop our entries from every event, and the events only ours were under.
/// Cursor's schema is flat: `{"version": 1, "hooks": {<event>: [{"command"}]}}`.
fn remove_ours(doc: &mut Value) {
    let Some(hooks) = doc.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    let mut emptied = Vec::new();
    for (event, entries) in hooks.iter_mut() {
        let Some(entries) = entries.as_array_mut() else {
            continue;
        };
        let before = entries.len();
        entries.retain(|e| !e["command"].as_str().is_some_and(is_ours));
        if entries.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        hooks.shift_remove(&event);
    }
}

fn add_ours(doc: &mut Value, command: &str) -> Result<()> {
    let root = doc.as_object_mut().context(crate::i18n::t(
        "hooks.json 顶层不是对象",
        "hooks.json is not an object at its top level",
    ))?;
    // Cursor loads no hooks from a file without a version; a user's own stays.
    root.entry("version").or_insert_with(|| json!(1));
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context(crate::i18n::t(
            "hooks.json 的 hooks 不是对象",
            "hooks in hooks.json is not an object",
        ))?;
    for event in EVENTS {
        hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| crate::tr!("hooks.{event} 不是数组", "hooks.{event} is not an array"))?
            .push(json!({ "command": command }));
    }
    Ok(())
}

fn has_every_event(doc: &Value) -> bool {
    EVENTS.iter().all(|e| {
        doc["hooks"][e].as_array().is_some_and(|es| {
            es.iter()
                .any(|x| x["command"].as_str().is_some_and(is_ours))
        })
    })
}

fn read_hooks(path: &Path) -> Result<(Option<String>, Value)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(e)
                .with_context(|| crate::tr!("读 {} 失败", "could not read {}", path.display()));
        }
    };
    let v = serde_json::from_str(raw.as_deref().unwrap_or("{}"))
        .with_context(|| crate::tr!("{} 不是合法 JSON", "{} is not valid JSON", path.display()))?;
    Ok((raw, v))
}

fn write_hooks(path: &Path, before: Option<&str>, doc: &Value) -> Result<()> {
    if let Some(before) = before {
        let backup = path.with_extension("json.agent-office.bak");
        let tmp = path.with_extension("json.agent-office.bak.tmp");
        crate::atomic::write_private(&backup, &tmp, before.as_bytes()).with_context(|| {
            crate::tr!(
                "备份到 {} 失败",
                "could not back up to {}",
                backup.display()
            )
        })?;
        println!(
            "{}",
            crate::tr!(
                "原文件备份在 {}",
                "Backed up the original to {}",
                backup.display()
            )
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| crate::tr!("建 {} 失败", "could not create {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.agent-office.tmp");
    crate::atomic::write_keeping_mode(
        path,
        &tmp,
        (serde_json::to_string_pretty(doc)? + "\n").as_bytes(),
    )
    .with_context(|| crate::tr!("写 {} 失败", "could not write {}", path.display()))?;
    println!("{}", crate::tr!("已更新 {}", "Updated {}", path.display()));
    Ok(())
}

/// `agent-office connect cursor`.
pub fn connect() -> Result<()> {
    let path = hooks_path().context(crate::i18n::t(
        "找不到 Cursor 配置目录",
        "cannot find the Cursor config directory",
    ))?;
    let exe = crate::hook::exe_for_hooks()?;
    let command = format!(
        "{} hook --source {CURSOR_SOURCE}",
        sh_quote(&exe.display().to_string())
    );
    let (raw, mut doc) = read_hooks(&path)?;
    remove_ours(&mut doc);
    add_ours(&mut doc, &command)?;
    write_hooks(&path, raw.as_deref(), &doc)?;
    println!(
        "{}",
        crate::tr!(
            "Cursor 的 hooks 指向 {}；新开的 cursor-agent 才生效。移动或删除这个程序后要重新 connect cursor。",
            "Cursor hooks now point to {}; they apply to new cursor-agent runs. Run connect cursor again after moving or deleting this program.",
            exe.display()
        )
    );
    Ok(())
}

/// `agent-office disconnect cursor`. The task logs stay: they are the
/// user's sessions, not ours to drop.
pub fn disconnect() -> Result<()> {
    let path = hooks_path().context(crate::i18n::t(
        "找不到 Cursor 配置目录",
        "cannot find the Cursor config directory",
    ))?;
    let (raw, mut doc) = read_hooks(&path)?;
    if raw.is_none() {
        return Ok(());
    }
    remove_ours(&mut doc);
    write_hooks(&path, raw.as_deref(), &doc)
}

pub fn installed() -> bool {
    hooks_path()
        .and_then(|p| read_hooks(&p).ok())
        .is_some_and(|(_, doc)| has_every_event(&doc))
}

/// The task log line a hook payload makes, if any: the prompt as a user
/// turn, a call as a `tool_use`, its end as a `tool_result`, in the subset of
/// Claude Code's transcript shape the task reader parses.
fn log_line(v: &Value, now: &str) -> Option<Value> {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    let id = s("tool_use_id").filter(|i| !i.is_empty());
    Some(match s("hook_event_name")? {
        PROMPT_EVENT => {
            let prompt = s("prompt").filter(|p| !p.trim().is_empty())?;
            json!({
                "type": "user",
                "timestamp": now,
                "message": { "content": truncate(prompt, 4000) },
            })
        }
        REPLY_EVENT => {
            let text = s("text").filter(|t| !t.trim().is_empty())?;
            let n = text.chars().count();
            let tail: String = text.chars().skip(n.saturating_sub(3000)).collect();
            json!({
                "type": "assistant",
                "message": { "content": [{ "type": "text", "text": tail }] },
            })
        }
        "preToolUse" => {
            let mut call = json!({
                "type": "tool_use",
                "name": s("tool_name").unwrap_or("?"),
                "input": slim(v.get("tool_input")),
            });
            if let Some(id) = id {
                call["id"] = id.into();
            }
            json!({ "type": "assistant", "message": { "content": [call] } })
        }
        e @ ("postToolUse" | "postToolUseFailure") => {
            let failed = e == "postToolUseFailure";
            let mut result = json!({ "type": "tool_result", "is_error": failed });
            if let Some(id) = id {
                result["tool_use_id"] = id.into();
            }
            // A call the user turned down is their say, not the agent failing.
            if failed && s("failure_type") == Some("permission_denied") {
                result["rejected"] = true.into();
            }
            json!({ "type": "user", "message": { "content": [result] } })
        }
        _ => return None,
    })
}

fn truncate(s: &str, chars: usize) -> String {
    s.chars().take(chars).collect()
}

fn slim(input: Option<&Value>) -> Value {
    let mut out = Map::new();
    let Some(args) = input.and_then(Value::as_object) else {
        return Value::Object(out);
    };
    for k in KEEP_ARGS {
        match args.get(k) {
            Some(Value::Array(todos)) if k == "todos" => {
                let todos: Vec<Value> = todos
                    .iter()
                    .map(|t| json!({ "content": t["content"].as_str().unwrap_or(""), "status": t["status"] }))
                    .collect();
                out.insert(k.into(), todos.into());
            }
            Some(Value::String(v)) => {
                out.insert(k.into(), truncate(v, 300).into());
            }
            _ => {}
        }
    }
    Value::Object(out)
}

/// The session a payload belongs to: `session_id`, which pixtuoid's decoder
/// keys on, else Cursor's `conversation_id`.
fn session_of(v: &Value) -> Option<&str> {
    ["session_id", "conversation_id"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
}

/// Append what `v` says to its session's task log. Best-effort: a hook must
/// never fail Cursor's call.
pub fn record(v: &Value) {
    let (Some(dir), Some(session)) = (transcripts_dir(), session_of(v)) else {
        return;
    };
    record_in(&dir, session, v, &now_rfc3339());
}

fn record_in(dir: &Path, session: &str, v: &Value, now: &str) -> Option<()> {
    let line = log_line(v, now)?;
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir).ok()?;
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(crate::tasks::task_log(dir, session)).ok()?;
    std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()).ok()
}

/// Now as `YYYY-MM-DDTHH:MM:SSZ`, how the task reader reads a turn's start.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the app's own agents go to the Agents window: a terminal
    /// `cursor-agent` chat sent there is "not found" and the click lands
    /// nowhere, while the terminal jump would have found it.
    #[test]
    fn only_the_apps_own_agents_open_in_the_agents_window() {
        let home = "/Users/u";
        let app = format!(
            "{home}/Library/Application Support/Cursor/User/globalStorage/anysphere.cursor-agent-worker/agent-cli/.local/bin/cursor-agent"
        );
        assert!(run_by_app(&app));
        assert!(run_by_app(
            "Cursor Helper (Plugin): extension-host agent-office [2-6]"
        ));
        assert!(run_by_app(
            "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Plugin).app/Contents/MacOS/Cursor Helper (Plugin)"
        ));
        let cli =
            format!("{home}/.local/share/cursor-agent/versions/2026.11.05-86047df/cursor-agent");
        assert!(!run_by_app(&cli));
        assert!(!run_by_app("/opt/homebrew/bin/cursor-agent"));
        assert_eq!(
            agent_link("a6843c42-8a57-4cee-8f30-ea04ab2e9738"),
            "cursor://anysphere.cursor-deeplink/agent?id=a6843c42-8a57-4cee-8f30-ea04ab2e9738"
        );
        assert!(
            agent_link("x&y=1").ends_with("?id=x%26y%3D1"),
            "an id cannot add a parameter"
        );
    }

    /// The user's own hooks, their `version` and every other key must come
    /// through untouched, and reconnecting must not stack a second copy.
    #[test]
    fn connect_twice_then_disconnect_leaves_other_hooks_alone() {
        let original = json!({"version": 2, "hooks": {"preToolUse": [{"command": "guard.sh"}]}});
        let cmd = "'/usr/local/bin/agent-office' hook --source cursor";
        let mut doc = original.clone();
        for _ in 0..2 {
            remove_ours(&mut doc);
            add_ours(&mut doc, cmd).unwrap();
        }
        assert!(has_every_event(&doc));
        assert_eq!(doc["version"], 2);
        assert_eq!(doc["hooks"]["preToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(doc["hooks"]["sessionStart"][0]["command"], cmd);
        remove_ours(&mut doc);
        assert_eq!(doc, original);
        // Cursor loads nothing from a file without a version.
        let mut fresh = json!({});
        add_ours(&mut fresh, cmd).unwrap();
        assert_eq!(fresh["version"], 1);
    }

    /// The forwarder's payload must decode as Cursor's, or the router reads
    /// it as Claude Code's and keys the figure under the wrong CLI.
    #[test]
    fn a_stamped_cursor_payload_decodes_as_cursor() {
        let v = json!({
            "hook_event_name": "preToolUse", "session_id": "c7cef226",
            "workspace_roots": ["/repo"], "tool_name": "Shell",
            "tool_input": {"command": "cargo test"}, "tool_use_id": "t1",
            pixtuoid_core::source::decoder::SOURCE_STAMP: CURSOR_SOURCE,
        });
        let evs = pixtuoid_core::source::decoder::decode_hook_payload(v).unwrap();
        assert!(
            evs.iter()
                .any(|e| e.agent_id()
                    == pixtuoid_core::AgentId::from_parts(CURSOR_SOURCE, "c7cef226")),
            "{evs:?}"
        );
    }

    /// The task log is what a Cursor figure's task line, phase and stuck
    /// verdict come from: it must read back as the prompt, the call and its
    /// failure, and hold no file contents.
    #[test]
    fn the_task_log_reads_back_as_the_prompt_the_calls_and_their_failures() {
        let dir = std::env::temp_dir().join(format!("ao-cursor-log-{}", std::process::id()));
        let now = "2026-11-05T09:30:15Z";
        let ev = |e: &str, extra: Value| {
            let mut v = json!({"hook_event_name": e, "session_id": "s/1"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let events = [
            ev(PROMPT_EVENT, json!({"prompt": "给登录页补测试"})),
            ev(REPLY_EVENT, json!({"text": "先看一下现有的测试"})),
            ev(
                "preToolUse",
                json!({"tool_name": "Write", "tool_use_id": "t1",
                    "tool_input": {"file_path": "/repo/src/login.test.ts", "contents": "SECRET"}}),
            ),
            ev(
                "postToolUseFailure",
                json!({"tool_use_id": "t1", "failure_type": "error"}),
            ),
            ev(
                "postToolUseFailure",
                json!({"tool_use_id": "t2", "failure_type": "error"}),
            ),
            ev(
                "postToolUseFailure",
                json!({"tool_use_id": "t3", "failure_type": "permission_denied"}),
            ),
            ev("stop", json!({})),
        ];
        for v in &events {
            record_in(&dir, session_of(v).unwrap(), v, now);
        }
        let log = crate::tasks::task_log(&dir, "s/1");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(!text.contains("SECRET"), "no file contents in the log");
        let mut t = crate::tasks::extract::Transcript::default();
        crate::tasks::extract::Tail::new(log).poll(&mut t).unwrap();
        assert_eq!(t.latest_prompt(), Some("给登录页补测试"));
        assert!(t.turn_started.is_some());
        assert_eq!(t.phase, Some(crate::tasks::Phase::Code));
        assert_eq!(
            t.recent_tools.back().map(String::as_str),
            Some("Write login.test.ts")
        );
        assert_eq!(t.fail_streak, 2, "a call turned down is not a failure");
        assert_eq!(t.last_reply.as_deref(), Some("先看一下现有的测试"));
        std::fs::remove_dir_all(dir).ok();
    }

    /// A dispatched Cursor prompt is read back from the task log like any
    /// other: the figure must wear its role and show its task, and a long
    /// relay cut to the log's 4000 characters must still wear its role,
    /// since the marker leads.
    #[test]
    fn a_dispatched_prompt_reads_back_from_the_task_log() {
        use crate::roles::{Role, cursor_prompt};
        let now = "2026-11-05T09:30:15Z";
        let read_back = |p: String| {
            let v = json!({"hook_event_name": PROMPT_EVENT, "session_id": "s1", "prompt": p});
            let mut t = crate::tasks::extract::Transcript::default();
            t.feed(&log_line(&v, now).unwrap().to_string());
            t
        };
        let t = read_back(cursor_prompt(Role::Backend, "给登录页补测试"));
        assert_eq!(t.role, Some(Role::Backend));
        assert_eq!(t.latest_prompt(), Some("给登录页补测试"));
        let long = cursor_prompt(Role::Pm, &"审".repeat(5000));
        assert!(long.chars().count() > 4000);
        let t = read_back(long);
        assert_eq!(t.role, Some(Role::Pm));
        assert!(
            t.latest_prompt().is_some_and(|p| p.starts_with('审')),
            "{:?}",
            t.latest_prompt()
        );
    }

    #[test]
    fn now_is_written_the_way_the_reader_parses_it() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_793_871_015), "2026-11-05T09:30:15Z");
    }

    /// `%hh` runs decoded as UTF-8; `None` when a run is not UTF-8, which
    /// is where `decodeURIComponent` throws.
    fn pct_decode(s: &str) -> Option<String> {
        let b = s.as_bytes();
        let (mut out, mut i) = (Vec::new(), 0);
        while i < b.len() {
            let hex = |c: u8| char::from(c).to_digit(16);
            match (
                b[i],
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            ) {
                (b'%', Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                }
                (c, ..) => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).ok()
    }

    /// A query parameter as Cursor 3.29.9 reads it: the URI's query decoded
    /// once whole, the value cut at the next `&name=`, then `+` made a space
    /// and decoded again (left as it is where that throws).
    fn cursor_param(link: &str, key: &str) -> Option<String> {
        let query = pct_decode(link.split_once('?')?.1)?;
        let start = if query.starts_with(&format!("{key}=")) {
            key.len() + 1
        } else {
            query.find(&format!("&{key}="))? + key.len() + 2
        };
        let rest = &query[start..];
        let b = rest.as_bytes();
        let end = (0..b.len())
            .find(|&i| {
                b[i] == b'&'
                    && b.get(i + 1).is_some_and(u8::is_ascii_alphabetic)
                    && b[i + 2..]
                        .iter()
                        .find(|c| !(c.is_ascii_alphanumeric() || **c == b'_' || **c == b'-'))
                        == Some(&b'=')
            })
            .unwrap_or(b.len());
        let raw = rest[..end].replace('+', " ");
        Some(pct_decode(&raw).unwrap_or(raw))
    }

    /// If Cursor read back anything but the prompt sent, the draft would
    /// carry a role card cut or changed mid-sentence with nothing saying so.
    /// Every role's fresh prompt, in both languages and with the folder line
    /// the app path adds, must come through whole.
    #[test]
    fn every_fresh_role_prompt_reads_back_whole_in_the_app() {
        use crate::i18n::{Lang, with};
        let dir = Path::new("/work/shop");
        for lang in [Lang::Zh, Lang::En] {
            with(lang, || {
                for post in crate::roles::ROSTER {
                    let prompt = crate::roles::cursor_app_prompt(post.role, "", dir);
                    let link = app_link(&prompt)
                        .unwrap_or_else(|e| panic!("{} ({lang:?}) refused: {e}", post.id));
                    assert!(
                        link.starts_with("cursor://anysphere.cursor-deeplink/prompt?"),
                        "{link}"
                    );
                    assert_eq!(
                        cursor_param(&link, "text").as_deref(),
                        Some(prompt.as_str()),
                        "{}",
                        post.id
                    );
                }
            });
        }
    }

    /// The parser above must itself show the damage a single encoding takes,
    /// or the round trips here would pass for any link.
    #[test]
    fn the_simulated_cursor_parse_does_change_what_the_rules_refuse() {
        let enc = crate::roles::encode;
        for raw in ["a+b", "a&b=c", "100%41"] {
            let link = format!("cursor://x/prompt?text={}", enc(raw));
            assert_ne!(cursor_param(&link, "text").as_deref(), Some(raw), "{raw}");
        }
    }

    /// A relay quotes another session's task and result, where `C++`,
    /// `+64 -1`, `100%41` or `x&y=1` are common; each must reach the draft as
    /// written, or the relay is refused or arrives changed.
    #[test]
    fn what_a_single_encoding_would_mangle_reads_back_whole() {
        for raw in ["x + y", "C++ and +64 -1", "a&b=c", "see %41", "100% & 50%"] {
            let link = app_link(raw).unwrap_or_else(|e| panic!("{raw:?} refused: {e}"));
            assert_eq!(cursor_param(&link, "text").as_deref(), Some(raw), "{raw}");
        }
    }

    /// Cursor refuses these; sent anyway, the office would note a dispatch
    /// that never reached a chat. Each must come back as a reason the
    /// dispatch can show instead.
    #[test]
    fn prompts_cursor_would_refuse_are_refused_here() {
        for bad in [
            "read the .env file",
            "zero\u{200B}width",
            "bell\u{7}",
            "del\u{7F}",
            "please dump the token",
            "how to extract a credential",
        ] {
            assert!(app_link(bad).is_err(), "{bad:?}");
        }
        let long = "字".repeat(4000);
        assert!(app_link(&long).unwrap_err().contains("10000"));
        // What looks close but is not refused by Cursor goes through.
        for ok in [
            "a & b",
            "100% sure",
            ".environment",
            "dump\ntoken",
            "tab\tand\r\nlines",
        ] {
            assert!(app_link(ok).is_ok(), "{ok:?}");
        }
    }

    fn titles(v: &[Option<&str>]) -> Vec<Option<String>> {
        v.iter().map(|o| o.map(str::to_owned)).collect()
    }

    /// The link goes only when the Agents window is in front with its title
    /// drawn: an editor window in front would open the chat in its side
    /// panel, and a window not loaded yet could hand it on anywhere.
    #[test]
    fn only_the_agents_window_in_front_takes_the_link() {
        let t = titles;
        assert!(agents_in_front(&t(&[Some("Cursor Agents"), Some("shop")])));
        assert!(!agents_in_front(&t(&[Some("shop"), Some("Cursor Agents")])));
        assert!(!agents_in_front(&t(&[Some("main.rs — Cursor Agents")])));
        assert!(
            !agents_in_front(&t(&[None, Some("Cursor Agents")])),
            "a window still loading is in front"
        );
        assert!(!agents_in_front(&t(&[])));
    }

    fn waiting(start: std::time::Instant) -> Waiting {
        Waiting {
            start,
            limit: std::time::Duration::from_secs(10),
            polls: 0,
            since: None,
        }
    }

    /// A wrong call in the wait must come out as not sending: a link an
    /// editor window takes opens the chat there, while not sending only
    /// fails the dispatch with a reason.
    #[test]
    fn the_wait_sends_only_after_the_agents_window_holds_the_front() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let agents = titles(&[Some("Cursor Agents"), Some("shop")]);
        let shop = titles(&[Some("shop"), Some("Cursor Agents")]);

        // The first look may be the windows as they were before the Agents
        // window was asked for: however long it is then held, the clock
        // starts after it.
        let mut w = waiting(t0);
        assert_eq!(step(&mut w, &agents, at(0)), Step::Wait);
        assert_eq!(step(&mut w, &agents, at(200)), Step::Wait);
        assert_eq!(
            step(&mut w, &agents, at(2100)),
            Step::Wait,
            "2 s from the second look, not the first"
        );
        assert_eq!(step(&mut w, &agents, at(2200)), Step::Send);

        // An editor window taking the front restarts the clock.
        let mut w = waiting(t0);
        assert_eq!(step(&mut w, &agents, at(0)), Step::Wait);
        assert_eq!(step(&mut w, &agents, at(200)), Step::Wait);
        assert_eq!(step(&mut w, &shop, at(1800)), Step::Wait);
        assert_eq!(step(&mut w, &agents, at(2200)), Step::Wait);
        assert_eq!(step(&mut w, &agents, at(4100)), Step::Wait);
        assert_eq!(step(&mut w, &agents, at(4200)), Step::Send);

        // Never in front: gives up at the limit rather than sending.
        let mut w = waiting(t0);
        assert_eq!(step(&mut w, &shop, at(0)), Step::Wait);
        assert_eq!(step(&mut w, &shop, at(9900)), Step::Wait);
        assert_eq!(step(&mut w, &shop, at(10_000)), Step::Timeout);
    }

    /// A subagent hook often runs before Cursor has a nested transcript path,
    /// and its `stop` may never arrive. The office then seats it as a root
    /// that is still "working", and five quiet minutes put it in 卡住 with
    /// no prompt to answer. The transcript on disk is the parent and the end.
    #[test]
    fn a_subagent_whose_stop_never_arrived_nests_then_leaves_when_its_transcript_ends() {
        use pixtuoid_core::source::decoder::{SOURCE_STAMP, decode_hook_payload};
        use pixtuoid_core::state::ActivityState;
        use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
        use serde_json::json;
        use std::sync::Arc;
        use std::time::SystemTime;

        let root = std::env::temp_dir().join(format!("ao-cu-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let parent_sid = "parent-sess";
        let child_sid = "child-sess";
        let other_sid = "root-sess";
        let transcript = root
            .join("proj")
            .join("agent-transcripts")
            .join(parent_sid)
            .join("subagents")
            .join(format!("{child_sid}.jsonl"));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "{\"role\":\"assistant\",\"message\":{\"content\":[]}}\n",
        )
        .unwrap();

        let parent = AgentId::from_parts(CURSOR_SOURCE, parent_sid);
        let child = AgentId::from_parts(CURSOR_SOURCE, child_sid);
        let other = AgentId::from_parts(CURSOR_SOURCE, other_sid);
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let feed = |reducer: &mut Reducer, scene: &mut SceneState, v: serde_json::Value| {
            for ev in decode_hook_payload(v).unwrap() {
                reducer.apply(scene, ev, now, Transport::Hook);
            }
        };
        let hook = |event: &str, sid: &str| {
            json!({
                "hook_event_name": event,
                "session_id": sid,
                "cwd": "",
                "workspace_roots": ["/repo/myclaw"],
                "tool_name": "Read",
                "tool_use_id": "c1",
                "tool_input": {"file_path": "/repo/a.rs"},
                SOURCE_STAMP: CURSOR_SOURCE,
            })
        };
        feed(&mut reducer, &mut scene, hook("sessionStart", parent_sid));
        feed(&mut reducer, &mut scene, hook("preToolUse", child_sid));
        feed(&mut reducer, &mut scene, hook("postToolUse", child_sid));
        feed(&mut reducer, &mut scene, hook("preToolUse", other_sid));
        assert!(
            scene.agents[&child].parent_id.is_none(),
            "no nested path on the hook, so the child is a root"
        );
        assert!(matches!(
            scene.agents[&child].state,
            ActivityState::Running { .. }
        ));

        let mut index = SubagentIndex::default();
        index.seat_under(&mut reducer, &mut scene, &root, now);
        assert_eq!(scene.agents[&child].parent_id, Some(parent));
        assert!(scene.agents[&child].exiting_at.is_none());
        assert!(
            scene.agents[&other].parent_id.is_none() && scene.agents[&other].exiting_at.is_none(),
            "a root session is not a subagent file"
        );
        let views = Arc::new(std::collections::HashMap::from([(
            child,
            crate::tasks::TaskView {
                stuck: Some(crate::tasks::Stuck::Quiet(12)),
                ..crate::tasks::TaskView::default()
            },
        )]));
        let order = crate::panel::order(&scene, &views, now, &std::collections::HashMap::new());
        assert_eq!(
            order.iter().find(|(id, _)| *id == child).map(|(_, d)| *d),
            Some(1),
            "it sits under its parent, not in 卡住 on its own"
        );
        assert!(
            order.iter().filter(|(_, d)| *d == 0).all(|(id, _)| {
                crate::panel::Group::of(&scene.agents[id], views.get(id), now, false)
                    != crate::panel::Group::Stuck
            }),
            "no root is stuck"
        );

        std::fs::write(
            &transcript,
            "{\"role\":\"assistant\",\"message\":{\"content\":[]}}\n{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
        )
        .unwrap();
        index.seat_under(&mut reducer, &mut scene, &root, now);
        assert_eq!(
            scene.agents[&child].exiting_at,
            Some(now),
            "turn_ended is the stop that never arrived"
        );
        assert!(scene.agents[&parent].exiting_at.is_none());
        assert!(
            crate::tasks::stuck_at_work(&scene.agents[&child], views.get(&child)).is_none(),
            "a child already leaving is not something to handle"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
