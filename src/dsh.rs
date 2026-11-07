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

/// The Session a click asked to see, left for this Host's page: DSH has no
/// deep link to one, so the page asks its own Host for the id.
fn jump_file(dir: &Path) -> PathBuf {
    dir.join("jump.json")
}

/// How long a click's Session waits for the page: long enough for a window
/// being raised, short enough that a page opened much later does not jump.
const JUMP_FOR: std::time::Duration = std::time::Duration::from_secs(60);

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

fn plugin_source(
    exe: &Path,
    transcripts: &Path,
    viewing: &Path,
    dispatch: &Path,
    jump: &Path,
) -> Result<String> {
    // JSON string literals are valid JS string literals.
    let js = |p: &Path| serde_json::to_string(&p.display().to_string());
    Ok(TEMPLATE
        .replace("__HOOK_EXE__", &js(exe)?)
        .replace("__TRANSCRIPTS__", &js(transcripts)?)
        .replace("__VIEWING__", &js(viewing)?)
        .replace("__DISPATCH__", &js(dispatch)?)
        .replace("__JUMP__", &js(jump)?))
}

/// What a click leaves for the Host whose process start is `marker`: both a
/// desktop app and a `dsh web` can share one DSH_HOME, so the pid and marker
/// name the one whose page may consume it. It expires: a click on a page that
/// only opens later must not switch to a Session long after.
fn jump_json(
    session_id: &str,
    pid: i32,
    marker: u64,
    now: std::time::SystemTime,
) -> serde_json::Value {
    let expires_at = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .saturating_add(JUMP_FOR)
        .as_millis();
    serde_json::json!({
        "sessionId": session_id,
        "pid": pid,
        "startMarker": marker,
        "expiresAt": u64::try_from(expires_at).unwrap_or(u64::MAX),
    })
}

/// A click's Session left for a Host's page, and the bytes written, so the
/// click can tell whether a page took it. The Host removes the file as it
/// serves it, so a file still ours after the wait means no page was polling.
#[derive(Debug)]
pub struct Jump {
    path: PathBuf,
    written: Vec<u8>,
}

impl Jump {
    /// Whether a page took the click within `wait`. One nobody took is
    /// dropped: a page opened much later must not jump on an old click.
    pub fn taken(&self, wait: std::time::Duration) -> bool {
        wait_taken(&self.path, &self.written, wait)
    }
}

