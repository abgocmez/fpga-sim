//! Tokenising of the line-oriented list formats (`.prj`, `.f`, `elab.opt`, shell scripts).

use std::path::{Path, PathBuf};

/// One token of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tok {
    /// The text with quotes removed and variables expanded.
    pub text: String,
    /// It was written inside quotes (a library name is never quoted, a path often is).
    pub quoted: bool,
}

/// Splits text into logical lines: a trailing backslash joins the next line. Each line carries
/// the 1-based number of its first physical line.
pub(crate) fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        if cur.is_empty() {
            start = i + 1;
        }
        let trimmed = line.trim_end();
        if let Some(head) = trimmed.strip_suffix('\\') {
            // A continuation, unless the backslash closes a Windows path such as `"C:\dir\"`
            // (then the quote follows it, so it is not the last character).
            cur.push_str(head);
            cur.push(' ');
            continue;
        }
        cur.push_str(line);
        out.push((start, std::mem::take(&mut cur)));
    }
    if !cur.is_empty() {
        out.push((start, cur));
    }
    out
}

/// Expands `$NAME`, `${NAME}`, `$(NAME)`, `$::env(NAME)` and `%NAME%` with `lookup`; names it
/// cannot resolve stay in the text and are pushed to `unresolved`.
pub(crate) fn expand(
    s: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    unresolved: &mut Vec<String>,
) -> String {
    if !s.contains('$') && !s.contains('%') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let is_id = |c: char| c.is_ascii_alphanumeric() || c == '_';
    while i < chars.len() {
        let c = chars[i];
        let mut take = |name: String, end: usize, i: &mut usize, out: &mut String| {
            match lookup(&name) {
                Some(v) => out.push_str(&v),
                None => {
                    unresolved.push(name);
                    out.extend(&chars[*i..end]);
                }
            }
            *i = end;
        };
        if c == '$' && i + 1 < chars.len() {
            let n = chars[i + 1];
            if n == '{' || n == '(' {
                let close = if n == '{' { '}' } else { ')' };
                if let Some(rel) = chars[i + 2..].iter().position(|&x| x == close) {
                    let name: String = chars[i + 2..i + 2 + rel].iter().collect();
                    take(name, i + 2 + rel + 1, &mut i, &mut out);
                    continue;
                }
            } else if is_id(n) {
                let mut j = i + 1;
                while j < chars.len() && is_id(chars[j]) {
                    j += 1;
                }
                let name: String = chars[i + 1..j].iter().collect();
                take(name, j, &mut i, &mut out);
                continue;
            } else if n == ':' {
                // `$::env(NAME)`
                let rest: String = chars[i..].iter().collect();
                if let Some(r) = rest.strip_prefix("$::env(")
                    && let Some(end) = r.find(')')
                {
                    let name = r[..end].to_string();
                    let len = "$::env(".len() + end + 1;
                    take(name, i + len, &mut i, &mut out);
                    continue;
                }
            }
        } else if c == '%'
            && let Some(rel) = chars[i + 1..].iter().position(|&x| x == '%')
            && rel > 0
            && chars[i + 1..i + 1 + rel].iter().all(|&x| is_id(x))
        {
            let name: String = chars[i + 1..i + 1 + rel].iter().collect();
            take(name, i + 1 + rel + 1, &mut i, &mut out);
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Splits one logical line into tokens. A comment (`#`, `//`, or `/* ... */` on the line) is
/// recognised only at the start of a token, so `a//b` and URLs inside quotes survive.
pub(crate) fn tokenize(
    line: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    unresolved: &mut Vec<String>,
) -> Vec<Tok> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == '#' || (chars[i] == '/' && chars.get(i + 1) == Some(&'/')) {
            break;
        }
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            let mut j = i + 2;
            while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == '/') {
                j += 1;
            }
            i = (j + 2).min(chars.len());
            continue;
        }
        let mut text = String::new();
        let mut quoted = false;
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            match quote {
                Some(q) => {
                    if c == q {
                        quote = None;
                    } else {
                        text.push(c);
                    }
                }
                None if (c == '"' || c == '\'') && (text.is_empty() || c == '"') => {
                    quote = Some(c);
                    quoted = true;
                }
                None if c.is_whitespace() => break,
                None => text.push(c),
            }
            i += 1;
        }
        out.push(Tok {
            text: expand(&text, lookup, unresolved),
            quoted,
        });
    }
    out
}

/// `true` for `C:/x`, `C:\x`, `/x`, `\\server\x`.
pub(crate) fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/')
        || p.starts_with('\\')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\'))
}

/// Joins `p` onto `base` (unless absolute) and removes `.` and `..` lexically; the result
/// uses forward slashes, which every consumer here (xvlog, cmd, Rust on Windows) accepts.
pub(crate) fn resolve(base: &Path, p: &str) -> PathBuf {
    let p = p.replace('\\', "/");
    let joined = if is_absolute(&p) {
        p
    } else {
        format!("{}/{}", base.display().to_string().replace('\\', "/"), p)
    };
    let mut parts: Vec<&str> = Vec::new();
    let mut prefix = String::new();
    let mut rest = joined.as_str();
    if let Some(r) = rest.strip_prefix("//") {
        prefix.push_str("//");
        rest = r;
    } else if rest.len() >= 2 && rest.as_bytes()[1] == b':' {
        prefix.push_str(&rest[..2]);
        rest = &rest[2..];
    }
    let abs = rest.starts_with('/');
    if abs {
        prefix.push('/');
    }
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !abs {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    PathBuf::from(format!("{prefix}{}", parts.join("/")))
}
