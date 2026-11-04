//! agent-office: the city across Victoria Harbour. The office looks north over
//! the water: the harbour fills the foot of the glass, the far shore's towers
//! stand along it with the ICC rising over them, the Kowloon hills with Lion
//! Rock's head behind, a Star Ferry crossing, and at eight each evening the
//! light show's beams over the roofs.
//!
//! Everything here draws onto the [`CityStrip`], so the sky, the clouds and
//! the weather treat it as they treat any building.

use pixtuoid_core::sprite::Rgb;

use super::{CityStrip, Plane, PlaneColours, hash};
use crate::atmosphere::SkyTones;
use crate::layout::pct;

/// The share of the glass, in percent, the harbour's water fills at its foot.
const WATER_PCT: u16 = 22;

/// The rows of water at the foot of glass `glass_h` tall: the shore the far
/// side's buildings stand on is this far above the sill.
pub(super) fn water_rows(glass_h: u16) -> u16 {
    pct(glass_h, WATER_PCT).max(2).min(glass_h / 2)
}

/// The harbour by day and by night, before the sky's light mixes in.
const WATER_DAY: Rgb = Rgb {
    r: 52,
    g: 104,
    b: 122,
};
const WATER_NIGHT: Rgb = Rgb {
    r: 9,
    g: 20,
    b: 38,
};
/// How much of the sky's horizon the water takes on.
const WATER_SKY: f32 = 0.2;
/// How strongly the far shore shows in the water at the shoreline, fading
/// down toward the sill.
const REFLECT: f32 = 0.5;

/// The ICC: about where along the run it stands, as a share of the run (it
/// moves to the middle of the nearest pane), and how wide, in logical units.
const ICC_AT_PCT: u16 = 60;
const ICC_W: u16 = 5;

/// Lion Rock's head: where along the run, as a share of it.
const LION_AT_PCT: u16 = 28;

/// A Star Ferry's crossing, one way, in milliseconds; it turns and comes back.
const FERRY_CROSSING_MS: u64 = 90_000;
const FERRY_HULL: Rgb = Rgb {
    r: 34,
    g: 92,
    b: 60,
};
const FERRY_DECK: Rgb = Rgb {
    r: 232,
    g: 228,
    b: 214,
};
const FERRY_LIGHT: Rgb = Rgb {
    r: 255,
    g: 214,
    b: 130,
};

/// The light show's beams, in turn.
const BEAMS: [Rgb; 4] = [
    Rgb {
        r: 80,
        g: 255,
        b: 140,
    },
    Rgb {
        r: 90,
        g: 220,
        b: 255,
    },
    Rgb {
        r: 255,
        g: 90,
        b: 200,
    },
    Rgb {
        r: 240,
        g: 240,
        b: 255,
    },
];
/// The roofs, as shares of the run, the beams rise from.
const BEAM_ROOTS_PCT: [u16; 5] = [12, 30, 47, 78, 92];

/// What the harbour adds to `strip`, a run `run_w` units wide behind glass
/// `glass_h` tall at `d` art pixels to the unit.
pub(super) struct Harbour<'a> {
    pub(super) run_w: u16,
    pub(super) glass_h: u16,
    pub(super) d: u16,
    pub(super) look: &'a SkyTones,
    pub(super) colours: &'a [PlaneColours; 3],
    pub(super) beat: crate::anim::Beat,
    /// How far into the light show the clock is, or `None`.
    pub(super) show: Option<f32>,
    /// The run's glass, in units from its west end; empty when unknown.
    pub(super) panes: &'a [std::ops::Range<u16>],
}

