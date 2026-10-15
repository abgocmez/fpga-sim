use crate::server::{Control, McpServer};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver};

/// Control implementation over the stdio transport: writes progress
/// notifications and scans queued input for cancellations.
struct StdioControl<'a, W: Write> {
    out: &'a mut W,
    rx: &'a Receiver<String>,
    pending: &'a mut VecDeque<String>,
}

impl<W: Write> Control for StdioControl<'_, W> {
    fn progress(&mut self, token: &Value, done: f64, total: f64) {
        let note = json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": { "progressToken": token, "progress": done, "total": total }
        });
        let _ = writeln!(self.out, "{note}");
        let _ = self.out.flush();
    }

    fn cancelled(&mut self, request_id: &Value) -> bool {
        let mut hit = false;
        while let Ok(line) = self.rx.try_recv() {
            let is_cancel = serde_json::from_str::<Value>(&line).is_ok_and(|v| {
                v.get("method").and_then(Value::as_str) == Some("notifications/cancelled")
                    && v.get("params").and_then(|p| p.get("requestId")) == Some(request_id)
            });
            if is_cancel {
                hit = true;
            } else {
                self.pending.push_back(line);
            }
        }
        hit
    }
}

/// Runs the server over a line-oriented transport until `input` reaches EOF.
///
/// A reader thread feeds lines into a channel so that cancellation requests
/// can be noticed while a long tool call is in progress.
pub fn serve<R, W>(server: &mut McpServer, input: R, mut output: W) -> io::Result<()>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("zynqsim-mcp-reader".to_string())
        .spawn(move || {
            for line in input.lines() {
                match line {
                    Ok(l) if l.trim().is_empty() => {}
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })?;

    let mut pending: VecDeque<String> = VecDeque::new();
    loop {
        let line = match pending.pop_front() {
            Some(l) => l,
            None => match rx.recv() {
                Ok(l) => l,
                Err(_) => return Ok(()),
            },
        };

        let response = match serde_json::from_str::<Value>(&line) {
            Err(e) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": format!("parse error: {e}") }
            })),
            Ok(Value::Array(_)) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32600, "message": "batch requests are not supported" }
            })),
            Ok(msg) => {
                let mut ctl = StdioControl {
                    out: &mut output,
                    rx: &rx,
                    pending: &mut pending,
                };
                server.handle(msg, &mut ctl)
            }
        };
        if let Some(r) = response {
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
}
