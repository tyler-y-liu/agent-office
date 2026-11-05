//! `character_frame` is the per-agent recolor both profiles draw through.

use std::time::SystemTime;

use crate::pack::OfficeArt;
use pixtuoid_core::AgentSlot;
use pixtuoid_core::sprite::format::Piece;
use pixtuoid_core::sprite::{Frame, Rgb};

use crate::frame_cache::FrameCache;
use crate::sim::{CharacterGlow, outfit_seed_for};

mod colors;
mod hair;
#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
pub(crate) use colors::{HAIR_KEY, PANTS_KEY, SHIRT_KEY, SKIN_KEY};
pub(crate) use colors::{agent_overrides, work_glow_tint};
#[cfg(all(test, feature = "cutaway-assets"))]
pub(crate) use hair::dress;
pub(crate) use hair::{Dress, dress_for};

/// Which image of a character's art to draw: the part of its
/// [`FrameKey`](crate::frame_cache::FrameKey) the sim's placement decides; the
/// agent, its burn tier and the scale's density key the rest. A request, not
/// the cache's key, which is published and keys the image it made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpritePose {
    pub(crate) anim_name: Piece,
    pub(crate) frame_idx: usize,
    /// Which way the character FACES: the art mirrored horizontally.
    pub(crate) flip_x: bool,
    /// The monitor glow blended into the skin by [`agent_overrides`], so a row
    /// of typing agents shows their tools at a glance; `None` unless typing at
    /// a desk.
    pub(crate) glow_tint: Option<Rgb>,
}

impl SpritePose {
    /// The pose `placement` draws `agent` in under `theme`. The ONE resolution
    /// of the sim's theme-free [`CharacterGlow`], shared by both profiles: were
    /// each to resolve it, the same agent would glow differently in the two for
    /// no reason anyone chose.
    pub(crate) fn of(
        placement: &crate::sim::CharacterPlacement,
        agent: &AgentSlot,
        theme: &crate::theme::Theme,
    ) -> Self {
        Self {
            anim_name: placement.anim_name,
            frame_idx: placement.frame_idx,
            flip_x: placement.flip_x,
            glow_tint: match placement.glow {
                CharacterGlow::None => None,
                CharacterGlow::Work => work_glow_tint(agent, &theme.tool_glow),
            },
        }
    }
}

/// The per-agent RECOLORED sprite for one character, and where the art marks a
/// head, DRESSED, from the cache.
///
/// One fn so a second profile gets the identical palette without a second
/// copy of the rule. The ART and the BLIT differ
/// between profiles — the art is [`densest_frame`](crate::pack::densest_frame)'s at
/// `scale`, so the classic pass (at `RenderScale::ONE`) draws the base sprite
/// 1:1 and the cutaway the densest variant its scale lands — and a per-agent
/// palette is exactly the thing that must NOT differ: hair, skin and the
/// cwd-keyed outfit are how a viewer tells two agents apart, so an agent who is
/// auburn in one profile and default-brown in the other is two different
/// people to the eye.
pub(crate) fn character_frame<'c>(
    pose: SpritePose,
    agent: &AgentSlot,
    pack: &OfficeArt,
    scale: crate::render_scale::RenderScale,
    cache: &'c mut FrameCache,
    now: SystemTime,
) -> CharacterFrame<'c> {
    let dense = crate::pack::densest_frame(pack, pose.anim_name, pose.frame_idx, scale);
    let key = character_key_at(&dense, pack, pose, agent, now);
    recolor(dense, &key, pack, cache)
}

/// agent-office: `agent`'s head as the office draws it standing, recoloured and
/// dressed in its hairstyle, cropped to the head and its hair: the face the
/// panel's card wears, so a card and its figure are seen to be one agent. Drawn
/// from the densest art at `scale`; the whole frame's top half where the art
/// marks no head.
pub fn portrait(
    agent: &AgentSlot,
    pack: &OfficeArt,
    scale: crate::render_scale::RenderScale,
    now: SystemTime,
) -> Frame {
    let pose = SpritePose {
        anim_name: Piece::Standing,
        frame_idx: 0,
        flip_x: false,
        glow_tint: None,
    };
    let mut cache = FrameCache::new();
    let head = crate::pack::densest_frame(pack, Piece::Standing, 0, scale).head;
    let art = character_frame(pose, agent, pack, scale, &mut cache, now);
    let f = art.frame;
    // The mark sits mid-face: as far again below it is the chin.
    let bottom = head.map_or(f.height() / 2, |h| art.rise + h.y() * 2 + 1);
    let bottom = bottom.min(f.height());
    let opaque = |x: u16, y: u16| f.get(x, y).copied().flatten().is_some();
    let cols: Vec<u16> = (0..f.width())
        .filter(|&x| (0..bottom).any(|y| opaque(x, y)))
        .collect();
    let top = (0..bottom)
        .find(|&y| (0..f.width()).any(|x| opaque(x, y)))
        .unwrap_or(0);
    let (Some(&x0), Some(&x1)) = (cols.first(), cols.last()) else {
        return Frame::from_pixels(0, 0, Vec::new());
    };
    let (w, h) = (x1 - x0 + 1, bottom - top);
    let pixels = (top..bottom)
        .flat_map(|y| (x0..=x1).map(move |x| (x, y)))
        .map(|(x, y)| f.get(x, y).copied().flatten())
        .collect();
    Frame::from_pixels(w, h, pixels)
}

