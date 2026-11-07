//! Back to where an agent runs: the Claude desktop app opens a session it
//! hosts from a `claude://code/continue?session=local_…` link. Which host id
//! a session has is only in CC's live registry (`sessions/<pid>.json`), which
//! pixtuoid reads for the pid alone.
//!
//! Any other session, in a terminal, is reached the way pixtuoid's
//! `focus` module does it (MIT, pixtuoid authors): from the CLI's pid up to the
//! first ancestor that is a GUI app, the terminal, which is brought to the
//! front. In iTerm2 and Terminal.app its tab is then picked by the session's
//! tty, over AppleScript; other terminals come forward as they were.
//!
//! A session inside tmux has no terminal above it: the tmux server is
//! daemonized, so the walk ends at launchd. Its pane is found among the
//! default server's panes instead, by the pane's shell being one of the
//! session's ancestors; tmux shows that pane, and the terminal raised is the
//! one a client attached to it runs in.

use std::path::Path;

use pixtuoid_core::source::claude_code::{live_cc_session_ids, registry_dir};
use pixtuoid_core::{AgentId, AgentSlot, SceneState};

/// The desktop app's list of sessions waiting on the user.
pub const NEEDS_INPUT: &str = "claude://code/needs-input";

/// The slot `open_agent` opens: a subagent's root session. Kept separate so
/// the next-waiting path can prove its selected child reaches this same root
/// without asking the OS to open anything.
pub(crate) fn root_agent(scene: &SceneState, id: AgentId) -> Result<&AgentSlot, String> {
    let mut slot = scene
        .agents
        .get(&id)
        .ok_or(crate::i18n::t("这个 agent 已经走了", "this agent has left"))?;
    // At most one step per agent, so a parent cycle cannot hang the window.
    for _ in 0..scene.agents.len() {
        match slot.parent_id.and_then(|p| scene.agents.get(&p)) {
            Some(parent) => slot = parent,
            None => break,
        }
    }
    Ok(slot)
}

/// Open the session `id` belongs to; a subagent opens its root's. `Ok` is the
/// footer's note on where it went, the error what to tell the user.
pub fn open_agent(scene: &SceneState, id: AgentId) -> Result<&'static str, String> {
    let slot = root_agent(scene, id)?;
    let pid = match &*slot.source {
        crate::pipeline::CC_SOURCE => {
            let dir = crate::tasks::projects_root()
                .and_then(|root| registry_dir(&root))
                .ok_or(crate::i18n::t(
                    "找不到 ~/.claude/sessions",
                    "cannot find ~/.claude/sessions",
                ))?;
            let (pid, entry) = registry_entry(&dir, &slot.session_id)?;
            if let Some(url) = url_for(&entry).map_err(str::to_owned)? {
                return open_desktop(&url);
            }
            pid
        }
        // Codex is read from its rollouts, so no hook carries its pid: the
        // process holding the rollout open is found at click time.
        crate::pipeline::CODEX_SOURCE => codex_pid(&slot.session_id).ok_or_else(gone)?,
        // dsh's plugin and Cursor's hook stamp the CLI's pid on its events.
        _ => stamped_pid(slot, pixtuoid_core::source::pid_start_marker)?,
    };
    if !cfg!(target_os = "macos") {
        return Err(crate::i18n::t(
            "只有 macOS 上能切到终端",
            "switching to a terminal works only on macOS",
        )
        .to_owned());
    }
    focus_terminal(&OsProcessTable, &OsTmux, pid, activate_os, select_tab_os)
}

fn gone() -> &'static str {
    crate::i18n::t("这个会话的进程已经不在了", "this session's process is gone")
}

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
        return Err(gone().to_owned());
    }
    let stamp = slot.pid.ok_or(crate::i18n::t(
        "这个会话还没报过进程号，等它下次调用工具再试",
        "this session has not reported its pid yet; try again after its next tool call",
    ))?;
    if stamp
        .started
        .is_some_and(|started| marker(stamp.pid) != Some(started))
    {
        return Err(gone().to_owned());
    }
    Ok(stamp.pid)
}

