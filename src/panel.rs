//! The always-on task list beside the office: one card per agent, its
//! subagents under it, grouped by what each needs from you — waiting on you
//! first — and each group named after where its agents are in the office.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::{ActivityState, AgentId, AgentSlot, SceneState, ToolUse};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use crate::tasks::{TaskView, TaskViews, ago};
use crate::text::{one_line, truncate_cells};

const DIM: Color = Color::Rgb(0x8a, 0x8f, 0x98);
const TEXT: Color = Color::Rgb(0xe6, 0xe6, 0xe6);
const TASK: Color = Color::Rgb(0xff, 0xe8, 0xa3);
pub const WAIT: Color = Color::Rgb(0xff, 0x9e, 0x64);
const WORK: Color = Color::Rgb(0x7d, 0xcf, 0xff);
const STUCK: Color = Color::Rgb(0xf7, 0x76, 0x8e);
const SELECTED_BG: Color = Color::Rgb(0x2a, 0x2f, 0x3a);
/// The panel's own background, which a faded card's colours sink toward.
const PANEL_BG: Color = Color::Rgb(0x16, 0x18, 0x1d);
/// A card waiting on you sits on a solid warm ground: it is the one to act on.
const WAIT_BG: Color = Color::Rgb(0x4a, 0x30, 0x1e);
const WAIT_SELECTED_BG: Color = Color::Rgb(0x5e, 0x3e, 0x28);
/// How far an idle card's colours sink into the panel: it needs nothing.
const IDLE_FADE: f32 = 0.55;
/// Text on a coloured chip or avatar.
const CHIP_INK: Color = Color::Rgb(0x16, 0x18, 0x1d);

#[derive(Debug, Default)]
pub struct PanelState {
    pub selected: Option<AgentId>,
    pub expanded: bool,
    /// When you last opened each session from here, or looked at it in the
    /// desktop app ([`crate::desktop::mark_read`]): what it had done by then
    /// is read.
    pub seen: HashMap<AgentId, SystemTime>,
    /// The first row of cards shown: the wheel moves it, and a new
    /// selection moves it as far as it takes to show that card.
    pub scroll: usize,
    /// Bring the selected card into view on the next draw. Only when the
    /// selection or its size changes, so the wheel can scroll away from it.
    pub reveal: bool,
}

impl PanelState {
    /// Scroll the cards by `rows`, down when positive; the draw keeps it
    /// within the cards.
    pub fn scroll_by(&mut self, rows: isize) {
        self.scroll = self.scroll.saturating_add_signed(rows);
        self.reveal = false;
    }
}

/// Whether `slot` is idle and was opened since it went idle: its result is
/// read, so it no longer waits in the pantry to be looked at.
pub fn read_since_idle(slot: &AgentSlot, seen: &HashMap<AgentId, SystemTime>) -> bool {
    matches!(slot.state, ActivityState::Idle)
        && seen
            .get(&slot.agent_id)
            .is_some_and(|&at| at >= slot.state_started_at)
}

/// What a session waiting on you waits for, the more urgent first: the
/// order it lines up for your office in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Wait {
    /// A prompt: a permission, a dialog. It holds the session mid-turn.
    Prompt,
    /// Its turn is done and ended on a question to you (the outcome's
    /// 「问：」).
    Reply,
}

/// What `slot` waits on you for, if anything. `read` says it was opened
/// since it went idle ([`read_since_idle`]): its question was seen, so it no
/// longer lines up for it.
pub fn waits_on_you(slot: &AgentSlot, view: Option<&TaskView>, read: bool) -> Option<Wait> {
    match slot.state {
        _ if slot.exiting_at.is_some() => None,
        ActivityState::RequiresAction { .. } => Some(Wait::Prompt),
        ActivityState::Idle
            if !read
                && view.is_some_and(|v| {
                    v.outcome_summarized
                        && v.outcome.as_deref().is_some_and(|o| o.starts_with("问："))
                }) =>
        {
            Some(Wait::Reply)
        }
        _ => None,
    }
}

/// What a session needs from you, which is also where it is in the office.
/// Declared in the order the panel lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// Waiting on a prompt or question: answer it.
    Waiting,
    /// Working but stuck: have a look.
    Stuck,
    /// Done a while ago at most, in the pantry: read what it did.
    Done,
    /// Working.
    Working,
    /// Idle long enough to have gone up to the rooftop.
    Idle,
}

