//! The office's roles: a software team's posts, installed as Claude Code
//! agent definitions (`~/.claude/agents/office-<id>.md`) so a session can be
//! one, or hand work to one, and recognised in transcripts so each worker
//! shows which post it holds.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pixtuoid_core::sprite::Rgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Pm,
    Product,
    Ux,
    Architect,
    Frontend,
    Backend,
    Qa,
    Devops,
    Reviewer,
}

pub struct Post {
    pub role: Role,
    pub id: &'static str,
    pub zh: &'static str,
    /// The one character its chip shows.
    pub glyph: &'static str,
    pub ink: Rgb,
    template: &'static str,
}

const fn ink(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// In the picker's order: digits 1–9 pick these, 0 the PM.
pub const ROSTER: &[Post] = &[
    Post {
        role: Role::Product,
        id: "product",
        zh: "产品",
        glyph: "产",
        ink: ink(0xe0, 0x8f, 0x5a),
        template: include_str!("../roles/office-product.md"),
    },
    Post {
        role: Role::Ux,
        id: "ux",
        zh: "设计",
        glyph: "设",
        ink: ink(0xd9, 0x6f, 0xb8),
        template: include_str!("../roles/office-ux.md"),
    },
    Post {
        role: Role::Architect,
        id: "architect",
        zh: "架构",
        glyph: "架",
        ink: ink(0x9a, 0x7c, 0xe0),
        template: include_str!("../roles/office-architect.md"),
    },
    Post {
        role: Role::Frontend,
        id: "frontend",
        zh: "前端",
        glyph: "前",
        ink: ink(0x4f, 0xa8, 0xe8),
        template: include_str!("../roles/office-frontend.md"),
    },
    Post {
        role: Role::Backend,
        id: "backend",
        zh: "后端",
        glyph: "后",
        ink: ink(0x3f, 0xb8, 0x9c),
        template: include_str!("../roles/office-backend.md"),
    },
    Post {
        role: Role::Qa,
        id: "qa",
        zh: "测试",
        glyph: "测",
        ink: ink(0x8c, 0xc0, 0x4f),
        template: include_str!("../roles/office-qa.md"),
    },
    Post {
        role: Role::Devops,
        id: "devops",
        zh: "运维",
        glyph: "运",
        ink: ink(0xc9, 0xa2, 0x3a),
        template: include_str!("../roles/office-devops.md"),
    },
    Post {
        role: Role::Reviewer,
        id: "reviewer",
        zh: "审查",
        glyph: "审",
        ink: ink(0x8a, 0x9b, 0xb0),
        template: include_str!("../roles/office-reviewer.md"),
    },
    Post {
        role: Role::Pm,
        id: "pm",
        zh: "项目经理",
        glyph: "管",
        ink: ink(0xe8, 0x5d, 0x5d),
        template: include_str!("../roles/office-pm.md"),
    },
];

/// Agent names carry this before the role's id, so a user's own agents
/// never read as one of ours.
const PREFIX: &str = "office-";

impl Role {
    pub fn post(self) -> &'static Post {
        ROSTER
            .iter()
            .find(|p| p.role == self)
            .expect("every role is on the roster")
    }

    pub fn of_id(id: &str) -> Option<Self> {
        ROSTER.iter().find(|p| p.id == id).map(|p| p.role)
    }

    /// The role a subagent type names: `office-qa`, or a plugin's
    /// `plugin:office-qa`. Built-in types (Explore, Plan, general-purpose)
    /// are no role.
    pub fn of_agent_type(t: &str) -> Option<Self> {
        let name = t.rsplit(':').next().unwrap_or(t);
        Self::of_id(name.strip_prefix(PREFIX)?)
    }

    pub fn agent_name(self) -> String {
        format!("{PREFIX}{}", self.post().id)
    }
}

/// A prompt opening with `[office:<id>]` names the role its session is to
/// take; the rest of the prompt follows it. Only at the very start and only
/// for a known id, so a prompt quoting a marker mid-text, or a typo, is
/// nobody's role.
pub fn parse_marker(text: &str) -> Option<(Role, &str)> {
    let rest = text.trim_start().strip_prefix("[office:")?;
    let (id, rest) = rest.split_once(']')?;
    Some((Role::of_id(id)?, rest))
}

/// The line a dispatched prompt's task starts on, after the role's card.
pub const TASK_HEAD: &str = "任务：";
/// What a prompt dispatched without a task says in its place.
pub const NO_TASK: &str = "还没定，先问我要做什么。";

