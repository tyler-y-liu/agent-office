//! agent-office's own Claude Code hooks, in place of pixtuoid's plugin and its
//! `pixtuoid-hook` shim: `connect` points every hook event Claude Code fires at
//! `agent-office hook`, which forwards the payload as one line to the office's
//! socket, the line `HookRouter` reads. A socket of its own also lets the office
//! run beside pixtuoid.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// The events the reducer reads; the ones pixtuoid's plugin hooks, and the
/// prompt and its end, which keep a figure at its desk for its whole turn.
const EVENTS: [&str; 10] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Stop",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "PermissionRequest",
    "SubagentStart",
    "SubagentStop",
];

/// Under the listener's 2 MiB per-connection cap. A larger payload (a huge
/// tool output) is dropped, and the transcript still carries that step.
const STDIN_CAP: u64 = 1024 * 1024;

/// `/tmp/agent-office-<uid>/`, as pixtuoid keeps its socket: not `TMPDIR` or
/// `USER`, which the hook process Claude Code spawns need not share with the
/// window.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("AGENT_OFFICE_SOCKET").filter(|p| !p.is_empty()) {
        return p.into();
    }
    #[cfg(unix)]
    let dir = format!("/tmp/agent-office-{}", rustix::process::getuid().as_raw());
    #[cfg(not(unix))]
    let dir = std::env::temp_dir().join("agent-office");
    PathBuf::from(dir).join("agent-office.sock")
}

/// The socket's directory, which must be ours alone: in a shared `/tmp` another
/// user could have made it first and would then receive every hook payload.
/// A path from `AGENT_OFFICE_SOCKET` is the user's choice and is not checked.
pub fn prepare_socket_dir(socket: &std::path::Path) -> Result<()> {
    let Some(dir) = socket.parent() else {
        return Ok(());
    };
    if std::env::var_os("AGENT_OFFICE_SOCKET").is_some_and(|p| !p.is_empty()) {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
        .with_context(|| format!("创建 {} 失败", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let m = std::fs::symlink_metadata(dir)?;
        let mine = m.uid() == rustix::process::getuid().as_raw();
        if !m.is_dir() || !mine || m.permissions().mode() & 0o077 != 0 {
            anyhow::bail!(
                "{} 不是只属于你的目录（可能被别人抢先建了）：删掉它，或者用 AGENT_OFFICE_SOCKET 换个路径",
                dir.display()
            );
        }
    }
    Ok(())
}

/// `agent-office hook`. Claude Code waits on every hook, so this never fails,
/// never prints and gives up quickly: with no office running the event is
/// simply not seen.
///
/// `source` is the CLI a non-Claude caller speaks for (`--source dsh`): the
/// router reads it from the stamp pixtuoid's shim writes, and an unstamped
/// payload is Claude Code's.
pub fn forward(source: Option<&str>) {
    let mut raw = String::new();
    if std::io::stdin()
        .take(STDIN_CAP)
        .read_to_string(&mut raw)
        .is_err()
    {
        return;
    }
    if source == Some(crate::pipeline::CURSOR_SOURCE)
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
    {
        crate::cursor::record(&v);
        // Only the task log takes the prompt and the reply: pixtuoid's
        // decoder refuses an event it does not know. Cursor reads the answer
        // to the prompt's.
        if v["hook_event_name"] == crate::cursor::PROMPT_EVENT {
            println!("{}", json!({ "continue": true }));
            return;
        }
        if v["hook_event_name"] == crate::cursor::REPLY_EVENT {
            return;
        }
    }
    send(&raw, source, &socket_path());
}

fn send(raw: &str, source: Option<&str>, socket: &std::path::Path) -> Option<()> {
    let mut v: Value = serde_json::from_str(raw).ok()?;
    if let Some(source) = source {
        v.as_object_mut()?.insert(
            pixtuoid_core::source::decoder::SOURCE_STAMP.to_owned(),
            source.into(),
        );
    }
    // The router keeps this pid on the session for a double-click to walk up
    // to its terminal from. dsh's plugin stamps its own; Claude Code and Codex
    // are found from their own files.
    if source == Some(crate::pipeline::CURSOR_SOURCE)
        && let Some(pid) = cli_pid(std::process::id() as i32, crate::jump::ps_row)
    {
        v.as_object_mut()?
            .entry(pixtuoid_core::source::decoder::PID_STAMP)
            .or_insert(pid.into());
    }
    #[cfg(unix)]
    {
        let mut s = std::os::unix::net::UnixStream::connect(socket).ok()?;
        s.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
        writeln!(s, "{v}").ok()?;
    }
    Some(())
}

/// Shells a CLI may run its hook through, by image name.
const SHELLS: [&str; 7] = ["sh", "bash", "zsh", "dash", "ksh", "fish", "busybox"];

/// The CLI that ran the hook process `me`: its first ancestor that is not a
/// shell. Cursor `eval`s a hook inside a shell that exits after it, which a
/// click would find gone (pixtuoid-hook's `cli_pid`, MIT, pixtuoid authors).
fn cli_pid(me: i32, row_of: impl Fn(i32) -> Option<(i32, String)>) -> Option<i32> {
    let mut pid = me;
    // A real chain has a shell or two in it; the bound ends a corrupt one.
    for _ in 0..8 {
        let (parent, _) = row_of(pid)?;
        if parent <= 1 {
            return None;
        }
        if !SHELLS.contains(&row_of(parent)?.1.as_str()) {
            return Some(parent);
        }
        pid = parent;
    }
    None
}

fn settings_path() -> Result<PathBuf> {
    Ok(crate::tasks::claude_dir()
        .context("找不到 Claude 配置目录")?
        .join("settings.json"))
}

/// The settings and their text as read, `None` when the file does not exist.
fn read_settings() -> Result<(PathBuf, Option<String>, Value)> {
    let path = settings_path()?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("读 {} 失败", path.display())),
    };
    let v = serde_json::from_str(raw.as_deref().unwrap_or("{}"))
        .with_context(|| format!("{} 不是合法 JSON", path.display()))?;
    Ok((path, raw, v))
}