/// Poll for `path` to stop holding `ours`; a file another click replaced is
/// not ours to judge or remove.
fn wait_taken(path: &Path, ours: &[u8], wait: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        match std::fs::read(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Ok(text) if text != ours => return false,
            _ => {}
        }
        if start.elapsed() >= wait {
            if std::fs::read(path).is_ok_and(|text| text == ours) {
                let _ = std::fs::remove_file(path);
            }
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Leave `session_id` for the Host process `pid` to show in its page. Only
/// that Host's page opens it; a later click replaces this one.
pub fn request_jump(session_id: &str, pid: i32) -> Result<Jump> {
    anyhow::ensure!(
        !session_id.is_empty() && session_id.len() <= 200,
        crate::i18n::t("这个会话没有 id", "this session has no id")
    );
    let dir = our_dir().context(crate::i18n::t(
        "找不到 dsh 目录",
        "cannot find the dsh directory",
    ))?;
    // The page compares this with its own process start: without it no page
    // can tell this Host's incarnation from one that reused its pid.
    let marker = pixtuoid_core::source::pid_start_marker(pid).context(crate::i18n::t(
        "认不出运行这个会话的 dsh 进程",
        "cannot identify the dsh process running this session",
    ))?;
    std::fs::create_dir_all(&dir)
        .with_context(|| crate::tr!("建 {} 失败", "could not create {}", dir.display()))?;
    let written = serde_json::to_vec(&jump_json(
        session_id,
        pid,
        marker,
        std::time::SystemTime::now(),
    ))?;
    let path = jump_file(&dir);
    crate::atomic::write_private(&path, &dir.join("jump.json.tmp"), &written)
        .with_context(|| crate::tr!("写 {} 失败", "could not write {}", path.display()))?;
    Ok(Jump { path, written })
}

/// The patch file's top-level sequence items, each with the comment lines
/// above it. File-leading trivia stays separate so it survives unmounting.
/// A file that is only `[]` (dsh's empty patch) has none.
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
            // File-leading trivia belongs to the document, not the first row:
            // removing that row must not discard a marker or the user's comments.
            if out.is_empty() && !pending.is_empty() && pending.lines().all(flow_trivia_line) {
                out.push(std::mem::take(&mut pending));
            }
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

/// Whether `wanted` has a live insert not disabled by dsh's plugin manager.
/// The manager's disable is a separate item, so it must override every item
/// for that id rather than being mistaken for another way to mount it.
fn plugin_active(items: &[String], wanted: &str) -> bool {
    let mut inserted = false;
    for item in items.iter().filter(|item| is_plugin(item, wanted)) {
        for line in item.lines() {
            let line = line.trim_start();
            if line.starts_with('#') {
                continue;
            }
            let field = line.strip_prefix("- ").unwrap_or(line).trim_start();
            inserted |= field == "insert:";
            if let Some(value) = field.strip_prefix("disabled:") {
                let value = value
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim_matches(['"', '\'']);
                if value.eq_ignore_ascii_case("true") {
                    return false;
                }
            }
        }
    }
    inserted
}

fn flow_trivia_line(line: &str) -> bool {
    line.trim().is_empty() || line.trim_start().starts_with('#') || line.trim() == "---"
}

/// Split leading whitespace, whole-line comments and document starts from a
/// top-level flow sequence without interpreting the YAML values themselves.
fn flow_parts(text: &str) -> Option<(&str, &str)> {
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if rest.starts_with('[') {
            return Some((&text[..text.len() - rest.len()], rest));
        }
        let (line, next) = rest.split_once('\n')?;
        if flow_trivia_line(line) {
            rest = next;
        } else {
            return None;
        }
    }
}

/// A non-empty flow sequence (`[ {id: x} ]`, which dsh's yaml writer keeps
/// when it adds a row to `[]`): a block item appended after it is invalid
/// YAML, and dsh would refuse to boot.
fn is_flow(text: &str) -> bool {
    flow_parts(text).is_some_and(|(_, flow)| flow.trim() != "[]")
}

fn without_plugin(text: &str, wanted: &str) -> String {
    // We only ever write block items, so a flow sequence holds none of ours.
    if is_flow(text) {
        return text.to_owned();
    }
    // Replacing an empty flow must remove its marker but keep its trivia.
    if let Some((prefix, _)) = flow_parts(text) {
        return prefix.to_owned();
    }
    items(text).into_iter().filter(|i| !is_plugin(i, wanted)).collect()
}

/// What disconnect leaves: dsh's plugin manager refuses a patch that is not
/// a sequence, and a file with no items left is null, not `[]`.
fn after_unmount(text: &str, wanted: &str) -> String {
    let kept = without_plugin(text, wanted);
    let no_items = kept.lines().all(flow_trivia_line);
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
    read_patch(home).is_ok_and(|text| plugin_active(&items(&text), wanted))
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
    std::fs::write(
        &plugin,
        plugin_source(&exe, &transcripts, &viewing, &dispatch, &jump_file(&dir))?,
    )
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
    if let Ok(child) = std::process::Command::new("open")
        .arg("dsh://open")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        drop(reap_in_background(child));
    }
}

