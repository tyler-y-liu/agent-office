//! Where sessions sit. The office groups one repository's sessions on one
//! bench, so a glance at a bench shows how many sessions a repository has open
//! and what each is doing. These drive the real `SessionStart` path.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::{BENCH_SEATS, project_root};
use pixtuoid_core::{AgentId, Reducer, SceneState};

struct Office {
    scene: SceneState,
    reducer: Reducer,
    t: SystemTime,
}

impl Office {
    fn new(desks: usize) -> Self {
        Self {
            scene: SceneState::uniform(desks),
            reducer: Reducer::new(),
            t: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        }
    }

    /// Start a session in `cwd` and return the desk it was given.
    fn start(&mut self, sid: &str, cwd: &str) -> usize {
        let id = AgentId::from_parts("claude-code", sid);
        self.t += Duration::from_secs(1);
        self.reducer.apply(
            &mut self.scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "claude-code".into(),
                session_id: sid.into(),
                cwd: PathBuf::from(cwd),
                parent_id: None,
            },
            self.t,
            Transport::Hook,
        );
        self.scene.agents[&id].desk_index.0
    }

    fn label(&self, sid: &str) -> String {
        let id = AgentId::from_parts("claude-code", sid);
        self.scene.agents[&id].label.text().to_string()
    }
}

fn bench(desk: usize) -> usize {
    desk / BENCH_SEATS
}

#[test]
fn a_second_repository_opens_its_own_bench_instead_of_squeezing_in() {
    let mut o = Office::new(16);
    let blog = o.start("a", "/src/blog");
    let api = o.start("b", "/src/api");
    let blog2 = o.start("c", "/src/blog");
    assert_ne!(bench(blog), bench(api), "api must not take a seat on blog's bench");
    assert_eq!(bench(blog), bench(blog2), "blog's second session joins its bench");
}

#[test]
fn a_full_bench_spills_onto_an_empty_one_not_a_stranger_s() {
    let mut o = Office::new(16);
    let first = o.start("b0", "/src/blog");
    let api = o.start("a0", "/src/api");
    for i in 1..BENCH_SEATS {
        assert_eq!(bench(o.start(&format!("b{i}"), "/src/blog")), bench(first));
    }
    let overflow = o.start("b9", "/src/blog");
    assert_ne!(bench(overflow), bench(first), "the bench is full");
    assert_ne!(bench(overflow), bench(api), "api's bench is not blog's");
}

#[test]
fn with_every_bench_taken_a_session_still_gets_a_desk() {
    let mut o = Office::new(8);
    o.start("a", "/src/blog");
    o.start("b", "/src/api");
    // Both benches are spoken for; the third repository still has to sit.
    let d = o.start("c", "/src/notes");
    assert!(d < 8);
}

#[test]
fn a_worktree_session_sits_and_is_named_with_its_repository() {
    assert_eq!(
        project_root(PathBuf::from("/src/blog/.claude/worktrees/fix-paging").as_path()),
        PathBuf::from("/src/blog").as_path()
    );
    let mut o = Office::new(16);
    let main = o.start("a", "/src/blog");
    o.start("b", "/src/api");
    let wt = o.start("c", "/src/blog/.claude/worktrees/fix-paging");
    assert_eq!(bench(wt), bench(main));
    assert_eq!(o.label("c"), o.label("a"), "the bench plate and label read the repo name");
}
