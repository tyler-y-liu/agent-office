//! agent-office's window: an always-on-top desktop window drawing the office
//! as pixtuoid's floating window does, the cutaway at the pack's full density
//! upscaled to real pixels, with the task panel beside it in the same pixel
//! font. Drawn on the CPU (softbuffer): one frame is a copy and an upscale.

mod font;
mod vector;

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::{AgentId, SceneState};
use pixtuoid_scene::anim::Motion;
use pixtuoid_scene::display::{Align, Badge, Content, HostText, TextRole, TextRun};
use pixtuoid_scene::floor::{FloorInputs, FloorMeta, OfficeSession, PetInputs};
use pixtuoid_scene::layout::{Bounds, Size};
use pixtuoid_scene::look::{Dim, Picture, Place, RenderInputs, WorldText};
use pixtuoid_scene::pack::OfficeArt;
use pixtuoid_scene::pet::{Pet, PetKind};
use pixtuoid_scene::render_scale::PixelFit;
use pixtuoid_scene::sky::WeatherPolicy;
use pixtuoid_scene::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::core::{Action, Core, Floors};
use crate::panel;
use crate::text::truncate_cells;
use font::{Canvas, LINE_H, rgb};
use vector::Faces;

const FRAME: Duration = Duration::from_millis(50);
/// How long the window must keep its size before the office lays out for
/// it: every few pixels of a drag is a new layout, its desks and agents
/// reshuffled, so a drag redraws the office it started with.
const RESIZE_SETTLE: Duration = Duration::from_millis(300);
/// Two clicks on one agent this close together are a double click (macOS's
/// default interval is about this).
const DOUBLE_CLICK: Duration = Duration::from_millis(450);
/// Panel rows one notch of a mouse wheel scrolls, as a terminal does.
const WHEEL_ROWS: f64 = 3.0;
/// Cells across the panel.
const PANEL_COLS: u32 = 46;
/// Logical units a badge's plate sits over its anchor (`LABEL_GAP` in
/// pixtuoid-scene's `display/text.rs`).
const LABEL_GAP: u16 = 2;
/// A plate's padding and its height on the art grid (`PLATE_PAD`, `PLATE_H`
/// in `display/compose.rs`).
const PLATE_PAD: u32 = 1;
const PLATE_H: u32 = LINE_H + PLATE_PAD;
/// Cells a head label may take.
const LABEL_CELLS: u16 = 14;
const PANEL_BG: u32 = 0x0016_181d;
const TEXT: u32 = 0x00e6_e6e6;
const TASK: u32 = 0x00ff_e8a3;

/// How many real pixels a logical unit gets: what keeps the office about
/// 180 units tall, pixtuoid's `floating::geometry::office_scale`.
fn office_scale(h: u32) -> u16 {
    (f64::from(h) / 180.0).round().max(1.0) as u16
}

/// How long `--shot` lets the office settle: transcripts read, summaries
/// back, agents seated, and the idle ones walked out to their lounges.
const SHOT_AFTER: Duration = Duration::from_secs(20);

/// Open the window; with `shot`, save its frame there after [`SHOT_AFTER`]
/// and close: the window's own pixels, for checking the look.
pub fn run(rt: &tokio::runtime::Runtime, shot: Option<std::path::PathBuf>) -> Result<()> {
    let pack = Arc::new(pixtuoid_scene::pack::load_bundled_pack()?);
    let event_loop = EventLoop::new()?;
    let mut app = Floating {
        rt,
        pack,
        theme: theme(),
        win: None,
        error: None,
        shot: shot.map(|p| (p, Instant::now() + SHOT_AFTER)),
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

/// agent-office's own `harbour`, or the theme `AGENT_OFFICE_THEME` names.
fn theme() -> &'static Theme {
    std::env::var("AGENT_OFFICE_THEME")
        .ok()
        .and_then(|n| pixtuoid_scene::theme::theme_by_name(&n))
        .unwrap_or(&pixtuoid_scene::theme::HARBOUR)
}

struct Floating<'rt> {
    rt: &'rt tokio::runtime::Runtime,
    pack: Arc<OfficeArt>,
    theme: &'static Theme,
    win: Option<Win>,
    error: Option<anyhow::Error>,
    shot: Option<(std::path::PathBuf, Instant)>,
}

/// Everything that exists once the window does.
struct Win {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    core: Core,
    office: Office,
    on_top: bool,
    cursor: PhysicalPosition<f64>,
    /// A trackpad's scroll in pixels not yet a whole panel row.
    wheel_px: f64,
    /// The last frame's layout, which a click is read against.
    last: Option<Layout>,
    /// The agent last clicked and when, so a second click on it soon after
    /// opens its session.
    last_click: Option<(AgentId, Instant)>,
    /// The sign clicked for what its zone is for, by its text, and when.
    about: Option<(String, Instant)>,
    next_frame: Instant,
    occluded: bool,
    /// The frame, and the office at art density before its upscale: kept
    /// between frames so a frame allocates nothing.
    canvas: Canvas,
    art: Canvas,
    faces: Faces,
    hold: ResizeHold,
}

/// The office's fit across a window resize: the one shown holds while the
/// window is still changing size, and the new size's is taken once it has
/// settled for [`RESIZE_SETTLE`].
#[derive(Default)]
struct ResizeHold {
    shown: Option<PixelFit>,
    resized_at: Option<Instant>,
}

impl ResizeHold {
    fn resized(&mut self, now: Instant) {
        self.resized_at = Some(now);
    }

    /// The fit a frame at `now` draws, `want` the window's size's own.
    fn fit(&mut self, want: PixelFit, now: Instant) -> PixelFit {
        let resizing = self
            .resized_at
            .is_some_and(|at| now.duration_since(at) < RESIZE_SETTLE);
        match self.shown {
            Some(shown) if resizing => shown,
            _ => {
                self.resized_at = None;
                *self.shown.insert(want)
            }
        }
    }
}

/// Where the last frame put things, in physical pixels.
struct Layout {
    office_fit: PixelFit,
    panel_x: u32,
    grid: Grid,
    panel_rows: Vec<(u16, AgentId)>,
    /// Each badge and task label drawn, in window pixels.
    tag_hits: Vec<(Area, AgentId)>,
    /// Each door sign and bench plate drawn, by its text.
    sign_hits: Vec<(Area, String)>,
}

struct Office {
    session: OfficeSession,
    /// The office size the desk capacities were last published for.
    caps_size: Option<Size>,
    /// The shop cat that wanders every floor, as a cha chaan teng keeps one.
    pets: Vec<Pet>,
}

