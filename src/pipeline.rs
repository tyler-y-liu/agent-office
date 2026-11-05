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
use pixtuoid_core::source::manager::SourceManager;
use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentEvent, Reducer, SceneState, TaggedReceiver, Transport};
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
    SourceManager::new()
        .with_source(Box::new(router))
        .with_source(Box::new(cc))
        .with_source(Box::new(codex))
        .spawn(tx);

    Pipeline {
        scene_rx,
        floor_caps,
    }
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
        for (i, a) in floor_caps.caps.iter().enumerate() {
            scene.floor_capacities[i] = a.load(Ordering::Relaxed);
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