fn is_ours(command: &str) -> bool {
    command.contains("agent-office") && command.ends_with(" hook")
}

/// Drop our groups from every event, and the events only ours were under.
fn remove_ours(settings: &mut Value) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    let mut emptied = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let before = groups.len();
        groups.retain(|g| {
            !g["hooks"].as_array().is_some_and(|hs| {
                hs.iter()
                    .any(|h| h["command"].as_str().is_some_and(is_ours))
            })
        });
        if groups.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        hooks.shift_remove(&event);
    }
}

fn add_ours(settings: &mut Value, command: &str) -> Result<()> {
    let root = settings
        .as_object_mut()
        .context("settings.json 顶层不是对象")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("settings.json 的 hooks 不是对象")?;
    for event in EVENTS {
        hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("hooks.{event} 不是数组"))?
            .push(json!({
                "matcher": "",
                "hooks": [{"type": "command", "command": command, "timeout": 5}],
            }));
    }
    Ok(())
}

/// Whether every event in [`EVENTS`] reaches us: a connect from an older
/// build lacks the turn's own events, and its figures leave their desks
/// between calls.
pub fn installed() -> bool {
    read_settings().is_ok_and(|(_, _, s)| has_every_event(&s))
}

fn has_every_event(settings: &Value) -> bool {
    EVENTS.iter().all(|event| {
        settings["hooks"][event].as_array().is_some_and(|groups| {
            groups.iter().any(|g| {
                g["hooks"].as_array().is_some_and(|hs| {
                    hs.iter()
                        .any(|h| h["command"].as_str().is_some_and(is_ours))
                })
            })
        })
    })
}

fn write_settings(path: &PathBuf, before: Option<&str>, settings: &Value) -> Result<()> {
    if let Some(before) = before {
        let backup = path.with_extension("json.agent-office.bak");
        std::fs::write(&backup, before)
            .with_context(|| format!("备份到 {} 失败", backup.display()))?;
        println!("原文件备份在 {}", backup.display());
    }
    let text = serde_json::to_string_pretty(settings)? + "\n";
    // Claude Code reads this file while running: a rename never shows it half
    // written. A symlinked settings.json (dotfiles) is written through instead,
    // so the link survives.
    let linked = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let result = if linked {
        std::fs::write(path, text)
    } else {
        let tmp = path.with_extension("json.agent-office.tmp");
        std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path))
    };
    result.with_context(|| format!("写 {} 失败", path.display()))?;
    println!("已更新 {}", path.display());
    Ok(())
}

