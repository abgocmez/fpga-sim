//! Reading Verilator's messages.
//!
//! Verilator prints one line per message, `%Error: file:line:col: text`,
//! `%Error-UNSUPPORTED: file:line:col: Unsupported: text` or
//! `%Warning-CODE: file:line:col: text`, followed by a source excerpt
//! (`    5 |     $display("x");`) and a caret line. Summary lines such as
//! `%Error: Exiting due to 1 error(s)` and `%Error: 'make ...' exited with 2`
//! name no location. The formats below were captured from Verilator 5.053.

use std::fmt;

/// How serious a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// `%Warning`: the design is built anyway (`-Wno-fatal`).
    Warning,
    /// `%Error`: the build fails.
    Error,
}

/// One message of Verilator or of the build step it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// The code after the dash (`UNSUPPORTED`, `SPECIFYIGN`, `SYNTAX`), if printed.
    pub code: Option<String>,
    /// The text after the location.
    pub message: String,
    /// The file as Verilator printed it (forward slashes when given so).
    pub file: Option<String>,
    /// 1-based line.
    pub line: Option<u32>,
    /// The source line the message points at (from Verilator's excerpt), or else the message.
    pub construct: String,
}

impl Diagnostic {
    /// `true` for `%Error` messages.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The final path component of the file, for compact output.
    pub fn file_name(&self) -> Option<&str> {
        self.file
            .as_deref()
            .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        match (&self.file, self.line) {
            (Some(file), Some(line)) => write!(f, "{file}:{line}: ")?,
            (Some(file), None) => write!(f, "{file}: ")?,
            _ => {}
        }
        write!(f, "verilator {sev}")?;
        if let Some(c) = &self.code {
            write!(f, " [{c}]")?;
        }
        write!(f, ": {}", self.message)?;
        if !self.construct.is_empty() && self.construct != self.message {
            write!(f, " (`{}`)", self.construct)?;
        }
        Ok(())
    }
}

/// Parses every `%Error` / `%Warning` message in `text`, in order.
pub fn parse_messages(text: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let mut out: Vec<Diagnostic> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(body) = line.strip_prefix('%') else {
            continue;
        };
        let (severity, rest) = if let Some(r) = body.strip_prefix("Error") {
            (Severity::Error, r)
        } else if let Some(r) = body.strip_prefix("Warning") {
            (Severity::Warning, r)
        } else {
            continue;
        };
        // `-CODE` is optional, then `:`.
        let (code, rest) = match rest.strip_prefix('-') {
            Some(r) => match r.split_once(':') {
                Some((c, tail)) => (Some(c.to_string()), tail),
                None => continue,
            },
            None => match rest.strip_prefix(':') {
                Some(tail) => (None, tail),
                None => continue,
            },
        };
        let rest = rest.trim_start();
        let (file, line_no, col_text_end) = match locate(rest) {
            Some(l) => l,
            None => {
                // Summary or tool message without a location.
                if rest.starts_with("Exiting due to") {
                    continue;
                }
                out.push(Diagnostic {
                    severity,
                    code,
                    message: rest.to_string(),
                    file: None,
                    line: None,
                    construct: rest.to_string(),
                });
                continue;
            }
        };
        let message = rest[col_text_end..].trim().to_string();
        let construct = excerpt_after(&lines, i, line_no).unwrap_or_else(|| message.clone());
        out.push(Diagnostic {
            severity,
            code,
            message,
            file: Some(file),
            line: Some(line_no),
            construct,
        });
    }
    out
}

/// Splits `file:line:col: text` into (file, line, index of `text`). The file may contain
/// `:` (a drive letter), so the first `:digits:digits: ` after a colon wins.
fn locate(s: &str) -> Option<(String, u32, usize)> {
    for (pos, _) in s.match_indices(':') {
        let after = &s[pos + 1..];
        let Some((line_digits, rest)) = split_digits(after) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let (_, rest) = split_digits(rest).unwrap_or(("", rest));
        let Some(text) = rest.strip_prefix(": ").or_else(|| rest.strip_prefix(':')) else {
            continue;
        };
        let Ok(line) = line_digits.parse::<u32>() else {
            continue;
        };
        let text_start = s.len() - text.len();
        return Some((s[..pos].to_string(), line, text_start));
    }
    None
}

/// Leading ASCII digits and the rest; `None` when there are no digits.
fn split_digits(s: &str) -> Option<(&str, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (n > 0).then(|| (&s[..n], &s[n..]))
}

