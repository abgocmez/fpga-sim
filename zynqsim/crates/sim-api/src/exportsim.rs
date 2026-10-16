//! `[rtl]` sources beyond a literal file list: glob patterns in `files`, and a Vivado
//! simulation export (`export_dir` / `filelist`, read by the `xsim-export` crate) whose sources,
//! include directories, defines and libraries feed the XSim adapter (ADR-017).

use crate::project::{Diagnostic, RtlSection, Severity};
use std::path::{Path, PathBuf};
use xsim_export::ExportDesign;

/// `true` when a path component uses glob syntax.
pub fn has_wildcard(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

/// Matches one path component against a pattern with `*`, `?`, `[abc]`, `[a-z]` and `[!x]`.
/// Case-insensitive on Windows, where file names are.
fn segment_matches(pattern: &str, name: &str) -> bool {
    let fold = |s: &str| -> Vec<char> {
        if cfg!(windows) {
            s.to_lowercase().chars().collect()
        } else {
            s.chars().collect()
        }
    };
    fn go(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len()).any(|i| go(&p[1..], &s[i..])),
            Some('?') => !s.is_empty() && go(&p[1..], &s[1..]),
            Some('[') => {
                let Some(close) = p.iter().skip(2).position(|&c| c == ']').map(|i| i + 2) else {
                    return !s.is_empty() && s[0] == '[' && go(&p[1..], &s[1..]);
                };
                if s.is_empty() {
                    return false;
                }
                let set = &p[1..close];
                let (neg, set) = match set.first() {
                    Some('!' | '^') => (true, &set[1..]),
                    _ => (false, set),
                };
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= set[i] <= s[0] && s[0] <= set[i + 2];
                        i += 3;
                    } else {
                        hit |= set[i] == s[0];
                        i += 1;
                    }
                }
                hit != neg && go(&p[close + 1..], &s[1..])
            }
            Some(&c) => !s.is_empty() && s[0] == c && go(&p[1..], &s[1..]),
        }
    }
    go(&fold(pattern), &fold(name))
}

/// Expands a glob pattern relative to `base` (or absolute). `**` matches any number of
/// directories. The matches of one pattern are sorted by path, so a project builds the same
/// way on every machine. A pattern without wildcards is returned as it is, existing or not.
pub fn glob(base: &Path, pattern: &str) -> Result<Vec<PathBuf>, String> {
    let pat = pattern.replace('\\', "/");
    if !has_wildcard(&pat) {
        return Ok(vec![resolve(base, &pat)]);
    }
    // The root: a drive or `/` for absolute patterns, else the project directory.
    let (root, rest): (PathBuf, &str) = match pat.as_bytes() {
        [d, b':', b'/', ..] if d.is_ascii_alphabetic() => (PathBuf::from(&pat[..3]), &pat[3..]),
        [b'/', ..] => (PathBuf::from("/"), pat.trim_start_matches('/')),
        _ => (base.to_path_buf(), pat.as_str()),
    };
    let segs: Vec<&str> = rest
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if segs.is_empty() {
        return Err(format!("`{pattern}` names no file"));
    }
    let mut out = Vec::new();
    walk(&root, &segs, &mut out, 0);
    out.sort();
    out.dedup();
    Ok(out)
}

