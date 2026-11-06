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
    pub en: &'static str,
    /// The one character its chip shows in Chinese.
    pub glyph: &'static str,
    /// What its chip shows in English: a CJK glyph means nothing to an
    /// English reader.
    pub abbr: &'static str,
    pub ink: Rgb,
    template: &'static str,
    template_en: &'static str,
}

/// What a role's chip sits on, its ink the text: a block of the ink itself
/// outweighed the status chip beside it.
pub const CHIP_BG: Rgb = ink(0x25, 0x2a, 0x33);

// Inks stay clear of the status colours (working green, stuck red, done
// yellow, waiting orange), since a chip sits next to its status everywhere.
const fn ink(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// In the picker's order: digits 1–9 pick these, 0 the PM.
pub const ROSTER: &[Post] = &[
    Post {
        role: Role::Product,
        id: "product",
        zh: "产品",
        en: "Product",
        glyph: "产",
        abbr: "PD",
        ink: ink(0xe0, 0x8f, 0x5a),
        template: include_str!("../roles/office-product.md"),
        template_en: include_str!("../roles/en/office-product.md"),
    },
    Post {
        role: Role::Ux,
        id: "ux",
        zh: "设计",
        en: "Design",
        glyph: "设",
        abbr: "UX",
        ink: ink(0xee, 0x86, 0xd0),
        template: include_str!("../roles/office-ux.md"),
        template_en: include_str!("../roles/en/office-ux.md"),
    },
    Post {
        role: Role::Architect,
        id: "architect",
        zh: "架构",
        en: "Architect",
        glyph: "架",
        abbr: "AR",
        ink: ink(0x7f, 0x8c, 0xf0),
        template: include_str!("../roles/office-architect.md"),
        template_en: include_str!("../roles/en/office-architect.md"),
    },
    Post {
        role: Role::Frontend,
        id: "frontend",
        zh: "前端",
        en: "Frontend",
        glyph: "前",
        abbr: "FE",
        ink: ink(0x4f, 0xa8, 0xe8),
        template: include_str!("../roles/office-frontend.md"),
        template_en: include_str!("../roles/en/office-frontend.md"),
    },
    Post {
        role: Role::Backend,
        id: "backend",
        zh: "后端",
        en: "Backend",
        glyph: "后",
        abbr: "BE",
        ink: ink(0x3f, 0xa8, 0xb8),
        template: include_str!("../roles/office-backend.md"),
        template_en: include_str!("../roles/en/office-backend.md"),
    },
    Post {
        role: Role::Qa,
        id: "qa",
        zh: "测试",
        en: "QA",
        glyph: "测",
        abbr: "QA",
        ink: ink(0x5c, 0xc8, 0xd8),
        template: include_str!("../roles/office-qa.md"),
        template_en: include_str!("../roles/en/office-qa.md"),
    },
    Post {
        role: Role::Devops,
        id: "devops",
        zh: "运维",
        en: "DevOps",
        glyph: "运",
        abbr: "OPS",
        ink: ink(0xc9, 0xa2, 0x3a),
        template: include_str!("../roles/office-devops.md"),
        template_en: include_str!("../roles/en/office-devops.md"),
    },
    Post {
        role: Role::Reviewer,
        id: "reviewer",
        zh: "审查",
        en: "Reviewer",
        glyph: "审",
        abbr: "RV",
        ink: ink(0x8a, 0x9b, 0xb0),
        template: include_str!("../roles/office-reviewer.md"),
        template_en: include_str!("../roles/en/office-reviewer.md"),
    },
    Post {
        role: Role::Pm,
        id: "pm",
        zh: "项目经理",
        en: "PM",
        glyph: "管",
        abbr: "PM",
        ink: ink(0xc4, 0x9c, 0xf0),
        template: include_str!("../roles/office-pm.md"),
        template_en: include_str!("../roles/en/office-pm.md"),
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

    /// What its chip shows, in the UI's language.
    pub fn chip(self) -> &'static str {
        let post = self.post();
        crate::i18n::t(post.glyph, post.abbr)
    }

    /// Its post in the UI's language.
    pub fn name(self) -> &'static str {
        let post = self.post();
        crate::i18n::t(post.zh, post.en)
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

/// The line a dispatched prompt's task starts on, after the role's card:
/// Chinese, then English.
const TASK_HEADS: [&str; 2] = ["任务：", "Task: "];
/// What a prompt dispatched without a task says in its place.
const NO_TASKS: [&str; 2] = ["还没定，先问我要做什么。", "Not set yet: ask me what to do first."];

fn task_head() -> &'static str {
    crate::i18n::t(TASK_HEADS[0], TASK_HEADS[1])
}

/// The task in what follows a marker: past the card when the prompt was
/// dispatched with one, else all of it; `None` when there is none yet.
/// Either language's head counts: a transcript outlives a change of
/// `AGENT_OFFICE_LANG`. The earliest one is the real head; a later one is
/// inside the task, e.g. a relayed reply in the other language.
pub fn task_of(rest: &str) -> Option<&str> {
    let task = TASK_HEADS
        .iter()
        .filter_map(|h| {
            let head = format!("\n{h}");
            rest.find(&*head).map(|i| (i, i + head.len()))
        })
        .min()
        .map_or(rest, |(_, end)| &rest[end..])
        .trim();
    (!task.is_empty() && !NO_TASKS.contains(&task)).then_some(task)
}

impl Post {
    /// Its agent file in the UI's language, so a role answers in the
    /// language its user reads.
    fn template(&self) -> &'static str {
        crate::i18n::t(self.template, self.template_en)
    }
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
    let t = role.post().template();
    let body = &t[frontmatter_end(t)..];
    body.split("\n## ").next().unwrap_or(body).trim()
}