/// What the terminal walk reads, so the walk can be tested without processes.
pub trait ProcessTable {
    fn ppid(&self, pid: i32) -> Option<i32>;
    /// Whether `pid` is an app with a window of its own, a terminal.
    fn focusable(&self, pid: i32) -> bool;
    /// `pid`'s image name, without its directory.
    fn name(&self, pid: i32) -> Option<String>;
    /// The terminal `pid` reads from, as `/dev/ttys003`.
    fn tty(&self, pid: i32) -> Option<String>;
}

/// A tmux pane: its shell's pid, its id (`%3`), its session's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub pid: i32,
    pub id: String,
    pub session: String,
}

/// A terminal attached to tmux: its tmux client's pid, its tty, the session
/// it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub pid: i32,
    pub tty: String,
    pub session: String,
}

/// What the jump asks of tmux, so it can be tested without a server.
pub trait Tmux {
    /// Every pane of the default server; `None` with no server running.
    fn panes(&self) -> Option<Vec<Pane>>;
    fn clients(&self) -> Vec<Client>;
    /// Show `pane`, on the client at `tty` when given (switching it to the
    /// pane's session), else on the clients already showing its session.
    fn show(&self, pane: &str, tty: Option<&str>) -> bool;
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

/// `start` and its ancestors up to launchd, bounded as [`terminal_of`] is.
fn ancestors(table: &impl ProcessTable, start: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let mut pid = start;
    while pid > 1 && !out.contains(&pid) {
        out.push(pid);
        match table.ppid(pid) {
            Some(p) => pid = p,
            None => break,
        }
    }
    out
}

/// Show `pid`'s tmux pane; the client it is shown on. A client already on
/// the pane's session is used as it is, so no one's view of another session
/// is pulled away; with none, the first client is switched over.
fn show_tmux_pane(table: &impl ProcessTable, tmux: &impl Tmux, pid: i32) -> Result<Client, String> {
    let not_in_tmux = crate::i18n::t("找不到它所在的终端", "cannot find the terminal it runs in");
    let up = ancestors(table, pid);
    let pane = tmux
        .panes()
        .ok_or(not_in_tmux)?
        .into_iter()
        .find(|p| up.contains(&p.pid))
        .ok_or(not_in_tmux)?;
    let clients = tmux.clients();
    let (client, tty) = match clients.iter().find(|c| c.session == pane.session) {
        Some(c) => (c, None),
        None => {
            let c = clients.first().ok_or(crate::i18n::t(
                "它在 tmux 里，但没有终端连着 tmux（先 tmux attach）",
                "it runs in tmux, but no terminal is attached to tmux (tmux attach first)",
            ))?;
            (c, Some(c.tty.as_str()))
        }
    };
    if !tmux.show(&pane.id, tty) {
        return Err(crate::i18n::t("tmux 没切过去", "tmux did not switch to it").to_owned());
    }
    Ok(client.clone())
}

/// Raise the terminal `pid` runs in, then its tab; inside tmux, its pane
/// first, and the terminal its client runs in. `Ok` is the footer's note.
fn focus_terminal(
    table: &impl ProcessTable,
    tmux: &impl Tmux,
    pid: i32,
    activate: impl FnOnce(i32) -> bool,
    select_tab: impl FnOnce(Terminal, String),
) -> Result<&'static str, String> {
    let (app, tty, note) = match terminal_of(table, pid) {
        Some(app) => (
            app,
            table.tty(pid),
            crate::i18n::t("已切到它所在的终端", "switched to its terminal"),
        ),
        None => {
            let client = show_tmux_pane(table, tmux, pid)?;
            let app = terminal_of(table, client.pid).ok_or(crate::i18n::t(
                "找不到连着 tmux 的那个终端",
                "cannot find the terminal attached to tmux",
            ))?;
            (
                app,
                Some(client.tty),
                crate::i18n::t("已切到它所在的 tmux 窗格", "switched to its tmux pane"),
            )
        }
    };
    if !activate(app) {
        return Err(crate::i18n::t(
            "macOS 没让终端切到前台",
            "macOS did not bring the terminal forward",
        )
        .to_owned());
    }
    if let (Some(term), Some(tty)) = (table.name(app).as_deref().and_then(Terminal::of), tty) {
        select_tab(term, tty);
    }
    Ok(note)
}

