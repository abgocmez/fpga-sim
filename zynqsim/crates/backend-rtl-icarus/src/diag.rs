//! Reading Icarus Verilog's messages.
//!
//! `iverilog` prints one line per message: `file:line: error: text` or
//! `file:line: warning: text`; a message without a location is `error: text`. Continuation
//! lines of the form `file:line:      : text` repeat the location of the message before them;
//! they are skipped. The formats were captured from Icarus Verilog 14.0 (devel).

use std::fmt;

/// How serious a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// `warning:`: the design is built anyway.
    Warning,
    /// `error:`: the build fails.
    Error,
}

/// One message of Icarus Verilog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// The file as Icarus printed it, when the message has a location.
    pub file: Option<String>,
    /// 1-based line.
    pub line: Option<u32>,
    /// The construct the message names: the text after the location.
    pub construct: String,
    /// The same text (kept for callers that print messages).
    pub message: String,
}

impl Diagnostic {
    /// `true` for `error:` messages.
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
            (Some(file), Some(line)) => write!(f, "{file}:{line}: {sev}: ")?,
            _ => write!(f, "{sev}: ")?,
        }
        write!(f, "{}", self.message)
    }
}

/// Every message in an `iverilog` log, in order.
pub fn parse_messages(log: &str) -> Vec<Diagnostic> {
    log.lines().filter_map(parse_line).collect()
}

/// The `error` messages of a log.
pub fn errors(log: &str) -> Vec<Diagnostic> {
    parse_messages(log)
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect()
}

fn parse_line(line: &str) -> Option<Diagnostic> {
    let (prefix, severity, text) = if let Some(i) = line.find(": error: ") {
        (Some(&line[..i]), Severity::Error, &line[i + ": error: ".len()..])
    } else if let Some(i) = line.find(": warning: ") {
        (
            Some(&line[..i]),
            Severity::Warning,
            &line[i + ": warning: ".len()..],
        )
    } else if let Some(t) = line.strip_prefix("error: ") {
        (None, Severity::Error, t)
    } else if let Some(t) = line.strip_prefix("warning: ") {
        (None, Severity::Warning, t)
    } else {
        return None;
    };
    let (file, lineno) = match prefix.and_then(|p| p.rsplit_once(':')) {
        Some((file, n)) => match n.trim().parse::<u32>() {
            Ok(n) => (Some(file.to_string()), Some(n)),
            Err(_) => (None, None),
        },
        None => (None, None),
    };
    // A continuation line (`file:9:      : text`) has no message of its own.
    if file.is_none() && prefix.is_some() {
        return None;
    }
    Some(Diagnostic {
        severity,
        file,
        line: lineno,
        construct: text.trim().to_string(),
        message: text.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_located_error_gives_file_line_and_text() {
        let log = "t.sv:6: error: Function t.rp_next port addr is not an input port.\n\
                   t.sv:6:      : Function arguments must be input ports.\n\
                   4 error(s) during elaboration.\n";
        let d = errors(log);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].file.as_deref(), Some("t.sv"));
        assert_eq!(d[0].line, Some(6));
        assert!(d[0].construct.contains("is not an input port"));
    }

    #[test]
    fn a_windows_path_keeps_its_drive_letter() {
        let log = "C:\\work\\t.sv:12: error: Unable to bind wire/reg/memory `x'\n";
        let d = errors(log);
        assert_eq!(d[0].file.as_deref(), Some("C:\\work\\t.sv"));
        assert_eq!(d[0].line, Some(12));
        assert_eq!(d[0].file_name(), Some("t.sv"));
    }

    #[test]
    fn a_message_without_a_location_and_a_warning_are_kept_apart() {
        let log = "error: Failed to link 'zynqsim.vpi'. Try rebuilding it with iverilog-vpi.\n\
                   a.v:3: warning: implicit definition of wire x.\n";
        let all = parse_messages(log);
        assert_eq!(all.len(), 2);
        assert!(all[0].is_error() && all[0].file.is_none());
        assert_eq!(all[1].severity, Severity::Warning);
        assert_eq!(errors(log).len(), 1);
    }

    #[test]
    fn the_summary_and_the_vpi_load_notes_are_not_errors() {
        let log = "warning: 'x.vpi' has no vpip_set_callback()\nFailed to link VPI module z\n";
        assert!(errors(log).is_empty());
    }
}
