//! DeepSeek Harness (`dsh`): `connect dsh` writes agent-office's cordis
//! plugin (`dsh_plugin.mjs`) under `$DSH_HOME` and mounts it through the
//! home-level `cordis.patch.yml`. The plugin forwards dsh's session events to
//! `agent-office hook --source dsh` and keeps a per-session task log the task
//! reader tails, since dsh's own session file is zstd frames no tail can read.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const SOURCE: &str = "dsh";

/// The cordis plugin id, also how our row in the patch file is found again.
const PLUGIN_ID: &str = "agent-office";
const TEMPLATE: &str = include_str!("dsh_plugin.mjs");

/// The package whose browser half opens a dispatched Session in the app. A
/// package, not a file: DSH serves browser code only from a `package.json`
/// declaring `dsh.client`, found from the mounted row's path.
const WEB_ID: &str = "agent-office-web";
const WEB_FILES: &[(&str, &str)] = &[
    ("package.json", include_str!("dsh_web/package.json")),
    ("host.mjs", include_str!("dsh_web/host.mjs")),
    ("client.js", include_str!("dsh_web/client.js")),
];

/// `$DSH_HOME`, else `~/.dsh`, as dsh resolves its home.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("DSH_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".dsh")))
}

fn our_dir() -> Option<PathBuf> {
    home().map(|h| h.join("agent-office"))
}

/// Where the plugin writes one `<sessionId>.jsonl` task log per session.
pub fn transcripts_dir() -> Option<PathBuf> {
    our_dir().map(|d| d.join("transcripts"))
}

/// Where the plugin keeps, a file per dsh host, the ids of the sessions a
/// dsh client shows now.
pub fn viewing_dir() -> Option<PathBuf> {
    our_dir().map(|d| d.join("viewing"))
}

/// Local requests from `dispatch --agent dsh` to a running DSH host.
pub fn dispatch_dir() -> Option<PathBuf> {
    our_dir().map(|d| d.join("dispatch"))
}

fn plugin_path(dir: &Path) -> PathBuf {
    dir.join("agent-office-dsh.mjs")
}

fn web_dir(dir: &Path) -> PathBuf {
    dir.join("web")
}

fn web_entry(dir: &Path) -> PathBuf {
    web_dir(dir).join("host.mjs")
}

fn patch_path(home: &Path) -> PathBuf {
    home.join("cordis.patch.yml")
}

fn plugin_source(exe: &Path, transcripts: &Path, viewing: &Path, dispatch: &Path) -> Result<String> {
    // JSON string literals are valid JS string literals.
    let js = |p: &Path| serde_json::to_string(&p.display().to_string());
    Ok(TEMPLATE
        .replace("__HOOK_EXE__", &js(exe)?)
        .replace("__TRANSCRIPTS__", &js(transcripts)?)
        .replace("__VIEWING__", &js(viewing)?)
        .replace("__DISPATCH__", &js(dispatch)?))
}