impl Group {
    /// `read` says it was opened since it went idle ([`read_since_idle`]).
    pub fn of(slot: &AgentSlot, view: Option<&TaskView>, now: SystemTime, read: bool) -> Self {
        if crate::tasks::stuck_at_work(slot, view).is_some() {
            return Self::Stuck;
        }
        if waits_on_you(slot, view, read).is_some() {
            return Self::Waiting;
        }
        if read {
            return Self::Idle;
        }
        match slot.state {
            ActivityState::RequiresAction { .. } => Self::Waiting,
            ActivityState::Running { .. } => Self::Working,
            // The office's own threshold, so the card and the figure agree.
            ActivityState::Idle
                if now
                    .duration_since(slot.state_started_at)
                    .unwrap_or_default()
                    .as_millis()
                    < u128::from(pixtuoid_scene::pose::IDLE_ROOFTOP_MS) =>
            {
                Self::Done
            }
            ActivityState::Idle => Self::Idle,
        }
    }

    /// The group's header: what it needs from you, then where they are.
    pub fn title(self) -> &'static str {
        match self {
            Self::Waiting => "等你 · 等候区",
            Self::Stuck => "卡住 · 工位",
            Self::Done => "做完待看 · 茶水间",
            Self::Working => "干活 · 工位",
            Self::Idle => "闲着 · 天台",
        }
    }
}

/// The agents in display order: roots grouped by [`Group`], those waiting on
/// you prompts first, each kind in the order it started waiting (the
/// office's waiting line), the rest oldest first; each followed by its
/// subagents.
pub fn order(
    scene: &SceneState,
    views: &TaskViews,
    now: SystemTime,
    seen: &HashMap<AgentId, SystemTime>,
) -> Vec<(AgentId, u8)> {
    let cc: Vec<&AgentSlot> = scene
        .agents
        .values()
        .filter(|s| crate::pipeline::is_shown_source(&s.source))
        .collect();
    let is_shown = |id: &AgentId| cc.iter().any(|s| s.agent_id == *id);
    let mut roots: Vec<&AgentSlot> = cc
        .iter()
        .copied()
        .filter(|s| !s.parent_id.as_ref().is_some_and(is_shown))
        .collect();
    roots.sort_by_key(|s| {
        let view = views.get(&s.agent_id);
        let read = read_since_idle(s, seen);
        let g = Group::of(s, view, now, read);
        let (wait, since) = if g == Group::Waiting {
            (waits_on_you(s, view, read), s.state_started_at)
        } else {
            (None, s.created_at)
        };
        (g, wait, since)
    });
    let mut out = Vec::new();
    for r in roots {
        push_tree(&cc, r, 0, &mut out);
    }
    out
}

