//! The office's two sounds, off unless `AGENT_OFFICE_SOUND=on`: a ding when
//! someone starts waiting for your approval, and a soft pop when confetti
//! falls. Which moments sound is decided here off the scene; the sounds are
//! synthesized into WAV files once and handed to the system's player, so the
//! office links no audio library.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};

use crate::tasks::TaskViews;

/// A moment the office sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chime {
    /// Someone is waiting for your approval.
    Ding,
    /// Confetti for a cheer.
    Pop,
}

/// Whether the sounds are on: `AGENT_OFFICE_SOUND=on` (or `1`).
pub fn enabled() -> bool {
    std::env::var("AGENT_OFFICE_SOUND")
        .is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true"))
}

/// Picks the moments to sound out of the scene, frame by frame.
#[derive(Debug, Default)]
pub struct Ear {
    /// Who was waiting for approval last frame.
    waiting: HashSet<AgentId>,
    /// The cheer each agent last had, by when it was earned.
    cheered: HashMap<AgentId, SystemTime>,
    /// The first frame only notes the scene: what was already so when the
    /// office opened is no news.
    primed: bool,
}

impl Ear {
    /// What sounds this frame: a ding for each agent newly waiting for
    /// approval, a pop for each new cheer, once each a frame.
    pub fn hear(&mut self, scene: &SceneState, views: &TaskViews) -> Vec<Chime> {
        let waiting: HashSet<AgentId> = scene
            .agents
            .values()
            .filter(|s| {
                s.exiting_at.is_none() && matches!(s.state, ActivityState::RequiresAction { .. })
            })
            .map(|s| s.agent_id)
            .collect();
        let cheered: HashMap<AgentId, SystemTime> = views
            .iter()
            .filter_map(|(id, v)| Some((*id, v.cheer?.1)))
            .collect();
        let mut out = Vec::new();
        if self.primed {
            if waiting.difference(&self.waiting).next().is_some() {
                out.push(Chime::Ding);
            }
            if cheered
                .iter()
                .any(|(id, at)| self.cheered.get(id) != Some(at))
            {
                out.push(Chime::Pop);
            }
        }
        self.waiting = waiting;
        self.cheered = cheered;
        self.primed = true;
        out
    }
}

/// A chime this soon after the same one is left out, so a burst of
/// approvals is one ding.
const QUIET_FOR: Duration = Duration::from_millis(800);

/// Plays chimes through the system's player.
#[derive(Debug, Default)]
pub struct Speaker {
    files: HashMap<Chime, PathBuf>,
    last: HashMap<Chime, Instant>,
}

impl Speaker {
    pub fn play(&mut self, chime: Chime) {
        let now = Instant::now();
        if self.last.get(&chime).is_some_and(|t| now - *t < QUIET_FOR) {
            return;
        }
        self.last.insert(chime, now);
        let Some(path) = self.file(chime) else { return };
        // Fire and forget: a missing player leaves the office silent, no worse.
        #[cfg(target_os = "macos")]
        let player = std::process::Command::new("afplay")
            .arg("-v")
            .arg("0.5")
            .arg(&path)
            .spawn();
        #[cfg(not(target_os = "macos"))]
        let player = std::process::Command::new("aplay")
            .arg("-q")
            .arg(&path)
            .spawn();
        if let Ok(mut child) = player {
            // Reaped off the window's thread, so no zombie is left behind.
            std::thread::spawn(move || child.wait());
        }
    }

    /// The chime's WAV file, written to the temporary directory on first use.
    fn file(&mut self, chime: Chime) -> Option<PathBuf> {
        if let Some(p) = self.files.get(&chime) {
            return Some(p.clone());
        }
        // TMPDIR is a valid choice of parent; the final directory is checked
        // below. USER is not: it is environment input, not an identity.
        #[cfg(unix)]
        let dir = std::env::temp_dir().join(format!(
            "agent-office-{}",
            rustix::process::getuid().as_raw()
        ));
        #[cfg(not(unix))]
        let dir = std::env::temp_dir().join("agent-office");
        let path = write_chime(&dir, chime).ok()?;
        self.files.insert(chime, path.clone());
        Some(path)
    }
}

fn write_chime(dir: &std::path::Path, chime: Chime) -> std::io::Result<PathBuf> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;

    let metadata = std::fs::symlink_metadata(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let safe = !metadata.file_type().is_symlink()
            && metadata.is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o777 == 0o700;
        if !safe {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "sound directory is not a non-symlink 0700 directory owned by this uid",
            ));
        }
    }
    #[cfg(not(unix))]
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "sound path is not a real directory",
        ));
    }

    let name = match chime {
        Chime::Ding => "ding.wav",
        Chime::Pop => "pop.wav",
    };
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    // A rename replaces a planted final symlink rather than its target. The
    // private directory also keeps another user from racing the temporary file.
    crate::atomic::write_private(&path, &tmp, &wav(&samples(chime)))?;
    Ok(path)
}

const RATE: u32 = 22_050;

