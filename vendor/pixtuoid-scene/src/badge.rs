//! A name badge's words and colours: its agent's disambiguated, truncated
//! name, its activity tone, its inks and its plate, which
//! [`Badge`](crate::display::Badge) puts together for every painter.

use std::collections::HashMap;

use pixtuoid_core::AgentSlot;
use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::ActivityState;

use crate::layout::DESK_W;
use crate::theme::Theme;

/// The separator between a label's source prefix and its cwd/disambiguation
/// tail (`cc·repo`, `cc·repo·1a2b`). The label is WRITTEN core-side as a bare
/// `·` at THREE sites — `decoder::cwd_basename_label` (the chokepoint),
/// `claude_code::cc_derive_label`'s project-dir fallback, and the reducer's
/// SessionStart back-fill, both of which document their bypass — and a crate
/// boundary keeps this const out of reach of all three, so it must MATCH that
/// char.
const LABEL_SEP: char = '\u{b7}';

/// Activity-derived label tone — backend-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BadgeTone {
    Running,
    RequiresAction,
    Idle,
    Exiting,
}

impl BadgeTone {
    /// This tone's theme color role — the SINGLE authority every label painter
    /// shares. The `hovered` near-white highlight is deliberately NOT a
    /// `BadgeTone`; it stays a per-painter surface choice.
    pub(crate) fn rgb(self, theme: &Theme) -> Rgb {
        match self {
            BadgeTone::Exiting => theme.ui.label_exiting,
            BadgeTone::Running => theme.ui.label_running,
            BadgeTone::RequiresAction => theme.ui.label_requires_action,
            BadgeTone::Idle => theme.ui.label_idle,
        }
    }
}

/// The source's badge hue for a name-badge label, or `None` when the label has
/// no `LABEL_SEP` prefix (a bare-prefix / cwd-less label like `cx`) or the
/// prefix is unregistered.
pub(crate) fn badge_hue(text: &str, theme: &Theme) -> Option<Rgb> {
    text.split_once(LABEL_SEP)
        .and_then(|(prefix, _)| theme.source.by_prefix(prefix))
}

/// agent-office: the colours a repository can wear, apart on a dark plate
/// and from each other, and clear of the panel's state colours (stuck red,
/// waiting amber, working green, done ice blue): red, amber and green here
/// read as their agents being stuck, waiting or working. A badge's marker
/// and a card's avatar are graphics, so each holds 3:1 on the dark plate.
/// The first few, which a small office wears, are the furthest apart.
/// Public, so the host checks them against its state colours and each other.
pub const REPO_HUES: [Rgb; 8] = [
    Rgb {
        r: 0x4c,
        g: 0xe1,
        b: 0xcd,
    },
    Rgb {
        r: 0x00,
        g: 0x8b,
        b: 0xff,
    },
    Rgb {
        r: 0x99,
        g: 0x66,
        b: 0xd7,
    },
    Rgb {
        r: 0xff,
        g: 0xb2,
        b: 0xfa,
    },
    Rgb {
        r: 0x18,
        g: 0x85,
        b: 0x81,
    },
    Rgb {
        r: 0x1b,
        g: 0x7e,
        b: 0xb5,
    },
    Rgb {
        r: 0xcf,
        g: 0x44,
        b: 0xa1,
    },
    Rgb {
        r: 0x92,
        g: 0x6c,
        b: 0x9a,
    },
];

// agent-office: core deals the indices this palette is read with.
const _: () = assert!(REPO_HUES.len() == pixtuoid_core::state::REPO_HUE_COUNT as usize);

/// agent-office: `agent`'s repository's colour — its badges' marker and
/// its cards' avatar, so they read as one project. Core
/// deals it ([`SceneState::assign_repo_hues`]); `None` while the agent's
/// directory is unknown.
///
/// [`SceneState::assign_repo_hues`]: pixtuoid_core::state::SceneState::assign_repo_hues
pub fn repo_hue(agent: &AgentSlot) -> Option<Rgb> {
    agent.repo_hue.map(|i| REPO_HUES[usize::from(i)])
}

/// The colours of a badge's two parts, the one decision every painter reads:
/// the name in the activity tone, which every theme holds at text contrast,
/// and the source's identity on the marker, a graphic, since a brand hue as
/// text can fall below that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeInk {
    /// The source's badge hue, else the tone: the terminal's leading `●`,
    /// the pixel painters' strip.
    pub marker: Rgb,
    /// The label text: the tone.
    pub name: Rgb,
}

