//! Source → reducer → latest `SceneState`, the shown CLIs only. Mirrors
//! pixtuoid's `runtime/driver.rs` + `pipeline.rs` (MIT, pixtuoid authors),
//! minus the other CLIs, daemon presence and the Sources panel gate.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::hook::HookRouter;
use pixtuoid_core::source::jsonl::ChildEndUnclaims;
use pixtuoid_core::source::manager::{SourceDeath, SourceManager};
use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentEvent, Reducer, SceneState, TaggedReceiver, TaggedSender, Transport};
use tokio::sync::{mpsc, watch};

pub const CC_SOURCE: &str = "claude-code";
pub const CODEX_SOURCE: &str = pixtuoid_core::source::codex::SOURCE_NAME;
pub const CURSOR_SOURCE: &str = pixtuoid_core::source::cursor::SOURCE_NAME;
/// The CLIs the office shows: Claude Code; DeepSeek Harness through
/// agent-office's own plugin (`connect dsh`); Codex, from its rollouts and
/// the hooks `connect codex` adds; Cursor, from the hooks `connect cursor` adds.
pub const SHOWN_SOURCES: [&str; 4] = [CC_SOURCE, crate::dsh::SOURCE, CODEX_SOURCE, CURSOR_SOURCE];

pub fn is_shown_source(source: &str) -> bool {
    SHOWN_SOURCES.contains(&source)
}

pub type FloorCaps = Arc<Floors>;

/// What the window's office seats, shared with the reducer: each floor's desk
/// count, and the bench each desk sits on so a repository's sessions sit on
/// one row as the office draws it.
pub struct Floors {
    caps: [AtomicUsize; MAX_FLOORS],
    /// A new layout's benches, until the reducer takes them.
    benches: Mutex<Option<[Vec<u16>; MAX_FLOORS]>>,
}

pub struct Pipeline {
    pub scene_rx: watch::Receiver<Arc<SceneState>>,
    pub floor_caps: FloorCaps,
    /// Fatal source exits, retained for Core to show instead of a healthy
    /// hooks/summary footer after its input has stopped.
    pub deaths_rx: watch::Receiver<Vec<SourceDeath>>,
}

/// Must run inside a tokio runtime: it spawns the reducer and the sources.
/// `buf_w`×`buf_h` is the office the first frame will draw.
pub fn spawn(socket_path: PathBuf, buf_w: u16, buf_h: u16) -> Pipeline {
    let boot_caps = caps_for(buf_w, buf_h);
    let floor_caps: FloorCaps = Arc::new(Floors {
        caps: std::array::from_fn(|i| AtomicUsize::new(boot_caps[i])),
        benches: Mutex::new(Some(benches_for(buf_w, buf_h))),
    });
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::new(boot_caps)));
    let (tx, rx) =
        mpsc::channel::<(Transport, AgentEvent)>(pixtuoid_core::source::EVENT_CHANNEL_CAPACITY);
    tokio::spawn(reducer_task(rx, scene_tx, Arc::clone(&floor_caps)));

    // The hook tee and the transcript watcher share one unclaim set so a
    // subagent's end seen on the hook releases its transcript claim.
    let unclaims = ChildEndUnclaims::new();
    let mut cc = ClaudeCodeSource::default_paths();
    cc.child_end_unclaims = Some(unclaims.clone());
    let mut codex = CodexSource::default_paths();
    codex.child_end_unclaims = Some(unclaims.clone());
    let router = HookRouter::new(socket_path).with_child_end_unclaims(Some(unclaims));
    let deaths_rx = spawn_sources(
        SourceManager::new()
            .with_source(Box::new(router))
            .with_source(Box::new(cc))
            .with_source(Box::new(codex)),
        tx,
    );

    Pipeline {
        scene_rx,
        floor_caps,
        deaths_rx,
    }
}

/// Start sources with their fatal exits retained on a receiver. Keeping this
/// channel in `Pipeline` is what lets the UI distinguish stale data from health.
fn spawn_sources(manager: SourceManager, tx: TaggedSender) -> watch::Receiver<Vec<SourceDeath>> {
    let (deaths_tx, deaths_rx) = watch::channel(Vec::new());
    manager.spawn_with_health(tx, deaths_tx);
    deaths_rx
}