/// A terminal whose tabs can be picked by tty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    ITerm,
    Apple,
}

impl Terminal {
    /// By the app's image name, as `ps` gives it.
    fn of(name: &str) -> Option<Self> {
        match name {
            "iTerm2" => Some(Self::ITerm),
            "Terminal" => Some(Self::Apple),
            _ => None,
        }
    }

    /// The AppleScript that brings the tab reading `item 1 of argv` forward.
    fn script(self) -> &'static str {
        match self {
            Self::ITerm => {
                r#"on run argv
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if tty of s is (item 1 of argv) then
            select w
            select t
            select s
            return
          end if
        end repeat
      end repeat
    end repeat
  end tell
end run"#
            }
            Self::Apple => {
                r#"on run argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if tty of t is (item 1 of argv) then
          set selected of t to true
          set index of w to 1
          return
        end if
      end repeat
    end repeat
  end tell
end run"#
            }
        }
    }
}

/// Pick the tab on `tty` in `term`. An Apple Event, so macOS asks once
/// whether agent-office may control the terminal; refused, the terminal is
/// still in front, on whatever tab it was. Off the window's thread, as the
/// ask blocks until answered.
fn select_tab_os(term: Terminal, tty: String) {
    std::thread::spawn(move || {
        let _ = std::process::Command::new("osascript")
            .args(["-e", term.script(), &tty])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// `ps`'s tty column as a device path; `None` for a process with none (`??`).
fn parse_tty(col: &str) -> Option<String> {
    let col = col.trim();
    let name = col.strip_prefix("/dev/").unwrap_or(col);
    (name.starts_with("tty") && name.len() > 3).then(|| format!("/dev/{name}"))
}

/// `tmux list-panes -a -F '#{pane_pid}\t#{pane_id}\t#{session_name}'`.
fn parse_panes(out: &str) -> Vec<Pane> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            Some(Pane {
                pid: f.next()?.parse().ok()?,
                id: f.next()?.to_owned(),
                session: f.next()?.to_owned(),
            })
        })
        .collect()
}

/// `tmux list-clients -F '#{client_pid}\t#{client_tty}\t#{client_session}'`.
fn parse_clients(out: &str) -> Vec<Client> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            Some(Client {
                pid: f.next()?.parse().ok()?,
                tty: f.next()?.to_owned(),
                session: f.next()?.to_owned(),
            })
        })
        .collect()
}

struct OsTmux;

impl OsTmux {
    /// tmux's output for `args`, `None` when it fails. An app started from
    /// the Dock has no Homebrew on its PATH, so Homebrew's places are tried
    /// after the PATH's.
    fn run(args: &[&str]) -> Option<String> {
        ["tmux", "/opt/homebrew/bin/tmux", "/usr/local/bin/tmux"]
            .iter()
            .find_map(|bin| {
                std::process::Command::new(bin)
                    .args(args)
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()
            })
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Tmux for OsTmux {
    fn panes(&self) -> Option<Vec<Pane>> {
        Self::run(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_pid}\t#{pane_id}\t#{session_name}",
        ])
        .map(|out| parse_panes(&out))
    }

    fn clients(&self) -> Vec<Client> {
        Self::run(&[
            "list-clients",
            "-F",
            "#{client_pid}\t#{client_tty}\t#{client_session}",
        ])
        .map(|out| parse_clients(&out))
        .unwrap_or_default()
    }