impl Floors for Office {
    fn current_floor(&self) -> usize {
        self.session.nav().current()
    }
    fn n_floors(&self) -> usize {
        self.session.n_floors()
    }
    fn navigate(&mut self, floor: usize, now: SystemTime) {
        self.session.navigate(floor, now);
    }
}

/// The panel's cell grid on this display: text of `pt` points, a cell the
/// monospace face's advance, a row the line and air around it.
#[derive(Clone, Copy)]
struct Grid {
    px: u32,
    cell_w: u32,
    row_h: u32,
}

/// Points the panel's text is set in, as a Mac app's list text.
const PANEL_PT: f64 = 13.0;

fn grid(faces: &mut Faces, scale_factor: f64, pt: f64) -> Grid {
    let px = (pt * scale_factor).round().max(8.0) as u32;
    Grid {
        px,
        cell_w: faces.cell_w(px),
        row_h: (f64::from(px) * 1.6).round() as u32,
    }
}

/// The smallest window whose office still lays out beside the panel at
/// grid `g` (pixtuoid's `min_window`, plus the panel). Redone when the
/// window changes displays, since the panel's width follows the display.
fn set_min_size(window: &Window, pack: &OfficeArt, g: Grid) {
    let min = pixtuoid_scene::layout::min_layout_size();
    let d = u32::from(pack.max_density_variant().get());
    window.set_min_inner_size(Some(winit::dpi::PhysicalSize::new(
        u32::from(min.w) * d + PANEL_COLS * g.cell_w,
        u32::from(min.h) * d + g.row_h,
    )));
}

/// The office's part of a `w`×`h` window, the rest the panel and footer.
fn office_px(w: u32, h: u32, g: Grid) -> (u32, u32) {
    (
        w.saturating_sub(PANEL_COLS * g.cell_w),
        h.saturating_sub(g.row_h),
    )
}

/// Units tall the office may run at the scale it is shown at before a new
/// scale is taken. The scale steps by the art's density (4, 8, 12...), so a
/// step halves or doubles the office: without this band a window an edge
/// away from a step flipped between two offices.
const KEEP_SCALE_UNITS: std::ops::RangeInclusive<u16> = 110..=300;

/// The fit of a `w`×`h` office, `shown` the one on screen: its scale while
/// it still lays out within [`KEEP_SCALE_UNITS`], else this size's own.
fn fit_for(pack: &OfficeArt, w: u32, h: u32, shown: Option<PixelFit>) -> PixelFit {
    let px = Size {
        w: u16::try_from(w).unwrap_or(u16::MAX),
        h: u16::try_from(h).unwrap_or(u16::MAX),
    };
    let min = pixtuoid_scene::layout::min_layout_size();
    let held = shown.map(|f| f.over(px)).filter(|f| {
        let l = f.logical();
        l.w >= min.w && l.h >= min.h && KEEP_SCALE_UNITS.contains(&l.h)
    });
    held.unwrap_or_else(|| {
        PixelFit::at_least_density(office_scale(h), pack.max_density_variant(), px)
    })
}

impl ApplicationHandler for Floating<'_> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        match self.open(el) {
            Ok(w) => self.win = Some(w),
            Err(e) => {
                self.error = Some(e);
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(win) = self.win.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => {
                if let Err(e) = win.redraw(&self.pack, self.theme) {
                    self.error = Some(e);
                    el.exit();
                }
            }
            WindowEvent::Occluded(hidden) => win.occluded = hidden,
            // Drawn now, not on the next tick: until a frame of the new size
            // is presented, macOS shows the old one pinned top-left.
            WindowEvent::Resized(_) => {
                win.hold.resized(Instant::now());
                if let Err(e) = win.redraw(&self.pack, self.theme) {
                    self.error = Some(e);
                    el.exit();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let g = grid(&mut win.faces, scale_factor, PANEL_PT);
                set_min_size(&win.window, &self.pack, g);
            }
            WindowEvent::CursorMoved { position, .. } => win.cursor = position,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => win.click(),
            WindowEvent::MouseWheel { delta, .. } => win.wheel(delta),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let action = match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Escape) => Action::Back,
                    Key::Named(NamedKey::ArrowDown) => Action::Down,
                    Key::Named(NamedKey::ArrowUp) => Action::Up,
                    Key::Named(NamedKey::Enter) => Action::Expand,
                    Key::Named(NamedKey::PageDown) => Action::NextFloor,
                    Key::Named(NamedKey::PageUp) => Action::PrevFloor,
                    Key::Character("q") => Action::Quit,
                    Key::Character("j") => Action::Down,
                    Key::Character("k") => Action::Up,
                    Key::Character("r") => Action::Resummarize,
                    Key::Character("b") => Action::ToggleLabels,
                    Key::Character("]") => Action::NextFloor,
                    Key::Character("[") => Action::PrevFloor,
                    Key::Character("o") => Action::Open,
                    Key::Character("w") => Action::OpenWaiting,
                    Key::Character("t") => {
                        win.on_top = !win.on_top;
                        win.window.set_window_level(if win.on_top {
                            WindowLevel::AlwaysOnTop
                        } else {
                            WindowLevel::Normal
                        });
                        return;
                    }
                    _ => return,
                };
                if win.core.act(action, &mut win.office) {
                    el.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let Some(win) = self.win.as_mut() else { return };
        let now = Instant::now();
        // Drawn here, not on a redraw request: macOS sends none to a covered
        // window, and the shot must not wait on one.
        if let Some((path, at)) = &self.shot
            && now >= *at
        {
            let saved = win
                .redraw(&self.pack, self.theme)
                .and_then(|()| save_png(&win.canvas, path));
            if let Err(e) = saved {
                self.error = Some(e);
            }
            el.exit();
            return;
        }
        // A hidden widget left running all day should cost nothing.
        if now >= win.next_frame && (!win.occluded || self.shot.is_some()) {
            win.next_frame = now + FRAME;
            win.window.request_redraw();
        }
        el.set_control_flow(ControlFlow::WaitUntil(win.next_frame));
    }
}

