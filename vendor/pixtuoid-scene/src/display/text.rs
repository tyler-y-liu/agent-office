//! Text as the display list lays it out: a grapheme cluster takes the cells
//! ratatui's buffer writes for it, each `ADVANCE` art pixels wide on a line
//! `LINE_H` tall. At the pack's 4x art a cell is one logical column, the
//! classic badge's terminal column. The rasterizer's font draws into these
//! cells.

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::{AgentId, AgentSlot};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::display::Icon;
use crate::display::cells::{CellMap, CellRect};
use crate::display::pen::ArtPx;
use crate::layout::Point;
use crate::theme::Theme;

/// A glyph's width in art pixels, its cell less the gap.
pub(crate) const GLYPH_W: u16 = 3;
/// A cell's width in art pixels: one glyph and its gap.
pub(crate) const ADVANCE: u16 = GLYPH_W + 1;
/// The row above the capitals, which only a lowercase letter's tall accent
/// (`ã`, `å`) inks: an accented capital keeps to the capitals' rows.
pub(crate) const ACCENT_ROWS: u16 = 1;
/// Fusion Pixel 8px's capital height in art pixels.
pub(crate) const CAP_H: u16 = 6;
/// A line's height in art pixels: the accent rows, the capitals, and a row for
/// descenders, Fusion Pixel 8px's own box. Pinned to `fusion-pixel-8px.bin`'s header.
pub(crate) const LINE_H: u16 = ACCENT_ROWS + CAP_H + 1;

/// `text`'s width in art pixels, from its first ink column to its last: its
/// last cell's gap column is ink only when [`fills_cell`].
pub(crate) fn width(text: &str) -> ArtPx {
    let joins = clusters(text)
        .last()
        .is_some_and(|(cluster, _)| cluster.chars().all(fills_cell));
    let advance = advance(text).0;
    ArtPx(if joins {
        advance
    } else {
        advance.saturating_sub(1)
    })
}

/// Whether `c` is one of the light box-drawing lines or block elements the
/// cutaway draws to its cell's edges so neighbours join; every other glyph
/// leaves its cell's last column as the gap.
pub(crate) fn fills_cell(c: char) -> bool {
    matches!(
        c,
        '\u{2500}'
            | '\u{2502}'
            | '\u{250c}'
            | '\u{2510}'
            | '\u{2514}'
            | '\u{2518}'
            | '\u{251c}'
            | '\u{2524}'
            | '\u{252c}'
            | '\u{2534}'
            | '\u{253c}'
            | '\u{256d}'..='\u{2570}'
            | '\u{2574}'..='\u{2577}'
            | '\u{2580}'..='\u{259f}'
    )
}

/// Art pixels from a run's left edge to where a run `n` cells on starts.
pub(crate) fn columns(n: u16) -> ArtPx {
    ArtPx(n.saturating_mul(ADVANCE))
}

/// The cells `cluster` takes on a terminal: ratatui-core 0.1.2 writes none
/// for one holding a control (`Buffer::set_stringn`) and else its
/// `unicode-width` plus one per halfwidth (semi-)voiced sound mark
/// (`CellWidth for str`, `buffer/cell_width.rs:42-43`).
fn cluster_cells(cluster: &str) -> u16 {
    if cluster.contains(char::is_control) {
        return 0;
    }
    let marks = cluster
        .chars()
        .filter(|c| HALFWIDTH_SOUND_MARKS.contains(c))
        .count();
    u16::try_from(cluster.width() + marks).unwrap_or(u16::MAX)
}

/// U+FF9E and U+FF9F, which `unicode-width` gives no width but a terminal
/// draws in a cell of their own (ratatui-core's `count_halfwidth_sound_marks`).
const HALFWIDTH_SOUND_MARKS: [char; 2] = ['\u{ff9e}', '\u{ff9f}'];

/// Each grapheme cluster of `text` that takes a cell, and the cells it takes.
pub(crate) fn clusters(text: &str) -> impl Iterator<Item = (&str, u16)> {
    text.graphemes(true)
        .map(|cluster| (cluster, cluster_cells(cluster)))
        .filter(|&(_, n)| n > 0)
}

