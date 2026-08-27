//! Loading addon manifests off disk.
//!
//! This lives in the frontend rather than in `tinybird-games` on purpose: that
//! crate has no filesystem dependency, which is what lets the same addons run
//! in the browser where there is no filesystem to have.
//!
//! Every failure here is reported and skipped rather than fatal. A manifest is
//! a file someone hand-edited, and one bad file should cost that one addon —
//! not the emulator.

use std::path::{Path, PathBuf};

use tinybird_addons::ManifestAddon;

/// Where manifests live unless `TINYBIRD_ADDONS` says otherwise.
const DEFAULT_DIR: &str = "addons";

/// What loading found, so the caller can say so rather than guess.
pub struct Loaded {
    pub addons: Vec<ManifestAddon>,
    /// One line per file that could not be used, ready to print.
    pub problems: Vec<String>,
    pub dir: PathBuf,
}

/// Read every `*.json` in the addon directory.
///
/// A missing directory is not a problem: most people have no manifests, and
/// the shipped addons are compiled in.
pub fn load() -> Loaded {
    let dir = std::env::var("TINYBIRD_ADDONS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DIR));

    let mut loaded = Loaded {
        addons: Vec::new(),
        problems: Vec::new(),
        dir: dir.clone(),
    };

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return loaded;
    };

    // Sorted, so the order addons are registered in does not depend on how the
    // filesystem happens to hand them back.
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();

    for path in paths {
        match read_one(&path) {
            Ok(addon) => loaded.addons.push(addon),
            Err(problem) => loaded.problems.push(problem),
        }
    }

    loaded
}

fn read_one(path: &Path) -> Result<ManifestAddon, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let json = std::fs::read_to_string(path).map_err(|err| format!("{name}: {err}"))?;
    ManifestAddon::parse(&json).map_err(|err| format!("{name}: {err}"))
}

/// Load manifests and hand them to the registry, printing what happened.
///
/// Called once at startup, before anything reads a snapshot.
pub fn install() {
    let loaded = load();

    for problem in &loaded.problems {
        eprintln!("addon manifest ignored - {problem}");
    }

    if loaded.addons.is_empty() {
        return;
    }

    let count = loaded.addons.len();
    match tinybird_games::install_manifests(loaded.addons) {
        Ok(_) => println!(
            "Loaded {count} addon manifest{} from {}",
            if count == 1 { "" } else { "s" },
            loaded.dir.display()
        ),
        Err(err) => eprintln!("addon manifests not installed: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that is not there is the normal case, not a failure: most
    /// people have no manifests and the shipped addons are compiled in.
    #[test]
    fn a_missing_directory_is_quiet() {
        std::env::set_var("TINYBIRD_ADDONS", "no-such-directory-anywhere");
        let loaded = load();
        assert!(loaded.addons.is_empty());
        assert!(loaded.problems.is_empty());
        std::env::remove_var("TINYBIRD_ADDONS");
    }

    /// One bad file costs that one addon. Anything else would let a typo in a
    /// hand-edited file take the emulator down with it.
    #[test]
    fn a_broken_manifest_is_skipped_and_reported() {
        let dir = std::env::temp_dir().join("tinybird-manifest-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        std::fs::write(dir.join("good.json"), GOOD).expect("write good");
        std::fs::write(dir.join("broken.json"), "{ not json").expect("write broken");
        // Not a manifest at all, and not picked up.
        std::fs::write(dir.join("notes.txt"), "ignored").expect("write txt");

        std::env::set_var("TINYBIRD_ADDONS", &dir);
        let loaded = load();
        std::env::remove_var("TINYBIRD_ADDONS");

        assert_eq!(loaded.addons.len(), 1, "the good one should still load");
        assert_eq!(loaded.problems.len(), 1);
        assert!(
            loaded.problems[0].starts_with("broken.json:"),
            "the report should name the file: {}",
            loaded.problems[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    const GOOD: &str = r#"{
      "addon_id": "test.good",
      "display_name": "Good",
      "matches": { "game_code_prefix": ["BPR"] },
      "sections": [{ "id": "s", "title": "S", "kind": "key_value",
        "fields": [{ "label": "X", "read": { "u8": "0x02000000" } }] }]
    }"#;
}
