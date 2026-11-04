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

    #[test]
    fn one_line_strips_escapes_and_newlines() {
        assert_eq!(one_line("a\n\x1b[31mb\t c"), "a [31mb c");
    }
}