/// `agent-office connect`: hook this binary in, replacing an earlier install.
pub fn connect() -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let quoted = exe.display().to_string().replace('\'', "'\\''");
    let command = format!("'{quoted}' hook");
    let (path, raw, mut settings) = read_settings()?;
    remove_ours(&mut settings);
    add_ours(&mut settings, &command)?;
    write_settings(&path, raw.as_deref(), &settings)?;
    println!(
        "hooks 指向 {}；新开的 Claude 会话才生效。移动或删除这个程序后要重新 connect。",
        exe.display()
    );
    Ok(())
}

/// `agent-office disconnect`.
pub fn disconnect() -> Result<()> {
    let (path, raw, mut settings) = read_settings()?;
    remove_ours(&mut settings);
    write_settings(&path, raw.as_deref(), &settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's own hooks and every other key must come through untouched,
    /// and reconnecting must not stack a second copy of ours.
    #[test]
    fn connect_twice_then_disconnect_leaves_other_hooks_alone() {
        let theirs =
            json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "guard.py"}]});
        let original =
            json!({"model": "opus", "hooks": {"PreToolUse": [theirs.clone()], "PreCompact": []}});
        let mut s = original.clone();
        let cmd = "'/usr/local/bin/agent-office' hook";
        for _ in 0..2 {
            remove_ours(&mut s);
            add_ours(&mut s, cmd).unwrap();
        }
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["SessionStart"][0]["hooks"][0]["command"], cmd);
        remove_ours(&mut s);
        assert_eq!(s, original);
    }

    /// An install from before the turn's events must read as not connected,
    /// so doctor sends the user back to `connect`.
    #[test]
    fn an_install_missing_an_event_is_not_connected() {
        let cmd = "'/usr/local/bin/agent-office' hook";
        let mut s = json!({});
        add_ours(&mut s, cmd).unwrap();
        assert!(has_every_event(&s));
        s["hooks"].as_object_mut().unwrap().shift_remove("Stop");
        assert!(!has_every_event(&s));
    }

    /// Payloads must reach the router as one line it can parse.
    #[cfg(unix)]
    #[test]
    fn a_forwarded_payload_arrives_as_one_json_line() {
        use std::io::BufRead;
        let dir = std::env::temp_dir().join(format!("ao-hook-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let pretty = "{\n \"hook_event_name\": \"Stop\",\n \"session_id\": \"s1\"\n}";
        assert!(send(pretty, None, &sock).is_some());
        let (conn, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(conn).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["session_id"], "s1");
        std::fs::remove_dir_all(dir).ok();
    }

    /// Unstamped, the router reads a dsh payload as Claude Code's and keys
    /// the figure under the wrong CLI; stamped, the dsh decoder claims it.
    #[cfg(unix)]
    #[test]
    fn a_dsh_payload_arrives_stamped_and_decodes_as_dsh() {
        use std::io::BufRead;
        let dir = std::env::temp_dir().join(format!("ao-hook-dsh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let raw = r#"{"type":"session_start","sessionId":"session-1","cwd":"/repo"}"#;
        assert!(send(raw, Some("dsh"), &sock).is_some());
        let (conn, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(conn).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        let evs = pixtuoid_core::source::decoder::decode_hook_payload(v).unwrap();
        assert!(
            evs.iter().any(|e| matches!(e,
                pixtuoid_core::AgentEvent::SessionStart { source, session_id, .. }
                    if source == "dsh" && session_id == "session-1")),
            "{evs:?}"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    /// Cursor runs the hook in a shell that exits right after; stamping that
    /// shell would send a double-click to a dead pid. The CLI above it is
    /// what the session's terminal is found from.
    #[test]
    fn the_cli_pid_skips_the_shell_the_hook_ran_in() {
        let rows = std::collections::HashMap::from([
            (50, (40, "agent-office".to_owned())),
            (40, (30, "zsh".to_owned())),
            (30, (20, "cursor-agent".to_owned())),
            (20, (1, "login".to_owned())),
        ]);
        let row_of = |pid| rows.get(&pid).cloned();
        assert_eq!(cli_pid(50, row_of), Some(30));
        // Started straight from launchd: nobody's CLI.
        assert_eq!(cli_pid(20, row_of), None);
    }
}
