//! What the GUI remembers between runs (window size, UI scale, theme, recent
//! projects) and the lists it offers (examples). Plain JSON in the user's
//! config directory; nothing here touches the simulation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How many recently opened projects are remembered.
pub const RECENT_CAP: usize = 8;

/// UI scale steps (Ctrl+= and Ctrl+- move along them).
pub const SCALES: &[f32] = &[0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// Window size used when nothing was remembered, in points.
pub const DEFAULT_WINDOW: [f32; 2] = [1280.0, 820.0];

/// Settings and history kept between runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Inner window size in points, as last seen.
    pub window: Option<[f32; 2]>,
    /// Zoom factor of the whole UI (one of [`SCALES`]).
    pub ui_scale: f32,
    /// True for the dark theme.
    pub dark: bool,
    /// Project files opened most recently first, absolute paths, no duplicates.
    pub recent: Vec<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            window: None,
            ui_scale: 1.0,
            dark: true,
            recent: Vec::new(),
        }
    }
}

impl Prefs {
    /// `%APPDATA%\zynqsim\gui.json` on Windows, `$XDG_CONFIG_HOME` or
    /// `~/.config` elsewhere. `None` when no such directory is known.
    pub fn default_path() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        }?;
        Some(base.join("zynqsim").join("gui.json"))
    }

    /// Reads the file. A missing or unreadable file gives the defaults: the
    /// GUI must start even when its settings are damaged.
    pub fn load(path: &Path) -> Prefs {
        let mut p: Prefs = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // Hand-edited or damaged values fall back to something usable.
        if !SCALES.contains(&p.ui_scale) {
            p.ui_scale = 1.0;
        }
        p.window = p
            .window
            .filter(|[w, h]| (200.0..=10_000.0).contains(w) && (200.0..=10_000.0).contains(h));
        p.recent.truncate(RECENT_CAP);
        p
    }

    /// Writes the file, creating its directory.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text + "\n")
    }

    /// Moves `file` to the front of the recent list (absolute, no duplicates),
    /// keeping at most [`RECENT_CAP`] entries.
    pub fn push_recent(&mut self, file: &Path) {
        let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
        self.recent.retain(|r| !same_path(r, &file));
        self.recent.insert(0, file);
        self.recent.truncate(RECENT_CAP);
    }

    /// Forgets every recent project.
    pub fn clear_recent(&mut self) {
        self.recent.clear();
    }

    /// Drops a recent entry (used when its file has gone).
    pub fn forget(&mut self, file: &Path) {
        let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
        self.recent.retain(|r| !same_path(r, &file));
    }
}

/// Same file by name: case-insensitive on Windows, where paths are.
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// The next UI scale step up or down from `current`.
pub fn next_scale(current: f32, up: bool) -> f32 {
    // The nearest step first, so an odd value from a hand-edited file moves
    // to a real step.
    let i = SCALES
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - current).abs().total_cmp(&(b.1 - current).abs()))
        .map_or(2, |(i, _)| i);
    let j = if up {
        (i + 1).min(SCALES.len() - 1)
    } else {
        i.saturating_sub(1)
    };
    SCALES[j]
}

/// The `examples` directory of this checkout or installation: next to the
/// executable (a release archive), above it (`target/...`), above the working
/// directory, and last the source tree the crate was built from. The first one
/// that holds a `project.toml` wins.
pub fn find_examples_dir() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        roots.extend(exe.ancestors().take(6).map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().take(5).map(Path::to_path_buf));
    }
    let mut dirs: Vec<PathBuf> = roots.into_iter().map(|r| r.join("examples")).collect();
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples"));
    dirs.into_iter().find(|d| !list_examples(d).is_empty())
}

/// Every `<dir>/<name>/project.toml`, as `(name, project.toml path)`, by name.
pub fn list_examples(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let project = e.path().join("project.toml");
            project
                .is_file()
                .then(|| (e.file_name().to_string_lossy().into_owned(), project))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir, unique per test run.
    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zynqsim-prefs-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recent_projects_dedupe_move_to_front_and_cap_at_eight() {
        let mut p = Prefs::default();
        for i in 0..10 {
            p.push_recent(Path::new(&format!("proj{i}/project.toml")));
        }
        assert_eq!(p.recent.len(), RECENT_CAP);
        assert!(
            p.recent[0].to_string_lossy().contains("proj9"),
            "newest first"
        );
        assert!(
            !p.recent
                .iter()
                .any(|r| r.to_string_lossy().contains("proj1/")),
            "the two oldest fell off"
        );
        // Opening an entry again moves it to the front without duplicating it.
        p.push_recent(Path::new("proj5/project.toml"));
        assert_eq!(p.recent.len(), RECENT_CAP);
        assert!(p.recent[0].to_string_lossy().contains("proj5"));
        assert_eq!(
            p.recent
                .iter()
                .filter(|r| r.to_string_lossy().contains("proj5"))
                .count(),
            1
        );
        p.forget(Path::new("proj5/project.toml"));
        assert_eq!(p.recent.len(), RECENT_CAP - 1);
        p.clear_recent();
        assert!(p.recent.is_empty());
    }

    #[test]
    fn settings_survive_a_save_and_load_in_a_temp_dir() {
        let dir = scratch("roundtrip");
        let path = dir.join("nested").join("gui.json");
        let mut p = Prefs {
            window: Some([1111.0, 700.0]),
            ui_scale: 1.25,
            dark: false,
            recent: Vec::new(),
        };
        p.push_recent(Path::new("examples/demo/project.toml"));
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), p, "the file reads back as saved");
        // A missing or damaged file gives the defaults, never an error.
        assert_eq!(Prefs::load(&dir.join("absent.json")), Prefs::default());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Prefs::load(&path), Prefs::default());
        // Values out of range are repaired on load.
        std::fs::write(
            &path,
            r#"{"window":[5.0,9.0],"ui_scale":7.0,"dark":true,"recent":[]}"#,
        )
        .unwrap();
        let q = Prefs::load(&path);
        assert_eq!(q.window, None);
        assert_eq!(q.ui_scale, 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scale_steps_move_one_notch_and_stop_at_the_ends() {
        assert_eq!(next_scale(1.0, true), 1.1);
        assert_eq!(next_scale(1.0, false), 0.9);
        assert_eq!(next_scale(2.0, true), 2.0);
        assert_eq!(next_scale(0.8, false), 0.8);
        // An odd stored value moves to the nearest real step first (1.2 is 1.25).
        assert_eq!(next_scale(1.2, true), 1.5);
        assert_eq!(next_scale(1.2, false), 1.1);
    }

    #[test]
    fn examples_discovery_lists_the_repos_examples() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let list = list_examples(&dir);
        let names: Vec<&str> = list.iter().map(|e| e.0.as_str()).collect();
        for want in ["demo", "rust-debug", "timer-modes", "oled-demo"] {
            assert!(names.contains(&want), "{want} is listed: {names:?}");
        }
        assert!(list.iter().all(|e| e.1.is_file()));
        assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted by name");
        // A folder without a project file is not listed (the folder is only a
        // board file here, so this checks the rule on a path that is not one).
        assert!(!names.contains(&"custom-board-missing"));
        assert!(list.iter().all(|(_, p)| p.is_file()));
        assert!(find_examples_dir().is_some(), "the locator finds them too");
    }
}