/// The cells `text` takes, the one width every run of it is laid out by.
pub fn cells(text: &str) -> u16 {
    clusters(text).fold(0, |sum, (_, n)| sum.saturating_add(n))
}

/// [`columns`] past all of `text`: where the run after it starts.
pub(crate) fn advance(text: &str) -> ArtPx {
    columns(cells(text))
}

/// Logical rows between a badge's anchor and the line it sits on.
pub(crate) const LABEL_GAP: u16 = 2;

/// Logical rows a chitchat bubble's anchor rides over its speaker's badge's: a
/// cell row clear of the badge.
const BUBBLE_LIFT: u16 = 2 * crate::layout::CELL_ROWS;

/// One line of text a frame shows: its spans laid end to end, on `plate` if it
/// has one. Its inks are resolved, so a painter reads no theme for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextRun {
    /// The logical point it is placed by, as `align` says.
    pub at: Point,
    /// Where `at` lies on its line.
    pub align: Align,
    /// Its spans, in reading order.
    pub spans: Vec<TextSpan>,
    /// The fill behind it, if any.
    pub plate: Option<Rgb>,
    /// A bar down its plate's left edge, in this ink.
    pub strip: Option<Rgb>,
    /// What it labels.
    pub role: TextRole,
}

/// A stretch of a [`TextRun`] in one ink.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextSpan {
    /// What it shows.
    pub content: Content,
    /// Its colour.
    pub ink: Rgb,
}

impl TextSpan {
    /// `text` in `ink`.
    pub fn new(text: impl Into<String>, ink: Rgb) -> Self {
        Self {
            content: Content::Text(text.into()),
            ink,
        }
    }

    /// It as a terminal writes it: [`Content::text`].
    pub fn text(&self) -> &str {
        self.content.text()
    }
}

/// What a stretch of text shows: characters, or an icon, which takes the
/// cells its terminal glyph does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Content {
    Text(String),
    Icon(super::Icon),
}

impl Content {
    /// It as a terminal writes it: an icon as its glyph.
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Icon(icon) => icon.terminal(),
        }
    }
}

/// An agent's name badge: its name in its tone on the badge plate, marked in
/// the source's hue, centred `LABEL_GAP` rows over `at`. A terminal marks it
/// with a [`BADGE_MARKER`](crate::badge::BADGE_MARKER) before the name, a
/// pixel painter with a bar down the plate's left edge. Its parts are fields,
/// so a painter reads them instead of a run's spans by position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Badge {
    /// Whose badge it is.
    pub agent: AgentId,
    /// The point it hangs over, its sprite's top-centre.
    pub at: Point,
    /// The marker's ink.
    pub marker: Rgb,
    /// The name, in its ink.
    pub name: TextSpan,
    /// The fill behind it.
    pub plate: Rgb,
}

impl Badge {
    /// `agent`'s badge over `anchor`.
    pub(crate) fn new(
        anchor: Point,
        agent: &AgentSlot,
        namesakes: &crate::badge::Namesakes<'_>,
        theme: &Theme,
    ) -> Self {
        let text = namesakes.text(agent);
        let ink = crate::badge::badge_ink(&text, crate::badge::tone_of(agent), theme);
        Self {
            agent: agent.agent_id,
            at: anchor,
            // agent-office: its repository's colour.
            marker: crate::badge::repo_hue(agent, &namesakes.2).unwrap_or(ink.marker),
            name: TextSpan::new(text, ink.name),
            plate: crate::badge::badge_plate_colour(theme),
        }
    }

    /// It as a pixel painter's run: the name on the plate, the marker its
    /// [`strip`](TextRun::strip).
    pub(crate) fn run(&self) -> TextRun {
        TextRun {
            at: self.at,
            align: Align::Over,
            spans: vec![self.name.clone()],
            plate: Some(self.plate),
            strip: Some(self.marker),
            role: TextRole::Badge(self.agent),
        }
    }

    /// The cells of `map` a line `w` cells wide takes over its anchor.
    pub fn line_at(&self, w: u16, map: CellMap) -> CellRect {
        line_at(self.at, Align::Over, w, map)
    }
}