/// [`character_frame`] for a [`CharacterKey`] resolved at `scale`.
pub(crate) fn keyed_character_frame<'c>(
    key: &CharacterKey,
    pack: &OfficeArt,
    scale: crate::render_scale::RenderScale,
    cache: &'c mut FrameCache,
) -> CharacterFrame<'c> {
    let dense = crate::pack::densest_frame(pack, key.frame.anim_name, key.frame.frame_idx, scale);
    recolor(dense, key, pack, cache)
}

fn recolor<'c>(
    dense: crate::pack::DenseFrame<'_>,
    key: &CharacterKey,
    pack: &OfficeArt,
    cache: &'c mut FrameCache,
) -> CharacterFrame<'c> {
    let flip_x = key.frame.flip_x;
    // A cwd backfill re-keys the outfit (Team Palette) mid-lifetime — flag the
    // change so the cache drops the agent's stale recolors before the lookup.
    cache.note_outfit_seed(key.frame.agent_id, key.outfit);
    let frame = cache.get_or_make(key.frame.clone(), || {
        let bare = dense.recolorable.recolored(&key.palette);
        // Dressed before the facing flip: a style's layers are drawn for the
        // art as authored. The agent id and the art's density pick the style,
        // and the cache's key carries both.
        let recolored = match &key.dress {
            Some(dress) => hair::dress(
                &bare,
                dress,
                dress
                    .style
                    .as_deref()
                    .and_then(|name| pack.hairstyle(name, dense.density)),
                &key.palette,
                pack.character_outline(),
            ),
            None => bare,
        };
        if flip_x {
            recolored.mirror_horizontal()
        } else {
            recolored
        }
    });
    let rise = key.dress.as_ref().map_or(0, |d| d.rise());
    CharacterFrame {
        frame,
        blit_at: dense.blit_at,
        rise,
    }
}

/// Every input [`character_frame`] recolors from: the cache's key, the outfit
/// seed the cache drops an agent's entries on, and the palette itself. Within
/// one pack, two equal keys recolor to the same frame.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CharacterKey {
    pub(crate) frame: crate::frame_cache::FrameKey,
    pub(crate) outfit: u64,
    /// The agent's colours, resolved: the recolor reads nothing of the agent
    /// beyond what this and `frame` carry.
    palette: [(char, pixtuoid_core::sprite::Pixel); 4],
    /// How the frame is dressed, resolved once for every reader: the recolor
    /// and the cutaway's figure box.
    pub(crate) dress: Option<Dress>,
}

/// [`character_frame`]'s [`CharacterKey`], without the recolor.
pub(crate) fn character_key(
    pose: SpritePose,
    agent: &AgentSlot,
    pack: &OfficeArt,
    scale: crate::render_scale::RenderScale,
    now: SystemTime,
) -> CharacterKey {
    let dense = crate::pack::densest_frame(pack, pose.anim_name, pose.frame_idx, scale);
    character_key_at(&dense, pack, pose, agent, now)
}

fn character_key_at(
    dense: &crate::pack::DenseFrame<'_>,
    pack: &OfficeArt,
    pose: SpritePose,
    agent: &AgentSlot,
    now: SystemTime,
) -> CharacterKey {
    let SpritePose {
        anim_name,
        frame_idx,
        flip_x,
        glow_tint,
    } = pose;
    let burn = crate::burn::slot_burn_tier(agent, now);
    let density = dense.density;
    CharacterKey {
        frame: crate::frame_cache::FrameKey {
            agent_id: agent.agent_id,
            anim_name,
            frame_idx,
            flip_x,
            glow_tint,
            burn,
            density,
        },
        outfit: outfit_seed_for(agent),
        palette: agent_overrides(agent, glow_tint, burn),
        dress: dress_for(pack, agent.agent_id, dense.frame, dense.head, density),
    }
}

/// A recolored character frame and how to draw it at the scale it was picked
/// for; `blit_at` as in [`DenseFrame`](crate::pack::DenseFrame).
pub(crate) struct CharacterFrame<'c> {
    pub(crate) frame: &'c Frame,
    pub(crate) blit_at: std::num::NonZeroU16,
    /// Art rows the frame reaches above its logical top: its
    /// [`Dress::rise`].
    pub(crate) rise: u16,
}

