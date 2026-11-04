//! The pantry aggregate: bounds + the counter size + the island.

use pixtuoid_core::sprite::format::Piece;

use crate::layout::placement::centred;
use crate::layout::{
    Bounds, Facing, Furniture, OBSTACLE_PAD_PX, PANTRY_COUNTER_LARGE_W, Point, Size, WALL_THICK_H,
    Waypoint, WaypointKind, furniture_def, pct,
};

/// The compact counter — the fallback for a pantry too narrow for
/// `LARGE_COUNTER`, and the size consumers read when no pantry exists. The
/// `Furniture::Pantry` row is runtime-sized, so this pair is the counter's only
/// size authority; each is its sprite's size, the ground being
/// `pantry_ground_rect`'s shallow strip.
pub(crate) const COMPACT_COUNTER: Size = Size { w: 20, h: 8 };

/// The detailed kitchen-run counter, for a pantry wide enough to host it.
pub(crate) const LARGE_COUNTER: Size = Size {
    w: PANTRY_COUNTER_LARGE_W,
    h: 10,
};

/// The pantry counter pieces, compact then large.
pub(crate) const PANTRY_COUNTER_ANIMS: [Piece; 2] = [Piece::PantrySmall, Piece::Pantry];

/// The pantry counter piece for a counter `counter_w` px wide: the large
/// kitchen run when the room fits it, else the compact one.
pub(crate) fn pantry_counter_anim(counter_w: u16) -> Piece {
    let [compact, large] = PANTRY_COUNTER_ANIMS;
    if counter_w >= PANTRY_COUNTER_LARGE_W {
        large
    } else {
        compact
    }
}

/// The water cooler's size, [`PantryRoom::water_cooler_rect`]'s box.
pub(crate) const WATER_COOLER: Size = Size { w: 4, h: 9 };

/// The pantry room: its bounds plus what it owns — the counter's chosen
/// footprint and the kitchen-island body centre (`None` when the room can't host
/// it clear of walls + the counter — refuse-don't-force).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PantryRoom {
    /// The pantry room's interior rectangle (buffer pixels).
    pub bounds: Bounds,
    /// Size of the pantry counter sprite: `LARGE_COUNTER` when the pantry is
    /// wide enough, else `COMPACT_COUNTER`. The renderer reads this to
    /// pick which sprite to paint (`pantry` vs `pantry_small`).
    pub(crate) counter_size: Size,
    /// Kitchen-island body centre.
    pub(crate) kitchen_island: Option<Point>,
}

/// Vertical position of the pantry counter as a percent of the room height.
/// SINGLE SOURCE: the island clamp and the counter's own waypoint placement
/// both read it — were they to drift, a clamp would guard a phantom counter
/// position.
pub(crate) fn pantry_counter_y_pct(counter_w: u16) -> u16 {
    if counter_w >= PANTRY_COUNTER_LARGE_W {
        65
    } else {
        60
    }
}

impl PantryRoom {
    /// Absolute y of the counter's blocked centre line inside a room of `bounds`.
    /// THE one derivation the island clamp, the snack-shelf clamp and the
    /// counter's own waypoint all read, so a percent change can't move one and
    /// strand the others.
    pub(crate) fn counter_center_y(bounds: Bounds, counter: Size) -> u16 {
        bounds.y + pct(bounds.height, pantry_counter_y_pct(counter.w))
    }

    /// The counter sprite's top less an [`OBSTACLE_PAD_PX`] margin: the anti-merge ceiling
    /// the island body and the snack shelf sit clear of, not a row the mask blocks.
    pub(crate) fn counter_north(bounds: Bounds, counter: Size) -> u16 {
        Self::counter_center_y(bounds, counter).saturating_sub(counter.h / 2 + OBSTACLE_PAD_PX)
    }

