//! `source_line`, the source context of `cpu_regs`, and `file.rs:LINE`
//! breakpoints (ADR-013).

use super::{McpServer, Out, str_arg, unexpected};
use serde_json::{Value, json};
use sim_api::{ApiError, BreakKind, Command, Query, Response};

/// Lines of context on each side when the caller does not say.
const DEFAULT_CONTEXT: u64 = 3;
/// Most context lines a caller may ask for on each side.
const MAX_CONTEXT: u64 = 50;

/// `a` as `file.rs:LINE`: the part after the last colon is the line.
fn split_location(t: &str) -> Option<(&str, u32)> {
    let (file, line) = t.rsplit_once(':')?;
    let line: u32 = line.trim().parse().ok()?;
    (!file.trim().is_empty() && line > 0).then_some((file.trim(), line))
}

impl McpServer {
    /// The numbered lines around `line` of `file`, or why they are not
    /// available. `(resolved path, lines)`.
    pub(super) fn source_window(
        &mut self,
        file: &str,
        line: u32,
        context: u64,
    ) -> Result<(String, Vec<Value>), String> {
        match self.client.query(Query::SourceFile {
            path: file.to_string(),
        }) {
            Ok(Response::SourceFile { text, resolved, .. }) => {
                let from = u64::from(line).saturating_sub(context).max(1);
                let to = u64::from(line) + context;
                let lines = text
                    .lines()
                    .enumerate()
                    .map(|(i, l)| (i as u64 + 1, l))
                    .filter(|(n, _)| (from..=to).contains(n))
                    .map(|(n, l)| json!({ "line": n, "text": l, "current": n == u64::from(line) }))
                    .collect();
                Ok((resolved, lines))
            }
            Ok(r) => Err(format!("unexpected response {r:?}")),
            Err(e) => Err(e.message),
        }
    }

    /// Source position of an address, a symbol or a `file.rs:LINE`, with a few
    /// lines of the file around it.
    pub(super) fn source_line(&mut self, a: &Value) -> Result<Out, ApiError> {
        let context = a
            .get("context")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_CONTEXT)
            .min(MAX_CONTEXT);
        let addr = if let Some(addr) = a.get("addr").and_then(Value::as_u64) {
            addr
        } else if let Some(sym) = a.get("symbol").and_then(Value::as_str) {
            match self.client.query(Query::Symbol {
                query: sym.to_string(),
            })? {
                Response::Symbol(s) => s.addr + s.offset,
                r => return Err(unexpected(&r)),
            }
        } else if let Some(loc) = a.get("location").and_then(Value::as_str) {
            let (file, line) = split_location(loc).ok_or_else(|| {
                ApiError::invalid_argument("`location` must look like `main.rs:42`")
            })?;
            match self.client.query(Query::SourceAddrs {
                file: file.to_string(),
                line,
            })? {
                Response::SourceAddrs { addrs, .. } => addrs
                    .first()
                    .copied()
                    .ok_or_else(|| ApiError::not_found(format!("no code at {loc}")))?,
                r => return Err(unexpected(&r)),
            }
        } else {
            return Err(ApiError::invalid_argument(
                "give `addr`, `symbol` or `location` (file.rs:LINE)",
            ));
        };
        let l = match self.client.query(Query::SourceLine { addr })? {
            Response::SourceLine(l) => l,
            r => return Err(unexpected(&r)),
        };
        let mut out = json!({
            "addr": addr,
            "addr_hex": format!("{addr:#x}"),
            "file": l.file,
            "line": l.line,
            "column": l.column,
            "function": l.function,
        });
        match self.source_window(&l.file, l.line, context) {
            Ok((resolved, lines)) => {
                out["resolved_path"] = json!(resolved);
                out["lines"] = json!(lines);
            }
            Err(why) => out["source_unavailable"] = json!(why),
        }
        Ok(Out::json(out))
    }

    /// `breakpoint_set` with a `location` (`file.rs:LINE`, a symbol or an
    /// address) as an alternative to `addr`. One source line can start code in
    /// several places; a breakpoint is set at each.
    pub(super) fn breakpoint_at_location(&mut self, a: &Value) -> Result<Out, ApiError> {
        let loc = str_arg(a, "location")?.trim().to_string();
        let enable = a.get("enable").and_then(Value::as_bool).unwrap_or(true);
        let addrs: Vec<u64> =
            if let Some((file, line)) = split_location(&loc) {
                match self.client.query(Query::SourceAddrs {
                    file: file.to_string(),
                    line,
                })? {
                    Response::SourceAddrs { addrs, .. } => addrs,
                    r => return Err(unexpected(&r)),
                }
            } else if let Some(h) = loc.strip_prefix("0x").or_else(|| loc.strip_prefix("0X")) {
                vec![u64::from_str_radix(h, 16).map_err(|_| {
                    ApiError::invalid_argument(format!("`{loc}` is not a hex address"))
                })?]
            } else {
                match self.client.query(Query::Symbol { query: loc.clone() })? {
                    Response::Symbol(s) => vec![s.addr],
                    r => return Err(unexpected(&r)),
                }
            };
        let mut set = Vec::new();
        for addr in &addrs {
            self.client.command(Command::SetBreakpoint {
                addr: *addr,
                kind: BreakKind::Exec,
                len: 4,
                enable,
            })?;
            let src = match self.client.query(Query::SourceLine { addr: *addr }) {
                Ok(Response::SourceLine(l)) => Some(json!({ "file": l.file, "line": l.line })),
                _ => None,
            };
            set.push(json!({ "addr": addr, "addr_hex": format!("{addr:#x}"), "source": src }));
        }
        Ok(Out::json(json!({
            "location": loc,
            "kind": BreakKind::Exec,
            "set": enable,
            "breakpoints": set,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::split_location;

    #[test]
    fn locations_split_at_the_last_colon() {
        assert_eq!(split_location("main.rs:42"), Some(("main.rs", 42)));
        assert_eq!(
            split_location(r"C:\src\main.rs:7"),
            Some((r"C:\src\main.rs", 7))
        );
        assert_eq!(split_location("main"), None);
        assert_eq!(split_location("main.rs:0"), None);
        assert_eq!(split_location(":3"), None);
        assert_eq!(split_location("0x100614"), None);
    }
}
