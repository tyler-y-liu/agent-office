//! Office roles as DeepSeek Harness Agent Presets. One generated Cordis
//! plugin registers the whole roster from the same templates Claude Code uses.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::roles::{ROSTER, Role};

const PLUGIN_ID: &str = "agent-office-roles";
const TEMPLATE: &str = include_str!("dsh_roles_plugin.mjs");
const STANDARD_PLUGINS: &str = include_str!("../roles/dsh-standard.json");
const MARK: &str = "// agent-office roles sha=";

fn our_dir() -> Option<PathBuf> {
    crate::dsh::home().map(|h| h.join("agent-office"))
}

fn plugin_path(dir: &Path) -> PathBuf {
    dir.join("agent-office-dsh-roles.mjs")
}

/// DSH pm delegates by preset, not by Claude Code's subagent type. Keep the
/// shared role brief intact apart from the tool contract its host provides.
fn dsh_brief(post: &crate::roles::Post) -> Result<String> {
    let template = post.template();
    let body = template[crate::roles::frontmatter_end(template)..].trim();
    if post.role != Role::Pm {
        return Ok(body.to_owned());
    }
    let out = body
        .replace(
            "用 Agent 工具派活，subagent_type 写角色名。",
            "用 subagent 工具派活，agent_preset 写角色名。",
        )
        .replace("你手上没有 Agent 工具时", "你手上没有 subagent 工具时")
        .replace(
            "（TodoWrite、TaskCreate、update_plan，有哪个用哪个）",
            "（todo_write）",
        )
        .replace(
            "Hand out work with the Agent tool, with the role name as subagent_type.",
            "Hand out work with the subagent tool, with the role name as agent_preset.",
        )
        .replace("Without the Agent tool", "Without the subagent tool")
        .replace("(TodoWrite, TaskCreate, update_plan)", "(todo_write)");
    anyhow::ensure!(
        !out.contains("subagent_type")
            && !out.contains("Agent tool")
            && out.contains("agent_preset"),
        "PM 角色模板里的派活说明变了，DSH 适配需要同步更新"
    );
    Ok(out)
}

/// Materialize role changes in the complete plugin tree, including tools in
/// Cordis groups. Runtime string checks cannot tell a nested subagent tool
/// from the unrelated top-level id, so tests inspect this resulting JSON.
fn configure_plugins(
    rows: &mut [serde_json::Value],
    prefix: &str,
    role_ids: &[String],
    found: &mut (bool, usize),
) {
    for row in rows {
        let id = row
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        match id.as_deref() {
            Some("persona") => {
                row["config"]["prefix"] = prefix.into();
                found.0 = true;
            }
            Some("tool-subagent") => {
                // Independent spawn children can hold another office role;
                // subagent_fork keeps the parent's composition by design.
                row["config"]["allowedAgentPresets"] = role_ids.into();
                found.1 += 1;
            }
            _ => {}
        }
        if let Some(children) = row
            .get_mut("config")
            .and_then(serde_json::Value::as_array_mut)
        {
            configure_plugins(children, prefix, role_ids, found);
        }
    }
}

fn source_body() -> Result<String> {
    let base: serde_json::Value = serde_json::from_str(STANDARD_PLUGINS)?;
    let base = base.as_array().context("DSH Standard 插件模板不是数组")?;
    let role_ids: Vec<String> = ROSTER.iter().map(|p| p.role.agent_name()).collect();
    let mut roles = Vec::with_capacity(ROSTER.len());
    for (i, post) in ROSTER.iter().enumerate() {
        let template = post.template();
        let description = template
            .lines()
            .find_map(|line| line.strip_prefix("description: "))
            .context("角色模板缺 description")?;
        let prefix = format!(
            "You are a coding agent powered by the {{{{model}}}} model.\n\n{}",
            dsh_brief(post)?
        );
        let mut plugins = base.clone();
        let mut found = (false, 0);
        configure_plugins(&mut plugins, &prefix, &role_ids, &mut found);
        anyhow::ensure!(
            found == (true, 1),
            "DSH Standard 插件模板缺 persona 或有重复的 subagent 工具：{found:?}"
        );
        roles.push(serde_json::json!({
            "id": post.role.agent_name(),
            "name": format!("Office · {}", crate::i18n::t(post.zh, post.en)),
            "description": description,
            "order": 100 + i,
            "plugins": plugins,
        }));
    }
    Ok(TEMPLATE
        .replace("__ROLES__", &serde_json::to_string_pretty(&roles)?)
        .trim_end()
        .to_owned())
}

