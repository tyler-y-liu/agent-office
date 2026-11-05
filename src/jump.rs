//! Back to where an agent runs: the Claude desktop app opens a session it
//! hosts from a `claude://code/continue?session=local_…` link. Which host id
//! a session has is only in CC's live registry (`sessions/<pid>.json`), which
//! pixtuoid reads for the pid alone.
//!
//! Any other session, in a terminal, is reached the way pixtuoid's
//! `focus` module does it (MIT, pixtuoid authors): from the CLI's pid up to the
//! first ancestor that is a GUI app, the terminal, which is brought to the
//! front. The app only, not its tab. A session inside tmux misses: the tmux
//! server is daemonized, so the walk ends at launchd.

use std::path::Path;

use pixtuoid_core::source::claude_code::{live_cc_session_ids, registry_dir};
use pixtuoid_core::{AgentId, AgentSlot, SceneState};

/// The desktop app's list of sessions waiting on the user.
pub const NEEDS_INPUT: &str = "claude://code/needs-input";

/// Open the session `id` belongs to; a subagent opens its root's. `Ok` is the
/// footer's note on where it went, the error what to tell the user.
pub fn open_agent(scene: &SceneState, id: AgentId) -> Result<&'static str, String> {
    let mut slot = scene.agents.get(&id).ok_or("这个 agent 已经走了")?;
    // At most one step per agent, so a parent cycle cannot hang the window.
    for _ in 0..scene.agents.len() {
        match slot.parent_id.and_then(|p| scene.agents.get(&p)) {
            Some(parent) => slot = parent,
            None => break,
        }
    }
    let pid = match &*slot.source {
        crate::pipeline::CC_SOURCE => {
            let dir = crate::tasks::projects_root()
                .and_then(|root| registry_dir(&root))
                .ok_or("找不到 ~/.claude/sessions")?;
            let (pid, entry) = registry_entry(&dir, &slot.session_id)?;
            if let Some(url) = url_for(&entry).map_err(str::to_owned)? {
                return open_url(&url).map(|()| "已在 Claude 打开");
            }
            pid
        }
        // Codex is read from its rollouts, so no hook carries its pid: the
        // process holding the rollout open is found at click time.
        crate::pipeline::CODEX_SOURCE => codex_pid(&slot.session_id).ok_or(GONE)?,
        // dsh's plugin and Cursor's hook stamp the CLI's pid on its events.
        _ => stamped_pid(slot, pixtuoid_core::source::pid_start_marker)?,
    };
    if !cfg!(target_os = "macos") {
        return Err("只有 macOS 上能切到终端".to_owned());
    }
    focus_terminal(&OsProcessTable, pid, activate_os)?;
    Ok("已切到它所在的终端")
}

const GONE: &str = "这个会话的进程已经不在了";

fn codex_pid(session_id: &str) -> Option<i32> {
    let source = crate::pipeline::CODEX_SOURCE;
    let probe = pixtuoid_core::source::registry::descriptor_for(source)?.focus_probe()?;
    (probe.paths)()
        .ok()?
        .iter()
        .find_map(|path| (probe.pid_for_session)(path, session_id))
}

/// The pid a hook stamped on `slot`, refused once that process is gone or its
/// pid was handed to another: the start marker read now must be the one read
/// when it was stamped, or the jump would raise some unrelated app.
fn stamped_pid(slot: &AgentSlot, marker: impl Fn(i32) -> Option<u64>) -> Result<i32, String> {
    if slot.exiting_at.is_some() {
        return Err(GONE.to_owned());
    }
    let stamp = slot
        .pid
        .ok_or("这个会话还没报过进程号，等它下次调用工具再试")?;
    if stamp
        .started
        .is_some_and(|started| marker(stamp.pid) != Some(started))
    {
        return Err(GONE.to_owned());
    }
    Ok(stamp.pid)
}

/// What the terminal walk reads, so the walk can be tested without processes.
pub trait ProcessTable {
    fn ppid(&self, pid: i32) -> Option<i32>;
    /// Whether `pid` is an app with a window of its own, a terminal.
    fn focusable(&self, pid: i32) -> bool;
}

/// `pid` itself or its nearest ancestor that is focusable. Stops at launchd and
/// on a cycle, so a racing process table cannot hang the window.
fn terminal_of(table: &impl ProcessTable, start: i32) -> Option<i32> {
    let mut seen = std::collections::HashSet::new();
    let mut pid = start;
    while pid > 1 && seen.insert(pid) {
        if table.focusable(pid) {
            return Some(pid);
        }
        pid = table.ppid(pid)?;
    }
    None
}