#[cfg(all(test, feature = "cutaway-assets"))]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// agent-office: the panel's card wears its agent's head, so the card
    /// matches the figure: cropped above the shoulders, wearing the agent's
    /// own hair, and so unlike a coworker's.
    #[test]
    fn a_portrait_is_the_agents_own_head() {
        let pack = crate::pack::test_office();
        let scale = crate::render_scale::RenderScale::new(4).expect("4x");
        let now = SystemTime::UNIX_EPOCH;
        let a = test_support::make_slot_cwd("/a.jsonl", "/demo/api", false);
        let b = test_support::make_slot_cwd("/zz.jsonl", "/demo/web", false);
        let pa = portrait(&a, &pack, scale, now);
        let body = crate::pack::densest_frame(&pack, Piece::Standing, 0, scale);
        assert!(
            pa.height() > 0 && pa.height() < body.frame.height() * 2 / 3,
            "cropped to the head"
        );
        let hair = test_support::color_of(&a, HAIR_KEY);
        let has = |f: &Frame, c| {
            (0..f.height()).any(|y| (0..f.width()).any(|x| f.get(x, y).copied().flatten() == c))
        };
        assert!(has(&pa, hair), "wears its own hair colour");
        let px = |f: &Frame| -> Vec<Option<Rgb>> {
            (0..f.height())
                .flat_map(|y| (0..f.width()).map(move |x| (x, y)))
                .map(|(x, y)| f.get(x, y).copied().flatten())
                .collect()
        };
        assert_ne!(px(&pa), px(&portrait(&b, &pack, scale, now)));
    }

    /// The README banner's coworker: whose transcript and working directory
    /// pick their outfit, hair, skin and hairstyle, as any agent's do.
    const BANNER_AGENT: (&str, &str) = ("/h.jsonl", "/tmp/lab");

    /// `art`'s frames as text, every colour named by one key in `@palette`.
    fn golden(art: &[(&str, u32, &Frame)]) -> String {
        const KEYS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let (mut keys, mut frames) = (Vec::<Rgb>::new(), String::new());
        for (name, ms, f) in art {
            writeln!(frames, "@frame {name} {ms}").expect("a String takes every write");
            for y in 0..f.height() {
                let row: Vec<String> = (0..f.width())
                    .map(|x| match f.get(x, y).copied().flatten() {
                        None => ".".to_owned(),
                        Some(c) => {
                            let i = keys.iter().position(|&k| k == c).unwrap_or_else(|| {
                                keys.push(c);
                                keys.len() - 1
                            });
                            KEYS.chars().nth(i).expect("a key per colour").to_string()
                        }
                    })
                    .collect();
                writeln!(frames, "{}", row.join(" ")).expect("a String takes every write");
            }
        }
        let palette: Vec<String> = keys
            .iter()
            .zip(KEYS.chars())
            .map(|(c, k)| format!("{k}=#{:02x}{:02x}{:02x}", c.r, c.g, c.b))
            .collect();
        format!("@palette {}\n{frames}", palette.join(" "))
    }

    /// The banner art for `agent`: their coworker holding a coffee as
    /// [`character_frame`] draws them at the pack's densest art, and the
    /// sleeping cat at the same density.
    fn banner_art(pack: &OfficeArt, agent: &AgentSlot) -> String {
        let density = pack.max_density_variant();
        let scale = crate::render_scale::RenderScale::new(density.get()).expect("nonzero");
        let mut cache = FrameCache::new();
        let pose = SpritePose {
            anim_name: Piece::HoldingCoffee,
            frame_idx: 0,
            flip_x: false,
            glow_tint: None,
        };
        let coffee = pack.variants_of(Piece::HoldingCoffee)[&density].frame_ms();
        let coworker =
            character_frame(pose, agent, pack, scale, &mut cache, SystemTime::UNIX_EPOCH)
                .frame
                .clone();
        let cat = &pack.variants_of(Piece::CatSleep)[&density];
        let art: Vec<_> = std::iter::once(("coworker", coffee, &coworker))
            .chain(cat.frames().iter().map(|f| ("cat", cat.frame_ms(), f)))
            .collect();
        golden(&art)
    }

    /// `banner.golden` is the README banner's cutaway art as the office draws
    /// it, so `scripts/gen-banner.py`, which only lays it out, cannot drift from
    /// the pack, the recolor or the dressing. `just gen-banner` rewrites it.
    #[test]
    fn the_readme_banner_art_is_the_office_s_own() {
        const BANNER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/character/banner.golden");
        let (transcript, cwd) = BANNER_AGENT;
        let agent = test_support::make_slot_cwd(transcript, cwd, false);
        let pinned = snapbox::Data::read_from(
            std::path::Path::new(BANNER),
            Some(snapbox::data::DataFormat::Text),
        );
        snapbox::assert_data_eq!(
            banner_art(&crate::pack::test_office(), &agent),
            pinned.raw()
        );
    }
}