/// The task in what follows a marker: past the card when the prompt was
/// dispatched with one, else all of it; `None` when there is none yet.
pub fn task_of(rest: &str) -> Option<&str> {
    let head = format!("\n{TASK_HEAD}");
    let task = rest.split_once(&*head).map_or(rest, |(_, t)| t).trim();
    (!task.is_empty() && task != NO_TASK).then_some(task)
}

fn frontmatter_end(template: &str) -> usize {
    template
        .strip_prefix("---\n")
        .and_then(|t| t.find("\n---\n"))
        .map_or(0, |i| i + "---\n".len() + "\n---\n".len())
}

/// The short part of a role's brief: what it does, without its checklist,
/// for a prompt to carry in full.
pub fn card(role: Role) -> &'static str {
    let t = role.post().template;
    let body = &t[frontmatter_end(t)..];
    body.split("\n## ").next().unwrap_or(body).trim()
}

/// What sits under our ownership mark: the text the hash in it covers.
const MARK: &str = "<!-- agent-office role=";

/// A role's file as installed: its template and our mark, with a hash of
/// the template so an edit by hand is told from a stale copy.
fn rendered(post: &Post) -> String {
    let body = post.template.trim_end();
    let sha = crate::tasks::memo::key(&[body]);
    format!("{body}\n{MARK}{} sha={sha} -->\n", post.id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Missing,
    /// Ours and as written: the current template, or an older one.
    Ours {
        current: bool,
    },
    /// Ours but changed by hand since.
    Edited,
    /// A file of the same name that is not ours.
    Foreign,
}

fn state_of(post: &Post, text: Option<&str>) -> State {
    let Some(text) = text else {
        return State::Missing;
    };
    let Some(at) = text.rfind(MARK) else {
        return State::Foreign;
    };
    let body = text[..at].trim_end();
    let sha = text[at..]
        .split("sha=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next());
    // The mark hashes the template it came from, which is all the body was
    // unless someone changed it.
    if sha != Some(crate::tasks::memo::key(&[body]).as_str()) {
        return State::Edited;
    }
    State::Ours {
        current: text == rendered(post),
    }
}

pub fn agents_dir() -> Result<PathBuf> {
    Ok(crate::tasks::claude_dir()
        .context("找不到 Claude 配置目录")?
        .join("agents"))
}

fn path_of(dir: &Path, post: &Post) -> PathBuf {
    dir.join(format!("{PREFIX}{}.md", post.id))
}

fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("读 {} 失败", path.display())),
    }
}

pub fn status(dir: &Path) -> Result<Vec<(&'static Post, State)>> {
    ROSTER
        .iter()
        .map(|p| Ok((p, state_of(p, read(&path_of(dir, p))?.as_deref()))))
        .collect()
}

/// Claude Code reads these while running: a rename never shows one half
/// written; a symlinked file (dotfiles) is written through so the link
/// survives.
fn write(path: &Path, text: &str) -> Result<()> {
    let linked = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let result = if linked {
        std::fs::write(path, text)
    } else {
        let tmp = path.with_extension("md.agent-office.tmp");
        std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path))
    };
    result.with_context(|| format!("写 {} 失败", path.display()))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub written: Vec<&'static str>,
    pub kept_edited: Vec<&'static str>,
    pub kept_foreign: Vec<&'static str>,
}

/// Write every role missing or out of date. One edited by hand is kept
/// unless `force`; a file of the same name that is not ours never is.
pub fn install(dir: &Path, force: bool) -> Result<Outcome> {
    std::fs::create_dir_all(dir).with_context(|| format!("建 {} 失败", dir.display()))?;
    let mut out = Outcome::default();
    for (post, state) in status(dir)? {
        match state {
            State::Ours { current: true } => continue,
            State::Foreign => {
                out.kept_foreign.push(post.id);
                continue;
            }
            State::Edited if !force => {
                out.kept_edited.push(post.id);
                continue;
            }
            _ => {}
        }
        write(&path_of(dir, post), &rendered(post))?;
        out.written.push(post.id);
    }
    Ok(out)
}

/// Delete our roles. One edited by hand is kept unless `force`; a file that
/// is not ours never goes.
pub fn remove(dir: &Path, force: bool) -> Result<Outcome> {
    let mut out = Outcome::default();
    for (post, state) in status(dir)? {
        match state {
            State::Missing => {}
            State::Foreign => out.kept_foreign.push(post.id),
            State::Edited if !force => out.kept_edited.push(post.id),
            _ => {
                let path = path_of(dir, post);
                std::fs::remove_file(&path)
                    .with_context(|| format!("删 {} 失败", path.display()))?;
                out.written.push(post.id);
            }
        }
    }
    Ok(out)
}

