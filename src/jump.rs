//! Back to where an agent runs: the Claude desktop app opens a session it
//! hosts from a `claude://code/continue?session=local_…` link. Which host id
//! a session has is only in CC's live registry (`sessions/<pid>.json`), which
//! pixtuoid reads for the pid alone.

use std::path::Path;

use pixtuoid_core::source::claude_code::{live_cc_session_ids, registry_dir};
use pixtuoid_core::{AgentId, SceneState};

/// The desktop app's list of sessions waiting on the user.
pub const NEEDS_INPUT: &str = "claude://code/needs-input";

/// Open the session `id` belongs to; a subagent opens its root's. The error is
/// what to tell the user.
pub fn open_agent(scene: &SceneState, id: AgentId) -> Result<(), String> {
    let mut slot = scene.agents.get(&id).ok_or("这个 agent 已经走了")?;
    // At most one step per agent, so a parent cycle cannot hang the window.
    for _ in 0..scene.agents.len() {
        match slot.parent_id.and_then(|p| scene.agents.get(&p)) {
            Some(parent) => slot = parent,
            None => break,
        }
    }
    let dir = crate::tasks::projects_root()
        .and_then(|root| registry_dir(&root))
        .ok_or("找不到 ~/.claude/sessions")?;
    let url = session_url(&dir, &slot.session_id)?;
    open_url(&url)
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

fn session_url(dir: &Path, session_id: &str) -> Result<String, String> {
    let live = live_cc_session_ids(dir).ok_or("读不了 ~/.claude/sessions")?;
    let pid = live
        .pid_of
        .get(session_id)
        .ok_or("这个会话的进程已经不在了")?;
    let bytes = std::fs::read(dir.join(format!("{pid}.json")))
        .map_err(|e| format!("读不了会话登记：{e}"))?;
    let entry: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "会话登记不是 JSON".to_owned())?;
    url_for(&entry).map_err(str::to_owned)
}

/// The link that opens the registry entry `entry`'s session.
fn url_for(entry: &serde_json::Value) -> Result<String, &'static str> {
    let field = |k: &str| entry.get(k).and_then(|v| v.as_str());
    if field("entrypoint") != Some("claude-desktop") {
        return Err("终端里的会话，跳不过去");
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
    Ok(format!("claude://code/continue?session={host}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_desktop_session_opens_by_its_host_id() {
        let e = json!({"entrypoint": "claude-desktop", "hostSessionId": "local_demo"});
        assert_eq!(
            url_for(&e).unwrap(),
            "claude://code/continue?session=local_demo"
        );
    }

    #[test]
    fn a_terminal_session_says_why_it_cannot_jump() {
        let e = json!({"entrypoint": "cli", "hostSessionId": "local_x"});
        assert_eq!(url_for(&e), Err("终端里的会话，跳不过去"));
    }

    /// The app would silently drop these; the user must hear about it.
    #[test]
    fn an_id_the_app_would_reject_is_refused_here() {
        for host in ["local_", "session_abc", "local_a b", "local_a&x=1"] {
            let e = json!({"entrypoint": "claude-desktop", "hostSessionId": host});
            assert!(url_for(&e).is_err(), "{host}");
        }
    }
}