fn rendered() -> Result<String> {
    let body = source_body()?;
    let sha = crate::tasks::memo::key(&[body.as_str()]);
    Ok(format!("{body}\n{MARK}{sha}\n"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Missing,
    Current,
    Stale,
    Edited,
    Foreign,
}

fn state_of(text: Option<&str>, current: &str) -> State {
    let Some(text) = text else {
        return State::Missing;
    };
    let Some(at) = text.rfind(MARK) else {
        return State::Foreign;
    };
    let body = text[..at].trim_end();
    let footer = &text[at + MARK.len()..];
    let (sha, appended) = footer.split_once('\n').unwrap_or((footer, ""));
    // Anything after the ownership line belongs to the user, as with the
    // Claude role files: never overwrite or delete it without --force.
    if !appended.trim().is_empty() || sha.trim() != crate::tasks::memo::key(&[body]) {
        return State::Edited;
    }
    if text == current {
        State::Current
    } else {
        State::Stale
    }
}

fn read_state(path: &Path, current: &str) -> Result<State> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(state_of(Some(&text), current)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::Missing),
        Err(e) => Err(e).with_context(|| format!("读 {} 失败", path.display())),
    }
}

/// Whether the current generated roles are mounted. An edited file counts as
/// not ready rather than letting dispatch assume it provides every role.
pub fn installed() -> bool {
    let (Some(home), Some(dir)) = (crate::dsh::home(), our_dir()) else {
        return false;
    };
    let Ok(current) = rendered() else {
        return false;
    };
    read_state(&plugin_path(&dir), &current).is_ok_and(|s| s == State::Current)
        && crate::dsh::plugin_mounted(&home, PLUGIN_ID)
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Missing => crate::i18n::t("没装", "missing"),
        State::Current => crate::i18n::t("已装（当前版本）", "installed (current)"),
        State::Stale => crate::i18n::t(
            "已装（旧版，install 会升级）",
            "installed (stale; install upgrades)",
        ),
        State::Edited => crate::i18n::t("改过，--force 才覆盖", "edited; --force overwrites"),
        State::Foreign => {
            crate::i18n::t("同名文件不是 agent-office 的", "foreign file at our path")
        }
    }
}

pub fn doctor_line() -> (bool, String) {
    let Some(dir) = our_dir() else {
        return (false, "找不到 dsh 目录".into());
    };
    let path = plugin_path(&dir);
    match rendered().and_then(|current| read_state(&path, &current)) {
        Ok(state) => {
            let mounted =
                crate::dsh::home().is_some_and(|h| crate::dsh::plugin_mounted(&h, PLUGIN_ID));
            let ready = state == State::Current && mounted;
            let mut line = format!(
                "{}，{}；{}",
                state_name(state),
                if mounted {
                    crate::i18n::t("已挂载", "mounted")
                } else {
                    crate::i18n::t("未挂载", "not mounted")
                },
                path.display()
            );
            if !ready {
                line += crate::i18n::t(
                    "；agent-office roles install --agent dsh 安装",
                    "; run agent-office roles install --agent dsh",
                );
            }
            (ready, line)
        }
        Err(e) => (false, format!("{e:#}")),
    }
}

