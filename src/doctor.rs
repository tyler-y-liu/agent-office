//! `agent-office doctor`: what the office can see, and why not. Read-only.

use std::path::Path;
use std::time::Instant;

use anyhow::Result;

use crate::i18n::t;
use crate::tasks::client::{Summarizer, SummaryInput};
use crate::tr;

/// Whether something already listens on the hook socket (another
/// agent-office): two listeners would split the hook events between them.
pub fn socket_in_use(socket: &Path) -> Option<String> {
    #[cfg(unix)]
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return Some(tr!(
            "已有程序在监听 {}（多半是另一个 agent-office 正在运行）。先退出它再启动。",
            "Something already listens on {} (most likely another agent-office). Quit it first.",
            socket.display()
        ));
    }
    None
}

pub fn run() -> Result<()> {
    let ok = |b: bool| if b { "✓" } else { "✗" };
    let root = crate::tasks::projects_root();
    let has_root = root.as_ref().is_some_and(|r| r.is_dir());
    println!(
        "[{}] {}  {}",
        ok(has_root),
        t("会话记录", "sessions"),
        root.map_or_else(
            || t(
                "找不到 Claude 配置目录",
                "cannot find the Claude config directory"
            )
            .into(),
            |r| r.display().to_string()
        )
    );

    println!(
        "[✓] {}",
        t(
            "语言      简体中文（窗口里按 l 或设 AGENT_OFFICE_LANG=en 换成英文）",
            "language  English (press l in the window, or AGENT_OFFICE_LANG=zh, for Chinese)"
        )
    );

    if crate::hook::installed() {
        println!(
            "{}",
            t(
                "[✓] hooks     已接入：实时状态和「等你确认」可用（新开的 Claude 会话才生效）",
                "[✓] hooks     connected: live state and \"waiting for approval\" work (in new Claude sessions)"
            )
        );
    } else {
        println!(
            "{}",
            t(
                "[!] hooks     未接入或不全：只靠会话记录也能用，但看不到「等你确认」，小人在两次调用之间会离开工位。\n              \
                 接入：agent-office connect（会改 ~/.claude/settings.json，先备份）",
                "[!] hooks     missing or partial: transcripts alone still work, but \"waiting for approval\" never shows and workers leave their desks between tool calls.\n              \
                 To connect: agent-office connect (edits ~/.claude/settings.json, backing it up first)"
            )
        );
    }

    if crate::dsh::installed() {
        println!(
            "{}",
            t(
                "[✓] dsh       插件已挂上：DeepSeek Harness 会话也会进办公室（新开的 dsh 进程才生效）",
                "[✓] dsh       plugin mounted: DeepSeek Harness sessions join the office (in new dsh processes)"
            )
        );
    } else {
        println!(
            "{}",
            t(
                "[ ] dsh       未接入；用 DeepSeek Harness 的话：agent-office connect dsh",
                "[ ] dsh       not connected; if you use DeepSeek Harness: agent-office connect dsh"
            )
        );
    }

    if crate::codex::installed() {
        println!(
            "{}",
            t(
                "[✓] codex     插件已装上：Codex 会话能显示「等你确认」（新开的 Codex 会话才生效）",
                "[✓] codex     plugin installed: Codex sessions show \"waiting for approval\" (in new Codex sessions)"
            )
        );
    } else {
        println!(
            "{}",
            t(
                "[ ] codex     未接入 hooks；Codex 会话照样进办公室，但看不到「等你确认」：agent-office connect codex",
                "[ ] codex     hooks not connected; Codex sessions still join the office, but \"waiting for approval\" never shows: agent-office connect codex"
            )
        );
    }

    if crate::cursor::installed() {
        println!(
            "{}",
            t(
                "[✓] cursor    hooks 已接入：cursor-agent 会话也会进办公室（新开的才生效）",
                "[✓] cursor    hooks connected: cursor-agent sessions join the office (new ones)"
            )
        );
    } else {
        println!(
            "{}",
            t(
                "[ ] cursor    未接入；用 Cursor 的 cursor-agent 的话：agent-office connect cursor",
                "[ ] cursor    not connected; if you use Cursor's cursor-agent: agent-office connect cursor"
            )
        );
    }

    let (all_in, roles) = crate::roles::doctor_line(crate::roles::Backend::Claude);
    println!("[{}] roles     {roles}", if all_in { "✓" } else { " " });
    let (cursor_roles_in, cursor_roles) = crate::roles::doctor_line(crate::roles::Backend::Cursor);
    println!(
        "[{}] cursor roles {cursor_roles}",
        if cursor_roles_in { "✓" } else { " " }
    );
    let (dsh_roles_in, dsh_roles) = crate::dsh_roles::doctor_line();
    println!(
        "[{}] dsh roles {dsh_roles}",
        if dsh_roles_in { "✓" } else { " " }
    );

    let socket = crate::hook::socket_path();
    match socket_in_use(&socket) {
        Some(w) => println!("[!] socket    {w}"),
        None => println!(
            "[✓] socket    {}",
            tr!("{} 空闲", "{} is free", socket.display())
        ),
    }

    let (limits, bad) = crate::tasks::StuckLimits::from_env();
    let minutes = |d: std::time::Duration| d.as_secs() / 60;
    let line = tr!(
        "连续失败 {} 次，或 {} 分钟没动静（构建、子 agent 这类长调用 {} 分钟）算卡住",
        "{} failures in a row, or {} minutes with no activity ({} minutes during long calls like builds or subagents), counts as stuck",
        limits.fails,
        minutes(limits.quiet),
        minutes(limits.quiet_long_call)
    );
    let stuck = t("卡点      ", "stuck     ");
    if bad.is_empty() {
        println!("[✓] {stuck}{line}");
    } else {
        println!(
            "[!] {stuck}{}",
            tr!(
                "{} 不是大于 0 的整数，用的默认值：{line}",
                "{} is not an integer above 0, so the default applies: {line}",
                bad.join(t("、", ", "))
            )
        );
    }

    let summary = t("摘要      ", "summary   ");
    match Summarizer::from_env() {
        None => println!(
            "[!] {summary}{}",
            t(
                "关闭（没配 AGENT_OFFICE_API_BASE / _API_KEY / _MODEL，或 AGENT_OFFICE_SUMMARY=off）：显示原始指令",
                "off (AGENT_OFFICE_API_BASE / _API_KEY / _MODEL not set, or AGENT_OFFICE_SUMMARY=off): raw prompts are shown"
            )
        ),
        Some(s) => {
            let rt = tokio::runtime::Runtime::new()?;
            let input = SummaryInput {
                project: "demo".into(),
                latest_prompt: t(
                    "登录页点提交没反应，帮我查一下原因修好，再补个回归测试",
                    "Clicking submit on the login page does nothing; find out why, fix it, and add a regression test",
                )
                .into(),
                ..Default::default()
            };
            let started = Instant::now();
            match rt.block_on(s.summarize(&input)) {
                Ok(line) => println!(
                    "[✓] {summary}{}",
                    tr!(
                        "{} 试跑 {:.1}s →「{line}」",
                        "{} test run {:.1}s → \"{line}\"",
                        s.model(),
                        started.elapsed().as_secs_f32()
                    )
                ),
                Err(e) => println!(
                    "[✗] {summary}{}",
                    tr!("{} 失败：{e:#}", "{} failed: {e:#}", s.model())
                ),
            }
        }
    }
    Ok(())
}
