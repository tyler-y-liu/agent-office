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
/// Waiting and working in the office's own badge colours (the harbour
/// theme's), so a card and its figure's name read as the same state.
pub const WAIT: Color = Color::Rgb(0xe0, 0xaf, 0x68);
const WORK: Color = Color::Rgb(0x9e, 0xce, 0x6a);
const STUCK: Color = Color::Rgb(0xf7, 0x76, 0x8e);
const SELECTED_BG: Color = Color::Rgb(0x2a, 0x2f, 0x3a);
/// The panel's own background, which a faded card's colours sink toward.
const PANEL_BG: Color = Color::Rgb(0x16, 0x18, 0x1d);
/// A card waiting on you sits on a solid warm ground: it is the one to act on.
const WAIT_BG: Color = Color::Rgb(0x4a, 0x30, 0x1e);
const WAIT_SELECTED_BG: Color = Color::Rgb(0x5e, 0x3e, 0x28);
/// Secondary text on that warm ground, where [`DIM`] fell to 3.7:1.
const DIM_ON_WAIT: Color = Color::Rgb(0xb8, 0xbc, 0xc4);
/// A worker with a post: a neutral avatar, its chip says who it is.
const ROLE_AVATAR: Color = Color::Rgb(0x6b, 0x71, 0x7e);
/// The card under the pointer, which a click selects.
const HOVER_BG: Color = Color::Rgb(0x1f, 0x23, 0x2b);
/// How far an idle card's colours sink into the panel: it needs nothing,
/// but its dim text must still read (past this it fell near 2:1).
const IDLE_FADE: f32 = 0.35;
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
    /// The card under the pointer.
    pub hovered: Option<AgentId>,
    /// Task lines are summarized: a raw one is then marked as not yet.
    pub summaries: bool,
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
                    v.needs_task
                        || v.outcome_summarized
                            && v.outcome.as_deref().is_some_and(|o| crate::i18n::is_ask(o))
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

    /// The group's header: what it needs from you. Where its agents are is
    /// left out: they walk there, and a room named over someone still at
    /// their desk sent you looking in the wrong place.
    pub fn title(self) -> &'static str {
        match self {
            Self::Waiting => crate::i18n::t("等你", "Waiting"),
            Self::Stuck => crate::i18n::t("卡住", "Stuck"),
            Self::Done => crate::i18n::t("做完待看", "Done, to read"),
            Self::Working => crate::i18n::t("干活", "Working"),
            Self::Idle => crate::i18n::t("闲着", "Idle"),
        }
    }

    /// The word the panel's header counts this group by.
    fn tally_word(self) -> &'static str {
        match self {
            Self::Done => crate::i18n::t("待看", "To read"),
            g => g.title(),
        }
    }

    /// Its colour, the same on the panel's header and cards and on its
    /// agents' name badges in the office.
    pub fn color(self) -> Color {
        match self {
            Self::Waiting => WAIT,
            Self::Stuck => STUCK,
            Self::Done => TASK,
            Self::Working => WORK,
            Self::Idle => DIM,
        }
    }
}

/// The panel's header: how many sessions need what of you, in the order the
/// groups are listed, and the working and idle after. A group with no one in
/// it is left out; with no one needing you it says so first, so a glance
/// answers "do I have anything to do" without reading the cards.
fn header_spans(roots: &[Group], waiting: usize) -> Vec<Span<'static>> {
    let count = |g: Group| match g {
        // The footer's and the Dock's count, a subagent's prompt included.
        Group::Waiting => waiting,
        g => roots.iter().filter(|&&r| r == g).count(),
    };
    let bold = |c: Color| Style::default().fg(c).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    let needs = [Group::Waiting, Group::Stuck, Group::Done];
    if needs.iter().all(|&g| count(g) == 0) {
        spans.push(Span::styled(
            crate::i18n::t("暂时不用你", "Nothing needs you"),
            bold(TEXT),
        ));
    }
    for g in needs.into_iter().chain([Group::Working, Group::Idle]) {
        let n = count(g);
        if n == 0 {
            continue;
        }
        if spans.len() > 1 {
            spans.push(Span::styled(" · ", Style::default().fg(DIM)));
        }
        let style = if needs.contains(&g) {
            bold(g.color())
        } else {
            Style::default().fg(g.color())
        };
        spans.push(Span::styled(format!("{n} {}", g.tally_word()), style));
    }
    spans.push(Span::raw(" "));
    spans
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

