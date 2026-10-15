//! Reads the file lists Vivado's `export_simulation` writes and plain `.f` lists.

use serde::Serialize;

/// One file named by a list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileListEntry {
    /// The path as written (may be relative to the list).
    pub path: String,
    /// `verilog`, `sv`, `vhdl` for a `.prj`; `None` in a `.f` file.
    pub language: Option<String>,
    /// The library (`xil_defaultlib`) of a `.prj` entry.
    pub library: Option<String>,
}

/// Splits a line into whitespace-separated tokens, honouring double quotes.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Parses a Vivado `.prj` (`verilog LIB "file"`, `sv LIB "file"`, `vhdl LIB "file"`, `nosort`,
/// `-i dir`, `-d DEF`) or a plain `.f` file list (one path per line, `//` and `#` comments,
/// `+incdir+`/`+define+`/`-v`-style options skipped). `.prj` is chosen when a line starts with
/// one of the language words.
pub fn parse_filelist(text: &str) -> Vec<FileListEntry> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw
            .split("//")
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        if line.is_empty() {
            continue;
        }
        let t = tokens(line);
        let Some(first) = t.first() else { continue };
        let lang = first.to_ascii_lowercase();
        if matches!(
            lang.as_str(),
            "verilog" | "sv" | "systemverilog" | "vhdl" | "vhdl2008"
        ) {
            let library = t.get(1).cloned();
            let mut skip_next = false;
            for p in t.iter().skip(2) {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if p.starts_with('-') {
                    // `-i dir`, `-d DEF`, `-L lib` take a value.
                    skip_next = p.len() == 2;
                    continue;
                }
                out.push(FileListEntry {
                    path: p.clone(),
                    language: Some(lang.clone()),
                    library: library.clone(),
                });
            }
        } else if first.starts_with('+') || first.starts_with('-') || lang == "nosort" {
            continue;
        } else {
            for p in &t {
                out.push(FileListEntry {
                    path: p.clone(),
                    language: None,
                    library: None,
                });
            }
        }
    }
    out
}