fn walk(dir: &Path, segs: &[&str], out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Some((first, rest)) = segs.split_first() else {
        return;
    };
    let entries = |d: &Path| -> Vec<(String, PathBuf)> {
        let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(d)
            .map(|rd| {
                rd.flatten()
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    if *first == "**" {
        if rest.is_empty() {
            // `dir/**` is every file below.
            walk(dir, &["*"], out, depth + 1);
            for (_, p) in entries(dir).into_iter().filter(|(_, p)| p.is_dir()) {
                walk(&p, segs, out, depth + 1);
            }
            return;
        }
        walk(dir, rest, out, depth + 1);
        for (_, p) in entries(dir).into_iter().filter(|(_, p)| p.is_dir()) {
            walk(&p, segs, out, depth + 1);
        }
        return;
    }
    if has_wildcard(first) {
        for (name, p) in entries(dir) {
            if !segment_matches(first, &name) {
                continue;
            }
            if rest.is_empty() {
                if p.is_file() {
                    out.push(p);
                }
            } else if p.is_dir() {
                walk(&p, rest, out, depth + 1);
            }
        }
        return;
    }
    let p = dir.join(first);
    if rest.is_empty() {
        if p.is_file() {
            out.push(p);
        }
    } else if p.is_dir() {
        walk(&p, rest, out, depth + 1);
    }
}

fn resolve(base: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// What `[rtl]` resolves to.
#[derive(Clone, Debug, Default)]
pub struct RtlResolved {
    /// The user's source files in order (patterns expanded, duplicates dropped).
    pub files: Vec<PathBuf>,
    /// The Vivado export, if `export_dir` or `filelist` is set.
    pub export: Option<ExportDesign>,
    /// The unit to elaborate: `rtl.top`, else the export's single top.
    pub top: Option<String>,
    /// Everything wrong or unsupported, with the project field it belongs to.
    pub diagnostics: Vec<Diagnostic>,
}

impl RtlResolved {
    /// The error diagnostics joined into one message (`None` when there are none).
    pub fn first_error(&self) -> Option<String> {
        let e: Vec<String> = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}: {}", d.field, d.message))
            .collect();
        (!e.is_empty()).then(|| e.join("; "))
    }
}

/// Variable lookup for the export lists: the process environment, and `XILINX_VIVADO` from the
/// detected install (`ZYNQSIM_VIVADO` included) when it is not set.
fn env_lookup(name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(name) {
        return Some(v);
    }
    if name == "XILINX_VIVADO" {
        let root = std::env::var_os("ZYNQSIM_VIVADO")
            .map(PathBuf::from)
            .or_else(crate::tools::find_vivado)?;
        return Some(root.display().to_string());
    }
    None
}

/// Reads an export (folder, `.prj`, `.f`, ...) with Vivado variables resolved.
pub fn read_export(path: &Path) -> Result<ExportDesign, String> {
    xsim_export::Reader::with_env(env_lookup).read(path)
}

fn diag(sev: Severity, field: &str, message: String) -> Diagnostic {
    Diagnostic {
        severity: sev,
        field: field.to_string(),
        message,
    }
}

/// Resolves `[rtl]`: globs, the export, the top. Never touches the simulator.
pub fn resolve_rtl(r: &RtlSection, base: &Path) -> RtlResolved {
    let mut out = RtlResolved::default();
    for f in &r.files {
        if has_wildcard(f) {
            match glob(base, f) {
                Ok(v) if v.is_empty() => out.diagnostics.push(diag(
                    Severity::Error,
                    "rtl.files",
                    format!("pattern `{f}` matches no files (relative to the project file)"),
                )),
                Ok(v) => {
                    for p in v {
                        if !out.files.contains(&p) {
                            out.files.push(p);
                        }
                    }
                }
                Err(e) => out.diagnostics.push(diag(Severity::Error, "rtl.files", e)),
            }
        } else {
            let p = resolve(base, f);
            if !p.is_file() {
                out.diagnostics.push(diag(
                    Severity::Error,
                    "rtl.files",
                    format!("file `{f}` does not exist"),
                ));
            }
            if !out.files.contains(&p) {
                out.files.push(p);
            }
        }
    }
    // VHDL in `files` needs the per-language compile of the export path (an export with no
    // lists of its own).
    if out
        .files
        .iter()
        .any(|p| xsim_export::Lang::from_path(p).is_some_and(xsim_export::Lang::is_vhdl))
        && r.export_dir.is_none()
        && r.filelist.is_none()
    {
        out.export = Some(ExportDesign::default());
    }
    let (field, path) = match (&r.export_dir, &r.filelist) {
        (Some(_), Some(_)) => {
            out.diagnostics.push(diag(
                Severity::Error,
                "rtl.export_dir",
                "`export_dir` and `filelist` are alternatives: set one of them".to_string(),
            ));
            return out;
        }
        (Some(p), None) => ("rtl.export_dir", Some(p)),
        (None, Some(p)) => ("rtl.filelist", Some(p)),
        (None, None) => ("", None),
    };
    if let Some(p) = path {
        match read_export(&resolve(base, p)) {
            Err(e) => out.diagnostics.push(diag(Severity::Error, field, e)),
            Ok(design) => {
                for d in &design.diagnostics {
                    let sev = match d.severity {
                        xsim_export::Severity::Error => Severity::Error,
                        xsim_export::Severity::Warning => Severity::Warning,
                    };
                    out.diagnostics.push(diag(sev, field, d.to_string()));
                }
                out.export = Some(design);
            }
        }
    }
    out.top = r.top.clone();
    if out.top.is_none()
        && let Some(e) = &out.export
    {
        let tops = e.top_modules();
        match tops.as_slice() {
            [one] => out.top = Some(one.clone()),
            [] => out.diagnostics.push(diag(
                Severity::Error,
                "rtl.top",
                "the export names no top unit (no elab.opt/script with one); set `rtl.top`"
                    .to_string(),
            )),
            many => out.diagnostics.push(diag(
                Severity::Error,
                "rtl.top",
                format!(
                    "the export names several top units ({}); set `rtl.top`",
                    many.join(", ")
                ),
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(n: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!("zs-glob-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn touch(&self, rel: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(base: &Path, v: Vec<PathBuf>) -> Vec<String> {
        v.iter()
            .map(|p| {
                p.strip_prefix(base)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn segment_patterns() {
        assert!(segment_matches("*.v", "a.v"));
        assert!(!segment_matches("*.v", "a.sv"));
        assert!(segment_matches("a?.v", "ab.v"));
        assert!(segment_matches("m[0-3].v", "m2.v"));
        assert!(!segment_matches("m[0-3].v", "m7.v"));
        assert!(segment_matches("m[!0-3].v", "m7.v"));
        assert!(segment_matches("*", "anything"));
        assert!(segment_matches("a*b*c", "aXXbYYc"));
        assert!(!segment_matches("a*b*c", "aXXbYY"));
        assert!(segment_matches("[[]x", "[x"));
    }

    #[test]
    fn globs_expand_sorted_recursive_and_only_to_files() {
        let t = Tmp::new("g");
        for f in [
            "rtl/b.v",
            "rtl/a.v",
            "rtl/c.sv",
            "rtl/sub/d.v",
            "rtl/sub/deep/e.v",
            "rtl/dir.v/x.txt",
            "other/f.v",
        ] {
            t.touch(f);
        }
        let g = |p: &str| names(&t.0, glob(&t.0, p).unwrap());
        assert_eq!(g("rtl/*.v"), ["rtl/a.v", "rtl/b.v"], "sorted, files only");
        assert_eq!(g("rtl/*.*v"), ["rtl/a.v", "rtl/b.v", "rtl/c.sv"]);
        assert_eq!(
            g("rtl/**/*.v"),
            ["rtl/a.v", "rtl/b.v", "rtl/sub/d.v", "rtl/sub/deep/e.v"]
        );
        assert_eq!(g("*/f.v"), ["other/f.v"]);
        assert_eq!(g("rtl/su?/*.v"), ["rtl/sub/d.v"]);
        assert_eq!(g("rtl/./a.v"), ["rtl/./a.v"], "no wildcard: as written");
        assert!(g("rtl/*.vhd").is_empty());
        // Backslashes work too (Windows project files).
        assert_eq!(g("rtl\\*.sv"), ["rtl/c.sv"]);
        // Absolute patterns.
        let abs = format!("{}/rtl/s*.v", t.0.display().to_string().replace('\\', "/"));
        assert_eq!(glob(Path::new("elsewhere"), &abs).unwrap().len(), 0);
        let abs = format!("{}/rtl/?.v", t.0.display().to_string().replace('\\', "/"));
        assert_eq!(glob(Path::new("elsewhere"), &abs).unwrap().len(), 2);
    }

    #[test]
    fn resolve_rtl_expands_patterns_dedupes_and_names_empty_ones() {
        let t = Tmp::new("r");
        t.touch("rtl/a.v");
        t.touch("rtl/b.v");
        let r = RtlSection {
            files: vec!["rtl/*.v".into(), "rtl/a.v".into(), "nothing/*.sv".into()],
            top: Some("a".into()),
            ..RtlSection::default()
        };
        let x = resolve_rtl(&r, &t.0);
        assert_eq!(names(&t.0, x.files.clone()), ["rtl/a.v", "rtl/b.v"]);
        let e = x.first_error().unwrap();
        assert!(
            e.contains("nothing/*.sv") && e.contains("matches no files"),
            "{e}"
        );
    }

    #[test]
    fn export_and_filelist_are_alternatives() {
        let r = RtlSection {
            export_dir: Some("a".into()),
            filelist: Some("b.f".into()),
            ..RtlSection::default()
        };
        let x = resolve_rtl(&r, Path::new("."));
        assert!(x.first_error().unwrap().contains("alternatives"));
    }
}