/// The world text a host sets over a frame in its own font, in paint order:
/// the badges, each bubble over them, then the signs over every badge and
/// bubble.
#[derive(Debug, Clone, Copy)]
pub struct HostText<'a> {
    /// Each drawn agent's badge.
    pub badges: &'a [Badge],
    /// Each chitchat bubble, hung over its speaker's badge.
    pub bubbles: &'a [TextRun],
    /// The wall board's lines and the floor indicator.
    pub signs: &'a [TextRun],
    /// The dim of the frame it is set over, which it takes too.
    pub dim: crate::look::Dim,
}

/// A frame's [`HostText`], held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HostRuns {
    pub(crate) badges: Vec<Badge>,
    pub(crate) bubbles: Vec<TextRun>,
    pub(crate) signs: Vec<TextRun>,
}

impl HostRuns {
    /// Its text, set over a frame under `dim`.
    pub(crate) fn view(&self, dim: crate::look::Dim) -> HostText<'_> {
        HostText {
            badges: &self.badges,
            bubbles: &self.bubbles,
            signs: &self.signs,
            dim,
        }
    }
}

/// Where a [`TextRun`]'s `at` lies on its line, a cell row tall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    /// Centred over it, `LABEL_GAP` rows above: a badge over a head.
    Over,
    /// At its top-left.
    Left,
    /// At its top-right.
    Right,
    /// At its top-centre.
    Centre,
}

/// What a [`TextRun`] labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// An agent's name badge.
    Badge(AgentId),
    /// The wall board's brand.
    Brand,
    /// The wall board's star: a link to the repo.
    Star,
    /// A line of the wall board's mood or context.
    Board,
    /// The floor indicator over the elevator.
    Indicator,
    /// An agent's chitchat bubble.
    Bubble(AgentId),
    /// agent-office: a zone's sign by its door.
    DoorSign,
    /// agent-office: a bench's plate naming the repository sat at it.
    BenchPlate,
}

impl TextRun {
    /// Each of `runs` that is set, with where `place` sets it: a run yields
    /// to one before it that it `meets` there, since on a grid wider than the
    /// one the runs were laid out on the star would write over the brand
    /// (`no_run_overprints_another_on_its_line`). Each painter's grid brings
    /// its own `place` and `meets`.
    pub fn set_first<P>(
        runs: &[TextRun],
        place: impl Fn(&TextRun) -> P,
        meets: impl Fn(&P, &P) -> bool,
    ) -> Vec<(&TextRun, P)> {
        let mut set: Vec<(&TextRun, P)> = Vec::new();
        for run in runs {
            let at = place(run);
            if !set.iter().any(|(_, s)| meets(s, &at)) {
                set.push((run, at));
            }
        }
        set
    }

    /// The floor indicator naming `floor` over the elevator at `door`:
    /// centred on the door, in the cell over it, on the badge plate, an arrow
    /// with no floor its way dim.
    pub(crate) fn indicator(door: Point, floor: crate::floor::FloorMeta, theme: &Theme) -> Self {
        Self {
            at: Point {
                x: door.x + crate::layout::ELEVATOR_W / 2,
                y: crate::layout::floor_indicator_rows(door.y).start,
            },
            align: Align::Centre,
            spans: crate::layout::floor_indicator(floor)
                .into_iter()
                .map(|content| TextSpan {
                    ink: match content {
                        Content::Icon(Icon::NoUp | Icon::NoDown) => theme.ui.tooltip_dim,
                        _ => theme.ui.neon_brand,
                    },
                    content,
                })
                .collect(),
            plate: Some(crate::badge::badge_plate_colour(theme)),
            strip: None,
            role: TextRole::Indicator,
        }
    }

    /// The room's signs: `board`'s lines, the floor indicator naming
    /// `floor` over the elevator, then agent-office's door signs.
    pub(crate) fn signs(
        board: &crate::neon_sign::BoardModel,
        layout: &crate::layout::SceneLayout,
        floor: crate::floor::FloorMeta,
        theme: &Theme,
    ) -> Vec<Self> {
        let mut runs = board.runs(theme);
        runs.push(Self::indicator(layout.door, floor, theme));
        runs.extend(Self::door_signs(layout));
        runs
    }

