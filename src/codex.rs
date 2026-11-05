//! Codex: the office watches its rollouts by itself, and `connect codex`
//! adds the hooks only they carry (the approval prompt and its answer). Codex
//! loads hooks through its plugin system, so this lays out a local
//! marketplace holding one plugin and registers it with the `codex` CLI, the
//! way pixtuoid's Codex install does (verified there against Codex 0.164).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// The marketplace and the plugin share one name.
const NAME: &str = "agent-office";
/// The events pixtuoid's Codex decoder reads.
const EVENTS: [&str; 9] = [
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "PermissionRequest",
    "SessionEnd",
];

fn plugin_id() -> String {
    format!("{NAME}@{NAME}")
}

/// `~/.config/agent-office/codex-plugin`, the marketplace root.
fn root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/agent-office/codex-plugin"))
}

/// What the marketplace root holds: its manifest, the plugin's, and the
/// hooks file, each as `(path under the root, content)`.
fn layout(hook_command: &str) -> Result<Vec<(PathBuf, String)>> {
    let group = json!([{
        "hooks": [{
            "type": "command",
            "command": hook_command,
            "timeout": 5,
            "statusMessage": "agent-office",
        }]
    }]);
    let hooks: serde_json::Map<String, Value> = EVENTS
        .iter()
        .map(|e| ((*e).to_owned(), group.clone()))
        .collect();
    let pretty = |v: Value| serde_json::to_string_pretty(&v).map(|s| s + "\n");
    Ok(vec![
        (
            PathBuf::from(".agents/plugins/marketplace.json"),
            pretty(json!({
                "name": NAME,
                "plugins": [{
                    "name": NAME,
                    "source": { "source": "local", "path": format!("./{NAME}") },
                }],
            }))?,
        ),
        (
            // No `version`: Codex copies a version-less plugin to a `local`
            // dir of its cache, which [`installed`] reads.
            PathBuf::from(NAME).join(".codex-plugin/plugin.json"),
            pretty(json!({
                "name": NAME,
                "description": "Sends Codex's session events to the agent-office window",
            }))?,
        ),
        (
            PathBuf::from(NAME).join("hooks/hooks.json"),
            pretty(json!({ "hooks": hooks }))?,
        ),
    ])
}

fn hook_command(exe: &Path) -> String {
    let quoted = exe.display().to_string().replace('\'', "'\\''");
    format!("'{quoted}' hook --source {}", crate::pipeline::CODEX_SOURCE)
}

/// Run `codex <args>`; its own words when it fails.
fn codex(args: &[&std::ffi::OsStr]) -> Result<()> {
    let shown = args
        .iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let out = Command::new("codex")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .context("运行 codex 失败：codex 不在 PATH 上？")?;
    if !out.status.success() {
        let why = if out.stderr.is_empty() {
            &out.stdout
        } else {
            &out.stderr
        };
        bail!(
            "`codex {shown}` 失败：{}",
            String::from_utf8_lossy(why).trim()
        );
    }
    Ok(())
}

fn config_toml() -> PathBuf {
    pixtuoid_core::source::codex::codex_home().join("config.toml")
}

/// The hooks file Codex runs: its cache copy of our plugin.
fn cached_hooks() -> PathBuf {
    pixtuoid_core::source::codex::codex_home()
        .join("plugins/cache")
        .join(NAME)
        .join(NAME)
        .join("local/hooks/hooks.json")
}

/// `agent-office connect codex`.
pub fn connect() -> Result<()> {
    let root = root().context("找不到 HOME")?;
    let exe = std::env::current_exe()?.canonicalize()?;
    for (rel, content) in layout(&hook_command(&exe))? {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("建 {} 失败", dir.display()))?;
        }
        std::fs::write(&path, content).with_context(|| format!("写 {} 失败", path.display()))?;
    }
    // Adding a marketplace Codex already records from another dir is an
    // error, and from this one a no-op; dropping it first covers both.
    if registered_marketplace() {
        codex(&[
            "plugin".as_ref(),
            "marketplace".as_ref(),
            "remove".as_ref(),
            NAME.as_ref(),
        ])?;
    }
    codex(&[
        "plugin".as_ref(),
        "marketplace".as_ref(),
        "add".as_ref(),
        root.as_os_str(),
    ])?;
    // Also what refreshes Codex's cached copy after a reconnect.
    codex(&["plugin".as_ref(), "add".as_ref(), plugin_id().as_ref()])?;
    println!(
        "已在 Codex 装上插件 {}；新开的 Codex 会话才生效。移动或删除这个程序后要重新 connect codex。",
        plugin_id()
    );
    Ok(())
}