fn report(dir: &Path, verb: &str, out: &Outcome) {
    if out == &Outcome::default() {
        println!("没有要{verb}的");
    }
    if !out.written.is_empty() {
        println!("已{verb}：{}", out.written.join("、"));
    }
    if !out.kept_edited.is_empty() {
        println!(
            "改过的没动：{}（加 --force 覆盖）",
            out.kept_edited.join("、")
        );
    }
    if !out.kept_foreign.is_empty() {
        println!(
            "同名但不是 agent-office 写的，没动：{}",
            out.kept_foreign.join("、")
        );
    }
    println!("目录：{}", dir.display());
}

/// `agent-office roles [install|remove] [--force]`.
pub fn cli(args: &[&str]) -> Result<()> {
    let dir = agents_dir()?;
    match args {
        [] => {
            for (post, state) in status(&dir)? {
                let s = match state {
                    State::Missing => "没装",
                    State::Ours { current: true } => "已装",
                    State::Ours { current: false } => "已装（旧版，install 会升级）",
                    State::Edited => "已装（改过）",
                    State::Foreign => "同名文件不是 agent-office 的",
                };
                println!("{:<17} {} {s}", post.role.agent_name(), post.zh);
            }
            println!("目录：{}", dir.display());
        }
        ["install", rest @ ..] if rest.iter().all(|a| *a == "--force") => {
            let out = install(&dir, rest.contains(&"--force"))?;
            report(&dir, "写入", &out);
            println!(
                "新开的 Claude 会话才看得到。项目里 .claude/agents/ 下的同名文件会盖过这里的。"
            );
        }
        ["remove", rest @ ..] if rest.iter().all(|a| *a == "--force") => {
            let out = remove(&dir, rest.contains(&"--force"))?;
            report(&dir, "删除", &out);
        }
        _ => anyhow::bail!("用法：agent-office roles [install|remove] [--force]"),
    }
    Ok(())
}

