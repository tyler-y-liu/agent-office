//! The always-on task list beside the office: one card per agent, its
//! subagents under it, whoever is waiting on you first.

use std::collections::HashMap;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::{ActivityState, AgentId, AgentSlot, SceneState, ToolUse};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use crate::pipeline::CC_SOURCE;
use crate::tasks::{TaskView, TaskViews, ago};
use crate::text::{one_line, truncate_cells};

const DIM: Color = Color::Rgb(0x8a, 0x8f, 0x98);
const TEXT: Color = Color::Rgb(0xe6, 0xe6, 0xe6);
const TASK: Color = Color::Rgb(0xff, 0xe8, 0xa3);
const WAIT: Color = Color::Rgb(0xff, 0x9e, 0x64);
const WORK: Color = Color::Rgb(0x7d, 0xcf, 0xff);
const STUCK: Color = Color::Rgb(0xf7, 0x76, 0x8e);
const SELECTED_BG: Color = Color::Rgb(0x2a, 0x2f, 0x3a);

#[derive(Debug, Default)]
pub struct PanelState {
    pub selected: Option<AgentId>,
    pub expanded: bool,
}

/// The agents in display order: roots waiting on you first, then working,
/// then idle (oldest first within each), each followed by its subagents.
pub fn order(scene: &SceneState) -> Vec<(AgentId, u8)> {
    let cc: Vec<&AgentSlot> = scene
        .agents
        .values()
        .filter(|s| &*s.source == CC_SOURCE)
        .collect();
    let is_shown = |id: &AgentId| cc.iter().any(|s| s.agent_id == *id);
    let mut roots: Vec<&AgentSlot> = cc
        .iter()
        .copied()
        .filter(|s| !s.parent_id.as_ref().is_some_and(is_shown))
        .collect();
    let rank = |s: &AgentSlot| match s.state {
        ActivityState::RequiresAction { .. } => 0,
        ActivityState::Running { .. } => 1,
        ActivityState::Idle => 2,
    };
    roots.sort_by_key(|s| (rank(s), s.created_at));
    let mut out = Vec::new();
    for r in roots {
        push_tree(&cc, r, 0, &mut out);
    }
    out
}

/// `slot` then its subagents, theirs under them, oldest first at each level.
fn push_tree(cc: &[&AgentSlot], slot: &AgentSlot, depth: u8, out: &mut Vec<(AgentId, u8)>) {
    // A parent cycle in corrupt state must not recurse forever.
    if out.iter().any(|(id, _)| *id == slot.agent_id) {
        return;
    }
    out.push((slot.agent_id, depth));
    let mut kids: Vec<&AgentSlot> = cc
        .iter()
        .copied()
        .filter(|s| s.parent_id == Some(slot.agent_id))
        .collect();
    kids.sort_by_key(|s| s.created_at);
    for k in kids {
        push_tree(cc, k, depth.saturating_add(1), out);
    }
}