fn focus_terminal(
    table: &impl ProcessTable,
    pid: i32,
    activate: impl FnOnce(i32) -> bool,
) -> Result<(), String> {
    let app = terminal_of(table, pid).ok_or("找不到它所在的终端（tmux 里的会话找不到）")?;
    if !activate(app) {
        return Err("macOS 没让终端切到前台".to_owned());
    }
    Ok(())
}

/// `pid`'s parent and image name, from `ps`: it reads any pid, including the
/// setuid-root `login` a Terminal.app chain runs through, where the same-user
/// `proc_pidinfo` stops one hop short. The name has no directory and no
/// login shell's leading `-`.
pub fn ps_row(pid: i32) -> Option<(i32, String)> {
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_ps_row(&String::from_utf8_lossy(&out.stdout))
}

fn parse_ps_row(line: &str) -> Option<(i32, String)> {
    let (ppid, comm) = line.trim().split_once(char::is_whitespace)?;
    let comm = comm.trim();
    let name = comm.rsplit('/').next().unwrap_or(comm);
    Some((ppid.parse().ok()?, name.trim_start_matches('-').to_owned()))
}

struct OsProcessTable;

impl ProcessTable for OsProcessTable {
    fn ppid(&self, pid: i32) -> Option<i32> {
        ps_row(pid).map(|(ppid, _)| ppid)
    }