/// What sits under our ownership mark: the text the hash in it covers.
const MARK: &str = "<!-- agent-office role=";

/// A role's file as installed: its template and our mark, with a hash of
/// the template so an edit by hand is told from a stale copy.
fn rendered(post: &Post) -> String {
    render(post, post.template())
}

fn render(post: &Post, template: &str) -> String {
    let body = template.trim_end();
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
    /// Ours and current, but in the other language: `install` brings it
    /// over to the UI's, yet it is no older version.
    OtherLang,
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
    let Some(end) = text[at..].find("-->").map(|i| at + i + "-->".len()) else {
        return State::Edited;
    };
    let sha = text[at..end]
        .split("sha=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next());
    // The mark hashes the template it came from, which is all the body was
    // unless someone changed it; anything added after the mark is the
    // user's too, and a rewrite or delete would lose it.
    if sha != Some(crate::tasks::memo::key(&[body]).as_str()) || !text[end..].trim().is_empty() {
        return State::Edited;
    }
    if text == rendered(post) {
        return State::Ours { current: true };
    }
    let other = crate::i18n::t(post.template_en, post.template);
    if text == render(post, other) {
        return State::OtherLang;
    }
    State::Ours { current: false }
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
                    State::OtherLang => "已装（另一种语言的版本，install 会换成界面语言）",
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
    let ours = count(|s| matches!(s, State::Ours { .. } | State::OtherLang | State::Edited));
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
    let task = if task.is_empty() {
        crate::i18n::t(NO_TASKS[0], NO_TASKS[1])
    } else {
        task
    };
    let card = if with_card {
        format!("\n{}\n", card(role))
    } else {
        String::new()
    };
    let label = crate::i18n::t("角色：", "Role: ");
    format!(
        "[office:{}] {label}{}\n{card}\n{}{task}",
        post.id,
        role.name(),
        task_head()
    )
}

/// The card a relay picks up from: what the selected session came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    /// Its turn is over and its result unread.
    Done,
    Stuck,
}

/// Who to hand on to, best first: finished work goes to be tested and
/// reviewed; stuck work to its own trade and to the architect. Never the
/// session's own post for done work, which would only redo it.
pub fn next_posts(after: After, own: Option<Role>) -> Vec<Role> {
    let wanted = match after {
        After::Done => vec![Role::Qa, Role::Reviewer],
        After::Stuck => own
            .filter(|r| *r != Role::Pm)
            .into_iter()
            .chain([Role::Architect])
            .collect(),
    };
    let mut out: Vec<Role> = Vec::new();
    for r in wanted {
        if !out.contains(&r) && !(after == After::Done && Some(r) == own) {
            out.push(r);
        }
    }
    out
}