/// Move the selection `delta` rows through `order`, starting at the top.
pub fn step(order: &[(AgentId, u8)], cur: Option<AgentId>, delta: isize) -> Option<AgentId> {
    if order.is_empty() {
        return None;
    }
    let i = cur
        .and_then(|c| order.iter().position(|(id, _)| *id == c))
        .map_or(0, |i| {
            (i as isize + delta).clamp(0, order.len() as isize - 1) as usize
        });
    Some(order[i].0)
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

fn status(slot: &AgentSlot, view: Option<&TaskView>) -> (String, Color) {
    if slot.exiting_at.is_some() {
        return ("离开中".into(), DIM);
    }
    // Waiting on the user says what to do; a stuck verdict is for one working.
    if let Some(s) = view.and_then(|v| v.stuck)
        && matches!(slot.state, ActivityState::Running { .. })
    {
        return (format!("⚠ {}", s.describe()), STUCK);
    }
    match &slot.state {
        ActivityState::RequiresAction { .. } => ("! 等你确认".into(), WAIT),
        ActivityState::Running { item: Some(_) } => match view.and_then(|v| v.phase) {
            Some(p) => (format!("▸ {}", p.label()), WORK),
            None => ("▸ 工作中".into(), WORK),
        },
        ActivityState::Running { item: None } => ("… 思考中".into(), WORK),
        ActivityState::Idle => (format!("· 空闲 {}", ago(slot.state_started_at)), DIM),
    }
}

fn doing(slot: &AgentSlot, view: Option<&TaskView>) -> Option<(String, Color)> {
    match &slot.state {
        ActivityState::Running {
            item: Some(ToolUse {
                detail: Some(d), ..
            }),
        } => Some((one_line(d), TEXT)),
        ActivityState::RequiresAction { reason } => {
            let last_tool = view.and_then(|v| v.recent_tools.last());
            Some((
                waiting_on(reason.as_deref(), last_tool.map(String::as_str)),
                WAIT,
            ))
        }
        _ => None,
    }
}

/// What the agent waits on, in words. pixtuoid's reason is `"permission"`
/// for a PermissionRequest hook and Claude's own English line for a
/// Notification; the call awaiting approval is already the transcript's last.
fn waiting_on(reason: Option<&str>, last_tool: Option<&str>) -> String {
    match reason {
        Some("permission") | None => match last_tool {
            Some(t) => format!("等你批准：{t}"),
            None => "等你批准工具调用".to_owned(),
        },
        Some(r) if r.contains("waiting for your input") => "等你回复".to_owned(),
        Some(r) if r.contains("needs your permission") => match last_tool {
            Some(t) => format!("等你批准：{t}"),
            None => one_line(r),
        },
        Some(r) => one_line(r),
    }
}

/// `text` broken into lines of at most `w` cells.
fn wrap(text: &str, w: u16) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let mut out = Vec::new();
    let mut line = String::new();
    let mut used = 0u16;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0) as u16;
        if used + cw > w && !line.is_empty() {
            out.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push(c);
        used += cw;
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

struct Card {
    id: AgentId,
    lines: Vec<Line<'static>>,
}

fn card(
    slot: &AgentSlot,
    depth: u8,
    view: Option<&TaskView>,
    ink: Option<Rgb>,
    expanded: bool,
    w: u16,
) -> Vec<Line<'static>> {
    let nest = "  ".repeat(usize::from(depth.saturating_sub(1)));
    let indent = if depth == 0 {
        String::new()
    } else {
        format!("{nest}  └ ")
    };
    let pad = if depth == 0 {
        "  ".to_owned()
    } else {
        format!("{nest}      ")
    };
    let pad = pad.as_str();
    let body_w = w.saturating_sub(pad.len() as u16);
    let marker = ink.map_or(TEXT, rgb);
    let (st, st_color) = status(slot, view);
    let name = match view.and_then(|v| v.agent_type.as_deref()) {
        Some(t) if depth > 0 && !slot.label.contains(t) => format!("{t} {}", &*slot.label),
        _ => slot.label.to_string(),
    };
    let name_w = w.saturating_sub(indent.chars().count() as u16 + 2 + crate::text::cells(&st) + 1);
    let mut lines = vec![Line::from(vec![
        Span::raw(indent.clone()),
        Span::styled("● ", Style::default().fg(marker)),
        Span::styled(
            truncate_cells(&name, name_w),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(st, Style::default().fg(st_color)),
    ])];
    let task = view.and_then(|v| v.line.clone());
    let summarized = view.is_some_and(|v| v.summarized);
    let pending = view.is_some_and(|v| v.pending);
    let mut spans = vec![Span::raw(pad.to_owned())];
    match task {
        Some(t) if summarized => spans.push(Span::styled(
            truncate_cells(&t, body_w),
            Style::default().fg(TASK).add_modifier(Modifier::BOLD),
        )),
        Some(t) => {
            spans.push(Span::styled(
                truncate_cells(&t, body_w.saturating_sub(8)),
                Style::default().fg(DIM),
            ));
            if !pending {
                spans.push(Span::styled(" ·未总结", Style::default().fg(DIM)));
            }
        }
        None => spans.push(Span::styled("（还没有指令）", Style::default().fg(DIM))),
    }
    if pending {
        spans.push(Span::styled(" …", Style::default().fg(DIM)));
    }
    lines.push(Line::from(spans));
    if let Some((d, c)) = doing(slot, view) {
        lines.push(Line::from(vec![
            Span::raw(pad.to_owned()),
            Span::styled(
                format!("↳ {}", truncate_cells(&d, body_w.saturating_sub(2))),
                Style::default().fg(c),
            ),
        ]));
    }
    if expanded && let Some(v) = view {
        if let Some(p) = &v.prompt {
            lines.push(Line::from(Span::styled(
                format!("{pad}指令"),
                Style::default().fg(DIM),
            )));
            for l in wrap(p, body_w).into_iter().take(6) {
                lines.push(Line::from(Span::styled(
                    format!("{pad}{l}"),
                    Style::default().fg(TEXT),
                )));
            }
        }
        if let Some(t) = &v.todo {
            lines.push(Line::from(vec![
                Span::styled(format!("{pad}todo "), Style::default().fg(DIM)),
                Span::styled(
                    truncate_cells(t, body_w.saturating_sub(5)),
                    Style::default().fg(TEXT),
                ),
            ]));
        }
        if !v.recent_tools.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("{pad}最近动作"),
                Style::default().fg(DIM),
            )));
            for t in v.recent_tools.iter().rev() {
                lines.push(Line::from(Span::styled(
                    format!("{pad}· {}", truncate_cells(t, body_w.saturating_sub(2))),
                    Style::default().fg(TEXT),
                )));
            }
        }
    }
    lines
}