    #[cfg(target_os = "macos")]
    fn focusable(&self, pid: i32) -> bool {
        use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
        // A regular activation policy is a Dock app, the terminal; a shell or
        // daemon has no NSRunningApplication or is not regular.
        // SAFETY: plain Cocoa class-method calls on a plain pid.
        unsafe {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .is_some_and(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn focusable(&self, _pid: i32) -> bool {
        false
    }
}

/// Bring the app `pid` to the front; whether macOS took the request. Plain
/// Cocoa, not an Apple Event, so no permission prompt.
#[cfg(target_os = "macos")]
fn activate_os(pid: i32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    // SAFETY: as in `focusable`.
    unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .map(|app| {
                #[expect(
                    deprecated,
                    reason = "still honored; the option-less activate() is dropped while the user is busy in another app"
                )]
                app.activateWithOptions(NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps)
            })
            .unwrap_or(false)
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_os(_pid: i32) -> bool {
    false
}

pub fn open_url(url: &str) -> Result<(), String> {
    // Reaped off the window's thread: `open` hands the link to the app and
    // returns, and a frame should not stall on it, nor leave it a zombie.
    let mut child = std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map_err(|e| format!("open 失败：{e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The live pid of CC session `session_id` and its registry entry.
fn registry_entry(dir: &Path, session_id: &str) -> Result<(i32, serde_json::Value), String> {
    let live = live_cc_session_ids(dir).ok_or("读不了 ~/.claude/sessions")?;
    let pid = *live.pid_of.get(session_id).ok_or(GONE)?;
    let bytes = std::fs::read(dir.join(format!("{pid}.json")))
        .map_err(|e| format!("读不了会话登记：{e}"))?;
    let entry = serde_json::from_slice(&bytes).map_err(|_| "会话登记不是 JSON".to_owned())?;
    Ok((pid, entry))
}

/// The link that opens the registry entry `entry`'s session, `None` for a
/// session not hosted by the desktop app.
fn url_for(entry: &serde_json::Value) -> Result<Option<String>, &'static str> {
    let field = |k: &str| entry.get(k).and_then(|v| v.as_str());
    if field("entrypoint") != Some("claude-desktop") {
        return Ok(None);
    }
    let host = field("hostSessionId").ok_or("桌面会话没有 hostSessionId")?;
    // The app drops a link whose id is not `^local_[A-Za-z0-9-]{1,64}$`; a
    // bad one is caught here so the user is told, not ignored.
    let tail = host.strip_prefix("local_").unwrap_or_default();
    if tail.is_empty()
        || tail.len() > 64
        || !tail.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("hostSessionId 格式认不出");
    }
    Ok(Some(format!("claude://code/continue?session={host}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn a_desktop_session_opens_by_its_host_id() {
        let e = json!({"entrypoint": "claude-desktop", "hostSessionId": "local_demo"});
        assert_eq!(
            url_for(&e).unwrap().as_deref(),
            Some("claude://code/continue?session=local_demo")
        );
    }

    /// A terminal session has no link to open; it goes on to its terminal
    /// instead of failing.
    #[test]
    fn a_terminal_session_has_no_link_and_goes_to_its_terminal() {
        let e = json!({"entrypoint": "cli", "hostSessionId": "local_x"});
        assert_eq!(url_for(&e), Ok(None));
    }

    /// The app would silently drop these; the user must hear about it.
    #[test]
    fn an_id_the_app_would_reject_is_refused_here() {
        for host in ["local_", "session_abc", "local_a b", "local_a&x=1"] {
            let e = json!({"entrypoint": "claude-desktop", "hostSessionId": host});
            assert!(url_for(&e).is_err(), "{host}");
        }
    }

    struct Table {
        parents: HashMap<i32, i32>,
        apps: Vec<i32>,
    }

    impl ProcessTable for Table {
        fn ppid(&self, pid: i32) -> Option<i32> {
            self.parents.get(&pid).copied()
        }
        fn focusable(&self, pid: i32) -> bool {
            self.apps.contains(&pid)
        }
    }

    /// claude(300) → zsh(200) → login(150) → iTerm2(100) → launchd(1): the
    /// terminal is raised, not the shell or launchd.
    #[test]
    fn a_terminal_session_raises_the_app_it_runs_in() {
        let t = Table {
            parents: HashMap::from([(300, 200), (200, 150), (150, 100), (100, 1)]),
            apps: vec![100],
        };
        let mut raised = None;
        let done = focus_terminal(&t, 300, |p| {
            raised = Some(p);
            true
        });
        assert_eq!(done, Ok(()));
        assert_eq!(raised, Some(100));
    }

    /// Under tmux the chain ends at launchd with no app on it: the user is told
    /// why, and nothing is raised.
    #[test]
    fn a_session_with_no_terminal_above_it_raises_nothing() {
        let t = Table {
            parents: HashMap::from([(300, 200), (200, 1)]),
            apps: vec![],
        };
        let done = focus_terminal(&t, 300, |_| panic!("nothing to raise"));
        assert!(done.unwrap_err().contains("tmux"));
    }

    /// A racing table that loops must not hang the window's thread.
    #[test]
    fn a_parent_cycle_ends_the_walk() {
        let t = Table {
            parents: HashMap::from([(300, 200), (200, 300)]),
            apps: vec![],
        };
        assert_eq!(terminal_of(&t, 300), None);
    }

    #[test]
    fn macos_refusing_the_raise_is_reported() {
        let t = Table {
            parents: HashMap::new(),
            apps: vec![300],
        };
        assert!(focus_terminal(&t, 300, |_| false).is_err());
    }

    fn slot_with(pid: Option<pixtuoid_core::source::PidIdentity>) -> AgentSlot {
        let mut scene = SceneState::uniform(8);
        let id = AgentId::from_parts(crate::pipeline::CURSOR_SOURCE, "s");
        pixtuoid_core::Reducer::new().apply(
            &mut scene,
            pixtuoid_core::AgentEvent::SessionStart {
                agent_id: id,
                source: crate::pipeline::CURSOR_SOURCE.into(),
                session_id: "s".into(),
                cwd: "/repo".into(),
                parent_id: None,
            },
            std::time::SystemTime::UNIX_EPOCH,
            pixtuoid_core::Transport::Hook,
        );
        let mut slot = scene.agents.remove(&id).unwrap();
        slot.pid = pid;
        slot
    }

    /// A pid handed to another process since it was stamped would raise an
    /// unrelated app; only the same process, by its start marker, is jumped to.
    #[test]
    fn a_stamped_pid_is_used_only_while_it_is_the_same_process() {
        let stamp = |started| Some(pixtuoid_core::source::PidIdentity::new(4242, started));
        let slot = slot_with(stamp(Some(7)));
        assert_eq!(stamped_pid(&slot, |_| Some(7)), Ok(4242));
        assert!(stamped_pid(&slot, |_| Some(8)).is_err(), "recycled");
        assert!(stamped_pid(&slot, |_| None).is_err(), "gone");
        assert!(
            stamped_pid(&slot_with(None), |_| Some(7)).is_err(),
            "never stamped"
        );
    }

    #[test]
    fn a_ps_row_names_the_image_without_its_path_or_login_dash() {
        assert_eq!(
            parse_ps_row("  412 /Applications/iTerm.app/Contents/MacOS/iTerm2\n"),
            Some((412, "iTerm2".to_owned()))
        );
        assert_eq!(parse_ps_row("9 -zsh"), Some((9, "zsh".to_owned())));
        assert_eq!(parse_ps_row(""), None);
    }
}