pub use crate::display::BADGE_MARKER;

// A decoded label ends in core's ELLIPSIS only past this many chars, and a
// badge keeps no more chars than cells (`truncate_label`), so it never shows
// the mark.
const _: () =
    assert!((BADGE_CELLS as usize) < pixtuoid_core::source::decoder::MAX_DECODED_FIELD_CHARS);

/// A badge's width in terminal cells, the terminal's marker included: the
/// classic painter clips its badge to it, and a label is truncated to what is
/// left.
pub const BADGE_CELLS: u16 = DESK_W + BADGE_OVERHANG;
/// The cells a badge may run past its desk's width.
const BADGE_OVERHANG: u16 = 4;

/// The [`BadgeInk`] of a badge reading `text` in `tone`.
pub fn badge_ink(text: &str, tone: BadgeTone, theme: &Theme) -> BadgeInk {
    let name = tone.rgb(theme);
    BadgeInk {
        marker: badge_hue(text, theme).unwrap_or(name),
        name,
    }
}

/// How many agents wear each label: an agent sharing its label carries its
/// session's suffix, even where the namesake is not drawn.
pub(crate) struct Namesakes(
    /// agent-office: each agent sharing its label, numbered from 1 by when
    /// it started ([`start_order`]).
    HashMap<pixtuoid_core::AgentId, usize>,
    HashMap<pixtuoid_core::AgentId, usize>,
);

impl Namesakes {
    pub(crate) fn of<'a>(agents: impl IntoIterator<Item = &'a AgentSlot>) -> Self {
        let agents: Vec<&AgentSlot> = agents.into_iter().collect();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for agent in &agents {
            *counts.entry(&*agent.label).or_insert(0) += 1;
        }
        Self(start_order(&agents, &counts), HashMap::new())
    }

    /// agent-office: also mark each parent's brief sub-agents, which are not
    /// drawn, as "+N" on its badge ([`crate::team::brief_subs`]).
    pub(crate) fn with_brief_subs<'a>(
        mut self,
        agents: impl IntoIterator<Item = &'a AgentSlot>,
        now: std::time::SystemTime,
    ) -> Self {
        self.1 = crate::team::brief_subs(agents, now);
        self
    }

    /// agent-office: `agent`'s label as shown ([`shown_label`]), with its
    /// namesake number when it shares the label, untruncated.
    pub(crate) fn name(&self, agent: &AgentSlot) -> String {
        let label = shown_label(&agent.label);
        match self.0.get(&agent.agent_id) {
            Some(n) => format!("{label}{LABEL_SEP}{n}"),
            None => label.to_owned(),
        }
    }

    /// `agent`'s badge text, disambiguated and truncated, with no marker.
    pub(crate) fn text(&self, agent: &AgentSlot) -> String {
        // agent-office: a namesake carries its start order, not a hash of its
        // session id, so the suffix says which session it is.
        let raw = self.name(agent);
        let marker = crate::display::text::cells(BADGE_MARKER.encode_utf8(&mut [0; 4]));
        // agent-office: the "+N" is kept whole and the name truncated to fit.
        let team = match self.1.get(&agent.agent_id) {
            Some(n) => format!(" +{n}"),
            None => String::new(),
        };
        let room = BADGE_CELLS.saturating_sub(marker + crate::display::text::cells(&team));
        format!("{}{team}", truncate_label(&raw, room))
    }
}

/// agent-office: `label` as the office shows it: Claude Code's sessions,
/// the usual kind, without their `cc·` source prefix, which on every name
/// was noise; other sources keep theirs, as their figures' accessories do.
pub fn shown_label(label: &str) -> &str {
    label.strip_prefix("cc\u{b7}").unwrap_or(label)
}

/// agent-office: each of `agents`' names as its badge reads before
/// truncation: [`shown_label`] with its namesake number, so a card beside
/// the office names the same session its badge does.
pub fn shown_names<'a>(
    agents: impl IntoIterator<Item = &'a AgentSlot>,
) -> HashMap<pixtuoid_core::AgentId, String> {
    let agents: Vec<&AgentSlot> = agents.into_iter().collect();
    let names = Namesakes::of(agents.iter().copied());
    agents
        .iter()
        .map(|a| (a.agent_id, names.name(a)))
        .collect()
}