/// The chime as samples at [`RATE`]: a bell's two partials fading over half
/// a second, or a short falling blip, quiet.
fn samples(chime: Chime) -> Vec<i16> {
    let (secs, peak) = match chime {
        Chime::Ding => (0.6, 0.35),
        Chime::Pop => (0.07, 0.18),
    };
    let n = (f64::from(RATE) * secs) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let tau = std::f64::consts::TAU;
            let v = match chime {
                Chime::Ding => {
                    let fade = (-t * 7.0).exp();
                    let attack = (t / 0.004).min(1.0);
                    attack
                        * fade
                        * (0.75 * (tau * 1318.5 * t).sin() + 0.25 * (tau * 2637.0 * t).sin())
                }
                Chime::Pop => {
                    // A pitch falling from 900 to 300 Hz under a fast fade.
                    let f = 900.0 - 600.0 * t / secs;
                    let fade = 1.0 - t / secs;
                    fade * fade * (tau * f * t).sin()
                }
            };
            (v * peak * f64::from(i16::MAX)) as i16
        })
        .collect()
}

/// `samples` as a mono 16-bit PCM WAV file at [`RATE`].
fn wav(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{Cheer, TaskView};
    use pixtuoid_core::Reducer;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use std::sync::Arc;

    fn scene_of(n: usize) -> (SceneState, Vec<AgentId>) {
        let mut scene = SceneState::uniform(8);
        let mut r = Reducer::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let ids = (0..n)
            .map(|i| {
                let id = AgentId::from_parts("claude-code", &i.to_string());
                r.apply(
                    &mut scene,
                    AgentEvent::SessionStart {
                        agent_id: id,
                        source: "claude-code".into(),
                        session_id: i.to_string(),
                        cwd: "/src/shop".into(),
                        parent_id: None,
                    },
                    t0,
                    Transport::Hook,
                );
                scene.agents.get_mut(&id).unwrap().state = ActivityState::Running { item: None };
                id
            })
            .collect();
        (scene, ids)
    }

    /// The ding is for news: an agent starting to wait sounds once, not every
    /// frame it waits, and not for one already waiting when the office opened,
    /// or a restart would ding at you for nothing new.
    #[test]
    fn a_ding_sounds_once_when_someone_starts_waiting() {
        let (mut scene, ids) = scene_of(2);
        let views = TaskViews::default();
        let mut ear = Ear::default();
        scene.agents.get_mut(&ids[0]).unwrap().state =
            ActivityState::RequiresAction { reason: None };
        assert!(
            ear.hear(&scene, &views).is_empty(),
            "already waiting at open"
        );
        assert!(ear.hear(&scene, &views).is_empty(), "still waiting");
        scene.agents.get_mut(&ids[1]).unwrap().state =
            ActivityState::RequiresAction { reason: None };
        assert_eq!(ear.hear(&scene, &views), vec![Chime::Ding]);
        assert!(ear.hear(&scene, &views).is_empty());
    }

    /// Each new cheer pops once; the same cheer seen again is not new.
    #[test]
    fn a_pop_sounds_once_for_each_new_cheer() {
        let (scene, ids) = scene_of(1);
        let mut ear = Ear::default();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100);
        let cheer = |at| -> TaskViews {
            Arc::new(HashMap::from([(
                ids[0],
                TaskView {
                    cheer: Some((Cheer::TestsPassed, at)),
                    ..TaskView::default()
                },
            )]))
        };
        assert!(ear.hear(&scene, &TaskViews::default()).is_empty());
        assert_eq!(ear.hear(&scene, &cheer(at)), vec![Chime::Pop]);
        assert!(ear.hear(&scene, &cheer(at)).is_empty());
        let later = at + Duration::from_secs(60);
        assert_eq!(ear.hear(&scene, &cheer(later)), vec![Chime::Pop]);
    }

    /// A sound path planted by another local user must never be followed:
    /// otherwise enabling a chime could overwrite any file that user can
    /// write, and the player could be handed the attacker's file instead.
    #[cfg(unix)]
    #[test]
    fn a_preplanted_chime_symlink_cannot_overwrite_its_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = crate::atomic::tests::scratch("sound-link");
        let dir = root.join("sounds");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let victim = root.join("victim.wav");
        std::fs::write(&victim, b"do not overwrite").unwrap();
        symlink(&victim, dir.join("ding.wav")).unwrap();

        let path = write_chime(&dir, Chime::Ding).unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"do not overwrite");
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&path).unwrap(), wav(&samples(Chime::Ding)));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A preplanted directory is not trusted merely because it resolves to a
    /// directory. Failing the write also means Speaker has no path to give a
    /// player, rather than risking a file in an attacker-controlled location.
    #[cfg(unix)]
    #[test]
    fn an_untrusted_sound_directory_is_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = crate::atomic::tests::scratch("sound-dir");
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let linked = root.join("linked");
        symlink(&target, &linked).unwrap();
        assert!(write_chime(&linked, Chime::Ding).is_err());
        assert!(!target.join("ding.wav").exists());

        let loose = root.join("loose");
        std::fs::create_dir(&loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(write_chime(&loose, Chime::Pop).is_err());
        assert!(!loose.join("pop.wav").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The file is a WAV a player takes: its header's sizes match its data.
    #[test]
    fn a_chime_is_a_well_formed_wav() {
        let s = samples(Chime::Ding);
        let w = wav(&s);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize,
            w.len() - 8
        );
        assert_eq!(
            u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize,
            s.len() * 2
        );
        assert!(s.iter().any(|&v| v.unsigned_abs() > 1000), "it is audible");
    }
}
