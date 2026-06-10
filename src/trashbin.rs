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

/// Send a path to the trash. Returns where it landed, if we could determine it
/// (always in managed mode; best-effort with the OS trash).
pub fn send(path: &Path, txn: &str) -> Result<Option<PathBuf>> {
    if let Ok(root) = std::env::var("NAVI_TRASH_DIR") {
        let dir = Path::new(&root).join(txn);
        fs::create_dir_all(&dir)?;
        let name = path
            .file_name()
            .ok_or_else(|| NaviError::new("invalid_path", "path has no file name"))?;
        let dest = dir.join(name);
        move_path(path, &dest)?;
        return Ok(Some(dest));
    }

    let before = os_trash_snapshot();
    os_trash(path)?;
    let after = os_trash_snapshot();
    Ok(after.into_iter().find(|p| !before.contains(p)))
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