impl Floating<'_> {
    fn open(&self, el: &ActiveEventLoop) -> Result<Win> {
        let attrs = Window::default_attributes()
            .with_title("agent-office")
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(1180, 620))
            .with_min_inner_size(LogicalSize::new(720, 360));
        let window = Rc::new(el.create_window(attrs)?);
        // The smallest window whose office still lays out beside the panel
        // at this display's text scale (pixtuoid's `min_window`, plus ours).
        let mut faces = Faces::load()?;
        let g = grid(&mut faces, window.scale_factor(), PANEL_PT);
        set_min_size(&window, &self.pack, g);
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;
        // Seat the floors for the office the first frame will draw, so an
        // agent arriving before it lands on a desk that exists.
        let size = window.inner_size();
        let (ow, oh) = office_px(size.width, size.height, g);
        let logical = fit_for(&self.pack, ow, oh, None).logical();
        Ok(Win {
            window,
            surface,
            core: Core::start(self.rt, crate::pipeline::caps_for(logical.w, logical.h)),
            office: Office {
                session: OfficeSession::new(Arc::clone(&self.pack)),
                caps_size: None,
                pets: vec![Pet {
                    kind: PetKind::Cat,
                    name: "店长".to_string(),
                }],
            },
            on_top: true,
            cursor: PhysicalPosition::default(),
            wheel_px: 0.0,
            last: None,
            last_click: None,
            about: None,
            next_frame: Instant::now(),
            occluded: false,
            canvas: Canvas::new(0, 0, 0),
            art: Canvas::new(0, 0, 0),
            faces,
            hold: ResizeHold::default(),
        })
    }
}