/// Draw the panel; which agent's card each drawn row belongs to, for a
/// pointer to select by.
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    scene: &SceneState,
    views: &TaskViews,
    inks: &HashMap<AgentId, Rgb>,
    state: &PanelState,
) -> Vec<(u16, AgentId)> {
    let order = order(scene);
    let waiting = order
        .iter()
        .filter(|(id, _)| {
            scene
                .agents
                .get(id)
                .is_some_and(|s| matches!(s.state, ActivityState::RequiresAction { .. }))
        })
        .count();
    let title = if waiting > 0 {
        format!(" 任务 · {} 个 agent · {waiting} 个等你 ", order.len())
    } else {
        format!(" 任务 · {} 个 agent ", order.len())
    };
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::Rgb(0x3b, 0x40, 0x4c)))
        .title(Span::styled(
            title,
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
    };
    if order.is_empty() {
        buf.set_line(
            inner.x,
            inner.y,
            &Line::from(Span::styled(
                "还没有 Claude Code 会话在运行",
                Style::default().fg(DIM),
            )),
            inner.width,
        );
        return Vec::new();
    }
    let cards: Vec<Card> = order
        .iter()
        .filter_map(|(id, depth)| {
            let slot = scene.agents.get(id)?;
            let sel = state.selected == Some(*id);
            let mut lines = card(
                slot,
                *depth,
                views.get(id),
                inks.get(id).copied(),
                sel && state.expanded,
                inner.width,
            );
            // A blank line before each root group but the first.
            if *depth == 0 && !std::ptr::eq(id, &order[0].0) {
                lines.insert(0, Line::raw(""));
            }
            Some(Card { id: *id, lines })
        })
        .collect();
    // Scroll so the selected card is wholly in view.
    let mut start = 0usize;
    let mut sel_range = None;
    let mut y = 0usize;
    for c in &cards {
        if state.selected == Some(c.id) {
            sel_range = Some((y, y + c.lines.len()));
        }
        y += c.lines.len();
    }
    let h = usize::from(inner.height);
    if let Some((top, bottom)) = sel_range
        && bottom > h
    {
        start = (bottom - h).min(top);
    }
    let mut row = 0usize;
    let mut rows = Vec::new();
    for c in &cards {
        let sel = state.selected == Some(c.id);
        for line in &c.lines {
            if row >= start && row - start < h {
                let y = inner.y + (row - start) as u16;
                rows.push((y, c.id));
                if sel {
                    buf.set_style(
                        Rect::new(inner.x, y, inner.width, 1),
                        Style::default().bg(SELECTED_BG),
                    );
                }
                buf.set_line(inner.x, y, line, inner.width);
            }
            row += 1;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn start(
        scene: &mut SceneState,
        r: &mut Reducer,
        sid: &str,
        parent: Option<AgentId>,
        at: SystemTime,
    ) -> AgentId {
        let id = AgentId::from_parts(CC_SOURCE, sid);
        r.apply(
            scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: CC_SOURCE.into(),
                session_id: sid.into(),
                cwd: PathBuf::from("/repo"),
                parent_id: parent,
            },
            at,
            Transport::Hook,
        );
        id
    }

    #[test]
    fn subagents_sit_under_their_parent_and_selection_stays_in_bounds() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, t0);
        let b = start(&mut scene, &mut r, "b", None, t0 + Duration::from_secs(1));
        let a1 = start(
            &mut scene,
            &mut r,
            "agent-a1",
            Some(a),
            t0 + Duration::from_secs(2),
        );
        let o = order(&scene);
        let ids: Vec<AgentId> = o.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![a, a1, b]);
        assert_eq!(o[1].1, 1);
        assert_eq!(step(&o, None, 1), Some(a));
        assert_eq!(step(&o, Some(b), 1), Some(b));
        assert_eq!(step(&o, Some(a1), -5), Some(a));

        let a11 = start(
            &mut scene,
            &mut r,
            "agent-a11",
            Some(a1),
            t0 + Duration::from_secs(3),
        );
        let o = order(&scene);
        assert_eq!(
            o.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![a, a1, a11, b]
        );
        assert_eq!(
            o[2].1, 2,
            "a subagent's own subagent is listed, one level deeper"
        );
    }

    #[test]
    fn a_wait_names_the_call_awaiting_approval() {
        assert_eq!(
            waiting_on(Some("permission"), Some("Bash rm -rf build")),
            "等你批准：Bash rm -rf build"
        );
        assert_eq!(waiting_on(Some("permission"), None), "等你批准工具调用");
        assert_eq!(
            waiting_on(Some("Claude is waiting for your input"), None),
            "等你回复"
        );
        assert_eq!(waiting_on(Some("Something else"), None), "Something else");
    }

    #[test]
    fn wrap_breaks_cjk_by_cells() {
        let lines = wrap("一二三四五六七", 6);
        assert_eq!(lines, vec!["一二三", "四五六", "七"]);
    }
}
