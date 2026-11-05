//! Word that a session waits on you, for when the office is out of sight: a
//! banner as each wait begins, and the Dock icon's badge counting who waits.
//! A wait is the panel's own ([`panel::waits_on_you`]): a prompt held long
//! enough to line up for your office, or a turn that ended asking you
//! something.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use pixtuoid_core::{AgentId, SceneState};

use crate::panel::{self, Wait};
use crate::tasks::TaskViews;

/// A prompt is told once it has waited as long as the office takes to line
/// it up: one you answer at the terminal within seconds is not news.
const PROMPT_TOLD_AFTER: Duration =
    Duration::from_millis(pixtuoid_scene::waiting::LINE_UP_AFTER_MS);

/// One banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub agent: AgentId,
    /// The session's name, as its card shows it.
    pub title: String,
    pub subtitle: &'static str,
    /// What it is about: its task line, or the question it asked.
    pub body: String,
}

/// What a tick asks the OS for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tick {
    pub alerts: Vec<Alert>,
    /// The badge's new count, when it changed.
    pub badge: Option<usize>,
}

#[derive(Debug, Default)]
pub struct Notifier {
    /// Each wait already told, by agent and when it began: a wait is told
    /// once, and a new one by the same session is told again.
    told: HashSet<(AgentId, SystemTime)>,
    badge: Option<usize>,
    /// The waits standing at the first tick are noted, not told: you just
    /// opened the office and see them.
    primed: bool,
    /// A banner went out since the window last had focus.
    pub unseen: bool,
}

impl Notifier {
    /// The scene at `now`. While `focused` the window is in front of you, so
    /// a new wait is noted but not bannered; the badge counts regardless.
    pub fn tick(
        &mut self,
        scene: &SceneState,
        views: &TaskViews,
        seen: &HashMap<AgentId, SystemTime>,
        now: SystemTime,
        focused: bool,
    ) -> Tick {
        let waiting: Vec<(AgentId, Wait, SystemTime)> = panel::order(scene, views, now, seen)
            .into_iter()
            .filter_map(|(id, _)| {
                let slot = scene.agents.get(&id)?;
                let read = panel::read_since_idle(slot, seen);
                let wait = panel::waits_on_you(slot, views.get(&id), read)?;
                let held = now
                    .duration_since(slot.state_started_at)
                    .unwrap_or_default();
                (wait == Wait::Reply || held >= PROMPT_TOLD_AFTER).then_some((
                    id,
                    wait,
                    slot.state_started_at,
                ))
            })
            .collect();
        let mut tick = Tick::default();
        let quiet = focused || !self.primed;
        for &(id, wait, since) in &waiting {
            if !self.told.insert((id, since)) || quiet {
                continue;
            }
            let slot = &scene.agents[&id];
            let view = views.get(&id);
            let (subtitle, body) = match wait {
                Wait::Prompt => ("等你确认", view.and_then(|v| v.line.clone())),
                Wait::Reply => (
                    "问你",
                    view.and_then(|v| v.outcome.as_deref())
                        .map(|o| o.trim_start_matches("问：").to_owned()),
                ),
            };
            tick.alerts.push(Alert {
                agent: id,
                title: slot.label.to_string(),
                subtitle,
                body: body.unwrap_or_default(),
            });
        }
        self.primed = true;
        self.unseen |= !tick.alerts.is_empty();
        self.told
            .retain(|told| waiting.iter().any(|&(id, _, since)| *told == (id, since)));
        if self.badge != Some(waiting.len()) {
            self.badge = Some(waiting.len());
            tick.badge = self.badge;
        }
        tick
    }
}

/// Whether banners are wanted: `AGENT_OFFICE_NOTIFY=off` keeps them and the
/// Dock's bounce off; the badge stays, being silent.
pub fn banners_on() -> bool {
    std::env::var("AGENT_OFFICE_NOTIFY").map_or(true, |v| v != "off")
}

/// Post `alert` as a banner and bounce the Dock icon once.
#[cfg(target_os = "macos")]
pub fn post(alert: &Alert) {
    // Through argv, never spliced into the script, so a task line cannot
    // run as AppleScript. Reaped off the window's thread.
    let child = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)",
            "-e",
            "end run",
            &alert.title,
            alert.subtitle,
            &alert.body,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
        use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
        NSApplication::sharedApplication(mtm)
            .requestUserAttention(NSRequestUserAttentionType::NSInformationalRequest);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn post(_alert: &Alert) {}