impl Win {
    /// Draw a frame into [`Self::canvas`] and present it.
    fn redraw(&mut self, pack: &OfficeArt, theme: &'static Theme) -> Result<()> {
        let size = self.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        self.surface
            .resize(w, h)
            .map_err(|e| anyhow::anyhow!("resize: {e}"))?;
        let sf = self.window.scale_factor();
        let g = grid(&mut self.faces, sf, PANEL_PT);
        let (ow, oh) = office_px(w.get(), h.get(), g);
        let bg = to_u32(theme.surface.bg_fallback);
        let mut canvas = std::mem::replace(&mut self.canvas, Canvas::new(0, 0, 0));
        canvas.reset(w.get(), h.get(), bg);
        let frame = self.core.frame();

        let want = fit_for(pack, ow, oh, self.hold.shown);
        let fit = self.hold.fit(want, Instant::now());
        if self.office.caps_size != Some(fit.logical()) {
            self.office.caps_size = Some(fit.logical());
            crate::pipeline::store_caps(
                &self.core.pipe.floor_caps,
                fit.logical().w,
                fit.logical().h,
            );
        }
        let min = pixtuoid_scene::layout::min_layout_size();
        let mut tag_hits = Vec::new();
        let mut sign_hits = Vec::new();
        let mut inks = HashMap::new();
        if fit.logical().w < min.w || fit.logical().h < min.h {
            let base = i64::from(g.row_h) + i64::from(self.faces.ascent(g.px));
            self.faces.draw(
                &mut canvas,
                16,
                base,
                "窗口太小，放大后显示办公室",
                TEXT,
                g.px,
            );
        } else if let Some((tags, signs)) =
            self.render_office(&frame.office, fit, theme, &frame.labels, sf)
        {
            canvas.blit_scaled(&self.art, 0, 0, u32::from(fit.upscale()));
            paint_door_signs(&mut canvas, &mut self.faces, &signs, sf);
            let stuck: Vec<AgentId> = frame
                .scene
                .agents
                .values()
                .filter(|s| crate::tasks::stuck_at_work(s, frame.views.get(&s.agent_id)).is_some())
                .map(|s| s.agent_id)
                .collect();
            paint_stuck(
                &mut canvas,
                &mut self.faces,
                &tags,
                &stuck,
                sf,
                fit.upscale(),
            );
            paint_tags(
                &mut canvas,
                &mut self.faces,
                &tags,
                sf,
                self.core.ui.selected,
            );
            inks = tags.iter().map(|t| (t.agent, t.marker_rgb)).collect();
            tag_hits = tags
                .iter()
                .flat_map(|t| {
                    std::iter::once(t.badge)
                        .chain(t.label)
                        .map(|r| (r, t.agent))
                })
                .collect();
            if let Some((text, at)) = &self.about
                && at.elapsed() < ABOUT_FOR
                && let Some(sign) = signs.iter().find(|s| &s.text == text)
            {
                paint_about(&mut canvas, &mut self.faces, sign, (ow, oh), sf);
            }
            sign_hits = signs.iter().map(|s| (s.area, s.text.clone())).collect();
        }

        // The cards and footer are laid out as cells, then set in the readable
        // faces.
        let panel_x = ow;
        let cols = u16::try_from(w.get().saturating_sub(panel_x) / g.cell_w).unwrap_or(0);
        let rows = u16::try_from(oh / g.row_h).unwrap_or(0);
        let panel_area = Rect::new(0, 0, cols, rows);
        let mut cells = Buffer::empty(panel_area);
        let panel_rows = panel::render(
            &mut cells,
            panel_area,
            &frame.scene,
            &frame.views,
            &inks,
            &mut self.core.ui,
        );
        canvas.fill(i64::from(panel_x), 0, w.get() - panel_x, h.get(), PANEL_BG);
        paint_cells(&mut canvas, &mut self.faces, &cells, (panel_x, 0), g);
        let foot_cols = u16::try_from(w.get() / g.cell_w).unwrap_or(0);
        let foot_area = Rect::new(0, 0, foot_cols, 1);
        let mut foot = Buffer::empty(foot_area);
        crate::core::footer(
            &mut foot,
            foot_area,
            &self.office,
            &self.core,
            &[
                ("点门牌", "看说明"),
                (
                    "t",
                    if self.on_top {
                        "取消置顶"
                    } else {
                        "置顶"
                    },
                ),
            ],
        );
        canvas.fill(0, i64::from(oh), w.get(), h.get() - oh, PANEL_BG);
        paint_cells(&mut canvas, &mut self.faces, &foot, (0, oh), g);

        self.last = Some(Layout {
            office_fit: fit,
            panel_x,
            grid: g,
            panel_rows,
            tag_hits,
            sign_hits,
        });
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("buffer: {e}"))?;
        buffer.copy_from_slice(&canvas.px);
        buffer
            .present()
            .map_err(|e| anyhow::anyhow!("present: {e}"))?;
        self.canvas = canvas;
        Ok(())
    }

    /// The office at the pack's density into [`Self::art`] with its signs and
    /// bubbles, ready to upscale, and the badges to set over it once it is;
    /// `None` when the floor showing could not lay out.
    fn render_office(
        &mut self,
        scene: &SceneState,
        fit: PixelFit,
        theme: &'static Theme,
        labels: &HashMap<AgentId, String>,
        scale_factor: f64,
    ) -> Option<(Vec<Tag>, Vec<DoorSign>)> {
        let _feed = self.office.session.render(
            fit.look(WorldText::Host),
            RenderInputs {
                world: FloorInputs {
                    scene,
                    now: SystemTime::now(),
                    floor: FloorMeta::ground()
                        .with_weather(WeatherPolicy::Clock)
                        .with_motion(Motion::Full),
                    pets: PetInputs::default(),
                },
                theme,
                size: fit.logical(),
                place: Place {
                    gateway: pixtuoid_scene::tally::office_gateway(scene),
                    floor: None,
                },
                debug_walkable: false,
                dim: Dim::NONE,
            },
            &self.office.pets,
            theme.surface.bg_fallback,
        );
        let picture = self.office.session.picture()?;
        let pixels = picture.pixels();
        let art = &mut self.art;
        art.reset(u32::from(pixels.width()), u32::from(pixels.height()), 0);
        for (dst, src) in art.px.iter_mut().zip(pixels.as_slice()) {
            *dst = to_u32(*src);
        }
        let mut tags = Vec::new();
        let mut signs = Vec::new();
        if let Picture::Floor(floor) = picture
            && let Some(text) = floor.text
        {
            let d = u32::from(fit.density().get());
            let u = u32::from(fit.upscale());
            signs = door_signs(&mut self.faces, text, d, u, scale_factor);
            let avoid: Vec<Area> = signs.iter().map(|s| s.area).collect();
            tags = place_tags(&mut self.faces, text, d, u, labels, scale_factor, &avoid);
            paint_world_text(art, text, d, u, &tags);
        }
        Some((tags, signs))
    }

    /// Scroll the panel's cards when the pointer is over the panel. Up the
    /// wheel, or two fingers down a trackpad, shows the cards above.
    fn wheel(&mut self, delta: MouseScrollDelta) {
        let Some(last) = &self.last else { return };
        if self.cursor.x < f64::from(last.panel_x) {
            return;
        }
        let rows = match delta {
            MouseScrollDelta::LineDelta(_, y) => -f64::from(y) * WHEEL_ROWS,
            MouseScrollDelta::PixelDelta(p) => {
                self.wheel_px -= p.y;
                let row_h = f64::from(last.grid.row_h.max(1));
                let rows = (self.wheel_px / row_h).trunc();
                self.wheel_px -= rows * row_h;
                rows
            }
        };
        if rows != 0.0 {
            self.core.ui.scroll_by(rows as isize);
        }
    }

    /// Select the card or the figure under the pointer; a double click opens
    /// its session, and one on the panel's title the sessions waiting. A sign
    /// toggles what its zone is for; any other click puts that away.
    fn click(&mut self) {
        let Some(last) = &self.last else { return };
        let (x, y) = (self.cursor.x.max(0.0) as u32, self.cursor.y.max(0.0) as u32);
        let shown = self.about.take();
        let point = Area {
            x: i64::from(x),
            y: i64::from(y),
            w: 1,
            h: 1,
        };
        if let Some((_, text)) = last
            .sign_hits
            .iter()
            .filter(|_| x < last.panel_x)
            .find(|(r, _)| r.meets(&point))
        {
            let again = shown.is_some_and(|(t, at)| &t == text && at.elapsed() < ABOUT_FOR);
            if !again {
                self.about = Some((text.clone(), Instant::now()));
            }
            return;
        }
        if x >= last.panel_x && y < last.grid.row_h {
            self.core.act(Action::OpenWaiting, &mut self.office);
            return;
        }
        // A badge or label spilling east is painted over by the panel, so it
        // takes no clicks there.
        let on_tag = last
            .tag_hits
            .iter()
            .rev()
            .filter(|_| x < last.panel_x)
            .find(|(r, _)| {
                r.meets(&Area {
                    x: i64::from(x),
                    y: i64::from(y),
                    w: 1,
                    h: 1,
                })
            });
        let id = if let Some((_, id)) = on_tag {
            Some(*id)
        } else if x >= last.panel_x {
            let row = u16::try_from(y / last.grid.row_h).unwrap_or(u16::MAX);
            last.panel_rows
                .iter()
                .find(|(r, _)| *r == row)
                .map(|(_, id)| *id)
        } else {
            let scale = u32::from(last.office_fit.scale().get());
            let at = Bounds {
                x: u16::try_from(x / scale).unwrap_or(u16::MAX),
                y: u16::try_from(y / scale).unwrap_or(u16::MAX),
                width: 1,
                height: 1,
            };
            let picture = self.office.session.picture();
            match picture.as_ref().and_then(|p| match p {
                Picture::Floor(f) => f.hits.at(at, None),
                Picture::Slide(_) => None,
            }) {
                Some(pixtuoid_scene::hit::SceneHit::Figure(
                    pixtuoid_scene::display::HoverTarget::Agent(id),
                )) => Some(*id),
                _ => None,
            }
        };
        let Some(id) = id else { return };
        let now = Instant::now();
        let double = self
            .last_click
            .is_some_and(|(prev, at)| prev == id && now - at < DOUBLE_CLICK);
        if double {
            self.last_click = None;
            self.core.ui.selected = Some(id);
            self.core.act(Action::Open, &mut self.office);
        } else {
            self.last_click = Some((id, now));
            self.core.select(id, &mut self.office);
        }
    }
}

fn save_png(c: &Canvas, path: &std::path::Path) -> Result<()> {
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, c.w, c.h);
    enc.set_color(png::ColorType::Rgb);
    let data: Vec<u8> =
        c.px.iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
            .collect();
    enc.write_header()?.write_image_data(&data)?;
    Ok(())
}

fn to_u32(c: Rgb) -> u32 {
    rgb(c.r, c.g, c.b)
}

fn color_u32(c: Color, fallback: u32) -> u32 {
    match c {
        Color::Rgb(r, g, b) => rgb(r, g, b),
        Color::White => 0x00ff_ffff,
        _ => fallback,
    }
}

