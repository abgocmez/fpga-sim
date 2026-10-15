//! The engine's record of the board's Ethernet traffic: retained frames with
//! virtual-time stamps, counters, the trace events they produce and the
//! `.pcap` export. See ADR-011.

use crate::types::{NetDirection, NetFrame, NetStatus, Response};
use backend_qemu::net::{
    Direction, FrameInfo, FrameKind, NetConfig, RawFrame, describe, direction, resolve_virtual_ns,
    write_pcap,
};
use std::collections::VecDeque;
use trace::{Fnv64, TraceEvent};

/// Frames kept for `net_capture` and the pcap export; the counters and the
/// trace cover all of them.
const RETAIN: usize = 8192;

pub(crate) struct NetLog {
    frames: VecDeque<NetFrame>,
    /// Index of `frames[0]`.
    first: u64,
    total: u64,
    stats: NetStatus,
    /// Virtual time of the previous ingest: the start of the next window.
    last_ns: u64,
}

fn kind_name(k: FrameKind) -> &'static str {
    match k {
        FrameKind::Arp => "arp",
        FrameKind::IcmpEchoRequest | FrameKind::IcmpEchoReply | FrameKind::IcmpOther => "icmp",
        FrameKind::Udp => "udp",
        FrameKind::Tcp => "tcp",
        FrameKind::Ipv4Other => "ipv4",
        FrameKind::Ipv6 => "ipv6",
        FrameKind::Other => "other",
    }
}