/// agent-office: the agents among `agents` whose label `counts` says is
/// shared, each numbered from 1 among its namesakes by when it started (its
/// session id breaking a tie), so the first session opened in a repository
/// reads `·1` and the next `·2`.
fn start_order(
    agents: &[&AgentSlot],
    counts: &HashMap<&str, usize>,
) -> HashMap<pixtuoid_core::AgentId, usize> {
    let mut shared: Vec<&AgentSlot> = agents
        .iter()
        .copied()
        .filter(|a| counts.get(&*a.label).copied().unwrap_or(0) > 1)
        .collect();
    shared.sort_by(|a, b| {
        (&*a.label, a.created_at, &*a.session_id).cmp(&(&*b.label, b.created_at, &*b.session_id))
    });
    let mut order = HashMap::new();
    let mut prev: Option<&str> = None;
    let mut n = 0;
    for a in shared {
        n = if prev == Some(&*a.label) { n + 1 } else { 1 };
        prev = Some(&*a.label);
        order.insert(a.agent_id, n);
    }
    order
}

/// `agent`'s badge tone: exiting over whatever it last did.
pub(crate) fn tone_of(agent: &AgentSlot) -> BadgeTone {
    if agent.exiting_at.is_some() {
        return BadgeTone::Exiting;
    }
    match &agent.state {
        ActivityState::Running { .. } => BadgeTone::Running,
        ActivityState::RequiresAction { .. } => BadgeTone::RequiresAction,
        ActivityState::Idle => BadgeTone::Idle,
    }
}

/// The plate a badge drawn in pixels sits on, so its text keeps one contrast
/// whatever the room behind it
/// (`every_badge_ink_reads_on_its_plate_in_every_theme`).
pub(crate) fn badge_plate_colour(theme: &Theme) -> Rgb {
    theme.ui.tooltip_bg
}

/// Fit a label into `budget` terminal cells, and as many chars, without
/// losing the `·xxxx` session-id disambiguation suffix. Truncates from the
/// base (left of the `·`), not the suffix — otherwise the disambig becomes
/// useless ("TikTok-Android·a" tells us nothing the base alone wouldn't).
pub(crate) fn truncate_label(label: &str, budget: u16) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if span(label) <= budget {
        return Cow::Borrowed(label);
    }
    if let Some(sep_byte) = label.rfind(LABEL_SEP) {
        let suffix = &label[sep_byte..];
        let suffix_span = span(suffix);
        if suffix_span < budget {
            let base = take_span(&label[..sep_byte], budget - suffix_span);
            return Cow::Owned(format!("{base}{suffix}"));
        }
    }
    Cow::Borrowed(take_span(label, budget))
}

/// What a label spends of a badge's budget: its cells, or its chars where a
/// run of zero-width marks has more.
fn span(text: &str) -> u16 {
    let chars = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    crate::display::text::cells(text).max(chars)
}

/// The longest start of `text` whose [`span`] fits `budget`, its clusters
/// whole.
fn take_span(text: &str, budget: u16) -> &str {
    use unicode_segmentation::UnicodeSegmentation;
    let end = text
        .grapheme_indices(true)
        .map(|(i, cluster)| i + cluster.len())
        .take_while(|&end| span(&text[..end]) <= budget)
        .last()
        .unwrap_or(0);
    &text[..end]
}