/// Set a ratatui cell grid at `origin`, each cell `g.cell_w` wide and
/// `g.row_h` tall, in the readable faces.
fn paint_cells(
    canvas: &mut Canvas,
    faces: &mut Faces,
    cells: &Buffer,
    origin: (u32, u32),
    g: Grid,
) {
    let baseline = i64::from((g.row_h - g.px) / 2) + i64::from(faces.ascent(g.px));
    // Every background first: a wide character's glyph spans the next cell,
    // whose fill would otherwise cut it in half.
    // A wide character's background covers both its cells: ratatui leaves
    // the second unstyled.
    for y in 0..cells.area.height {
        for x in 0..cells.area.width {
            let cell = &cells[(x, y)];
            if let Color::Rgb(r, gr, b) = cell.bg {
                let px = i64::from(origin.0 + u32::from(x) * g.cell_w);
                let py = i64::from(origin.1 + u32::from(y) * g.row_h);
                let span = Faces::cells(cell.symbol()).max(1);
                canvas.fill(px, py, g.cell_w * span, g.row_h, rgb(r, gr, b));
            }
        }
    }
    for y in 0..cells.area.height {
        for x in 0..cells.area.width {
            let cell = &cells[(x, y)];
            let px = i64::from(origin.0 + u32::from(x) * g.cell_w);
            let py = i64::from(origin.1 + u32::from(y) * g.row_h);
            let sym = cell.symbol();
            if sym.trim().is_empty() {
                continue;
            }
            faces.draw(
                canvas,
                px,
                py + baseline,
                sym,
                color_u32(cell.fg, TEXT),
                g.px,
            );
        }
    }
}

/// A rectangle in the pixels of whichever canvas it is drawn on: the art
/// grid for what pixtuoid would bake, the window for the readable text.
#[derive(Clone, Copy)]
struct Area {
    x: i64,
    y: i64,
    w: u32,
    h: u32,
}

impl Area {
    fn meets(&self, o: &Area) -> bool {
        self.x < o.x + i64::from(o.w)
            && o.x < self.x + i64::from(self.w)
            && self.y < o.y + i64::from(o.h)
            && o.y < self.y + i64::from(self.h)
    }
}

/// A plate around `text` centred on art column `centre`, its top at `top`
/// (`plate_at` in `display/compose.rs`).
fn plate_at(centre: i64, top: i64, text: &str) -> Area {
    let w = font::width(text) + 2 * PLATE_PAD;
    Area {
        x: centre - i64::from(w / 2),
        y: top,
        w,
        h: PLATE_H,
    }
}

/// Where a run sits on the art grid at density `d` (`run_rect` in
/// `display/compose.rs`).
fn run_rect(run: &TextRun, d: u32) -> Area {
    let text: String = run.spans.iter().map(|s| s.text().to_owned()).collect();
    let art = |l: u16| i64::from(u32::from(l) * d);
    let cell = i64::from(u32::from(pixtuoid_scene::layout::CELL_ROWS) * d);
    match run.align {
        Align::Over => {
            let bottom = art(run.at.y.saturating_sub(LABEL_GAP));
            plate_at(art(run.at.x), bottom - i64::from(PLATE_H), &text)
        }
        Align::Centre => Area {
            h: cell.max(i64::from(LINE_H)) as u32,
            ..plate_at(art(run.at.x), art(run.at.y), &text)
        },
        Align::Left | Align::Right => {
            let x = art(run.at.x);
            let x = if run.align == Align::Right {
                x - i64::from(font::advance(&text))
            } else {
                x
            };
            Area {
                x,
                y: art(run.at.y) + (cell - i64::from(LINE_H)).max(0) / 2,
                w: font::width(&text).max(1),
                h: LINE_H,
            }
        }
    }
}

/// Paint `run` as the cutaway bakes one: plate, strip, then its spans.
fn paint_run(art: &mut Canvas, run: &TextRun, rect: Area) {
    if let Some(plate) = run.plate {
        art.fill(rect.x, rect.y, rect.w, rect.h, to_u32(plate));
    }
    if let Some(strip) = run.strip {
        art.fill(rect.x, rect.y, PLATE_PAD, rect.h, to_u32(strip));
    }
    let inset = match run.align {
        Align::Over | Align::Centre => i64::from(PLATE_PAD),
        Align::Left | Align::Right => 0,
    };
    let mut x = rect.x + inset;
    for span in &run.spans {
        let ink = to_u32(span.ink);
        match &span.content {
            Content::Text(t) => art.text(x, rect.y, t, ink, 1),
            Content::Icon(_) => art.icon(x, rect.y, span.text(), ink),
        }
        x += i64::from(font::advance(span.text()));
    }
}

fn badge_run(b: &Badge) -> TextRun {
    TextRun {
        at: b.at,
        align: Align::Over,
        spans: vec![b.name.clone()],
        plate: Some(b.plate),
        strip: Some(b.marker),
        role: TextRole::Badge(b.agent),
    }
}

/// A badge set in the readable faces over the upscaled office, and the task
/// over it, both placed in window pixels.
pub(crate) struct Tag {
    agent: AgentId,
    badge: Area,
    /// The window row its figure's top stands at, whichever tier its badge
    /// stepped up to.
    figure_top: i64,
    label: Option<Area>,
    plate: u32,
    marker: u32,
    /// `marker` as the panel's card dot takes it.
    marker_rgb: Rgb,
    name: String,
    ink: u32,
    task: Option<String>,
}

/// Rows over its badge a task label may climb to find room.
const LABEL_TIERS: i64 = 2;
/// Tiers a badge may step up off a neighbour's.
const BADGE_TIERS: usize = 3;

/// Points a badge's name and a task label are set in.
const NAME_PT: f64 = 10.0;
const TASK_PT: f64 = 11.5;

fn pt_px(pt: f64, scale_factor: f64) -> u32 {
    (pt * scale_factor).round().max(8.0) as u32
}

fn tag_pad(scale_factor: f64) -> u32 {
    (2.0 * scale_factor).round() as u32
}