/// How much of the earlier session's task and last reply a relay carries: enough to
/// go on, short of crowding out the role's card.
const RELAY_REPLY_CHARS: usize = 2000;
/// Of that, how much comes from the start; the rest is the end, where a
/// hand-back puts what was not done and the risks.
const RELAY_HEAD_CHARS: usize = 300;

/// `text` within `RELAY_REPLY_CHARS`: its start and its end, saying how much
/// of the middle was left out, so the next post knows it is not all.
fn clip(text: &str) -> String {
    let n = text.chars().count();
    if n <= RELAY_REPLY_CHARS {
        return text.to_owned();
    }
    let tail = RELAY_REPLY_CHARS - RELAY_HEAD_CHARS;
    let head: String = text.chars().take(RELAY_HEAD_CHARS).collect();
    let end: String = text.chars().skip(n - tail).collect();
    let cut = n - RELAY_REPLY_CHARS;
    let mark = crate::tr!("…（中间省略 {} 字）…", "… ({} characters cut) …", cut);
    format!("{head}\n{mark}\n{end}")
}

/// The task a relay hands on: what the earlier session was asked, what it
/// came to (or where it stuck), and where its changes are, so the next
/// post starts from that instead of asking what to do.
pub fn relay_task(
    name: &str,
    after: After,
    task: Option<&str>,
    came_to: Option<&str>,
    stuck: Option<&str>,
    cwd: &Path,
) -> String {
    let mut out = crate::tr!(
        "接着「{}」的活往下做。\n",
        "Pick up where \"{}\" left off.\n",
        name
    );
    if let Some(t) = task.map(str::trim).filter(|t| !t.is_empty()) {
        let t = clip(t);
        out.push_str(&crate::tr!("它的任务：{}\n", "Its task: {}\n", t));
    }
    if let (After::Stuck, Some(why)) = (after, stuck) {
        out.push_str(&crate::tr!("它卡住了：{}\n", "It got stuck: {}\n", why));
    }
    if let Some(c) = came_to.map(str::trim).filter(|c| !c.is_empty()) {
        let c = clip(c);
        let head = match after {
            After::Done => crate::i18n::t("它的结论：", "What it came to: "),
            After::Stuck => crate::i18n::t("它最后说的：", "The last it said: "),
        };
        out.push_str(&format!("{head}{c}\n"));
    }
    out.push_str(&crate::tr!(
        "它的改动在：{}\n先看它改了什么（git status / git diff），再做你这个角色该做的。",
        "Its changes are in: {}\nLook at what it changed first (git status / git diff), then do your role's part.",
        cwd.display()
    ));
    out
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
        .then(|| {
            crate::i18n::t(
                "角色还没装：先运行 agent-office roles install",
                "roles not installed: run agent-office roles install first",
            )
            .to_owned()
        })
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
    folder_ready(dir)?;
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
    crate::jump::open_url_checked(&link).map_err(anyhow::Error::msg)?;
    Ok(format!(
        "已在 Claude 打开新会话（{}，{}），确认后发送",
        role.post().zh,
        dir.display()
    ))
}