/// 4-hex-char disambiguation suffix, hashed from the WHOLE `session_id` —
/// shape-agnostic where any slice of the id is not (a raw-cwd id collides on
/// both head and tail: `/x/app` vs `/y/app`), and multi-byte-safe by
/// construction. `DefaultHasher` is per-process: a display aid, not an id.
pub fn disambig_suffix(session_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut h);
    format!("{:04x}", h.finish() & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::{BadgeTone, Namesakes, badge_hue, disambig_suffix, tone_of, truncate_label};
    use super::{REPO_HUES, repo_hue};
    use pixtuoid_core::AgentId;
    use pixtuoid_core::state::{
        ActivityState, AgentSlot, GlobalDeskIndex, SceneState, ToolKind, ToolUse,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    /// agent-office: a session in `cwd` that started `secs` in.
    fn in_repo(name: &str, cwd: &str, secs: u64) -> AgentSlot {
        AgentSlot {
            cwd: Arc::from(PathBuf::from(cwd).as_path()),
            created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            ..slot(name, &format!("s-{name}"), 0, running())
        }
    }

    /// agent-office: `slots` seated, their colours dealt.
    fn dealt(slots: &[AgentSlot]) -> SceneState {
        let mut s = scene_of(slots.to_vec());
        s.assign_repo_hues();
        s
    }

    fn hue_in(s: &SceneState, a: &AgentSlot) -> Option<pixtuoid_core::sprite::Rgb> {
        repo_hue(&s.agents[&a.agent_id])
    }

    /// agent-office: a repository's sessions, worktrees included, share one
    /// colour and up to eight repositories never share one, so a bench and
    /// its cards read as one project at a glance; a session whose directory
    /// is unknown may be anyone's, so it wears none.
    #[test]
    fn each_repository_wears_its_own_colour() {
        let mut agents: Vec<AgentSlot> = (0..8)
            .map(|i| in_repo(&format!("r{i}"), &format!("/w/repo{i}"), i))
            .collect();
        agents.push(in_repo("same", "/w/repo3", 20));
        agents.push(in_repo("tree", "/w/repo5/.claude/worktrees/x", 21));
        let mut unknown = in_repo("u", "/w/x", 30);
        unknown.unknown_cwd = true;
        agents.push(unknown.clone());
        let s = dealt(&agents);
        let hue = |a: &AgentSlot| hue_in(&s, a).expect("a hue");
        let mut distinct: Vec<_> = agents[..8].iter().map(hue).map(|c| (c.r, c.g, c.b)).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 8, "eight repositories, eight colours");
        assert_eq!(hue(&agents[8]), hue(&agents[3]), "a repository's sessions match");
        assert_eq!(hue(&agents[9]), hue(&agents[5]), "a worktree is its repository's");
        assert_eq!(hue_in(&s, &unknown), None);
    }

    /// agent-office: the colour is how a repository's sitters and cards are
    /// told apart, so it holds while the repository has a session — dealing
    /// by start order recoloured every later repository whenever an earlier
    /// one's sessions all left. Driven through the reducer, so it also proves
    /// the apply path and tick's sweep both deal colours.
    #[test]
    fn a_repository_keeps_its_colour_when_an_older_one_leaves() {
        use pixtuoid_core::{AgentEvent, Reducer, Transport};
        let mut r = Reducer::new();
        let mut scene = SceneState::uniform(8);
        let t0 = SystemTime::now();
        let ids: Vec<AgentId> = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, repo)| {
                let id = AgentId::from_transcript_path(&format!("/p/{repo}.jsonl"));
                r.apply(
                    &mut scene,
                    AgentEvent::SessionStart {
                        agent_id: id,
                        source: "claude-code".into(),
                        session_id: format!("s-{repo}").into(),
                        cwd: PathBuf::from(format!("/w/{repo}")),
                        parent_id: None,
                    },
                    t0 + Duration::from_secs(i as u64),
                    Transport::Hook,
                );
                id
            })
            .collect();
        let hue = |s: &SceneState, id: &AgentId| repo_hue(&s.agents[id]);
        let before: Vec<_> = ids.iter().map(|id| hue(&scene, id)).collect();
        assert!(before.iter().all(Option::is_some), "each dealt a colour");
        r.apply(
            &mut scene,
            AgentEvent::SessionEnd {
                agent_id: ids[0],
                as_child: false,
            },
            t0 + Duration::from_secs(3),
            Transport::Hook,
        );
        // Past the walkout, well short of any stale sweep of b and c.
        r.tick(
            &mut scene,
            t0 + Duration::from_secs(4) + pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW,
        );
        assert!(!scene.agents.contains_key(&ids[0]), "the oldest has left");
        assert_eq!(hue(&scene, &ids[1]), before[1], "b keeps its colour");
        assert_eq!(hue(&scene, &ids[2]), before[2], "c keeps its colour");
    }

    /// agent-office: two repositories asking for one colour are parted, and
    /// when the one that won it leaves the other keeps the colour it took.
    #[test]
    fn colliding_repositories_part_and_stay_parted() {
        let alone = |cwd: &str| {
            let a = in_repo("probe", cwd, 0);
            hue_in(&dealt(std::slice::from_ref(&a)), &a)
        };
        let want = alone("/w/r0");
        let rival = (1..)
            .map(|i| format!("/w/r{i}"))
            .find(|p| alone(p) == want)
            .expect("some path asks for the same colour");
        let first = in_repo("first", "/w/r0", 0);
        let second = in_repo("second", &rival, 1);
        let mut s = dealt(&[first.clone(), second.clone()]);
        assert_eq!(hue_in(&s, &first), want, "the older takes what it asked for");
        let took = hue_in(&s, &second);
        assert_ne!(took, want, "the younger is parted from it");
        s.agents.remove(&first.agent_id);
        s.assign_repo_hues();
        assert_eq!(hue_in(&s, &second), took, "and keeps what it took");
    }

    /// agent-office: a repository reopened in a quiet office wears the colour
    /// it had, so "blue is this project" can stick from run to run; a session
    /// that moves into a repository wears that repository's.
    #[test]
    fn a_reopened_repository_wears_its_own_colour_again() {
        let a = in_repo("a", "/w/agent-office", 0);
        let mut s = dealt(std::slice::from_ref(&a));
        let own = hue_in(&s, &a);
        s.agents.clear();
        s.assign_repo_hues();
        let again = in_repo("again", "/w/agent-office", 100);
        s.agents.insert(again.agent_id, again.clone());
        s.assign_repo_hues();
        assert_eq!(hue_in(&s, &again), own, "its own colour back");

        let mover = in_repo("mover", "/w/elsewhere", 101);
        s.agents.insert(mover.agent_id, mover.clone());
        s.assign_repo_hues();
        s.agents.get_mut(&mover.agent_id).unwrap().cwd =
            Arc::from(PathBuf::from("/w/agent-office/.claude/worktrees/y").as_path());
        s.assign_repo_hues();
        assert_eq!(hue_in(&s, &mover), own, "moved in, so it matches");
        assert!(REPO_HUES.contains(&own.unwrap()));
    }

    fn slot(label: &str, session_id: &str, desk: usize, state: ActivityState) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path(&format!("/p/{label}-{session_id}.jsonl")),
            source: Arc::from("claude-code"),
            session_id: Arc::from(session_id),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: label.into(),
            state,
            state_started_at: SystemTime::UNIX_EPOCH,
            last_event_at: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            exiting_at: None,
            desk_index: GlobalDeskIndex(desk),
            floor_idx: 0,
            tool_call_count: 0,
            running_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
            repo_hue: None,
            turn_aborted_at: None,
        }
    }

    fn running() -> ActivityState {
        ActivityState::Running {
            item: Some(ToolUse {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("Edit")),
                kind: ToolKind::Edit,
            }),
        }
    }

    fn scene_of(slots: Vec<AgentSlot>) -> SceneState {
        let mut s = SceneState::uniform(16);
        for slot in slots {
            s.agents.insert(slot.agent_id, slot);
        }
        s
    }

    #[test]
    fn a_lone_running_agent_reads_its_bare_label_in_the_running_tone() {
        let a = slot("cc", "sess-abcd", 0, running());
        let s = scene_of(vec![a.clone()]);
        assert_eq!(Namesakes::of(s.agents.values()).text(&a), "cc");
        assert_eq!(tone_of(&a), BadgeTone::Running);
    }

    #[test]
    fn badge_hue_resolves_a_registered_prefix() {
        let theme = &crate::theme::NORMAL;
        assert!(badge_hue("cc·repo", theme).is_some());
        assert_eq!(badge_hue("cc·repo", theme), theme.source.by_prefix("cc"));
    }

    #[test]
    fn badge_hue_is_none_for_a_bare_prefix_without_separator() {
        assert_eq!(badge_hue("cx", &crate::theme::NORMAL), None);
    }

    #[test]
    fn badge_hue_is_none_for_an_unregistered_prefix() {
        assert_eq!(badge_hue("zz·repo", &crate::theme::NORMAL), None);
    }

    /// agent-office: sessions sharing a repository's name are told apart by
    /// the order they were opened in, which a person can match to their
    /// terminals; a hash of the session id matched nothing they could see.
    #[test]
    fn namesakes_are_numbered_by_when_they_started() {
        let at = |sid: &str, secs: u64, desk: usize| AgentSlot {
            created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            ..slot("cc\u{b7}repo", sid, desk, running())
        };
        // Inserted newest first and with ids sorting against their age, so
        // only the start time can give this order.
        let late = at("session-aaaa", 30, 0);
        let early = at("session-zzzz", 10, 1);
        let middle = at("session-mmmm", 20, 2);
        let other = slot("cc\u{b7}other", "session-oooo", 3, running());
        let s = scene_of(vec![
            late.clone(),
            early.clone(),
            middle.clone(),
            other.clone(),
        ]);
        let namesakes = Namesakes::of(s.agents.values());
        assert_eq!(namesakes.text(&early), "repo\u{b7}1");
        assert_eq!(namesakes.text(&middle), "repo\u{b7}2");
        assert_eq!(namesakes.text(&late), "repo\u{b7}3");
        // A label no one else wears stays bare.
        assert_eq!(namesakes.text(&other), "other");
        // The panel's cards name each session as its badge does, so the two
        // can be matched by eye.
        let names = super::shown_names(s.agents.values());
        assert_eq!(names[&middle.agent_id], "repo\u{b7}2");
    }

    /// agent-office: Claude Code is the usual source, so its `cc·` on every
    /// name said nothing; another source's prefix is what tells it apart.
    #[test]
    fn only_claude_codes_prefix_is_dropped() {
        assert_eq!(super::shown_label("cc\u{b7}blog"), "blog");
        assert_eq!(super::shown_label("cx\u{b7}blog"), "cx\u{b7}blog");
        assert_eq!(super::shown_label("ccx\u{b7}blog"), "ccx\u{b7}blog");
    }

    #[test]
    fn tone_maps_state_and_exiting_overrides_running() {
        let requires_action = slot(
            "wa",
            "sess-w",
            0,
            ActivityState::RequiresAction {
                reason: Some(Arc::from("perm")),
            },
        );
        let idle = slot("id", "sess-i", 1, ActivityState::Idle);
        let mut exiting = slot("ex", "sess-e", 2, running());
        exiting.exiting_at = Some(now());

        assert_eq!(tone_of(&requires_action), BadgeTone::RequiresAction);
        assert_eq!(tone_of(&idle), BadgeTone::Idle);
        assert_eq!(tone_of(&exiting), BadgeTone::Exiting);
    }

    /// A budget never splits a grapheme cluster: a ZWJ sequence is kept whole
    /// or dropped whole.
    #[test]
    fn a_take_keeps_clusters_whole() {
        let coder = "\u{1f469}\u{200d}\u{1f4bb}";
        let text = format!("a{coder}b");
        // The sequence is three chars in two cells, so it takes three.
        assert_eq!(super::take_span(&text, 4), format!("a{coder}"));
        assert_eq!(super::take_span(&text, 3), "a");
        assert_eq!(super::take_span(&text, u16::MAX), text);
    }

    /// A first cluster wider than the budget takes nothing, as a terminal
    /// can't draw half of it.
    #[test]
    fn a_cluster_wider_than_the_budget_is_dropped() {
        assert_eq!(super::take_span("\u{65e5}x", 1), "");
    }

    #[test]
    fn a_label_of_zero_width_marks_never_shows_the_cap_mark() {
        use pixtuoid_core::source::decoder::{ELLIPSIS, MAX_DECODED_FIELD_CHARS};
        let capped = format!(
            "a{}{ELLIPSIS}",
            "\u{301}".repeat(MAX_DECODED_FIELD_CHARS - 1)
        );
        let shown = truncate_label(&capped, super::BADGE_CELLS);
        assert!(!shown.contains(ELLIPSIS), "{shown:?}");
    }

    #[test]
    fn truncate_label_passes_short_labels_through() {
        assert_eq!(truncate_label("hello", 16), "hello");
    }

    #[test]
    fn truncate_label_preserves_disambig_suffix() {
        let out = truncate_label("TikTok-Android\u{00b7}a09a", 16);
        assert_eq!(out.chars().count(), 16);
        assert!(out.ends_with("\u{00b7}a09a"), "suffix lost: {out}");
        assert!(out.starts_with("TikTok"), "base over-truncated: {out}");
    }

    #[test]
    fn truncate_label_falls_back_to_plain_truncate_when_no_separator() {
        let out = truncate_label("a-very-long-project-name", 8);
        assert_eq!(out, "a-very-l");
    }

    #[test]
    fn truncate_label_plain_take_when_suffix_exceeds_budget() {
        let out = truncate_label("x\u{00b7}abcdefgh", 4);
        assert_eq!(out.chars().count(), 4);
        assert_eq!(out, "x\u{00b7}ab");
    }

    /// A long name's badge, marker and all, fills [`BADGE_CELLS`] and no
    /// more, short only by the half a wide character would split.
    ///
    /// [`BADGE_CELLS`]: super::BADGE_CELLS
    #[test]
    fn a_long_names_badge_fills_its_cells_marker_included() {
        use crate::display::text::cells;
        for label in [
            "cc\u{b7}a-very-long-project-name",
            "cc\u{b7}日本語プロジェクト管理ツール",
            "cc\u{b7}a日本語プロジェクト管理",
        ] {
            let a = slot(label, "sess-abcd", 0, running());
            let s = scene_of(vec![a.clone()]);
            let name = Namesakes::of(s.agents.values()).text(&a);
            let badge = format!("{}{name}", super::BADGE_MARKER);
            let n = cells(&badge);
            assert!(
                (super::BADGE_CELLS - 1..=super::BADGE_CELLS).contains(&n),
                "{badge:?} takes {n} cells"
            );
        }
    }

    /// The budget is in cells, which a CJK character takes two of: a wide name
    /// gets half the characters, the suffix kept as for any other.
    #[test]
    fn truncate_label_budgets_cells_not_chars() {
        assert_eq!(truncate_label("日本語プロジェクト", 8), "日本語プ");
        assert_eq!(
            truncate_label("日本語プロジェクト\u{00b7}a09a", 12),
            "日本語\u{00b7}a09a"
        );
    }

    #[test]
    fn uuid_ids_get_distinct_suffixes() {
        let a = disambig_suffix("c0f7fb3f-dc9c-47c3-840d-f775dd2855a3");
        let b = disambig_suffix("019ea57d-7fa7-7812-b864-bdcb9b6c7e17");
        assert_ne!(a, b);
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn ag_full_path_ids_get_distinct_suffixes() {
        let a = disambig_suffix("/users/me/.gravity/sessions/proj/alpha-01.jsonl");
        let b = disambig_suffix("/users/me/.gravity/sessions/proj/beta-02.jsonl");
        assert_ne!(a, b);
    }

    #[test]
    fn rx_cwd_ids_with_colliding_basenames_get_distinct_suffixes() {
        let a = disambig_suffix("/work/client-x/app");
        let b = disambig_suffix("/work/client-y/app");
        assert_ne!(a, b);
    }

    /// WCAG 2.2's relative luminance
    /// (<https://www.w3.org/TR/WCAG22/#dfn-relative-luminance>).
    fn luminance(c: pixtuoid_core::sprite::Rgb) -> f64 {
        let linear = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    }

    /// WCAG 2.2's contrast ratio (<https://www.w3.org/TR/WCAG22/#dfn-contrast-ratio>).
    fn contrast(a: pixtuoid_core::sprite::Rgb, b: pixtuoid_core::sprite::Rgb) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// A badge's name is small text, never large-scale, so it holds WCAG 2.2
    /// AA's 4.5:1 (<https://www.w3.org/TR/WCAG22/#contrast-minimum>) on its
    /// plate; its marker is a graphic, held to 3:1
    /// (<https://www.w3.org/TR/WCAG22/#non-text-contrast>). Over every source's
    /// label and a bare one, in every tone. The guarantee holds on the plate,
    /// which the cutaway paints: the painter #873 measured an idle badge in at
    /// 2.05:1, straight on the floor.
    #[test]
    fn every_badge_ink_reads_on_its_plate_in_every_theme() {
        use super::{badge_ink, badge_plate_colour};
        let labels: Vec<String> = pixtuoid_core::source::registry::REGISTRY
            .iter()
            .map(|s| format!("{}\u{b7}repo", s.label_prefix))
            .chain(["bare".to_owned()])
            .collect();
        for theme in crate::theme::ALL_THEMES {
            let plate = badge_plate_colour(theme);
            for tone in [
                BadgeTone::Running,
                BadgeTone::RequiresAction,
                BadgeTone::Idle,
                BadgeTone::Exiting,
            ] {
                for label in &labels {
                    let ink = badge_ink(label, tone, theme);
                    let name = contrast(ink.name, plate);
                    let marker = contrast(ink.marker, plate);
                    let at = format!("{} {tone:?} {label}", theme.name);
                    assert!(name >= 4.5, "{at}: name {name:.2}:1");
                    assert!(marker >= 3.0, "{at}: marker {marker:.2}:1");
                }
            }
        }
    }

    #[test]
    fn multibyte_ids_are_safe_and_deterministic() {
        let a = disambig_suffix("/naïveté/app");
        assert_eq!(a, disambig_suffix("/naïveté/app"));
        assert_eq!(a.len(), 4);
    }
}
