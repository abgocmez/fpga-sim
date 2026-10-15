use crate::log::{TraceEvent, TraceLog};
use serde_json::json;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The value-change name used for a UART byte stream.
fn uart_signal(port: u8, tx: bool) -> String {
    format!("uart{port}.{}", if tx { "tx" } else { "rx" })
}

/// The `(name, value, width)` a record contributes to a waveform, if any.
fn as_signal(ev: &TraceEvent) -> Option<(String, u64, u8)> {
    match ev {
        TraceEvent::Signal { name, value, width } => Some((name.clone(), *value, *width)),
        TraceEvent::Uart { port, tx, byte } => Some((uart_signal(*port, *tx), u64::from(*byte), 8)),
        _ => None,
    }
}

/// VCD identifier code for signal number `n`: base-94 over `!`..`~`.
fn vcd_id(mut n: usize) -> String {
    let mut s = String::new();
    loop {
        s.push((b'!' + (n % 94) as u8) as char);
        n /= 94;
        if n == 0 {
            break;
        }
        n -= 1;
    }
    s
}

fn vcd_value(value: u64, width: u8, id: &str) -> String {
    if width == 1 {
        format!("{}{id}", value & 1)
    } else {
        let w = usize::from(width.clamp(1, 64));
        format!("b{:0w$b} {id}", value, w = w)
    }
}

/// Renders every signal-like record as a VCD file with a 1 ps timescale.
///
/// Signals are declared in name order and start as `x` until first driven.
pub fn to_vcd(log: &TraceLog) -> String {
    let mut widths: BTreeMap<String, u8> = BTreeMap::new();
    for r in log.records() {
        if let Some((name, _, width)) = as_signal(&r.event) {
            let w = widths.entry(name).or_insert(width);
            *w = (*w).max(width);
        }
    }
    let ids: BTreeMap<&str, String> = widths
        .keys()
        .enumerate()
        .map(|(i, n)| (n.as_str(), vcd_id(i)))
        .collect();

    let mut out = String::new();
    out.push_str("$version zynqsim $end\n$timescale 1 ps $end\n$scope module zynqsim $end\n");
    for (name, width) in &widths {
        let _ = writeln!(out, "$var wire {width} {} {name} $end", ids[name.as_str()]);
    }
    out.push_str("$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n");
    for (name, width) in &widths {
        let id = &ids[name.as_str()];
        if *width == 1 {
            let _ = writeln!(out, "x{id}");
        } else {
            let _ = writeln!(out, "bx {id}");
        }
    }
    out.push_str("$end\n");

    let mut last_t = 0u64;
    for r in log.records() {
        if let Some((name, value, _)) = as_signal(&r.event) {
            if r.t.0 != last_t {
                let _ = writeln!(out, "#{}", r.t.0);
                last_t = r.t.0;
            }
            let id = &ids[name.as_str()];
            let _ = writeln!(out, "{}", vcd_value(value, widths[&name], id));
        }
    }
    out
}

/// Renders the log in the Chrome trace-event JSON format, which Perfetto
/// (<https://ui.perfetto.dev>) loads directly. Timestamps are microseconds.
pub fn to_perfetto_json(log: &TraceLog) -> String {
    let events: Vec<serde_json::Value> = log
        .records()
        .iter()
        .map(|r| {
            let ts = r.t.0 as f64 / 1_000_000.0;
            match &r.event {
                TraceEvent::Signal { name, value, .. } => json!({
                    "name": name, "ph": "C", "ts": ts, "pid": 1, "tid": 1,
                    "args": { "value": value }
                }),
                TraceEvent::Uart { port, tx, byte } => json!({
                    "name": format!("uart{port} {}", if *tx { "tx" } else { "rx" }),
                    "ph": "i", "s": "t", "ts": ts, "pid": 1, "tid": 2,
                    "args": { "byte": byte, "char": char::from(*byte).to_string() }
                }),
                TraceEvent::Input { what } => json!({
                    "name": what, "ph": "i", "s": "g", "ts": ts, "pid": 1, "tid": 3
                }),
                TraceEvent::Marker { text } => json!({
                    "name": text, "ph": "i", "s": "g", "ts": ts, "pid": 1, "tid": 4
                }),
                TraceEvent::Bus {
                    port,
                    write,
                    addr,
                    len,
                    data,
                    ok,
                } => json!({
                    "name": format!("{port} {} {addr:#x}", if *write { "wr" } else { "rd" }),
                    "ph": "i", "s": "t", "ts": ts, "pid": 1, "tid": 5,
                    "args": { "len": len, "data": data, "ok": ok }
                }),
            }
        })
        .collect();
    json!({ "displayTimeUnit": "ns", "traceEvents": events }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::Ps;

    fn log() -> TraceLog {
        let mut l = TraceLog::new();
        l.push(
            Ps(0),
            TraceEvent::Signal {
                name: "board.led0".into(),
                value: 0,
                width: 1,
            },
        );
        l.push(
            Ps(1000),
            TraceEvent::Signal {
                name: "board.led0".into(),
                value: 1,
                width: 1,
            },
        );
        l.push(
            Ps(1000),
            TraceEvent::Uart {
                port: 1,
                tx: true,
                byte: 0x41,
            },
        );
        l.push(Ps(2000), TraceEvent::Marker { text: "end".into() });
        l
    }

    #[test]
    fn vcd_declares_and_dumps_changes() {
        let v = to_vcd(&log());
        assert!(v.contains("$timescale 1 ps $end"));
        assert!(v.contains("$var wire 1 ! board.led0 $end"));
        assert!(v.contains("$var wire 8 \" uart1.tx $end"));
        // Initial dump is x, then led0 goes 0 at #0 and 1 at #1000 with the byte.
        let body = v.split("$enddefinitions $end\n").nth(1).unwrap();
        assert!(body.contains("x!\nbx \"\n"));
        assert!(body.contains("#1000\n1!\nb01000001 \"\n"));
        // Markers produce no value changes and no timestamp.
        assert!(!body.contains("#2000"));
    }

    #[test]
    fn vcd_id_codes_do_not_collide() {
        let ids: std::collections::HashSet<String> = (0..20_000).map(vcd_id).collect();
        assert_eq!(ids.len(), 20_000);
        assert_eq!(vcd_id(0), "!");
        assert_eq!(vcd_id(93), "~");
        assert_eq!(vcd_id(94), "!!");
    }

    #[test]
    fn perfetto_is_valid_json_with_all_events() {
        let j: serde_json::Value = serde_json::from_str(&to_perfetto_json(&log())).unwrap();
        let evs = j["traceEvents"].as_array().unwrap();
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[1]["ph"], "C");
        assert_eq!(evs[1]["ts"], 0.001);
        assert_eq!(evs[2]["args"]["char"], "A");
    }
}