impl Harbour<'_> {
    /// The ICC's west edge, in units: centred in the pane nearest its place,
    /// so no post hides it.
    fn icc_x(&self) -> u16 {
        let aim = pct(self.run_w, ICC_AT_PCT);
        self.panes
            .iter()
            .filter(|p| p.end - p.start >= ICC_W + 2)
            .map(|p| (p.start + p.end) / 2)
            .min_by_key(|c| c.abs_diff(aim))
            .unwrap_or(aim)
            .saturating_sub(ICC_W / 2)
    }

    fn shore_px(&self) -> i32 {
        i32::from(self.glass_h - water_rows(self.glass_h)) * i32::from(self.d)
    }

    /// How tall the far plane may stand, in art pixels: the ICC's height.
    fn far_band_px(&self) -> i32 {
        i32::from(Plane::Far.band(self.glass_h, 0.0)) * i32::from(self.d)
    }

    /// The ICC, drawn before the far shore's buildings so they stand in
    /// front of its foot.
    pub(super) fn icc(&self, strip: &mut CityStrip) {
        let d = i32::from(self.d);
        let c = &self.colours[Plane::Mid.index()];
        let x0 = i32::from(self.icc_x()) * d;
        let w = i32::from(ICC_W) * d;
        let top = self.shore_px() - self.far_band_px();
        let lit = self.look.darkness > 0.3;
        for y in top..self.shore_px() {
            // Its crown steps in twice near the top.
            let inset = match y - top {
                r if r < d => w / 3,
                r if r < 3 * d => w / 6,
                _ => 0,
            };
            for x in x0 + inset..x0 + w - inset {
                let colour = if lit {
                    self.icc_facade(x - x0, y - top)
                } else if (x - x0) % 2 == 1 {
                    c.material(super::Material::Glass)
                } else {
                    c.material(super::Material::Facade)
                };
                strip.put(x, y, colour);
            }
        }
    }

    /// The ICC's lit facade: its whole face is a screen, a band of colour
    /// climbing it, faster while the show runs.
    fn icc_facade(&self, x: i32, y: i32) -> Rgb {
        let c = &self.colours[Plane::Mid.index()];
        if x % 2 == 0 {
            return c.material(super::Material::Mullion);
        }
        let speed = if self.show.is_some() { 60 } else { 240 };
        let phase = (self.beat.ms() / speed) as i32;
        let band = (y + phase).rem_euclid(24) / 6;
        let hue = BEAMS[band as usize % BEAMS.len()];
        c.material(super::Material::Glass).mix(hue, 0.55)
    }

    /// The Kowloon hills behind every building: only where nothing else
    /// stands yet.
    pub(super) fn ridge(&self, strip: &mut CityStrip) {
        let d = f32::from(self.d);
        let shore = self.shore_px();
        let band = self.far_band_px() as f32;
        let far = &self.colours[Plane::Far.index()];
        // The hills read as a silhouette against the sky: darker than it by
        // night, a hazed green-grey by day.
        let face = far.material(super::Material::Shade).ramp(-2);
        let dusk = 1.0 - self.look.darkness * 0.7;
        let rock = face.mix(self.look.glass_horizon, 0.25 * dusk);
        let shade = face.mix(self.look.glass_horizon, 0.1 * dusk).ramp(-1);
        let lion = f32::from(pct(self.run_w, LION_AT_PCT));
        for x in 0..i32::from(strip.w) {
            let t = x as f32 / d;
            let roll = 0.6 + 0.08 * (t / 21.0).sin() + 0.05 * (t / 8.3 + 1.3).sin();
            // The lion lies facing west: a steep brow over a long back.
            let from_head = t - lion;
            let head = if from_head < 0.0 {
                (1.0 + from_head / 5.0).max(0.0)
            } else {
                (1.0 - from_head / 22.0).max(0.0)
            };
            let h = (band * (roll + 0.3 * head.powf(1.5))).min(band * 0.95) as i32;
            for y in (shore - h).max(0)..shore {
                let colour = if y - (shore - h) < self.d.into() {
                    rock
                } else {
                    shade
                };
                strip.put_behind(x, y, colour);
            }
        }
    }

    /// The water, the far shore mirrored in it, the promenade's lamps along
    /// the shore, and glints off the swell.
    pub(super) fn water(&self, strip: &mut CityStrip) {
        let shore = self.shore_px();
        let foot = i32::from(strip.h);
        let depth = (foot - shore).max(1);
        let base = WATER_DAY
            .mix(WATER_NIGHT, self.look.darkness)
            .mix(self.look.glass_horizon, WATER_SKY);
        let swell = self.beat.ms() / 250;
        for y in shore..foot {
            let k = y - shore;
            let fade = 1.0 - k as f32 / depth as f32;
            let water = base.ramp(-((k * 2 / depth) as i8));
            // Every other pair of rows the swell shifts the reflection a
            // pixel, back and forth with the beat.
            let ripple = if ((y as u64 + swell) / 2).is_multiple_of(2) {
                1
            } else {
                -1
            };
            for x in 0..i32::from(strip.w) {
                let mirrored = strip.at_i(x + ripple, shore - 1 - k);
                let mut colour = match mirrored {
                    Some(c) => water.mix(c, REFLECT * fade),
                    None => water,
                };
                let glint = hash((x as u32) ^ (y as u32).wrapping_mul(977) ^ (swell / 3) as u32);
                if glint.is_multiple_of(53) {
                    colour = colour.ramp(2);
                }
                strip.put(x, y, colour);
            }
        }
        // The promenade: a lamp every few pixels along the shore, lit after dark.
        if self.look.darkness > 0.2 {
            let lamp = FERRY_LIGHT.mix(base, 1.0 - self.look.darkness);
            for x in (0..i32::from(strip.w)).step_by(3) {
                strip.put(x, shore, lamp);
            }
        }
    }

    /// A Star Ferry, green under white, crossing the water and back.
    pub(super) fn ferry(&self, strip: &mut CityStrip) {
        let d = i32::from(self.d);
        let (len, hull, deck) = (8 * d, d.max(2), d.max(2));
        let span = i32::from(strip.w) + len;
        let leg = self.beat.ms() % (2 * FERRY_CROSSING_MS);
        let along = (leg % FERRY_CROSSING_MS) as f32 / FERRY_CROSSING_MS as f32;
        let eastward = leg < FERRY_CROSSING_MS;
        let run = (along * span as f32) as i32;
        let x0 = if eastward { run - len } else { span - run - len };
        let water = i32::from(strip.h) - self.shore_px();
        let waterline = self.shore_px() + water * 3 / 5;
        let lit = self.look.darkness > 0.3;
        for x in x0..x0 + len {
            for y in waterline - hull..waterline {
                strip.put(x, y, FERRY_HULL);
            }
            // The deckhouse, short of the bow and the stern.
            if x >= x0 + d && x < x0 + len - d {
                for y in waterline - hull - deck..waterline - hull {
                    let window = lit && (x - x0) % 2 == 0 && y == waterline - hull - deck / 2;
                    strip.put(x, y, if window { FERRY_LIGHT } else { FERRY_DECK });
                }
            }
        }
        // Its wake: a lighter line of water astern.
        let stern = if eastward { x0 - 1 } else { x0 + len };
        let step = if eastward { -1 } else { 1 };
        for i in 0..2 * d {
            let x = stern + step * i;
            if let Some(c) = strip.at_i(x, waterline - 1) {
                strip.put(x, waterline - 1, c.ramp(2));
            }
        }
    }

    /// The light show: beams from the roofs sweeping the sky, only where the
    /// sky shows, so the towers stand in front of them.
    pub(super) fn beams(&self, strip: &mut CityStrip) {
        let Some(progress) = self.show else { return };
        let d = i32::from(self.d);
        // f64: an epoch-scale beat in f32 steps in minutes, freezing the sweep.
        let secs = self.beat.ms() as f64 / 1000.0;
        // One more from the ICC's crown.
        let icc = self.icc_x() + ICC_W / 2;
        let roots = BEAM_ROOTS_PCT.iter().map(|at| pct(self.run_w, *at));
        for (i, at) in roots.chain([icc]).enumerate() {
            let x0 = i32::from(at) * d;
            // Each sweeps its own arc, out of step with its neighbours, and
            // the colours change hands through the show.
            let sway = (secs / 1.4 + i as f64 * 1.7).sin() as f32;
            let lean = sway * 0.6;
            // From the roof under the root: the first building down the column.
            let roof = (0..self.shore_px())
                .find(|&y| strip.at_i(x0, y).is_some())
                .unwrap_or(self.shore_px())
                - 1;
            let hue = BEAMS[(i + (progress * 12.0) as usize) % BEAMS.len()];
            let mut y = roof;
            let mut x = x0 as f32;
            let thick = (d / 2).max(1);
            while y >= 0 {
                for t in 0..thick {
                    strip.put_behind(x.round() as i32 + t, y, hue);
                }
                y -= 1;
                x += lean;
            }
        }
    }
}