    /// agent-office: a plate over each bench naming the repository its
    /// sitters work in, on that repository's colour ([`crate::badge::repo_hues`]),
    /// so the benches read as projects; a bench shared by several names its
    /// first sitter's, and an empty bench has none. `agents` are the floor's.
    pub(crate) fn bench_plates(
        layout: &crate::layout::SceneLayout,
        agents: &[pixtuoid_core::AgentSlot],
    ) -> Vec<Self> {
        let hues = crate::badge::repo_hues(agents);
        // A bench is a row of desks a desk's stride apart, facing one way;
        // an aisle parts two in a row, and a pod's two rows face each other.
        let stride = crate::layout::DESK_W + crate::layout::INTRA_POD_GAP_X;
        let mut desks: Vec<crate::layout::HomeDesk> = layout.home_desks.clone();
        desks.sort_by_key(|d| (d.at.y, d.at.x));
        let mut benches: Vec<(u16, u16, crate::layout::HomeDesk)> = Vec::new();
        for d in desks {
            match benches.last_mut() {
                Some((_, last, first)) if first.at.y == d.at.y && d.at.x - *last <= stride => {
                    *last = d.at.x;
                }
                _ => benches.push((d.at.x, d.at.x, d)),
            }
        }
        benches
            .into_iter()
            .filter_map(|(x0, x1, first)| {
                let sitter = agents
                    .iter()
                    .filter(|a| {
                        layout
                            .home_desk(a.desk_index.single_floor_local())
                            .is_some_and(|d| d.at.y == first.at.y && (x0..=x1).contains(&d.at.x))
                    })
                    .min_by_key(|a| (a.created_at, a.agent_id))?;
                let repo = sitter
                    .label
                    .split_once('·')
                    .map_or(&*sitter.label, |(_, r)| r)
                    .to_owned();
                let plate = crate::badge::repo_hue(sitter, &hues)?;
                Some(Self {
                    // On the sitters' side of the screens: over a row facing
                    // the viewer, under one facing the windows.
                    at: Point {
                        x: (x0 + x1 + crate::layout::DESK_W) / 2,
                        y: match first.facing {
                            crate::layout::Facing::North => {
                                first.at.y + crate::layout::DESK_H + BENCH_PLATE_DROP
                            }
                            _ => first.at.y.saturating_sub(BENCH_PLATE_LIFT),
                        },
                    },
                    align: Align::Centre,
                    spans: vec![TextSpan::new(
                        repo,
                        Rgb {
                            r: 0x16,
                            g: 0x18,
                            b: 0x1d,
                        },
                    )],
                    plate: Some(plate),
                    strip: None,
                    role: TextRole::BenchPlate,
                })
            })
            .collect()
    }