impl NetLog {
    pub(crate) fn new(cfg: &NetConfig) -> NetLog {
        let m = cfg.mac;
        let stats = NetStatus {
            mac: format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            ),
            subnet: format!("{}/{}", cfg.subnet, cfg.prefix),
            gateway: cfg.gateway.to_string(),
            restricted: cfg.restrict,
            ..NetStatus::default()
        };
        NetLog {
            frames: VecDeque::new(),
            first: 0,
            total: 0,
            stats,
            last_ns: 0,
        }
    }

    /// Takes the frames found in the window `(last, now_ns]`, records them and
    /// returns the trace events to log, each with its virtual time in ns.
    pub(crate) fn ingest(&mut self, raw: Vec<RawFrame>, now_ns: u64) -> Vec<(u64, TraceEvent)> {
        let lo = self.last_ns;
        self.last_ns = self.last_ns.max(now_ns);
        let mut events = Vec::new();
        for r in raw {
            let t_ns = resolve_virtual_ns(r.ts_usec, lo, now_ns);
            let info: FrameInfo = describe(&r.data);
            let dir = match direction(&r.data) {
                Direction::Tx => NetDirection::Tx,
                Direction::Rx => NetDirection::Rx,
            };
            let len = r.data.len() as u64;
            match dir {
                NetDirection::Tx => {
                    self.stats.frames_tx += 1;
                    self.stats.bytes_tx += len;
                }
                NetDirection::Rx => {
                    self.stats.frames_rx += 1;
                    self.stats.bytes_rx += len;
                }
            }
            match kind_name(info.kind) {
                "arp" => self.stats.arp += 1,
                "icmp" => self.stats.icmp += 1,
                "udp" => self.stats.udp += 1,
                "tcp" => self.stats.tcp += 1,
                _ => self.stats.other += 1,
            }
            if !info.checksums_ok {
                self.stats.bad_checksums += 1;
            }
            // The digest covers the headers' summary and a hash of the whole
            // frame, so a payload difference changes it too.
            let mut h = Fnv64::new();
            h.write(&r.data);
            let (word, count) = match dir {
                NetDirection::Tx => ("tx", self.stats.frames_tx),
                NetDirection::Rx => ("rx", self.stats.frames_rx),
            };
            events.push((
                t_ns,
                TraceEvent::Marker {
                    text: format!(
                        "net {word} {len}B {} fnv={:08x}",
                        info.summary,
                        h.finish() as u32
                    ),
                },
            ));
            events.push((
                t_ns,
                TraceEvent::Signal {
                    name: format!("net.{word}_frames"),
                    value: count,
                    width: 32,
                },
            ));
            self.frames.push_back(NetFrame {
                index: self.total,
                t_ns,
                dir,
                kind: kind_name(info.kind).to_string(),
                summary: info.summary,
                checksums_ok: info.checksums_ok,
                data: r.data,
            });
            self.total += 1;
            while self.frames.len() > RETAIN {
                self.frames.pop_front();
                self.first += 1;
            }
        }
        events
    }

    pub(crate) fn status(&self) -> NetStatus {
        self.stats.clone()
    }

    pub(crate) fn page(&self, from: u64, max: usize) -> Response {
        let start = from.clamp(self.first, self.first + self.frames.len() as u64);
        let skip = (start - self.first) as usize;
        let frames: Vec<NetFrame> = self.frames.iter().skip(skip).take(max).cloned().collect();
        Response::Net {
            next: start + frames.len() as u64,
            frames,
            total: self.total,
            oldest: self.first,
        }
    }

    pub(crate) fn pcap(&self) -> Vec<u8> {
        write_pcap(self.frames.iter().map(|f| (f.t_ns, &f.data[..])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arp_request() -> Vec<u8> {
        let mut f = vec![0xff; 6];
        f.extend_from_slice(&[0x02, 0, 0, 0, 0, 1]);
        f.extend_from_slice(&[0x08, 0x06, 0, 1, 8, 0, 6, 4, 0, 1]);
        f.extend_from_slice(&[0x02, 0, 0, 0, 0, 1, 10, 0, 2, 15]);
        f.extend_from_slice(&[0; 6]);
        f.extend_from_slice(&[10, 0, 2, 2]);
        f
    }

    fn rx_frame() -> Vec<u8> {
        let mut f = arp_request();
        f[6..12].copy_from_slice(&[0x52, 0x55, 0x0a, 0, 2, 2]);
        f[21] = 2;
        f
    }

    #[test]
    fn frames_are_counted_timestamped_and_paged() {
        let mut log = NetLog::new(&NetConfig::default());
        let raw = vec![
            RawFrame {
                ts_sec: 7,
                ts_usec: 120,
                data: arp_request(),
            },
            RawFrame {
                ts_sec: 7,
                ts_usec: 125,
                data: rx_frame(),
            },
        ];
        let ev = log.ingest(raw, 130_000);
        assert_eq!(ev.len(), 4);
        assert_eq!(ev[0].0, 120_000);
        assert_eq!(ev[2].0, 125_000);
        let s = log.status();
        assert_eq!((s.frames_tx, s.frames_rx, s.arp), (1, 1, 2));
        assert_eq!(s.bytes_tx, 42);
        assert_eq!(s.mac, "00:0a:35:00:1e:53");
        assert_eq!(s.subnet, "10.0.2.0/24");
        let Response::Net {
            frames,
            next,
            total,
            oldest,
        } = log.page(0, 10)
        else {
            panic!("not a page");
        };
        assert_eq!((frames.len(), next, total, oldest), (2, 2, 2, 0));
        assert_eq!(frames[0].dir, NetDirection::Tx);
        assert_eq!(frames[1].dir, NetDirection::Rx);
        assert_eq!(frames[0].kind, "arp");
        // A later page starts where the previous ended.
        let Response::Net { frames, next, .. } = log.page(1, 10) else {
            panic!();
        };
        assert_eq!((frames.len(), next), (1, 2));
        let Response::Net { frames, .. } = log.page(5, 10) else {
            panic!();
        };
        assert!(frames.is_empty());
    }

    #[test]
    fn the_trace_digest_depends_on_the_payload() {
        let mut a = NetLog::new(&NetConfig::default());
        let mut b = NetLog::new(&NetConfig::default());
        // An ARP frame padded to the Ethernet minimum: the summary ignores the
        // padding, so only the frame hash can tell the two apart.
        let mut f1 = arp_request();
        f1.extend_from_slice(&[0; 18]);
        let mut f2 = f1.clone();
        f2[50] ^= 1;
        let mk = |d: Vec<u8>| {
            vec![RawFrame {
                ts_sec: 0,
                ts_usec: 5,
                data: d,
            }]
        };
        let ea = a.ingest(mk(f1), 10_000);
        let eb = b.ingest(mk(f2), 10_000);
        assert_ne!(ea[0].1, eb[0].1);
    }

    #[test]
    fn retention_is_bounded_but_counters_are_not() {
        let mut log = NetLog::new(&NetConfig::default());
        let raw: Vec<RawFrame> = (0..RETAIN + 10)
            .map(|_| RawFrame {
                ts_sec: 0,
                ts_usec: 1,
                data: arp_request(),
            })
            .collect();
        log.ingest(raw, 5_000);
        let Response::Net { total, oldest, .. } = log.page(0, 1) else {
            panic!();
        };
        assert_eq!((total, oldest), ((RETAIN + 10) as u64, 10));
        assert_eq!(log.status().frames_tx, (RETAIN + 10) as u64);
        // The export holds the retained frames only.
        assert_eq!(log.pcap().len(), 24 + RETAIN * (16 + 42));
    }
}
