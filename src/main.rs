//! agent-office: pixtuoid's pixel office in an always-on-top window, with
//! every agent's task in one line over its head and in a panel beside it.

mod codex;
mod core;
mod cursor;
mod desktop;
mod doctor;
mod dsh;
mod floating;
mod fun;
mod hook;
mod jump;
mod notify;
mod panel;
mod pipeline;
mod roles;
mod tasks;
mod text;

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `floating` was the window's subcommand while a terminal front end
    // existed; still accepted so old invocations keep working.
    let args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .skip_while(|a| *a == "floating")
        .collect();
    match args.first() {
        Some(&"hook") => {
            let source = match args.get(1..) {
                Some(["--source", s, ..]) => Some(*s),
                _ => None,
            };
            hook::forward(source);
            return Ok(());
        }
        Some(&cmd @ ("connect" | "disconnect")) => {
            return match (cmd, args.get(1)) {
                ("connect", None) => hook::connect(),
                ("disconnect", None) => hook::disconnect(),
                ("connect", Some(&"dsh")) => dsh::connect(),
                ("disconnect", Some(&"dsh")) => dsh::disconnect(),
                ("connect", Some(&"codex")) => codex::connect(),
                ("disconnect", Some(&"codex")) => codex::disconnect(),
                ("connect", Some(&"cursor")) => cursor::connect(),
                ("disconnect", Some(&"cursor")) => cursor::disconnect(),
                // A typo must not touch Claude Code's hooks instead.
                (_, Some(other)) => {
                    anyhow::bail!(
                        "不认识 {other}：只有 {cmd}、{cmd} dsh、{cmd} codex、{cmd} cursor"
                    )
                }
                _ => unreachable!(),
            };
        }
        Some(&"doctor") => return doctor::run(),
        Some(&"roles") => return roles::cli(&args[1..]),
        _ => {}
    }
    let socket = hook::socket_path();
    hook::prepare_socket_dir(&socket)?;
    if let Some(warning) = doctor::socket_in_use(&socket) {
        eprintln!("{warning}");
        return Ok(());
    }
    // The windows look out on Victoria Harbour, so the sky keeps Hong Kong's
    // time whatever zone the machine is in.
    pixtuoid_scene::sky::set_clock_zone(fun::utc_offset());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let shot = (args.first() == Some(&"--shot"))
        .then(|| args.get(1).map_or("floating.png".into(), Into::into));
    floating::run(&rt, shot)
}