/// `agent-office disconnect codex`.
pub fn disconnect() -> Result<()> {
    let config = std::fs::read_to_string(config_toml()).unwrap_or_default();
    if config.contains(&plugin_id()) {
        codex(&["plugin".as_ref(), "remove".as_ref(), plugin_id().as_ref()])?;
    }
    if registered_marketplace() {
        codex(&[
            "plugin".as_ref(),
            "marketplace".as_ref(),
            "remove".as_ref(),
            NAME.as_ref(),
        ])?;
    }
    if let Some(root) = root().filter(|r| r.is_dir()) {
        std::fs::remove_dir_all(&root).with_context(|| format!("删 {} 失败", root.display()))?;
    }
    println!("已从 Codex 移除插件 {}", plugin_id());
    Ok(())
}

/// Whether Codex's config records our marketplace (`[marketplaces.<name>]`).
fn registered_marketplace() -> bool {
    std::fs::read_to_string(config_toml()).is_ok_and(|c| records_marketplace(&c))
}

fn records_marketplace(config: &str) -> bool {
    config.lines().any(|l| {
        let l = l.trim();
        l == format!("[marketplaces.{NAME}]") || l == format!("[marketplaces.\"{NAME}\"]")
    })
}

/// Whether Codex runs our hooks: its copy of the plugin forwards to us.
pub fn installed() -> bool {
    std::fs::read_to_string(cached_hooks())
        .is_ok_and(|h| h.contains(&format!("hook --source {}", crate::pipeline::CODEX_SOURCE)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codex finds the plugin through the marketplace manifest and runs the
    /// hooks file in Claude Code's shape; every event the decoder reads must
    /// run our forwarder, stamped as Codex's, or the router reads it as
    /// Claude Code's and keys the figure under the wrong CLI.
    #[test]
    fn every_event_runs_the_forwarder_stamped_as_codex() {
        let cmd = hook_command(Path::new("/opt/it's/agent-office"));
        assert_eq!(cmd, r"'/opt/it'\''s/agent-office' hook --source codex");
        let files = layout(&cmd).unwrap();
        let get = |rel: &str| -> Value {
            let (_, c) = files.iter().find(|(p, _)| p == Path::new(rel)).unwrap();
            serde_json::from_str(c).unwrap()
        };
        let market = get(".agents/plugins/marketplace.json");
        assert_eq!(market["plugins"][0]["source"]["path"], "./agent-office");
        assert_eq!(get("agent-office/.codex-plugin/plugin.json")["name"], NAME);
        let hooks = get("agent-office/hooks/hooks.json");
        for e in EVENTS {
            assert_eq!(hooks["hooks"][e][0]["hooks"][0]["command"], cmd, "{e}");
        }
    }

    /// The forwarder's payload must decode as Codex's: an approval prompt is
    /// what the hooks are for.
    #[test]
    fn a_stamped_codex_permission_request_decodes_as_a_wait() {
        let v = json!({
            "hook_event_name": "PermissionRequest", "session_id": "019e7762-9ded-7e33-be41-946ecf105bf4",
            "turn_id": "t1", "cwd": "/repo", "tool_name": "exec_command",
            pixtuoid_core::source::decoder::SOURCE_STAMP: crate::pipeline::CODEX_SOURCE,
        });
        let evs = pixtuoid_core::source::decoder::decode_hook_payload(v).unwrap();
        assert!(
            evs.iter()
                .any(|e| matches!(e, pixtuoid_core::AgentEvent::UserActionRequest { .. })),
            "{evs:?}"
        );
    }

    #[test]
    fn our_marketplace_is_found_in_codexs_config() {
        assert!(records_marketplace(
            "model = \"x\"\n\n[marketplaces.agent-office]\nsource = \"/h/.config/agent-office/codex-plugin\"\n"
        ));
        assert!(!records_marketplace("[marketplaces.pixtuoid]\n"));
    }
}
