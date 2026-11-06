//! Terminal-cell widths: CJK takes two cells, so byte or char counts misplace
//! labels.

use unicode_width::UnicodeWidthChar;

pub fn cells(s: &str) -> u16 {
    pixtuoid_scene::display::text::cells(s)
}

/// `s` cut to at most `max` cells, an ellipsis marking the cut.
pub fn truncate_cells(s: &str, max: u16) -> String {
    let max = usize::from(max);
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// A tool line (`Edit: /repo/src/x.rs`) fitted to `max` cells: paths under
/// `cwd` made relative to it, and a lone path still too long cut in its
/// middle, so the file name it ends in stays; a cut end lost exactly that.
pub fn fit_tool_line(s: &str, cwd: &std::path::Path, max: u16) -> String {
    // Only a lone path is made relative: inside a command, dropping the
    // directory could leave `cd  && make`.
    let root = format!("{}/", cwd.to_string_lossy().trim_end_matches('/'));
    let s = match s.split_once(": ") {
        Some((head, path)) if root.len() > 1 && !path.contains(' ') => {
            format!("{head}: {}", path.strip_prefix(&root).unwrap_or(path))
        }
        _ => s.to_owned(),
    };
    if cells(&s) <= max {
        return s;
    }
    match s.split_once(": ") {
        Some((head, path))
            if path.contains('/') && !path.contains(' ') && cells(head) + 6 < max =>
        {
            let room = max - cells(head) - 3;
            format!("{head}: …{}", tail_cells(path, room))
        }
        _ => truncate_cells(&s, max),
    }
}

/// The longest end of `s` at most `max` cells wide.
fn tail_cells(s: &str, max: u16) -> &str {
    let mut used = 0;
    let mut start = s.len();
    for (i, c) in s.char_indices().rev() {
        let w = c.width().unwrap_or(0) as u16;
        if used + w > max {
            break;
        }
        used += w;
        start = i;
    }
    &s[start..]
}

/// One line: control characters and newlines (transcript text is untrusted
/// and multi-line) collapsed to single spaces.
pub fn one_line(s: &str) -> String {
    s.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_counts_double_so_a_cut_never_overflows_its_cells() {
        let t = truncate_cells("修复拼读页音频不播放", 9);
        assert_eq!(t, "修复拼读…");
        assert!(cells(&t) <= 9);
        assert_eq!(truncate_cells("短", 9), "短");
    }

    /// The file a tool works on is the part of its line worth reading: the
    /// repo's own prefix goes, and a deep path loses its middle, not its end.
    #[test]
    fn a_tool_line_keeps_the_file_name() {
        let cwd = std::path::Path::new("/Users/demo/blog");
        assert_eq!(
            fit_tool_line("Edit: /Users/demo/blog/src/pages/index.tsx", cwd, 40),
            "Edit: src/pages/index.tsx"
        );
        let long = fit_tool_line("Read: /opt/vendor/some/deep/tree/of/dirs/main.rs", cwd, 24);
        assert!(
            long.starts_with("Read: …") && long.ends_with("/main.rs"),
            "{long}"
        );
        assert!(cells(&long) <= 24, "{long}");
        // A command is left as run, and cut at its end like any text.
        assert_eq!(
            fit_tool_line("Bash: cd /Users/demo/blog/ && make", cwd, 60),
            "Bash: cd /Users/demo/blog/ && make"
        );
        let cmd = fit_tool_line("Bash: cd /tmp/x && make build-everything", cwd, 20);
        assert!(cmd.starts_with("Bash: cd") && cmd.ends_with('…'), "{cmd}");
    }

    /// Hooks and transcripts carry absolute paths, often 50+ characters:
    /// pixtuoid used to cut them to 40 before this saw them, so the card
    /// showed a directory prefix and never the file. Through the real
    /// decoder, the file name must survive both the cap and the fit.
    #[test]
    fn a_decoded_long_path_still_shows_its_file_name() {
        use pixtuoid_core::AgentEvent;
        // A session started in a scratchpad: its file's path runs past 200
        // characters, and the session's own directory must still come off.
        let cwd = "/tmp/demo-office/-home-demo-code-personal-blog--demo-worktrees-project-review-optim-000000/00000000-0000-0000-0000-000000000000/scratchpad/demo-review/repos/shop-web";
        let path = format!("{cwd}/src/components/checkout/CouponField.tsx");
        let v = serde_json::json!({
            "hook_event_name": "PreToolUse", "session_id": "00000000-0000-0000-0000-000000000000",
            "cwd": cwd, "tool_name": "Edit", "tool_use_id": "t1",
            "tool_input": { "file_path": path },
        });
        let evs = pixtuoid_core::source::decoder::decode_hook_payload(v).unwrap();
        let shown = evs
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolUseStart { detail: Some(d), .. } => Some(d.display().to_owned()),
                _ => None,
            })
            .expect("a tool call");
        let cwd = std::path::Path::new(cwd);
        assert_eq!(
            fit_tool_line(&shown, cwd, 60),
            "Edit: src/components/checkout/CouponField.tsx"
        );
        let narrow = fit_tool_line(&shown, cwd, 30);
        assert!(narrow.ends_with("/CouponField.tsx"), "{narrow}");
        assert!(cells(&narrow) <= 30, "{narrow}");
        // A path outside the session's directory keeps its file name too.
        let other = "/tmp/demo/-home-demo-code-personal-blog/00000000/scratchpad/notes/plan.md";
        let line = fit_tool_line(&format!("Read: {other}"), cwd, 30);
        assert!(line.ends_with("/plan.md"), "{line}");
    }

    #[test]
    fn one_line_strips_escapes_and_newlines() {
        assert_eq!(one_line("a\n\x1b[31mb\t c"), "a [31mb c");
    }
}
