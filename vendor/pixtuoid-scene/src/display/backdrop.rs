//! What the cutaway lays under a display list's pieces, resolved from the
//! layout and the theme where the list is built, so the rasterizer reads
//! neither.

use std::ops::Range;

use pixtuoid_core::sprite::Rgb;

use crate::layout::{Bounds, FixtureKind, SceneLayout, Size};
use crate::theme::Theme;

/// The ground, the north wall band and the floor coverings: none of it moves
/// within a layout and theme.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Backdrop {
    /// The office's size, in layout cells.
    pub(crate) size: Size,
    /// The north wall band's height ([`SceneLayout::wall_band_h`]).
    pub(crate) wall_band_h: u16,
    /// The posts between the band's windows
    /// ([`window_posts`](crate::layout::window_posts)).
    pub(crate) posts: Vec<Range<u16>>,
    /// The band's window rows ([`window_rows`](crate::layout::window_rows)).
    pub(crate) window_rows: Range<u16>,
    /// The band's trim row ([`wall_trim_row`](crate::layout::wall_trim_row)).
    pub(crate) trim_row: u16,
    /// Each floor covering, in the layout's fixture order.
    pub(crate) coverings: Vec<(Bounds, Covering)>,
    pub(crate) tones: BackdropTones,
}

/// The theme's tones the backdrop is laid in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BackdropTones {
    /// What a buffer holds before its first frame.
    pub(crate) bg: Rgb,
    pub(crate) wall: Rgb,
    pub(crate) window_frame: Rgb,
    pub(crate) wall_trim: Rgb,
}

/// A fixture flat on the ground, laid with the backdrop under every shadow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Covering {
    Rug { trim: Rgb, accent: Rgb, field: Rgb },
    #[allow(dead_code, reason = "agent-office: the corridor is laid in terrazzo")]
    Runner { base: Rgb, stripe: Rgb, edge: Rgb },
    /// agent-office: a cha chaan teng's mosaic floor, its colours its own.
    Mosaic,
    /// agent-office: a Hong Kong rooftop's green waterproof paint.
    RooftopPaint,
    /// agent-office: a zone's own floor, so where an agent is reads off what
    /// it stands on.
    Floor(Floor),
}

/// agent-office: the floor each zone of the office is laid in, its colours
/// its own under every theme (the hour's light still falls on it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Floor {
    /// The benches': grey-blue carpet tiles.
    Carpet,
    /// The corridor's: terrazzo, chips set in cement.
    Terrazzo,
    /// Your office's: rosewood in herringbone.
    Herringbone,
}

impl Covering {
    /// `kind`'s covering in `theme`'s tones; `None` for a fixture that stands
    /// rather than lies.
    pub(crate) fn of(kind: FixtureKind, theme: &Theme) -> Option<Self> {
        use FixtureKind as K;
        let f = &theme.furniture;
        match kind {
            // agent-office: the pantry is a cha chaan teng, tiled.
            K::PantryMat | K::IslandMat => Some(Self::Mosaic),
            K::MeetingRug { .. } | K::LoungeRug | K::Doormat { .. } => Some(Self::Rug {
                trim: f.rug_trim,
                accent: f.rug_accent,
                field: f.rug_field,
            }),
            // agent-office: the corridor is terrazzo, not a runner.
            K::Runner => Some(Self::Floor(Floor::Terrazzo)),
            K::Desk(_)
            | K::FilingCabinet(_)
            | K::DeskChair(_)
            | K::Station { .. }
            | K::Plant { .. }
            | K::Pod { .. }
            | K::Wall { .. }
            | K::MeetingSofa { .. }
            | K::MeetingTable { .. }
            | K::MeetingChair { .. }
            | K::CoatRack { .. }
            | K::NoticeBoard { .. }
            | K::LoungeCouch
            | K::SideTable
            | K::FloorLamp
            | K::FishTank
            | K::KitchenIsland
            | K::WaterCooler
            | K::TrashBin
            | K::Door
            | K::NeonSign
            | K::Clock => None,
        }
    }
}

