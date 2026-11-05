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

/// The prompt that starts a session in `role`: the marker the office reads
/// its post from, then the role's card when the session will not load the
/// agent itself (the desktop app takes no agent), then the task. Without a
/// task it says to ask for one, so a link sent as it is still makes sense.
pub fn prompt(role: Role, task: &str, with_card: bool) -> String {
    let post = role.post();
    let task = task.trim();
    let task = if task.is_empty() { NO_TASK } else { task };
    let card = if with_card {
        format!("\n{}\n", card(role))
    } else {
        String::new()
    };
    format!("[office:{}] 角色：{}\n{card}\n{TASK_HEAD}{task}", post.id, post.zh)
}

/// The most a `claude://code/new` prompt may hold: the desktop app cuts
/// one longer to this many UTF-16 units, and a cut brief is worse than none.
const DESKTOP_PROMPT_MAX: usize = 14_336;

/// RFC 3986 percent-encoding of everything but the unreserved characters:
/// a `+` for a space would reach the app as a plus.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The link that opens the desktop app's new session in `dir` with
/// `prompt` filled in for the user to send.
pub fn desktop_link(dir: &Path, prompt: &str) -> Result<String> {
    let units = prompt.encode_utf16().count();
    anyhow::ensure!(
        units <= DESKTOP_PROMPT_MAX,
        "任务太长（{units} 字符，桌面 app 最多 {DESKTOP_PROMPT_MAX}）：加 --terminal 在终端开"
    );
    Ok(format!(
        "claude://code/new?folder={}&q={}",
        encode(&dir.to_string_lossy()),
        encode(prompt)
    ))
}

/// The main checkout of the repository `dir` is in: a session dispatched
/// from a worktree's figure must not edit that worktree under its owner.
/// `dir` itself when it is in no repository.
pub fn repo_root(dir: &Path) -> PathBuf {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output();
    let common = match out {
        Ok(o) if o.status.success() => PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()),
        _ => return dir.to_path_buf(),
    };
    match common.file_name() {
        Some(n) if n == ".git" => common.parent().map_or(dir.to_path_buf(), Path::to_path_buf),
        // A bare repository has no checkout to send anyone to.
        _ => dir.to_path_buf(),
    }
}

/// What `agent-office dispatch` was asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Dispatch {
    pub role: Role,
    pub dir: Option<PathBuf>,
    pub task: String,
    pub terminal: bool,
}

fn roster_ids() -> String {
    ROSTER.iter().map(|p| p.id).collect::<Vec<_>>().join("、")
}

const DISPATCH_USAGE: &str =
    "用法：agent-office dispatch (--role <角色> | --auto) [--dir <目录>] [--terminal] [任务…]";

/// `dispatch`'s arguments: a role or `--auto` (the PM), and the words left
/// over are the task.
pub fn parse_dispatch(args: &[&str]) -> Result<Dispatch> {
    let (mut role, mut dir, mut terminal, mut words) = (None, None, false, Vec::new());
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--role" => {
                let id = it.next().context(DISPATCH_USAGE)?;
                let r = Role::of_id(id).with_context(|| {
                    format!("没有 {id} 这个角色，可选：{}", roster_ids())
                })?;
                anyhow::ensure!(role.is_none(), "--role 和 --auto 只能选一个");
                role = Some(r);
            }
            "--auto" => {
                anyhow::ensure!(role.is_none(), "--role 和 --auto 只能选一个");
                role = Some(Role::Pm);
            }
            "--dir" => dir = Some(PathBuf::from(it.next().context(DISPATCH_USAGE)?)),
            "--terminal" => terminal = true,
            w => words.push(w),
        }
    }
    let role = role.with_context(|| format!("{DISPATCH_USAGE}\n角色：{}", roster_ids()))?;
    Ok(Dispatch {
        role,
        dir,
        task: words.join(" "),
        terminal,
    })
}