    /// agent-office: a sign on each zone's north wall over its door, in the
    /// zone's colour, so the plan reads at a glance: the meeting rooms, the
    /// pantry, the waiting area, your office, and the rooftop's door out.
    pub(crate) fn door_signs(layout: &crate::layout::SceneLayout) -> Vec<Self> {
        use crate::layout::RoomStyle;
        const INK: Rgb = Rgb {
            r: 0xf6,
            g: 0xf2,
            b: 0xe8,
        };
        let sign = |x: u16, y: u16, text: String, plate: Rgb| Self {
            at: Point { x, y },
            align: Align::Centre,
            spans: vec![TextSpan::new(text, INK)],
            plate: Some(plate),
            strip: None,
            role: TextRole::DoorSign,
        };
        let door_x = |b: crate::layout::Bounds, pct: u16| b.x + crate::layout::pct(b.width, pct);
        let mut meeting = 0u8;
        let mut out: Vec<Self> = layout
            .meeting_rooms
            .iter()
            .filter_map(|r| {
                let b = r.bounds;
                let (x, text, plate) = match r.style {
                    RoomStyle::Office => {
                        meeting += 1;
                        (
                            door_x(b, 50),
                            format!("MTG {}", char::from(b'@' + meeting)),
                            Rgb {
                                r: 0x2f,
                                g: 0x5f,
                                b: 0xa8,
                            },
                        )
                    }
                    RoomStyle::Waiting => (
                        door_x(b, 50),
                        "WAIT".to_owned(),
                        Rgb {
                            r: 0xb3,
                            g: 0x78,
                            b: 0x1e,
                        },
                    ),
                    RoomStyle::Boss => (
                        door_x(b, crate::layout::OFFICE_DOOR_PCT),
                        "OFFICE".to_owned(),
                        Rgb {
                            r: 0x86,
                            g: 0x2c,
                            b: 0x22,
                        },
                    ),
                    RoomStyle::Rooftop => return None,
                };
                Some(sign(x, b.y.saturating_sub(SIGN_LIFT), text, plate))
            })
            .collect();
        if let Some(p) = layout.pantry {
            let b = p.bounds;
            out.push(sign(
                door_x(b, 50),
                b.y.saturating_sub(SIGN_LIFT),
                "PANTRY".to_owned(),
                Rgb {
                    r: 0x24,
                    g: 0x75,
                    b: 0x52,
                },
            ));
        }
        if let Some(roof) = layout.rooftop() {
            let b = roof.bounds;
            out.push(sign(
                b.x + b.width / 2,
                layout.corridor.y.saturating_sub(SIGN_LIFT),
                "ROOF".to_owned(),
                Rgb {
                    r: 0x1f,
                    g: 0x7f,
                    b: 0x77,
                },
            ));
        }
        out
    }

    /// `bubble` over its speaker's badge, which hangs over `badge_at`, in
    /// the tooltip's ink on its plate.
    pub(crate) fn bubble(
        bubble: &crate::chitchat::ChitchatBubble,
        badge_at: Point,
        theme: &Theme,
    ) -> Self {
        Self {
            at: Point {
                x: badge_at.x,
                y: badge_at.y.saturating_sub(BUBBLE_LIFT),
            },
            align: Align::Over,
            spans: vec![TextSpan::new(bubble.text, theme.ui.tooltip_text)],
            plate: Some(theme.ui.tooltip_bg),
            strip: None,
            role: TextRole::Bubble(bubble.speaker),
        }
    }

    /// Its spans' text, end to end.
    pub(crate) fn text(&self) -> String {
        self.spans.iter().map(TextSpan::text).collect()
    }

    /// The cells of `map` a line `w` cells wide takes where its align puts
    /// it: the one placement every terminal painter and the star's hit test
    /// read.
    pub fn line_at(&self, w: u16, map: CellMap) -> CellRect {
        line_at(self.at, self.align, w, map)
    }
}

/// agent-office: rows a bench's plate stands over a row facing the viewer,
/// and under a row facing the windows, past its sitters' chairs.
const BENCH_PLATE_LIFT: u16 = 7;
const BENCH_PLATE_DROP: u16 = 3;

/// agent-office: rows a door sign stands above its wall's line, on the
/// corridor side.
const SIGN_LIFT: u16 = 3;

/// The cells of `map` a line `w` cells wide takes, placed by `at` as `align`
/// says: the row `at`'s cell lies in, `LABEL_GAP` rows up for
/// [`Align::Over`].
fn line_at(at: Point, align: Align, w: u16, map: CellMap) -> CellRect {
    let anchor = match align {
        Align::Over => Point {
            y: at.y.saturating_sub(LABEL_GAP),
            ..at
        },
        Align::Left | Align::Right | Align::Centre => at,
    };
    let (col, row) = map.cell_of(anchor);
    let x = match align {
        Align::Left => col,
        Align::Right => col.saturating_sub(w),
        Align::Over | Align::Centre => col.saturating_sub(w / 2),
    };
    CellRect { x, y: row, w, h: 1 }
}

#[cfg(test)]
mod tests {