/// The sessions in `order` waiting on you ([`waits_on_you`]): the one count
/// the panel's title, the footer and the waiting room's ticket machine show,
/// so no two of them disagree.
pub fn waiting(
    scene: &SceneState,
    views: &TaskViews,
    order: &[(AgentId, u8)],
    seen: &HashMap<AgentId, SystemTime>,
) -> Vec<AgentId> {
    order
        .iter()
        .filter_map(|(id, _)| scene.agents.get(id))
        .filter(|s| waits_on_you(s, views.get(&s.agent_id), read_since_idle(s, seen)).is_some())
        .map(|s| s.agent_id)
        .collect()
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
fn status(
    slot: &AgentSlot,
    view: Option<&TaskView>,
    read: bool,
    now: SystemTime,
) -> (String, Color) {
    if slot.exiting_at.is_some() {
        return (crate::i18n::t("离开中", "leaving").into(), DIM);
    }
    // Waiting on the user says what to do; a stuck verdict is for one working.
    if let Some(s) = crate::tasks::stuck_at_work(slot, view) {
        return (format!("{STUCK_MARK} {}", s.describe()), STUCK);
    }
    match &slot.state {
        ActivityState::RequiresAction { .. } => (
            crate::i18n::t("! 等你确认", "! needs approval").into(),
            WAIT,
        ),
        ActivityState::Running { item } => {
            let what = match (item, view.and_then(|v| v.phase)) {
                (Some(_), Some(p)) => format!("▸ {}", p.label()),
                (Some(_), None) => crate::i18n::t("▸ 工作中", "▸ working").to_owned(),
                (None, _) => crate::i18n::t("… 思考中", "… thinking").to_owned(),
            };
            let took = view
                .and_then(|v| v.turn_started)
                .and_then(|t| turn_took(t, now));
            match took {
                Some(took) => (format!("{what} {took}"), WORK),
                None => (what, WORK),
            }
        }
        // Opened or not, the question stays unanswered until you reply.
        ActivityState::Idle if waits_on_you(slot, view, false) == Some(Wait::Reply) => {
            let what = if view.is_some_and(|v| v.needs_task) {
                needs_task()
            } else {
                reply_wait()
            };
            (format!("? {what} {}", ago(slot.state_started_at)), WAIT)
        }
        // Read, it is up on the rooftop however short its idle stretch: the
        // card says why, or "空闲 7m" up there reads as a bug.
        ActivityState::Idle if read => (
            crate::tr!(
                "✓ 已看 · 空闲 {}",
                "✓ read · idle {}",
                ago(slot.state_started_at)
            ),
            DIM,
        ),
        // In the pantry to be read: its chip says done, in its group's
        // colour, not idle under a header that says to read it.
        ActivityState::Idle if Group::of(slot, view, now, read) == Group::Done => (
            crate::tr!("✓ 做完 {}", "✓ done {}", ago(slot.state_started_at)),
            TASK,
        ),
        ActivityState::Idle => (
            crate::tr!("· 空闲 {}", "· idle {}", ago(slot.state_started_at)),
            DIM,
        ),
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
                Some(o) if wait == reply_wait() => {
                    Some((crate::tr!("{wait}：{o}", "{wait}: {o}"), WAIT))
                }
                _ => Some((wait, WAIT)),
            }
        }
        // Done: what it came to, so you know which to read first.
        ActivityState::Idle if slot.exiting_at.is_none() => {
            let v = view?;
            let o = v.outcome.clone()?;
            let color = if !v.outcome_summarized {
                DIM
            } else if crate::i18n::is_ask(&o) {
                WAIT
            } else {
                TEXT
            };
            Some((o, color))
        }
        _ => None,
    }
}

/// Dispatched in a role with no task: it waits for you to give it one.
pub fn needs_task() -> &'static str {
    crate::i18n::t("等你写任务", "Needs a task")
}

/// The agent finished its turn and waits on your answer.
fn reply_wait() -> &'static str {
    crate::i18n::t("等你回复", "Waiting for your reply")
}

