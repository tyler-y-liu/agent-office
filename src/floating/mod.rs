//! agent-office's window: an always-on-top desktop window drawing the office
//! as pixtuoid's floating window does, the cutaway at the pack's full density
//! upscaled to real pixels, with the task panel beside it in the same pixel
//! font. Drawn on the CPU (softbuffer): one frame is a copy and an upscale.

mod font;
mod lively;
mod rooms;
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
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};

use crate::core::{Action, Core, Floors};
use crate::roles::Role;
use crate::panel;
use crate::text::truncate_cells;
use font::{Canvas, LINE_H, rgb};
use vector::Faces;

const FRAME: Duration = Duration::from_millis(50);
/// The office holds one frame per beat while nothing moves between beats
/// (`OfficeSession::moves_off_beat`), so a calm window repaints on the beat.
const BEAT_MS: u64 = pixtuoid_scene::anim::FULL_TICK_MS;
/// How often the waits are checked for the Dock badge and banners: a
/// second late is still in time, and a covered window wakes no more often.
const WATCH_EVERY: Duration = Duration::from_secs(1);
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
    // The footer says who waits on you, readable; the wall board's art font
    // is too small for its chatty lines.
    pixtuoid_scene::neon_sign::set_personas_shown(false);
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
    if let Some(win) = app.win.as_mut() {
        win.core.watch.flush();
    }
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
    /// The agent last clicked, when, and whether its card was expanded
    /// before: a second click on it soon after opens its session and undoes
    /// what the first did to the card.
    last_click: Option<(AgentId, Instant, bool)>,
    /// An idle worker clicked once, whose answer waits out the double-click
    /// time: a double click opens it and is not also a poke.
    pending_poke: Option<(AgentId, Instant)>,
    /// The pointer is showing a hand, over something a click acts on.
    pointer: bool,
    /// The pointer is over the window: past its edge it hovers nothing.
    cursor_in: bool,
    /// The sign clicked for what its zone is for, by its text, and when.
    about: Option<(String, Instant)>,
    /// The role picker `p` opens, with what a pick dispatches: the next
    /// key picks or closes it.
    picker: Option<crate::core::DispatchPlan>,
    /// The modifier keys held now, so Cmd+W is not taken for `w`.
    mods: ModifiersState,
    /// When the office opened: subagents already running then were handed
    /// their work before it, and get no ticket.
    opened: SystemTime,
    /// What a click last got out of the office, and when.
    reaction: Option<(Reaction, Instant)>,
    /// How many times each idle worker has been clicked, so it answers
    /// something new each time.
    pokes: HashMap<AgentId, u64>,
    /// When the evening board's replay of the day was started.
    replay: Option<Instant>,
    /// The sounds, when they are on.
    sound: Option<(crate::fun::sound::Ear, crate::fun::sound::Speaker)>,
    next_frame: Instant,
    /// Whether the last frame had something moving between beats (a walk,
    /// a reaction, confetti, the hopping "!"), which wants [`FRAME`].
    quick: bool,
    occluded: bool,
    focused: bool,
    /// When the waits are next checked for the Dock and banners.
    next_watch: Instant,
    /// The frame, and the office at art density before its upscale: kept
    /// between frames so a frame allocates nothing.
    canvas: Canvas,
    /// Each card's head and when it was drawn: recolouring a figure for its
    /// head is a whole character frame's work, and a head rarely changes.
    portraits: HashMap<AgentId, (pixtuoid_core::sprite::Frame, Instant)>,
    /// Keep the presented frame in `canvas` (copied to the window rather than
    /// swapped in), for `--shot` to save it.
    keep_frame: bool,
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
    /// The evening board's tally, which a click replays the day on.
    board_hits: Vec<Area>,
    /// The cat sitting with a stuck worker, where it is drawn.
    cat_hit: Option<Area>,
    /// Window pixels to a logical unit, which the scene's hits are in.
    unit: i64,
}

/// What a click got out of the office.
#[derive(Clone, Copy)]
enum Reaction {
    /// An idle worker, poked.
    Worker(AgentId, crate::fun::Poke),
    /// The shop cat, at this window point.
    Cat(i64, i64),
}

/// How long a card's drawn head is reused before it is drawn again.
const PORTRAIT_FOR: Duration = Duration::from_secs(2);
/// How long a click's reaction stays up.
const REACTION_FOR: Duration = Duration::from_millis(2_200);

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

/// How far, as a share of the height, a window may sit past a scale step
/// and keep the scale shown. The scale steps by the art's density (4, 8,
/// 12...), so a step halves or doubles the office: without this band a
/// window an edge away from a step flipped between two offices. Kept to the
/// step's own neighbourhood, since a scale held further on draws the art
/// at half its size in an office of empty rooms.
const KEEP_SCALE_SHARE: f64 = 0.1;

/// The fit of a `w`×`h` office, `shown` the one on screen: its scale while
/// the height is within [`KEEP_SCALE_SHARE`] of one that takes it, else this
/// size's own.
fn fit_for(pack: &OfficeArt, w: u32, h: u32, shown: Option<PixelFit>) -> PixelFit {
    let px = Size {
        w: u16::try_from(w).unwrap_or(u16::MAX),
        h: u16::try_from(h).unwrap_or(u16::MAX),
    };
    let density = pack.max_density_variant();
    let own = |h: f64| PixelFit::at_least_density(office_scale(h as u32), density, px);
    let min = pixtuoid_scene::layout::min_layout_size();
    let near =
        [1.0 - KEEP_SCALE_SHARE, 1.0 + KEEP_SCALE_SHARE].map(|k| own(f64::from(h) * k).scale());
    let held = shown.map(|f| f.over(px)).filter(|f| {
        let l = f.logical();
        l.w >= min.w && l.h >= min.h && near.contains(&f.scale())
    });
    held.unwrap_or_else(|| own(f64::from(h)))
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
            WindowEvent::Focused(focused) => {
                win.focused = focused;
                if focused {
                    win.core.focus_gained(&mut win.office);
                }
            }
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
                // A new display scales the window's pixels by its ratio to the
                // old, a whole step or more: the scale shown there would draw
                // the art at half (or twice) its size here.
                win.hold.shown = None;
            }
            WindowEvent::CursorMoved { position, .. } => {
                win.cursor = position;
                win.cursor_in = true;
                win.hover();
            }
            WindowEvent::CursorLeft { .. } => {
                win.cursor_in = false;
                win.core.ui.hovered = None;
                win.next_frame = Instant::now();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // A click is not a pick: the picker goes, so a digit typed
                // later does not dispatch into whatever got selected.
                if win.picker.take().is_some() {
                    win.core.watch.tally("picker.cancel");
                }
                win.click();
                win.next_frame = Instant::now();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                win.wheel(delta);
                win.next_frame = Instant::now();
            }
            WindowEvent::ModifiersChanged(m) => win.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if !takes_key(event.logical_key.as_ref(), event.repeat, win.mods) {
                    return;
                }
                // Open, the picker takes the next key: a digit picks, Esc
                // and `p` only close it, and any other key closes it and
                // still does its own thing, so an `o` meant for the
                // selection is not lost.
                if let Some(plan) = win.picker.take() {
                    win.next_frame = Instant::now();
                    match event.logical_key.as_ref() {
                        Key::Character(c) if picked_role(c).is_some() => {
                            if let Some(role) = picked_role(c) {
                                win.core.dispatch(&plan, role);
                            }
                            return;
                        }
                        Key::Named(NamedKey::Escape) | Key::Character("p") => {
                            win.core.watch.tally("picker.cancel");
                            return;
                        }
                        _ => win.core.watch.tally("picker.cancel"),
                    }
                }
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
                    Key::Character("n") => Action::OpenNext,
                    Key::Character("l") => Action::SwitchLang,
                    Key::Character("p") => {
                        win.picker = win.core.plan_dispatch();
                        win.next_frame = Instant::now();
                        return;
                    }
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
                if action == Action::SwitchLang {
                    win.relabel();
                }
                win.next_frame = Instant::now();
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
        // A shot is for looking at the office, not for posting banners.
        if now >= win.next_watch && self.shot.is_none() {
            win.next_watch = now + WATCH_EVERY;
            win.core.watch_waits(win.focused);
        }
        // A hidden widget left running all day should cost nothing.
        if now >= win.next_frame && (!win.occluded || self.shot.is_some()) {
            win.next_frame = now
                + if win.quick {
                    FRAME
                } else {
                    to_next_beat(SystemTime::now())
                };
            win.window.request_redraw();
        }
        // Covered, the frame clock stands still, so the waits' clock wakes it.
        let wake = if win.occluded && self.shot.is_none() {
            win.next_watch
        } else {
            win.next_frame.min(win.next_watch)
        };
        el.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

/// Give the window the color space softbuffer tags its frames with
/// (device RGB). Otherwise Core Animation converts every presented frame to
/// the display's space on the main thread, which was most of a frame's cost;
/// with the two matching, the window server does it on the GPU instead.
#[cfg(target_os = "macos")]
fn match_frame_color_space() {
    use objc2_app_kit::{NSApplication, NSColorSpace};
    let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
        return;
    };
    // SAFETY: AppKit on the main thread, which `mtm` proves.
    unsafe {
        let space = NSColorSpace::deviceRGBColorSpace();
        for w in NSApplication::sharedApplication(mtm).windows() {
            w.setColorSpace(Some(&space));
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn match_frame_color_space() {}

impl Floating<'_> {
    fn open(&self, el: &ActiveEventLoop) -> Result<Win> {
        let attrs = Window::default_attributes()
            .with_title("agent-office")
            .with_window_level(WindowLevel::Normal)
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
        match_frame_color_space();
        // Seat the floors for the office the first frame will draw, so an
        // agent arriving before it lands on a desk that exists.
        let size = window.inner_size();
        let (ow, oh) = office_px(size.width, size.height, g);
        let logical = fit_for(&self.pack, ow, oh, None).logical();
        Ok(Win {
            window,
            surface,
            core: Core::start(self.rt, logical.w, logical.h),
            office: Office {
                session: OfficeSession::new(Arc::clone(&self.pack)),
                caps_size: None,
                pets: vec![Pet {
                    kind: PetKind::Cat,
                    name: cat_name().to_string(),
                }],
            },
            on_top: false,
            cursor: PhysicalPosition::default(),
            wheel_px: 0.0,
            last: None,
            last_click: None,
            pending_poke: None,
            pointer: false,
            cursor_in: false,
            about: None,
            picker: None,
            mods: ModifiersState::empty(),
            opened: SystemTime::now(),
            reaction: None,
            pokes: HashMap::new(),
            replay: None,
            sound: crate::fun::sound::enabled().then(Default::default),
            next_frame: Instant::now(),
            quick: true,
            occluded: false,
            focused: true,
            next_watch: Instant::now(),
            canvas: Canvas::new(0, 0, 0),
            keep_frame: self.shot.is_some(),
            portraits: HashMap::new(),
            art: Canvas::new(0, 0, 0),
            faces,
            hold: ResizeHold::default(),
        })
    }
}

fn cat_name() -> &'static str {
    crate::i18n::t("店长", "Manager")
}