/// Show `n` on the Dock icon, nothing at 0.
#[cfg(target_os = "macos")]
pub fn set_badge(n: usize) {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSString};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let label = (n > 0).then(|| NSString::from_str(&n.to_string()));
    // SAFETY: AppKit on the main thread, which `mtm` proves.
    unsafe {
        NSApplication::sharedApplication(mtm)
            .dockTile()
            .setBadgeLabel(label.as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_badge(_n: usize) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::TaskView;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use pixtuoid_core::{ActivityState, Reducer};
    use std::sync::Arc;

    const T0: Duration = Duration::from_secs(1_800_000_000);

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + T0 + Duration::from_secs(secs)
    }

    fn scene_of(sids: &[&str]) -> (SceneState, Vec<AgentId>) {
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let ids = sids
            .iter()
            .map(|sid| {
                let id = AgentId::from_parts(crate::pipeline::CC_SOURCE, sid);
                r.apply(
                    &mut scene,
                    AgentEvent::SessionStart {
                        agent_id: id,
                        source: crate::pipeline::CC_SOURCE.into(),
                        session_id: (*sid).into(),
                        cwd: "/repo".into(),
                        parent_id: None,
                    },
                    at(0),
                    Transport::Hook,
                );
                id
            })
            .collect();
        (scene, ids)
    }

    fn prompt(scene: &mut SceneState, id: AgentId, since: u64) {
        let s = scene.agents.get_mut(&id).unwrap();
        s.state = ActivityState::RequiresAction {
            reason: Some("permission".into()),
        };
        s.state_started_at = at(since);
    }

    fn working(scene: &mut SceneState, id: AgentId, since: u64) {
        let s = scene.agents.get_mut(&id).unwrap();
        s.state = ActivityState::Running { item: None };
        s.state_started_at = at(since);
    }

    fn views(pairs: &[(AgentId, TaskView)]) -> TaskViews {
        Arc::new(pairs.iter().cloned().collect())
    }

    /// Started with the office already open: what stands then is on screen,
    /// so nothing is bannered, but the badge counts it.
    fn primed(n: &mut Notifier, scene: &SceneState, v: &TaskViews, now: u64) -> Tick {
        n.tick(scene, v, &HashMap::new(), at(now), false)
    }

    /// A prompt you answer at the terminal within seconds would buzz on every
    /// tool call; it is told only once it has held as long as the office
    /// takes to line it up, and then exactly once.
    #[test]
    fn a_prompt_is_told_once_after_it_has_held_a_while() {
        let (mut scene, ids) = scene_of(&["a"]);
        let v = views(&[(
            ids[0],
            TaskView {
                line: Some("修登录页".into()),
                ..TaskView::default()
            },
        )]);
        let mut n = Notifier::default();
        primed(&mut n, &scene, &v, 100);
        prompt(&mut scene, ids[0], 100);
        assert!(
            primed(&mut n, &scene, &v, 105).alerts.is_empty(),
            "too soon"
        );
        let t = primed(&mut n, &scene, &v, 121);
        assert_eq!(t.alerts.len(), 1);
        assert_eq!(t.alerts[0].subtitle, "等你确认");
        assert_eq!(t.alerts[0].body, "修登录页");
        assert_eq!(t.badge, Some(1));
        assert!(n.unseen);
        let again = primed(&mut n, &scene, &v, 140);
        assert!(again.alerts.is_empty(), "told once");
        assert_eq!(again.badge, None, "badge unchanged");
    }

    /// The same session asking again later is a new wait you have not seen.
    #[test]
    fn a_new_wait_by_the_same_session_is_told_again() {
        let (mut scene, ids) = scene_of(&["a"]);
        let v = views(&[]);
        let mut n = Notifier::default();
        primed(&mut n, &scene, &v, 0);
        prompt(&mut scene, ids[0], 0);
        assert_eq!(primed(&mut n, &scene, &v, 30).alerts.len(), 1);
        working(&mut scene, ids[0], 31);
        assert_eq!(primed(&mut n, &scene, &v, 32).badge, Some(0));
        prompt(&mut scene, ids[0], 40);
        assert_eq!(primed(&mut n, &scene, &v, 70).alerts.len(), 1);
    }

    /// A turn that ended on a question waits on you at once: there is no
    /// terminal prompt you might already be answering. The banner carries the
    /// question without the panel's 「问：」 marker.
    #[test]
    fn a_question_is_told_at_once_with_the_question_as_its_body() {
        let (mut scene, ids) = scene_of(&["a"]);
        let mut n = Notifier::default();
        primed(&mut n, &scene, &views(&[]), 0);
        scene.agents.get_mut(&ids[0]).unwrap().state_started_at = at(50);
        let v = views(&[(
            ids[0],
            TaskView {
                outcome: Some("问：要不要顺便升级依赖？".into()),
                outcome_summarized: true,
                ..TaskView::default()
            },
        )]);
        let t = primed(&mut n, &scene, &v, 51);
        assert_eq!(t.alerts.len(), 1);
        assert_eq!(t.alerts[0].subtitle, "问你");
        assert_eq!(t.alerts[0].body, "要不要顺便升级依赖？");
    }

    /// With the window in front of you a banner only repeats what you see;
    /// the wait is noted so it is not bannered when you look away.
    #[test]
    fn nothing_is_bannered_while_the_window_has_focus() {
        let (mut scene, ids) = scene_of(&["a"]);
        let v = views(&[]);
        let mut n = Notifier::default();
        primed(&mut n, &scene, &v, 0);
        prompt(&mut scene, ids[0], 0);
        let t = n.tick(&scene, &v, &HashMap::new(), at(30), true);
        assert!(t.alerts.is_empty());
        assert_eq!(t.badge, Some(1));
        assert!(primed(&mut n, &scene, &v, 31).alerts.is_empty());
    }

    /// Opening the office must not fire a banner per session already waiting.
    #[test]
    fn waits_standing_when_the_office_opens_are_not_bannered() {
        let (mut scene, ids) = scene_of(&["a", "b"]);
        prompt(&mut scene, ids[0], 0);
        prompt(&mut scene, ids[1], 0);
        let v = views(&[]);
        let mut n = Notifier::default();
        let t = primed(&mut n, &scene, &v, 60);
        assert!(t.alerts.is_empty());
        assert_eq!(t.badge, Some(2));
        assert!(!n.unseen);
    }
}