/// What the agent waits on, in words. pixtuoid's reason is `"permission"`
/// for a PermissionRequest hook and Claude's own English line for a
/// Notification; the call awaiting approval is already the transcript's last.
fn waiting_on(reason: Option<&str>, last_tool: Option<&str>) -> String {
    match reason {
        Some("permission") | None => match last_tool {
            Some(t) => crate::tr!("等你批准：{t}", "Approve: {t}"),
            None => crate::i18n::t("等你批准工具调用", "Waiting for you to approve a tool call")
                .to_owned(),
        },
        Some(r) if r.contains("waiting for your input") => reply_wait().to_owned(),
        Some(r) if r.contains("needs your permission") => match last_tool {
            Some(t) => crate::tr!("等你批准：{t}", "Approve: {t}"),
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

/// What a stuck agent is marked with, on its chip and by its badge in the
/// office alike, so the two read as one state.
pub const STUCK_MARK: &str = "⚠";

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
    /// The rows (first, last) of each card marked with a bar: done, waiting
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

/// How a card is drawn beyond its agent: what its badge calls it, its
/// repository's colour, and the panel's state for it.
struct CardLook<'a> {
    name: &'a str,
    ink: Option<Rgb>,
    selected: bool,
    expanded: bool,
    read: bool,
    summaries: bool,
}

fn card(
    slot: &AgentSlot,
    depth: u8,
    view: Option<&TaskView>,
    look: &CardLook<'_>,
    w: u16,
) -> Vec<Line<'static>> {
    let CardLook {
        ink,
        expanded,
        read,
        ..
    } = *look;
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
    let (st, st_color) = status(slot, view, read, SystemTime::now());
    let role = view.and_then(|v| v.role).map(crate::roles::Role::post);
    // A worker with a post is told apart by its chip: a coloured avatar
    // beside it read as a second role.
    let marker = if role.is_some() {
        ROLE_AVATAR
    } else {
        ink.map_or(DIM, rgb)
    };
    // A role's chip says what an `office-*` type would.
    let name = match view.and_then(|v| v.agent_type.as_deref()) {
        Some(t) if depth > 0 && role.is_none() && !look.name.contains(t) => {
            format!("{t} {}", look.name)
        }
        _ => look.name.to_owned(),
    };
    // The selected card says whether Enter opens or folds it.
    let name = match (look.selected, expanded) {
        (true, true) => format!("▾ {name}"),
        (true, false) => format!("▸ {name}"),
        _ => name,
    };
    let chip = format!(" {st} ");
    let avatar = format!(" {} ", avatar_initial(&slot.label));
    let role_chip = role.map(|p| format!("{} ", p.role.chip()));
    let name_w = w.saturating_sub(
        indent.chars().count() as u16
            + crate::text::cells(&avatar)
            + role_chip.as_deref().map_or(0, crate::text::cells)
            + 1
            + crate::text::cells(&chip)
            + 1,
    );
    let mut first = vec![
        Span::raw(indent.clone()),
        Span::styled(
            avatar,
            Style::default()
                .bg(marker)
                .fg(CHIP_INK)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let (Some(p), Some(c)) = (role, role_chip) {
        first.push(Span::styled(
            c,
            Style::default()
                .bg(rgb(crate::roles::CHIP_BG))
                .fg(rgb(p.ink))
                .add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![Line::from(
        first
            .into_iter()
            .chain([
                Span::raw(" "),
                Span::styled(
                    truncate_cells(&name, name_w),
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(chip, Style::default().bg(st_color).fg(CHIP_INK)),
            ])
            .collect::<Vec<_>>(),
    )];
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
            // With summaries off a raw line is all there is: no mark for it.
            let mark =
                (look.summaries && !pending).then(|| crate::i18n::t(" ·未总结", " ·unsummarized"));
            let room = 2 + mark.map_or(0, crate::text::cells);
            spans.push(Span::styled(
                truncate_cells(&t, body_w.saturating_sub(room)),
                Style::default().fg(DIM),
            ));
            if let Some(mark) = mark {
                spans.push(Span::styled(mark, Style::default().fg(DIM)));
            }
        }
        None => {
            let none = match (role, view.is_some_and(|v| v.needs_task)) {
                (Some(p), true) => crate::tr!("{} · 还没给任务", "{} · no task yet", p.role.name()),
                _ => crate::i18n::t("（还没有指令）", "(no prompt yet)").to_owned(),
            };
            spans.push(Span::styled(none, Style::default().fg(DIM)));
        }
    }
    if pending {
        spans.push(Span::styled(" …", Style::default().fg(DIM)));
    }
    lines.push(Line::from(spans));
    if let Some((d, c)) = doing(slot, view) {
        lines.push(Line::from(vec![
            Span::raw(pad.to_owned()),
            Span::styled(
                format!(
                    "↳ {}",
                    crate::text::fit_tool_line(&d, &slot.cwd, body_w.saturating_sub(2))
                ),
                Style::default().fg(c),
            ),
        ]));
    }
    if expanded && let Some(v) = view {
        if let Some(p) = &v.prompt {
            lines.push(Line::from(Span::styled(
                format!("{pad}{}", crate::i18n::t("指令", "Prompt")),
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
                format!("{pad}{}", crate::i18n::t("最后回复", "Last reply")),
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
                Span::styled(
                    format!("{pad}{}", crate::i18n::t("待办 ", "Todo ")),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    truncate_cells(t, body_w.saturating_sub(5)),
                    Style::default().fg(TEXT),
                ),
            ]));
        }
        if !v.recent_tools.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("{pad}{}", crate::i18n::t("最近动作", "Recent actions")),
                Style::default().fg(DIM),
            )));
            for t in v.recent_tools.iter().rev() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{pad}· {}",
                        crate::text::fit_tool_line(t, &slot.cwd, body_w.saturating_sub(2))
                    ),
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
    let names = pixtuoid_scene::badge::shown_names(scene.agents.values());
    // Counted as the footer and the Dock count: a question to you waits as a
    // prompt does.
    let waiting = waiting(scene, views, &order, &state.seen).len();
    let roots: Vec<Group> = order
        .iter()
        .filter(|(_, d)| *d == 0)
        .filter_map(|(id, _)| scene.agents.get(id))
        .map(|s| group_of(s))
        .collect();
    let title = Line::from(header_spans(&roots, waiting));
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::Rgb(0x3b, 0x40, 0x4c)))
        .title(title);
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
                crate::i18n::t("还没有 agent 会话在运行", "No agent sessions running yet"),
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
            let look = CardLook {
                name: names.get(id).map_or(&*slot.label, String::as_str),
                // The repository's colour, as its sitters' badges wear it in
                // the office.
                ink: pixtuoid_scene::badge::repo_hue(slot).or_else(|| inks.get(id).copied()),
                selected: sel,
                expanded: sel && state.expanded,
                read: read_since_idle(slot, &state.seen),
                summaries: state.summaries,
            };
            let mut lines = card(slot, *depth, views.get(id), &look, inner.width);
            // A subagent's card goes with its root's.
            if *depth == 0 {
                root_group = group_of(slot);
            }
            let group = root_group;
            if group == Group::Idle {
                // It needs nothing from you: its name and chip are enough,
                // and the busy cards above keep the room.
                if !sel {
                    lines.truncate(1);
                }
                lines = lines.into_iter().map(faded).collect();
            }
            if group == Group::Waiting {
                lines = lines.into_iter().map(on_wait).collect();
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
                                .fg(Group::color(group))
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
                let own = i >= c.lead;
                // A group's header and the blank over it are no card's: a
                // click there selects nothing, and no card's ground covers it.
                if own {
                    drawn.rows.push((y, c.id));
                }
                let hover = state.hovered == Some(c.id);
                let bg = match (own, c.group == Group::Waiting, sel) {
                    (false, _, _) => None,
                    (true, true, true) => Some(WAIT_SELECTED_BG),
                    (true, true, false) => Some(WAIT_BG),
                    (true, false, true) => Some(SELECTED_BG),
                    (true, false, false) => hover.then_some(HOVER_BG),
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
    // Cards past either edge, marked on the title row, which holds no card:
    // without a mark a long list looked like it ended at the window.
    let more = match (start > 0, y > start + h) {
        (true, true) => Some(" ▲▼ "),
        (true, false) => Some(" ▲ "),
        (false, true) => Some(" ▼ "),
        (false, false) => None,
    };
    if let Some(more) = more {
        let w = crate::text::cells(more);
        let x = inner.x + inner.width.saturating_sub(w);
        buf.set_string(x, area.y, more, Style::default().fg(TEXT).bg(SELECTED_BG));
    }
    drawn
}

/// `line` with its dim text lifted to read on a waiting card's ground.
fn on_wait(line: Line<'static>) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|mut s| {
            if s.style.fg == Some(DIM) {
                s.style.fg = Some(DIM_ON_WAIT);
            }
            s
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::CC_SOURCE;
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    /// The header answers "is there anything for me" at a glance: what needs
    /// you is counted by name, in its group's colour, and with nothing
    /// needing you it says so rather than leaving you to read every card.
    #[test]
    fn the_header_counts_what_needs_you_first() {
        let text = |sp: &[Span]| sp.iter().map(|s| s.content.as_ref()).collect::<String>();
        let roots = [
            Group::Done,
            Group::Working,
            Group::Stuck,
            Group::Done,
            Group::Idle,
        ];
        let spans = header_spans(&roots, 1);
        assert_eq!(
            text(&spans).trim(),
            "1 等你 · 1 卡住 · 2 待看 · 1 干活 · 1 闲着"
        );
        let stuck = spans.iter().find(|s| s.content.contains("卡住")).unwrap();
        assert_eq!(stuck.style.fg, Some(STUCK));
        let calm = header_spans(&[Group::Working, Group::Working], 0);
        assert_eq!(text(&calm).trim(), "暂时不用你 · 2 干活");
    }

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
        assert_eq!(status(slot, Some(&view), false, SystemTime::now()).1, STUCK);
        slot.state = ActivityState::RequiresAction { reason: None };
        assert!(crate::tasks::stuck_at_work(slot, Some(&view)).is_none());
        assert_eq!(status(slot, Some(&view), false, SystemTime::now()).1, WAIT);
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

    /// A group's header belongs to no card: clicking it selected the card
    /// under it, and a selected card's ground spread over the header and the
    /// blank above it, so the selection looked a row taller than its card.
    #[test]
    fn a_selection_covers_its_card_and_not_its_groups_header() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, t0);
        let views = TaskViews::default();
        let area = Rect::new(0, 0, 40, 12);
        let mut state = PanelState {
            selected: Some(a),
            ..PanelState::default()
        };
        let mut buf = Buffer::empty(area);
        let drawn = render(&mut buf, area, &scene, &views, &HashMap::new(), &mut state);
        let first = drawn.rows.first().map(|(y, _)| *y).expect("a card");
        let header = first - 1;
        let header_text: String = (0..area.width)
            .map(|x| buf[(x, header)].symbol().to_owned())
            .collect();
        assert!(
            header_text.contains('('),
            "the row over the card is its header"
        );
        assert!(drawn.rows.iter().all(|(y, _)| *y != header), "no card's");
        assert_ne!(buf[(area.width - 1, header)].bg, SELECTED_BG);
        assert_eq!(buf[(area.width - 1, first)].bg, SELECTED_BG);
    }

    /// A session idle on the rooftop needs nothing from you: its card is its
    /// name and state alone, leaving the room to the cards that do, until you
    /// select it.
    #[test]
    fn an_idle_card_is_one_line_until_selected() {
        let now = SystemTime::now();
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(
            &mut scene,
            &mut r,
            "a",
            None,
            now - Duration::from_secs(7200),
        );
        let slot = scene.agents.get_mut(&a).unwrap();
        slot.state = ActivityState::Idle;
        slot.state_started_at = now - Duration::from_secs(3600);
        let views = TaskViews::default();
        let area = Rect::new(0, 0, 40, 12);
        let rows = |state: &mut PanelState| {
            let mut buf = Buffer::empty(area);
            render(&mut buf, area, &scene, &views, &HashMap::new(), state)
                .rows
                .iter()
                .filter(|(_, id)| *id == a)
                .count()
        };
        let mut state = PanelState::default();
        assert_eq!(rows(&mut state), 1);
        state.selected = Some(a);
        assert!(rows(&mut state) > 1, "selected, it shows its task");
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

    /// A role dispatched with no task can only ask what to do. With
    /// summaries off nothing reads its reply as a question, and it used to
    /// land in the pantry as done, while it waited on you for its task.
    #[test]
    fn no_task_dispatch_waits_on_you() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let now = t0 + Duration::from_secs(3600);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let id = start(&mut scene, &mut r, "ux", None, t0);
        let s = scene.agents.get_mut(&id).unwrap();
        s.state = ActivityState::Idle;
        s.state_started_at = now - Duration::from_secs(20);
        let view = TaskView {
            role: Some(crate::roles::Role::Ux),
            needs_task: true,
            // Summaries off: the raw reply, not the model's 「问：」.
            outcome: Some("这次要我看哪个页面或流程？".into()),
            outcome_summarized: false,
            ..TaskView::default()
        };
        let slot = &scene.agents[&id];
        assert_eq!(waits_on_you(slot, Some(&view), false), Some(Wait::Reply));
        assert_eq!(Group::of(slot, Some(&view), now, false), Group::Waiting);
        let (chip, color) = status(slot, Some(&view), false, now);
        assert!(chip.contains("等你写任务"), "{chip}");
        assert_eq!(color, WAIT);
        // Opened, you have seen what it asked, as with any question.
        assert_eq!(waits_on_you(slot, Some(&view), true), None);
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
            status(
                &scene.agents[&asked],
                views.get(&asked),
                false,
                SystemTime::now()
            )
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
        let look = CardLook {
            name: &slot.label,
            ink: Some(ink),
            selected: false,
            expanded: false,
            read: false,
            summaries: true,
        };
        let lines = card(slot, 0, None, &look, 40);
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

    /// The post is what tells two sessions of one repository apart at a
    /// glance: a role's card wears its chip, while a built-in subagent type
    /// is no post and still reads as before its name.
    #[test]
    fn a_role_card_wears_its_chip_and_explore_keeps_its_type() {
        use crate::roles::Role;
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let a = start(&mut scene, &mut r, "a", None, t0);
        let slot = scene.agents.get(&a).unwrap();
        let look = CardLook {
            name: "agent-office",
            ink: None,
            selected: false,
            expanded: false,
            read: false,
            summaries: true,
        };
        let qa = TaskView {
            role: Some(Role::Qa),
            agent_type: Some("office-qa".into()),
            ..TaskView::default()
        };
        let lines = card(slot, 1, Some(&qa), &look, 40);
        let first = &lines[0].spans;
        let chip = first
            .iter()
            .find(|s| s.style.fg == Some(rgb(Role::Qa.post().ink)))
            .expect("a role chip");
        // The role's ink is the text on a dark chip, not a block of colour
        // that outweighs the status beside it.
        assert_eq!(chip.style.bg, Some(rgb(crate::roles::CHIP_BG)));
        assert_eq!(chip.content.trim(), "测");
        assert!(!first.iter().any(|s| s.content.contains("office-qa")));
        let explore = TaskView {
            agent_type: Some("Explore".into()),
            ..TaskView::default()
        };
        let lines = card(slot, 1, Some(&explore), &look, 40);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("Explore agent-office")));
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
        assert!(
            status(&slot, None, true, now)
                .0
                .starts_with("✓ 已看 · 空闲 ")
        );
        // Unread, it is in the pantry under 做完待看: its chip says done, not
        // idle, which read as nothing to look at.
        assert!(status(&slot, None, false, now).0.starts_with("✓ 做完 "));
        let later = now + Duration::from_secs(20 * 60);
        assert!(status(&slot, None, false, later).0.starts_with("· 空闲 "));
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
        assert_eq!(
            status(slot, Some(&view), false, SystemTime::now()).0,
            "▸ 写码 12m"
        );
        slot.state = ActivityState::Running { item: None };
        assert_eq!(
            status(slot, Some(&view), false, SystemTime::now()).0,
            "… 思考中 12m"
        );
        slot.state = ActivityState::RequiresAction { reason: None };
        assert_eq!(
            status(slot, Some(&view), false, SystemTime::now()).0,
            "! 等你确认"
        );
        // No start known: the card says nothing rather than guess.
        slot.state = ActivityState::Running { item: None };
        assert_eq!(
            status(slot, Some(&TaskView::default()), false, SystemTime::now()).0,
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