/// agent-office: the floors laid under each zone: the benches' carpet, the
/// pantry's mosaic and your office's herringbone.
/// The corridor's terrazzo is its runner's.
fn zone_floors(layout: &SceneLayout) -> impl Iterator<Item = (Bounds, Covering)> + '_ {
    use crate::layout::RoomStyle;
    let rooms = layout.meeting_rooms.iter().filter_map(|r| match r.style {
        RoomStyle::Boss => Some((r.bounds, Covering::Floor(Floor::Herringbone))),
        RoomStyle::Office | RoomStyle::Rooftop => None,
    });
    let benches = Bounds {
        y: layout.wall_band_h(),
        height: (layout.cubicle_band.y + layout.cubicle_band.height)
            .saturating_sub(layout.wall_band_h()),
        ..layout.cubicle_band
    };
    std::iter::once((benches, Covering::Floor(Floor::Carpet)))
        .chain(layout.pantry.map(|p| (p.bounds, Covering::Mosaic)))
        .chain(rooms)
}

impl Backdrop {
    /// `layout`'s backdrop in `theme`'s tones.
    pub(crate) fn of(layout: &SceneLayout, theme: &Theme) -> Self {
        let band_h = layout.wall_band_h();
        let s = &theme.surface;
        Self {
            size: Size {
                w: layout.buf_w,
                h: layout.buf_h,
            },
            wall_band_h: band_h,
            // agent-office: none out over the rooftop.
            posts: layout.window_posts().collect(),
            window_rows: crate::layout::window_rows(band_h),
            trim_row: crate::layout::wall_trim_row(band_h),
            // agent-office: the rooftop's paint and each zone's floor first,
            // under everything.
            coverings: layout
                .rooftop()
                .map(|r| (r.bounds, Covering::RooftopPaint))
                .into_iter()
                .chain(zone_floors(layout))
                .chain(
                    layout
                        .fixtures()
                        .filter_map(|f| Covering::of(f.kind, theme).map(|c| (f.visual, c))),
                )
                .collect(),
            tones: BackdropTones {
                bg: s.bg_fallback,
                wall: s.wall,
                window_frame: s.window_frame,
                wall_trim: s.wall_trim,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::RoomStyle;

    /// agent-office: each zone is laid in a floor of its own, so where an
    /// agent is — at the benches, in the corridor, the pantry, the waiting
    /// area or your office — reads off what it stands on.
    #[test]
    fn each_zone_has_its_own_floor() {
        let l = SceneLayout::compute_with_seed(320, 180, None, 0).expect("fits");
        let b = Backdrop::of(&l, &crate::theme::NORMAL);
        let floor_at = |bounds: Bounds| {
            b.coverings
                .iter()
                .find(|(at, c)| *at == bounds && !matches!(c, Covering::Rug { .. }))
                .map(|(_, c)| *c)
        };
        let room = |style| {
            l.meeting_rooms
                .iter()
                .find(|r| r.style == style)
                .expect("the room")
                .bounds
        };
        assert_eq!(
            floor_at(room(RoomStyle::Boss)),
            Some(Covering::Floor(Floor::Herringbone))
        );
        assert_eq!(
            floor_at(room(RoomStyle::Rooftop)),
            Some(Covering::RooftopPaint)
        );
        assert_eq!(
            floor_at(l.pantry.expect("a pantry").bounds),
            Some(Covering::Mosaic)
        );
        assert!(
            b.coverings
                .iter()
                .any(|(_, c)| *c == Covering::Floor(Floor::Terrazzo)),
            "the corridor's terrazzo"
        );
        let carpet = b
            .coverings
            .iter()
            .find(|(_, c)| *c == Covering::Floor(Floor::Carpet))
            .expect("the benches' carpet")
            .0;
        assert!(
            l.home_desks
                .iter()
                .all(|d| carpet.x <= d.at.x && d.at.y < carpet.y + carpet.height),
            "every desk stands on the carpet"
        );
    }
}