/// Why `role` cannot be dispatched yet, if it cannot: the PM hands work to
/// the other roles as agents, which must be installed; a terminal session
/// loads its own role as one.
pub fn not_ready(role: Role, terminal: bool) -> Option<String> {
    let dir = agents_dir().ok()?;
    let states = status(&dir).ok()?;
    let missing = |r: Role| {
        states
            .iter()
            .any(|(p, s)| p.role == r && *s == State::Missing)
    };
    let needed: Vec<Role> = if role == Role::Pm {
        ROSTER.iter().map(|p| p.role).collect()
    } else if terminal {
        vec![role]
    } else {
        Vec::new()
    };
    needed
        .into_iter()
        .any(missing)
        .then(|| "角色还没装：先运行 agent-office roles install".to_owned())
}

/// The arguments that start `claude` as `role` on `task`; the agent file
/// brings the brief, so the prompt carries only the marker and the task.
pub fn terminal_args(role: Role, task: &str) -> Vec<String> {
    let post = role.post();
    let name = format!("{}·{}", post.glyph, crate::text::truncate_cells(task.trim(), 24));
    vec![
        "--agent".into(),
        role.agent_name(),
        "-n".into(),
        name,
        prompt(role, task, false),
    ]
}

/// Where `claude` is: on PATH, else where its installers put it. A shell
/// alias or function of the same name is not seen from here.
fn claude_bin() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("claude"));
    let known = [".claude/local/claude", ".local/bin/claude"]
        .into_iter()
        .filter_map(|rel| home.as_ref().map(|h| h.join(rel)))
        .chain(["/opt/homebrew/bin/claude", "/usr/local/bin/claude"].map(PathBuf::from));
    on_path.chain(known).find(|p| p.is_file())
}

