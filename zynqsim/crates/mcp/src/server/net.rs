//! `net_capture` and `net_send`: the board's Ethernet (ADR-011).

use super::{McpServer, Out, unexpected};
use crate::util::to_hex;
use serde_json::{Value, json};
use sim_api::{ApiError, NetDirection, Query, Response};

/// Frame bytes shown per frame; the summary and `len` always describe the
/// whole frame.
const HEX_BYTES: usize = 96;

impl McpServer {
    /// A page of captured frames with the counters.
    pub(super) fn net_capture(&mut self, a: &Value) -> Result<Out, ApiError> {
        let from = a.get("from").and_then(Value::as_u64).unwrap_or(0);
        let max = a.get("max").and_then(Value::as_u64).unwrap_or(50).min(500) as usize;
        let stats = match self.client.query(Query::NetStats)? {
            Response::NetStats(s) => s,
            r => return Err(unexpected(&r)),
        };
        match self.client.query(Query::NetCapture { from, max })? {
            Response::Net {
                frames,
                next,
                total,
                oldest,
            } => {
                let frames: Vec<Value> = frames
                    .iter()
                    .map(|f| {
                        json!({
                            "index": f.index,
                            "t_ns": f.t_ns,
                            "direction": match f.dir {
                                NetDirection::Tx => "tx",
                                NetDirection::Rx => "rx",
                            },
                            "kind": f.kind,
                            "summary": f.summary,
                            "checksums_ok": f.checksums_ok,
                            "len": f.data.len(),
                            "hex": to_hex(&f.data[..f.data.len().min(HEX_BYTES)]),
                            "truncated": f.data.len() > HEX_BYTES,
                        })
                    })
                    .collect();
                Ok(Out::json(json!({
                    "stats": stats,
                    "frames": frames,
                    "next": next,
                    "total": total,
                    "oldest_retained": oldest,
                })))
            }
            r => Err(unexpected(&r)),
        }
    }

    /// Frame injection is not offered by the user-mode network backend.
    pub(super) fn net_send(&mut self, _a: &Value) -> Result<Out, ApiError> {
        Err(ApiError::unsupported(
            "`net_send` is not available: the board's network is QEMU's user-mode stack, \
             which has no host-side injection point, and a frame delivered from outside the \
             simulation would arrive at a host-dependent instruction",
        )
        .with_hint(
            "the simulated LAN answers ARP, ICMP echo and DHCP itself; see \
             docs/adr/011-ethernet.md for what a host-side peer would cost",
        ))
    }
}
