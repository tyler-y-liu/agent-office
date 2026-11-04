//! agent-office: pixtuoid's pixel office in an always-on-top window, with
//! every agent's task in one line over its head and in a panel beside it.

mod core;
mod doctor;
mod floating;
mod hook;
mod jump;
mod panel;
mod pipeline;
mod tasks;
mod text;

use anyhow::Result;

/// Seconds east of UTC.
const HONG_KONG_UTC_OFFSET: i32 = 8 * 3600;

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
            hook::forward();
            return Ok(());
        }
        Some(&"connect") => return hook::connect(),
        Some(&"disconnect") => return hook::disconnect(),
        Some(&"doctor") => return doctor::run(),
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
    let offset = std::env::var("AGENT_OFFICE_UTC_OFFSET")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|o| o.abs() < 24 * 3600)
        .unwrap_or(HONG_KONG_UTC_OFFSET);
    pixtuoid_scene::sky::set_clock_zone(offset);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let shot = (args.first() == Some(&"--shot"))
        .then(|| args.get(1).map_or("floating.png".into(), Into::into));
    floating::run(&rt, shot)
}