/// The patch file's top-level sequence items, each with the comment lines
/// above it. A file that is only `[]` (dsh's empty patch) has none.
fn items(text: &str) -> Vec<String> {
    if text.trim() == "[]" {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut pending = String::new();
    for line in text.lines() {
        let starts_item = line.starts_with("- ") || line == "-";
        let loose = line.trim().is_empty() || line.starts_with('#');
        if starts_item {
            out.push(std::mem::take(&mut pending) + line + "\n");
        } else if loose || out.is_empty() {
            pending.push_str(line);
            pending.push('\n');
        } else {
            let last = out.last_mut().expect("checked non-empty");
            last.push_str(&pending);
            pending.clear();
            last.push_str(line);
            last.push('\n');
        }
    }
    if !pending.trim().is_empty() {
        out.push(pending);
    }
    out
}

/// Whether an item is about our plugin alone: the insert row we wrote, or a
/// row dsh's plugin manager added to disable it. An item that also names
/// another plugin is not ours to drop.
fn is_plugin(item: &str, wanted: &str) -> bool {
    let ids: Vec<&str> = item
        .lines()
        .filter_map(|l| l.trim_start().trim_start_matches("- ").strip_prefix("id:"))
        .map(|v| v.trim().trim_matches('"'))
        .collect();
    !ids.is_empty() && ids.iter().all(|id| *id == wanted)
}

fn is_ours(item: &str) -> bool {
    is_plugin(item, PLUGIN_ID)
}

/// A non-empty flow sequence (`[ {id: x} ]`, which dsh's yaml writer keeps
/// when it adds a row to `[]`): a block item appended after it is invalid
/// YAML, and dsh would refuse to boot.
fn is_flow(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('[') && text.trim() != "[]"
}

fn without_plugin(text: &str, wanted: &str) -> String {
    // We only ever write block items, so a flow sequence holds none of ours.
    if is_flow(text) {
        return text.to_owned();
    }
    items(text).into_iter().filter(|i| !is_plugin(i, wanted)).collect()
}

/// What disconnect leaves: dsh's plugin manager refuses a patch that is not
/// a sequence, and a file with no items left is null, not `[]`.
fn after_unmount(text: &str, wanted: &str) -> String {
    let kept = without_plugin(text, wanted);
    let no_items = kept
        .lines()
        .all(|l| l.trim().is_empty() || l.starts_with('#'));
    if no_items && !text.trim().is_empty() {
        kept + "[]\n"
    } else {
        kept
    }
}

fn after_connect(text: &str, plugin: &Path, dir: &Path) -> Result<String> {
    with_plugin(&with_ours(text, plugin)?, WEB_ID, &web_entry(dir))
}

fn after_disconnect(text: &str) -> String {
    after_unmount(&after_unmount(text, PLUGIN_ID), WEB_ID)
}

fn with_plugin(text: &str, wanted: &str, plugin: &Path) -> Result<String> {
    anyhow::ensure!(
        !is_flow(text),
        crate::i18n::t(
            "cordis.patch.yml 是 [ ... ] 写法，改成每行一个 `- ` 的写法后再 connect dsh",
            "cordis.patch.yml uses the [ ... ] form; rewrite it with one `- ` item per line, then connect dsh again"
        )
    );
    let name = serde_json::to_string(&plugin.display().to_string())?;
    let mut out = without_plugin(text, wanted);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "- insert:\n    - id: {wanted}\n      name: {name}\n"
    ));
    Ok(out)
}

fn with_ours(text: &str, plugin: &Path) -> Result<String> {
    with_plugin(text, PLUGIN_ID, plugin)
}

fn read_patch(home: &Path) -> Result<String> {
    let path = patch_path(home);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => {
            Err(e).with_context(|| crate::tr!("读 {} 失败", "could not read {}", path.display()))
        }
    }
}

/// dsh reads the patch at boot: a rename never shows it half written.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("yml.agent-office.tmp");
    crate::atomic::write_keeping_mode(path, &tmp, text.as_bytes())
        .with_context(|| crate::tr!("写 {} 失败", "could not write {}", path.display()))
}

pub(crate) fn plugin_mounted(home: &Path, wanted: &str) -> bool {
    read_patch(home).is_ok_and(|text| items(&text).iter().any(|item| is_plugin(item, wanted)))
}

pub(crate) fn mount_plugin(home: &Path, wanted: &str, plugin: &Path) -> Result<()> {
    let before = read_patch(home)?;
    write_atomic(&patch_path(home), &with_plugin(&before, wanted, plugin)?)
}

pub(crate) fn unmount_plugin(home: &Path, wanted: &str) -> Result<()> {
    let before = read_patch(home)?;
    let after = after_unmount(&before, wanted);
    if after != before {
        write_atomic(&patch_path(home), &after)?;
    }
    Ok(())
}

