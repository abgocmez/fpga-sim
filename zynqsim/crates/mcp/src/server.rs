use crate::schema;
use crate::tools::{Gate, ToolDef, tool_defs};
use crate::util::{parse_hex, to_hex};
use base64::Engine as _;
use serde_json::{Value, json};
use sim_api::{
    ApiError, BreakKind, Command, ErrorCode, Input, Project, Ps, Query, Recording, Response,
    Severity, SimClient, SimConfig, Simulator, TraceFormat, detect_backends, parse_duration,
};
use std::path::{Path, PathBuf};

mod boot;
mod net;
mod source;
mod waves;

/// Longest simulated span a single `sim_run` may cover.
const MAX_RUN_PS: u64 = 3600 * Ps::PER_S;
/// Progress is reported, and cancellation checked, every simulated second.
const CHUNK_PS: u64 = Ps::PER_S;
/// Waveform text returned inline is cut at this many bytes.
const INLINE_TEXT_CAP: usize = 200 * 1024;

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Callbacks the server uses during long operations.
pub trait Control {
    /// Reports progress for a request that supplied a progress token.
    fn progress(&mut self, token: &Value, done: f64, total: f64);
    /// True once the client has sent `notifications/cancelled` for `request_id`.
    fn cancelled(&mut self, request_id: &Value) -> bool;
}

/// A [`Control`] that never reports and is never cancelled.
pub struct NoControl;

impl Control for NoControl {
    fn progress(&mut self, _: &Value, _: f64, _: f64) {}
    fn cancelled(&mut self, _: &Value) -> bool {
        false
    }
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
        }
    }
}

/// What a tool handler produces.
struct Out {
    /// Machine-readable result, also rendered as the text content.
    structured: Value,
    /// Extra content items (images).
    extra: Vec<Value>,
}

impl Out {
    fn json(structured: Value) -> Out {
        Out {
            structured,
            extra: Vec::new(),
        }
    }
}

struct OpenProject {
    path: PathBuf,
    project: Project,
    diagnostics: Vec<sim_api::Diagnostic>,
}

/// Per-call context handed to tool handlers.
struct Call<'a> {
    id: &'a Value,
    progress_token: Option<Value>,
    ctl: &'a mut dyn Control,
}

/// The MCP server state: one simulation, a set of allowed roots and the
/// per-UART read cursors.
pub struct McpServer {
    client: SimClient,
    roots: Vec<PathBuf>,
    project: Option<OpenProject>,
    uart_cursor: [u64; 8],
    tools: Vec<ToolDef>,
}

impl McpServer {
    /// Creates a server with a default simulation. File access is limited to
    /// `roots`; each must exist.
    pub fn new(roots: Vec<PathBuf>) -> Result<McpServer, ApiError> {
        let mut canon = Vec::new();
        for r in roots {
            canon.push(std::fs::canonicalize(&r).map_err(|e| {
                ApiError::io(format!("allowed root {} is not usable: {e}", r.display()))
            })?);
        }
        if canon.is_empty() {
            return Err(ApiError::invalid_argument(
                "at least one allowed root is required",
            ));
        }
        Ok(McpServer {
            client: SimClient::spawn(SimConfig::default())?,
            roots: canon,
            project: None,
            uart_cursor: [0; 8],
            tools: tool_defs(),
        })
    }