/// Kept apart from `dispatch` so a test can check it without the link ever
/// being opened. A relay's folder can be a worktree removed since; another
/// folder would put the work in the wrong repository, so none is guessed.
fn folder_ready(dir: &Path) -> Result<()> {
    anyhow::ensure!(
        dir.is_dir(),
        crate::tr!(
            "目录 {} 已经不在了（worktree 删了？），没有派",
            "folder {} is gone (worktree removed?); nothing dispatched",
            dir.display()
        )
    );
    Ok(())
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
    use crate::i18n::{with, Lang};

    const LANGS: [Lang; 2] = [Lang::Zh, Lang::En];

    #[test]
    fn every_template_is_named_office_its_id_and_installs_with_our_mark() {
        for lang in LANGS {
            with(lang, || {
                for post in ROSTER {
                    let t = post.template();
                    let name = format!("\nname: office-{}\n", post.id);
                    assert!(t.starts_with("---\n"), "{} {lang:?}", post.id);
                    assert!(t.contains(&name), "{} {lang:?}", post.id);
                    assert!(t.contains("\ndescription: "), "{} {lang:?}", post.id);
                    assert_eq!(
                        state_of(post, Some(&rendered(post))),
                        State::Ours { current: true },
                        "{} {lang:?}",
                        post.id
                    );
                    assert_eq!(
                        Role::of_agent_type(&post.role.agent_name()),
                        Some(post.role)
                    );
                }
            });
        }
    }

    /// The English file is a translation, not a different role: a reviewer
    /// or architect that could edit in one language but not the other
    /// would break the read-only promise for half the users, and a missing
    /// section would leave a PM checking hand-backs that lack it.
    #[test]
    fn both_languages_grant_the_same_tools_and_keep_the_same_sections() {
        let meta = |t: &'static str| -> Vec<&'static str> {
            t[..frontmatter_end(t)]
                .lines()
                .filter(|l| !l.starts_with("description:"))
                .collect()
        };
        let sections = |t: &str| t.matches("\n## ").count();
        for post in ROSTER {
            assert_eq!(meta(post.template), meta(post.template_en), "{}", post.id);
            assert_eq!(
                sections(post.template),
                sections(post.template_en),
                "{}",
                post.id
            );
            // The last thing a role writes is what the PM and the next
            // post read: every role says how to write it.
            assert!(post.template.contains("\n## 交回格式\n"), "{}", post.id);
            assert!(post.template_en.contains("\n## Hand-back format\n"), "{}", post.id);
        }
    }

    /// A dispatched prompt carries the card in full: it has to be the brief
    /// and not the checklist, and short enough not to eat the prompt's room.
    #[test]
    fn a_card_is_the_brief_without_frontmatter_or_checklist() {
        for (lang, opening) in [(Lang::Zh, "你是这间办公室的"), (Lang::En, "You are the office's")] {
            with(lang, || {
                for post in ROSTER {
                    let c = card(post.role);
                    assert!(!c.contains("name:"), "{} {lang:?}", post.id);
                    assert!(!c.contains("## "), "{} {lang:?}", post.id);
                    assert!(c.starts_with(opening), "{} {lang:?}", post.id);
                    assert!(c.lines().count() <= 10, "{} {lang:?}", post.id);
                }
            });
        }
    }

    /// The PM's job is to hand work to the others: without the Agent tool
    /// it could only plan. A `tools:` list that forgot it would cut that off.
    #[test]
    fn the_pm_keeps_every_tool() {
        for t in [Role::Pm.post().template, Role::Pm.post().template_en] {
            assert!(!t[..frontmatter_end(t)].contains("tools:"));
        }
    }

    /// Roles installed in one language and the UI switched to the other:
    /// `install` brings them over, since a role answers in the language of
    /// its file, but a hand edit still stays.
    #[test]
    fn install_switches_the_roles_language_but_keeps_a_hand_edit() {
        let dir = temp_dir("lang");
        with(Lang::Zh, || install(&dir, false).unwrap());
        let ux = path_of(&dir, Role::Ux.post());
        std::fs::write(&ux, rendered(Role::Ux.post()).replace("设计师", "画师")).unwrap();
        with(Lang::En, || {
            let pm = Role::Pm.post();
            assert_eq!(
                state_of(pm, read(&path_of(&dir, pm)).unwrap().as_deref()),
                State::OtherLang
            );
            let out = install(&dir, false).unwrap();
            assert_eq!(out.written.len(), ROSTER.len() - 1);
            assert_eq!(out.kept_edited, ["ux"]);
            let text = std::fs::read_to_string(path_of(&dir, pm)).unwrap();
            assert!(text.contains("You are the office's project manager"));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A transcript outlives a change of language: a session dispatched in
    /// English still reads back as its role and task in a Chinese office,
    /// and an English empty dispatch is still no task.
    #[test]
    fn a_prompt_in_either_language_reads_back_in_the_other() {
        let (en, en_blank) = with(Lang::En, || {
            (
                prompt(Role::Backend, "add a /health endpoint", true),
                prompt(Role::Qa, "", false),
            )
        });
        assert!(en.starts_with("[office:backend] Role: Backend\n"), "{en}");
        assert!(en.contains("You are the office's backend engineer"));
        with(Lang::Zh, || {
            let (role, rest) = parse_marker(&en).unwrap();
            assert_eq!((role, task_of(rest)), (Role::Backend, Some("add a /health endpoint")));
            let (_, rest) = parse_marker(&en_blank).unwrap();
            assert_eq!(task_of(rest), None);
        });
        let zh = prompt(Role::Qa, "跑测试", true);
        with(Lang::En, || {
            let (_, rest) = parse_marker(&zh).unwrap();
            assert_eq!(task_of(rest), Some("跑测试"));
        });
        // A relayed Chinese reply inside an English task is part of the task.
        let mixed = with(Lang::En, || prompt(Role::Qa, "check it\n任务：上一棒的原话", true));
        let (_, rest) = parse_marker(&mixed).unwrap();
        assert_eq!(task_of(rest), Some("check it\n任务：上一棒的原话"));
    }

    /// An English relay must brief the next post as fully as a Chinese one.
    #[test]
    fn an_english_relay_carries_what_came_before() {
        let task = with(Lang::En, || {
            relay_task(
                "api",
                After::Stuck,
                Some("fix login"),
                Some("tried twice"),
                Some("same test failed 5 times"),
                Path::new("/work/api"),
            )
        });
        for part in ["\"api\"", "fix login", "tried twice", "same test failed 5 times", "/work/api", "git diff"] {
            assert!(task.contains(part), "{part} missing from {task}");
        }
        assert!(!task.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "{task}");
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

    /// Lines added under our mark are the user's: install used to read the
    /// file as an old version of ours and rewrite it, and remove to delete
    /// it, losing them either way.
    #[test]
    fn lines_added_after_our_mark_keep_the_file_from_install_and_remove() {
        let dir = temp_dir("appended");
        install(&dir, false).unwrap();
        let qa = path_of(&dir, Role::Qa.post());
        let appended = format!("{}\n## 我自己加的\n- 先跑 make lint\n", rendered(Role::Qa.post()));
        std::fs::write(&qa, &appended).unwrap();
        assert_eq!(state_of(Role::Qa.post(), Some(&appended)), State::Edited);
        let out = install(&dir, false).unwrap();
        assert_eq!((out.written.len(), out.kept_edited.clone()), (0, vec!["qa"]));
        assert_eq!(std::fs::read_to_string(&qa).unwrap(), appended);
        let out = remove(&dir, false).unwrap();
        assert_eq!(out.kept_edited, ["qa"]);
        assert_eq!(std::fs::read_to_string(&qa).unwrap(), appended);
        // Whitespace after the mark is no edit.
        let spaced = format!("{}\n\n", rendered(Role::Qa.post()));
        assert_eq!(state_of(Role::Qa.post(), Some(&spaced)), State::Ours { current: false });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Roles installed in Chinese and the UI switched to English are not an
    /// older version: calling them that sent users to upgrade what was
    /// already current.
    #[test]
    fn a_role_in_the_other_language_is_not_called_old() {
        let dir = temp_dir("otherlang");
        with(Lang::Zh, || install(&dir, false).unwrap());
        with(Lang::En, || {
            for (post, state) in status(&dir).unwrap() {
                assert_eq!(state, State::OtherLang, "{}", post.id);
            }
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worktree merged and removed leaves its worker done on the card: a
    /// relay from it must fail saying so, not open a session in a folder
    /// that is not there or in some other repository.
    #[test]
    fn dispatch_to_a_folder_that_is_gone_fails() {
        let gone = temp_dir("gone");
        let err = folder_ready(&gone).unwrap_err().to_string();
        assert!(err.contains(&gone.display().to_string()), "{err}");
        assert!(folder_ready(&std::env::temp_dir()).is_ok());
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
    /// Finished work goes on to be checked, never back to its own post;
    /// stuck work to someone of its trade and to the architect.
    /// A chip sits beside a status everywhere: its ink must read on the
    /// chip (AA), and must not pass for working green or stuck red, which
    /// the old QA green and PM red did.
    #[test]
    fn role_inks_read_on_their_chip_and_stay_clear_of_status_colours() {
        fn lum(c: Rgb) -> f64 {
            let f = |v: u8| {
                let x = f64::from(v) / 255.0;
                if x <= 0.039_28 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
        }
        let dist = |a: Rgb, b: Rgb| {
            let d = |x: u8, y: u8| (f64::from(x) - f64::from(y)).powi(2);
            (d(a.r, b.r) + d(a.g, b.g) + d(a.b, b.b)).sqrt()
        };
        let working = ink(0x9e, 0xce, 0x6a);
        let stuck = ink(0xf7, 0x76, 0x8e);
        for p in ROSTER {
            let contrast = (lum(p.ink) + 0.05) / (lum(CHIP_BG) + 0.05);
            assert!(contrast >= 4.5, "{} {contrast:.2}", p.id);
            assert!(dist(p.ink, working) > 60.0, "{} reads as working", p.id);
            assert!(dist(p.ink, stuck) > 60.0, "{} reads as stuck", p.id);
        }
    }

    /// An English UI shows a Latin chip: a CJK glyph means nothing there.
    #[test]
    fn every_role_has_a_latin_chip() {
        for p in ROSTER {
            assert!(p.abbr.chars().all(|c| c.is_ascii_uppercase()), "{}", p.id);
            assert!((2..=3).contains(&p.abbr.len()), "{}", p.id);
        }
    }

    #[test]
    fn relay_suggests_the_next_post() {
        assert_eq!(next_posts(After::Done, None), vec![Role::Qa, Role::Reviewer]);
        assert_eq!(next_posts(After::Done, Some(Role::Qa)), vec![Role::Reviewer]);
        assert_eq!(
            next_posts(After::Stuck, Some(Role::Backend)),
            vec![Role::Backend, Role::Architect]
        );
        assert_eq!(next_posts(After::Stuck, None), vec![Role::Architect]);
        // A stuck PM needs a design look, not a second PM.
        assert_eq!(next_posts(After::Stuck, Some(Role::Pm)), vec![Role::Architect]);
    }

    /// A relay must start the next post on the work, not on a question:
    /// it carries the task, what came of it and where the changes are, and
    /// still fits the desktop app's prompt with the role's card.
    #[test]
    fn a_relay_task_carries_what_came_before() {
        let dir = Path::new("/work/shop-app");
        let task = relay_task(
            "docs-site",
            After::Done,
            Some("审一下安装文档的改动"),
            Some("审完了，两处命令写错"),
            None,
            dir,
        );
        for part in ["docs-site", "审一下安装文档的改动", "两处命令写错", "/work/shop-app"] {
            assert!(task.contains(part), "{part} missing from {task}");
        }
        let stuck = relay_task("api", After::Stuck, None, None, Some("同一个测试连失败 5 次"), dir);
        assert!(stuck.contains("同一个测试连失败 5 次"));
        // A hand-back ends on what was not done and the risks: a long
        // reply keeps its end, and says that its middle was cut.
        let reply = format!("开头{}没做的：压测；风险：迁移不可逆", "中".repeat(5000));
        let task = relay_task("api", After::Done, None, Some(&reply), None, dir);
        assert!(task.contains("开头") && task.contains("风险：迁移不可逆"), "{task}");
        assert!(task.contains("中间省略"), "{task}");
        assert!(task.chars().count() < 2300);
        let en = with(Lang::En, || relay_task("api", After::Done, None, Some(&reply), None, dir));
        assert!(en.contains("characters cut") && en.contains("风险：迁移不可逆"), "{en}");
        // A pasted prompt of any length still makes a link.
        let long = "长".repeat(20_000);
        let task = relay_task("api", After::Done, Some(&long), Some(&long), None, dir);
        for post in ROSTER {
            assert!(desktop_link(dir, &prompt(post.role, &task, true)).is_ok(), "{}", post.id);
        }
        // Read back, the relay is a task, so the new session is no empty dispatch.
        let p = prompt(Role::Qa, &task, true);
        let (_, rest) = parse_marker(&p).unwrap();
        assert!(task_of(rest).is_some());
    }

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