    /// The counter's centre in a room of `bounds`, or `None` for a room
    /// no wider than the counter: refuse rather than force.
    pub(crate) fn counter_center(bounds: Bounds, counter: Size) -> Option<Point> {
        let half_cw = counter.w / 2;
        let max_cx = bounds.x + bounds.width.saturating_sub(half_cw + 1);
        let min_cx = bounds.x + half_cw;
        (min_cx <= max_cx).then(|| Point {
            x: if counter.w >= PANTRY_COUNTER_LARGE_W {
                (bounds.x + bounds.width / 2).clamp(min_cx, max_cx)
            } else {
                (bounds.x + pct(bounds.width, 60)).clamp(min_cx, max_cx)
            },
            // Single-sourced with the island clamp.
            y: Self::counter_center_y(bounds, counter),
        })
    }

    /// The counter's sprite box, centred where [`Self::counter_center`] stands
    /// it.
    pub(crate) fn counter_rect(&self) -> Option<Bounds> {
        Self::counter_center(self.bounds, self.counter_size).map(|c| centred(c, self.counter_size))
    }

    /// `r`, or `r` slid out past the counter's nearer end when the counter's
    /// art would cover it — `None` when that leaves the room.
    fn off_the_counter(&self, r: Bounds) -> Option<Bounds> {
        let Some(counter) = self.counter_rect().filter(|c| c.overlaps(r)) else {
            return Some(r);
        };
        let b = self.bounds;
        // `counter_center` keeps the counter inside the room's columns, so each
        // side's one limit is the floor between its end and the room's edge.
        let x = if r.x + r.width / 2 < counter.x + counter.width / 2 {
            (counter.x - b.x >= r.width).then(|| counter.x - r.width)
        } else {
            let east_end = counter.x + counter.width;
            (b.x + b.width - east_end >= r.width).then_some(east_end)
        };
        x.map(|x| Bounds { x, ..r })
    }

    /// The water cooler's sprite box against the pantry's east side, clear of
    /// the counter, or `None` when the room can't fit it or it gives way
    /// (`clear_of`). THE one authority `paint_water_cooler` AND the hover
    /// roster both read, so the drawn sprite and its hover box can't drift.
    pub(crate) fn water_cooler_rect(&self) -> Option<Bounds> {
        /// Columns between the cooler's east edge and the room's.
        const EAST_GAP: u16 = 3;
        /// Rows from the room's top to just past the cooler's base.
        const BASE_DY: u16 = 15;
        let b = self.bounds;
        // Gated first: the `b.width - …` would `u16`-underflow in a sub-gate room.
        if b.height <= 25 || b.width <= 12 {
            return None;
        }
        self.off_the_counter(Bounds {
            x: b.x + b.width - EAST_GAP - WATER_COOLER.w,
            y: b.y + BASE_DY - WATER_COOLER.h,
            width: WATER_COOLER.w,
            height: WATER_COOLER.h,
        })
        .filter(|&r| clear_of(r, [self.snack_shelf_rect()]))
    }

    /// The trash bin's sprite box by the pantry's west edge, clear of the counter,
    /// or `None` when the room can't fit it or it gives way (`clear_of`). Shared
    /// placement authority for `paint_trash_bin` and the hover hit-test — see
    /// [`Self::water_cooler_rect`].
    pub(crate) fn trash_bin_rect(&self) -> Option<Bounds> {
        let b = self.bounds;
        // Gated first: `b.height - 14` must not run below the gate.
        if b.height <= 20 {
            return None;
        }
        self.off_the_counter(Bounds {
            x: b.x + 3,
            y: b.y + b.height - 14,
            width: 4,
            height: 5,
        })
        .filter(|&r| clear_of(r, [self.snack_shelf_rect(), self.water_cooler_rect()]))
    }