    /// Handles one JSON-RPC message. Returns the response, or `None` for
    /// notifications.
    pub fn handle(&mut self, msg: Value, ctl: &mut dyn Control) -> Option<Value> {
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str).map(String::from) else {
            // A response from the client, or garbage: nothing to answer.
            return id
                .map(|id| rpc_error(id, RpcError::new(-32600, "invalid request: no `method`")));
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let result: Result<Value, RpcError> = match method.as_str() {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(&params, id.as_ref().unwrap_or(&Value::Null), ctl),
            "resources/list" => Ok(resources_list()),
            "resources/read" => self.resources_read(&params),
            "prompts/list" => Ok(prompts_list()),
            "prompts/get" => prompts_get(&params),
            m if m.starts_with("notifications/") => return None,
            other => Err(RpcError::new(-32601, format!("method not found: {other}"))),
        };

        let id = id?; // requests without an id are notifications
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => rpc_error(id, e),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let version = requested
            .filter(|v| PROTOCOL_VERSIONS.contains(v))
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "listChanged": false, "subscribe": false },
                "prompts": { "listChanged": false }
            },
            "serverInfo": { "name": "zynqsim-mcp", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "ZynqSim simulates a Zynq-7000 / Zybo Z7-20 board. Call status_get first \
                and read `fidelity` and `capabilities`: in this build the backend is the built-in \
                `demo` model, which has no CPU or RTL, so cpu_*, mem_* and pl_* tools report \
                `unsupported` instead of guessing."
        })
    }

    fn tools_list(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|d| {
                let mut desc = d.description.to_string();
                if let Gate::Planned(m) = d.gate {
                    desc.push_str(&format!(" [not available: planned for milestone {m}]"));
                }
                json!({ "name": d.name, "description": desc, "inputSchema": d.schema })
            })
            .collect();
        json!({ "tools": tools })
    }

    fn tools_call(
        &mut self,
        params: &Value,
        id: &Value,
        ctl: &mut dyn Control,
    ) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(-32602, "tools/call needs a string `name`"))?;
        let idx = self
            .tools
            .iter()
            .position(|d| d.name == name)
            .ok_or_else(|| RpcError::new(-32602, format!("unknown tool `{name}`")))?;
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);

        let outcome = self.run_checked(idx, &args, id, params, ctl);
        Ok(match outcome {
            Ok(out) => {
                let text = serde_json::to_string_pretty(&out.structured).unwrap_or_default();
                let mut content = vec![json!({ "type": "text", "text": text })];
                content.extend(out.extra);
                json!({ "content": content, "structuredContent": out.structured, "isError": false })
            }
            Err(e) => {
                let body = serde_json::to_value(&e).unwrap_or_else(|_| json!({}));
                let text = match &e.hint {
                    Some(h) => format!("{}: {} (hint: {h})", code_name(e.code), e.message),
                    None => format!("{}: {}", code_name(e.code), e.message),
                };
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": { "error": body },
                    "isError": true
                })
            }
        })
    }

    /// Validates arguments and the capability gate, then runs the handler.
    fn run_checked(
        &mut self,
        idx: usize,
        args: &Value,
        id: &Value,
        params: &Value,
        ctl: &mut dyn Control,
    ) -> Result<Out, ApiError> {
        let (name, gate) = (self.tools[idx].name, self.tools[idx].gate);
        schema::validate(&self.tools[idx].schema, args)
            .map_err(|m| ApiError::invalid_argument(format!("{name}: {m}")))?;

        match gate {
            Gate::Open => {}
            Gate::Planned(m) => {
                return Err(ApiError::unsupported(format!(
                    "`{name}` is planned for milestone {m} and is not in this build"
                ))
                .with_hint("see docs/STATUS.md for the feature status table"));
            }
            Gate::Needs(cap) => {
                let st = self.client.status()?;
                if !st.capabilities.contains(&cap) {
                    return Err(ApiError::unsupported(format!(
                        "`{name}` needs the `{cap:?}` capability, which the ps=`{}` / pl=`{}` \
                         backend set (fidelity {:?}) does not provide",
                        st.ps_backend, st.pl_backend, st.fidelity
                    ))
                    .with_hint(match cap {
                        sim_api::Capability::CpuState => {
                            "use the qemu backend with `[debug] enabled = true` in project.toml"
                        }
                        _ => "see docs/STATUS.md for which backends provide this",
                    }));
                }
            }
        }

        let token = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .cloned();
        let mut call = Call {
            id,
            progress_token: token,
            ctl,
        };
        self.dispatch(name, args, &mut call)
    }

    fn dispatch(&mut self, name: &str, a: &Value, call: &mut Call<'_>) -> Result<Out, ApiError> {
        match name {
            "project_open" => self.project_open(a),
            "project_validate" => self.project_validate(a),
            "project_create_from_xsa" => self.project_create_from_xsa(a),
            "boot_inspect" => self.boot_inspect(a),
            "project_create_from_export" => self.project_create_from_export(a),
            "sim_run" => {
                let d = parse_duration(str_arg(a, "duration")?)?;
                self.run_chunked(d, call)?;
                self.time_result()
            }
            "sim_run_until" => {
                let t = parse_duration(str_arg(a, "time")?)?;
                let now = self.client.status()?.now_ps;
                self.run_chunked(t.saturating_sub(now), call)?;
                self.time_result()
            }
            "sim_step" => {
                self.client.command(Command::Step)?;
                self.time_result()
            }
            "sim_pause" => {
                self.client.command(Command::Pause)?;
                self.time_result()
            }
            "sim_reset" => {
                let seed = a.get("seed").and_then(Value::as_u64);
                self.client.command(Command::Input(Input::Reset { seed }))?;
                self.uart_cursor = [0; 8];
                self.time_result()
            }
            "snapshot_save" => {
                let n = str_arg(a, "name")?.to_string();
                self.client
                    .command(Command::SnapshotSave { name: n.clone() })?;
                Ok(Out::json(json!({ "saved": n })))
            }
            "snapshot_restore" => {
                let n = str_arg(a, "name")?.to_string();
                self.client
                    .command(Command::SnapshotRestore { name: n.clone() })?;
                self.uart_cursor = [0; 8];
                let mut out = self.time_result()?;
                out.structured["restored"] = json!(n);
                Ok(out)
            }
            "session_replay" => self.session_replay(a),
            "uart_write" => self.uart_write(a),
            "uart_read" => self.uart_read(a),
            "board_set_switch" => {
                let def = self.client.board_def()?;
                let index = resolve_index(a, "switch", def.switches.iter().map(|s| s.id.as_str()))?;
                let on = a.get("on").and_then(Value::as_bool).unwrap_or(false);
                self.client
                    .command(Command::Input(Input::SetSwitch { index, on }))?;
                Ok(Out::json(
                    json!({ "switch": def.switches[index].id, "on": on }),
                ))
            }
            "board_press_button" => self.press_button(a, call),
            "board_get_leds" => self.get_leds(),
            "pin_get" => {
                let name = str_arg(a, "name")?.to_string();
                match self.client.query(Query::Pin { name })? {
                    Response::Pin { name, level } => Ok(Out::json(
                        json!({ "name": name, "level": level.symbol().to_string() }),
                    )),
                    r => Err(unexpected(&r)),
                }
            }
            "waveform_export" => self.waveform_export(a),
            "waveform_get" => self.waveform_get(a),
            "trace_get" => {
                let from = a.get("from").and_then(Value::as_u64).unwrap_or(0) as usize;
                let max = a.get("max").and_then(Value::as_u64).unwrap_or(100) as usize;
                match self.client.query(Query::Trace { from, max })? {
                    Response::Trace {
                        records,
                        next,
                        total,
                        hash,
                    } => Ok(Out::json(
                        json!({ "records": records, "next": next, "total": total, "hash": hash }),
                    )),
                    r => Err(unexpected(&r)),
                }
            }
            "mem_read" => {
                let addr = a.get("addr").and_then(Value::as_u64).unwrap_or(0);
                let len = a.get("len").and_then(Value::as_u64).unwrap_or(0) as u32;
                match self.client.query(Query::MemRead { addr, len })? {
                    Response::Memory { addr, data } => Ok(Out::json(json!({
                        "addr": addr,
                        "len": data.len(),
                        "hex": to_hex(&data),
                    }))),
                    r => Err(unexpected(&r)),
                }
            }
            "mem_write" => {
                let addr = a.get("addr").and_then(Value::as_u64).unwrap_or(0);
                let data = parse_hex(str_arg(a, "hex")?)?;
                let n = data.len();
                self.client.command(Command::MemWrite { addr, data })?;
                Ok(Out::json(json!({ "addr": addr, "bytes_written": n })))
            }
            "video_frame" => match self.client.query(Query::VideoFrame)? {
                Response::Video {
                    width,
                    height,
                    frame,
                    png,
                } => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                    let mut out = Out::json(json!({
                        "mime_type": "image/png",
                        "width": width,
                        "height": height,
                        "frame": frame,
                        "bytes": png.len(),
                    }));
                    out.extra
                        .push(json!({ "type": "image", "data": b64, "mimeType": "image/png" }));
                    Ok(out)
                }
                r => Err(unexpected(&r)),
            },
            "audio_capture" => {
                let d = parse_duration(str_arg(a, "duration")?)?;
                let max_frames =
                    (u128::from(d) * 48_000 / 1_000_000_000_000u128).clamp(1, 480_000) as u32;
                match self.client.query(Query::AudioCapture { max_frames })? {
                    Response::Audio {
                        sample_rate,
                        first_frame,
                        frames,
                        peak,
                        rms,
                        freq_hz,
                        wav,
                    } => {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&wav);
                        let mut out = Out::json(json!({
                            "mime_type": "audio/wav",
                            "sample_rate": sample_rate,
                            "first_frame": first_frame,
                            "frames": frames,
                            "duration_s": frames as f64 / f64::from(sample_rate),
                            "peak": peak,
                            "rms": rms,
                            "freq_hz": freq_hz,
                            "bytes": wav.len(),
                        }));
                        out.extra
                            .push(json!({ "type": "audio", "data": b64, "mimeType": "audio/wav" }));
                        Ok(out)
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "media_source_set" => self.media_source_set(a),
            "cpu_regs" => {
                let core = a.get("core").and_then(Value::as_u64).unwrap_or(0) as u8;
                match self.client.query(Query::CpuRegs { core })? {
                    Response::CpuRegs(r) => {
                        let mut regs = serde_json::Map::new();
                        for (i, v) in r.r.iter().enumerate() {
                            let name = match i {
                                13 => "sp".to_string(),
                                14 => "lr".to_string(),
                                15 => "pc".to_string(),
                                n => format!("r{n}"),
                            };
                            regs.insert(name, json!(format!("{v:#010x}")));
                        }
                        let f = |bit: u32| r.cpsr & (1 << bit) != 0;
                        let mut out = json!({
                            "core": r.core,
                            "mode": r.mode,
                            "pc_symbol": r.pc_symbol,
                            "pc_source": r.pc_source,
                            "regs": regs,
                            "cpsr": format!("{:#010x}", r.cpsr),
                            "flags": { "N": f(31), "Z": f(30), "C": f(29), "V": f(28) },
                            "irq_masked": f(7),
                            "fiq_masked": f(6),
                            "thumb": f(5),
                        });
                        // The lines around the pc, when the file is readable.
                        if let Some(s) = &r.pc_source {
                            if let Ok((resolved, lines)) = self.source_window(&s.file, s.line, 3) {
                                out["source_context"] =
                                    json!({ "resolved_path": resolved, "lines": lines });
                            }
                        }
                        Ok(Out::json(out))
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "source_line" => self.source_line(a),
            "breakpoint_set" if a.get("addr").is_none() && a.get("location").is_some() => {
                self.breakpoint_at_location(a)
            }
            "breakpoint_set" | "watchpoint_set" => {
                if a.get("addr").is_none() {
                    return Err(ApiError::invalid_argument(if name == "breakpoint_set" {
                        "give `addr` or `location` (symbol, address or file.rs:LINE)"
                    } else {
                        "`addr` is required"
                    }));
                }
                let addr = a.get("addr").and_then(Value::as_u64).unwrap_or(0);
                let enable = a.get("enable").and_then(Value::as_bool).unwrap_or(true);
                let (kind, len) = if name == "breakpoint_set" {
                    (BreakKind::Exec, 4)
                } else {
                    let kind = match a.get("access").and_then(Value::as_str) {
                        Some("read") => BreakKind::Read,
                        Some("access") => BreakKind::Access,
                        _ => BreakKind::Write,
                    };
                    (
                        kind,
                        a.get("len").and_then(Value::as_u64).unwrap_or(4) as u32,
                    )
                };
                self.client.command(Command::SetBreakpoint {
                    addr,
                    kind,
                    len,
                    enable,
                })?;
                Ok(Out::json(
                    json!({ "addr": addr, "kind": kind, "len": len, "set": enable }),
                ))
            }
            "cpu_disasm" => {
                let addr = a.get("addr").and_then(Value::as_u64).unwrap_or(0);
                let count = a.get("count").and_then(Value::as_u64).unwrap_or(16) as u32;
                let thumb = a.get("thumb").and_then(Value::as_bool);
                match self.client.query(Query::Disasm { addr, count, thumb })? {
                    Response::Disasm { thumb, lines } => {
                        Ok(Out::json(json!({ "thumb": thumb, "instructions": lines })))
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "pmod_attach" => {
                let port = pmod_index(str_arg(a, "port")?)?;
                let kind = str_arg(a, "module")?.to_string();
                self.client.command(Command::AttachDevice {
                    port,
                    kind: kind.clone(),
                })?;
                Ok(Out::json(json!({ "port": a["port"], "module": kind })))
            }
            "pmod_frame" => {
                let port = pmod_index(str_arg(a, "port")?)?;
                match self.client.query(Query::DeviceImage { port })? {
                    Response::DeviceImage {
                        kind,
                        width,
                        height,
                        png,
                        description,
                        ..
                    } => {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                        let mut out = Out::json(json!({
                            "port": a["port"],
                            "module": kind,
                            "mime_type": "image/png",
                            "width": width,
                            "height": height,
                            "state": description,
                        }));
                        out.extra
                            .push(json!({ "type": "image", "data": b64, "mimeType": "image/png" }));
                        Ok(out)
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "pmod_set" => {
                let port = pmod_index(str_arg(a, "port")?)?;
                let name = str_arg(a, "name")?.to_string();
                let value = a
                    .get("value")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| ApiError::invalid_argument("`value` must be a number"))?;
                self.client
                    .command(Command::SetDeviceValue { port, name, value })?;
                device_state(&mut self.client, port, &a["port"])
            }
            "pmod_state" => {
                let port = pmod_index(str_arg(a, "port")?)?;
                device_state(&mut self.client, port, &a["port"])
            }
            "breakpoint_list" => match self.client.query(Query::Breakpoints)? {
                Response::Breakpoints(list) => Ok(Out::json(json!({ "breakpoints": list }))),
                r => Err(unexpected(&r)),
            },
            "symbol_lookup" => {
                let query = str_arg(a, "query")?.to_string();
                match self.client.query(Query::Symbol { query })? {
                    Response::Symbol(s) => Ok(Out::json(json!({
                        "name": s.name,
                        "addr": s.addr,
                        "addr_hex": format!("{:#x}", s.addr),
                        "size": s.size,
                        "offset": s.offset,
                    }))),
                    r => Err(unexpected(&r)),
                }
            }
            "net_capture" => self.net_capture(a),
            "net_send" => self.net_send(a),
            "board_screenshot" => {
                let scale = a.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                match self.client.query(Query::Screenshot { scale })? {
                    Response::Png(bytes) => {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let mut out = Out::json(json!({
                            "mime_type": "image/png",
                            "bytes": bytes.len(),
                            "scale": scale
                        }));
                        out.extra
                            .push(json!({ "type": "image", "data": b64, "mimeType": "image/png" }));
                        Ok(out)
                    }
                    r => Err(unexpected(&r)),
                }
            }
            "config_set" => self.config_set(a),
            "status_get" => {
                let st = self.client.status()?;
                let project = self.project.as_ref().map(|p| p.path.display().to_string());
                let mut out = json!({ "status": st, "project": project });
                if st.capabilities.contains(&sim_api::Capability::Network) {
                    if let Response::NetStats(n) = self.client.query(Query::NetStats)? {
                        out["network"] = json!(n);
                    }
                }
                Ok(Out::json(out))
            }
            "backend_list" => Ok(Out::json(json!({ "backends": detect_backends() }))),
            // Reaching here means the gate passed but no handler exists: a
            // backend advertised a capability this server cannot serve yet.
            other => Err(ApiError::unsupported(format!(
                "`{other}` is advertised by the backend but this server has no handler for it"
            ))),
        }
    }

    // ------------------------------------------------------------- helpers

    fn time_result(&self) -> Result<Out, ApiError> {
        let st = self.client.status()?;
        Ok(Out::json(json!({
            "now_ps": st.now_ps,
            "now": Ps(st.now_ps).to_string(),
            "trace_len": st.trace_len,
            "trace_hash": st.trace_hash,
            "fidelity": st.fidelity,
            "debug": st.debug,
        })))
    }

    fn run_chunked(&mut self, total: u64, call: &mut Call<'_>) -> Result<(), ApiError> {
        if total > MAX_RUN_PS {
            return Err(ApiError::invalid_argument(format!(
                "a single call may cover at most {} s of simulated time",
                MAX_RUN_PS / Ps::PER_S
            ))
            .with_hint("call sim_run repeatedly"));
        }
        let mut done = 0u64;
        while done < total {
            if call.ctl.cancelled(call.id) {
                return Err(ApiError::cancelled(format!(
                    "cancelled after {} of {}",
                    Ps(done),
                    Ps(total)
                )));
            }
            let step = CHUNK_PS.min(total - done);
            self.client.command(Command::Run { duration_ps: step })?;
            done += step;
            // A breakpoint ends the call: the next one continues.
            if self.client.status()?.debug.is_some_and(|d| d.stopped) {
                break;
            }
            if let Some(tok) = &call.progress_token {
                call.ctl
                    .progress(tok, done as f64 / 1e12, total as f64 / 1e12);
            }
        }
        Ok(())
    }

    /// Resolves `p` against the allowed roots. For `must_exist == false` only
    /// the parent directory has to exist (output files).
    fn resolve(&self, p: &str, must_exist: bool) -> Result<PathBuf, ApiError> {
        let raw = Path::new(p);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.roots[0].join(raw)
        };
        let canon = if must_exist {
            std::fs::canonicalize(&joined)
                .map_err(|_| ApiError::not_found(format!("`{p}` does not exist")))?
        } else {
            let parent = joined
                .parent()
                .ok_or_else(|| ApiError::invalid_argument(format!("`{p}` has no parent")))?;
            let file = joined
                .file_name()
                .ok_or_else(|| ApiError::invalid_argument(format!("`{p}` has no file name")))?;
            std::fs::canonicalize(parent)
                .map_err(|_| ApiError::not_found(format!("directory of `{p}` does not exist")))?
                .join(file)
        };
        if self.roots.iter().any(|r| canon.starts_with(r)) {
            Ok(canon)
        } else {
            Err(
                ApiError::invalid_argument(format!("`{p}` is outside the allowed roots"))
                    .with_hint("pass --root <dir> to zynqsim-mcp to allow another directory"),
            )
        }
    }

    fn media_source_set(&mut self, a: &Value) -> Result<Out, ApiError> {
        let video_in = a.get("video_in").and_then(Value::as_str);
        let audio_in = a.get("audio_in").and_then(Value::as_str);
        if video_in.is_none() && audio_in.is_none() {
            return Err(ApiError::invalid_argument(
                "give `video_in`, `audio_in` or both",
            ));
        }
        // File sources must be inside an allowed root: resolve first, then
        // hand the confined absolute path to the shared parser.
        let confine = |s: &str, special: &[&str], prefixes: &[&str]| -> Result<String, ApiError> {
            let t = s.trim();
            let named = special.contains(&t) || prefixes.iter().any(|p| t.starts_with(p));
            if named {
                Ok(t.to_string())
            } else {
                Ok(self.resolve(t, true)?.display().to_string())
            }
        };
        let base = self.roots[0].clone();
        let video = video_in
            .map(|v| {
                let v = confine(
                    v,
                    &["bars", "gradient"],
                    &["bars:", "gradient:", "colorbars"],
                )?;
                sim_api::parse_video_spec(&v, &base)
            })
            .transpose()?;
        let audio = audio_in
            .map(|v| {
                let v = confine(v, &["silence"], &["tone:"])?;
                sim_api::parse_audio_spec(&v, &base)
            })
            .transpose()?;
        self.client
            .command(Command::SetMediaSource { video, audio })?;
        Ok(Out::json(
            json!({ "video_in": video_in, "audio_in": audio_in }),
        ))
    }

    fn project_file(&self, p: &str) -> Result<PathBuf, ApiError> {
        let path = self.resolve(p, true)?;
        if path.is_dir() {
            let f = path.join("project.toml");
            if !f.is_file() {
                return Err(ApiError::not_found(format!(
                    "no project.toml in {}",
                    path.display()
                )));
            }
            Ok(f)
        } else {
            Ok(path)
        }
    }

    fn project_open(&mut self, a: &Value) -> Result<Out, ApiError> {
        let file = self.project_file(str_arg(a, "path")?)?;
        let project = Project::load(&file)?;
        let base = file.parent().unwrap_or(Path::new("."));
        let diagnostics = project.validate(base);
        let errors = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let started = errors == 0;
        if started {
            let base = file.parent().unwrap_or(Path::new("."));
            let engine = project.build_engine(base)?;
            self.client = SimClient::spawn_engine(engine);
            self.uart_cursor = [0; 8];
        }
        let out = json!({
            "path": file.display().to_string(),
            "project": project,
            "diagnostics": diagnostics,
            "errors": errors,
            "simulation_started": started,
        });
        self.project = Some(OpenProject {
            path: file,
            project,
            diagnostics,
        });
        Ok(Out::json(out))
    }

    /// `project_create_from_xsa`: plans and writes a project, never starts it.
    fn project_create_from_xsa(&mut self, a: &Value) -> Result<Out, ApiError> {
        use sim_api::xsa;
        let xsa_file = self.resolve(str_arg(a, "xsa_path")?, true)?;
        let out_dir = self.resolve(str_arg(a, "out_dir")?, false)?;
        let mut rtl = Vec::new();
        if let Some(list) = a.get("rtl").and_then(Value::as_array) {
            for item in list.iter().filter_map(Value::as_str) {
                rtl.push(self.resolve(item, true)?);
            }
        }
        let opts = xsa::ImportOptions {
            rtl,
            // An existing file inside the roots becomes a path relative to `out_dir`;
            // anything else is written as given (the ELF may be built later).
            elf: a.get("elf").and_then(Value::as_str).map(|e| {
                self.resolve(e, true)
                    .map_or_else(|_| e.to_string(), |p| p.display().to_string())
            }),
            elf1: None,
            filelist: None,
        };
        let plan = xsa::plan(&xsa_file, &out_dir, &opts)?;
        let created = plan.write(&out_dir)?;
        let file = out_dir.join("project.toml");
        let project = Project::load(&file)?;
        let diagnostics = project.validate(&out_dir);
        let errors = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let d = &plan.handoff.design;
        Ok(Out::json(json!({
            "project": file.display().to_string(),
            "report": out_dir.join("xsa-report.md").display().to_string(),
            "created": created.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "design": d.name,
            "vivado_version": d.vivado_version,
            "ps7": d.ps7,
            "pl_instances": d.ips.iter().map(|i| json!({
                "instance": i.instance,
                "vlnv": i.vlnv,
                "class": i.class,
                "address": i.address.first().map(|a| json!({
                    "base": a.base, "size": a.size(), "master_port": a.master_interface,
                })),
            })).collect::<Vec<_>>(),
            "interrupts": d.interrupts,
            "testbench_module": plan.testbench_module,
            "unmodelled": plan.unmodelled,
            "warnings": plan.warnings,
            "diagnostics": diagnostics,
            "errors": errors,
            "runnable": errors == 0,
        })))
    }

    /// `project_create_from_export`: a project from a Vivado simulation export.
    fn project_create_from_export(&mut self, a: &Value) -> Result<Out, ApiError> {
        use sim_api::exportinit::{InitOptions, plan_init};
        let export = self.resolve(str_arg(a, "export_path")?, true)?;
        let out_dir = self.resolve(str_arg(a, "out_dir")?, false)?;
        let opts = InitOptions {
            elf: a.get("elf").and_then(Value::as_str).map(|e| {
                self.resolve(e, true)
                    .map_or_else(|_| e.to_string(), |p| p.display().to_string())
            }),
            elf1: None,
            module: a.get("module").and_then(Value::as_str).map(String::from),
            base: None,
        };
        let plan = plan_init(&export, &out_dir, &opts)?;
        let created = plan.write(&out_dir)?;
        let file = out_dir.join("project.toml");
        let project = Project::load(&file)?;
        let diagnostics = project.validate(&out_dir);
        let errors = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        Ok(Out::json(json!({
            "project": file.display().to_string(),
            "report": out_dir.join("export-report.md").display().to_string(),
            "created": created.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "top": plan.top,
            "driven_module": plan.driven_module,
            "sources": plan.design.sources.iter().map(|s| json!({
                "path": s.path.display().to_string(),
                "language": s.lang.prj_word(),
                "library": s.library,
                "listed_at": s.origin.to_string(),
            })).collect::<Vec<_>>(),
            "link_libraries": plan.design.link_libs,
            "export_tops": plan.design.tops,
            "warnings": plan.warnings,
            "diagnostics": diagnostics,
            "errors": errors,
            "runnable": errors == 0,
        })))
    }

    fn project_validate(&mut self, a: &Value) -> Result<Out, ApiError> {
        if let Some(p) = a.get("path").and_then(Value::as_str) {
            let file = self.project_file(p)?;
            let project = Project::load(&file)?;
            let diagnostics = project.validate(file.parent().unwrap_or(Path::new(".")));
            Ok(Out::json(json!({
                "path": file.display().to_string(),
                "diagnostics": diagnostics,
                "valid": diagnostics.iter().all(|d| d.severity != Severity::Error),
            })))
        } else if let Some(p) = &self.project {
            Ok(Out::json(json!({
                "path": p.path.display().to_string(),
                "diagnostics": p.diagnostics,
                "valid": p.diagnostics.iter().all(|d| d.severity != Severity::Error),
            })))
        } else {
            Err(
                ApiError::not_found("no project is open and no `path` was given")
                    .with_hint("call project_open first or pass `path`"),
            )
        }
    }

    fn session_replay(&mut self, a: &Value) -> Result<Out, ApiError> {
        let rec: Recording = if let Some(p) = a.get("path").and_then(Value::as_str) {
            let file = self.resolve(p, true)?;
            let text = std::fs::read_to_string(&file)
                .map_err(|e| ApiError::io(format!("cannot read {}: {e}", file.display())))?;
            serde_json::from_str(&text)
                .map_err(|e| ApiError::invalid_argument(format!("not a recording: {e}")))?
        } else {
            match self.client.query(Query::Recording)? {
                Response::Recording(mut r) => {
                    if !r.journal.is_empty() {
                        r.project = self.project.as_ref().map(|p| p.path.display().to_string());
                    }
                    *r
                }
                r => return Err(unexpected(&r)),
            }
        };
        let report = if rec.journal.is_empty() {
            Simulator::replay(&rec)?.1
        } else {
            sim_api::replay_recording(&rec)?
        };
        Ok(Out::json(json!({
            "report": report,
            "inputs": rec.entries.len().max(rec.journal.len()),
            "deterministic": report.matches,
        })))
    }

    fn uart_write(&mut self, a: &Value) -> Result<Out, ApiError> {
        let port = a.get("port").and_then(Value::as_u64).unwrap_or(0) as u8;
        let data = match (a.get("text").and_then(Value::as_str), a.get("hex")) {
            (Some(t), None) => t.as_bytes().to_vec(),
            (None, Some(h)) => parse_hex(h.as_str().unwrap_or_default())?,
            _ => {
                return Err(ApiError::invalid_argument(
                    "uart_write needs exactly one of `text` or `hex`",
                ));
            }
        };
        if data.is_empty() || data.len() > 65536 {
            return Err(ApiError::invalid_argument(
                "data must be between 1 and 65536 bytes",
            ));
        }
        let n = data.len();
        self.client
            .command(Command::Input(Input::UartWrite { port, data }))?;
        Ok(Out::json(json!({ "port": port, "bytes_written": n })))
    }

    fn uart_read(&mut self, a: &Value) -> Result<Out, ApiError> {
        let port = a.get("port").and_then(Value::as_u64).unwrap_or(0) as u8;
        let max = a.get("max").and_then(Value::as_u64).unwrap_or(4096) as usize;
        let explicit = a.get("from").and_then(Value::as_u64);
        let from = explicit.unwrap_or(self.uart_cursor[usize::from(port.min(7))]);
        match self.client.query(Query::UartRead { port, from, max })? {
            Response::Uart { data, next, oldest } => {
                if explicit.is_none() {
                    self.uart_cursor[usize::from(port.min(7))] = next;
                }
                Ok(Out::json(json!({
                    "port": port,
                    "from": from,
                    "next": next,
                    "oldest_retained": oldest,
                    "bytes": data.len(),
                    "text": String::from_utf8_lossy(&data),
                    "hex": to_hex(&data),
                })))
            }
            r => Err(unexpected(&r)),
        }
    }

    fn press_button(&mut self, a: &Value, call: &mut Call<'_>) -> Result<Out, ApiError> {
        let def = self.client.board_def()?;
        let index = resolve_index(a, "button", def.buttons.iter().map(|b| b.id.as_str()))?;
        let action = a.get("action").and_then(Value::as_str).unwrap_or("tap");
        let set = |pressed| Command::Input(Input::SetButton { index, pressed });
        match action {
            "down" => {
                self.client.command(set(true))?;
            }
            "up" => {
                self.client.command(set(false))?;
            }
            _ => {
                let hold = match a.get("hold").and_then(Value::as_str) {
                    Some(h) => parse_duration(h)?,
                    None => 50 * Ps::PER_MS,
                };
                if hold > MAX_RUN_PS {
                    return Err(ApiError::invalid_argument(format!(
                        "`hold` may be at most {} s",
                        MAX_RUN_PS / Ps::PER_S
                    )));
                }
                self.client.command(set(true))?;
                // Release even if the hold is cancelled, so the button is
                // never left stuck down by an aborted call.
                let held = self.run_chunked(hold, call);
                self.client.command(set(false))?;
                held?;
            }
        }
        let mut out = self.time_result()?;
        out.structured["button"] = json!(def.buttons[index].id);
        out.structured["action"] = json!(action);
        Ok(out)
    }

    fn get_leds(&self) -> Result<Out, ApiError> {
        let def = self.client.board_def()?;
        let st = self.client.board_state()?;
        let leds: Vec<Value> = def
            .leds
            .iter()
            .zip(&st.leds)
            .map(|(l, d)| json!({ "id": l.id, "duty": d, "on": *d >= 128 }))
            .collect();
        let rgb: Vec<Value> = def
            .rgb_leds
            .iter()
            .zip(&st.rgb)
            .map(|(l, c)| json!({ "id": l.id, "r": c[0], "g": c[1], "b": c[2] }))
            .collect();
        Ok(Out::json(json!({ "leds": leds, "rgb_leds": rgb })))
    }

    fn waveform_export(&mut self, a: &Value) -> Result<Out, ApiError> {
        let fmt = match str_arg(a, "format")? {
            "vcd" => TraceFormat::Vcd,
            _ => TraceFormat::Perfetto,
        };
        let text = match self.client.query(Query::TraceExport { format: fmt })? {
            Response::Text(t) => t,
            r => return Err(unexpected(&r)),
        };
        if let Some(p) = a.get("path").and_then(Value::as_str) {
            let file = self.resolve(p, false)?;
            std::fs::write(&file, &text)
                .map_err(|e| ApiError::io(format!("cannot write {}: {e}", file.display())))?;
            Ok(Out::json(
                json!({ "path": file.display().to_string(), "bytes": text.len() }),
            ))
        } else {
            let truncated = text.len() > INLINE_TEXT_CAP;
            let mut end = text.len().min(INLINE_TEXT_CAP);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            Ok(Out::json(json!({
                "text": &text[..end],
                "bytes": text.len(),
                "truncated": truncated,
            })))
        }
    }

    fn config_set(&mut self, a: &Value) -> Result<Out, ApiError> {
        if a.as_object().is_none_or(|o| o.is_empty()) {
            return Err(ApiError::invalid_argument(
                "config_set needs `quantum` and/or `seed`",
            ));
        }
        if let Some(q) = a.get("quantum").and_then(Value::as_str) {
            let ps = parse_duration(q)?;
            self.client
                .command(Command::SetQuantum { quantum_ps: ps })?;
        }
        if let Some(seed) = a.get("seed").and_then(Value::as_u64) {
            self.client
                .command(Command::Input(Input::Reset { seed: Some(seed) }))?;
            self.uart_cursor = [0; 8];
        }
        let st = self.client.status()?;
        Ok(Out::json(json!({ "status": st })))
    }

    // ------------------------------------------------------------ resources

    fn resources_read(&mut self, params: &Value) -> Result<Value, RpcError> {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(-32602, "resources/read needs a string `uri`"))?;
        let fail = |e: ApiError| RpcError::new(-32603, e.to_string());
        let (mime, text) = match uri {
            "zynqsim://status" => (
                "application/json",
                to_pretty(&json!(self.client.status().map_err(fail)?)),
            ),
            "zynqsim://board" => (
                "application/json",
                to_pretty(&json!(self.client.board_def().map_err(fail)?)),
            ),
            "zynqsim://project" => (
                "application/json",
                to_pretty(&match &self.project {
                    Some(p) => json!({
                        "path": p.path.display().to_string(),
                        "project": p.project,
                        "diagnostics": p.diagnostics
                    }),
                    None => json!({ "project": null, "note": "no project is open" }),
                }),
            ),
            "zynqsim://trace" => {
                let r = self
                    .client
                    .query(Query::Trace { from: 0, max: 200 })
                    .map_err(fail)?;
                ("application/json", to_pretty(&json!(r)))
            }
            "zynqsim://waveform.vcd" => match self
                .client
                .query(Query::TraceExport {
                    format: TraceFormat::Vcd,
                })
                .map_err(fail)?
            {
                Response::Text(t) => ("text/plain", t),
                _ => return Err(RpcError::new(-32603, "unexpected response")),
            },
            "zynqsim://docs/status" => (
                "text/markdown",
                include_str!("../../../docs/STATUS.md").to_string(),
            ),
            other => {
                return Err(RpcError::new(
                    -32002,
                    format!("resource not found: {other}"),
                ));
            }
        };
        Ok(json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] }))
    }
}

fn to_pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// `JA`..`JE` (any case) to the port index 0..=4.
fn pmod_index(s: &str) -> Result<u8, ApiError> {
    match s.to_ascii_uppercase().as_str() {
        "JA" => Ok(0),
        "JB" => Ok(1),
        "JC" => Ok(2),
        "JD" => Ok(3),
        "JE" => Ok(4),
        "CAM" => Ok(5),
        other => Err(ApiError::invalid_argument(format!(
            "`{other}` is not a Pmod port with PL pins (JA..JE)"
        ))),
    }
}

/// The state of the device on `port` as a tool result.
fn device_state(client: &mut SimClient, port: u8, port_arg: &Value) -> Result<Out, ApiError> {
    match client.query(Query::DeviceState { port })? {
        Response::DeviceState {
            kind,
            description,
            values,
            registers,
            ..
        } => Ok(Out::json(json!({
            "port": port_arg,
            "module": kind,
            "state": description,
            "values": values,
            "registers": registers,
        }))),
        r => Err(unexpected(&r)),
    }
}

fn unexpected(r: &Response) -> ApiError {
    ApiError::internal(format!("unexpected response variant: {r:?}"))
}

fn code_name(c: ErrorCode) -> &'static str {
    match c {
        ErrorCode::InvalidArgument => "invalid_argument",
        ErrorCode::NotFound => "not_found",
        ErrorCode::Unsupported => "unsupported",
        ErrorCode::Io => "io",
        ErrorCode::Disconnected => "disconnected",
        ErrorCode::Cancelled => "cancelled",
        ErrorCode::Internal => "internal",
    }
}

fn rpc_error(id: Value, e: RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } })
}

