//! Source → reducer → latest `SceneState`, Claude Code only. Mirrors
//! pixtuoid's `runtime/driver.rs` + `pipeline.rs` (MIT, pixtuoid authors),
//! minus the other CLIs, daemon presence and the Sources panel gate.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::hook::HookRouter;
use pixtuoid_core::source::jsonl::ChildEndUnclaims;
use pixtuoid_core::source::manager::SourceManager;
use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentEvent, Reducer, SceneState, TaggedReceiver, Transport};
use tokio::sync::{mpsc, watch};

pub const CC_SOURCE: &str = "claude-code";

pub type FloorCaps = Arc<[AtomicUsize; MAX_FLOORS]>;

pub struct Pipeline {
    pub scene_rx: watch::Receiver<Arc<SceneState>>,
    pub floor_caps: FloorCaps,
}

/// Must run inside a tokio runtime: it spawns the reducer and the sources.
pub fn spawn(socket_path: PathBuf, boot_caps: [usize; MAX_FLOORS]) -> Pipeline {
    let floor_caps: FloorCaps = Arc::new(std::array::from_fn(|i| AtomicUsize::new(boot_caps[i])));
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::new(boot_caps)));
    let (tx, rx) =
        mpsc::channel::<(Transport, AgentEvent)>(pixtuoid_core::source::EVENT_CHANNEL_CAPACITY);
    tokio::spawn(reducer_task(rx, scene_tx, Arc::clone(&floor_caps)));

    // The hook tee and the transcript watcher share one unclaim set so a
    // subagent's end seen on the hook releases its transcript claim.
    let unclaims = ChildEndUnclaims::new();
    let mut cc = ClaudeCodeSource::default_paths();
    cc.child_end_unclaims = Some(unclaims.clone());
    let router = HookRouter::new(socket_path).with_child_end_unclaims(Some(unclaims));
    SourceManager::new()
        .with_source(Box::new(router))
        .with_source(Box::new(cc))
        .spawn(tx);

    Pipeline {
        scene_rx,
        floor_caps,
    }
}

async fn reducer_task(
    mut rx: TaggedReceiver,
    tx: watch::Sender<Arc<SceneState>>,
    floor_caps: FloorCaps,
) {
    let mut reducer = Reducer::new();
    let caps: [usize; MAX_FLOORS] = std::array::from_fn(|i| floor_caps[i].load(Ordering::Relaxed));
    let mut scene = SceneState::new(caps);
    let connected: HashSet<String> = HashSet::from([CC_SOURCE.to_string()]);
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        for (i, a) in floor_caps.iter().enumerate() {
            scene.floor_capacities[i] = a.load(Ordering::Relaxed);
        }
        tokio::select! {
            event = rx.recv() => {
                let Some((transport, ev)) = event else { break };
                if is_claude(&scene, &ev) {
                    reducer.apply(&mut scene, ev, SystemTime::now(), transport);
                }
            }
            _ = sweep.tick() => {
                let now = SystemTime::now();
                // Walks out a slot that slipped the gate with a blank source.
                reducer.reconcile_connected(&mut scene, &connected, now);
                reducer.tick(&mut scene, now);
            }
        }
        if tx.send(Arc::new(scene.clone())).is_err() {
            break;
        }
    }
}

/// The hook socket is shared with every CLI pixtuoid connected (Cursor, dsh,
/// ...): their events must not reach the reducer, or each one walks a
/// figure in that the next sweep walks out again. The gate pixtuoid's
/// `runtime/gate.rs` keeps, narrowed to one source.
fn is_claude(scene: &SceneState, ev: &AgentEvent) -> bool {
    let source = match ev {
        AgentEvent::SessionStart { source, .. } | AgentEvent::Identity { source, .. }
            if !source.is_empty() =>
        {
            Some(source.as_str())
        }
        _ => scene.agents.get(&ev.agent_id()).map(|s| &*s.source),
    };
    source.is_none_or(|s| s == CC_SOURCE)
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
    for (slot, cap) in caps.iter().zip(caps_for(buf_w, buf_h)) {
        slot.store(cap, Ordering::Relaxed);
    }
}
