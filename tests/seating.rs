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

    /// The ground floor of a `w`×`h` office, seated by the rows it draws.
    fn drawn(w: u16, h: u16) -> Self {
        let seed = pixtuoid_scene::floor::floor_seed(0);
        let mut o = Self::new(pixtuoid_scene::floor::floor_capacity(w, h, seed));
        o.scene.floor_benches[0] = pixtuoid_scene::floor::floor_benches(w, h, seed);
        o
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
    assert_ne!(
        bench(blog),
        bench(api),
        "api must not take a seat on blog's bench"
    );
    assert_eq!(
        bench(blog),
        bench(blog2),
        "blog's second session joins its bench"
    );
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
    assert_eq!(
        o.label("c"),
        o.label("a"),
        "the bench plate and label read the repo name"
    );
}

/// A new repository takes a row of its own while the office draws an empty
/// one, so a bench's plate names everyone on it. At 168×120 a partial pod
/// column deals one desk to each of two rows (desks 16 and 17); counting
/// indices in fours took them for one bench, so once 16 was taken the next
/// repository sat beside another's on desk 1 while 17's row stood empty.
#[test]
fn a_new_repository_takes_an_empty_drawn_row_while_there_is_one() {
    let mut o = Office::drawn(168, 120);
    let rows = o.scene.floor_benches[0].clone();
    assert_ne!(
        rows[16], rows[17],
        "the partial column's desks sit on two rows"
    );
    let mut on_row = std::collections::BTreeMap::new();
    for repo in ["a", "b", "c", "d", "e", "f"] {
        let desk = o.start(repo, &format!("/src/{repo}"));
        if let Some(other) = on_row.insert(rows[desk], repo) {
            panic!("{repo} sat on {other}'s row, desk {desk}");
        }
    }
}
