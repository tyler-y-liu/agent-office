//! `agent-office doctor`: what the office can see, and why not. Read-only.

use std::path::Path;
use std::time::Instant;

use anyhow::Result;

use crate::tasks::client::{Summarizer, SummaryInput};

/// Whether something already listens on the hook socket (another
/// agent-office): two listeners would split the hook events between them.
pub fn socket_in_use(socket: &Path) -> Option<String> {
    #[cfg(unix)]
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return Some(format!(
            "已有程序在监听 {}（多半是另一个 agent-office 正在运行）。先退出它再启动。",
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
        "[{}] 会话记录  {}",
        ok(has_root),
        root.map_or_else(
            || "找不到 Claude 配置目录".into(),
            |r| r.display().to_string()
        )
    );

    if crate::hook::installed() {
        println!("[✓] hooks     已接入：实时状态和「等你确认」可用（新开的 Claude 会话才生效）");
    } else {
        println!(
            "[!] hooks     未接入：只靠会话记录也能用，但看不到「等你确认」。\n              \
             接入：agent-office connect（会改 ~/.claude/settings.json，先备份）"
        );
    }

    let socket = crate::hook::socket_path();
    match socket_in_use(&socket) {
        Some(w) => println!("[!] socket    {w}"),
        None => println!("[✓] socket    {} 空闲", socket.display()),
    }

    match Summarizer::from_env() {
        None => println!(
            "[!] 摘要      关闭（没配 AGENT_OFFICE_API_BASE / _API_KEY / _MODEL，或 AGENT_OFFICE_SUMMARY=off）：显示原始指令"
        ),
        Some(s) => {
            let rt = tokio::runtime::Runtime::new()?;
            let input = SummaryInput {
                project: "demo".into(),
                latest_prompt: "登录页点提交没反应，帮我查一下原因修好，再补个回归测试".into(),
                ..Default::default()
            };
            let t = Instant::now();
            match rt.block_on(s.summarize(&input)) {
                Ok(line) => println!(
                    "[✓] 摘要      {} 试跑 {:.1}s →「{line}」",
                    s.model(),
                    t.elapsed().as_secs_f32()
                ),
                Err(e) => println!("[✗] 摘要      {} 失败：{e:#}", s.model()),
            }
        }
    }
    Ok(())
}