    /// agent-office: each bench row with a sitter names the repository it
    /// works in, on that repository's colour, so the benches read as
    /// projects; an empty row has no plate.
    #[test]
    fn each_occupied_bench_names_its_repository() {
        use crate::character::test_support::make_slot;
        let l = crate::layout::SceneLayout::compute_with_seed(320, 180, None, 0).expect("fits");
        let mut a = make_slot(
            pixtuoid_core::AgentId::from_transcript_path("/p/a.jsonl"),
            pixtuoid_core::state::ActivityState::Idle,
        );
        a.label = "cc·agent-office".into();
        a.cwd = std::sync::Arc::from(std::path::Path::new("/w/agent-office"));
        a.desk_index = pixtuoid_core::state::GlobalDeskIndex(0);
        let agents = [a];
        let plates = super::TextRun::bench_plates(&l, &agents);
        assert_eq!(plates.len(), 1, "one occupied row, one plate");
        let plate = &plates[0];
        assert_eq!(plate.text(), "agent-office");
        let hues = crate::badge::repo_hues(&agents);
        assert_eq!(plate.plate, crate::badge::repo_hue(&agents[0], &hues));
        let desk = l.home_desks[0];
        assert!(
            plate.at.x.abs_diff(desk.at.x) < 4 * (crate::layout::DESK_W + 4),
            "over its own bench"
        );
    }

    /// agent-office: every zone is named by a sign over its door, in a colour
    /// of its own, so the plan reads at a glance; a zone the window has no
    /// room for has no sign.
    #[test]
    fn each_zone_is_signed_once_in_its_own_colour() {
        let wide = crate::layout::SceneLayout::compute_with_seed(320, 180, None, 0).expect("fits");
        let signs = super::TextRun::door_signs(&wide);
        let names: Vec<String> = signs.iter().map(super::TextRun::text).collect();
        for name in ["MTG A", "MTG B", "PANTRY", "WAIT", "OFFICE", "ROOF"] {
            assert_eq!(
                names.iter().filter(|n| *n == name).count(),
                1,
                "{name} in {names:?}"
            );
        }
        let mut plates: Vec<_> = signs
            .iter()
            .filter(|s| s.text() != "MTG B")
            .map(|s| s.plate)
            .collect();
        plates.dedup();
        assert_eq!(plates.len(), 5, "a colour per kind of zone");
        let narrow =
            crate::layout::SceneLayout::compute_with_seed(160, 120, None, 0).expect("fits");
        let names: Vec<String> = super::TextRun::door_signs(&narrow)
            .iter()
            .map(super::TextRun::text)
            .collect();
        assert!(
            !names.iter().any(|n| n == "ROOF" || n == "MTG B"),
            "no sign for a zone that is not there: {names:?}"
        );
    }
    use super::*;

    /// The indicator's arrow is hollow and dim where no floor is its way:
    /// both lit between floors, the down arrow out on the ground floor, the up
    /// one on the top.
    #[test]
    fn an_arrow_with_no_floor_its_way_is_dim() {
        let theme = &crate::theme::NORMAL;
        let arrows = |floor, floors| -> Vec<(Icon, Rgb)> {
            let meta = crate::floor::FloorMeta::for_floor(floor, floors);
            TextRun::indicator(Point { x: 0, y: 0 }, meta, theme)
                .spans
                .into_iter()
                .filter_map(|s| match s.content {
                    Content::Icon(icon) => Some((icon, s.ink)),
                    Content::Text(_) => None,
                })
                .collect()
        };
        let (lit, dim) = (theme.ui.neon_brand, theme.ui.tooltip_dim);
        assert_eq!(arrows(1, 3), [(Icon::Up, lit), (Icon::Down, lit)]);
        assert_eq!(arrows(0, 3), [(Icon::Up, lit), (Icon::NoDown, dim)]);
        assert_eq!(arrows(2, 3), [(Icon::NoUp, dim), (Icon::Down, lit)]);
        assert_eq!(arrows(0, 1), [(Icon::NoUp, dim), (Icon::NoDown, dim)]);
    }

    #[test]
    fn a_run_is_its_advances_less_the_trailing_gap() {
        assert_eq!(width(""), ArtPx(0));
        assert_eq!(width("cc\u{b7}a"), ArtPx(4 * ADVANCE - 1));
    }
}