/// The most events folded into one publish.
const BURST: usize = 256;

async fn reducer_task(
    mut rx: TaggedReceiver,
    tx: watch::Sender<Arc<SceneState>>,
    floor_caps: FloorCaps,
) {
    let mut reducer = Reducer::new();
    let caps: [usize; MAX_FLOORS] =
        std::array::from_fn(|i| floor_caps.caps[i].load(Ordering::Relaxed));
    let mut scene = SceneState::new(caps);
    let connected: HashSet<String> = SHOWN_SOURCES.iter().map(|s| s.to_string()).collect();
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let before = scene.floor_capacities;
        for (i, a) in floor_caps.caps.iter().enumerate() {
            scene.floor_capacities[i] = a.load(Ordering::Relaxed);
        }
        // A resize rereads every seat's desk against new floors; settle them
        // before the next arrival is seated against the stale reading.
        if scene.floor_capacities != before {
            scene.settle_floors();
        }
        if let Some(benches) = floor_caps.benches.lock().ok().and_then(|mut b| b.take()) {
            scene.floor_benches = benches;
        }
        // Whether there is anything new to publish: a copy of the whole
        // scene per event would be most of the work during a replay or a
        // burst of hooks.
        let mut changed = false;
        tokio::select! {
            event = rx.recv() => {
                let Some((transport, ev)) = event else { break };
                let now = SystemTime::now();
                if is_shown(&scene, &ev) {
                    reducer.apply(&mut scene, ev, now, transport);
                    changed = true;
                }
                // The rest of a burst goes into the same publish.
                for _ in 0..BURST {
                    let Ok((transport, ev)) = rx.try_recv() else { break };
                    if is_shown(&scene, &ev) {
                        reducer.apply(&mut scene, ev, now, transport);
                        changed = true;
                    }
                }
            }
            _ = sweep.tick() => {
                let now = SystemTime::now();
                // Walks out a slot that slipped the gate with a blank source.
                reducer.reconcile_connected(&mut scene, &connected, now);
                reducer.tick(&mut scene, now);
                changed = true;
            }
        }
        if changed && tx.send(Arc::new(scene.clone())).is_err() {
            break;
        }
    }
}

/// The hook socket is shared with every CLI pixtuoid connected (OpenCode,
/// Grok, ...): their events must not reach the reducer, or each one walks a
/// figure in that the next sweep walks out again. The gate pixtuoid's
/// `runtime/gate.rs` keeps, narrowed to the shown sources.
fn is_shown(scene: &SceneState, ev: &AgentEvent) -> bool {
    let source = match ev {
        AgentEvent::SessionStart { source, .. } | AgentEvent::Identity { source, .. }
            if !source.is_empty() =>
        {
            Some(source.as_str())
        }
        _ => scene.agents.get(&ev.agent_id()).map(|s| &*s.source),
    };
    source.is_none_or(is_shown_source)
}

/// Each floor's desk count for an office `buf_w`×`buf_h` laid out; at least
/// one, so an agent that arrives before the first frame still gets a desk.
pub fn caps_for(buf_w: u16, buf_h: u16) -> [usize; MAX_FLOORS] {
    std::array::from_fn(|i| {
        let seed = pixtuoid_scene::floor::floor_seed(i);
        pixtuoid_scene::floor::floor_capacity(buf_w, buf_h, seed).max(1)
    })
}

/// Set each floor's desk count to what an office `buf_w`×`buf_h` seats,
/// shrinking too: a window's size is exact, and a desk it no longer lays out
/// must stop taking agents, who would sit there unseen (pixtuoid's floating
/// `sync_floor_caps` stores for the same reason).
pub fn store_caps(caps: &FloorCaps, buf_w: u16, buf_h: u16) {
    for (slot, cap) in caps.caps.iter().zip(caps_for(buf_w, buf_h)) {
        slot.store(cap, Ordering::Relaxed);
    }
    if let Ok(mut b) = caps.benches.lock() {
        *b = Some(benches_for(buf_w, buf_h));
    }
}

