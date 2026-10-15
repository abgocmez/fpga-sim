//! `waveform_get`: value changes of chosen signals in a time window, with an
//! optional SVG or PNG drawing (ADR-014).

use super::{McpServer, Out, unexpected};
use base64::Engine as _;
use serde_json::{Value, json};
use sim_api::trace::wave_scene::{SceneOpts, scene, to_rgba, to_svg};
use sim_api::trace::waveform::{DEFAULT_MAX_CHANGES, HARD_MAX_CHANGES, Waveform};
use sim_api::{ApiError, Query, Response, parse_duration};

/// Largest picture width accepted.
const MAX_PX: u64 = 4000;

/// A value as JSON: a number while exact in a double, else a `0x` string.
fn value_json(v: u64) -> Value {
    if v < (1 << 53) {
        json!(v)
    } else {
        json!(format!("{v:#x}"))
    }
}

fn wave_json(w: &Waveform) -> Value {
    let signals: Vec<Value> = w
        .signals
        .iter()
        .map(|s| {
            let changes: Vec<Value> = s
                .changes
                .iter()
                .map(|&(t, v)| json!([t, value_json(v)]))
                .collect();
            json!({
                "name": s.name,
                "width": s.width,
                "found": s.found,
                "initial": s.initial.map(value_json),
                "changes": changes,
                "in_window": s.in_window,
                "truncated": s.truncated,
                "covered_to_ps": s.covered_to_ps,
            })
        })
        .collect();
    json!({
        "from_ps": w.from_ps,
        "to_ps": w.to_ps,
        "now_ps": w.now_ps,
        "trace_len": w.trace_len,
        "dropped_records": w.dropped,
        "max_changes": w.max_changes,
        "truncated": w.truncated(),
        "signals_truncated": w.signals_truncated,
        "change_format": "[time_ps, value]; `initial` is the value in effect at from_ps (null = not driven yet)",
        "signals": signals,
    })
}

impl McpServer {
    /// The signal catalogue (no `signals` given) or a window of changes.
    pub(super) fn waveform_get(&mut self, a: &Value) -> Result<Out, ApiError> {
        let names: Vec<String> = match a.get("signals") {
            None => Vec::new(),
            Some(Value::Array(v)) => v
                .iter()
                .map(|x| {
                    x.as_str().map(String::from).ok_or_else(|| {
                        ApiError::invalid_argument("`signals` must be an array of strings")
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(ApiError::invalid_argument(
                    "`signals` must be an array of strings",
                ));
            }
        };

        if names.is_empty() {
            return match self.client.query(Query::SignalCatalogue)? {
                Response::Signals {
                    signals,
                    now_ps,
                    trace_len,
                } => Ok(Out::json(json!({
                    "signals": signals,
                    "now_ps": now_ps,
                    "trace_len": trace_len,
                    "note": "pass `signals` (names, or `prefix*`) with `from`/`to` to get value changes",
                }))),
                r => Err(unexpected(&r)),
            };
        }

        let now = match self.client.query(Query::SignalCatalogue)? {
            Response::Signals { now_ps, .. } => now_ps,
            r => return Err(unexpected(&r)),
        };
        // A bare `0` is accepted: it needs no unit.
        let time = |s: &str| {
            if s.trim() == "0" {
                Ok(0)
            } else {
                parse_duration(s)
            }
        };
        let from_ps = match a.get("from").and_then(Value::as_str) {
            Some(s) => time(s)?,
            None => 0,
        };
        let to_ps = match a.get("to").and_then(Value::as_str) {
            Some(s) => time(s)?,
            None => now.max(from_ps),
        };
        let max_changes = a
            .get("max_changes")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_CHANGES, |n| (n as usize).min(HARD_MAX_CHANGES));
        let w = match self.client.query(Query::Waveform {
            signals: names,
            from_ps,
            to_ps,
            max_changes,
        })? {
            Response::Waveform(w) => *w,
            r => return Err(unexpected(&r)),
        };

        let mut out = wave_json(&w);
        let render = a.get("render").and_then(Value::as_str).unwrap_or("none");
        if render != "none" {
            let width = a
                .get("width")
                .and_then(Value::as_u64)
                .unwrap_or(1000)
                .clamp(200, MAX_PX) as f32;
            let png = render == "png";
            // The raster font advances 8 px per character.
            let char_w = if png { 8.0 } else { 7.0 };
            let opts = SceneOpts {
                plot_w: width - 200.0,
                label_w: 200.0,
                row_h: 28.0,
                header_h: 22.0,
                char_w,
                cursor_ps: None,
            };
            let sc = scene(&w, &opts);
            let path = a.get("path").and_then(Value::as_str);
            if png {
                let (pw, ph, rgba) = to_rgba(&sc);
                let bytes = sim_api::board::encode_rgba_png(pw, ph, rgba)
                    .map_err(|e| ApiError::internal(format!("PNG encoding failed: {e}")))?;
                out["image"] = json!({ "mime_type": "image/png", "width": pw, "height": ph,
                                       "bytes": bytes.len() });
                if let Some(p) = path {
                    let file = self.resolve(p, false)?;
                    std::fs::write(&file, &bytes).map_err(|e| {
                        ApiError::io(format!("cannot write {}: {e}", file.display()))
                    })?;
                    out["image"]["path"] = json!(file.display().to_string());
                }
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let mut o = Out::json(out);
                o.extra
                    .push(json!({ "type": "image", "data": b64, "mimeType": "image/png" }));
                return Ok(o);
            }
            let svg = to_svg(&sc, char_w);
            if let Some(p) = path {
                let file = self.resolve(p, false)?;
                std::fs::write(&file, &svg)
                    .map_err(|e| ApiError::io(format!("cannot write {}: {e}", file.display())))?;
                out["svg_path"] = json!(file.display().to_string());
                out["svg_bytes"] = json!(svg.len());
            } else {
                out["svg"] = json!(svg);
            }
        }
        Ok(Out::json(out))
    }
}