/// Where each badge goes, at `u` window pixels an art pixel: on the foot of
/// the plate pixtuoid would bake, centred on it; then each task on a plate of
/// its own over its badge, climbing a tier when a neighbour holds the row,
/// left out when no tier is free or it would leave the window's top. The
/// names and tasks are set here rather than baked because 8 art pixels are
/// too small to read.
fn place_tags(
    faces: &mut Faces,
    text: HostText<'_>,
    d: u32,
    u: u32,
    labels: &HashMap<AgentId, String>,
    scale_factor: f64,
    avoid: &[Area],
) -> Vec<Tag> {
    let (name_px, task_px) = (pt_px(NAME_PT, scale_factor), pt_px(TASK_PT, scale_factor));
    let pad = tag_pad(scale_factor);
    let mut tags: Vec<Tag> = text
        .badges
        .iter()
        .map(|b| {
            let at = run_rect(&badge_run(b), d);
            let name = b.name.text().to_owned();
            let w = Faces::cells(&name) * faces.cell_w(name_px) + 3 * pad;
            let h = name_px + 2 * pad;
            let cx = (at.x + i64::from(at.w / 2)) * i64::from(u);
            let foot = (at.y + i64::from(at.h)) * i64::from(u);
            Tag {
                agent: b.agent,
                // Not off the window's west edge, by the rooftop.
                badge: Area {
                    x: (cx - i64::from(w / 2)).max(0),
                    y: foot - i64::from(h),
                    w,
                    h,
                },
                figure_top: foot + i64::from(LABEL_GAP) * i64::from(u),
                label: None,
                plate: to_u32(b.plate),
                marker: to_u32(b.marker),
                marker_rgb: b.marker,
                name,
                ink: to_u32(b.name.ink),
                task: labels.get(&b.agent).map(|t| truncate_cells(t, LABEL_CELLS)),
            }
        })
        .collect();
    // A badge that would cover a neighbour's (sitters side by side on a
    // sofa) steps up a tier over its own figure, so every name reads.
    let mut taken: Vec<Area> = Vec::with_capacity(tags.len());
    for t in &mut tags {
        let step = i64::from(t.badge.h + pad);
        for _ in 0..BADGE_TIERS {
            if !taken.iter().any(|o| o.meets(&t.badge)) || t.badge.y - step < 0 {
                break;
            }
            t.badge.y -= step;
        }
        taken.push(t.badge);
    }
    // A task label keeps off the door signs too.
    taken.extend(avoid.iter().copied());
    for t in &mut tags {
        let Some(task) = &t.task else { continue };
        let w = Faces::cells(task) * faces.cell_w(task_px) + 2 * pad;
        let h = task_px + 2 * pad;
        let step = i64::from(h + pad);
        let b = t.badge;
        t.label = (1..=LABEL_TIERS)
            .map(|tier| Area {
                x: (b.x + i64::from(b.w / 2) - i64::from(w / 2)).max(0),
                y: b.y - step * tier,
                w,
                h,
            })
            .find(|r| r.y >= 0 && !taken.iter().any(|o| o.meets(r)));
        taken.extend(t.label);
    }
    tags
}

/// The signs and chitchat bubbles pixtuoid would bake, on the art grid in
/// its order. A bubble is skipped over an agent that has a task to show, and
/// where it would sit under a readable badge or label (`tags`, in window
/// pixels, `u` to an art pixel).
fn paint_world_text(art: &mut Canvas, text: HostText<'_>, d: u32, u: u32, tags: &[Tag]) {
    let u = i64::from(u.max(1));
    let to_art = |r: &Area| Area {
        x: r.x / u,
        y: r.y / u,
        w: (r.w as i64 / u + 1) as u32,
        h: (r.h as i64 / u + 1) as u32,
    };
    let readable: Vec<Area> = tags
        .iter()
        .flat_map(|t| std::iter::once(t.badge).chain(t.label))
        .map(|r| to_art(&r))
        .collect();
    for run in text.bubbles {
        if let TextRole::Bubble(agent) = run.role
            && tags.iter().any(|t| t.agent == agent && t.task.is_some())
        {
            continue;
        }
        let rect = run_rect(run, d);
        if !readable.iter().any(|t| t.meets(&rect)) {
            paint_run(art, run, rect);
        }
    }
    let mut signs: Vec<Area> = Vec::new();
    // Door signs are set in the readable faces over the upscaled office
    // ([`door_signs`]): the art font has no Chinese.
    for run in text.signs.iter().filter(|r| !set_readable(r.role)) {
        let rect = run_rect(run, d);
        if signs.iter().any(|s| s.meets(&rect)) {
            continue;
        }
        signs.push(rect);
        paint_run(art, run, rect);
    }
}

/// Whether the window sets a sign of `role` in the readable faces rather
/// than the art font: the zones' door signs, in Chinese, and the benches'
/// repository plates, whose names may be.
fn set_readable(role: TextRole) -> bool {
    matches!(role, TextRole::DoorSign | TextRole::BenchPlate)
}

/// A door sign or a bench's plate as the window sets it, on its colour,
/// centred where pixtuoid hangs it.
pub(crate) struct DoorSign {
    /// Its plate, in window pixels.
    area: Area,
    text: String,
    /// What its zone is for, shown when it is clicked.
    about: &'static str,
    plate: u32,
    ink: u32,
}

/// The Chinese for a zone pixtuoid signs in ASCII, the art font having no
/// Chinese.
fn door_sign_zh(text: &str) -> Option<&'static str> {
    Some(match text {
        "MTG A" => "会议室 A",
        "MTG B" => "会议室 B",
        "PANTRY" => "茶水间",
        "WAIT" => "等候区",
        "OFFICE" => "你的办公室",
        "ROOF" => "天台",
        _ => return None,
    })
}

/// What a zone pixtuoid signs in ASCII is for: who is in there, and what
/// moves them on, so the office reads without the README.
fn zone_about(text: &str) -> Option<&'static str> {
    Some(match text {
        "MTG A" | "MTG B" => "子 agent 跑满 15 秒的会话，和它的子 agent 一起在这儿开会。",
        "PANTRY" => {
            "刚做完、你还没看的会话在这儿等你看结果。双击打开看过，或者做完满 15 分钟，就上天台。"
        }
        "WAIT" => {
            "等你批准或回复超过 20 秒的会话，按先后在这儿排队，排第一个的坐你办公室的访客椅。你处理完它就回工位。"
        }
        "OFFICE" => "访客椅上坐的是等你最久的那个会话，先处理它。",
        "ROOF" => {
            "闲着、不用你管的会话：做完超过 15 分钟，或者你已经打开看过。你给它发新消息它就回工位。"
        }
        _ => return None,
    })
}

/// What a bench is for, its plate naming the repo.
const DESK_ABOUT: &str =
    "这张长桌坐这个仓库的会话，干活的坐在工位上。红圈加红「?」的是卡住了，去看看。";

/// How long a sign's explanation stays up unless clicked away.
const ABOUT_FOR: Duration = Duration::from_secs(10);
/// Cells an explanation's line may take.
const ABOUT_CELLS: u32 = 26;
const ABOUT_BG: u32 = 0x0024_2833;
const ABOUT_EDGE: u32 = 0x005a_6070;