/// The bench each desk of each floor sits on, for an office `buf_w`×`buf_h`.
fn benches_for(buf_w: u16, buf_h: u16) -> [Vec<u16>; MAX_FLOORS] {
    std::array::from_fn(|i| {
        let seed = pixtuoid_scene::floor::floor_seed(i);
        pixtuoid_scene::floor::floor_benches(buf_w, buf_h, seed)
    })
}

#[cfg(test)]
mod tests {
    /// The figure sits at its desk for its whole turn. Between
    /// two calls the model is still working, so only the turn's end (`Stop`,
    /// or Esc's interrupt marker, which fires no `Stop`) lets it go idle;
    /// without the bracket it left its desk after every call.
    #[test]
    fn a_turn_keeps_its_session_working_between_calls_until_it_ends() {
        use pixtuoid_core::source::claude_code::{SOURCE_NAME, decode_cc_line};
        use pixtuoid_core::source::decoder::decode_hook_payload;
        use pixtuoid_core::state::ActivityState;
        use pixtuoid_core::{AgentEvent, AgentId, Reducer, SceneState, Transport};
        use serde_json::{Value, json};
        use std::time::{Duration, SystemTime};

        let sid = "5717f13f-3c42-4a73-bfa5-3cd14932f155";
        let path = format!("/h/.claude/projects/-repo/{sid}.jsonl");
        let id = AgentId::from_parts(SOURCE_NAME, sid);
        let hook = |event: &str| {
            decode_hook_payload(json!({
                "hook_event_name": event, "session_id": sid, "cwd": "/repo",
                "transcript_path": path, "prompt": "fix it",
                "tool_name": "Read", "tool_use_id": "toolu_1",
                "tool_input": {"file_path": "/repo/a.rs"},
            }))
            .expect("decodes")
            .into_iter()
            .map(|e| (Transport::Hook, e))
            .collect::<Vec<_>>()
        };
        let line = |v: Value| {
            decode_cc_line(&path, SOURCE_NAME, v)
                .into_iter()
                .map(|e| (Transport::Jsonl, e))
                .collect::<Vec<_>>()
        };
        let user_text = |text: &str| {
            line(json!({"type": "user", "message": {"role": "user",
                "content": [{"type": "text", "text": text}]}}))
        };
        let tool_result = line(json!({"type": "user", "message": {"role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}));

        let run = |steps: Vec<Vec<(Transport, AgentEvent)>>| {
            let mut scene = SceneState::uniform(4);
            let mut r = Reducer::new();
            let mut now = SystemTime::UNIX_EPOCH;
            let mut states = Vec::new();
            for step in steps {
                for (from, ev) in step {
                    r.apply(&mut scene, ev, now, from);
                }
                now += Duration::from_secs(1);
                states.push(scene.agents[&id].state.clone());
            }
            states
        };
        let between_calls = ActivityState::Running { item: None };

        let stopped = run(vec![
            hook("SessionStart"),
            hook("UserPromptSubmit"),
            user_text("fix it"),
            hook("PreToolUse"),
            hook("PostToolUse"),
            hook("Stop"),
        ]);
        assert_eq!(
            stopped[2], between_calls,
            "the prompt's own line ends nothing"
        );
        assert_eq!(stopped[4], between_calls, "a call's end is not the turn's");
        assert_eq!(stopped[5], ActivityState::Idle);

        let interrupted = run(vec![
            hook("SessionStart"),
            hook("UserPromptSubmit"),
            hook("PreToolUse"),
            tool_result,
            user_text("[Request interrupted by user for tool use]"),
        ]);
        assert_eq!(interrupted[3], between_calls);
        assert_eq!(
            interrupted[4],
            ActivityState::Idle,
            "Esc fires no Stop; the transcript's marker ends the turn"
        );

        let failed = run(vec![
            hook("SessionStart"),
            hook("UserPromptSubmit"),
            line(json!({"type": "assistant", "isApiErrorMessage": true,
                "message": {"role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text", "text": "rate limited"}]}})),
        ]);
        assert_eq!(
            failed[2],
            ActivityState::Idle,
            "a failed request ends the turn; the figure must not type forever"
        );
    }

    /// agent-office restarts while a session is mid-turn, so its prompt
    /// (`UserPromptSubmit`) is never seen. Its live calls still prove the
    /// turn is open: without that the figure went idle after every call and
    /// the card flipped between working and done until the turn's `Stop`.
    /// A transcript replay of old calls is history and must not open one.
    #[test]
    fn a_turn_whose_prompt_went_unseen_still_keeps_its_session_working() {
        use pixtuoid_core::source::claude_code::{SOURCE_NAME, decode_cc_line};
        use pixtuoid_core::source::decoder::decode_hook_payload;
        use pixtuoid_core::state::ActivityState;
        use pixtuoid_core::{AgentEvent, AgentId, Reducer, SceneState, Transport};
        use serde_json::json;
        use std::time::{Duration, SystemTime};

        let sid = "5717f13f-3c42-4a73-bfa5-3cd14932f155";
        let path = format!("/h/.claude/projects/-repo/{sid}.jsonl");
        let id = AgentId::from_parts(SOURCE_NAME, sid);
        let hook = |event: &str, call: &str| {
            decode_hook_payload(json!({
                "hook_event_name": event, "session_id": sid, "cwd": "/repo",
                "transcript_path": path, "tool_name": "Read", "tool_use_id": call,
                "tool_input": {"file_path": "/repo/a.rs"},
            }))
            .expect("decodes")
            .into_iter()
            .map(|e| (Transport::Hook, e))
            .collect::<Vec<_>>()
        };
        let run = |steps: Vec<Vec<(Transport, AgentEvent)>>| {
            let mut scene = SceneState::uniform(4);
            let mut r = Reducer::new();
            let mut now = SystemTime::UNIX_EPOCH;
            let mut states = Vec::new();
            for step in steps {
                for (from, ev) in step {
                    r.apply(&mut scene, ev, now, from);
                }
                now += Duration::from_secs(1);
                states.push(scene.agents[&id].state.clone());
            }
            states
        };
        let between_calls = ActivityState::Running { item: None };

        let resumed = run(vec![
            hook("PreToolUse", "toolu_1"),
            hook("PostToolUse", "toolu_1"),
            hook("PreToolUse", "toolu_2"),
            hook("PostToolUse", "toolu_2"),
            hook("Stop", ""),
        ]);
        assert_eq!(resumed[1], between_calls, "a call's end is not the turn's");
        assert_eq!(resumed[3], between_calls);
        assert_eq!(resumed[4], ActivityState::Idle, "the turn's Stop still ends it");

        let line = |v| {
            decode_cc_line(&path, SOURCE_NAME, v)
                .into_iter()
                .map(|e| (Transport::Jsonl, e))
                .collect::<Vec<_>>()
        };
        let replayed = run(vec![
            hook("SessionStart", ""),
            line(json!({"type": "assistant", "message": {"role": "assistant",
                "content": [{"type": "tool_use", "id": "toolu_0", "name": "Read",
                "input": {"file_path": "/repo/a.rs"}}]}})),
            line(json!({"type": "user", "message": {"role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "toolu_0", "content": "ok"}]}})),
        ]);
        assert_eq!(
            replayed[2],
            ActivityState::Idle,
            "an old call read back from the transcript opens no turn"
        );
    }

    /// A minute after its turn ends, Claude Code nudges the session with a
    /// `Notification`. It must leave the session idle, done since its `Stop`:
    /// read as a prompt it walked every finished session to your office and
    /// out of the pantry. A real prompt still asks for you.
    #[test]
    fn the_idle_nudge_leaves_a_finished_session_done() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        use pixtuoid_core::state::ActivityState;
        use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
        use serde_json::json;
        use std::time::{Duration, SystemTime};

        let sid = "5717f13f-3c42-4a73-bfa5-3cd14932f155";
        let id = AgentId::from_parts(CC_SOURCE, sid);
        let apply = |scene: &mut SceneState, r: &mut Reducer, extra: serde_json::Value, at| {
            let mut v = json!({
                "session_id": sid, "cwd": "/repo",
                "transcript_path": format!("/h/.claude/projects/-repo/{sid}.jsonl"),
            });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            for ev in decode_hook_payload(v).expect("decodes") {
                r.apply(scene, ev, at, Transport::Hook);
            }
        };
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut scene = SceneState::uniform(4);
        let mut r = Reducer::new();
        for (i, event) in ["SessionStart", "UserPromptSubmit", "Stop"]
            .iter()
            .enumerate()
        {
            let at = t0 + Duration::from_secs(i as u64);
            apply(
                &mut scene,
                &mut r,
                json!({"hook_event_name": event, "prompt": "fix it"}),
                at,
            );
        }
        let done_at = scene.agents[&id].state_started_at;
        apply(
            &mut scene,
            &mut r,
            json!({"hook_event_name": "Notification", "notification_type": "idle_prompt",
                "message": "Claude is waiting for your input"}),
            t0 + Duration::from_secs(62),
        );
        assert_eq!(scene.agents[&id].state, ActivityState::Idle);
        assert_eq!(
            scene.agents[&id].state_started_at, done_at,
            "done since its Stop"
        );

        apply(
            &mut scene,
            &mut r,
            json!({"hook_event_name": "Notification", "notification_type": "permission_prompt",
                "message": "Claude needs your permission to use Bash"}),
            t0 + Duration::from_secs(70),
        );
        assert!(matches!(
            scene.agents[&id].state,
            ActivityState::RequiresAction { .. }
        ));
    }

    /// Claude Code's `Notification` fires for a lot that is not a prompt:
    /// a login finishing, a background session finishing, an elicitation
    /// already answered, a quota wait that resumes on its own. Read as a
    /// prompt, the figure stands in your office under 「等你确认」while you
    /// have nothing to approve. A prompt, an elicitation still open, a
    /// background session that needs input, and a quota wait that wants
    /// Enter still ask for you.
    #[test]
    fn a_notice_that_needs_nothing_does_not_make_the_session_wait() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        use pixtuoid_core::state::ActivityState;
        use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
        use serde_json::json;
        use std::time::{Duration, SystemTime};

        let sid = "5717f13f-3c42-4a73-bfa5-3cd14932f155";
        let id = AgentId::from_parts(CC_SOURCE, sid);
        let notify = |scene: &mut SceneState, r: &mut Reducer, kind: &str, at| {
            let v = json!({
                "hook_event_name": "Notification",
                "notification_type": kind,
                "message": kind,
                "session_id": sid,
                "cwd": "/repo",
                "transcript_path": format!("/h/.claude/projects/-repo/{sid}.jsonl"),
            });
            for ev in decode_hook_payload(v).expect("decodes") {
                r.apply(scene, ev, at, Transport::Hook);
            }
        };
        let idle = || {
            let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
            let mut scene = SceneState::uniform(4);
            let mut r = Reducer::new();
            for (i, event) in ["SessionStart", "UserPromptSubmit", "Stop"].iter().enumerate() {
                let v = json!({
                    "hook_event_name": event,
                    "session_id": sid,
                    "cwd": "/repo",
                    "prompt": "fix it",
                    "transcript_path": format!("/h/.claude/projects/-repo/{sid}.jsonl"),
                });
                for ev in decode_hook_payload(v).expect("decodes") {
                    r.apply(&mut scene, ev, t0 + Duration::from_secs(i as u64), Transport::Hook);
                }
            }
            assert_eq!(scene.agents[&id].state, ActivityState::Idle);
            (scene, r, t0)
        };
        for kind in [
            "auth_success",
            "elicitation_complete",
            "elicitation_response",
            "agent_completed",
            "quota_auto_resume_fired",
        ] {
            let (mut scene, mut r, t0) = idle();
            notify(&mut scene, &mut r, kind, t0 + Duration::from_secs(10));
            assert_eq!(
                scene.agents[&id].state,
                ActivityState::Idle,
                "{kind} needs nothing from you"
            );
        }
        for kind in [
            "elicitation_dialog",
            "elicitation_url_dialog",
            "agent_needs_input",
            "quota_auto_resume_stale",
        ] {
            let (mut scene, mut r, t0) = idle();
            notify(&mut scene, &mut r, kind, t0 + Duration::from_secs(10));
            assert!(
                matches!(scene.agents[&id].state, ActivityState::RequiresAction { .. }),
                "{kind} still asks for you, got {:?}",
                scene.agents[&id].state
            );
        }
    }

    /// Cursor's `stop` says how the turn ended. `aborted` and `error` are
    /// work that did not finish: the card must not say done, and the figure
    /// must not line up in your office to be read. A `completed` stop still
    /// does, once you have not opened it.
    #[test]
    fn an_aborted_or_failed_cursor_stop_is_not_a_finished_result() {
        use pixtuoid_core::source::decoder::{SOURCE_STAMP, decode_hook_payload};
        use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
        use serde_json::json;
        use std::time::{Duration, SystemTime};

        let sid = "0b7d6a1e-cu-session";
        let id = AgentId::from_parts(CURSOR_SOURCE, sid);
        let run = |status: &str| {
            let mut scene = SceneState::uniform(4);
            let mut r = Reducer::new();
            let mut now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
            for event in ["sessionStart", "beforeSubmitPrompt", "preToolUse", "postToolUse", "stop"]
            {
                let v = json!({
                    "hook_event_name": event,
                    "session_id": sid,
                    "cwd": "",
                    "workspace_roots": ["/repo"],
                    "prompt": "fix it",
                    "tool_name": "Shell",
                    "tool_input": {"command": "cargo test"},
                    "status": status,
                    SOURCE_STAMP: CURSOR_SOURCE,
                });
                for ev in decode_hook_payload(v).expect("decodes") {
                    r.apply(&mut scene, ev, now, Transport::Hook);
                }
                now += Duration::from_secs(1);
            }
            (scene, now)
        };
        let view = crate::tasks::TaskView {
            prompt: Some("fix it".into()),
            reply: Some("tests still fail".into()),
            ..crate::tasks::TaskView::default()
        };
        let judge = |status: &str| {
            let (scene, now) = run(status);
            let slot = &scene.agents[&id];
            let later = now + Duration::from_secs(5);
            (
                crate::panel::Group::of(slot, Some(&view), later, false),
                crate::panel::status(slot, Some(&view), false, later).0,
                pixtuoid_scene::waiting::lines_up(slot, later),
            )
        };
        let (group, chip, lined) = judge("completed");
        assert_eq!(group, crate::panel::Group::Done, "a finished turn is there to read");
        assert!(chip.starts_with("✓ 做完 "), "{chip}");
        assert!(lined, "a finished turn lines up");
        for status in ["aborted", "error"] {
            let (group, chip, lined) = judge(status);
            assert_eq!(
                group,
                crate::panel::Group::Idle,
                "{status} stopped without finishing, and it is not waiting on you"
            );
            assert!(chip.starts_with("· 空闲 "), "{status} must not read as done: {chip}");
            assert!(!lined, "{status} must not line up as a result to read");
        }
    }

    /// A cursor-agent figure sits at its desk for its whole turn too: the
    /// model thinks between calls, so only `stop` lets it go idle. Without
    /// the prompt opening a turn it left its desk after every call and
    /// walked back at the next one, mid-conversation.
    #[test]
    fn a_cursor_turn_keeps_its_session_working_between_calls_until_it_stops() {
        use pixtuoid_core::source::decoder::{SOURCE_STAMP, decode_hook_payload};
        use pixtuoid_core::state::ActivityState;
        use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
        use serde_json::json;
        use std::time::{Duration, SystemTime};

        let sid = "0b7d6a1e-cu-session";
        let id = AgentId::from_parts(CURSOR_SOURCE, sid);
        let mut scene = SceneState::uniform(4);
        let mut r = Reducer::new();
        let mut now = SystemTime::UNIX_EPOCH;
        let mut states = Vec::new();
        for event in [
            "sessionStart",
            "beforeSubmitPrompt",
            "preToolUse",
            "postToolUse",
            "preToolUse",
            "postToolUseFailure",
            "stop",
        ] {
            let v = json!({
                "hook_event_name": event, "session_id": sid, "cwd": "",
                "workspace_roots": ["/repo"], "prompt": "fix it",
                "tool_name": "Shell", "tool_input": {"command": "cargo test"},
                "status": "completed", SOURCE_STAMP: CURSOR_SOURCE,
            });
            for ev in decode_hook_payload(v).expect("decodes") {
                r.apply(&mut scene, ev, now, Transport::Hook);
            }
            now += Duration::from_secs(1);
            states.push(scene.agents[&id].state.clone());
        }
        let between_calls = ActivityState::Running { item: None };
        assert_eq!(states[1], between_calls, "the prompt starts the work");
        assert_eq!(states[3], between_calls, "a call's end is not the turn's");
        assert_eq!(states[5], between_calls, "nor is a failed call's");
        assert_eq!(states[6], ActivityState::Idle);
    }

    /// A dsh figure sits at its desk for its whole turn: the plugin's
    /// `turn_start` opens it and `turn_end` closes it. Without the start the
    /// figure left its desk after every call.
    #[test]
    fn a_dsh_turn_keeps_its_session_working_between_calls_until_it_ends() {
        use pixtuoid_core::source::decoder::{SOURCE_STAMP, decode_hook_payload};
        use pixtuoid_core::state::ActivityState;
        use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
        use serde_json::json;
        use std::time::{Duration, SystemTime};

        let sid = "ds-session-1";
        let id = AgentId::from_parts(crate::dsh::SOURCE, sid);
        let mut scene = SceneState::uniform(4);
        let mut r = Reducer::new();
        let mut now = SystemTime::UNIX_EPOCH;
        let mut states = Vec::new();
        for ty in [
            "session_start",
            "turn_start",
            "tool_call",
            "tool_result",
            "turn_end",
        ] {
            let v = json!({
                "type": ty, "sessionId": sid, "cwd": "/repo",
                "callId": "c1", "toolName": "bash", "_pid": 4242,
                SOURCE_STAMP: crate::dsh::SOURCE,
            });
            for ev in decode_hook_payload(v).expect("decodes") {
                r.apply(&mut scene, ev, now, Transport::Hook);
            }
            now += Duration::from_secs(1);
            states.push(scene.agents[&id].state.clone());
        }
        let between_calls = ActivityState::Running { item: None };
        assert_eq!(states[1], between_calls, "the turn starts the work");
        assert_eq!(states[3], between_calls, "a call's end is not the turn's");
        assert_eq!(states[4], ActivityState::Idle);
    }

    use super::*;

    struct DyingSource;

    impl pixtuoid_core::Source for DyingSource {
        fn name(&self) -> &str {
            CC_SOURCE
        }

        async fn run(self: Box<Self>, _tx: TaggedSender) -> anyhow::Result<()> {
            anyhow::bail!("listener exploded\n\x1b[31m")
        }
    }

    /// Once a source dies the scene is only stale history. The retained health
    /// receiver must carry its name and fatal error all the way to the footer
    /// status helper, rather than dropping them in tracing and looking healthy.
    #[tokio::test]
    async fn a_fatal_source_exit_reaches_the_user_visible_status() {
        let (tx, _rx) = mpsc::channel(1);
        let manager = SourceManager::new().with_source(Box::new(DyingSource));
        let mut deaths_rx = spawn_sources(manager, tx);
        deaths_rx.changed().await.unwrap();
        let deaths = deaths_rx.borrow_and_update().clone();
        let visible = crate::core::source_death_note(&deaths).expect("death is visible");
        assert!(visible.contains(CC_SOURCE), "{visible}");
        assert!(visible.contains("listener exploded"), "{visible}");
        assert!(!visible.contains(['\n', '\x1b']), "one safe footer line: {visible:?}");
    }

    fn start(source: &str) -> AgentEvent {
        AgentEvent::SessionStart {
            agent_id: pixtuoid_core::AgentId::from_parts(source, "s1"),
            source: source.into(),
            session_id: "s1".into(),
            cwd: "/repo".into(),
            parent_id: None,
        }
    }

    /// The CLIs agent-office connects must pass the gate; another CLI
    /// pixtuoid hooked into the same socket must not, or its figure walks in
    /// and out every sweep.
    #[test]
    fn the_gate_lets_the_connected_clis_through_and_no_other() {
        let scene = SceneState::new([1; MAX_FLOORS]);
        for source in [CC_SOURCE, crate::dsh::SOURCE, "codex", "cursor"] {
            assert!(is_shown(&scene, &start(source)), "{source}");
        }
        assert!(!is_shown(&scene, &start("opencode")));
    }
}
