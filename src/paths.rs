//! Where navi keeps its state: telemetry log and edit/move/remove journal.
//! Override the root with `NAVI_DATA_DIR` (used by tests for isolation).

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NAVI_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .map(|d| d.join("navi"))
        .unwrap_or_else(|| PathBuf::from("/tmp/navi"))
}

pub fn telemetry_log() -> PathBuf {
    data_dir().join("telemetry.jsonl")
}

pub fn journal_dir() -> PathBuf {
    data_dir().join("journal")
}
