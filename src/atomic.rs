//! Replacing files in the user's home that may hold secrets (an API key in
//! settings.json's env or our env file, task titles in the summary memo).
//! Readers that run alongside us (Claude Code, Cursor, dsh) must never see a
//! half-written file, and a rewrite must never leave it readable by more
//! people than before.

use std::io::Write;
use std::path::Path;

/// Replace `path` with `bytes` through `tmp`, keeping the mode `path` has;
/// a new file is 0600.
pub fn write_keeping_mode(path: &Path, tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write(path, tmp, bytes, true)
}

/// Replace `path` with `bytes` through `tmp` as 0600, whatever it was: for
/// files only agent-office reads.
pub fn write_private(path: &Path, tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write(path, tmp, bytes, false)
}

fn write(path: &Path, tmp: &Path, bytes: &[u8], keep: bool) -> std::io::Result<()> {
    let kept = if keep {
        std::fs::metadata(path).ok().map(|m| m.permissions())
    } else {
        None
    };
    // A tmp left by a crash would keep its own, possibly wider, mode if
    // opened as is; create_new also refuses a symlink planted there.
    match std::fs::remove_file(tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(tmp)?;
    let result = (|| {
        if let Some(p) = kept {
            f.set_permissions(p)?;
        }
        f.write_all(bytes)?;
        // Without it a power loss after the rename can leave an empty file
        // where the key used to be.
        f.sync_all()?;
        std::fs::rename(tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(crate) fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    pub(crate) fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ao-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A tmp left behind world-readable by an older build or a crash must
    /// not hand its mode to the file holding the key.
    #[test]
    fn a_leftover_tmp_does_not_widen_the_new_file() {
        let dir = scratch("atomic-tmp");
        let (path, tmp) = (dir.join("f"), dir.join("f.tmp"));
        std::fs::write(&tmp, "stale").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_keeping_mode(&path, &tmp, b"new").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The owner's chosen mode is theirs (0640 to share with a group, say);
    /// a private file stays private even if an old build left it 0644.
    #[test]
    fn keeping_keeps_the_owners_mode_and_private_forces_0600() {
        let dir = scratch("atomic-mode");
        let (path, tmp) = (dir.join("f"), dir.join("f.tmp"));
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_keeping_mode(&path, &tmp, b"new").unwrap();
        assert_eq!(mode(&path), 0o640);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, &tmp, b"newer").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
