//! Native file dialogs (feature `dialogs`); without it the UI falls back to
//! typing paths.

use std::path::{Path, PathBuf};

/// Whether this build can show native file dialogs.
pub const AVAILABLE: bool = cfg!(feature = "dialogs");

/// Asks for one existing file. `filters` are `(label, extensions)`.
#[cfg(feature = "dialogs")]
pub fn pick_file(title: &str, dir: &Path, filters: &[(&str, &[&str])]) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_title(title).set_directory(dir);
    for (name, exts) in filters {
        d = d.add_filter(*name, exts);
    }
    d.pick_file()
}

/// Asks for one or more existing files.
#[cfg(feature = "dialogs")]
pub fn pick_files(title: &str, dir: &Path, filters: &[(&str, &[&str])]) -> Vec<PathBuf> {
    let mut d = rfd::FileDialog::new().set_title(title).set_directory(dir);
    for (name, exts) in filters {
        d = d.add_filter(*name, exts);
    }
    d.pick_files().unwrap_or_default()
}

/// Asks where to save a file.
#[cfg(feature = "dialogs")]
pub fn save_file(title: &str, dir: &Path, name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .set_directory(dir)
        .set_file_name(name)
        .save_file()
}

/// Asks for one existing file (dialogs are not built in: always `None`).
#[cfg(not(feature = "dialogs"))]
pub fn pick_file(_title: &str, _dir: &Path, _filters: &[(&str, &[&str])]) -> Option<PathBuf> {
    None
}

/// Asks for files (dialogs are not built in: always empty).
#[cfg(not(feature = "dialogs"))]
pub fn pick_files(_title: &str, _dir: &Path, _filters: &[(&str, &[&str])]) -> Vec<PathBuf> {
    Vec::new()
}

/// Asks where to save (dialogs are not built in: always `None`).
#[cfg(not(feature = "dialogs"))]
pub fn save_file(_title: &str, _dir: &Path, _name: &str) -> Option<PathBuf> {
    None
}

/// `path` relative to `base` when it lies below it, else as given. Project
/// files store paths relative to the project directory.
pub fn relative_to(base: &Path, path: &Path) -> String {
    let p = path
        .strip_prefix(base)
        .map(Path::to_path_buf)
        .ok()
        .or_else(|| relative_with_parents(base, path))
        .unwrap_or_else(|| path.to_path_buf());
    p.to_string_lossy().replace('\\', "/")
}

/// `../..`-style relative path between two absolute paths with a common root.
fn relative_with_parents(base: &Path, path: &Path) -> Option<PathBuf> {
    let base = std::path::absolute(base).ok()?;
    let path = std::path::absolute(path).ok()?;
    let b: Vec<_> = base.components().collect();
    let p: Vec<_> = path.components().collect();
    let common = b.iter().zip(&p).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return None; // different drives
    }
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c.as_os_str());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_made_relative_to_the_project_directory() {
        let base = std::path::absolute("proj/sub").unwrap();
        let below = base.join("fw/app.elf");
        assert_eq!(relative_to(&base, &below), "fw/app.elf");
        let sibling = std::path::absolute("proj/other/x.v").unwrap();
        assert_eq!(relative_to(&base, &sibling), "../other/x.v");
        // Slashes only, so a project file is portable.
        assert!(!relative_to(&base, &below).contains('\\'));
    }
}