fn str_arg<'a>(a: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    a.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid_argument(format!("`{key}` is required")))
}

/// Resolves the `index` / `id` pair of a board control.
fn resolve_index<'a>(
    a: &Value,
    what: &str,
    ids: impl Iterator<Item = &'a str> + Clone,
) -> Result<usize, ApiError> {
    let count = ids.clone().count();
    match (
        a.get("index").and_then(Value::as_u64),
        a.get("id").and_then(Value::as_str),
    ) {
        (Some(_), Some(_)) | (None, None) => Err(ApiError::invalid_argument(format!(
            "give exactly one of `index` or `id` for the {what}"
        ))),
        (Some(i), None) => {
            let i = i as usize;
            if i < count {
                Ok(i)
            } else {
                Err(ApiError::invalid_argument(format!(
                    "{what} index {i} out of range 0..{count}"
                )))
            }
        }
        (None, Some(id)) => ids.clone().position(|x| x == id).ok_or_else(|| {
            let all: Vec<&str> = ids.collect();
            ApiError::not_found(format!("no {what} with id `{id}`"))
                .with_hint(format!("available: {}", all.join(", ")))
        }),
    }
}

// -------------------------------------------------------------- prompts

fn prompts_list() -> Value {
    json!({ "prompts": [
        {
            "name": "debug_a_hang",
            "description": "Step-by-step procedure for finding why a simulated design stopped making progress.",
            "arguments": [{ "name": "symptom", "description": "What was observed, e.g. 'UART printed start then nothing'.", "required": false }]
        },
        {
            "name": "explain_axi_violation",
            "description": "Explain an AXI protocol-checker finding and propose a fix.",
            "arguments": [{ "name": "violation", "description": "The checker message, including timestamp.", "required": true }]
        }
    ]})
}

