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

fn plugin_path(dir: &Path) -> PathBuf {
    dir.join("agent-office-dsh.mjs")
}

fn patch_path(home: &Path) -> PathBuf {
    home.join("cordis.patch.yml")
}

fn plugin_source(exe: &Path, transcripts: &Path) -> Result<String> {
    // JSON string literals are valid JS string literals.
    let js = |p: &Path| serde_json::to_string(&p.display().to_string());
    Ok(TEMPLATE
        .replace("__HOOK_EXE__", &js(exe)?)
        .replace("__TRANSCRIPTS__", &js(transcripts)?))
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
fn is_ours(item: &str) -> bool {
    let ids: Vec<&str> = item
        .lines()
        .filter_map(|l| l.trim_start().trim_start_matches("- ").strip_prefix("id:"))
        .map(|v| v.trim().trim_matches('"'))
        .collect();
    !ids.is_empty() && ids.iter().all(|id| *id == PLUGIN_ID)
}

/// A non-empty flow sequence (`[ {id: x} ]`, which dsh's yaml writer keeps
/// when it adds a row to `[]`): a block item appended after it is invalid
/// YAML, and dsh would refuse to boot.
fn is_flow(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('[') && text.trim() != "[]"
}

fn without_ours(text: &str) -> String {
    // We only ever write block items, so a flow sequence holds none of ours.
    if is_flow(text) {
        return text.to_owned();
    }
    items(text).into_iter().filter(|i| !is_ours(i)).collect()
}

/// What disconnect leaves: dsh's plugin manager refuses a patch that is not
/// a sequence, and a file with no items left is null, not `[]`.
fn after_disconnect(text: &str) -> String {
    let kept = without_ours(text);
    let no_items = kept
        .lines()
        .all(|l| l.trim().is_empty() || l.starts_with('#'));
    if no_items && !text.trim().is_empty() {
        kept + "[]\n"
    } else {
        kept
    }
}

fn with_ours(text: &str, plugin: &Path) -> Result<String> {
    anyhow::ensure!(
        !is_flow(text),
        "cordis.patch.yml 是 [ ... ] 写法，改成每行一个 `- ` 的写法后再 connect dsh"
    );
    let name = serde_json::to_string(&plugin.display().to_string())?;
    let mut out = without_ours(text);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "- insert:\n    - id: {PLUGIN_ID}\n      name: {name}\n"
    ));
    Ok(out)
}

fn read_patch(home: &Path) -> Result<String> {
    let path = patch_path(home);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("读 {} 失败", path.display())),
    }
}

/// dsh reads the patch at boot: a rename never shows it half written.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("yml.agent-office.tmp");
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .with_context(|| format!("写 {} 失败", path.display()))
}

/// `agent-office connect dsh`.
pub fn connect() -> Result<()> {
    let home = home().context("找不到 dsh 目录（DSH_HOME 或 ~/.dsh）")?;
    let dir = our_dir().context("找不到 dsh 目录")?;
    let transcripts = transcripts_dir().context("找不到 dsh 目录")?;
    std::fs::create_dir_all(&transcripts)
        .with_context(|| format!("建 {} 失败", transcripts.display()))?;
    let exe = std::env::current_exe()?.canonicalize()?;
    let plugin = plugin_path(&dir);
    std::fs::write(&plugin, plugin_source(&exe, &transcripts)?)
        .with_context(|| format!("写 {} 失败", plugin.display()))?;
    let before = read_patch(&home)?;
    write_atomic(&patch_path(&home), &with_ours(&before, &plugin)?)?;
    println!(
        "已挂上 {}；新开的 dsh 进程才生效。移动或删除这个程序后要重新 connect dsh。",
        plugin.display()
    );
    Ok(())
}

/// `agent-office disconnect dsh`: unmount and delete the plugin. The task
/// logs stay: they are the user's sessions, not ours to drop.
pub fn disconnect() -> Result<()> {
    let home = home().context("找不到 dsh 目录（DSH_HOME 或 ~/.dsh）")?;
    let before = read_patch(&home)?;
    let after = after_disconnect(&before);
    if after != before {
        write_atomic(&patch_path(&home), &after)?;
        println!("已从 {} 移除", patch_path(&home).display());
    }
    if let Some(dir) = our_dir() {
        let _ = std::fs::remove_file(plugin_path(&dir));
    }
    Ok(())
}

/// Whether the patch mounts our plugin and the plugin file is there.
pub fn installed() -> bool {
    let (Some(home), Some(dir)) = (home(), our_dir()) else {
        return false;
    };
    plugin_path(&dir).is_file()
        && read_patch(&home).is_ok_and(|t| items(&t).iter().any(|i| is_ours(i)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(without_ours(&twice), original);
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
        assert_eq!(without_ours(flow), flow);
    }

    /// Disconnect must not take another plugin's row with it.
    #[test]
    fn an_insert_shared_with_another_plugin_is_kept() {
        let text = "- insert:\n    - id: pixtuoid\n      name: /p.mjs\n    - id: agent-office\n      name: /x.mjs\n";
        assert_eq!(without_ours(text), text);
    }

    /// Paths with quotes or backslashes must not break out of the JS string.
    #[test]
    fn the_plugin_embeds_paths_as_string_literals() {
        let src = plugin_source(Path::new("/a \"b\"/agent-office"), Path::new("/t")).unwrap();
        assert!(src.contains(r#"const HOOK_EXE = "/a \"b\"/agent-office""#));
        assert!(src.contains(r#"const TRANSCRIPTS = "/t""#));
        assert!(!src.contains("__HOOK_EXE__") && !src.contains("__TRANSCRIPTS__"));
    }
}
