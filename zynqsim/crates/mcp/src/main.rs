//! `zynqsim-mcp`: the ZynqSim MCP server over stdio.

use std::io::{self, BufReader};
use std::path::PathBuf;
use zynqsim_mcp::{McpServer, serve};

const USAGE: &str = "\
zynqsim-mcp [--root DIR]... [--project FILE]

Serves the Model Context Protocol on stdin/stdout.

  --root DIR       allow file access under DIR (repeatable; default: current directory)
  --project FILE   open this project.toml before serving
  -h, --help       show this help
";

fn main() {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut project: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => match args.next() {
                Some(d) => roots.push(PathBuf::from(d)),
                None => die("--root needs a directory"),
            },
            "--project" => match args.next() {
                Some(f) => project = Some(f),
                None => die("--project needs a file"),
            },
            "-h" | "--help" => {
                print!("{USAGE}");
                return;
            }
            other => die(&format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    if roots.is_empty() {
        roots.push(std::env::current_dir().unwrap_or_else(|e| die(&e.to_string())));
    }

    let mut server = McpServer::new(roots).unwrap_or_else(|e| die(&e.to_string()));
    if let Some(p) = project {
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": { "name": "project_open", "arguments": { "path": p } }
        });
        let reply = server.handle(req, &mut zynqsim_mcp::NoControl);
        let failed = reply
            .as_ref()
            .and_then(|r| r.pointer("/result/isError"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        if failed {
            die(&format!("cannot open project {p}: {reply:?}"));
        }
    }

    let stdin = BufReader::new(io::stdin());
    if let Err(e) = serve(&mut server, stdin, io::stdout().lock()) {
        die(&format!("transport error: {e}"));
    }
}

fn die(msg: &str) -> ! {
    eprintln!("zynqsim-mcp: {msg}");
    std::process::exit(2);
}
