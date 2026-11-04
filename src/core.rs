//! What the window acts on, apart from how it draws: the live scene, the task
//! lines, the selection, and what each key does to them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentId, SceneState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::panel::{self, PanelState};
use crate::{jump, pipeline, tasks, text};

/// An office that has floors to move between.
pub trait Floors {
    fn current_floor(&self) -> usize;
    fn n_floors(&self) -> usize;
    fn navigate(&mut self, floor: usize, now: SystemTime);
}

/// What a key asks for, whichever front end read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Back,
    Down,
    Up,
    Expand,
    Resummarize,
    ToggleLabels,
    NextFloor,
    PrevFloor,
    /// Open the selected agent's session in the desktop app.
    Open,
    /// Open the desktop app's list of sessions waiting on the user.
    OpenWaiting,
}

/// How long a note on what a key did stays in the footer.
const NOTE_FOR: Duration = Duration::from_secs(5);

pub struct Core {
    pub ui: PanelState,
    pub head_labels: bool,
    pub summary_note: String,
    /// What the last jump did, shown in place of `summary_note` a while.
    note: Option<(String, Instant)>,
    pub pipe: pipeline::Pipeline,
    views_rx: tokio::sync::watch::Receiver<tasks::TaskViews>,
    force_tx: tokio::sync::mpsc::UnboundedSender<AgentId>,
}

/// One frame's worth of state.
pub struct Frame {
    pub scene: Arc<SceneState>,
    pub views: tasks::TaskViews,
    /// The head label per agent; empty while they are hidden.
    pub labels: HashMap<AgentId, String>,
}

impl Core {
    /// Start the sources and the task loop on `rt`, the floors seating `caps`.
    pub fn start(rt: &tokio::runtime::Runtime, caps: [usize; MAX_FLOORS]) -> Self {
        let summarizer = tasks::client::Summarizer::from_env();
        let summary_note = match &summarizer {
            Some(s) => format!("摘要 {}", s.model()),
            None => "摘要关闭".to_owned(),
        };
        let socket = crate::hook::socket_path();
        let (pipe, views_rx, force_tx) = rt.block_on(async {
            let pipe = pipeline::spawn(socket, caps);
            let (views_rx, force_tx) = tasks::spawn(pipe.scene_rx.clone(), summarizer);
            (pipe, views_rx, force_tx)
        });
        Self {
            ui: PanelState::default(),
            head_labels: true,
            summary_note,
            note: None,
            pipe,
            views_rx,
            force_tx,
        }
    }

    pub fn scene(&self) -> Arc<SceneState> {
        self.pipe.scene_rx.borrow().clone()
    }

    /// The state to draw now; drops a selection whose agent left.
    pub fn frame(&mut self) -> Frame {
        let scene = self.scene();
        let views = self.views_rx.borrow().clone();
        let order = panel::order(&scene);
        if self
            .ui
            .selected
            .is_some_and(|id| !order.iter().any(|(o, _)| *o == id))
        {
            self.ui.selected = None;
            self.ui.expanded = false;
        }
        let labels = if self.head_labels {
            views
                .iter()
                .filter_map(|(id, v)| Some((*id, v.line.clone()?)))
                .collect()
        } else {
            HashMap::new()
        };
        Frame {
            scene,
            views,
            labels,
        }
    }

    /// Select `id` and show the floor it sits on.
    pub fn select(&mut self, id: AgentId, floors: &mut impl Floors) {
        if self.ui.selected == Some(id) {
            self.ui.expanded = !self.ui.expanded;
        } else {
            self.ui.selected = Some(id);
        }
        self.follow(floors);
    }

    fn follow(&self, floors: &mut impl Floors) {
        let scene = self.scene();
        if let Some(slot) = self.ui.selected.and_then(|id| scene.agents.get(&id))
            && slot.floor_idx != floors.current_floor()
        {
            floors.navigate(slot.floor_idx, SystemTime::now());
        }
    }

    /// Whether `action` quits.
    pub fn act(&mut self, action: Action, floors: &mut impl Floors) -> bool {
        match action {
            Action::Quit => return true,
            Action::Back if self.ui.expanded => self.ui.expanded = false,
            Action::Back => self.ui.selected = None,
            Action::Down | Action::Up => {
                let delta = if action == Action::Down { 1 } else { -1 };
                let order = panel::order(&self.scene());
                self.ui.selected = panel::step(&order, self.ui.selected, delta);
                self.follow(floors);
            }
            Action::Expand if self.ui.selected.is_some() => self.ui.expanded = !self.ui.expanded,
            Action::Expand => {}
            Action::Resummarize => {
                if let Some(id) = self.ui.selected {
                    let _ = self.force_tx.send(id);
                }
            }
            Action::ToggleLabels => self.head_labels = !self.head_labels,
            Action::NextFloor => {
                let last = floors.n_floors().saturating_sub(1);
                let next = (floors.current_floor() + 1).min(last);
                floors.navigate(next, SystemTime::now());
            }
            Action::PrevFloor => {
                let prev = floors.current_floor().saturating_sub(1);
                floors.navigate(prev, SystemTime::now());
            }
            Action::Open => {
                let done = match self.ui.selected {
                    Some(id) => jump::open_agent(&self.scene(), id),
                    None => Err("先选中一个 agent".to_owned()),
                };
                self.note_result(done);
            }
            Action::OpenWaiting => self.note_result(jump::open_url(jump::NEEDS_INPUT)),
        }
        false
    }

    fn note_result(&mut self, done: Result<(), String>) {
        let text = done.map_or_else(|e| format!("跳转：{e}"), |()| "已在 Claude 打开".to_owned());
        self.note = Some((text, Instant::now()));
    }

    /// The footer's right-hand note: a fresh jump result, else the summary model.
    fn status_note(&self) -> &str {
        match &self.note {
            Some((text, at)) if at.elapsed() < NOTE_FOR => text,
            _ => &self.summary_note,
        }
    }
}

/// The key hints and the floor and summary status along the bottom.
pub fn footer(
    buf: &mut Buffer,
    area: Rect,
    floors: &impl Floors,
    core: &Core,
    extra: &[(&str, &str)],
) {
    let dim = Style::default().fg(Color::Rgb(0x8a, 0x8f, 0x98));
    let key = Style::default().fg(Color::Rgb(0xe6, 0xe6, 0xe6));
    let labels = if core.head_labels {
        "隐藏头顶"
    } else {
        "显示头顶"
    };
    let mut spans = Vec::new();
    let base = [
        ("↑↓", "选中"),
        ("Enter", "展开"),
        ("o", "打开会话"),
        ("w", "等你的"),
        ("r", "重新总结"),
        ("b", labels),
        ("[ ]", "换楼层"),
    ];
    for (k, what) in base.iter().chain(extra).chain(&[("q", "退出")]) {
        spans.push(Span::styled(format!(" {k} "), key));
        spans.push(Span::styled(format!("{what} "), dim));
    }
    let right = format!(
        "F{}/{}  {} ",
        floors.current_floor() + 1,
        floors.n_floors().max(1),
        core.status_note()
    );
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
    let w = text::cells(&right);
    if area.width > w {
        buf.set_line(
            area.x + area.width - w,
            area.y,
            &Line::from(Span::styled(right, dim)),
            w,
        );
    }
}