/// `s` broken into lines of at most `max` cells, at any character: the
/// explanations are Chinese, which breaks anywhere.
fn wrap_cells(s: &str, max: u32) -> Vec<String> {
    let max = max.max(2);
    let mut lines = vec![String::new()];
    let mut used = 0;
    for c in s.chars() {
        let w = Faces::cells(&c.to_string());
        if used + w > max {
            lines.push(String::new());
            used = 0;
        }
        used += w;
        lines.last_mut().expect("never empty").push(c);
    }
    lines
}

/// `sign`'s explanation on a plate under it, or over it when there is no room
/// below, kept inside the office of `bounds` window pixels.
fn paint_about(
    canvas: &mut Canvas,
    faces: &mut Faces,
    sign: &DoorSign,
    bounds: (u32, u32),
    scale_factor: f64,
) {
    let px = pt_px(SIGN_PT, scale_factor);
    let pad = 2 * tag_pad(scale_factor);
    let cell = faces.cell_w(px).max(1);
    let fit = bounds.0.saturating_sub(2 * pad + 2) / cell;
    let lines = wrap_cells(sign.about, ABOUT_CELLS.min(fit));
    let line_h = px + px / 3;
    let widest = lines.iter().map(|l| Faces::cells(l)).max().unwrap_or(0);
    let w = widest * cell + 2 * pad;
    let h = u32::try_from(lines.len()).unwrap_or(0) * line_h + 2 * pad;
    let (bw, bh) = (i64::from(bounds.0), i64::from(bounds.1));
    let a = sign.area;
    let x = (a.x + i64::from(a.w / 2) - i64::from(w / 2)).clamp(0, (bw - i64::from(w)).max(0));
    let gap = i64::from(pad);
    let below = a.y + i64::from(a.h) + gap;
    let y = if below + i64::from(h) <= bh {
        below
    } else {
        (a.y - gap - i64::from(h)).max(0)
    };
    canvas.fill(x - 1, y - 1, w + 2, h + 2, ABOUT_EDGE);
    canvas.fill(x, y, w, h, ABOUT_BG);
    let ascent = i64::from(faces.ascent(px));
    for (i, line) in lines.iter().enumerate() {
        let top = y + i64::from(pad) + i64::try_from(i).unwrap_or(0) * i64::from(line_h);
        faces.draw(canvas, x + i64::from(pad), top + ascent, line, TEXT, px);
    }
}

/// The door signs among `text`'s, at `d` art pixels a layout unit and `u`
/// window pixels an art pixel.
fn door_signs(
    faces: &mut Faces,
    text: HostText<'_>,
    d: u32,
    u: u32,
    scale_factor: f64,
) -> Vec<DoorSign> {
    let cell = i64::from(u32::from(pixtuoid_scene::layout::CELL_ROWS) * d);
    let px = pt_px(SIGN_PT, scale_factor);
    let pad = tag_pad(scale_factor);
    text.signs
        .iter()
        .filter(|r| set_readable(r.role))
        .filter_map(|r| {
            let text: String = r.spans.iter().map(|s| s.text().to_owned()).collect();
            let zh = match r.role {
                TextRole::DoorSign => door_sign_zh(&text)?.to_owned(),
                _ => truncate_cells(&text, PLATE_CELLS),
            };
            let cx = i64::from(u32::from(r.at.x) * d) * i64::from(u);
            let cy = (i64::from(u32::from(r.at.y) * d) + cell / 2) * i64::from(u);
            let w = Faces::cells(&zh) * faces.cell_w(px) + 2 * pad;
            let h = px + 2 * pad;
            Some(DoorSign {
                area: Area {
                    x: cx - i64::from(w / 2),
                    y: cy - i64::from(h / 2),
                    w,
                    h,
                },
                about: match r.role {
                    TextRole::DoorSign => zone_about(&text)?,
                    _ => DESK_ABOUT,
                },
                text: zh,
                plate: r.plate.map_or(PANEL_BG, to_u32),
                ink: r.spans.first().map_or(TEXT, |s| to_u32(s.ink)),
            })
        })
        .collect()
}

/// Set each door sign on its plate in the readable faces.
fn paint_door_signs(canvas: &mut Canvas, faces: &mut Faces, signs: &[DoorSign], scale_factor: f64) {
    let px = pt_px(SIGN_PT, scale_factor);
    let pad = tag_pad(scale_factor);
    for s in signs {
        let Area { x, y, w, h } = s.area;
        canvas.fill(x, y, w, h, s.plate);
        let base = y + i64::from(pad) + i64::from(faces.ascent(px));
        faces.draw(canvas, x + i64::from(pad), base, &s.text, s.ink, px);
    }
}

/// Points a door sign is set in.
const SIGN_PT: f64 = 9.0;
/// Cells a bench's plate may take.
const PLATE_CELLS: u16 = 16;

/// Red for a stuck agent, the panel's stuck colour.
const STUCK_RED: u32 = 0x00f7_768e;

/// Mark each stuck agent in `stuck` where it sits: a red ring round its
/// figure and a red "?" by its badge, so it stands out from the agents
/// working normally around it. `u` is window pixels an art pixel.
fn paint_stuck(
    canvas: &mut Canvas,
    faces: &mut Faces,
    tags: &[Tag],
    stuck: &[AgentId],
    scale_factor: f64,
    u: u16,
) {
    let u = i64::from(u.max(1));
    let name_px = pt_px(NAME_PT, scale_factor);
    let pad = tag_pad(scale_factor);
    for t in tags.iter().filter(|t| stuck.contains(&t.agent)) {
        let b = t.badge;
        let cx = b.x + i64::from(b.w / 2);
        let top = t.figure_top;
        let half_h = i64::from(pixtuoid_scene::layout::CHARACTER_SPRITE_H) * u / 2;
        let half_w = i64::from(pixtuoid_scene::layout::CHARACTER_SPRITE_W) * u * 3 / 4;
        let dot = (u / 2).max(1) as u32;
        for (x, y) in ring(cx, top + half_h, half_w, half_h + u) {
            canvas.fill(x, y, dot, dot, STUCK_RED);
        }
        let w = faces.cell_w(name_px) + 2 * pad;
        let (x, y) = (b.x + i64::from(b.w + pad), b.y);
        canvas.fill(x, y, w, b.h, STUCK_RED);
        let base = y + i64::from(pad) + i64::from(faces.ascent(name_px));
        faces.draw(canvas, x + i64::from(pad), base, "?", TEXT, name_px);
    }
}