/// `agent-office connect dsh`.
pub fn connect() -> Result<()> {
    let home = home().context(crate::i18n::t(
        "找不到 dsh 目录（DSH_HOME 或 ~/.dsh）",
        "cannot find the dsh directory (DSH_HOME or ~/.dsh)",
    ))?;
    let dir = our_dir().context(crate::i18n::t(
        "找不到 dsh 目录",
        "cannot find the dsh directory",
    ))?;
    let transcripts = transcripts_dir().context(crate::i18n::t(
        "找不到 dsh 目录",
        "cannot find the dsh directory",
    ))?;
    let viewing = viewing_dir().context(crate::i18n::t(
        "找不到 dsh 目录",
        "cannot find the dsh directory",
    ))?;
    let dispatch = dispatch_dir().context(crate::i18n::t(
        "找不到 dsh 目录",
        "cannot find the dsh directory",
    ))?;
    std::fs::create_dir_all(&transcripts)
        .with_context(|| crate::tr!("建 {} 失败", "could not create {}", transcripts.display()))?;
    let exe = crate::hook::exe_for_hooks()?;
    let plugin = plugin_path(&dir);
    std::fs::write(&plugin, plugin_source(&exe, &transcripts, &viewing, &dispatch)?)
        .with_context(|| crate::tr!("写 {} 失败", "could not write {}", plugin.display()))?;
    let web = web_dir(&dir);
    std::fs::create_dir_all(&web)
        .with_context(|| crate::tr!("建 {} 失败", "could not create {}", web.display()))?;
    // A running DSH may serve these as they change: never half a bundle.
    for (name, text) in WEB_FILES {
        let path = web.join(name);
        crate::atomic::write_keeping_mode(&path, &web.join(format!("{name}.tmp")), text.as_bytes())
            .with_context(|| crate::tr!("写 {} 失败", "could not write {}", path.display()))?;
    }
    let before = read_patch(&home)?;
    write_atomic(&patch_path(&home), &after_connect(&before, &plugin, &dir)?)?;
    println!(
        "{}",
        crate::tr!(
            "已挂上 {}；新开的 dsh 进程才生效。移动或删除这个程序后要重新 connect dsh。",
            "Mounted {}; it applies to new dsh processes. Run connect dsh again after moving or deleting this program.",
            plugin.display()
        )
    );
    Ok(())
}

/// `agent-office disconnect dsh`: unmount and delete the plugin. The task
/// logs stay: they are the user's sessions, not ours to drop.
pub fn disconnect() -> Result<()> {
    let home = home().context(crate::i18n::t(
        "找不到 dsh 目录（DSH_HOME 或 ~/.dsh）",
        "cannot find the dsh directory (DSH_HOME or ~/.dsh)",
    ))?;
    let before = read_patch(&home)?;
    let after = after_disconnect(&before);
    if after != before {
        write_atomic(&patch_path(&home), &after)?;
        println!(
            "{}",
            crate::tr!(
                "已从 {} 移除",
                "Removed from {}",
                patch_path(&home).display()
            )
        );
    }
    if let Some(dir) = our_dir() {
        let _ = std::fs::remove_file(plugin_path(&dir));
        let _ = std::fs::remove_dir_all(web_dir(&dir));
    }
    Ok(())
}

/// Bring the Harness window forward: its app answers `dsh://open` by raising
/// its main window. A dispatch that leaves the new session behind another
/// window reads as a dispatch that did nothing.
pub fn focus_app() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("dsh://open")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Whether the patch mounts our plugin and the plugin file is there.
/// Whether our plugin is mounted and forwards the turn's start: one written
/// by an older build lacks it, and its figures leave their desks between calls.
/// Nor does such a one say which session you are looking at, so a reply you
/// watched land in the app would wait in the pantry to be read. Nor one
/// without the browser half: its dispatches land unseen behind the session
/// you were reading.
pub fn installed() -> bool {
    let (Some(home), Some(dir)) = (home(), our_dir()) else {
        return false;
    };
    std::fs::read_to_string(plugin_path(&dir))
        .is_ok_and(|t| forwards_turn_start(&t) && says_what_you_view(&t) && supports_dispatch(&t))
        && web_current(&web_dir(&dir))
        && read_patch(&home).is_ok_and(|t| {
            let items = items(&t);
            items.iter().any(|i| is_ours(i)) && items.iter().any(|i| is_plugin(i, WEB_ID))
        })
}

/// Every browser-half file is there as this build writes it: a stale one
/// from an older build may not open what this build dispatches.
fn web_current(web: &Path) -> bool {
    WEB_FILES
        .iter()
        .all(|(name, text)| std::fs::read_to_string(web.join(name)).is_ok_and(|t| t == *text))
}

fn forwards_turn_start(plugin: &str) -> bool {
    plugin.contains("\"turn_start\"")
}

