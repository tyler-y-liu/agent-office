//! Cursor (`cursor-agent`): hook-only. `connect cursor` points every event of
//! Cursor's user-global `hooks.json` at `agent-office hook --source cursor`,
//! which forwards the payload the way pixtuoid's Cursor decoder reads it and
//! keeps a per-session task log the task reader tails: Cursor's own
//! transcript has the conversation but not the tool calls.

use std::path::{Path, PathBuf};

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
/// put it. Terminal.app's login shell may not have the PATH this process has,
/// so the launch script runs this absolute path, not the bare name.
pub fn bin() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        // A relative entry (`.`) would resolve inside the repository the
        // script cd's into, running the repository's own `cursor-agent`.
        .filter(|d| d.is_absolute())
        .map(|d| d.join("cursor-agent"));
    let known = home
        .map(|h| h.join(".local/bin/cursor-agent"))
        .into_iter()
        .chain(["/opt/homebrew/bin/cursor-agent", "/usr/local/bin/cursor-agent"].map(PathBuf::from));
    on_path.chain(known).find(|p| p.is_file())
}

/// `s` as one POSIX shell word: inside single quotes nothing is special but
/// the quote itself, which closes, is escaped and reopens.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `.command` file Terminal.app runs to start a dispatched session: it
/// deletes itself first (it holds the prompt), then starts `bin` on `prompt`
/// in `dir`. Every value is one quoted word, so a task's quotes, `$(…)` or
/// backticks reach `cursor-agent` as text and never run. A NUL cannot be in
/// an argument at all, so it is dropped rather than cutting the prompt.
pub(crate) fn launch_script(bin: &Path, dir: &Path, prompt: &str) -> String {
    let prompt = prompt.replace('\0', "");
    format!(
        "#!/bin/sh\nrm -f -- \"$0\"\ncd -- {} || exit 1\nexec {} {}\n",
        sh_quote(&dir.to_string_lossy()),
        sh_quote(&bin.to_string_lossy()),
        sh_quote(&prompt),
    )
}

/// Where launch scripts wait for Terminal.app to pick them up.
fn launch_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/agent-office/cursor/launch"))
}

/// A script older than this was never run (Terminal refused it, or the Mac
/// went to sleep first); it still holds a prompt, so it does not stay.
const LAUNCH_LEFTOVER: std::time::Duration = std::time::Duration::from_secs(86_400);

fn sweep_leftovers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > LAUNCH_LEFTOVER);
        if old && path.extension().is_some_and(|x| x == "command") {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Open a new Terminal.app window running `cursor-agent` on `prompt` in
/// `dir`: cursor-agent has no app to hand a prompt to, so a dispatch from the
/// office window starts it in a terminal of its own. `tag` names the script
/// so a leftover says which dispatch it was.
pub fn open_in_terminal(dir: &Path, tag: &str, prompt: &str) -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "macos"),
        crate::i18n::t(
            "只有 macOS 能替你开 Terminal：加 --terminal 在当前终端启动",
            "only macOS can open Terminal for you: add --terminal to start in this terminal",
        )
    );
    let bin = bin().context(crate::i18n::t(
        "找不到 cursor-agent 命令：先装好 Cursor CLI",
        "cursor-agent not found: install the Cursor CLI first",
    ))?;
    let launch = launch_dir().context(crate::i18n::t("找不到 HOME", "HOME is not set"))?;
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(&launch)
        .with_context(|| crate::tr!("建 {} 失败", "could not create {}", launch.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // An existing directory keeps its own mode through create; the
        // scripts hold prompts, so it is narrowed either way.
        std::fs::set_permissions(&launch, std::fs::Permissions::from_mode(0o700))
            .with_context(|| crate::tr!("改 {} 权限失败", "could not chmod {}", launch.display()))?;
    }
    sweep_leftovers(&launch);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file = launch.join(format!("{tag}-{}-{nanos}.command", std::process::id()));
    write_script(&file, &launch_script(&bin, dir, prompt))
        .with_context(|| crate::tr!("写 {} 失败", "could not write {}", file.display()))?;
    let opened = std::process::Command::new("open")
        .args(["-a", "Terminal"])
        .arg(&file)
        .spawn()
        .and_then(|child| crate::jump::exit_within(child, std::time::Duration::from_secs(3)));
    let failed = match opened {
        // Still running: Terminal took the file and is starting up.
        Ok(None) => None,
        Ok(Some(s)) if s.success() => None,
        Ok(Some(s)) => Some(s.to_string()),
        Err(e) => Some(e.to_string()),
    };
    if let Some(why) = failed {
        let _ = std::fs::remove_file(&file);
        anyhow::bail!(crate::tr!(
            "没能用 Terminal 打开 cursor-agent（{why}）：加 --terminal 在当前终端启动",
            "could not open cursor-agent in Terminal ({why}): add --terminal to start in this terminal"
        ));
    }
    Ok(())
}

/// A new script, executable and readable by its owner only: it holds the
/// prompt. `create_new` also refuses a file or symlink planted at the name.
fn write_script(path: &Path, text: &str) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o700);
    let mut f = o.open(path)?;
    std::io::Write::write_all(&mut f, text.as_bytes())?;
    f.sync_all()
}

/// Where the forwarder writes one `<session>.jsonl` task log per session.
pub fn transcripts_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config/agent-office/cursor/transcripts"))
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

    /// The launch script is where a task's text meets a shell. Run for real
    /// with a fake cursor-agent, every byte of a hostile prompt must arrive
    /// as the one argument, nothing in it may run, the session must start in
    /// the dispatched directory even when its name has a space and a quote,
    /// and the script (which holds the prompt) must be gone afterwards.
    #[cfg(unix)]
    #[test]
    fn a_launch_script_passes_any_prompt_through_as_one_word() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ao-cursor-launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("my repo's 'x");
        let bins = root.join("bin 'dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&bins).unwrap();
        let bin = bins.join("cursor-agent");
        std::fs::write(&bin, "#!/bin/sh\nprintf '%s' \"$1\" > out\npwd > cwd\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let prompt = "[office:qa] it's \"quoted\" $(touch pwned) `touch pwned2` \\n back\\slash\n\
            second line 100% $HOME ${PATH} 中文任务 🎉\n";
        let script = root.join("launch.command");
        std::fs::write(&script, launch_script(&bin, &dir, prompt)).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let status = std::process::Command::new(&script)
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read(dir.join("out")).unwrap(), prompt.as_bytes());
        let cwd = std::fs::read_to_string(dir.join("cwd")).unwrap();
        assert_eq!(
            Path::new(cwd.trim_end()).canonicalize().unwrap(),
            dir.canonicalize().unwrap()
        );
        for p in [&root, &dir] {
            assert!(!p.join("pwned").exists() && !p.join("pwned2").exists(), "{}", p.display());
        }
        assert!(!script.exists(), "the script deletes itself");
        // A NUL cannot travel in argv: dropped, not the prompt cut there.
        assert!(launch_script(&bin, &dir, "a\0b").contains("'ab'"));
        std::fs::remove_dir_all(root).ok();
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
        assert!(t.latest_prompt().is_some_and(|p| p.starts_with('审')), "{:?}", t.latest_prompt());
    }

    #[test]
    fn now_is_written_the_way_the_reader_parses_it() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_793_871_015), "2026-11-05T09:30:15Z");
    }
}