    /// The snack shelf's centre in a room of `bounds`: against the WEST wall,
    /// since the pantry's only wall-free side is the EAST bridge, which must
    /// stay open; `None` for a room too narrow for the shelf plus an east-side
    /// stander cell, or one with no rows clear of the counter's padded north.
    pub(crate) fn snack_shelf_center(bounds: Bounds, counter: Size) -> Option<Point> {
        let vis = furniture_def(Furniture::SnackShelf).visual;
        let (half_w, half_h) = (vis.w / 2, vis.h / 2);
        let clr = WALL_THICK_H + OBSTACLE_PAD_PX;
        let counter_north = Self::counter_north(bounds, counter);
        // Width gate: 1px west margin + the shelf + 3px so the east-side stander has
        // an in-room walkable cell.
        let width_fits = bounds.width >= vis.w + 4;
        let min_y = bounds.y + clr + half_h;
        let max_y = counter_north.saturating_sub(half_h + 1);
        let target = bounds.y + pct(bounds.height, 30);
        (width_fits && min_y <= max_y).then(|| Point {
            x: bounds.x + 1 + half_w,
            y: target.clamp(min_y, max_y),
        })
    }

    /// The snack shelf's sprite box, where [`Self::snack_shelf_center`]
    /// stands it.
    pub(crate) fn snack_shelf_rect(&self) -> Option<Bounds> {
        Self::snack_shelf_center(self.bounds, self.counter_size)
            .map(|c| centred(c, furniture_def(Furniture::SnackShelf).visual))
    }
}

/// Whether `r` overlaps none of `others` that exist: the pantry's small
/// pieces give way, in the order snack shelf, then water cooler, then trash
/// bin, each to those before it — refuse-don't-force.
fn clear_of<const N: usize>(r: Bounds, others: [Option<Bounds>; N]) -> bool {
    others.into_iter().flatten().all(|o| !o.overlaps(r))
}

/// Place the kitchen island in room `pr`: refuse-don't-force with BOTH-axis
/// clamps, staying clear of the counter's padded north
/// ([`PantryRoom::counter_north`], the anti-merge routing constraint). Returns
/// the island body centre, or `None` when the room can't host it clear of walls +
/// counter. On success it ALSO pushes the four `WaypointKind::Island` bartender
/// stand slots (E/W behind the body, two S at the ±w/4 quarter points) onto
/// `waypoints`.
pub(crate) fn place_kitchen_island(
    pr: Bounds,
    counter: Size,
    waypoints: &mut Vec<Waypoint>,
) -> Option<Point> {
    let island = furniture_def(Furniture::KitchenIsland);
    let vis = island.visual;
    let (half_w, half_h) = (vis.w / 2, vis.h / 2);
    let clr = WALL_THICK_H + OBSTACLE_PAD_PX;
    // Stands flank the island 1 walkable cell beyond the body's padded footprint,
    // and must stay in-room too — so the x clamps price the stand extent, not the
    // body.
    let stand_dx = half_w + island.pad + 1;
    let counter_north = PantryRoom::counter_north(pr, counter);
    let min_x = pr.x + clr + stand_dx;
    let max_x = (pr.x + pr.width).saturating_sub(clr + stand_dx);
    // The bartenders' approach lane — the walkable row above the body's padded
    // strip — must be in-room too.
    let min_y = pr.y + clr + half_h + island.pad;
    let max_y = counter_north.saturating_sub(half_h + island.pad + 1);
    if min_x > max_x || min_y > max_y {
        return None;
    }
    let ix = (pr.x + pr.width / 2).clamp(min_x, max_x);
    let iy = (pr.y + pct(pr.height, 40)).clamp(min_y, max_y);
    // Bartender slots sit ON the island's center row at its quarter points, where
    // the sprites can't overlap each other. A BLOCKED pos is fine for an
    // `occupies_pos` slot (the couch-seat pattern): approach_point finds the lane
    // BEHIND the island, the settle glide bridges in, and the island's south-row
    // sort row occludes the standers' legs.
    let bar_dx = (vis.w / 4) as i16;
    for (dx, facing) in [
        (-(stand_dx as i16), Facing::East),
        (stand_dx as i16, Facing::West),
        (-bar_dx, Facing::South),
        (bar_dx, Facing::South),
    ] {
        waypoints.push(Waypoint {
            pos: Point {
                x: ix.saturating_add_signed(dx),
                y: iy,
            },
            kind: WaypointKind::Island,
            facing,
            room_id: None,
        });
    }
    Some(Point { x: ix, y: iy })
}