/// Start a session as `role` on `task` in `dir`: in the desktop app, its new
/// session opened with the prompt filled in; or here in the terminal.
pub fn dispatch(role: Role, dir: &Path, task: &str, terminal: bool) -> Result<String> {
    if let Some(why) = not_ready(role, terminal) {
        anyhow::bail!(why);
    }
    if terminal {
        let bin = claude_bin().context("找不到 claude 命令：装好 Claude Code 或去掉 --terminal")?;
        std::env::set_current_dir(&dir)
            .with_context(|| format!("进不了 {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&bin)
                .args(terminal_args(role, task))
                .exec();
            return Err(err).with_context(|| format!("启动 {} 失败", bin.display()));
        }
        #[cfg(not(unix))]
        anyhow::bail!("--terminal 只支持 macOS / Linux");
    }
    let link = desktop_link(&dir, &prompt(role, task, true))?;
    crate::jump::open_url(&link).map_err(anyhow::Error::msg)?;
    Ok(format!(
        "已在 Claude 打开新会话（{}，{}），确认后发送",
        role.post().zh,
        dir.display()
    ))
}

/// `agent-office dispatch`.
pub fn dispatch_cli(args: &[&str]) -> Result<()> {
    let d = parse_dispatch(args)?;
    // A directory named is taken as it is, a worktree included; run from
    // inside a worktree, the session goes to its repository instead.
    let dir = match d.dir {
        Some(p) => p
            .canonicalize()
            .with_context(|| format!("没有 {} 这个目录", p.display()))?,
        None => repo_root(&std::env::current_dir()?),
    };
    println!("{}", dispatch(d.role, &dir, &d.task, d.terminal)?);
    Ok(())
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
    /// The desktop app reads the link's query as form data would not: a
    /// `+` stays a plus, and a raw `&` or `#` would cut the prompt short.
    #[test]
    fn a_dispatch_link_is_percent_encoded_and_never_uses_plus() {
        let link = desktop_link(Path::new("/tmp/my repo"), "a b&c#d\n任务+").unwrap();
        assert_eq!(
            link,
            "claude://code/new?folder=%2Ftmp%2Fmy%20repo&q=a%20b%26c%23d%0A%E4%BB%BB%E5%8A%A1%2B"
        );
    }

    /// The app would cut a longer prompt silently, losing the task at its
    /// end: refused instead, with the way round it.
    #[test]
    fn a_prompt_past_the_apps_limit_is_refused_not_cut() {
        let long = "长".repeat(DESKTOP_PROMPT_MAX + 1);
        let err = desktop_link(Path::new("/tmp"), &long).unwrap_err();
        assert!(err.to_string().contains("--terminal"));
    }

    #[test]
    fn every_role_card_leaves_room_for_a_long_task() {
        let task = "改".repeat(2000);
        for post in ROSTER {
            assert!(desktop_link(Path::new("/tmp"), &prompt(post.role, &task, true)).is_ok());
        }
    }

    /// What dispatch writes is what the office reads back: a dispatched
    /// session wears its role and shows its task, not the card.
    #[test]
    fn a_dispatched_prompt_reads_back_as_its_role_and_task() {
        for with_card in [true, false] {
            let mut t = crate::tasks::extract::Transcript::default();
            let line = serde_json::json!({"type":"user","message":{"role":"user",
                "content": prompt(Role::Frontend, "设置页加暗色模式", with_card)}});
            t.feed(&line.to_string());
            assert_eq!(t.role, Some(Role::Frontend));
            assert_eq!(t.latest_prompt(), Some("设置页加暗色模式"));
        }
        let blank = prompt(Role::Qa, "  ", true);
        let (role, rest) = parse_marker(&blank).unwrap();
        assert_eq!((role, task_of(rest)), (Role::Qa, None), "no task is no task");
    }

    #[test]
    fn dispatch_args_take_one_role_and_the_rest_is_the_task() {
        let d = parse_dispatch(&["--role", "qa", "跑", "测试", "--dir", "/x"]).unwrap();
        assert_eq!(
            d,
            Dispatch {
                role: Role::Qa,
                dir: Some("/x".into()),
                task: "跑 测试".into(),
                terminal: false
            }
        );
        let auto = parse_dispatch(&["--auto", "--terminal", "做个功能"]).unwrap();
        assert_eq!((auto.role, auto.terminal), (Role::Pm, true));
        assert!(parse_dispatch(&["--role", "qa", "--auto"]).is_err());
        let unknown = parse_dispatch(&["--role", "tester"]).unwrap_err().to_string();
        assert!(unknown.contains("qa") && unknown.contains("frontend"), "{unknown}");
        assert!(parse_dispatch(&["做个功能"]).is_err(), "no role, no guess");
    }

    /// A terminal session loads the role as an agent, so the office must
    /// still see the marker (the agent setting leaves no trace) and the
    /// prompt need not repeat the brief.
    #[test]
    fn a_terminal_launch_names_the_agent_and_marks_the_prompt() {
        let args = terminal_args(Role::Reviewer, "审一下登录改动");
        assert_eq!(&args[..2], ["--agent", "office-reviewer"]);
        assert_eq!(args[2], "-n");
        assert!(args[3].starts_with("审·"));
        let p = args.last().unwrap();
        assert_eq!(parse_marker(p).map(|(r, rest)| (r, task_of(rest))), Some((Role::Reviewer, Some("审一下登录改动"))));
        assert!(!p.contains(card(Role::Reviewer)));
    }

    /// Two sessions in one worktree overwrite each other's edits: a figure
    /// in a worktree dispatches to its repository's main checkout.
    #[test]
    fn a_worktree_dispatches_into_its_repository() {
        let root = temp_dir("repo");
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "git {args:?}");
        };
        git(&main, &["init", "-q"]);
        git(&main, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"]);
        let wt = root.join("wt");
        git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
        let main = main.canonicalize().unwrap();
        assert_eq!(repo_root(&wt), main);
        assert_eq!(repo_root(&main), main);
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(repo_root(&plain), plain);
        let _ = std::fs::remove_dir_all(&root);
    }
}