    fn show(&self, pane: &str, tty: Option<&str>) -> bool {
        if let Some(tty) = tty
            && Self::run(&["switch-client", "-c", tty, "-t", pane]).is_none()
        {
            return false;
        }
        Self::run(&["select-window", "-t", pane]).is_some()
            && Self::run(&["select-pane", "-t", pane]).is_some()
    }
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

    fn name(&self, pid: i32) -> Option<String> {
        ps_row(pid).map(|(_, name)| name)
    }

    fn tty(&self, pid: i32) -> Option<String> {
        let out = std::process::Command::new("ps")
            .args(["-o", "tty=", "-p", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        parse_tty(&String::from_utf8_lossy(&out.stdout))
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

/// Open a desktop link only after `open` says something took it. Both desktop
/// jumps use this: spawning `open` alone also succeeds when no app claims the
/// scheme, which used to put a false success note in the footer.
pub fn open_desktop(url: &str) -> Result<&'static str, String> {
    open_desktop_with(url, open_url_checked)
}

/// The injectable seam keeps both callers' result mapping testable without
/// opening a real app.
fn open_desktop_with(
    url: &str,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<&'static str, String> {
    open(url).map(|()| crate::i18n::t("已在 Claude 打开", "opened in Claude"))
}

/// Wait for `open` to say whether anything took the link: a jump must not
/// report a session opened when no app claims `claude://`. The wait is bounded.
pub fn open_url_checked(url: &str) -> Result<(), String> {
    // Its output is dropped: stderr repeats the link, the whole prompt in it.
    let child = std::process::Command::new("open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    opened(child.and_then(|c| exit_within(c, OPEN_WAIT)))
}

/// Long enough for `open` to fail on a link no app claims (it does so at
/// once); a cold-starting app can hold it far longer.
const OPEN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// How `child` exited, or `None` if it is still running after `limit`; then
/// it is reaped off this thread so it does not become a zombie.
fn exit_within(
    mut child: std::process::Child,
    limit: std::time::Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if start.elapsed() >= limit {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// An `open` still running has found an app and is waiting on it to launch:
/// the link has been taken, and the frame should not stall until it starts.
fn opened(status: std::io::Result<Option<std::process::ExitStatus>>) -> Result<(), String> {
    let status = status.map_err(|e| crate::tr!("open 失败：{e}", "open failed: {e}"))?;
    match status {
        None => Ok(()),
        Some(s) if s.success() => Ok(()),
        Some(s) => Err(crate::tr!(
            "打不开 Claude 桌面 app（open {}）：装好它，或用 agent-office dispatch --terminal 在终端开",
            "could not open the Claude desktop app (open {}): install it, or use agent-office dispatch --terminal",
            s
        )),
    }
}

/// The live pid of CC session `session_id` and its registry entry.
fn registry_entry(dir: &Path, session_id: &str) -> Result<(i32, serde_json::Value), String> {
    let live = live_cc_session_ids(dir).ok_or(crate::i18n::t(
        "读不了 ~/.claude/sessions",
        "cannot read ~/.claude/sessions",
    ))?;
    let pid = *live.pid_of.get(session_id).ok_or_else(gone)?;
    let bytes = std::fs::read(dir.join(format!("{pid}.json"))).map_err(|e| {
        crate::tr!(
            "读不了会话登记：{e}",
            "cannot read the session registry: {e}"
        )
    })?;
    let entry = serde_json::from_slice(&bytes).map_err(|_| {
        crate::i18n::t("会话登记不是 JSON", "the session registry is not JSON").to_owned()
    })?;

    Ok((pid, entry))
}

/// The link that opens the registry entry `entry`'s session, `None` for a
/// session not hosted by the desktop app.
fn url_for(entry: &serde_json::Value) -> Result<Option<String>, &'static str> {
    let field = |k: &str| entry.get(k).and_then(|v| v.as_str());
    if field("entrypoint") != Some("claude-desktop") {
        return Ok(None);
    }
    let host = field("hostSessionId").ok_or(crate::i18n::t(
        "桌面会话没有 hostSessionId",
        "the desktop session has no hostSessionId",
    ))?;
    // The app drops a link whose id is not `^local_[A-Za-z0-9-]{1,64}$`; a
    // bad one is caught here so the user is told, not ignored.
    let tail = host.strip_prefix("local_").unwrap_or_default();
    if tail.is_empty()
        || tail.len() > 64
        || !tail.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(crate::i18n::t(
            "hostSessionId 格式认不出",
            "unrecognized hostSessionId format",
        ));
    }
    Ok(Some(format!("claude://code/continue?session={host}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// Without the desktop app nothing claims `claude://` and `open` exits
    /// 1: a dispatch that called that success would say a session is
    /// waiting when none is. The error must point to `--terminal` and must
    /// not echo the link, which carries the whole prompt.
    #[cfg(unix)]
    #[test]
    fn a_link_nothing_opens_is_an_error_naming_the_way_round() {
        use std::os::unix::process::ExitStatusExt;
        let exit = |code: i32| Ok(Some(std::process::ExitStatus::from_raw(code << 8)));
        assert_eq!(opened(exit(0)), Ok(()));
        let err = opened(exit(1)).unwrap_err();
        assert!(err.contains("--terminal"), "{err}");
        let missing = Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(opened(missing).is_err());
        // The real `open` on a scheme no app claims.
        #[cfg(target_os = "macos")]
        assert!(open_url_checked("agentofficenosuch://x").is_err());
    }

    /// Both desktop jump paths share this mapping. A refused URL must reach
    /// the footer as an error, never as the old spawn-only false success.
    #[test]
    fn a_desktop_jump_reports_the_checked_open_result() {
        let mut asked = None;
        let failed = open_desktop_with("claude://code/needs-input", |url| {
            asked = Some(url.to_owned());
            Err("no application claims claude://".into())
        });
        assert_eq!(asked.as_deref(), Some("claude://code/needs-input"));
        assert!(failed.is_err_and(|e| e.contains("no application")));

        let opened = open_desktop_with("claude://code/continue?session=local_demo", |_| Ok(()));
        assert!(opened.is_ok());
    }

    /// `open` waits while a cold app starts: the window must not freeze
    /// that long, and an `open` still running has taken the link, so it is
    /// not reported as a failure. One that exits at once is still heard.
    #[cfg(unix)]
    #[test]
    fn a_slow_open_does_not_hold_the_caller() {
        use std::time::{Duration, Instant};
        let sh = |script: &str| {
            std::process::Command::new("sh")
                .args(["-c", script])
                .spawn()
                .unwrap()
        };
        let start = Instant::now();
        let slow = exit_within(sh("sleep 5"), Duration::from_millis(200)).unwrap();
        assert!(slow.is_none());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(opened(Ok(slow)), Ok(()));
        let failed = exit_within(sh("exit 1"), Duration::from_secs(3)).unwrap();
        assert!(opened(Ok(failed)).is_err());
    }

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
        names: HashMap<i32, &'static str>,
        ttys: HashMap<i32, &'static str>,
    }

    impl Table {
        fn new(parents: &[(i32, i32)], apps: &[i32]) -> Self {
            Self {
                parents: parents.iter().copied().collect(),
                apps: apps.to_vec(),
                names: HashMap::new(),
                ttys: HashMap::new(),
            }
        }
    }

    impl ProcessTable for Table {
        fn ppid(&self, pid: i32) -> Option<i32> {
            self.parents.get(&pid).copied()
        }
        fn focusable(&self, pid: i32) -> bool {
            self.apps.contains(&pid)
        }
        fn name(&self, pid: i32) -> Option<String> {
            self.names.get(&pid).map(|n| (*n).to_owned())
        }
        fn tty(&self, pid: i32) -> Option<String> {
            self.ttys.get(&pid).map(|t| (*t).to_owned())
        }
    }

    /// A tmux server, recording what it was asked to show.
    #[derive(Default)]
    struct FakeTmux {
        panes: Option<Vec<Pane>>,
        clients: Vec<Client>,
        shown: std::cell::RefCell<Vec<(String, Option<String>)>>,
    }

    impl Tmux for FakeTmux {
        fn panes(&self) -> Option<Vec<Pane>> {
            self.panes.clone()
        }
        fn clients(&self) -> Vec<Client> {
            self.clients.clone()
        }
        fn show(&self, pane: &str, tty: Option<&str>) -> bool {
            self.shown
                .borrow_mut()
                .push((pane.to_owned(), tty.map(str::to_owned)));
            true
        }
    }

    fn pane(pid: i32, id: &str, session: &str) -> Pane {
        Pane {
            pid,
            id: id.into(),
            session: session.into(),
        }
    }

    fn client(pid: i32, tty: &str, session: &str) -> Client {
        Client {
            pid,
            tty: tty.into(),
            session: session.into(),
        }
    }

    /// What one jump did: its note, the app raised and the tab picked.
    type Jumped = (
        Result<&'static str, String>,
        Option<i32>,
        Option<(Terminal, String)>,
    );

    fn jump(t: &Table, tmux: &FakeTmux, pid: i32) -> Jumped {
        let mut raised = None;
        let mut tab = None;
        let done = focus_terminal(
            t,
            tmux,
            pid,
            |p| {
                raised = Some(p);
                true
            },
            |term, tty| tab = Some((term, tty)),
        );
        (done, raised, tab)
    }

    /// claude(300) → zsh(200) → login(150) → iTerm2(100) → launchd(1): the
    /// terminal is raised, not the shell or launchd.
    #[test]
    fn a_terminal_session_raises_the_app_it_runs_in() {
        let t = Table::new(&[(300, 200), (200, 150), (150, 100), (100, 1)], &[100]);
        let (done, raised, tab) = jump(&t, &FakeTmux::default(), 300);
        assert!(done.is_ok());
        assert_eq!(raised, Some(100));
        assert_eq!(tab, None, "a terminal not known to take a tab pick");
    }

    /// With several tabs open, raising the app alone lands on whichever tab
    /// was last used; in iTerm2 and Terminal.app the session's own is picked
    /// by its tty.
    #[test]
    fn in_iterm_the_sessions_own_tab_is_picked_by_its_tty() {
        let mut t = Table::new(&[(300, 200), (200, 100), (100, 1)], &[100]);
        t.names.insert(100, "iTerm2");
        t.ttys.insert(300, "/dev/ttys004");
        let (_, _, tab) = jump(&t, &FakeTmux::default(), 300);
        assert_eq!(tab, Some((Terminal::ITerm, "/dev/ttys004".to_owned())));
        t.names.insert(100, "Terminal");
        let (_, _, tab) = jump(&t, &FakeTmux::default(), 300);
        assert_eq!(tab, Some((Terminal::Apple, "/dev/ttys004".to_owned())));
    }

    /// Under tmux the session's chain ends at launchd, under the tmux server.
    /// Its pane is the one whose shell is among its ancestors; tmux shows it,
    /// and the terminal raised is the attached client's, on the client's tab.
    #[test]
    fn a_session_in_tmux_shows_its_pane_and_raises_the_clients_terminal() {
        // claude(300) → zsh(250, pane %7) → tmux server(240) → launchd;
        // tmux client(520) → zsh(510) → iTerm2(100).
        let mut t = Table::new(
            &[
                (300, 250),
                (250, 240),
                (240, 1),
                (520, 510),
                (510, 100),
                (100, 1),
            ],
            &[100],
        );
        t.names.insert(100, "iTerm2");
        let tmux = FakeTmux {
            panes: Some(vec![pane(260, "%6", "work"), pane(250, "%7", "work")]),
            clients: vec![client(520, "/dev/ttys009", "work")],
            ..FakeTmux::default()
        };
        let (done, raised, tab) = jump(&t, &tmux, 300);
        assert_eq!(done, Ok("已切到它所在的 tmux 窗格"));
        assert_eq!(*tmux.shown.borrow(), [("%7".to_owned(), None)]);
        assert_eq!(raised, Some(100));
        assert_eq!(tab, Some((Terminal::ITerm, "/dev/ttys009".to_owned())));
    }

    /// No client shows the pane's session: one is switched to it, rather
    /// than tmux showing a pane nobody can see.
    #[test]
    fn with_no_client_on_its_session_a_client_is_switched_over() {
        let t = Table::new(
            &[(300, 250), (250, 240), (240, 1), (520, 100), (100, 1)],
            &[100],
        );
        let tmux = FakeTmux {
            panes: Some(vec![pane(250, "%7", "side")]),
            clients: vec![client(520, "/dev/ttys009", "main")],
            ..FakeTmux::default()
        };
        assert!(jump(&t, &tmux, 300).0.is_ok());
        assert_eq!(
            *tmux.shown.borrow(),
            [("%7".to_owned(), Some("/dev/ttys009".to_owned()))]
        );
    }

    /// A detached tmux has no terminal to raise: the user is told to attach,
    /// and nothing is raised.
    #[test]
    fn a_detached_tmux_session_says_to_attach() {
        let t = Table::new(&[(300, 250), (250, 240), (240, 1)], &[]);
        let tmux = FakeTmux {
            panes: Some(vec![pane(250, "%7", "work")]),
            ..FakeTmux::default()
        };
        let (done, raised, _) = jump(&t, &tmux, 300);
        assert!(done.unwrap_err().contains("attach"));
        assert_eq!(raised, None);
    }

    /// Neither under a terminal nor in a tmux pane (another multiplexer, a
    /// tmux on its own socket): the user is told, and nothing is raised.
    #[test]
    fn a_session_with_no_terminal_above_it_raises_nothing() {
        let t = Table::new(&[(300, 200), (200, 1)], &[]);
        let tmux = FakeTmux {
            panes: Some(vec![pane(999, "%1", "other")]),
            ..FakeTmux::default()
        };
        let (done, raised, _) = jump(&t, &tmux, 300);
        assert!(done.is_err());
        assert_eq!(raised, None);
        assert!(tmux.shown.borrow().is_empty(), "no stranger's pane shown");
        assert!(jump(&t, &FakeTmux::default(), 300).0.is_err(), "no server");
    }

    /// tmux's own output, as the format strings ask for it; a session name
    /// may hold spaces.
    #[test]
    fn tmux_listings_are_read_by_their_tab_separated_fields() {
        assert_eq!(
            parse_panes("250\t%7\tmy work\nbad line\n"),
            [pane(250, "%7", "my work")]
        );
        assert_eq!(
            parse_clients("520\t/dev/ttys009\tmain\n"),
            [client(520, "/dev/ttys009", "main")]
        );
    }

    #[test]
    fn a_ps_tty_is_a_device_path_and_none_has_no_tab() {
        assert_eq!(parse_tty(" ttys003\n").as_deref(), Some("/dev/ttys003"));
        assert_eq!(parse_tty("??"), None);
        assert_eq!(parse_tty(""), None);
    }

    /// A racing table that loops must not hang the window's thread.
    #[test]
    fn a_parent_cycle_ends_the_walk() {
        let t = Table::new(&[(300, 200), (200, 300)], &[]);
        assert_eq!(terminal_of(&t, 300), None);
        assert_eq!(ancestors(&t, 300), [300, 200]);
    }

    #[test]
    fn macos_refusing_the_raise_is_reported() {
        let t = Table::new(&[], &[300]);
        let done = focus_terminal(
            &t,
            &FakeTmux::default(),
            300,
            |_| false,
            |_, _| panic!("no tab picked behind a refused raise"),
        );
        assert!(done.is_err());
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