/// Place the snack shelf in room `pr` where [`PantryRoom::snack_shelf_center`]
/// stands it, pushing its single `WaypointKind::SnackShelf` slot.
pub(crate) fn place_snack_shelf(pr: Bounds, counter: Size, waypoints: &mut Vec<Waypoint>) {
    if let Some(pos) = PantryRoom::snack_shelf_center(pr, counter) {
        waypoints.push(Waypoint {
            pos,
            kind: WaypointKind::SnackShelf,
            facing: Facing::West,
            room_id: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(x: u16, y: u16, width: u16, height: u16) -> Bounds {
        Bounds {
            x,
            y,
            width,
            height,
        }
    }

    /// Every way `off_the_counter` resolves: a piece the counter's art would
    /// cover slides past its nearer end when the floor there takes it, and
    /// stays put when the counter is clear of it or absent. agent-office: on
    /// pantries built here, the south row's pantry no longer being a size an
    /// office width picks.
    #[test]
    fn off_the_counter_resolves_each_way() {
        let pantry = |width| PantryRoom {
            bounds: bx(10, 20, width, 40),
            counter_size: COMPACT_COUNTER,
            kitchen_island: None,
        };
        let piece = Size { w: 4, h: 5 };
        // A room narrower than the counter has none, so nothing moves.
        let narrow = pantry(COMPACT_COUNTER.w - 1);
        assert_eq!(narrow.counter_rect(), None);
        let r = bx(12, 50, piece.w, piece.h);
        assert_eq!(narrow.off_the_counter(r), Some(r));
        let p = pantry(COMPACT_COUNTER.w + 12);
        let c = p.counter_rect().expect("a counter");
        let b = p.bounds;
        for (side, fits) in [(-1i8, true), (-1, false), (1, true), (1, false)] {
            // The floor on that side of the counter exactly the piece's
            // width, or one column short.
            let free = if side < 0 {
                c.x - b.x
            } else {
                b.x + b.width - (c.x + c.width)
            };
            let w = if fits { free } else { free + 1 };
            let at = if side < 0 { c.x } else { c.x + c.width - w };
            let r = bx(at, c.y, w, piece.h);
            let want = fits.then(|| Bounds {
                x: if side < 0 { c.x - w } else { c.x + c.width },
                ..r
            });
            assert_eq!(p.off_the_counter(r), want, "side {side} fits {fits}: {c:?}");
            // Clear of it, it stays.
            let clear = Bounds {
                y: c.y + c.height,
                ..r
            };
            assert_eq!(p.off_the_counter(clear), Some(clear));
        }
    }

    /// The premise of `off_the_counter`'s one check per side, and the bin that
    /// check keeps inside a room off the office's west edge.
    #[test]
    fn the_counter_and_the_bin_slid_off_it_keep_to_the_rooms_columns() {
        for counter_size in [COMPACT_COUNTER, LARGE_COUNTER] {
            for x in [0, 7] {
                for width in 0..=2 * LARGE_COUNTER.w {
                    let p = PantryRoom {
                        bounds: bx(x, 0, width, 40),
                        counter_size,
                        kitchen_island: None,
                    };
                    let counter = p.counter_rect();
                    assert_eq!(counter.is_some(), width > counter_size.w, "{:?}", p.bounds);
                    let Some(c) = counter else { continue };
                    let in_columns = |b: Bounds| x <= b.x && b.x + b.width <= x + width;
                    assert!(in_columns(c), "{c:?} outside {:?}", p.bounds);
                    if let Some(bin) = p.trash_bin_rect() {
                        assert!(
                            in_columns(bin) && !bin.overlaps(c),
                            "{bin:?} vs {c:?} in {:?}",
                            p.bounds
                        );
                    }
                }
            }
        }
    }
}