/// The session to deal with next: the one in your office's guest chair, so
/// what opens is who you see there; with the chair empty, the one that will
/// take it.
pub fn next_waiting(
    scene: &SceneState,
    views: &TaskViews,
    now: SystemTime,
    seen: &HashMap<AgentId, SystemTime>,
) -> Option<AgentId> {
    let waiting: Vec<&AgentSlot> = order(scene, views, now, seen)
        .into_iter()
        .filter(|(_, depth)| *depth == 0)
        .map(|(id, _)| &scene.agents[&id])
        .filter(|s| waits_on_you(s, views.get(&s.agent_id), read_since_idle(s, seen)).is_some())
        .collect();
    let lined_up = |s: &&AgentSlot| {
        now.duration_since(s.state_started_at)
            .unwrap_or_default()
            .as_millis()
            >= u128::from(pixtuoid_scene::waiting::LINE_UP_AFTER_MS)
    };
    waiting
        .iter()
        .copied()
        .find(lined_up)
        .or(waiting.first().copied())
        .map(|s| s.agent_id)
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

fn group_color(g: Group) -> Color {
    match g {
        Group::Waiting => WAIT,
        Group::Stuck => STUCK,
        Group::Done => TASK,
        Group::Working => WORK,
        Group::Idle => DIM,
    }
}

/// The first letter of the repository a label names (`cc·repo`), as an
/// avatar shows it.
fn avatar_initial(label: &str) -> char {
    let repo = label.split_once('·').map_or(label, |(_, r)| r);
    repo.chars()
        .find(|c| c.is_alphanumeric())
        .map_or('?', |c| c.to_uppercase().next().unwrap_or(c))
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// `read` says it was opened since it went idle ([`read_since_idle`]).
fn status(slot: &AgentSlot, view: Option<&TaskView>, read: bool) -> (String, Color) {
    if slot.exiting_at.is_some() {
        return ("离开中".into(), DIM);
    }
    // Waiting on the user says what to do; a stuck verdict is for one working.
    if let Some(s) = crate::tasks::stuck_at_work(slot, view) {
        return (format!("⚠ {}", s.describe()), STUCK);
    }
    match &slot.state {
        ActivityState::RequiresAction { .. } => ("! 等你确认".into(), WAIT),
        ActivityState::Running { item } => {
            let what = match (item, view.and_then(|v| v.phase)) {
                (Some(_), Some(p)) => format!("▸ {}", p.label()),
                (Some(_), None) => "▸ 工作中".to_owned(),
                (None, _) => "… 思考中".to_owned(),
            };
            let took = view
                .and_then(|v| v.turn_started)
                .and_then(|t| turn_took(t, SystemTime::now()));
            match took {
                Some(took) => (format!("{what} {took}"), WORK),
                None => (what, WORK),
            }
        }
        // Opened or not, the question stays unanswered until you reply.
        ActivityState::Idle if waits_on_you(slot, view, false) == Some(Wait::Reply) => (
            format!("? {REPLY_WAIT} {}", ago(slot.state_started_at)),
            WAIT,
        ),
        // Read, it is up on the rooftop however short its idle stretch: the
        // card says why, or "空闲 7m" up there reads as a bug.
        ActivityState::Idle if read => {
            (format!("✓ 已看 · 空闲 {}", ago(slot.state_started_at)), DIM)
        }
        ActivityState::Idle => (format!("· 空闲 {}", ago(slot.state_started_at)), DIM),
    }
}

/// How long a turn that began at `started` has run, from a minute on: under
/// that every turn is short, and a ticking count would only be noise.
fn turn_took(started: SystemTime, now: SystemTime) -> Option<String> {
    let s = now.duration_since(started).ok()?.as_secs();
    match s {
        0..60 => None,
        60..3600 => Some(format!("{}m", s / 60)),
        _ => Some(format!("{}h{:02}m", s / 3600, s % 3600 / 60)),
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
            let wait = waiting_on(reason.as_deref(), last_tool.map(String::as_str));
            // A turn that ended on a question waits for your reply: the
            // outcome says what it asked.
            match view.and_then(|v| v.outcome.as_deref()) {
                Some(o) if wait == REPLY_WAIT => Some((format!("{wait}：{o}"), WAIT)),
                _ => Some((wait, WAIT)),
            }
        }
        // Done: what it came to, so you know which to read first.
        ActivityState::Idle if slot.exiting_at.is_none() => {
            let v = view?;
            let o = v.outcome.clone()?;
            let color = if !v.outcome_summarized {
                DIM
            } else if o.starts_with("问：") {
                WAIT
            } else {
                TEXT
            };
            Some((o, color))
        }
        _ => None,
    }
}

/// The agent finished its turn and waits on your answer.
const REPLY_WAIT: &str = "等你回复";

/// What the agent waits on, in words. pixtuoid's reason is `"permission"`
/// for a PermissionRequest hook and Claude's own English line for a
/// Notification; the call awaiting approval is already the transcript's last.
fn waiting_on(reason: Option<&str>, last_tool: Option<&str>) -> String {
    match reason {
        Some("permission") | None => match last_tool {
            Some(t) => format!("等你批准：{t}"),
            None => "等你批准工具调用".to_owned(),
        },
        Some(r) if r.contains("waiting for your input") => REPLY_WAIT.to_owned(),
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
    /// Its root's group, which sets how heavy the card looks.
    group: Group,
    /// Lines before the card's own: its group's header, a blank.
    lead: usize,
    /// The cell its avatar starts at on its first line.
    avatar_x: u16,
}

/// Cells an avatar takes.
pub const AVATAR_CELLS: u16 = 3;

/// What a drawn panel holds beyond its cells, for the window to finish.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drawn {
    /// Which agent's card each drawn row belongs to, for a pointer to select by.
    pub rows: Vec<(u16, AgentId)>,
    /// Each drawn avatar: its row and first cell, whose head it shows, and
    /// whether its card is faded.
    pub avatars: Vec<(u16, u16, AgentId, bool)>,
    /// The rows (first, last) of each card drawn in outline: done, waiting
    /// for you to read it.
    pub outlines: Vec<(u16, u16)>,
}

/// `c` sunk `t` of the way into the panel's background.
fn fade(c: Color, t: f32) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) = (c, PANEL_BG) else {
        return c;
    };
    let mix = |a: u8, z: u8| (f32::from(a) + (f32::from(z) - f32::from(a)) * t).round() as u8;
    Color::Rgb(mix(r, br), mix(g, bg), mix(b, bb))
}

