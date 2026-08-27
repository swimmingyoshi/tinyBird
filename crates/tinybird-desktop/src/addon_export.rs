//! Writing the live addon snapshot to disk for overlays and external tools.
//!
//! The snapshot itself is produced by `tinybird-games`, which has no filesystem
//! dependency so the WebAssembly build can report identical data. Only the
//! writing lives here.

use std::fs;
use std::path::Path;

use tinybird_games::{snapshot_to_json, StreamSnapshot};

/// Where the live snapshot is written for overlays and external tools.
pub const STREAM_SNAPSHOT_PATH: &str = "stream-data/current-game.json";

/// Write the snapshot, skipping the write when nothing changed so overlays are
/// not woken four times a second.
pub fn write_stream_snapshot(snapshot: &StreamSnapshot, previous_json: &mut Option<String>) {
    let Ok(json) = snapshot_to_json(snapshot) else {
        return;
    };
    if previous_json.as_deref() == Some(json.as_str()) {
        return;
    }

    let path = Path::new(STREAM_SNAPSHOT_PATH);
    if let Some(parent) = path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!(
                "Failed to create addon export directory '{}': {}",
                parent.display(),
                err
            );
            return;
        }
    }

    match fs::write(path, &json) {
        Ok(()) => *previous_json = Some(json),
        Err(err) => eprintln!(
            "Failed to write addon snapshot '{}': {}",
            path.display(),
            err
        ),
    }
}