/// `roles install/remove --agent dsh`: the generated file is one owned unit,
/// so a hand edit stops the whole write just as it does for Claude's agents.
pub fn cli(args: &[&str]) -> Result<()> {
    let dir = our_dir().context(crate::i18n::t(
        "找不到 dsh 目录",
        "cannot find the dsh directory",
    ))?;
    let home = crate::dsh::home().context(crate::i18n::t(
        "找不到 dsh 目录",
        "cannot find the dsh directory",
    ))?;
    let path = plugin_path(&dir);
    match args {
        [] => {
            let (_, line) = doctor_line();
            println!("{}", line);
            for post in ROSTER {
                println!("{:<17} {} / {}", post.role.agent_name(), post.zh, post.en);
            }
        }
        ["install", rest @ ..] if rest.iter().all(|a| *a == "--force") => {
            let force = rest.contains(&"--force");
            let current = rendered()?;
            let state = read_state(&path, &current)?;
            match state {
                State::Foreign => println!("{}", state_name(state)),
                State::Edited if !force => println!("{}", state_name(state)),
                _ => {
                    std::fs::create_dir_all(&dir)
                        .with_context(|| format!("建 {} 失败", dir.display()))?;
                    if state != State::Current {
                        crate::atomic::write_keeping_mode(
                            &path,
                            &path.with_extension("mjs.agent-office.tmp"),
                            current.as_bytes(),
                        )
                        .with_context(|| format!("写 {} 失败", path.display()))?;
                    }
                    crate::dsh::mount_plugin(&home, PLUGIN_ID, &path)?;
                    println!(
                        "{}",
                        crate::tr!(
                            "已把 {} 个角色装成 DSH Agent Presets。新开的 DSH 进程/会话使用；运行中的进程重连或重启后生效。",
                            "Installed {} roles as DSH Agent Presets. Use a new DSH process/session; reconnect or restart a running process.",
                            ROSTER.len()
                        )
                    );
                }
            }
        }
        ["remove", rest @ ..] if rest.iter().all(|a| *a == "--force") => {
            let force = rest.contains(&"--force");
            let current = rendered()?;
            let state = read_state(&path, &current)?;
            match state {
                State::Foreign => println!("{}", state_name(state)),
                State::Edited if !force => println!("{}", state_name(state)),
                _ => {
                    crate::dsh::unmount_plugin(&home, PLUGIN_ID)?;
                    match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(e).with_context(|| format!("删 {} 失败", path.display()));
                        }
                    }
                    println!("已移除 DSH 角色 presets；已有的 DSH 会话保持原来的组合");
                }
            }
        }
        _ => anyhow::bail!("用法：agent-office roles [install|remove] [--force] --agent dsh"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DSH's PM must call the tool and argument its host actually exposes;
    /// silently retaining Claude's names leaves autonomous role dispatch dead.
    #[test]
    fn the_dsh_pm_delegates_by_agent_preset() {
        let source = source_body().unwrap();
        assert!(source.contains("\"id\": \"office-pm\""));
        assert!(source.contains("allowedAgentPresets"));
        assert!(!source.contains("subagent_type"));
        assert!(!source.contains("Agent tool"));
        assert!(source.contains("agent_preset"));
        assert!(!source.contains("__ROLES__") && !source.contains("__STANDARD_PLUGINS__"));
    }

    /// A valid-looking generator is not enough: the registered PM object must
    /// give its NESTED spawn tool all nine role choices or PM cannot delegate.
    #[test]
    fn the_generated_pm_has_a_nested_agent_preset_allowlist() {
        let source = source_body().unwrap();
        let start = source.find("const ROLES = ").unwrap() + "const ROLES = ".len();
        let end = source[start..].find("\n\nexport const name").unwrap() + start;
        let roles: Vec<serde_json::Value> = serde_json::from_str(&source[start..end]).unwrap();
        assert_eq!(roles.len(), 9);
        let pm = roles.iter().find(|r| r["id"] == "office-pm").unwrap();
        fn subagents<'a>(rows: &'a [serde_json::Value], out: &mut Vec<&'a serde_json::Value>) {
            for row in rows {
                if row["id"] == "tool-subagent" {
                    out.push(row);
                }
                if let Some(children) = row["config"].as_array() {
                    subagents(children, out);
                }
            }
        }
        let mut found = Vec::new();
        subagents(pm["plugins"].as_array().unwrap(), &mut found);
        assert_eq!(found.len(), 1);
        let allowed = found[0]["config"]["allowedAgentPresets"]
            .as_array()
            .unwrap();
        assert_eq!(allowed.len(), 9);
        for role in &roles {
            assert!(
                allowed.contains(&role["id"]),
                "missing {} from PM allowlist",
                role["id"]
            );
        }
    }

    /// The roles row is global, but preset-free headless must still start:
    /// defer the registry dependency instead of pending the whole plugin row.
    #[test]
    fn roles_wait_optionally_for_a_preset_capable_profile() {
        let source = source_body().unwrap();
        assert!(source.contains("export const inject = []"));
        assert!(source.contains("ctx.inject([\"agentPresets\"]"));
    }

    /// A generated upgrade is not a hand edit; only changing the body without
    /// updating its ownership hash is. This keeps install safe to re-run.
    #[test]
    fn generated_states_distinguish_stale_from_edited_and_foreign() {
        let current = rendered().unwrap();
        assert_eq!(state_of(None, &current), State::Missing);
        assert_eq!(state_of(Some(&current), &current), State::Current);
        let old_body = "const old = true";
        let old = format!(
            "{old_body}\n{MARK}{}\n",
            crate::tasks::memo::key(&[old_body])
        );
        assert_eq!(state_of(Some(&old), &current), State::Stale);
        let edited = current.replace("const ROLES", "const EDITED_ROLES");
        assert_eq!(state_of(Some(&edited), &current), State::Edited);
        let appended = format!("{current}\n// user customization\n");
        assert_eq!(state_of(Some(&appended), &current), State::Edited);
        let whitespace = format!("{current}\n\n");
        assert_eq!(state_of(Some(&whitespace), &current), State::Stale);
        assert_eq!(state_of(Some("// mine\n"), &current), State::Foreign);
    }
}