impl CityStrip {
    /// `colour` at `(x, y)` only where the sky still shows: what stands
    /// behind everything drawn so far.
    fn put_behind(&mut self, x: i32, y: i32, colour: Rgb) {
        if let (Ok(ux), Ok(uy)) = (u16::try_from(x), u16::try_from(y))
            && self.at(ux, uy).is_none()
        {
            self.put(x, y, colour);
        }
    }

    /// What stands at `(x, y)`, signed, `None` off the strip or on sky.
    fn at_i(&self, x: i32, y: i32) -> Option<Rgb> {
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
            return None;
        };
        self.at(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Motion;
    use crate::atmosphere::Moment;
    use crate::display::pen::test_density;
    use std::ops::Range;
    use std::time::UNIX_EPOCH;

    const RUN: (u16, u16) = (120, 30);

    fn strip(d: u16, panes: &[Range<u16>]) -> CityStrip {
        let pack = crate::pack::test_office();
        let theme = crate::theme::theme_by_name("harbour").expect("theme");
        let sky = crate::sky::Sky::clock(UNIX_EPOCH);
        let outlook =
            Moment::resolve(sky, theme, 0.0, Motion::Full.timing(UNIX_EPOCH)).outlook(theme);
        CityStrip::draw_through(&pack, RUN, &outlook, theme, test_density(d), panes)
    }

    /// The water is the foot of every window: below the shore no sky shows,
    /// at any density.
    #[test]
    fn the_harbour_fills_the_foot_of_the_glass() {
        for d in [1, 4] {
            let s = strip(d, &[]);
            let shore = (RUN.1 - water_rows(RUN.1)) * d;
            for y in shore..s.h {
                for x in 0..s.w {
                    assert!(s.at(x, y).is_some(), "sky at ({x}, {y}) at {d}x");
                }
            }
        }
    }

    /// The ICC is the one landmark: a post between panes must not hide it.
    #[test]
    fn the_icc_stands_in_the_pane_nearest_its_place() {
        let pack = crate::pack::test_office();
        let theme = crate::theme::theme_by_name("harbour").expect("theme");
        let sky = crate::sky::Sky::clock(UNIX_EPOCH);
        let outlook =
            Moment::resolve(sky, theme, 0.0, Motion::Full.timing(UNIX_EPOCH)).outlook(theme);
        let colours = Plane::ALL.map(|p| PlaneColours::of(p, outlook.look(), theme));
        let _ = pack;
        let panes = [0..50, 54..70, 74..120];
        let harbour = Harbour {
            run_w: RUN.0,
            glass_h: RUN.1,
            d: 1,
            look: outlook.look(),
            colours: &colours,
            beat: outlook.beat(),
            show: None,
            panes: &panes,
        };
        let x = harbour.icc_x();
        assert!(x >= 54 && x + ICC_W <= 70, "the ICC at {x} sits in 54..70");
    }

    /// A Star Ferry crosses on the water, never over the far shore.
    #[test]
    fn the_ferry_keeps_to_the_water() {
        let theme = crate::theme::theme_by_name("harbour").expect("theme");
        let sky = crate::sky::Sky::clock(UNIX_EPOCH);
        let colours_for = |ms: u64| {
            let at = UNIX_EPOCH + std::time::Duration::from_millis(ms);
            Moment::resolve(sky, theme, 0.0, Motion::Full.timing(at)).outlook(theme)
        };
        for ms in [0, 30_000, 60_000, 120_000] {
            let outlook = colours_for(ms);
            let colours = Plane::ALL.map(|p| PlaneColours::of(p, outlook.look(), theme));
            let harbour = Harbour {
                run_w: RUN.0,
                glass_h: RUN.1,
                d: 4,
                look: outlook.look(),
                colours: &colours,
                beat: outlook.beat(),
                show: None,
                panes: &[],
            };
            let mut s = CityStrip {
                w: RUN.0 * 4,
                h: RUN.1 * 4,
                px: vec![None; usize::from(RUN.0 * 4) * usize::from(RUN.1 * 4)],
            };
            harbour.ferry(&mut s);
            let shore = harbour.shore_px();
            for y in 0..i32::from(s.h) {
                for x in 0..i32::from(s.w) {
                    if s.at_i(x, y).is_some() {
                        assert!(y >= shore - 2 * 4, "the ferry at ({x}, {y}) rides the water");
                    }
                }
            }
        }
    }
}
