//! Sending paths to the trash, and the moves that restore relies on.
//!
//! Two modes. When `NAVI_TRASH_DIR` is set we manage our own holding dir and
//! know exactly where each item lands — deterministic, hermetic, used by tests
//! and sandboxes. Otherwise we use the real OS trash (macOS ~/.Trash, Windows
//! recycle bin, freedesktop on Linux) via the `trash` crate, and best-effort
//! capture the resulting location so `navi restore` can move it back without
//! needing the OS's (macOS-absent) restore API.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{NaviError, Result};
use crate::paths;

/// Send a path to the trash. Returns where it landed, if we could determine it
/// (always in managed mode; best-effort with the OS trash).
pub fn send(path: &Path, txn: &str) -> Result<Option<PathBuf>> {
    // Explicit managed mode (tests, sandboxes).
    if let Ok(root) = std::env::var("NAVI_TRASH_DIR") {
        return managed_send(Path::new(&root), path, txn).map(Some);
    }

    // The OS trash sends a file to its OWN volume's trash. Our capture only
    // watches ~/.Trash, so a file on another volume would be journaled with no
    // location and become unrestorable. Route those into the managed trash,
    // where we know the exact destination. Same-volume removals (the common
    // case) still go to the real OS trash.
    if !on_home_volume(path) {
        return managed_send(&paths::data_dir().join("trash"), path, txn).map(Some);
    }

    let before = os_trash_snapshot();
    os_trash(path)?;
    let after = os_trash_snapshot();
    let appeared: Vec<PathBuf> = after.into_iter().filter(|p| !before.contains(p)).collect();
    Ok(pick_trashed(&appeared, path))
}

/// From the entries that appeared in the trash, pick the one that resembles the
/// file we just trashed — by exact name, or macOS's collision-rename form
/// (`name 2.ext`). Never grab an unrelated entry that raced into the trash; a
/// missed capture is recoverable (journaled null), a wrong one is not.
fn pick_trashed(appeared: &[PathBuf], original: &Path) -> Option<PathBuf> {
    let orig = original.file_name()?.to_string_lossy().into_owned();
    let stem = original
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| orig.clone());
    appeared
        .iter()
        .find(|p| {
            p.file_name()
                .map(|n| resembles(&n.to_string_lossy(), &orig, &stem))
                .unwrap_or(false)
        })
        .cloned()
}

fn resembles(candidate: &str, orig: &str, stem: &str) -> bool {
    if candidate == orig {
        return true;
    }
    // Collision rename keeps the stem, then a separator (" 2.ext", " copy", ".ext").
    match candidate.strip_prefix(stem) {
        Some(rest) => rest.is_empty() || rest.starts_with(' ') || rest.starts_with('.'),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::resembles;

    #[test]
    fn matches_exact_and_collision_renames() {
        assert!(resembles("foo.txt", "foo.txt", "foo"));
        assert!(resembles("foo 2.txt", "foo.txt", "foo")); // macOS collision form
        assert!(resembles("foo", "foo", "foo")); // no extension
        assert!(resembles("foo 2", "foo", "foo")); // no extension, collision
    }

    #[test]
    fn rejects_unrelated_and_lookalike_names() {
        assert!(!resembles("unrelated.txt", "foo.txt", "foo")); // a stranger raced in
        assert!(!resembles("foobar.txt", "foo.txt", "foo")); // shares a prefix, different file
    }
}

/// Move a path into a managed holding dir namespaced by transaction, returning
/// the exact destination.
fn managed_send(root: &Path, path: &Path, txn: &str) -> Result<PathBuf> {
    let dir = root.join(txn);
    fs::create_dir_all(&dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| NaviError::new("invalid_path", "path has no file name"))?;
    let dest = dir.join(name);
    move_path(path, &dest)?;
    Ok(dest)
}

/// Is `path` on the same filesystem volume as home? If so the OS trash lands it
/// in ~/.Trash, where our capture can see it. Defaults to true on any doubt —
/// the worst case is a null capture, never data loss.
fn on_home_volume(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Some(home), Ok(m)) = (dirs::home_dir(), fs::metadata(path)) {
            if let Ok(h) = fs::metadata(home) {
                return h.dev() == m.dev();
            }
        }
    }
    true
}

/// Send to the OS trash. On macOS we use NSFileManager directly rather than the
/// crate's default Finder/AppleScript path, which times out in headless/
/// non-interactive contexts and needs automation permission. NSFileManager
/// still lands the item in ~/.Trash, so the capture diff works.
fn os_trash(path: &Path) -> Result<()> {
    let mut ctx = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx.delete(path)
        .map_err(|e| NaviError::new("trash_failed", format!("could not trash {}: {e}", path.display())))
}

/// Move a path, falling back to copy+remove across filesystems. Also the
/// restore primitive (trash → original location).
pub fn move_path(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if from.is_dir() {
        copy_dir(from, to)?;
        fs::remove_dir_all(from)?;
    } else {
        fs::copy(from, to)?;
        fs::remove_file(from)?;
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dst)?;
        } else {
            fs::copy(entry.path(), &dst)?;
        }
    }
    Ok(())
}

/// Top-level entries currently in the user's home trash — diffed before/after a
/// delete to learn where the item went.
fn os_trash_snapshot() -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    if let Some(home) = dirs::home_dir() {
        if let Ok(rd) = fs::read_dir(home.join(".Trash")) {
            for e in rd.flatten() {
                set.insert(e.path());
            }
        }
    }
    set
}