/// The source text of line `line` from the excerpt Verilator prints after the message at
/// `at` (`   N |  source`), if that excerpt is there.
fn excerpt_after(lines: &[&str], at: usize, line: u32) -> Option<String> {
    for l in lines.iter().skip(at + 1).take(4) {
        let t = l.trim_start();
        let (num, rest) = match t.split_once('|') {
            Some(p) => p,
            None => continue,
        };
        if num.trim().parse::<u32>().ok() == Some(line) {
            let src = rest.trim();
            return (!src.is_empty()).then(|| src.to_string());
        }
    }
    None
}

/// The `%Error` messages of `text`.
pub fn errors(text: &str) -> Vec<Diagnostic> {
    parse_messages(text)
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from Verilator 5.053 (`verilator_bin --binary --timing`).
    const SYNTAX: &str = "%Error: bad1.sv:5:5: syntax error, unexpected $display, expecting ';'\n\
        \x20   5 |     $display(\"x\");\n\
        \x20     |     ^~~~~~~~\n\
        \x20       ... See the manual at https://verilator.org/verilator_doc.html?v=5.053 for more assistance.\n\
        %Error: Exiting due to 1 error(s)\n";

    const SPECIFY: &str = "%Warning-SPECIFYIGN: bad2.sv:4:22: Ignoring unsupported: specify block construct\n\
        \x20   4 |     (w[0] => w[1]) = 1;\n\
        \x20     |                      ^\n\
        \x20                    ... For warning description see https://verilator.org/warn/SPECIFYIGN?v=5.053\n";

    const INTERFACE: &str = "%Error-UNSUPPORTED: bad3.sv:5:3: Unsupported: interface decls within module decls\n\
        \x20   5 |   bus_if b();\n\
        \x20     |   ^~~~~~\n\
        %Error: Exiting due to 1 error(s)\n";

    const MAKE_FAILED: &str = "%Error: 'make -C obj -f Vtop.mk -j 1' exited with 2\n";

    #[test]
    fn a_syntax_error_has_file_line_and_the_source_text() {
        let d = parse_messages(SYNTAX);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].code, None);
        assert_eq!(d[0].file.as_deref(), Some("bad1.sv"));
        assert_eq!(d[0].line, Some(5));
        assert_eq!(
            d[0].message,
            "syntax error, unexpected $display, expecting ';'"
        );
        assert_eq!(d[0].construct, "$display(\"x\");");
        assert_eq!(d[0].file_name(), Some("bad1.sv"));
    }

    #[test]
    fn an_unsupported_construct_is_an_error_with_its_code() {
        let d = parse_messages(INTERFACE);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code.as_deref(), Some("UNSUPPORTED"));
        assert!(d[0].is_error());
        assert_eq!(d[0].line, Some(5));
        assert_eq!(
            d[0].message,
            "Unsupported: interface decls within module decls"
        );
        assert_eq!(d[0].construct, "bus_if b();");
    }

    #[test]
    fn an_ignored_construct_is_only_a_warning() {
        let d = parse_messages(SPECIFY);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].code.as_deref(), Some("SPECIFYIGN"));
        assert_eq!(d[0].line, Some(4));
        assert!(!d[0].is_error());
        assert_eq!(errors(SPECIFY).len(), 0);
    }

    #[test]
    fn summaries_without_a_location_are_kept_but_the_exit_line_is_not_a_message() {
        let d = parse_messages(MAKE_FAILED);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, None);
        assert!(d[0].message.contains("exited with 2"));
        let exiting = parse_messages(SYNTAX);
        assert!(exiting.iter().all(|m| !m.message.contains("Exiting")));
    }

    #[test]
    fn windows_drive_letters_are_part_of_the_file_name() {
        let d = parse_messages("%Error: C:\\work\\top.sv:12:3: Unsupported: class declarations\n");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].file.as_deref(), Some("C:\\work\\top.sv"));
        assert_eq!(d[0].line, Some(12));
        assert_eq!(d[0].message, "Unsupported: class declarations");
    }

    #[test]
    fn display_names_file_line_and_construct() {
        let d = parse_messages(INTERFACE);
        let s = d[0].to_string();
        assert!(
            s.starts_with("bad3.sv:5: verilator error [UNSUPPORTED]"),
            "{s}"
        );
        assert!(s.contains("`bus_if b();`"), "{s}");
    }
}