impl Win {
    /// After a language switch: words kept from before it are put in the
    /// new language, or dropped where they come back by themselves.
    fn relabel(&mut self) {
        for p in &mut self.office.pets {
            if p.kind == PetKind::Cat {
                p.name = cat_name().to_string();
            }
        }
        self.about = None;
    }

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
        // Anything below that moves between beats sets this again.
        self.quick = false;
        self.land_poke();

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
        let mut board_hits = Vec::new();
        let mut cat_hit = None;
        let mut inks = HashMap::new();
        if let Some((ear, speaker)) = &mut self.sound {
            for chime in ear.hear(&frame.scene, &frame.views) {
                speaker.play(chime);
            }
        }
        let stuck: Vec<AgentId> = frame
            .scene
            .agents
            .values()
            .filter(|s| crate::tasks::stuck_at_work(s, frame.views.get(&s.agent_id)).is_some())
            .map(|s| s.agent_id)
            .collect();
        if fit.logical().w < min.w || fit.logical().h < min.h {
            let base = i64::from(g.row_h) + i64::from(self.faces.ascent(g.px));
            self.faces.draw(
                &mut canvas,
                16,
                base,
                crate::i18n::t(
                    "窗口太小，放大后显示办公室",
                    "Window too small: enlarge it to see the office",
                ),
                TEXT,
                g.px,
            );
        } else if let Some((tags, signs, speakers, rooms, boards)) = {
            let now = SystemTime::now();
            let board = frame
                .clock
                .evening()
                .then(|| lively::tally_lines(&self.core.watch.stats));
            // A typhoon keeps the boats in port.
            pixtuoid_scene::sky::set_harbour_closed(frame.outlook.typhoon());
            pixtuoid_scene::desk_show::set_desk_shows(
                frame
                    .office
                    .agents
                    .values()
                    .filter_map(|s| {
                        let show = crate::fun::desk_show(s, frame.views.get(&s.agent_id), now)?;
                        Some((s.agent_id, show))
                    })
                    .collect(),
            );
            let weather = frame
                .outlook
                .weather
                .map_or(WeatherPolicy::Clock, WeatherPolicy::Forced);
            self.render_office(
                &frame.office,
                fit,
                theme,
                &head_labels(&frame, &self.core.ui.seen, self.core.head_labels),
                &shown_in_full(&frame, self.core.ui.selected),
                sf,
                OfficeExtras {
                    weather,
                    roles: frame
                        .views
                        .iter()
                        .filter_map(|(id, v)| Some((*id, v.role?)))
                        .collect(),
                    board,
                    stuck: stuck.clone(),
                    now,
                },
            )
        } {
            // A floor slide moves every frame too, and the beat signal does
            // not count it.
            self.quick |= self.office.session.moves_off_beat()
                || self.office.session.nav().transition().is_some();
            canvas.blit_scaled(&self.art, 0, 0, u32::from(fit.upscale()));
            let now = SystemTime::now();
            let stage = lively::Stage {
                u: i64::from(fit.upscale().max(1)),
                unit: fit.scale().get(),
                scale_factor: sf,
                w: ow,
                h: oh,
                floor_top: i64::from(pixtuoid_scene::layout::north_wall_h(fit.logical().h))
                    * i64::from(fit.scale().get()),
                ms: lively::ms_of(now),
            };
            cat_hit = self.paint_goings_on_under(&mut canvas, &frame, &tags, &signs, stage, now);
            self.paint_rooms(&mut canvas, &frame, &tags, &signs, &rooms, stage);
            paint_door_signs(&mut canvas, &mut self.faces, &signs, sf);
            let tags = in_group_inks(tags, &frame, &self.core.ui.seen, now);
            paint_stuck(
                &mut canvas,
                &mut self.faces,
                &tags,
                &stuck,
                sf,
                (fit.upscale(), fit.scale().get()),
            );
            if let Some(first) = frame.first_waiting {
                // Its halo breathes smoother than the beat steps.
                self.quick |= tags.iter().any(|t| t.agent == first);
                paint_first_waiting(&mut canvas, &tags, first, stage, fit.scale().get());
            }
            if let Some(selected) = self.core.ui.selected {
                paint_selected(&mut canvas, &tags, selected, fit.scale().get(), sf);
            }
            paint_tags(
                &mut canvas,
                &mut self.faces,
                &tags,
                sf,
                self.core.ui.selected,
            );
            self.paint_goings_on_over(
                &mut canvas,
                &frame,
                &tags,
                &signs,
                &speakers,
                &rooms,
                stage,
                now,
            );
            self.paint_reaction(&mut canvas, &tags, stage);
            self.quick |= self.reaction.is_some()
                || self.pending_poke.is_some()
                || self.replay.is_some()
                || tags
                    .iter()
                    .any(|t| crate::fun::live_cheer(frame.views.get(&t.agent), now).is_some());
            let u = i64::from(fit.upscale());
            if frame.clock.evening() {
                paint_board_words(&mut canvas, &mut self.faces, &boards, u);
                board_hits = boards
                    .iter()
                    .map(|(a, _)| Area {
                        x: a.x * u,
                        y: a.y * u,
                        w: a.w * u as u32,
                        h: a.h * u as u32,
                    })
                    .collect();
            }
            match self.replay {
                Some(at) if at.elapsed() < lively::REPLAY_FOR => {
                    let stats = &self.core.watch.stats;
                    lively::replay(&mut canvas, &mut self.faces, stage, stats, at.elapsed());
                }
                _ => self.replay = None,
            }
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
        if let Some(plan) = &self.picker {
            paint_picker(&mut canvas, &mut self.faces, plan, (ow, oh), sf);
        }

        // The cards and footer are laid out as cells, then set in the readable
        // faces.
        let panel_x = ow;
        let cols = u16::try_from(w.get().saturating_sub(panel_x) / g.cell_w).unwrap_or(0);
        let rows = u16::try_from(oh / g.row_h).unwrap_or(0);
        let panel_area = Rect::new(0, 0, cols, rows);
        let mut cells = Buffer::empty(panel_area);
        let drawn = panel::render(
            &mut cells,
            panel_area,
            &frame.scene,
            &frame.views,
            &inks,
            &mut self.core.ui,
        );
        canvas.fill(i64::from(panel_x), 0, w.get() - panel_x, h.get(), PANEL_BG);
        paint_cells(&mut canvas, &mut self.faces, &cells, (panel_x, 0), g);
        paint_done_bars(&mut canvas, &drawn.outlines, panel_x, g, sf);
        let at = Instant::now();
        self.portraits.retain(|id, (_, made)| {
            at.duration_since(*made) < PORTRAIT_FOR && frame.scene.agents.contains_key(id)
        });
        for &(row, col, id, faded) in &drawn.avatars {
            if let Some(slot) = frame.scene.agents.get(&id) {
                let Some(scale) = pixtuoid_scene::render_scale::RenderScale::new(4) else {
                    continue;
                };
                let (head, _) = self.portraits.entry(id).or_insert_with(|| {
                    (
                        pixtuoid_scene::portrait(slot, pack, scale, SystemTime::now()),
                        at,
                    )
                });
                paint_portrait(&mut canvas, head, &cells, (row, col), (panel_x, g), faded);
            }
        }
        let panel_rows = drawn.rows;
        let foot_cols = u16::try_from(w.get() / g.cell_w).unwrap_or(0);
        let foot_area = Rect::new(0, 0, foot_cols, 1);
        let mut foot = Buffer::empty(foot_area);
        crate::core::footer(
            &mut foot,
            foot_area,
            &self.office,
            &self.core,
            frame.waiting.len(),
            frame.scene.agents.is_empty(),
            &[
                (
                    "t",
                    if self.on_top {
                        crate::i18n::t("取消置顶", "unpin")
                    } else {
                        crate::i18n::t("置顶", "pin on top")
                    },
                ),
                (
                    "",
                    crate::i18n::t("点门牌看说明", "click a sign to learn more"),
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
            board_hits,
            cat_hit,
            unit: i64::from(fit.scale().get()),
        });
        // The cards may have moved under a still pointer (the wheel, a
        // reorder): what it is over is read against this frame.
        self.hover();
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("buffer: {e}"))?;
        if self.keep_frame {
            buffer.copy_from_slice(&canvas.px);
        } else {
            // Swapped in, not copied: the canvas gets an older frame back,
            // and the next frame's reset paints over all of it.
            buffer.fill_from(&mut canvas.px);
        }
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
        full: &[AgentId],
        scale_factor: f64,
        extras: OfficeExtras,
    ) -> Option<Drawn> {
        let _feed = self.office.session.render(
            fit.look(WorldText::Host),
            RenderInputs {
                world: FloorInputs {
                    scene,
                    now: extras.now,
                    floor: FloorMeta::ground()
                        .with_weather(extras.weather)
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
        let mut speakers = Vec::new();
        let mut boards = Vec::new();
        let mut south = Vec::new();
        if let Picture::Floor(floor) = picture
            && let Some(text) = floor.text
        {
            let d = u32::from(fit.density().get());
            let u = u32::from(fit.upscale());
            south = rooms::rooms_of(floor.hits.layout(), d, u);
            signs = door_signs(&mut self.faces, text, d, u, scale_factor);
            let avoid: Vec<Area> = signs.iter().map(|s| s.area).collect();
            let win_w = u32::from(pixels.width()) * u;
            tags = place_tags(
                &mut self.faces,
                text,
                (d, u, win_w),
                labels,
                &extras.roles,
                full,
                scale_factor,
                &avoid,
                &extras.stuck,
            );
            boards = paint_world_text(art, text, d, extras.board.as_ref());
            speakers = text
                .bubbles
                .iter()
                .filter_map(|r| match r.role {
                    TextRole::Bubble(id) => Some(id),
                    _ => None,
                })
                .collect();
        }
        Some((tags, signs, speakers, south, boards))
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

    /// The figure in the office under window point (`x`, `y`), if any.
    fn figure_at(
        &self,
        last: &Layout,
        x: u32,
        y: u32,
    ) -> Option<pixtuoid_scene::display::HoverTarget> {
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
            Some(pixtuoid_scene::hit::SceneHit::Figure(t)) => Some(t.clone()),
            _ => None,
        }
    }

    /// A hand over what a click acts on, and the card under the pointer
    /// lit: the signs, the cat, the board and the panel's title took clicks
    /// with nothing to say so.
    fn hover(&mut self) {
        let Some(last) = self.last.as_ref().filter(|_| self.cursor_in) else {
            return;
        };
        let (x, y) = (self.cursor.x.max(0.0) as u32, self.cursor.y.max(0.0) as u32);
        let point = Area {
            x: i64::from(x),
            y: i64::from(y),
            w: 1,
            h: 1,
        };
        let in_office = x < last.panel_x;
        let card = if in_office {
            None
        } else {
            let row = u16::try_from(y / last.grid.row_h.max(1)).unwrap_or(u16::MAX);
            last.panel_rows
                .iter()
                .find(|(r, _)| *r == row)
                .map(|(_, id)| *id)
        };
        let clickable = if in_office {
            last.board_hits.iter().any(|r| r.meets(&point))
                || last.cat_hit.is_some_and(|r| r.meets(&point))
                || last.sign_hits.iter().any(|(r, _)| r.meets(&point))
                || last.tag_hits.iter().any(|(r, _)| r.meets(&point))
                || self.figure_at(last, x, y).is_some()
        } else {
            y < last.grid.row_h || card.is_some()
        };
        if self.core.ui.hovered != card {
            self.core.ui.hovered = card;
            self.next_frame = Instant::now();
        }
        if clickable != self.pointer {
            self.pointer = clickable;
            self.window.set_cursor(if clickable {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
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
        let in_office = x < last.panel_x;
        // A click on the office puts the replay away, whatever is under it.
        if in_office && self.replay.take().is_some() {
            return;
        }
        if in_office && last.board_hits.iter().any(|r| r.meets(&point)) {
            self.replay = Some(Instant::now());
            return;
        }
        if in_office && last.cat_hit.is_some_and(|r| r.meets(&point)) {
            self.reaction = Some((Reaction::Cat(point.x, point.y), Instant::now()));
            return;
        }
        if let Some((_, text)) = last
            .sign_hits
            .iter()
            .filter(|_| x < last.panel_x)
            .find(|(r, _)| r.meets(&point))
        {
            // Your office's sign opens whoever sits in its guest chair; with
            // no one waiting it says what the room is for, like any sign.
            if text == office_sign() && self.core.next_waiting().is_some() {
                self.core.act(Action::OpenNext, &mut self.office);
                return;
            }
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
            match self.figure_at(last, x, y) {
                Some(pixtuoid_scene::display::HoverTarget::Agent(id)) => Some(id),
                Some(pixtuoid_scene::display::HoverTarget::Pet(pet)) => {
                    let (cx, cy) = (i64::from(pet.centre.x), i64::from(pet.centre.y));
                    let at = (cx * last.unit, cy * last.unit);
                    self.reaction = Some((Reaction::Cat(at.0, at.1), Instant::now()));
                    return;
                }
                _ => None,
            }
        };
        let Some(id) = id else { return };
        let now = Instant::now();
        let first = self
            .last_click
            .filter(|&(prev, at, _)| prev == id && now - at < DOUBLE_CLICK);
        if let Some((_, _, expanded)) = first {
            self.last_click = None;
            // The first click's toggle and poke were on the way to this.
            self.pending_poke = None;
            self.core.ui.expanded = expanded;
            self.core.ui.selected = Some(id);
            self.core.act(Action::Open, &mut self.office);
        } else {
            self.last_click = Some((id, now, self.core.ui.expanded));
            self.core.select(id, &mut self.office);
            // An idle worker clicked in the office has something to say for
            // itself, once it is clear this is no double click; one at work
            // is left to it.
            let idle = self.core.scene().agents.get(&id).is_some_and(|s| {
                s.exiting_at.is_none() && matches!(s.state, pixtuoid_core::ActivityState::Idle)
            });
            if in_office && idle {
                self.pending_poke = Some((id, now));
            }
        }
    }

    /// The pending poke's answer, once a second click can no longer make it
    /// a double click.
    fn land_poke(&mut self) {
        let Some((id, at)) = self.pending_poke else {
            return;
        };
        if at.elapsed() < DOUBLE_CLICK {
            self.quick = true;
            return;
        }
        self.pending_poke = None;
        let n = self.pokes.entry(id).or_default();
        let poke = crate::fun::poke(id, *n);
        *n += 1;
        self.reaction = Some((Reaction::Worker(id, poke), Instant::now()));
    }
}

/// What [`Win::render_office`] leaves for the window to set over the office:
/// the badges, the signs, who is chatting, the south rooms, and the wall
/// board's lines on the art grid.
type Drawn = (
    Vec<Tag>,
    Vec<DoorSign>,
    Vec<AgentId>,
    Vec<rooms::Room>,
    Vec<BoardLine>,
);

/// What the office is drawn with beyond the scene itself.
struct OfficeExtras {
    weather: WeatherPolicy,
    /// Each worker's post, worn on its badge whether or not labels show.
    roles: HashMap<AgentId, Role>,
    /// The wall board's two lines, in place of its own.
    board: Option<[String; 2]>,
    /// The agents stuck at work, whose badges wear the stuck mark.
    stuck: Vec<AgentId>,
    now: SystemTime,
}

/// How long one chitchat line stays before the speaker says the next.
const CHITCHAT_TURN_MS: u64 = 4_000;

impl Win {
    /// What goes under the readable text: the late-night dark, the festival's dress, and what stands by each
    /// figure (supper, a briefcase, its mood, the cat, confetti); where the
    /// cat sits, when it does.
    fn paint_goings_on_under(
        &mut self,
        canvas: &mut Canvas,
        frame: &crate::core::Frame,
        tags: &[Tag],
        signs: &[DoorSign],
        stage: lively::Stage,
        now: SystemTime,
    ) -> Option<Area> {
        let clock = frame.clock;
        let overtime: Vec<AgentId> = frame
            .scene
            .agents
            .values()
            .filter(|s| crate::fun::overtime(s, frame.views.get(&s.agent_id), &clock, now))
            .map(|s| s.agent_id)
            .collect();
        if clock.late() && !overtime.is_empty() {
            lively::night_lights(canvas, stage, tags, &overtime);
        }
        if let Some(f) = clock.festival() {
            lively::festival(canvas, &mut self.faces, stage, f, signs);
        }
        let stuck: Vec<(AgentId, crate::tasks::Stuck)> = frame
            .scene
            .agents
            .values()
            .filter_map(|s| {
                crate::tasks::stuck_at_work(s, frame.views.get(&s.agent_id))
                    .map(|st| (s.agent_id, st))
            })
            .collect();
        let typhoon = frame.outlook.typhoon();
        for t in tags {
            let working =
                frame.scene.agents.get(&t.agent).is_some_and(|s| {
                    matches!(s.state, pixtuoid_core::ActivityState::Running { .. })
                });
            if overtime.contains(&t.agent) {
                lively::overtime_supper(canvas, stage, t);
            }
            if typhoon && working {
                lively::packing_up(canvas, stage, t);
            }
            if let Some((_, st)) = stuck.iter().find(|(id, _)| *id == t.agent) {
                lively::mood(canvas, stage, t, crate::fun::Mood::of(*st));
            }
            if let Some((_, ago)) = crate::fun::live_cheer(frame.views.get(&t.agent), now) {
                lively::confetti(canvas, stage, t, ago);
            }
        }
        let id = crate::fun::cat_visits(&stuck)?;
        let t = tags.iter().find(|t| t.agent == id)?;
        Some(lively::cat_by(canvas, stage, t))
    }

    /// What the last click got: a worker's stretch or word, the cat's meow.
    fn paint_reaction(&mut self, canvas: &mut Canvas, tags: &[Tag], stage: lively::Stage) {
        let Some((reaction, at)) = self.reaction else {
            return;
        };
        let ago = at.elapsed();
        if ago >= REACTION_FOR {
            self.reaction = None;
            return;
        }
        match reaction {
            Reaction::Worker(id, poke) => {
                let Some(t) = tags.iter().find(|t| t.agent == id) else {
                    return;
                };
                match poke {
                    crate::fun::Poke::Stretch => lively::stretch(canvas, stage, t, ago),
                    crate::fun::Poke::Say(line) => {
                        lively::chitchat(canvas, &mut self.faces, stage, t, line, &[]);
                    }
                }
            }
            Reaction::Cat(x, y) => {
                lively::meow(canvas, &mut self.faces, stage, (x, y), crate::fun::meow());
            }
        }
    }

    /// What sets the south rooms apart: the meeting rooms' whiteboards with
    /// their subagents' tasks, the waiting area's ticket machine counting
    /// those waiting on you, your office's window on the harbour.
    fn paint_rooms(
        &mut self,
        canvas: &mut Canvas,
        frame: &crate::core::Frame,
        tags: &[Tag],
        signs: &[DoorSign],
        rooms: &[rooms::Room],
        stage: lively::Stage,
    ) {
        use pixtuoid_scene::layout::RoomStyle;
        for room in rooms {
            match room.style {
                RoomStyle::Office => {
                    let (heading, lines) =
                        rooms::board_lines(&room.area, tags, &frame.scene, &frame.views);
                    rooms::whiteboard(
                        canvas,
                        &mut self.faces,
                        stage.scale_factor,
                        room,
                        (heading.as_deref(), &lines),
                    );
                }
                RoomStyle::Waiting => {
                    // The sessions standing in line: subagents wait with
                    // their parent, not in the queue.
                    let queued = frame
                        .waiting
                        .iter()
                        .filter(|id| {
                            frame
                                .scene
                                .agents
                                .get(id)
                                .is_some_and(|s| s.parent_id.is_none())
                        })
                        .count();
                    rooms::ticket_machine(canvas, room, queued, stage.ms);
                }
                RoomStyle::Boss => {
                    let door = signs
                        .iter()
                        .find(|s| s.text == office_sign())
                        .map(|s| &s.area);
                    rooms::sea_window(canvas, room, door, frame.clock.hour, stage.ms);
                }
                RoomStyle::Rooftop => {}
            }
        }
    }

    /// What goes over the readable text: cheers and the clapping for a
    /// comeback, the chitchat, the boss at your door, the typhoon signal.
    #[expect(clippy::too_many_arguments, reason = "one frame's layers, drawn once")]
    fn paint_goings_on_over(
        &mut self,
        canvas: &mut Canvas,
        frame: &crate::core::Frame,
        tags: &[Tag],
        signs: &[DoorSign],
        speakers: &[AgentId],
        rooms: &[rooms::Room],
        stage: lively::Stage,
        now: SystemTime,
    ) {
        use crate::fun::Occasion;
        let mut taken: Vec<Area> = tags
            .iter()
            .flat_map(|t| std::iter::once(t.badge).chain(t.label))
            .chain(signs.iter().map(|s| s.area))
            .collect();
        for t in tags {
            let Some((cheer, _)) = crate::fun::live_cheer(frame.views.get(&t.agent), now) else {
                continue;
            };
            lively::cheer_label(canvas, &mut self.faces, stage, t, cheer);
            if cheer == crate::tasks::Cheer::Recovered {
                for o in tags.iter().filter(|o| lively::neighbours(t, o, stage)) {
                    lively::clapping(canvas, stage, o);
                }
            }
        }
        // A PM handing a role its part: a ticket flies to the new figure.
        for t in tags {
            let Some(slot) = frame.scene.agents.get(&t.agent) else {
                continue;
            };
            let (Some(parent), Some(role)) = (
                slot.parent_id,
                frame.views.get(&t.agent).and_then(|v| v.role),
            ) else {
                continue;
            };
            let (Some(from), Some(p)) = (
                tags.iter().find(|o| o.agent == parent),
                lively::handoff_progress(slot.created_at, self.opened, now),
            ) else {
                continue;
            };
            lively::handoff(canvas, stage, from, t, p, to_u32(role.post().ink));
            self.quick = true;
        }
        let occasion = if frame.outlook.typhoon() {
            Occasion::Typhoon
        } else if frame.clock.lunch() {
            Occasion::Lunch
        } else {
            Occasion::Ordinary
        };
        let turn = stage.ms / CHITCHAT_TURN_MS;
        for id in speakers {
            let Some(t) = tags.iter().find(|t| t.agent == *id) else {
                continue;
            };
            // A typhoon is news even in a meeting; lunch is not.
            let occasion = if occasion != Occasion::Typhoon && rooms::in_meeting(t, rooms) {
                Occasion::Meeting
            } else {
                occasion
            };
            let line = crate::fun::chitchat(*id, frame.views.get(id), occasion, turn, now);
            if let Some(a) = lively::chitchat(canvas, &mut self.faces, stage, t, line, &taken) {
                taken.push(a);
            }
        }
        if let Some(ago) = self.core.watch.boss_at_door(now)
            && let Some(door) = signs.iter().find(|s| s.text == office_sign())
        {
            let stay = crate::fun::stats::BOSS_FOR;
            lively::boss(canvas, &mut self.faces, stage, door, ago, stay);
        }
        if let Some(signal) = frame.outlook.signal {
            lively::typhoon_signal(canvas, &mut self.faces, stage, signal);
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

/// Ink of the bar by a card done and waiting to be read: the panel's colour
/// for that group ([`panel::DONE`]).
const DONE_BAR: u32 = 0x00a6_d8ff;

/// A bar down the left of each card in `outlines` (its first and last panel
/// rows), in the gap between the panel's edge and its text. A box round
/// each read as the card being selected, and two at once as two selected.
fn paint_done_bars(canvas: &mut Canvas, outlines: &[(u16, u16)], panel_x: u32, g: Grid, sf: f64) {
    let t = (sf.round() as u32).max(1);
    let w = (2 * t).max(g.cell_w / 3);
    // Centred in the cell before the text, clear of the panel's border.
    let x = i64::from(panel_x + g.cell_w + (g.cell_w - w) / 2);
    for &(top, bottom) in outlines {
        let y0 = i64::from(u32::from(top) * g.row_h + t);
        let y1 = i64::from((u32::from(bottom) + 1) * g.row_h - t);
        canvas.fill(x, y0, w, u32::try_from(y1 - y0).unwrap_or(0), DONE_BAR);
    }
}

/// `slot`'s head over its avatar at (`row`, `col`) of the panel's `cells`, on
/// the avatar's own colour, so the card shows the figure it belongs to. Set at
/// a whole multiple of its art pixels where it fits, else every n-th of them;
/// `faded` sinks it into the panel as its card is.
fn paint_portrait(
    canvas: &mut Canvas,
    head: &pixtuoid_core::sprite::Frame,
    cells: &Buffer,
    (row, col): (u16, u16),
    (panel_x, g): (u32, Grid),
    faded: bool,
) {
    let (pw, ph) = (u32::from(head.width()), u32::from(head.height()));
    if pw == 0 || ph == 0 || col >= cells.area.width || row >= cells.area.height {
        return;
    }
    let (bx, by) = (
        panel_x + u32::from(col) * g.cell_w,
        u32::from(row) * g.row_h,
    );
    let (bw, bh) = (u32::from(panel::AVATAR_CELLS) * g.cell_w, g.row_h);
    let chip = color_u32(cells[(col, row)].bg, PANEL_BG);
    canvas.fill(i64::from(bx), i64::from(by), bw, bh, chip);
    let up = (bw / pw).min(bh / ph);
    let (k, step) = if up >= 1 {
        (up, 1)
    } else {
        (1, pw.div_ceil(bw).max(ph.div_ceil(bh)))
    };
    let (dw, dh) = (pw / step * k, ph / step * k);
    let (ox, oy) = (bx + (bw - dw.min(bw)) / 2, by + (bh - dh.min(bh)) / 2);
    let sink = |c: u32| {
        if !faded {
            return c;
        }
        let ch = |s: u32| {
            let (a, z) = ((c >> s) & 0xff, (PANEL_BG >> s) & 0xff);
            ((a * 45 + z * 55) / 100) << s
        };
        ch(16) | ch(8) | ch(0)
    };
    for y in 0..ph / step {
        for x in 0..pw / step {
            let px = head.get(
                u16::try_from(x * step).unwrap_or(u16::MAX),
                u16::try_from(y * step).unwrap_or(u16::MAX),
            );
            if let Some(Some(c)) = px.copied() {
                canvas.fill(
                    i64::from(ox + x * k),
                    i64::from(oy + y * k),
                    k,
                    k,
                    sink(to_u32(c)),
                );
            }
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
    /// The row its badge would take on its own, just over its head; above
    /// it, the badge stepped up off a neighbour's and a leader points back.
    rest_y: i64,
    /// Its back to the viewer, at a desk whose monitor rises over its head
    /// (pixtuoid hangs its badge over that): its desktop is beside it, not
    /// in front of it.
    faces_away: bool,
    label: Option<Area>,
    plate: u32,
    marker: u32,
    /// `marker` as the panel's card dot takes it.
    marker_rgb: Rgb,
    name: String,
    ink: u32,
    /// Its post, worn as a chip of the role's colour before its name.
    role: Option<Role>,
    task: Option<String>,
    /// Its task shown and its name whole; else its name alone, cut short
    /// ([`short_name`]) when no tier over its head has room for it whole.
    full: bool,
}

/// Up to this many figures every badge carries its task; past it only the
/// ones [`shown_in_full`] picks do, so a crowd's two-line tags do not pile
/// up over each other. A name reads whole wherever it finds room: a name cut
/// to "shop-…·1" beside one left whole told no one which repo sat there.
const ALL_FULL_UPTO: usize = 3;

/// Cells a short badge's name may take.
const SHORT_NAME_CELLS: u16 = 8;

/// `name` cut to [`SHORT_NAME_CELLS`], its namesake number (`·2`) kept: a
/// dot in its place told no one which session sat there.
fn short_name(name: &str) -> String {
    if crate::text::cells(name) <= SHORT_NAME_CELLS {
        return name.to_owned();
    }
    match name.rsplit_once('\u{b7}') {
        Some((base, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            let room = SHORT_NAME_CELLS.saturating_sub(crate::text::cells(n) + 1);
            format!("{}\u{b7}{n}", truncate_cells(base, room))
        }
        _ => truncate_cells(name, SHORT_NAME_CELLS),
    }
}

/// Each head's label: what it waits on you for while it does (the question
/// asked, the call to approve; [`panel::head_wait`]), else its task. Empty
/// while labels are hidden.
fn head_labels(
    frame: &crate::core::Frame,
    seen: &HashMap<AgentId, SystemTime>,
    shown: bool,
) -> HashMap<AgentId, String> {
    let mut labels = frame.labels.clone();
    if !shown {
        return labels;
    }
    for id in &frame.waiting {
        let Some(s) = frame.scene.agents.get(id) else {
            continue;
        };
        let read = panel::read_since_idle(s, seen);
        if let Some(w) = panel::head_wait(s, frame.views.get(id), read, LABEL_CELLS) {
            labels.insert(*id, w);
        }
    }
    labels
}

/// The agents whose badge and task read in full however many are in: the
/// one selected, and the ones waiting on you or stuck, which you are to act
/// on.
fn shown_in_full(frame: &crate::core::Frame, selected: Option<AgentId>) -> Vec<AgentId> {
    let stuck = frame
        .scene
        .agents
        .values()
        .filter(|s| crate::tasks::stuck_at_work(s, frame.views.get(&s.agent_id)).is_some());
    selected
        .into_iter()
        .chain(frame.waiting.iter().copied())
        .chain(stuck.map(|s| s.agent_id))
        .collect()
}

/// Rows over its badge a task label may climb to find room.
const LABEL_TIERS: i64 = 2;
/// Tiers a badge may step up off a neighbour's. Four up from a row of
/// desks reaches the heads of the row behind, and the top of the window
/// piles them up again.
const BADGE_TIERS: i64 = 3;

/// Layout rows a waiter's "!" or "?" rises over its head
/// (`plot_requires_action_mark` in pixtuoid-scene: up to 6, even-aligned).
const MARK_ROWS: i64 = 6;

/// Where `b`'s figure's head and the mark over it stand, in window pixels,
/// `scale` of them a layout unit: a neighbour's badge keeps off it, or it
/// hid who sat there and whether they wait on you.
fn head_area(b: &Badge, scale: i64) -> Area {
    let half_w = i64::from(pixtuoid_scene::layout::CHARACTER_SPRITE_W) * 3 / 8;
    let rows = MARK_ROWS + i64::from(pixtuoid_scene::layout::CHARACTER_SPRITE_H) / 3;
    Area {
        x: (i64::from(b.at.x) - half_w) * scale,
        y: (i64::from(b.head) - MARK_ROWS) * scale,
        w: u32::try_from(2 * half_w * scale).unwrap_or(0),
        h: u32::try_from(rows * scale).unwrap_or(0),
    }
}

/// Points a badge's name and a task label are set in.
const NAME_PT: f64 = 10.0;
const TASK_PT: f64 = 11.5;

fn pt_px(pt: f64, scale_factor: f64) -> u32 {
    (pt * scale_factor).round().max(8.0) as u32
}

fn tag_pad(scale_factor: f64) -> u32 {
    (2.0 * scale_factor).round() as u32
}

/// Where each badge goes, at `u` window pixels an art pixel in an office
/// `win_w` window pixels wide: just over its figure's head, whole, stepping
/// up a tier at a time while a neighbour holds the spot ([`stack_up`]); then
/// the task of each agent in `full` (all of them when only a few are in) on
/// a plate of its own over its badge, climbing a tier when a neighbour holds
/// the row, left out when no tier is free or it would leave the window's
/// top. The full ones take their rows first. The names and tasks are set
/// here rather than baked because 8 art pixels are too small to read.
#[expect(clippy::too_many_arguments, reason = "one frame's tags, placed once")]
fn place_tags(
    faces: &mut Faces,
    text: HostText<'_>,
    (d, u, win_w): (u32, u32, u32),
    labels: &HashMap<AgentId, String>,
    roles: &HashMap<AgentId, Role>,
    full: &[AgentId],
    scale_factor: f64,
    avoid: &[Area],
    stuck: &[AgentId],
) -> Vec<Tag> {
    let (name_px, task_px) = (pt_px(NAME_PT, scale_factor), pt_px(TASK_PT, scale_factor));
    let pad = tag_pad(scale_factor);
    let all_full = text.badges.len() <= ALL_FULL_UPTO;
    // Not off the window's either edge, by the rooftop.
    let inside = |x: i64, w: u32| x.min(i64::from(win_w) - i64::from(w)).max(0);
    let mut tags: Vec<Tag> = text
        .badges
        .iter()
        .map(|b| {
            // On the head, not where pixtuoid hangs it: over a back-turned
            // sitter's monitor that is two rows of desks up, over the heads
            // of whoever sits there.
            let on_head = Badge {
                at: pixtuoid_scene::layout::Point { y: b.head, ..b.at },
                ..b.clone()
            };
            let at = run_rect(&badge_run(&on_head), d);
            let shown = all_full || full.contains(&b.agent);
            let name = b.name.text().to_owned();
            let role = roles.get(&b.agent).copied();
            // The chip is part of the badge, so a neighbour steps clear of it
            // too, and a short badge keeps it: the post reads in a crowd.
            let chip = role.map_or(0, |r| role_chip_w(faces, r, name_px, pad) + pad);
            let (w, h) = (
                Faces::cells(&name) * faces.cell_w(name_px) + 3 * pad + chip,
                name_px + 2 * pad,
            );
            let cx = (at.x + i64::from(at.w / 2)) * i64::from(u);
            let foot = (at.y + i64::from(at.h)) * i64::from(u);
            Tag {
                agent: b.agent,
                badge: Area {
                    x: inside(cx - i64::from(w / 2), w),
                    y: foot - i64::from(h),
                    w,
                    h,
                },
                figure_top: i64::from(b.head) * i64::from(d * u),
                rest_y: foot - i64::from(h),
                faces_away: b.at.y < b.head,
                label: None,
                plate: to_u32(b.plate),
                marker: to_u32(b.marker),
                marker_rgb: b.marker,
                name,
                ink: to_u32(b.name.ink),
                role,
                task: labels
                    .get(&b.agent)
                    .filter(|_| shown)
                    .map(|t| truncate_cells(t, LABEL_CELLS)),
                full: shown,
            }
        })
        .collect();
    // Stable, so the short ones keep pixtuoid's order among themselves.
    tags.sort_by_key(|t| !t.full);
    let heads: Vec<(AgentId, Area)> = text
        .badges
        .iter()
        .map(|b| (b.agent, head_area(b, i64::from(d * u))))
        .collect();
    // A stuck badge wears its mark to its right ([`paint_stuck`]); a
    // neighbour keeps clear of that too.
    let mark_w = stuck_mark_w(faces, name_px, pad) + pad;
    let mut taken: Vec<Area> = Vec::with_capacity(tags.len());
    for t in &mut tags {
        let tail = if stuck.contains(&t.agent) { mark_w } else { 0 };
        let reach = |b: Area| Area { w: b.w + tail, ..b };
        let clear: Vec<Area> = heads
            .iter()
            .filter(|(id, _)| *id != t.agent)
            .map(|&(_, h)| h)
            .chain(taken.iter().copied())
            .collect();
        let step = i64::from(t.badge.h + pad);
        let mut spot = stack_up(reach(t.badge), &clear, step);
        // No tier has room for it whole: its name cut short, as a crowd's
        // were, over a name piled on the next row's heads.
        if spot.is_none() && !t.full {
            let short = short_name(&t.name);
            let cut = Faces::cells(&t.name).saturating_sub(Faces::cells(&short))
                * faces.cell_w(name_px);
            let (cx, w) = (t.badge.x + i64::from(t.badge.w / 2), t.badge.w - cut);
            t.badge = Area {
                x: inside(cx - i64::from(w / 2), w),
                w,
                ..t.badge
            };
            t.name = short;
            spot = stack_up(reach(t.badge), &clear, step);
        }
        // Still none: it stays on its head.
        if let Some(s) = spot {
            t.badge.y = s.y;
        }
        taken.push(reach(t.badge));
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
                x: inside(b.x + i64::from(b.w / 2) - i64::from(w / 2), w),
                y: b.y - step * tier,
                w,
                h,
            })
            .find(|r| r.y >= 0 && !taken.iter().any(|o| o.meets(r)));
        taken.extend(t.label);
    }
    tags
}

/// `badge` stepped up `step` at a time, at most [`BADGE_TIERS`] tiers and
/// not off the window's top, to the first spot that covers none of `taken`
/// (a neighbour's badge, another figure's head); `None` when every tier is
/// covered.
fn stack_up(badge: Area, taken: &[Area], step: i64) -> Option<Area> {
    (0..=BADGE_TIERS)
        .map(|tier| Area {
            y: badge.y - step * tier,
            ..badge
        })
        .take_while(|b| b.y >= 0)
        .find(|b| !taken.iter().any(|o| o.meets(b)))
}

/// The leader from a badge `t` stepped up off its head back down to its
/// figure, as a window-pixel rectangle `thick` wide: the badge can sit over
/// a neighbour's head, and without it read as theirs. `None` on its head.
fn leader(t: &Tag, thick: u32) -> Option<Area> {
    let bottom = t.badge.y + i64::from(t.badge.h);
    if t.badge.y >= t.rest_y || bottom >= t.figure_top {
        return None;
    }
    let cx = t.badge.x + i64::from(t.badge.w / 2);
    Some(Area {
        x: cx,
        y: bottom,
        w: thick,
        h: u32::try_from(t.figure_top - bottom).unwrap_or(0),
    })
}

/// The signs pixtuoid would bake, on the art grid in its order. Its
/// chitchat bubbles are left to the window, which sets them in Chinese
/// ([`lively::chitchat`]); `board`, when given, takes the place of the wall
/// board's two lines.
fn paint_world_text(
    art: &mut Canvas,
    text: HostText<'_>,
    d: u32,
    board: Option<&[String; 2]>,
) -> Vec<BoardLine> {
    let mut signs: Vec<Area> = Vec::new();
    let mut boards = Vec::new();
    let mut lines = board.into_iter().flatten();
    // Door signs are set in the readable faces over the upscaled office
    // ([`door_signs`]): the art font has no Chinese.
    for run in text.signs.iter().filter(|r| !set_readable(r.role)) {
        let swapped;
        let line = if run.role == TextRole::Board {
            lines.next()
        } else {
            None
        };
        // By day the board shows its name alone: its tally's 5px words were
        // too small to read, counted otherwise than the panel's header beside
        // it, and English only. The evening's tally takes its lines.
        if run.role == TextRole::Board && line.is_none() {
            continue;
        }
        let run = match line {
            Some(line) => {
                let ink = run.spans.first().map_or(
                    Rgb {
                        r: 0xe6,
                        g: 0xe6,
                        b: 0xe6,
                    },
                    |s| s.ink,
                );
                swapped = TextRun {
                    spans: vec![pixtuoid_scene::display::TextSpan::new(line.clone(), ink)],
                    ..run.clone()
                };
                &swapped
            }
            None => run,
        };
        let rect = run_rect(run, d);
        if signs.iter().any(|s| s.meets(&rect)) {
            continue;
        }
        signs.push(rect);
        if run.role == TextRole::Board {
            // The tally is Chinese, which the art font has none of: its
            // plate is painted here and its words by the window over it.
            let words = line
                .zip(run.spans.first())
                .map(|(l, sp)| (l.clone(), sp.ink));
            if words.is_some()
                && let Some(plate) = run.plate
            {
                art.fill(rect.x, rect.y, rect.w, rect.h, to_u32(plate));
            }
            boards.push((rect, words.clone()));
            if words.is_some() {
                continue;
            }
        }
        paint_run(art, run, rect);
    }
    boards
}

/// Set the evening board's tally lines in the readable faces over their
/// places on the art grid, `u` window pixels an art pixel.
fn paint_board_words(canvas: &mut Canvas, faces: &mut Faces, boards: &[BoardLine], u: i64) {
    for (a, words) in boards {
        let Some((line, ink)) = words else { continue };
        let px = (a.h * u32::try_from(u).unwrap_or(1)).max(8);
        // The art's dimmest board ink, made for a glowing 8px font, sank
        // into the board as smooth type; lifted halfway to white it reads.
        let lift = |c: u8| c + (0xff - c) / 2;
        let ink = Rgb {
            r: lift(ink.r),
            g: lift(ink.g),
            b: lift(ink.b),
        };
        let base = a.y * u + i64::from(faces.ascent(px));
        faces.draw(
            canvas,
            a.x * u + i64::from(PLATE_PAD) * u,
            base,
            line,
            to_u32(ink),
            px,
        );
    }
}

/// A line on the wall board, on the art grid, and the words the window sets
/// on it in place of the art font's.
type BoardLine = (Area, Option<(String, Rgb)>);

/// Whether the window sets a sign of `role` in the readable faces rather
/// than the art font: the zones' door signs, in Chinese, and the benches'
/// repository plates, whose names may be.
fn set_readable(role: TextRole) -> bool {
    matches!(role, TextRole::DoorSign)
}

/// A door sign as the window sets it, on its colour,
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

/// A zone pixtuoid signs in ASCII, in the office's language and the
/// readable faces: the art font's 8px English is all but unreadable, and it
/// has no Chinese.
fn door_sign(text: &str) -> Option<&'static str> {
    use crate::i18n::t;
    Some(match text {
        "MTG A" => t("会议室 A", "Meeting A"),
        "MTG B" => t("会议室 B", "Meeting B"),
        "PANTRY" => t("茶水间", "Pantry"),
        "WAIT" => t("等候区", "Waiting"),
        "OFFICE" => office_sign(),
        "ROOF" => t("天台", "Rooftop"),
        _ => return None,
    })
}

/// Your office's door sign, which a click on opens the head of the line.
pub(crate) fn office_sign() -> &'static str {
    crate::i18n::t("你的办公室", "Your office")
}

/// What a zone pixtuoid signs in ASCII is for: who is in there, and what
/// moves them on, so the office reads without the README.
fn zone_about(text: &str) -> Option<&'static str> {
    use crate::i18n::t;
    Some(match text {
        "MTG A" | "MTG B" => t(
            "子 agent 跑满 15 秒的会话，和它的子 agent 一起在这儿开会。",
            "A session whose subagents have run for 15 seconds meets here with them.",
        ),
        "PANTRY" => t(
            "刚做完、你还没看的会话在这儿等你看结果。双击打开看过，或者做完满 15 分钟，就上天台。",
            "Sessions that just finished and you have not read wait here for you. Once you open one with a double-click, or 15 minutes after it finished, it goes up to the rooftop.",
        ),
        "WAIT" => t(
            "等你批准超过 20 秒，或者做完了在问你问题的会话，在这儿排队：等你批准的排前面，同类按先后。排第一个的坐你办公室的访客椅。你处理完它就回工位。",
            "Sessions waiting over 20 seconds for your approval, or finished and asking you a question, queue here: approvals first, then by arrival. The first in line sits in your office's guest chair. Once you deal with one it goes back to its desk.",
        ),
        "OFFICE" => t(
            "访客椅上坐的是该先处理的会话：等你批准的优先，其次是在问你问题的，同类等得久的先。有人等你时，点这块门牌或按 n 直接打开它。",
            "The guest chair holds the session to deal with first: approvals before questions, the longest waiting first. While someone waits, click this sign or press n to open it.",
        ),
        "ROOF" => t(
            "闲着、不用你管的会话：做完超过 15 分钟，或者你已经打开看过。你给它发新消息它就回工位。",
            "Idle sessions that need nothing from you: finished over 15 minutes ago, or already opened. Send one a new message and it goes back to its desk.",
        ),
        _ => return None,
    })
}

/// Points a sign's explanation is set in: a paragraph to read, which at the
/// sign's own 9pt came out at 9 pixels on a plain display.
const ABOUT_PT: f64 = 10.5;

/// How long a sign's explanation stays up unless clicked away.
const ABOUT_FOR: Duration = Duration::from_secs(10);
/// Cells an explanation's line may take: English takes about twice the
/// cells Chinese does to say the same.
fn about_cells() -> u32 {
    if crate::i18n::is_zh() { 26 } else { 36 }
}
const ABOUT_BG: u32 = 0x0024_2833;
const ABOUT_EDGE: u32 = 0x005a_6070;

/// `s` broken into lines of at most `max` cells: Chinese breaks at any
/// character, an ASCII word only at a space, which the break drops. A word
/// longer than a line is cut where it must be.
fn wrap_cells(s: &str, max: u32) -> Vec<String> {
    let max = max.max(2);
    let mut lines = vec![String::new()];
    let mut used = 0;
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let len = if c.is_ascii() && c != ' ' {
            rest.find(|c: char| !c.is_ascii() || c == ' ')
                .unwrap_or(rest.len())
        } else {
            c.len_utf8()
        };
        let (token, after) = rest.split_at(len);
        rest = after;
        let w = Faces::cells(token);
        if token == " " && (used == 0 || used + w > max) {
            // A break's space would only push the line past its width.
            if used + w > max {
                lines.push(String::new());
                used = 0;
            }
            continue;
        }
        if used + w > max && used > 0 {
            let line = lines.last_mut().expect("never empty");
            line.truncate(line.trim_end().len());
            lines.push(String::new());
            used = 0;
        }
        for c in token.chars() {
            let cw = Faces::cells(&c.to_string());
            if used + cw > max {
                lines.push(String::new());
                used = 0;
            }
            used += cw;
            lines.last_mut().expect("never empty").push(c);
        }
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
    let px = pt_px(ABOUT_PT, scale_factor);
    let pad = 2 * tag_pad(scale_factor);
    let cell = faces.cell_w(px).max(1);
    let fit = bounds.0.saturating_sub(2 * pad + 2) / cell;
    let lines = wrap_cells(sign.about, about_cells().min(fit));
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

/// Whether a key press reaches the office's keys. A held key repeats only
/// where repeating means more of the same (moving the selection, paging
/// floors): a held `q` would pass the second-press quit check, and a held
/// `l` flip the language back and forth, rewriting the env file each time.
/// With Cmd or Ctrl down a letter belongs to the system (Cmd+W, Cmd+L).
fn takes_key(key: Key<&str>, repeat: bool, mods: ModifiersState) -> bool {
    if matches!(key, Key::Character(_)) && (mods.super_key() || mods.control_key()) {
        return false;
    }
    !repeat
        || matches!(
            key,
            Key::Named(
                NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::PageUp | NamedKey::PageDown
            ) | Key::Character("j" | "k" | "[" | "]")
        )
}

/// The role a key picks in the picker: 1–8 the roster in order, 0 the PM,
/// who sorts out who else is needed.
fn picked_role(key: &str) -> Option<Role> {
    let n: usize = key.parse().ok()?;
    match n {
        0 => Some(Role::Pm),
        _ => crate::roles::ROSTER
            .get(n - 1)
            .map(|p| p.role)
            .filter(|r| *r != Role::Pm),
    }
}

/// The picker's lines, as `(key, role)`, PM last; a relay's suggested
/// posts first, each still on its own key so the digits never move.
fn picker_rows(suggested: &[Role]) -> Vec<(char, Role)> {
    let all: Vec<(char, Role)> = ('1'..='9')
        .filter_map(|k| Some((k, picked_role(&k.to_string())?)))
        .chain([('0', Role::Pm)])
        .collect();
    let first = suggested
        .iter()
        .filter_map(|r| all.iter().find(|(_, a)| a == r).copied());
    let rest = all.iter().filter(|(_, r)| !suggested.contains(r)).copied();
    first.chain(rest).collect()
}

/// The role picker, centred over the office: where it dispatches to, then
/// each role's key, its chip and its post. On a relay, the posts to hand
/// on to lead, marked, and the head names whose work goes with it.
fn paint_picker(
    canvas: &mut Canvas,
    faces: &mut Faces,
    plan: &crate::core::DispatchPlan,
    bounds: (u32, u32),
    scale_factor: f64,
) {
    let px = pt_px(ABOUT_PT, scale_factor);
    let pad = 2 * tag_pad(scale_factor);
    let cell = faces.cell_w(px).max(1);
    let repo = plan.repo();
    let (head, sub) = match &plan.relay {
        Some(r) => (
            crate::tr!("接着「{}」派给：", "Hand on from {} to:", r.name),
            crate::tr!(
                "派到 {repo}，任务带上它的活和结论（其他键取消）",
                "into {repo}, carrying its task and result (any other key cancels)"
            ),
        ),
        None => (
            crate::tr!("派活到 {repo}：", "Dispatch into {repo}:"),
            if plan.backend == crate::roles::Backend::Dsh {
                crate::i18n::t(
                    "开一个新 DSH 会话，在会话列表打开后写任务（其他键取消）",
                    "a new DSH session; open it from the session list and write its task (any other key cancels)",
                )
            } else {
                crate::i18n::t(
                    "开一个新会话，任务在 Claude 里写（其他键取消）",
                    "a new session; write its task in Claude (any other key cancels)",
                )
            }
            .to_owned(),
        ),
    };
    let suggested = plan.relay.as_ref().map_or(&[][..], |r| &r.suggested[..]);
    let tag = crate::i18n::t(" 建议", " suggested");
    let rows: Vec<(char, Role, String, bool)> = picker_rows(suggested)
        .into_iter()
        .map(|(k, r)| {
            let what = if r == Role::Pm {
                crate::i18n::t("（自动分工）", " (splits the work)")
            } else {
                ""
            };
            (k, r, format!("{} {}{what}", r.chip(), r.name()), suggested.contains(&r))
        })
        .collect();
    let chip_cells = crate::roles::ROSTER
        .iter()
        .map(|p| Faces::cells(p.role.chip()))
        .max()
        .unwrap_or(2);
    let line_h = px + px / 3;
    let widest = rows
        .iter()
        .map(|(_, r, t, s)| {
            2 + chip_cells - Faces::cells(r.chip()) + Faces::cells(t) + if *s { Faces::cells(tag) } else { 0 }
        })
        .chain([Faces::cells(&head), Faces::cells(&sub)])
        .max()
        .unwrap_or(0);
    let w = widest * cell + 2 * pad;
    let h = u32::try_from(rows.len() + 2).unwrap_or(0) * line_h + 2 * pad;
    let x = (i64::from(bounds.0) - i64::from(w)).max(0) / 2;
    let y = (i64::from(bounds.1) - i64::from(h)).max(0) / 2;
    canvas.fill(x - 1, y - 1, w + 2, h + 2, ABOUT_EDGE);
    canvas.fill(x, y, w, h, ABOUT_BG);
    let ascent = i64::from(faces.ascent(px));
    let top = |i: usize| y + i64::from(pad) + i64::try_from(i).unwrap_or(0) * i64::from(line_h);
    faces.draw(canvas, x + i64::from(pad), top(0) + ascent, &head, TASK, px);
    faces.draw(canvas, x + i64::from(pad), top(1) + ascent, &sub, PICKER_DIM, px);
    for (i, (key, role, text, picked)) in rows.iter().enumerate() {
        let (tx, ty) = (x + i64::from(pad), top(i + 2));
        if *picked {
            canvas.fill(x, ty - 1, w, line_h, PICKER_SUGGESTED_BG);
        }
        faces.draw(canvas, tx, ty + ascent, &key.to_string(), TEXT, px);
        let chip_x = tx + i64::from(2 * cell);
        // The role's ink on a dark chip, as on the badges and cards.
        let glyph = role.chip();
        let chip_w = Faces::cells(glyph) * cell;
        canvas.fill(chip_x - 1, ty, chip_w + 2, px + px / 6, to_u32(crate::roles::CHIP_BG));
        faces.draw(canvas, chip_x, ty + ascent, glyph, to_u32(role.post().ink), px);
        let rest = &text[glyph.len()..];
        // Every name starts in one column, whichever chip is widest.
        let rest_x = chip_x + i64::from(chip_cells * cell);
        faces.draw(canvas, rest_x, ty + ascent, rest, TEXT, px);
        if *picked {
            let tag_x = rest_x + i64::from(Faces::cells(rest) * cell);
            faces.draw(canvas, tag_x, ty + ascent, tag, PICKER_TAG, px);
        }
    }
}

const PICKER_DIM: u32 = 0x008a_8f98;
const PICKER_SUGGESTED_BG: u32 = 0x002f_3542;
/// The office's working green: a suggestion is the way forward.
const PICKER_TAG: u32 = 0x009e_ce6a;

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
            let shown = door_sign(&text)?.to_owned();
            let cx = i64::from(u32::from(r.at.x) * d) * i64::from(u);
            let cy = (i64::from(u32::from(r.at.y) * d) + cell / 2) * i64::from(u);
            let w = Faces::cells(&shown) * faces.cell_w(px) + 2 * pad;
            let h = px + 2 * pad;
            Some(DoorSign {
                area: Area {
                    x: cx - i64::from(w / 2),
                    y: cy - i64::from(h / 2),
                    w,
                    h,
                },
                about: zone_about(&text)?,
                text: shown,
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

/// Red for a stuck agent, the panel's stuck colour.
const STUCK_RED: u32 = 0x00f7_768e;

/// Mark each stuck agent in `stuck` where it sits: a red ring round its
/// figure and the panel's stuck mark by its badge, so it stands out from the
/// agents working normally around it. Not a "?": that read as a question for
/// you, which is what waiting means. `u` is window pixels an art pixel,
/// `scale` a layout unit.
fn paint_stuck(
    canvas: &mut Canvas,
    faces: &mut Faces,
    tags: &[Tag],
    stuck: &[AgentId],
    scale_factor: f64,
    (u, scale): (u16, u16),
) {
    let u = i64::from(u.max(1));
    let name_px = pt_px(NAME_PT, scale_factor);
    let pad = tag_pad(scale_factor);
    for t in tags.iter().filter(|t| stuck.contains(&t.agent)) {
        let b = t.badge;
        let (cx, cy, half_w, half_h) = figure_box(t, scale);
        let dot = (u / 2).max(1) as u32;
        for (x, y) in ring(cx, cy, half_w, half_h + u) {
            canvas.fill(x, y, dot, dot, STUCK_RED);
        }
        let w = stuck_mark_w(faces, name_px, pad);
        let (x, y) = (b.x + i64::from(b.w + pad), b.y);
        canvas.fill(x, y, w, b.h, STUCK_RED);
        let base = y + i64::from(pad) + i64::from(faces.ascent(name_px));
        // Dark on the red, as the panel's stuck chip is: white on it read
        // at 2:1.
        faces.draw(
            canvas,
            x + i64::from(pad),
            base,
            crate::panel::STUCK_MARK,
            PANEL_BG,
            name_px,
        );
    }
}

/// The width of the stuck mark [`paint_stuck`] sets right of a badge.
fn stuck_mark_w(faces: &mut Faces, name_px: u32, pad: u32) -> u32 {
    faces.cell_w(name_px) + 2 * pad
}

/// The centre and half extents of `t`'s figure in window pixels, `scale`
/// of them a layout unit: the figure is drawn in layout units, so sized by
/// art pixels a ring round it shrank to its head.
fn figure_box(t: &Tag, scale: u16) -> (i64, i64, i64, i64) {
    let (w, h) = figure_size(scale);
    (t.badge.x + i64::from(t.badge.w / 2), t.figure_top + h / 2, w * 3 / 4, h / 2)
}

/// A figure's width and height in window pixels, `scale` of them a layout
/// unit.
fn figure_size(scale: u16) -> (i64, i64) {
    let s = i64::from(scale.max(1));
    (
        i64::from(pixtuoid_scene::layout::CHARACTER_SPRITE_W) * s,
        i64::from(pixtuoid_scene::layout::CHARACTER_SPRITE_H) * s,
    )
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

/// How long one breath of the first waiting agent's halo takes.
const BREATH_MS: u64 = 2_400;
/// How long from `now` until the next beat starts.
fn to_next_beat(now: SystemTime) -> Duration {
    Duration::from_millis(BEAT_MS - lively::ms_of(now) % BEAT_MS)
}

/// 0..1 and back over `period` ms, smoothly.
fn breath(ms: u64, period: u64) -> f64 {
    let phase = (ms % period) as f64 / period as f64;
    0.5 - 0.5 * (std::f64::consts::TAU * phase).cos()
}

/// Blend `ink` over the pixel at (`x`, `y`) by `alpha`, clipped.
fn tint(canvas: &mut Canvas, x: i64, y: i64, ink: u32, alpha: f64) {
    if x < 0 || y < 0 || x >= i64::from(canvas.w) || y >= i64::from(canvas.h) {
        return;
    }
    let at = (y as u32 * canvas.w + x as u32) as usize;
    let mix = |shift: u32| {
        let (a, b) = (
            f64::from((canvas.px[at] >> shift) & 0xff),
            f64::from((ink >> shift) & 0xff),
        );
        ((a + (b - a) * alpha).round() as u32) << shift
    };
    canvas.px[at] = mix(16) | mix(8) | mix(0);
}

/// Mark `first`, the session the panel heads its waiting line with, where
/// it stands: a halo in the panel's waiting colour breathing round its figure,
/// so the one to answer first is found in the office as fast as on the
/// panel. Its "!" or "?" is the head mark every waiter wears: a second one
/// floating over its tags sat far off its head and said the same.
fn paint_first_waiting(
    canvas: &mut Canvas,
    tags: &[Tag],
    first: AgentId,
    stage: lively::Stage,
    scale: u16,
) {
    let Some(t) = tags.iter().find(|t| t.agent == first) else {
        return;
    };
    let ink = color_u32(crate::panel::WAIT, STUCK_RED);
    let u = stage.u;
    let (cx, cy, half_w, half_h) = figure_box(t, scale);
    // A band a few art pixels wide, densest at its inner edge, that swells
    // and fades as it breathes.
    let swell = breath(stage.ms, BREATH_MS);
    let band = (u * 3).max(3);
    let (rx, ry) = (half_w + u, half_h + 2 * u);
    for y in cy - ry - band..=cy + ry + band {
        for x in cx - rx - band..=cx + rx + band {
            let (dx, dy) = ((x - cx) as f64 / rx as f64, (y - cy) as f64 / ry as f64);
            let past = ((dx * dx + dy * dy).sqrt() - 1.0) * rx.min(ry) as f64 / band as f64;
            if (0.0..1.0).contains(&past) {
                tint(canvas, x, y, ink, (1.0 - past) * (0.25 + 0.4 * swell));
            }
        }
    }
}

/// White, the selected card's name on its badge.
const SELECTED_INK: u32 = 0x00ff_ffff;

/// Mark the figure of the card selected on the panel: a solid ring round it
/// and a frame round its badge, so the card and the figure it belongs to
/// are found as one. Its name alone turning white was lost in a crowd.
fn paint_selected(
    canvas: &mut Canvas,
    tags: &[Tag],
    selected: AgentId,
    scale: u16,
    scale_factor: f64,
) {
    let Some(t) = tags.iter().find(|t| t.agent == selected) else {
        return;
    };
    let line = (scale_factor.round() as u32).max(1);
    let b = t.badge;
    let (cx, cy, half_w, half_h) = figure_box(t, scale);
    let gap = i64::from(scale) / 2;
    for k in 0..2 * i64::from(line) {
        for (x, y) in ring(cx, cy, half_w + gap + k, half_h + gap + k) {
            canvas.fill(x, y, 1, 1, SELECTED_INK);
        }
    }
    let l = i64::from(line);
    for r in std::iter::once(b).chain(t.label) {
        let (x, y, w, h) = (r.x - l, r.y - l, r.w + 2 * line, r.h + 2 * line);
        canvas.fill(x, y, w, line, SELECTED_INK);
        canvas.fill(x, y + i64::from(h) - l, w, line, SELECTED_INK);
        canvas.fill(x, y, line, h, SELECTED_INK);
        canvas.fill(x + i64::from(w) - l, y, line, h, SELECTED_INK);
    }
}

/// A role chip's width on a badge: its glyph with a pad either side.
fn role_chip_w(faces: &mut Faces, role: Role, name_px: u32, pad: u32) -> u32 {
    Faces::cells(role.chip()) * faces.cell_w(name_px) + 2 * pad
}

/// `tags` with each name in its agent's panel group colour: pixtuoid inks a
/// name by the session's raw state, which has no stuck and no done, so a
/// stuck one read as working and a done one as idle beside a card that said
/// otherwise. A subagent wears its root's, as its card sits in its root's
/// group. The idle keep pixtuoid's quiet ink.
fn in_group_inks(
    mut tags: Vec<Tag>,
    frame: &crate::core::Frame,
    seen: &HashMap<AgentId, SystemTime>,
    now: SystemTime,
) -> Vec<Tag> {
    for t in &mut tags {
        let Some(mut slot) = frame.scene.agents.get(&t.agent) else {
            continue;
        };
        // Bounded: a parent cycle in corrupt state must not spin.
        for _ in 0..8 {
            match slot
                .parent_id
                .as_ref()
                .and_then(|p| frame.scene.agents.get(p))
            {
                Some(parent) => slot = parent,
                None => break,
            }
        }
        let read = crate::panel::read_since_idle(slot, seen);
        let group = crate::panel::Group::of(slot, frame.views.get(&slot.agent_id), now, read);
        if group != crate::panel::Group::Idle {
            t.ink = color_u32(group.color(), t.ink);
        }
    }
    tags
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
    // Under every plate, so a leader runs behind the badges it passes.
    let thick = (scale_factor.round() as u32).max(1);
    for t in tags {
        if let Some(l) = leader(t, thick) {
            canvas.fill(l.x, l.y, l.w, l.h, t.marker);
        }
    }
    for t in tags {
        let r = t.badge;
        canvas.fill(r.x, r.y, r.w, r.h, t.plate);
        canvas.fill(r.x, r.y, pad, r.h, t.marker);
        let ink = if selected == Some(t.agent) {
            SELECTED_INK
        } else {
            t.ink
        };
        let base = r.y + i64::from(pad) + i64::from(faces.ascent(name_px));
        let mut x = r.x + i64::from(2 * pad);
        if let Some(role) = t.role {
            let post = role.post();
            let w = role_chip_w(faces, role, name_px, pad);
            canvas.fill(
                r.x + i64::from(pad),
                r.y,
                w,
                r.h,
                to_u32(crate::roles::CHIP_BG),
            );
            faces.draw(
                canvas,
                r.x + i64::from(2 * pad),
                base,
                role.chip(),
                to_u32(post.ink),
                name_px,
            );
            x = r.x + i64::from(pad + w + pad);
        }
        faces.draw(canvas, x, base, &t.name, ink, name_px);
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

    /// A box round each unread done card read as the card being selected,
    /// two at once as two selected. Its mark is a bar in the gap left of the
    /// text: nothing across the card, nothing over the panel's border.
    #[test]
    fn an_unread_done_card_gets_a_side_bar_not_a_box() {
        let g = Grid {
            px: 13,
            cell_w: 8,
            row_h: 18,
        };
        let panel_x = 100;
        let mut canvas = Canvas::new(400, 200, 0);
        paint_done_bars(&mut canvas, &[(2, 4)], panel_x, g, 1.0);
        let inked: Vec<(u32, u32)> = (0..canvas.h)
            .flat_map(|y| (0..canvas.w).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.px[(y * canvas.w + x) as usize] == DONE_BAR)
            .collect();
        assert!(!inked.is_empty(), "the card is marked");
        assert_eq!(
            DONE_BAR,
            color_u32(panel::DONE, 0),
            "the bar is the done group's colour"
        );
        let (x0, x1) = (panel_x + g.cell_w, panel_x + 2 * g.cell_w);
        assert!(
            inked.iter().all(|&(x, _)| (x0..x1).contains(&x)),
            "only in the cell before the text"
        );
        let (top, bottom) = (2 * g.row_h, 5 * g.row_h);
        assert!(inked.iter().all(|&(_, y)| (top..bottom).contains(&y)));
        assert!(
            inked.iter().any(|&(_, y)| y < top + g.row_h),
            "from its first row"
        );
        assert!(
            inked.iter().any(|&(_, y)| y >= bottom - g.row_h),
            "to its last"
        );
    }

    fn area(x: i64, y: i64) -> Area {
        Area { x, y, w: 40, h: 10 }
    }

    /// Sitters side by side: a badge that would cover its neighbour's steps
    /// up over it whole, rather than both overlapping or one being cut to
    /// "shop-…·1" while the other reads in full.
    #[test]
    fn a_badge_on_a_taken_spot_stacks_up_over_it() {
        let step = 12;
        let at = |b: Area, taken: &[Area]| stack_up(b, taken, step).map(|b| b.y);
        assert_eq!(
            at(area(0, 100), &[area(200, 100)]),
            Some(100),
            "nothing in the way: it stays on its head"
        );
        assert_eq!(
            at(area(10, 100), &[area(0, 100)]),
            Some(100 - step),
            "one tier up off its neighbour"
        );
        let one = area(10, 100 - step);
        assert_eq!(
            at(area(20, 100), &[area(0, 100), one]),
            Some(100 - 2 * step),
            "and over the one stacked there too"
        );
        // A tier more reached the heads of the row behind: past the last
        // tier there is no spot, and the caller cuts the name short.
        let column: Vec<Area> = (0..=BADGE_TIERS).map(|k| area(0, 100 - step * k)).collect();
        assert_eq!(at(area(5, 100), &column), None, "no higher than its tiers");
        assert_eq!(at(area(0, 5), &[area(0, 5)]), None, "never off the window's top");
    }

    /// A stuck badge wears its mark to its right; a neighbour sitting just
    /// clear of the badge itself still steps off the mark, or covered it.
    #[test]
    fn a_neighbour_keeps_off_a_stuck_badges_mark() {
        let Ok(mut faces) = Faces::load() else {
            return;
        };
        let ink = Rgb { r: 9, g: 9, b: 9 };
        let id = |n: usize| AgentId::from_parts("claude-code", &format!("s{n}"));
        let badge = |n: usize, x: u16| Badge {
            agent: id(n),
            at: pixtuoid_scene::layout::Point { x, y: 60 },
            head: 60,
            marker: ink,
            name: pixtuoid_scene::display::TextSpan::new("cc·repo", ink),
            plate: ink,
        };
        let place = |faces: &mut Faces, badges: &[Badge], stuck: &[AgentId]| {
            let text = HostText {
                badges,
                bubbles: &[],
                signs: &[],
                dim: pixtuoid_scene::look::Dim::NONE,
            };
            place_tags(
                faces,
                text,
                (1, 4, 1600),
                &HashMap::new(),
                &HashMap::new(),
                &[],
                2.0,
                &[],
                stuck,
            )
        };
        let w = i64::from(place(&mut faces, &[badge(0, 100)], &[])[0].badge.w);
        // Centres a badge and a pad apart: the badges just clear each other.
        let gap = u16::try_from((w + i64::from(tag_pad(2.0))) / 4 + 1).unwrap();
        let pair = [badge(0, 100), badge(1, 100 + gap)];
        let free = place(&mut faces, &pair, &[]);
        assert_eq!(free[1].badge.y, free[1].rest_y, "clear of the badge alone");
        let marked = place(&mut faces, &pair, &[id(0)]);
        let (a, b) = (&marked[0], &marked[1]);
        let pad = tag_pad(2.0);
        let mark = Area {
            x: a.badge.x + i64::from(a.badge.w + pad),
            w: stuck_mark_w(&mut faces, pt_px(NAME_PT, 2.0), pad),
            ..a.badge
        };
        assert!(!b.badge.meets(&mark), "the neighbour covers the stuck mark");
    }

    /// A badge with no room for its whole name keeps a short one, not a
    /// dot: a dot told no one which session sat there. Its namesake number
    /// stays, or two sessions of one repo read the same.
    #[test]
    fn a_short_badge_keeps_its_name_and_number() {
        assert_eq!(short_name("blog"), "blog");
        assert_eq!(short_name("weather-bot"), "weather…");
        let n = short_name("agent-office\u{b7}2");
        assert!(n.ends_with("\u{b7}2"), "{n}");
        assert!(crate::text::cells(&n) <= SHORT_NAME_CELLS, "{n}");
    }

    /// Six long names side by side on a row of desks: stacked without end,
    /// the fourth tier covered the heads (and the "!" or "?") of the row
    /// behind and the window's top piled them up again. No badge climbs past
    /// its tiers or covers another figure's head; the ones with no room left
    /// read short, the ones in full still read whole.
    #[test]
    fn a_row_of_long_names_stays_within_its_tiers_and_off_other_heads() {
        let Ok(mut faces) = Faces::load() else {
            return;
        };
        let ink = Rgb { r: 9, g: 9, b: 9 };
        let id = |n: usize| AgentId::from_parts("claude-code", &format!("s{n}"));
        // The cutaway's density; desks 10 layout columns apart.
        let (d, u, win_w) = (4, 2, 1600);
        let scale = i64::from(d * u);
        let row = |n: usize, head: u16| Badge {
            agent: id(n),
            at: pixtuoid_scene::layout::Point {
                x: 30 + 10 * (n % 6) as u16,
                y: head,
            },
            head,
            marker: ink,
            name: pixtuoid_scene::display::TextSpan::new(
                format!("cc·customer-notifications-service·{n}"),
                ink,
            ),
            plate: ink,
        };
        // Six on the front row, and two on the row behind whose heads the
        // stack used to climb onto.
        let badges: Vec<Badge> = (0..6)
            .map(|n| row(n, 60))
            .chain((6..8).map(|n| row(n, 36)))
            .collect();
        let text = HostText {
            badges: &badges,
            bubbles: &[],
            signs: &[],
            dim: pixtuoid_scene::look::Dim::NONE,
        };
        let selected = id(3);
        let tags = place_tags(
            &mut faces,
            text,
            (d, u, win_w),
            &HashMap::new(),
            &HashMap::new(),
            &[selected],
            2.0,
            &[],
            &[],
        );
        let step = i64::from(tags[0].badge.h + tag_pad(2.0));
        for t in &tags {
            assert!(
                t.badge.y >= t.rest_y - BADGE_TIERS * step,
                "{} climbed past its tiers",
                t.name
            );
            for b in badges.iter().filter(|b| b.agent != t.agent) {
                assert!(
                    !t.badge.meets(&head_area(b, scale)),
                    "{} covers another figure's head",
                    t.name
                );
            }
        }
        let whole = |t: &Tag| {
            t.name == badges.iter().find(|b| b.agent == t.agent).unwrap().name.text()
        };
        let sel = tags.iter().find(|t| t.agent == selected).unwrap();
        assert!(whole(sel), "the selected one reads whole: {}", sel.name);
        assert!(
            tags.iter().any(|t| !whole(t)),
            "a crowd this long cannot all read whole"
        );
        for t in tags.iter().filter(|t| !whole(t)) {
            let n = badges.iter().position(|b| b.agent == t.agent).unwrap();
            assert!(t.name.ends_with(&format!("\u{b7}{n}")), "{} keeps its number", t.name);
            assert!(crate::text::cells(&t.name) <= SHORT_NAME_CELLS, "{}", t.name);
        }
    }

    fn tag_at(badge: Area, rest_y: i64, figure_top: i64) -> Tag {
        let ink = Rgb { r: 9, g: 9, b: 9 };
        Tag {
            agent: AgentId::from_parts("claude-code", "s"),
            badge,
            figure_top,
            rest_y,
            faces_away: false,
            label: None,
            plate: 0,
            marker: 0,
            marker_rgb: ink,
            name: "cc·repo".into(),
            ink: 0,
            role: None,
            task: None,
            full: true,
        }
    }

    /// A badge stepped up off its head can sit over a neighbour's head and
    /// read as theirs: a leader runs from it down to its own figure. One on
    /// its head needs none.
    #[test]
    fn a_stacked_badge_points_back_to_its_figure() {
        let resting = tag_at(area(0, 90), 90, 102);
        assert!(leader(&resting, 2).is_none(), "on its head");
        let stacked = tag_at(area(0, 66), 90, 102);
        let l = leader(&stacked, 2).expect("stepped up: a leader");
        assert_eq!(l.y, 76, "from the badge's foot");
        assert_eq!(l.y + i64::from(l.h), 102, "to its figure's head");
        assert_eq!(l.x, 20, "down from the badge's middle");
        assert_eq!(l.w, 2);
    }

    /// The figure of the card selected on the panel is ringed round its
    /// whole body and its badge framed, so it is found in a crowd: its name
    /// alone turning white was not enough. Nothing is drawn with none.
    #[test]
    fn the_selected_figure_is_ringed_and_its_badge_framed() {
        let scale = 8;
        let t = tag_at(area(60, 40), 40, 52);
        let mut none = Canvas::new(200, 200, 0);
        paint_selected(&mut none, &[], t.agent, scale, 1.0);
        assert!(none.px.iter().all(|&p| p == 0));
        let mut c = Canvas::new(200, 200, 0);
        paint_selected(&mut c, std::slice::from_ref(&t), t.agent, scale, 1.0);
        let at = |x: i64, y: i64| c.px[(y as u32 * c.w + x as u32) as usize];
        let (cx, cy, half_w, half_h) = figure_box(&t, scale);
        let column: Vec<i64> = (0..200)
            .filter(|&y| at(cx, y) == SELECTED_INK && y > t.figure_top)
            .collect();
        let bottom = *column.last().expect("the ring passes under the figure");
        assert!(bottom >= cy + half_h, "round the whole body, not its head");
        assert!(at(cx + half_w + scale as i64 / 2, cy) == SELECTED_INK, "and beside it");
        assert_eq!(at(t.badge.x - 1, t.badge.y), SELECTED_INK, "the badge framed");
    }

    /// A back-turned sitter's figure is drawn under its desk's monitor, and
    /// pixtuoid hangs its badge over that, two rows of desks up and over
    /// other heads. The office hangs it on the figure's head instead, at
    /// most a cell above, so a name reads as the one under it.
    #[test]
    fn a_badge_hangs_on_its_figures_head_not_over_the_desk() {
        let Ok(mut faces) = Faces::load() else {
            return;
        };
        let ink = Rgb { r: 9, g: 9, b: 9 };
        // The cutaway's density: a layout row is d art rows, each u pixels.
        let (d, u) = (4, 2);
        let head = 60;
        let badges = [Badge {
            agent: AgentId::from_parts("claude-code", "s"),
            // Where the raised monitor put it: 20 rows over the head.
            at: pixtuoid_scene::layout::Point {
                x: 40,
                y: head - 20,
            },
            head,
            marker: ink,
            name: pixtuoid_scene::display::TextSpan::new("cc·repo", ink),
            plate: ink,
        }];
        let text = HostText {
            badges: &badges,
            bubbles: &[],
            signs: &[],
            dim: pixtuoid_scene::look::Dim::NONE,
        };
        let tags = place_tags(
            &mut faces,
            text,
            (d, u, 400),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            2.0,
            &[],
            &[],
        );
        let t = &tags[0];
        let head_px = i64::from(head) * i64::from(d * u);
        assert_eq!(t.figure_top, head_px, "its ring and marks find the figure");
        let gap = head_px - (t.badge.y + i64::from(t.badge.h));
        let cell = i64::from(u32::from(pixtuoid_scene::layout::CELL_ROWS) * d * u);
        assert!((0..=cell).contains(&gap), "{gap} px over the head");
        assert!(leader(t, 1).is_none());
    }

    /// CIE76 distance between two colours: how far apart they read.
    fn cie76(a: Rgb, b: Rgb) -> f64 {
        fn lab(c: Rgb) -> [f64; 3] {
            let lin = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            let (r, g, b) = (lin(c.r), lin(c.g), lin(c.b));
            let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
            let f = |t: f64| {
                if t > 0.008856 {
                    t.cbrt()
                } else {
                    7.787 * t + 16.0 / 116.0
                }
            };
            [
                116.0 * f(y) - 16.0,
                500.0 * (f(x) - f(y)),
                200.0 * (f(y) - f(z)),
            ]
        }
        lab(a)
            .iter()
            .zip(lab(b))
            .map(|(p, q)| (p - q).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    /// The panel's state colours mean stuck, waiting, working and done; a
    /// repository wearing one of them read as its agents being in that
    /// state. Every repo hue stays apart from each (CIE76 distance).
    #[test]
    fn no_repo_hue_reads_as_a_state() {
        let rgb = |c: Color| {
            let Color::Rgb(r, g, b) = c else {
                unreachable!()
            };
            Rgb { r, g, b }
        };
        let states = [
            panel::Group::Stuck,
            panel::Group::Waiting,
            panel::Group::Working,
            panel::Group::Done,
        ];
        for hue in pixtuoid_scene::badge::REPO_HUES {
            for g in states {
                let de = cie76(hue, rgb(g.color()));
                // The old amber repo hue sat 29 from the waiting colour.
                assert!(de >= 33.0, "{hue:?} is {de:.0} from {g:?}");
            }
        }
    }

    /// Two repositories side by side in the office are told apart by their
    /// colour alone; a pair 20 apart (a pink and a purple, two violets) read
    /// as one repo. Every pair of repo hues stays apart (CIE76 distance).
    #[test]
    fn no_two_repo_hues_read_as_one() {
        let hues = pixtuoid_scene::badge::REPO_HUES;
        for (i, &a) in hues.iter().enumerate() {
            for &b in &hues[i + 1..] {
                let de = cie76(a, b);
                assert!(de >= 25.0, "{a:?} is {de:.0} from {b:?}");
            }
        }
    }

    /// By day the wall board shows its name alone: its tally was too small
    /// to read and counted otherwise than the panel's header. From six the
    /// evening's tally still takes the board's lines, and a click on them
    /// replays the day.
    #[test]
    fn the_board_shows_its_tally_only_in_the_evening() {
        let ink = Rgb {
            r: 200,
            g: 200,
            b: 200,
        };
        let line = |y: u16, role: TextRole, text: &str| TextRun {
            at: pixtuoid_scene::layout::Point { x: 2, y },
            align: Align::Left,
            spans: vec![pixtuoid_scene::display::TextSpan::new(text, ink)],
            plate: Some(Rgb { r: 1, g: 1, b: 1 }),
            strip: None,
            role,
        };
        let signs = [
            line(0, TextRole::Brand, "agent-office"),
            line(12, TextRole::Board, "2 req 4 run"),
            line(24, TextRole::Board, "up 3m"),
        ];
        let text = HostText {
            badges: &[],
            bubbles: &[],
            signs: &signs,
            dim: pixtuoid_scene::look::Dim::NONE,
        };
        let inked = |art: &Canvas, y0: u32, y1: u32| {
            (y0..y1).any(|y| (0..art.w).any(|x| art.px[(y * art.w + x) as usize] != 0))
        };
        let mut day = Canvas::new(120, 40, 0);
        assert!(paint_world_text(&mut day, text, 1, None).is_empty());
        assert!(inked(&day, 0, 12), "the name stays");
        assert!(!inked(&day, 12, 40), "no tally by day");
        let mut evening = Canvas::new(120, 40, 0);
        let tally = ["12 turns today".to_owned(), "stuck 2×".to_owned()];
        let boards = paint_world_text(&mut evening, text, 1, Some(&tally));
        assert_eq!(boards.len(), 2, "the evening's two lines, to click");
    }

    /// Done and waiting were two yellows; done is a cool colour now, and as
    /// a card's chip and a name over the dark panel it still reads at AA.
    #[test]
    fn done_reads_apart_from_waiting_and_at_aa() {
        fn lum(c: u32) -> f64 {
            let ch = |s: u32| {
                let v = f64::from((c >> s) & 0xff) / 255.0;
                if v <= 0.039_28 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
        }
        let ratio = |a: u32, b: u32| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        let done = color_u32(panel::Group::Done.color(), 0);
        let wait = color_u32(panel::Group::Waiting.color(), 0);
        let blue_minus_red = |c: u32| i64::from(c & 0xff) - i64::from((c >> 16) & 0xff);
        assert!(blue_minus_red(done) > 0, "done is cool");
        assert!(blue_minus_red(wait) < 0, "waiting is warm");
        assert!(ratio(done, PANEL_BG) >= 4.5, "done text on the panel");
    }

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
        // A quarter past the next step up is no nudge: holding there drew
        // the art half size in an office a third bigger than the window's own.
        let next = (step..3000)
            .find(|&h| scale(fit_for(&pack, 1300, h, None)) > scale(big))
            .expect("a step above the big scale");
        let past = next + next / 4;
        assert_eq!(
            scale(fit_for(&pack, 1300, past, Some(big))),
            scale(fit_for(&pack, 1300, past, None)),
            "{past} tall"
        );
        assert_eq!(
            scale(fit_for(&pack, 1300, 700, Some(big))),
            scale(fit_for(&pack, 1300, 700, None))
        );
    }

    /// Every zone pixtuoid signs reads in the office's language in the
    /// window: a sign the map missed would vanish, not fall back to ASCII.
    #[test]
    fn every_door_sign_has_its_words() {
        for (ascii, zh) in [
            ("MTG A", "会议室 A"),
            ("MTG B", "会议室 B"),
            ("PANTRY", "茶水间"),
            ("WAIT", "等候区"),
            ("OFFICE", "你的办公室"),
            ("ROOF", "天台"),
        ] {
            assert_eq!(door_sign(ascii), Some(zh));
        }
        assert_eq!(door_sign("LOBBY"), None);
        let en = crate::i18n::with(crate::i18n::Lang::En, || {
            ["MTG A", "MTG B", "PANTRY", "WAIT", "OFFICE", "ROOF"].map(door_sign)
        });
        // The office sign is found by its words: a click on it opens the
        // head of the line, so it must match in either language.
        assert_eq!(
            en[4],
            crate::i18n::with(crate::i18n::Lang::En, || Some(office_sign()))
        );
        assert!(en.iter().all(|s| s.is_some_and(|s| s.is_ascii())), "{en:?}");
    }

    /// Clicking any zone's sign must say what the zone is for: a sign the
    /// map missed would click to nothing, and the office would read only
    /// with the README beside it.
    #[test]
    fn every_door_sign_says_what_its_zone_is_for() {
        for ascii in ["MTG A", "MTG B", "PANTRY", "WAIT", "OFFICE", "ROOF"] {
            assert!(zone_about(ascii).is_some_and(|s| !s.is_empty()), "{ascii}");
            let en = crate::i18n::with(crate::i18n::Lang::En, || zone_about(ascii));
            assert!(en.is_some_and(|s| s.is_ascii()), "{ascii}: {en:?}");
        }
        assert_eq!(zone_about("LOBBY"), None);
    }

    /// An explanation is set on a plate of bounded width: a line wider than
    /// asked would run off the office or under the panel, and losing or
    /// reordering a character would change what it says. Only the spaces
    /// it breaks at go.
    #[test]
    fn an_explanation_wraps_within_its_width_and_keeps_every_character() {
        use crate::i18n::{Lang, with};
        for lang in [Lang::Zh, Lang::En] {
            let s = with(lang, || zone_about("WAIT")).expect("signed");
            for max in [3, 10, 26, 36] {
                let lines = wrap_cells(s, max);
                assert!(
                    lines.iter().all(|l| Faces::cells(l) <= max),
                    "{max}: {lines:?}"
                );
                assert_eq!(lines.concat().replace(' ', ""), s.replace(' ', ""), "{max}");
            }
        }
        assert_eq!(wrap_cells("短", 26), vec!["短".to_owned()]);
    }

    /// English breaks between words: "approval" cut into "appro" and
    /// "val" reads as two words.
    #[test]
    fn english_wraps_between_words() {
        assert_eq!(
            wrap_cells("queue here for your approval", 12),
            vec!["queue here", "for your", "approval"]
        );
        // A word longer than the line is still cut, not run off the plate.
        assert_eq!(wrap_cells("abcdefgh", 3), vec!["abc", "def", "gh"]);
    }

    /// A crowd's two-line tags piled over each other and ran off the
    /// window's east edge. Past a few figures only the one selected or
    /// waiting shows its name and task, the rest a dot each, and every tag
    /// stays inside the office.
    #[test]
    fn in_a_crowd_only_the_ones_to_act_on_read_in_full_and_none_leave_the_office() {
        let Ok(mut faces) = Faces::load() else {
            // Not macOS, or the system faces moved: nothing to check here.
            return;
        };
        let ink = Rgb { r: 9, g: 9, b: 9 };
        let id = |n: usize| AgentId::from_parts("claude-code", &format!("s{n}"));
        let (d, u, win_w) = (1, 4, 400);
        // Shoulder to shoulder, the last at the east edge.
        let badges: Vec<Badge> = (0..6)
            .map(|n| Badge {
                agent: id(n),
                at: pixtuoid_scene::layout::Point {
                    x: 40 + 10 * n as u16,
                    y: 40,
                },
                head: 40,
                marker: ink,
                name: pixtuoid_scene::display::TextSpan::new(format!("cc·repo·{n}"), ink),
                plate: ink,
            })
            .chain(std::iter::once(Badge {
                agent: id(6),
                at: pixtuoid_scene::layout::Point { x: 99, y: 40 },
                head: 40,
                marker: ink,
                name: pixtuoid_scene::display::TextSpan::new("cc·toolhub-store", ink),
                plate: ink,
            }))
            .collect();
        let text = HostText {
            badges: &badges,
            bubbles: &[],
            signs: &[],
            dim: pixtuoid_scene::look::Dim::NONE,
        };
        let labels: HashMap<AgentId, String> =
            (0..7).map(|n| (id(n), "改登录页".to_owned())).collect();
        let tags = place_tags(
            &mut faces,
            text,
            (d, u, win_w),
            &labels,
            &HashMap::new(),
            &[id(2), id(6)],
            2.0,
            &[],
            &[],
        );
        assert_eq!(tags.len(), 7, "every figure keeps a tag to click");
        for t in &tags {
            let wanted = t.agent == id(2) || t.agent == id(6);
            assert_eq!(t.full, wanted, "{}", t.name);
            assert_eq!(t.task.is_some(), wanted, "only a full tag carries its task");
            for r in std::iter::once(t.badge).chain(t.label) {
                assert!(
                    r.x >= 0 && r.x + i64::from(r.w) <= i64::from(win_w),
                    "{} inside the office",
                    t.name
                );
            }
        }
        let full: Vec<Area> = tags
            .iter()
            .filter(|t| t.full)
            .flat_map(|t| std::iter::once(t.badge).chain(t.label))
            .collect();
        for (i, a) in full.iter().enumerate() {
            assert!(
                !full[i + 1..].iter().any(|b| a.meets(b)),
                "full tags do not overlap"
            );
        }
    }
    /// A role's chip is drawn inside its badge: placed without it, a chip
    /// would cover the name of the figure beside it. With it counted, the
    /// badge is wider and its neighbour still steps clear.
    #[test]
    fn a_role_chip_widens_its_badge_so_a_neighbour_still_steps_clear() {
        let Ok(mut faces) = Faces::load() else {
            return;
        };
        let ink = Rgb { r: 9, g: 9, b: 9 };
        let id = |n: usize| AgentId::from_parts("claude-code", &format!("s{n}"));
        let badges: Vec<Badge> = (0..2)
            .map(|n| Badge {
                agent: id(n),
                at: pixtuoid_scene::layout::Point {
                    x: 40 + 12 * n as u16,
                    y: 40,
                },
                head: 40,
                marker: ink,
                name: pixtuoid_scene::display::TextSpan::new("cc·repo", ink),
                plate: ink,
            })
            .collect();
        let text = HostText {
            badges: &badges,
            bubbles: &[],
            signs: &[],
            dim: pixtuoid_scene::look::Dim::NONE,
        };
        let place = |faces: &mut Faces, roles: &HashMap<AgentId, Role>| {
            place_tags(faces, text, (1, 4, 400), &HashMap::new(), roles, &[], 2.0, &[], &[])
        };
        let plain = place(&mut faces, &HashMap::new());
        let roles: HashMap<AgentId, Role> = [(id(0), Role::Frontend), (id(1), Role::Qa)].into();
        let worn = place(&mut faces, &roles);
        for (p, w) in plain.iter().zip(&worn) {
            assert_eq!(w.role, roles.get(&w.agent).copied());
            assert!(w.badge.w > p.badge.w, "the chip widens the badge");
        }
        assert!(!worn[0].badge.meets(&worn[1].badge), "neighbours step clear");
    }

    /// Quitting takes two presses of `q` so a stray one does not close the
    /// office; a held `q` must not count as two. A held arrow still runs down
    /// the list, and Cmd+W or Cmd+L is the system's, not `w` or `l`.
    #[test]
    fn held_and_system_keys_do_not_act() {
        let none = ModifiersState::empty();
        assert!(takes_key(Key::Character("q"), false, none));
        assert!(!takes_key(Key::Character("q"), true, none));
        assert!(!takes_key(Key::Character("l"), true, none));
        assert!(!takes_key(Key::Named(NamedKey::Enter), true, none));
        assert!(takes_key(Key::Named(NamedKey::ArrowDown), true, none));
        assert!(takes_key(Key::Character("j"), true, none));
        assert!(takes_key(Key::Named(NamedKey::PageUp), true, none));
        for m in [ModifiersState::SUPER, ModifiersState::CONTROL] {
            assert!(!takes_key(Key::Character("w"), false, m));
            assert!(!takes_key(Key::Character("l"), false, m));
            // Named keys are not letters the system binds here.
            assert!(takes_key(Key::Named(NamedKey::ArrowUp), false, m));
        }
        assert!(takes_key(Key::Character("Q"), false, ModifiersState::SHIFT));
    }

    /// The keys follow the roster as the picker lists it, and 0 is the PM:
    /// a key that picked another role than its line says would start the
    /// wrong session.
    #[test]
    fn picker_digits_follow_the_roster_and_0_is_the_pm() {
        let rows = picker_rows(&[]);
        assert_eq!(rows.len(), crate::roles::ROSTER.len());
        assert_eq!(rows.last(), Some(&('0', Role::Pm)));
        for (key, role) in &rows {
            assert_eq!(picked_role(&key.to_string()), Some(*role));
        }
        assert_eq!(picked_role("1"), Some(crate::roles::ROSTER[0].role));
        assert_eq!(picked_role("9"), None, "only eight roles before the PM");
        assert_eq!(picked_role("x"), None);
    }

    /// A relay's suggestions lead the list, but each keeps its own digit:
    /// the keys you learned still pick the same posts.
    #[test]
    fn suggested_posts_lead_and_keep_their_keys() {
        let rows = picker_rows(&[Role::Reviewer, Role::Qa]);
        assert_eq!(rows[0].1, Role::Reviewer);
        assert_eq!(rows[1].1, Role::Qa);
        assert_eq!(rows.len(), crate::roles::ROSTER.len());
        for (key, role) in &rows {
            assert_eq!(picked_role(&key.to_string()), Some(*role));
        }
    }
}