/// `open` returns after handing off the URL. Wait off the window's thread so
/// focusing neither stalls a frame nor leaves that short-lived child a zombie.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn reap_in_background(
    mut child: std::process::Child,
) -> std::thread::JoinHandle<std::io::Result<std::process::ExitStatus>> {
    std::thread::spawn(move || child.wait())
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
    let forwards = std::fs::read_to_string(plugin_path(&dir)).is_ok_and(|t| {
        forwards_turn_start(&t)
            && says_what_you_view(&t)
            && supports_dispatch(&t)
            && supports_jump(&t)
    });
    forwards
        && web_current(&web_dir(&dir))
        && read_patch(&home).is_ok_and(|t| {
            let items = items(&t);
            plugin_active(&items, PLUGIN_ID) && plugin_active(&items, WEB_ID)
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

/// After a crash, a live pid can belong to a new host while its old viewing
/// file remains. Only a written marker derived from the process start lets
/// desktop reject that stale file, so pid-only plugin versions need reconnecting.
fn says_what_you_view(plugin: &str) -> bool {
    plugin.contains("const VIEWING = ")
        && derives_process_start_marker(plugin)
        && wraps_direct_follow(plugin)
        && plugin.contains("START_MARKER === undefined")
        && plugin.contains(concat!(
            "JSON.stringify({ pid: process.pid, startMarker: START_MARKER, ",
            "sessions: [...open.keys()] })"
        ))
}

fn wraps_direct_follow(plugin: &str) -> bool {
    [
        "const controller = ctx.sessionController",
        "const follow = controller?.follow",
        "controller.follow = wrapped",
        "if (wrapped !== undefined && controller?.follow === wrapped) controller.follow = follow",
        "yield* follow.call(this, request, signal)",
    ]
    .into_iter()
    .all(|code| plugin.contains(code))
}

fn derives_process_start_marker(plugin: &str) -> bool {
    // `ps lstart` supplies the same start second as the kernel on macOS; a
    // clock reading or unrelated id cannot identify this incarnation.
    [
        "const START_MARKER = (() => {",
        r#"execFileSync("ps", ["-p", String(process.pid), "-o", "lstart="]"#,
        "Date.parse(started)",
        "Math.floor(milliseconds / 1000)",
        "Number.isSafeInteger(marker)",
    ]
    .into_iter()
    .all(|code| plugin.contains(code))
}

/// One from before Workspaces creates Sessions no sidebar lists: once you
/// look away from a dispatch, it is gone.
fn supports_dispatch(plugin: &str) -> bool {
    plugin.contains("const DISPATCH = ")
        && plugin.contains("sessionController.create")
        && plugin.contains("workspaceId: workspace.id")
}

/// One from before the jump route leaves a click's Session unopened: the
/// page would keep the window it had, which reads as a jump to nothing.
fn supports_jump(plugin: &str) -> bool {
    plugin.contains("const JUMP = ") && plugin.contains("connection.fetch.register")
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
        // Nor one from before dispatch: accepting work it cannot receive
        // would only time out in the spool.
        assert!(supports_dispatch(TEMPLATE));
        assert!(!supports_dispatch(&TEMPLATE.replace("sessionController.create", "sessionController.nope")));
        // Nor one that creates by cwd alone: its Sessions join no Workspace.
        assert!(!supports_dispatch(&TEMPLATE.replace("workspaceId: workspace.id", "cwd: request.cwd")));
        // Nor one without the jump route: a click would raise the window and
        // leave the Session it named unopened.
        assert!(supports_jump(TEMPLATE));
        assert!(!supports_jump(&TEMPLATE.replace("const JUMP = ", "const JUMP_PATH = ")));
        assert!(!supports_jump(&TEMPLATE.replace("connection.fetch.register", "connection.rpc.handle")));
    }

    /// A click's Session is left for one Host only: a desktop app and a
    /// `dsh web` can share DSH_HOME, so the pid and its process start say
    /// which page may open it, and an old click expires instead of opening
    /// a Session whenever that page next loads.
    #[test]
    fn a_jump_names_its_host_and_expires() {
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100);
        let v = jump_json("session-a", 42, 7, now);
        assert_eq!(v["sessionId"], "session-a");
        assert_eq!(v["pid"], 42);
        assert_eq!(v["startMarker"], 7);
        assert_eq!(v["expiresAt"], 160_000, "60s later, in ms");
    }

    /// A click is taken by the page that polls for it, and dropped once no
    /// page took it: a Session left on disk must not open long after the
    /// click, and a newer click is never ours to drop.
    #[test]
    fn a_click_is_taken_or_dropped() {
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("ao-jump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jump.json");

        // The Host's route removes the file as it serves the click.
        std::fs::write(&path, b"ours").unwrap();
        let served = path.clone();
        let page = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            std::fs::remove_file(served).unwrap();
        });
        assert!(wait_taken(&path, b"ours", Duration::from_secs(2)));
        page.join().unwrap();

        std::fs::write(&path, b"ours").unwrap();
        assert!(!wait_taken(&path, b"ours", Duration::from_millis(60)), "no page");
        assert!(!path.exists(), "an untaken click is dropped");

        std::fs::write(&path, b"newer").unwrap();
        assert!(!wait_taken(&path, b"ours", Duration::from_millis(60)));
        assert_eq!(std::fs::read(&path).unwrap(), b"newer", "a newer click stays");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A pre-marker or partially backported plugin writes files desktop must
    /// reject after a pid is reused. It is stale, however much of the newer
    /// source (or a START_MARKER mention) remains alongside the missing step.
    #[test]
    fn a_plugin_without_a_viewing_start_marker_is_not_connected() {
        assert!(says_what_you_view(TEMPLATE));
        for (version, source) in [
            (
                "before viewing support",
                TEMPLATE.replace("const VIEWING = ", "const X = "),
            ),
            (
                "pid-only viewing JSON",
                TEMPLATE.replace("startMarker: START_MARKER, ", ""),
            ),
            (
                "marker write deleted",
                TEMPLATE.replace("startMarker: START_MARKER, ", "startMarker: undefined, "),
            ),
            (
                "marker generation removed",
                TEMPLATE.replace(
                    "const START_MARKER = (() => {",
                    "const START_MARKER = undefined; (() => {",
                ),
            ),
            (
                "wall clock instead of process start",
                TEMPLATE.replace("Date.parse(started)", "Date.now()"),
            ),
            (
                "unreliable marker no longer suppresses writes",
                TEMPLATE.replace("START_MARKER === undefined", "false"),
            ),
        ] {
            assert!(!says_what_you_view(&source), "accepted {version}");
        }
    }

    /// Current DSH exposes follow directly on SessionController. A hybrid with
    /// new marker writes but the old history-only wrapper still produces no
    /// viewing updates, so doctor must require reconnecting it.
    #[test]
    fn viewing_requires_direct_session_controller_follow() {
        assert!(wraps_direct_follow(TEMPLATE));
        assert!(says_what_you_view(TEMPLATE));
        let history_only = TEMPLATE
            .replace(
                "const controller = ctx.sessionController\n    const follow = controller?.follow",
                "const history = ctx.sessionController?.history\n    const follow = history?.follow",
            )
            .replace("controller?.follow", "history?.follow")
            .replace("controller.follow", "history.follow");
        assert!(derives_process_start_marker(&history_only));
        assert!(!wraps_direct_follow(&history_only));
        assert!(!says_what_you_view(&history_only));
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

    /// A document marker is leading trivia, not a null YAML value. Unmounting
    /// must restore the valid empty flow instead of leaving only `---`.
    #[test]
    fn disconnect_preserves_document_markers_around_an_empty_flow() {
        for original in [
            "---\n[]\n",
            "# lead\n\n---\n[]\n",
            "---\n- insert:\n    - id: pixtuoid\n      name: /p.mjs\n",
        ] {
            assert_eq!(after_disconnect(original), original);
            let dir = Path::new("/h/.dsh/agent-office");
            let connected = after_connect(original, &plugin_path(dir), dir).unwrap();
            assert_eq!(after_disconnect(&connected), original);
        }
    }

    /// Only a live insert makes dispatch ready. The manager represents a
    /// disabled plugin with a separate row; treating that row as the mount
    /// would send work to a plugin that dsh will never start.
    #[test]
    fn only_an_enabled_insert_is_mounted() {
        let home = std::env::temp_dir().join(format!("ao-dsh-mounted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let patch = patch_path(&home);
        let normal = with_plugin("", PLUGIN_ID, Path::new("/x.mjs")).unwrap();
        std::fs::write(&patch, &normal).unwrap();
        assert!(plugin_mounted(&home, PLUGIN_ID));
        assert!(!plugin_mounted(&home, "pixtuoid"));

        let disabled = format!("{normal}- id: agent-office\n  disabled: true\n");
        std::fs::write(&patch, disabled).unwrap();
        assert!(!plugin_mounted(&home, PLUGIN_ID));

        std::fs::write(&patch, "- id: agent-office\n  disabled: true\n").unwrap();
        assert!(!plugin_mounted(&home, PLUGIN_ID));

        let other = with_plugin("", "pixtuoid", Path::new("/p.mjs")).unwrap();
        std::fs::write(
            &patch,
            format!("# - id: agent-office\n# disabled: true\n{other}"),
        )
        .unwrap();
        assert!(!plugin_mounted(&home, PLUGIN_ID));
        assert!(plugin_mounted(&home, "pixtuoid"));
        std::fs::remove_dir_all(home).unwrap();
    }

    /// A flow-style patch must be refused, not turned into invalid YAML that
    /// stops every dsh profile from booting.
    #[test]
    fn a_flow_style_patch_is_refused_and_left_alone() {
        let flow = "[ { id: web, disabled: true } ]\n";
        assert!(with_ours(flow, Path::new("/x.mjs")).is_err());
        assert_eq!(without_plugin(flow, PLUGIN_ID), flow);
    }

    /// Leading trivia is not a YAML value: hiding a flow sequence behind it
    /// and then appending a block item would still produce invalid YAML.
    #[test]
    fn a_non_empty_flow_after_leading_trivia_is_refused_and_left_alone() {
        let p = Path::new("/x.mjs");
        for flow in [
            "# comment\n[ { id: x } ]\n",
            "\n  # comment\n\n[ { id: x } ]\n",
            "---\n[ { id: x } ]\n",
            "# comment\n\n---\n\n[ { id: x } ]\n",
        ] {
            assert!(with_ours(flow, p).is_err(), "accepted {flow:?}");
            assert_eq!(without_plugin(flow, PLUGIN_ID), flow);
        }
    }

    /// The same leading trivia around dsh's empty `[]` must survive replacing
    /// the flow marker, without leaving `[]` next to the new block item.
    #[test]
    fn an_empty_flow_after_leading_trivia_becomes_a_block_sequence() {
        for (text, prefix) in [
            ("# comment\n[]\n", "# comment\n"),
            ("\n  # comment\n\n[]\n", "\n  # comment\n\n"),
            ("---\n[]\n", "---\n"),
            ("# comment\n\n---\n[]\n", "# comment\n\n---\n"),
        ] {
            assert_eq!(
                with_ours(text, Path::new("/x.mjs")).unwrap(),
                format!("{prefix}- insert:\n    - id: agent-office\n      name: \"/x.mjs\"\n")
            );
        }
    }

    /// Brackets in comments and mapping values do not make the top-level
    /// sequence flow-style, and ordinary block rows and comments stay intact.
    #[test]
    fn comments_and_plain_values_are_not_flow_sequences() {
        let block = "# [ a comment, not a sequence ]\n- insert:\n    - id: pixtuoid\n      name: /p.mjs\n";
        assert!(!is_flow("value: [ { id: x } ]\n"));
        let once = with_ours(block, Path::new("/x.mjs")).unwrap();
        assert!(once.starts_with(block));
        assert_eq!(without_plugin(&once, PLUGIN_ID), block);
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

    /// Focusing uses a short-lived `open` process. Its complete wait must be
    /// observable without launching a desktop app, or dropping the Child
    /// would leave a zombie every time the office raises DSH.
    #[cfg(unix)]
    #[test]
    fn the_focus_child_is_reaped_off_the_calling_thread() {
        let child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let status = reap_in_background(child).join().unwrap().unwrap();
        assert!(status.success());
    }

    /// DSH loads a browser half only from a package whose manifest declares
    /// `dsh.client` for the web and exports `./client`, and registers it under
    /// the id and with the services the bundle names. Any mismatch prevents
    /// DSH from installing the client, so this pins the package/loader shape.
    #[test]
    fn the_browser_package_is_shaped_as_dsh_loads_it() {
        let file = |name: &str| WEB_FILES.iter().find(|(n, _)| *n == name).unwrap().1;
        let pkg: serde_json::Value = serde_json::from_str(file("package.json")).unwrap();
        assert_eq!(pkg["dsh"]["client"]["platform"], "web");
        assert_eq!(
            pkg["dsh"]["client"]["inject"],
            serde_json::json!([
                "@deepseek-ai/dsh-api-session-controller",
                "@deepseek-ai/dsh-client-ui-workspace"
            ])
        );
        assert_eq!(pkg["exports"]["."], "./host.mjs");
        assert_eq!(pkg["exports"]["./client"], "./client.js");
        let client = file("client.js");
        let id = pkg["name"].as_str().unwrap();
        assert!(client.contains("window.__ModuleLoader__.load({"));
        assert!(client.contains(&format!("id: \"{id}\"")));
        assert!(client.contains(&format!("module.exports.name = \"{WEB_ID}\"")));
        assert!(client.contains("const inject = [\"uiWorkspace\"]"));
        assert!(client.contains("module.exports.inject = inject"));
        assert!(client.contains("module.exports.apply = apply"));
        // The mounted row resolves to the host half, whose nearest manifest
        // must be this package's.
        assert!(web_entry(Path::new("/d")).ends_with("web/host.mjs"));
        // A click names the Session the page opens: the route the page asks
        // and the one the Host serves must be the same string.
        assert!(client.contains("const JUMP_URL = \"/api/agent-office/jump\""));
        assert!(TEMPLATE.contains("path: \"/api/agent-office/jump\""));
        assert!(client.contains("ctx.uiWorkspace.openSession(sessionId)"));
        // The host still creates role-selected Sessions, but no association
        // between this page and an external dispatch identifies a list row.
        assert!(TEMPLATE.contains("const agentPreset = `office-${request.role}`"));
        // The Node suite proves that existing, fresh, late and subagent rows
        // have no viewport effects, and that a click alone opens one. Keep the
        // old eager heuristic out of the installed source: it cannot establish
        // dispatch ownership.
        assert!(!client.contains("FRESH_MS"));
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
            Path::new("/j/jump.json"),
        )
        .unwrap();
        assert!(src.contains(r#"const HOOK_EXE = "/a \"b\"/agent-office""#));
        assert!(src.contains(r#"const TRANSCRIPTS = "/t""#));
        assert!(src.contains(r#"const VIEWING = "/v""#));
        assert!(src.contains(r#"const DISPATCH = "/d""#));
        assert!(src.contains(r#"const JUMP = "/j/jump.json""#));
        assert!(!src.contains("__HOOK_EXE__") && !src.contains("__TRANSCRIPTS__"));
        assert!(!src.contains("__VIEWING__") && !src.contains("__DISPATCH__"));
        assert!(!src.contains("__JUMP__"));
    }
}