/// `line` with every colour faded ([`fade`]).
fn faded(line: Line<'static>) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|mut s| {
            s.style.fg = Some(fade(s.style.fg.unwrap_or(TEXT), IDLE_FADE));
            s.style.bg = s.style.bg.map(|b| fade(b, IDLE_FADE));
            s
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn card(
    slot: &AgentSlot,
    depth: u8,
    view: Option<&TaskView>,
    ink: Option<Rgb>,
    expanded: bool,
    read: bool,
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
    let marker = ink.map_or(DIM, rgb);
    let (st, st_color) = status(slot, view, read);
    let name = match view.and_then(|v| v.agent_type.as_deref()) {
        Some(t) if depth > 0 && !slot.label.contains(t) => format!("{t} {}", &*slot.label),
        _ => slot.label.to_string(),
    };
    let chip = format!(" {st} ");
    let avatar = format!(" {} ", avatar_initial(&slot.label));
    let name_w = w.saturating_sub(
        indent.chars().count() as u16
            + crate::text::cells(&avatar)
            + 1
            + crate::text::cells(&chip)
            + 1,
    );
    let mut lines = vec![Line::from(vec![
        Span::raw(indent.clone()),
        Span::styled(
            avatar,
            Style::default()
                .bg(marker)
                .fg(CHIP_INK)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            truncate_cells(&name, name_w),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(chip, Style::default().bg(st_color).fg(CHIP_INK)),
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
        if matches!(slot.state, ActivityState::Idle)
            && let Some(r) = &v.reply
        {
            lines.push(Line::from(Span::styled(
                format!("{pad}最后回复"),
                Style::default().fg(DIM),
            )));
            // The end of a long reply is where it concludes.
            let wrapped = wrap(r, body_w);
            for l in &wrapped[wrapped.len().saturating_sub(6)..] {
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

/// Draw the panel: waiting cards on a warm ground, idle ones faded, and
/// where the window draws the rest ([`Drawn`]).
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    scene: &SceneState,
    views: &TaskViews,
    inks: &HashMap<AgentId, Rgb>,
    state: &mut PanelState,
) -> Drawn {
    let now = SystemTime::now();
    let order = order(scene, views, now, &state.seen);
    let group_of = |s: &AgentSlot| {
        Group::of(
            s,
            views.get(&s.agent_id),
            now,
            read_since_idle(s, &state.seen),
        )
    };
    let hues = pixtuoid_scene::badge::repo_hues(scene.agents.values());
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
        return Drawn::default();
    }
    let mut root_group = Group::Working;
    let cards: Vec<Card> = order
        .iter()
        .filter_map(|(id, depth)| {
            let slot = scene.agents.get(id)?;
            let sel = state.selected == Some(*id);
            // The repository's colour, as its sitters' badges wear it in
            // the office.
            let mut lines = card(
                slot,
                *depth,
                views.get(id),
                pixtuoid_scene::badge::repo_hue(slot, &hues).or_else(|| inks.get(id).copied()),
                sel && state.expanded,
                read_since_idle(slot, &state.seen),
                inner.width,
            );
            // A subagent's card goes with its root's.
            if *depth == 0 {
                root_group = group_of(slot);
            }
            let group = root_group;
            if group == Group::Idle {
                lines = lines.into_iter().map(faded).collect();
            }
            let avatar_x = if *depth == 0 {
                0
            } else {
                crate::text::cells(&format!(
                    "{}  └ ",
                    "  ".repeat(usize::from(depth.saturating_sub(1)))
                ))
            };
            let before = lines.len();
            // A header over each group's first card, a blank line before
            // every other root.
            if *depth == 0 {
                let first = order
                    .iter()
                    .take_while(|(o, _)| o != id)
                    .filter(|(_, d)| *d == 0)
                    .filter_map(|(o, _)| scene.agents.get(o))
                    .all(|s| group_of(s) != group);
                if first {
                    let n = order
                        .iter()
                        .filter(|(_, d)| *d == 0)
                        .filter_map(|(o, _)| scene.agents.get(o))
                        .filter(|s| group_of(s) == group)
                        .count();
                    lines.insert(
                        0,
                        Line::from(Span::styled(
                            format!("{} ({n})", group.title()),
                            Style::default()
                                .fg(group_color(group))
                                .add_modifier(Modifier::BOLD),
                        )),
                    );
                }
                if !std::ptr::eq(id, &order[0].0) {
                    lines.insert(0, Line::raw(""));
                }
            }
            let lead = lines.len() - before;
            Some(Card {
                id: *id,
                lines,
                group,
                lead,
                avatar_x,
            })
        })
        .collect();
    let mut sel_range = None;
    let mut y = 0usize;
    for c in &cards {
        if state.selected == Some(c.id) {
            sel_range = Some((y, y + c.lines.len()));
        }
        y += c.lines.len();
    }
    let h = usize::from(inner.height);
    // A newly selected card is brought wholly into view, its top kept in
    // view if it is taller than the panel.
    if std::mem::take(&mut state.reveal)
        && let Some((top, bottom)) = sel_range
    {
        if bottom > state.scroll + h {
            state.scroll = bottom.saturating_sub(h);
        }
        state.scroll = state.scroll.min(top);
    }
    // No scrolling past the last card, so the wheel never leaves the panel
    // blank and a shrinking list pulls its cards back into view.
    state.scroll = state.scroll.min(y.saturating_sub(h));
    let start = state.scroll;
    let mut row = 0usize;
    let mut drawn = Drawn::default();
    for c in &cards {
        let sel = state.selected == Some(c.id);
        let mut shown: Option<(u16, u16)> = None;
        for (i, line) in c.lines.iter().enumerate() {
            if row >= start && row - start < h {
                let y = inner.y + (row - start) as u16;
                drawn.rows.push((y, c.id));
                let own = i >= c.lead;
                let bg = match (own && c.group == Group::Waiting, sel) {
                    (true, true) => Some(WAIT_SELECTED_BG),
                    (true, false) => Some(WAIT_BG),
                    (false, true) => Some(SELECTED_BG),
                    (false, false) => None,
                };
                if let Some(bg) = bg {
                    buf.set_style(
                        Rect::new(inner.x, y, inner.width, 1),
                        Style::default().bg(bg),
                    );
                }
                buf.set_line(inner.x, y, line, inner.width);
                if i == c.lead {
                    drawn
                        .avatars
                        .push((y, inner.x + c.avatar_x, c.id, c.group == Group::Idle));
                }
                if own {
                    shown = Some(shown.map_or((y, y), |(top, _)| (top, y)));
                }
            }
            row += 1;
        }
        if c.group == Group::Done
            && let Some(rows) = shown
        {
            drawn.outlines.push(rows);
        }
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::CC_SOURCE;
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

    /// The office marks a stuck agent at its desk, and the card says so, only
    /// while it works: one waiting on you is waiting, not stuck, and saying
    /// "stuck" there would hide that it needs an answer.
    #[test]
    fn only_a_working_agent_is_marked_stuck() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, t0);
        let view = TaskView {
            stuck: Some(crate::tasks::Stuck::Failing(3)),
            ..TaskView::default()
        };
        let slot = scene.agents.get_mut(&a).unwrap();
        slot.state = ActivityState::Running { item: None };
        assert!(crate::tasks::stuck_at_work(slot, Some(&view)).is_some());
        assert_eq!(status(slot, Some(&view), false).1, STUCK);
        slot.state = ActivityState::RequiresAction { reason: None };
        assert!(crate::tasks::stuck_at_work(slot, Some(&view)).is_none());
        assert_eq!(status(slot, Some(&view), false).1, WAIT);
        slot.state = ActivityState::Idle;
        assert!(crate::tasks::stuck_at_work(slot, Some(&view)).is_none());
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
        let o = order(
            &scene,
            &TaskViews::default(),
            t0 + Duration::from_secs(10),
            &HashMap::new(),
        );
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
        let o = order(
            &scene,
            &TaskViews::default(),
            t0 + Duration::from_secs(10),
            &HashMap::new(),
        );
        assert_eq!(
            o.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![a, a1, a11, b]
        );
        assert_eq!(
            o[2].1, 2,
            "a subagent's own subagent is listed, one level deeper"
        );
    }

    /// More sessions than fit must be reachable by the wheel, not only by
    /// selecting through them; a scroll past the end would blank the panel,
    /// and a selection that keeps snapping back would undo every scroll.
    #[test]
    fn the_wheel_scrolls_the_cards_and_a_selection_brings_its_card_back() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let ids: Vec<AgentId> = (0..8)
            .map(|i| {
                start(
                    &mut scene,
                    &mut r,
                    &format!("s{i}"),
                    None,
                    t0 + Duration::from_secs(i),
                )
            })
            .collect();
        let views = TaskViews::default();
        let area = Rect::new(0, 0, 40, 12);
        let mut state = PanelState::default();
        let draw = |state: &mut PanelState| {
            let mut buf = Buffer::empty(area);
            render(&mut buf, area, &scene, &views, &HashMap::new(), state)
        };
        let first_card = |rows: &[(u16, AgentId)]| rows.first().map(|(_, id)| *id);
        let draw = |state: &mut PanelState| draw(state).rows;
        let top = draw(&mut state);
        let shown_first = first_card(&top);
        assert!(
            ids.iter().any(|id| !top.iter().any(|(_, o)| o == id)),
            "the test needs more cards than the panel holds"
        );

        state.scroll_by(3);
        let scrolled = draw(&mut state);
        assert_eq!(state.scroll, 3);
        assert_ne!(scrolled, top, "the wheel moves the cards");

        state.scroll_by(1000);
        let bottom = draw(&mut state);
        assert!(state.scroll < 1000, "the scroll stops at the last card");
        assert_eq!(
            bottom.last().map(|(y, _)| *y),
            Some(area.height - 1),
            "scrolled to the end, the last row is still filled"
        );

        state.scroll_by(-1000);
        assert_eq!(first_card(&draw(&mut state)), shown_first);

        // Selecting an off-screen card brings it into view once ...
        let target = *ids
            .iter()
            .find(|id| !draw(&mut state).iter().any(|(_, o)| o == *id))
            .expect("some card starts off screen");
        state.selected = Some(target);
        state.reveal = true;
        assert!(draw(&mut state).iter().any(|(_, o)| *o == target));
        // ... and the wheel can then scroll away from it.
        state.scroll_by(-1000);
        assert!(!draw(&mut state).iter().any(|(_, o)| *o == target));
    }

    /// The panel lists sessions by what they need from you, most urgent
    /// first, so the top of the panel is always the next thing to do; those
    /// waiting on you keep the order they started waiting, the order they
    /// stand in the office's waiting line.
    #[test]
    fn roots_group_by_what_they_need_from_you() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(3600);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let ids: Vec<AgentId> = ["idle", "work", "done", "stuck", "wait2", "wait1"]
            .iter()
            .enumerate()
            .map(|(i, sid)| {
                start(
                    &mut scene,
                    &mut r,
                    sid,
                    None,
                    t0 + Duration::from_secs(i as u64),
                )
            })
            .collect();
        let [idle, work, done, stuck, wait2, wait1] = ids[..] else {
            unreachable!()
        };
        let set = |scene: &mut SceneState, id, state, since: Duration| {
            let s = scene.agents.get_mut(&id).unwrap();
            s.state = state;
            s.state_started_at = now - since;
        };
        let run = || ActivityState::Running { item: None };
        let ask = || ActivityState::RequiresAction { reason: None };
        set(
            &mut scene,
            idle,
            ActivityState::Idle,
            Duration::from_secs(20 * 60),
        );
        set(
            &mut scene,
            done,
            ActivityState::Idle,
            Duration::from_secs(5 * 60),
        );
        set(&mut scene, work, run(), Duration::from_secs(60));
        set(&mut scene, stuck, run(), Duration::from_secs(60));
        set(&mut scene, wait2, ask(), Duration::from_secs(30));
        set(&mut scene, wait1, ask(), Duration::from_secs(90));
        let views: TaskViews = std::sync::Arc::new(
            [(
                stuck,
                TaskView {
                    stuck: Some(crate::tasks::Stuck::Failing(3)),
                    ..TaskView::default()
                },
            )]
            .into_iter()
            .collect(),
        );
        let o: Vec<AgentId> = order(&scene, &views, now, &HashMap::new())
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(o, vec![wait1, wait2, stuck, done, work, idle]);
        let group = |id| Group::of(&scene.agents[&id], views.get(&id), now, false);
        assert_eq!(
            group(done),
            Group::Done,
            "idle under 15 min sits in the pantry"
        );
        assert_eq!(
            group(idle),
            Group::Idle,
            "idle 15 min+ is up on the rooftop"
        );
    }

    /// A done session asking you a question needs you as much as a prompt
    /// does, so it waits with them, not in the pantry; but behind them, since
    /// a prompt holds its session mid-turn. Once opened, the question was
    /// seen, and it idles like any read session.
    #[test]
    fn a_question_to_you_waits_behind_the_prompts() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(3600);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let [asked, prompt, done] =
            ["asked", "prompt", "done"].map(|sid| start(&mut scene, &mut r, sid, None, t0));
        for (id, state, ago) in [
            (asked, ActivityState::Idle, 600),
            (prompt, ActivityState::RequiresAction { reason: None }, 30),
            (done, ActivityState::Idle, 900 - 60),
        ] {
            let s = scene.agents.get_mut(&id).unwrap();
            s.state = state;
            s.state_started_at = now - Duration::from_secs(ago);
        }
        let outcome = |o: &str| TaskView {
            outcome: Some(o.into()),
            outcome_summarized: true,
            ..TaskView::default()
        };
        let views: TaskViews = std::sync::Arc::new(
            [
                (asked, outcome("问：要不要删旧接口")),
                (done, outcome("登录页已修好")),
            ]
            .into_iter()
            .collect(),
        );
        let none = HashMap::new();
        let o: Vec<AgentId> = order(&scene, &views, now, &none)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(o, vec![prompt, asked, done]);
        let group = |id| Group::of(&scene.agents[&id], views.get(&id), now, false);
        assert_eq!(group(asked), Group::Waiting);
        assert_eq!(
            group(done),
            Group::Done,
            "a plain result is still read in the pantry"
        );
        assert_eq!(next_waiting(&scene, &views, now, &none), Some(prompt));
        // A prompt just raised has not walked over yet: what opens is the
        // question you see in the guest chair.
        let mut fresh = scene.clone();
        fresh.agents.get_mut(&prompt).unwrap().state_started_at = now - Duration::from_secs(5);
        assert_eq!(next_waiting(&fresh, &views, now, &none), Some(asked));
        assert!(
            status(&scene.agents[&asked], views.get(&asked), false)
                .0
                .contains("等你回复")
        );

        let opened = HashMap::from([(asked, now - Duration::from_secs(5))]);
        let slot = &scene.agents[&asked];
        assert_eq!(
            waits_on_you(slot, views.get(&asked), read_since_idle(slot, &opened)),
            None
        );
        let mut answered = scene.clone();
        answered.agents.remove(&prompt);
        assert_eq!(
            next_waiting(&answered, &views, now, &opened),
            None,
            "nothing left to deal with"
        );
        assert_eq!(next_waiting(&answered, &views, now, &none), Some(asked));
    }

    /// A card leads with its repository's initial on the repository's colour
    /// (the colour its sitters' badges wear) and ends on its status as a chip
    /// in the status's colour, so a glance down the panel reads project and
    /// state without reading words.
    #[test]
    fn a_card_wears_its_repo_and_its_status() {
        assert_eq!(avatar_initial("cc·agent-office"), 'A');
        assert_eq!(avatar_initial("cc·_dotfiles"), 'D');
        assert_eq!(avatar_initial(""), '?');
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, t0);
        let slot = scene.agents.get_mut(&a).unwrap();
        slot.state = ActivityState::RequiresAction { reason: None };
        let ink = Rgb { r: 1, g: 2, b: 3 };
        let lines = card(slot, 0, None, Some(ink), false, false, 40);
        let first = &lines[0].spans;
        let avatar = first
            .iter()
            .find(|s| s.style.bg == Some(rgb(ink)))
            .expect("an avatar");
        assert_eq!(
            avatar.content.trim(),
            avatar_initial(&slot.label).to_string()
        );
        let chip = first.last().expect("a chip");
        assert_eq!(chip.style.bg, Some(WAIT), "the status on its colour");
        assert!(chip.content.contains("等你确认"));
    }

    /// A finished session waits in the pantry, under 做完待看, until you open
    /// it; opened, its result is read and it is simply idle. Opened while it
    /// still worked, what it went on to finish is unread.
    #[test]
    fn opening_a_finished_session_reads_it() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(600);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, t0);
        let slot = scene.agents.get_mut(&a).unwrap();
        slot.state = ActivityState::Idle;
        slot.state_started_at = now - Duration::from_secs(60);
        let slot = scene.agents[&a].clone();
        let group = |seen: &HashMap<AgentId, SystemTime>| {
            Group::of(&slot, None, now, read_since_idle(&slot, seen))
        };
        assert_eq!(group(&HashMap::new()), Group::Done);
        let opened = HashMap::from([(a, now - Duration::from_secs(10))]);
        assert_eq!(group(&opened), Group::Idle, "read");
        // Read a minute after it finished, it is up on the rooftop: the card
        // must say it was read, or a 1-minute idle up there reads as a bug.
        assert!(status(&slot, None, false).0.starts_with("· 空闲 "));
        assert!(status(&slot, None, true).0.starts_with("✓ 已看 · 空闲 "));
        let while_working = HashMap::from([(a, now - Duration::from_secs(120))]);
        assert_eq!(
            group(&while_working),
            Group::Done,
            "finished after it was opened"
        );
    }

    /// A working card says how long its turn has run, so a session that has
    /// gone on far longer than its task should stands out without opening it.
    /// One waiting on you says what to do instead: its clock is yours.
    #[test]
    fn a_working_card_says_how_long_its_turn_has_run() {
        let now = SystemTime::now();
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, now);
        let view = TaskView {
            phase: Some(crate::tasks::Phase::Code),
            turn_started: Some(now - Duration::from_secs(12 * 60 + 30)),
            ..TaskView::default()
        };
        let slot = scene.agents.get_mut(&a).unwrap();
        slot.state = ActivityState::Running {
            item: Some(ToolUse {
                tool_use_id: None,
                detail: None,
                kind: pixtuoid_core::state::ToolKind::Edit,
            }),
        };
        assert_eq!(status(slot, Some(&view), false).0, "▸ 写码 12m");
        slot.state = ActivityState::Running { item: None };
        assert_eq!(status(slot, Some(&view), false).0, "… 思考中 12m");
        slot.state = ActivityState::RequiresAction { reason: None };
        assert_eq!(status(slot, Some(&view), false).0, "! 等你确认");
        // No start known: the card says nothing rather than guess.
        slot.state = ActivityState::Running { item: None };
        assert_eq!(
            status(slot, Some(&TaskView::default()), false).0,
            "… 思考中"
        );
    }

    /// A finished card says what the turn came to, so you can tell which to
    /// read first without opening each; a question for you stands out in the
    /// waiting colour, a raw reply reads dim, and a working card shows its
    /// current call instead.
    #[test]
    fn a_finished_card_says_what_the_turn_came_to() {
        let now = SystemTime::now();
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, now);
        let slot = scene.agents.get_mut(&a).unwrap();
        slot.state = ActivityState::Idle;
        let done = |o: &str, summarized: bool| TaskView {
            outcome: Some(o.into()),
            outcome_summarized: summarized,
            ..TaskView::default()
        };
        assert_eq!(
            doing(slot, Some(&done("登录页已修好", true))),
            Some(("登录页已修好".into(), TEXT))
        );
        assert_eq!(
            doing(slot, Some(&done("问：要不要删旧接口", true))).map(|d| d.1),
            Some(WAIT)
        );
        assert_eq!(
            doing(slot, Some(&done("已修好，补了测试", false))).map(|d| d.1),
            Some(DIM)
        );
        slot.state = ActivityState::Running { item: None };
        assert_eq!(doing(slot, Some(&done("登录页已修好", true))), None);
        // Claude's idle prompt moves it to 等你批; the question stays shown.
        slot.state = ActivityState::RequiresAction {
            reason: Some("Claude is waiting for your input".into()),
        };
        assert_eq!(
            doing(slot, Some(&done("问：要不要删旧接口", true))).map(|d| d.0),
            Some("等你回复：问：要不要删旧接口".into())
        );
    }

    #[test]
    fn a_turn_counts_from_a_minute_and_in_hours_past_one() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let after = |s| turn_took(t0, t0 + Duration::from_secs(s));
        assert_eq!(after(59), None);
        assert_eq!(after(60).as_deref(), Some("1m"));
        assert_eq!(after(3599).as_deref(), Some("59m"));
        assert_eq!(after(3600 + 5 * 60).as_deref(), Some("1h05m"));
        // A clock that went backwards is no turn length at all.
        assert_eq!(turn_took(t0 + Duration::from_secs(5), t0), None);
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