/// The outline of an ellipse centred on (`cx`, `cy`) with radii `rx`, `ry`,
/// one point per window pixel of its rim.
fn ring(cx: i64, cy: i64, rx: i64, ry: i64) -> Vec<(i64, i64)> {
    let steps = (4 * (rx + ry)).max(8);
    (0..steps)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / steps as f64;
            (
                cx + (rx as f64 * a.cos()).round() as i64,
                cy + (ry as f64 * a.sin()).round() as i64,
            )
        })
        .collect()
}

/// Set the placed badges and task labels in the readable faces.
fn paint_tags(
    canvas: &mut Canvas,
    faces: &mut Faces,
    tags: &[Tag],
    scale_factor: f64,
    selected: Option<AgentId>,
) {
    let (name_px, task_px) = (pt_px(NAME_PT, scale_factor), pt_px(TASK_PT, scale_factor));
    let pad = tag_pad(scale_factor);
    for t in tags {
        let r = t.badge;
        canvas.fill(r.x, r.y, r.w, r.h, t.plate);
        canvas.fill(r.x, r.y, pad, r.h, t.marker);
        let ink = if selected == Some(t.agent) {
            0x00ff_ffff
        } else {
            t.ink
        };
        let base = r.y + i64::from(pad) + i64::from(faces.ascent(name_px));
        faces.draw(
            canvas,
            r.x + i64::from(2 * pad),
            base,
            &t.name,
            ink,
            name_px,
        );
    }
    for t in tags {
        let (Some(r), Some(task)) = (t.label, &t.task) else {
            continue;
        };
        canvas.fill(r.x, r.y, r.w, r.h, t.plate);
        let base = r.y + i64::from(pad) + i64::from(faces.ascent(task_px));
        faces.draw(canvas, r.x + i64::from(pad), base, task, TASK, task_px);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drag across many sizes keeps the office it started with, and its
    /// desk count with it: each size is its own layout, so taking each one
    /// as it came reshuffled the rooms and moved agents between floors
    /// every few pixels. Once the size settles, the office lays out for it.
    #[test]
    fn a_resize_holds_the_office_until_the_window_settles() {
        let pack = pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack");
        let fit = |w, h| fit_for(&pack, w, h, None);
        let mut hold = ResizeHold::default();
        let t0 = Instant::now();
        let before = fit(1500, 1100);
        assert_eq!(
            hold.fit(before, t0),
            before,
            "the first frame takes its own"
        );

        let step = Duration::from_millis(16);
        let mut now = t0;
        for (w, h) in [(1520, 1080), (1600, 1000), (1800, 900), (1900, 1400)] {
            now += step;
            hold.resized(now);
            assert_ne!(fit(w, h).logical(), before.logical(), "{w}x{h}");
            assert_eq!(hold.fit(fit(w, h), now), before, "mid-drag at {w}x{h}");
        }

        let settled = fit(1900, 1400);
        let late = now + RESIZE_SETTLE / 2;
        assert_eq!(hold.fit(settled, late), before, "not yet settled");
        let done = now + RESIZE_SETTLE;
        assert_eq!(hold.fit(settled, done), settled, "settled");
        assert_eq!(
            hold.fit(fit(1200, 900), done + step),
            fit(1200, 900),
            "no resize since, so a new size is taken at once"
        );
    }

    /// A height near a scale step keeps the office shown, whichever way it
    /// came: the step halves or doubles the office, so a window resting by
    /// it would flip between two layouts on an edge's nudge. Well past the
    /// band the scale steps, so the art neither shrinks to specks nor grows
    /// to fill the window with a desk.
    #[test]
    fn a_height_near_a_scale_step_keeps_the_scale_shown() {
        let pack = pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack");
        let scale = |f: PixelFit| f.scale().get();
        let (small, big) = (
            fit_for(&pack, 1300, 900, None),
            fit_for(&pack, 1300, 1300, None),
        );
        assert!(
            scale(small) < scale(big),
            "two scales either side of a step"
        );

        let step = (900..1300)
            .find(|&h| scale(fit_for(&pack, 1300, h, None)) != scale(small))
            .expect("a step between them");
        for h in [step - 40, step, step + 40] {
            let up = fit_for(&pack, 1300, h, Some(small));
            let down = fit_for(&pack, 1300, h, Some(big));
            assert_eq!(scale(up), scale(small), "growing to {h}");
            assert_eq!(scale(down), scale(big), "shrinking to {h}");
            assert_eq!(up.logical().h, u16::try_from(h).unwrap() / scale(small));
        }

        assert_eq!(
            scale(fit_for(&pack, 1300, 2400, Some(small))),
            scale(fit_for(&pack, 1300, 2400, None))
        );
        assert_eq!(
            scale(fit_for(&pack, 1300, 700, Some(big))),
            scale(fit_for(&pack, 1300, 700, None))
        );
    }

    /// Every zone pixtuoid signs reads in Chinese in the window: a sign the
    /// map missed would vanish, not fall back to ASCII.
    #[test]
    fn every_door_sign_has_its_chinese() {
        for (ascii, zh) in [
            ("MTG A", "会议室 A"),
            ("MTG B", "会议室 B"),
            ("PANTRY", "茶水间"),
            ("WAIT", "等候区"),
            ("OFFICE", "你的办公室"),
            ("ROOF", "天台"),
        ] {
            assert_eq!(door_sign_zh(ascii), Some(zh));
        }
        assert_eq!(door_sign_zh("LOBBY"), None);
    }

    /// Clicking any zone's sign must say what the zone is for: a sign the
    /// map missed would click to nothing, and the office would read only
    /// with the README beside it.
    #[test]
    fn every_door_sign_says_what_its_zone_is_for() {
        for ascii in ["MTG A", "MTG B", "PANTRY", "WAIT", "OFFICE", "ROOF"] {
            assert!(zone_about(ascii).is_some_and(|s| !s.is_empty()), "{ascii}");
        }
        assert_eq!(zone_about("LOBBY"), None);
    }

    /// An explanation is set on a plate of bounded width: a line wider than
    /// asked would run off the office or under the panel, and losing or
    /// reordering a character would change what it says.
    #[test]
    fn an_explanation_wraps_within_its_width_and_keeps_every_character() {
        let s = zone_about("WAIT").expect("signed");
        for max in [3, 10, 26] {
            let lines = wrap_cells(s, max);
            assert!(
                lines.iter().all(|l| Faces::cells(l) <= max),
                "{max}: {lines:?}"
            );
            assert_eq!(lines.concat(), s, "{max}");
        }
        assert_eq!(wrap_cells("短", 26), vec!["短".to_owned()]);
    }
}