fn says_what_you_view(plugin: &str) -> bool {
    plugin.contains("const VIEWING = ")
}

/// One from before Workspaces creates Sessions no sidebar lists: once you
/// look away from a dispatch, it is gone.
fn supports_dispatch(plugin: &str) -> bool {
    plugin.contains("const DISPATCH = ")
        && plugin.contains("sessionController.create")
        && plugin.contains("workspaceId: workspace.id")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin from before the turn's start must read as not connected, so
    /// doctor sends the user back to `connect dsh`.
    #[test]
    fn a_plugin_without_the_turn_start_is_not_connected() {
        assert!(forwards_turn_start(TEMPLATE));
        assert!(!forwards_turn_start(&TEMPLATE.replace("\"turn_start\"", "\"x\"")));
        // Nor one from before it said which session you view.
        assert!(says_what_you_view(TEMPLATE));
        assert!(!says_what_you_view(
            &TEMPLATE.replace("const VIEWING = ", "const X = ")
        ));
        // Nor one from before dispatch: accepting work it cannot receive
        // would only time out in the spool.
        assert!(supports_dispatch(TEMPLATE));
        assert!(!supports_dispatch(&TEMPLATE.replace("sessionController.create", "sessionController.nope")));
        // Nor one that creates by cwd alone: its Sessions join no Workspace.
        assert!(!supports_dispatch(&TEMPLATE.replace("workspaceId: workspace.id", "cwd: request.cwd")));
    }

    /// pixtuoid's row, the user's own rows and their comments must survive a
    /// connect and a disconnect, and reconnecting must not stack a second row.
    #[test]
    fn connect_twice_then_disconnect_leaves_other_plugins_alone() {
        let original = "# mine\n- insert:\n    - id: pixtuoid\n      name: /p/pixtuoid-dsh.mjs\n- id: web\n  disabled: true\n";
        let plugin = Path::new("/h/.dsh/agent-office/agent-office-dsh.mjs");
        let once = with_ours(original, plugin).unwrap();
        let twice = with_ours(&once, plugin).unwrap();
        assert_eq!(once, twice);
        assert_eq!(twice.matches("id: agent-office").count(), 1);
        assert!(twice.contains("name: \"/h/.dsh/agent-office/agent-office-dsh.mjs\""));
        assert_eq!(without_plugin(&twice, PLUGIN_ID), original);
    }

    /// dsh writes `[]` for an empty patch; appending a row after it would be
    /// invalid YAML and dsh would refuse to boot.
    #[test]
    fn an_empty_flow_patch_becomes_a_block_sequence() {
        let p = Path::new("/x.mjs");
        assert_eq!(
            with_ours("[]\n", p).unwrap(),
            "- insert:\n    - id: agent-office\n      name: \"/x.mjs\"\n"
        );
        assert_eq!(with_ours("", p).unwrap(), with_ours("[]\n", p).unwrap());
    }

    /// A disable row dsh's plugin manager adds for our id goes with us, or a
    /// reconnect would mount the plugin and keep it switched off.
    #[test]
    fn disconnect_also_drops_the_plugin_managers_row_for_us() {
        let text = "- insert:\n    - id: agent-office\n      name: \"/x.mjs\"\n- id: agent-office\n  disabled: true\n";
        assert_eq!(after_disconnect(text), "[]\n");
    }

    /// A flow-style patch must be refused, not turned into invalid YAML that
    /// stops every dsh profile from booting.
    #[test]
    fn a_flow_style_patch_is_refused_and_left_alone() {
        let flow = "[ { id: web, disabled: true } ]\n";
        assert!(with_ours(flow, Path::new("/x.mjs")).is_err());
        assert_eq!(without_plugin(flow, PLUGIN_ID), flow);
    }

    /// Disconnect must not take another plugin's row with it.
    #[test]
    fn an_insert_shared_with_another_plugin_is_kept() {
        let text = "- insert:\n    - id: pixtuoid\n      name: /p.mjs\n    - id: agent-office\n      name: /x.mjs\n";
        assert_eq!(without_plugin(text, PLUGIN_ID), text);
    }

    /// Roles and event forwarding have independent lifetimes: removing the
    /// role presets must not disconnect the office's session events.
    #[test]
    fn removing_role_presets_keeps_the_event_plugin() {
        let base = with_ours("", Path::new("/events.mjs")).unwrap();
        let both = with_plugin(&base, "agent-office-roles", Path::new("/roles.mjs")).unwrap();
        assert_eq!(after_unmount(&both, "agent-office-roles"), base);
    }

    /// The browser half rides along with the event plugin: connect mounts
    /// both once however often it runs, and disconnect takes both, leaving
    /// the user's rows as they were.
    #[test]
    fn connect_mounts_the_browser_half_and_disconnect_takes_it() {
        let original = "- insert:\n    - id: pixtuoid\n      name: /p.mjs\n";
        let dir = Path::new("/h/.dsh/agent-office");
        let plugin = plugin_path(dir);
        let once = after_connect(original, &plugin, dir).unwrap();
        assert_eq!(after_connect(&once, &plugin, dir).unwrap(), once);
        assert_eq!(once.matches("id: agent-office-web").count(), 1);
        assert!(once.contains("name: \"/h/.dsh/agent-office/web/host.mjs\""));
        assert_eq!(after_disconnect(&once), original);
    }

    /// DSH loads a browser half only from a package whose manifest declares
    /// `dsh.client` for the web and exports `./client`, and registers it under
    /// the id the bundle names. Any mismatch and dispatches stop opening,
    /// silently: this pins what the spike proved DSH accepts.
    #[test]
    fn the_browser_package_is_shaped_as_dsh_loads_it() {
        let file = |name: &str| WEB_FILES.iter().find(|(n, _)| *n == name).unwrap().1;
        let pkg: serde_json::Value = serde_json::from_str(file("package.json")).unwrap();
        assert_eq!(pkg["dsh"]["client"]["platform"], "web");
        assert_eq!(pkg["exports"]["."], "./host.mjs");
        assert_eq!(pkg["exports"]["./client"], "./client.js");
        let id = pkg["name"].as_str().unwrap();
        assert!(file("client.js").contains(&format!("id: \"{id}\"")));
        // The mounted row resolves to the host half, whose nearest manifest
        // must be this package's.
        assert!(web_entry(Path::new("/d")).ends_with("web/host.mjs"));
        // It opens exactly the presets a dispatch creates, and never the
        // subagents a DSH PM spawns under those same presets.
        assert!(TEMPLATE.contains("const agentPreset = `office-${request.role}`"));
        assert!(file("client.js").contains("preset.startsWith(\"office-\")"));
        assert!(file("client.js").contains("row.origin !== \"subagent\""));
        // Nor an old dispatch a page sees only on reconnecting.
        assert!(file("client.js").contains("Date.now() - row.updatedAt < FRESH_MS"));
    }

    /// A browser half left by an older build reads as not connected, so the
    /// user is sent back to `connect dsh` instead of dispatching blind.
    #[test]
    fn a_stale_browser_half_is_not_current() {
        let dir = std::env::temp_dir().join(format!("ao-dsh-web-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!web_current(&dir));
        for (name, text) in WEB_FILES {
            std::fs::write(dir.join(name), text).unwrap();
        }
        assert!(web_current(&dir));
        std::fs::write(dir.join("client.js"), "// old").unwrap();
        assert!(!web_current(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Paths with quotes or backslashes must not break out of the JS string.
    #[test]
    fn the_plugin_embeds_paths_as_string_literals() {
        let src = plugin_source(
            Path::new("/a \"b\"/agent-office"),
            Path::new("/t"),
            Path::new("/v"),
            Path::new("/d"),
        )
        .unwrap();
        assert!(src.contains(r#"const HOOK_EXE = "/a \"b\"/agent-office""#));
        assert!(src.contains(r#"const TRANSCRIPTS = "/t""#));
        assert!(src.contains(r#"const VIEWING = "/v""#));
        assert!(src.contains(r#"const DISPATCH = "/d""#));
        assert!(!src.contains("__HOOK_EXE__") && !src.contains("__TRANSCRIPTS__"));
        assert!(!src.contains("__VIEWING__") && !src.contains("__DISPATCH__"));
    }
}