/// The doctor's line on the roles, and whether every one is in.
pub fn doctor_line() -> (bool, String) {
    let Ok(dir) = agents_dir() else {
        return (false, "找不到 Claude 配置目录".into());
    };
    let Ok(states) = status(&dir) else {
        return (false, format!("读不了 {}", dir.display()));
    };
    let count = |f: fn(&State) -> bool| states.iter().filter(|(_, s)| f(s)).count();
    let ours = count(|s| matches!(s, State::Ours { .. } | State::Edited));
    let edited = count(|s| *s == State::Edited);
    let stale = count(|s| *s == State::Ours { current: false });
    let mut line = format!("{ours}/{} 个已装在 {}", ROSTER.len(), dir.display());
    if edited > 0 {
        line += &format!("，{edited} 个改过");
    }
    if stale > 0 {
        line += &format!("，{stale} 个是旧版（agent-office roles install 升级）");
    }
    if ours == 0 {
        line += "；agent-office roles install 安装";
    }
    (
        ours == ROSTER.len(),
        line + "。项目级同名文件会盖过这里的，只对新开的会话生效",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agent-office-roles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Claude Code calls an agent by its frontmatter name, and both the
    /// office's detection and a PM's `subagent_type` use `office-<id>`: a
    /// template named otherwise would install a role nobody can reach or
    /// recognise.
    #[test]
    fn every_template_is_named_office_its_id_and_installs_with_our_mark() {
        for post in ROSTER {
            let name = format!("\nname: office-{}\n", post.id);
            assert!(post.template.starts_with("---\n"), "{}", post.id);
            assert!(post.template.contains(&name), "{}", post.id);
            assert!(post.template.contains("\ndescription: "), "{}", post.id);
            assert_eq!(
                state_of(post, Some(&rendered(post))),
                State::Ours { current: true },
                "{}",
                post.id
            );
            assert_eq!(
                Role::of_agent_type(&post.role.agent_name()),
                Some(post.role)
            );
        }
    }

    /// A dispatched prompt carries the card in full: it has to be the brief
    /// and not the checklist, and short enough not to eat the prompt's room.
    #[test]
    fn a_card_is_the_brief_without_frontmatter_or_checklist() {
        for post in ROSTER {
            let c = card(post.role);
            assert!(!c.contains("name:"), "{}", post.id);
            assert!(!c.contains("## "), "{}", post.id);
            assert!(c.starts_with("你是这间办公室的"), "{}", post.id);
            assert!(c.lines().count() <= 10, "{}", post.id);
        }
    }

    /// The PM's job is to hand work to the others: without the Agent tool
    /// it could only plan. A `tools:` list that forgot it would cut that off.
    #[test]
    fn the_pm_keeps_every_tool() {
        let t = Role::Pm.post().template;
        assert!(!t[..frontmatter_end(t)].contains("tools:"));
    }

    #[test]
    fn a_marker_counts_only_at_the_start_and_for_a_known_role() {
        assert_eq!(
            parse_marker("  [office:qa] 跑测试"),
            Some((Role::Qa, " 跑测试"))
        );
        assert_eq!(parse_marker("[office:xyz] 跑测试"), None);
        assert_eq!(parse_marker("请看 [office:qa] 这个标记"), None);
        assert_eq!(parse_marker("[office:qa 没闭合"), None);
    }

    /// Built-in agent types are how Claude Code explores and plans, not a
    /// post anyone holds: calling Explore a role would mislabel half the
    /// office's subagents.
    #[test]
    fn only_office_agent_types_are_roles() {
        assert_eq!(Role::of_agent_type("office-frontend"), Some(Role::Frontend));
        assert_eq!(Role::of_agent_type("myplugin:office-qa"), Some(Role::Qa));
        for t in [
            "Explore",
            "Plan",
            "general-purpose",
            "fork",
            "office-xyz",
            "frontend",
        ] {
            assert_eq!(Role::of_agent_type(t), None, "{t}");
        }
    }

    /// The agents directory is the user's: install must add and upgrade
    /// only what agent-office wrote, keep a hand edit unless told, and never
    /// overwrite the user's own file of the same name, even when forced.
    #[test]
    fn install_fills_the_gaps_and_never_touches_a_users_own_file() {
        let dir = temp_dir("install");
        std::fs::create_dir_all(&dir).unwrap();
        let qa = Role::Qa.post();
        let ux = Role::Ux.post();
        let pm = Role::Pm.post();
        std::fs::write(path_of(&dir, qa), "my own qa agent").unwrap();
        let edited = rendered(ux).replace("你是这间办公室的", "你是我的");
        std::fs::write(path_of(&dir, ux), &edited).unwrap();
        // An older template of ours: its mark still matches its body.
        let old = "---\nname: office-pm\n---\nold\n";
        let stale = format!(
            "{}\n{MARK}pm sha={} -->\n",
            old.trim_end(),
            crate::tasks::memo::key(&[old.trim_end()])
        );
        std::fs::write(path_of(&dir, pm), &stale).unwrap();

        let out = install(&dir, false).unwrap();
        assert_eq!(out.kept_foreign, ["qa"]);
        assert_eq!(out.kept_edited, ["ux"]);
        assert!(out.written.contains(&"pm"));
        assert_eq!(out.written.len(), ROSTER.len() - 2);
        assert_eq!(
            std::fs::read_to_string(path_of(&dir, pm)).unwrap(),
            rendered(pm)
        );
        assert_eq!(std::fs::read_to_string(path_of(&dir, ux)).unwrap(), edited);

        let out = install(&dir, true).unwrap();
        assert_eq!(out.written, ["ux"]);
        assert_eq!(
            std::fs::read_to_string(path_of(&dir, qa)).unwrap(),
            "my own qa agent"
        );
        // Run again, nothing left to do.
        assert_eq!(
            install(&dir, false).unwrap(),
            Outcome {
                kept_foreign: vec!["qa"],
                ..Outcome::default()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_takes_only_ours() {
        let dir = temp_dir("remove");
        install(&dir, false).unwrap();
        let qa = path_of(&dir, Role::Qa.post());
        std::fs::write(&qa, "my own qa agent").unwrap();
        let ux = path_of(&dir, Role::Ux.post());
        std::fs::write(&ux, rendered(Role::Ux.post()).replace("设计师", "画师")).unwrap();
        let out = remove(&dir, false).unwrap();
        assert_eq!(out.kept_foreign, ["qa"]);
        assert_eq!(out.kept_edited, ["ux"]);
        assert!(qa.exists() && ux.exists());
        assert!(!path_of(&dir, Role::Pm.post()).exists());
        remove(&dir, true).unwrap();
        assert!(qa.exists() && !ux.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