fn prompts_get(params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(-32602, "prompts/get needs a string `name`"))?;
    let arg = |k: &str| {
        params
            .get("arguments")
            .and_then(|a| a.get(k))
            .and_then(Value::as_str)
    };
    let text = match name {
        "debug_a_hang" => format!(
            "Diagnose a hang in the simulated design.{}\n\n\
             1. Call status_get. Note `fidelity` and `capabilities`; say plainly which of the steps \
                below the current backend cannot do.\n\
             2. Call uart_read on ports 0 and 1 for the last output before the stall.\n\
             3. Call trace_get near the end of the trace to see the last inputs, signal changes and \
                UART bytes, and compare their timestamps with `now` from status_get.\n\
             4. If capabilities include CpuState, call cpu_regs for both cores and cpu_disasm at the \
                PC; a core in WFI/WFE or spinning on one address is the usual cause. If the PL is \
                involved and PlSignals is available, use pl_axi_log to find an AXI transaction with \
                VALID asserted and READY never raised.\n\
             5. Report the most likely cause, the evidence (with timestamps) and what you could not \
                check because of missing capabilities.",
            arg("symptom")
                .map(|s| format!(" Symptom: {s}."))
                .unwrap_or_default()
        ),
        "explain_axi_violation" => {
            let v = arg("violation").ok_or_else(|| {
                RpcError::new(
                    -32602,
                    "prompt `explain_axi_violation` requires `violation`",
                )
            })?;
            format!(
                "Explain this AXI protocol-checker finding and propose a fix: {v}\n\n\
                 Call status_get first. AXI monitoring needs the PlSignals capability; if it is not \
                 listed, say that the finding cannot be reproduced on this backend and reason only \
                 from the message. Otherwise use pl_axi_log around the reported timestamp. Cite the \
                 AMBA AXI rule violated (VALID/READY handshake, 4 KB boundary, WLAST, alignment) and \
                 the signal responsible."
            )
        }
        other => return Err(RpcError::new(-32602, format!("unknown prompt `{other}`"))),
    };
    Ok(json!({
        "description": name,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }]
    }))
}

fn resources_list() -> Value {
    json!({ "resources": [
        { "uri": "zynqsim://status", "name": "Simulation status", "mimeType": "application/json",
          "description": "Time, backends, fidelity, capabilities and trace digest." },
        { "uri": "zynqsim://board", "name": "Board description", "mimeType": "application/json",
          "description": "Components, pins and layout of the active board." },
        { "uri": "zynqsim://project", "name": "Open project", "mimeType": "application/json",
          "description": "The opened project.toml and its diagnostics." },
        { "uri": "zynqsim://trace", "name": "Trace (first 200 records)", "mimeType": "application/json",
          "description": "Start of the event trace; use the trace_get tool to page." },
        { "uri": "zynqsim://waveform.vcd", "name": "Waveform (VCD)", "mimeType": "text/plain",
          "description": "Every board signal and UART byte as a VCD file." },
        { "uri": "zynqsim://docs/status", "name": "Feature status", "mimeType": "text/markdown",
          "description": "docs/STATUS.md: what is verified, untested, stubbed or not planned." }
    ]})
}
