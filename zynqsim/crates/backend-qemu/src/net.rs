//! The host side of the board's Ethernet (PS GEM0): how QEMU's user-mode
//! network is configured, how the frames it carries are captured and decoded,
//! and the `.pcap` file the host writes from them. See ADR-011.
//!
//! ## Why the capture is a file
//!
//! QEMU's user-mode stack (slirp) is driven **synchronously by the vCPU thread**:
//! the guest's transmit is an MMIO write, slirp sees the frame inside that
//! write and its answer (ARP reply, ICMP echo reply, DHCP offer) is delivered to
//! the GEM before the write returns. Every frame therefore appears at an exact
//! guest instruction count, which a host socket peer cannot offer (the main
//! loop would deliver its reply at a host-scheduler-dependent moment). QEMU's
//! `filter-dump` object writes every frame to a pcap file from that same
//! thread, so by the time a sync is answered the frames of that quantum are
//! complete in the file, exactly like the `file:` UART path.
//!
//! The pcap *timestamps* QEMU writes are `wall-clock start seconds + virtual
//! time`, so the file itself is not byte-reproducible; the host re-reads the
//! frames and writes its own capture with virtual-time stamps.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

/// The default MAC: the Xilinx OUI and the `local-mac-address` of the co-sim
/// device tree's GEM0 node.
pub const DEFAULT_MAC: [u8; 6] = [0x00, 0x0a, 0x35, 0x00, 0x1e, 0x53];

/// Configuration of the board's Ethernet and its simulated network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetConfig {
    /// MAC address QEMU gives GEM0 (the reset value of its address register).
    pub mac: [u8; 6],
    /// Network address of the simulated LAN.
    pub subnet: Ipv4Addr,
    /// Prefix length of [`NetConfig::subnet`].
    pub prefix: u8,
    /// The router/host address: answers ARP and ping, serves DHCP.
    pub gateway: Ipv4Addr,
    /// Isolate the guest from the machine's real network (default).
    pub restrict: bool,
}

impl Default for NetConfig {
    fn default() -> NetConfig {
        NetConfig {
            mac: DEFAULT_MAC,
            subnet: Ipv4Addr::new(10, 0, 2, 0),
            prefix: 24,
            gateway: Ipv4Addr::new(10, 0, 2, 2),
            restrict: true,
        }
    }
}

/// Parses `aa:bb:cc:dd:ee:ff` (or `-` separated).
pub fn parse_mac(s: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = s.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(format!("`{s}` is not a MAC address like 00:0a:35:00:1e:53"));
    }
    let mut mac = [0u8; 6];
    for (b, p) in mac.iter_mut().zip(parts) {
        if p.len() != 2 {
            return Err(format!("`{s}` is not a MAC address like 00:0a:35:00:1e:53"));
        }
        *b = u8::from_str_radix(p, 16)
            .map_err(|_| format!("`{s}` is not a MAC address like 00:0a:35:00:1e:53"))?;
    }
    Ok(mac)
}

/// Parses `a.b.c.d/len`.
pub fn parse_cidr(s: &str) -> Result<(Ipv4Addr, u8), String> {
    let (a, l) = s
        .trim()
        .split_once('/')
        .ok_or_else(|| format!("`{s}` is not a network like 10.0.2.0/24"))?;
    let addr: Ipv4Addr = a
        .parse()
        .map_err(|_| format!("`{s}` is not a network like 10.0.2.0/24"))?;
    let len: u8 = l
        .parse()
        .ok()
        .filter(|&n| n <= 32)
        .ok_or_else(|| format!("`{s}` is not a network like 10.0.2.0/24"))?;
    Ok((addr, len))
}

impl NetConfig {
    /// Every reason the configuration cannot work (empty when valid).
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let mut p = Vec::new();
        if self.mac == [0; 6] {
            p.push(("mac", "must not be all zeros".to_string()));
        }
        if self.mac[0] & 1 != 0 {
            p.push((
                "mac",
                "has the multicast bit set; use a unicast address".to_string(),
            ));
        }
        if self.mac[0] == 0x52 && self.mac[1] == 0x55 {
            p.push((
                "mac",
                "52:55:... is the prefix of the simulated network's own addresses; \
                 the host tells the two directions apart by it"
                    .to_string(),
            ));
        }
        if !(8..=27).contains(&self.prefix) {
            p.push((
                "subnet",
                format!("prefix /{} is outside 8..=27", self.prefix),
            ));
            return p;
        }
        let mask = u32::MAX << (32 - u32::from(self.prefix));
        let net = u32::from(self.subnet);
        if net & mask != net {
            p.push((
                "subnet",
                format!(
                    "{}/{} has host bits set (network address is {})",
                    self.subnet,
                    self.prefix,
                    Ipv4Addr::from(net & mask)
                ),
            ));
        }
        let gw = u32::from(self.gateway);
        if gw & mask != net & mask {
            p.push((
                "gateway",
                format!("{} is not inside {}/{}", self.gateway, self.subnet, self.prefix),
            ));
        } else if gw == net & mask || gw == (net & mask) | !mask {
            p.push((
                "gateway",
                "must not be the network or the broadcast address".to_string(),
            ));
        } else if gw == (net & mask).wrapping_add(3) || gw == (net & mask).wrapping_add(15) {
            p.push((
                "gateway",
                "collides with the stack's DNS (.3) or first DHCP lease (.15) address".to_string(),
            ));
        }
        p
    }

    /// The QEMU arguments that attach the board's GEM0 to a user-mode network
    /// and mirror its frames to `pcap`.
    ///
    /// The generic-FDT machine creates both GEMs from the device tree; the
    /// first `-net nic` binds to GEM0 (`ethernet@e000b000`), found with
    /// `info network`/`info qtree`.
    pub fn qemu_args(&self, pcap: &Path) -> Vec<String> {
        let m = self.mac;
        let mut netdev = format!(
            "user,id=zsnet0,net={}/{},host={}",
            self.subnet, self.prefix, self.gateway
        );
        if self.restrict {
            netdev.push_str(",restrict=on");
        }
        vec![
            "-netdev".into(),
            netdev,
            "-net".into(),
            format!(
                "nic,netdev=zsnet0,macaddr={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            ),
            "-object".into(),
            // `,` is the option separator; a doubled comma is a literal one.
            format!(
                "filter-dump,id=zsdump0,netdev=zsnet0,file={}",
                pcap.display().to_string().replace(',', ",,")
            ),
        ]
    }
}

// ------------------------------------------------------------------ capture

/// One record read from QEMU's pcap file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFrame {
    /// `tv_sec` of the record (wall-clock start seconds plus virtual seconds).
    pub ts_sec: u32,
    /// `tv_usec`: microseconds of virtual time within the second.
    pub ts_usec: u32,
    /// The frame, without FCS.
    pub data: Vec<u8>,
}

/// Incremental reader of a pcap file that another process is appending to.
#[derive(Debug)]
pub struct PcapTail {
    path: PathBuf,
    offset: u64,
    buf: Vec<u8>,
    header: Option<bool>,
}

impl PcapTail {
    /// Follows `path`, which need not exist yet.
    pub fn new(path: PathBuf) -> PcapTail {
        PcapTail {
            path,
            offset: 0,
            buf: Vec::new(),
            header: None,
        }
    }

    /// Returns the records completed since the last call.
    pub fn poll(&mut self) -> Vec<RawFrame> {
        let Ok(mut f) = File::open(&self.path) else {
            return Vec::new();
        };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut chunk = Vec::new();
        if f.read_to_end(&mut chunk).is_ok() {
            self.offset += chunk.len() as u64;
            self.buf.extend_from_slice(&chunk);
        }
        self.parse()
    }

    fn parse(&mut self) -> Vec<RawFrame> {
        let mut out = Vec::new();
        let mut at = 0usize;
        if self.header.is_none() {
            if self.buf.len() < 24 {
                return out;
            }
            let m = [self.buf[0], self.buf[1], self.buf[2], self.buf[3]];
            // Classic pcap, microsecond resolution; either byte order.
            self.header = match m {
                [0xd4, 0xc3, 0xb2, 0xa1] => Some(true),
                [0xa1, 0xb2, 0xc3, 0xd4] => Some(false),
                _ => {
                    // Not a pcap file: drop everything rather than guess.
                    self.buf.clear();
                    return out;
                }
            };
            at = 24;
        }
        let le = self.header.unwrap_or(true);
        let word = |b: &[u8]| -> u32 {
            let a = [b[0], b[1], b[2], b[3]];
            if le {
                u32::from_le_bytes(a)
            } else {
                u32::from_be_bytes(a)
            }
        };
        while self.buf.len() >= at + 16 {
            let incl = word(&self.buf[at + 8..at + 12]) as usize;
            if incl > 262_144 {
                // Corrupt length: stop following instead of buffering forever.
                self.buf.clear();
                self.header = Some(le);
                return out;
            }
            if self.buf.len() < at + 16 + incl {
                break;
            }
            out.push(RawFrame {
                ts_sec: word(&self.buf[at..at + 4]),
                ts_usec: word(&self.buf[at + 4..at + 8]),
                data: self.buf[at + 16..at + 16 + incl].to_vec(),
            });
            at += 16 + incl;
        }
        self.buf.drain(..at);
        out
    }
}

/// The virtual time, in nanoseconds, a record was written at.
///
/// QEMU stamps `tv_sec = start_wall_seconds + virtual_seconds` and
/// `tv_usec = virtual_microseconds % 1_000_000`. The wall-clock part is not
/// known, but the record was written inside the window `(lo_ns, hi_ns]` the
/// host was running when it was found, which is far shorter than a second, so
/// the microsecond part identifies the instant uniquely.
pub fn resolve_virtual_ns(ts_usec: u32, lo_ns: u64, hi_ns: u64) -> u64 {
    let hi_us = hi_ns / 1000;
    let usec = u64::from(ts_usec).min(999_999);
    let k = hi_us.saturating_sub(usec) / 1_000_000;
    let t_us = k * 1_000_000 + usec;
    (t_us * 1000).clamp(lo_ns.min(hi_ns), hi_ns)
}

/// Which way a frame travels, from the board's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Board to network (sent by GEM0).
    Tx,
    /// Network to board (received by GEM0).
    Rx,
}

/// The simulated network's own frames carry a source MAC starting `52:55`
/// (QEMU's "special" Ethernet addresses); everything else comes from the board.
pub fn direction(frame: &[u8]) -> Direction {
    if frame.len() >= 12 && frame[6] == 0x52 && frame[7] == 0x55 {
        Direction::Rx
    } else {
        Direction::Tx
    }
}

/// A classification for counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// ARP.
    Arp,
    /// ICMP echo request.
    IcmpEchoRequest,
    /// ICMP echo reply.
    IcmpEchoReply,
    /// Any other ICMP.
    IcmpOther,
    /// UDP over IPv4.
    Udp,
    /// TCP over IPv4.
    Tcp,
    /// IPv4 with another protocol.
    Ipv4Other,
    /// IPv6.
    Ipv6,
    /// Anything else, including frames too short to decode.
    Other,
}

/// What [`describe`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Classification.
    pub kind: FrameKind,
    /// A one-line `tcpdump`-style summary.
    pub summary: String,
    /// False when an IPv4 header or ICMP checksum does not verify.
    pub checksums_ok: bool,
}

/// The RFC 1071 internet checksum (ones' complement sum, complemented) of
/// `data`; a buffer that already contains its checksum sums to zero.
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for c in &mut chunks {
        sum += u32::from(u16::from_be_bytes([c[0], c[1]]));
    }
    if let [last] = chunks.remainder() {
        sum += u32::from(*last) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn ip4(b: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(b[0], b[1], b[2], b[3])
}

fn mac_str(b: &[u8]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5]
    )
}

/// Decodes a frame far enough to count and summarise it. Never panics, whatever
/// the bytes: malformed frames are classified [`FrameKind::Other`].
pub fn describe(f: &[u8]) -> FrameInfo {
    let other = |text: String| FrameInfo {
        kind: FrameKind::Other,
        summary: text,
        checksums_ok: true,
    };
    if f.len() < 14 {
        return other(format!("runt frame ({} bytes)", f.len()));
    }
    let ethertype = be16(&f[12..14]);
    let l3 = &f[14..];
    match ethertype {
        0x0806 => {
            if l3.len() < 28 || be16(&l3[0..2]) != 1 || be16(&l3[2..4]) != 0x0800 || l3[4] != 6 || l3[5] != 4
            {
                return other(format!("malformed ARP ({} bytes)", f.len()));
            }
            let (sha, spa, tpa) = (&l3[8..14], ip4(&l3[14..18]), ip4(&l3[24..28]));
            let text = match be16(&l3[6..8]) {
                1 => format!("ARP who-has {tpa} tell {spa}"),
                2 => format!("ARP reply {spa} is-at {}", mac_str(sha)),
                op => format!("ARP op={op} {spa} > {tpa}"),
            };
            FrameInfo {
                kind: FrameKind::Arp,
                summary: text,
                checksums_ok: true,
            }
        }
        0x0800 => describe_ipv4(l3, f.len()),
        0x86dd => FrameInfo {
            kind: FrameKind::Ipv6,
            summary: format!("IPv6 ({} bytes)", f.len()),
            checksums_ok: true,
        },
        t => other(format!("ethertype 0x{t:04x} ({} bytes)", f.len())),
    }
}

fn describe_ipv4(ip: &[u8], frame_len: usize) -> FrameInfo {
    let bad = |what: &str| FrameInfo {
        kind: FrameKind::Other,
        summary: format!("malformed IPv4: {what} ({frame_len} bytes)"),
        checksums_ok: true,
    };
    if ip.len() < 20 || ip[0] >> 4 != 4 {
        return bad("short header or not version 4");
    }
    let ihl = usize::from(ip[0] & 0x0F) * 4;
    let total = usize::from(be16(&ip[2..4]));
    if ihl < 20 || total < ihl || ip.len() < total {
        return bad("inconsistent lengths");
    }
    let header_ok = internet_checksum(&ip[..ihl]) == 0;
    let (src, dst, proto) = (ip4(&ip[12..16]), ip4(&ip[16..20]), ip[9]);
    let l4 = &ip[ihl..total];
    let note = if header_ok { "" } else { " (bad IP checksum)" };
    match proto {
        1 => {
            if l4.len() < 8 {
                return bad("truncated ICMP");
            }
            let ok = internet_checksum(l4) == 0;
            let (ty, ident, seq) = (l4[0], be16(&l4[4..6]), be16(&l4[6..8]));
            let (kind, name) = match ty {
                8 => (FrameKind::IcmpEchoRequest, "echo-request"),
                0 => (FrameKind::IcmpEchoReply, "echo-reply"),
                _ => (FrameKind::IcmpOther, "other"),
            };
            let text = if matches!(ty, 0 | 8) {
                format!(
                    "ICMP {name} {src} > {dst} id=0x{ident:04x} seq={seq} data={}",
                    l4.len() - 8
                )
            } else {
                format!("ICMP type={ty} code={} {src} > {dst}", l4[1])
            };
            FrameInfo {
                kind,
                summary: format!("{text}{note}{}", if ok { "" } else { " (bad ICMP checksum)" }),
                checksums_ok: header_ok && ok,
            }
        }
        17 if l4.len() >= 8 => FrameInfo {
            kind: FrameKind::Udp,
            summary: format!(
                "UDP {src}:{} > {dst}:{} len={}{note}",
                be16(&l4[0..2]),
                be16(&l4[2..4]),
                l4.len() - 8
            ),
            checksums_ok: header_ok,
        },
        6 if l4.len() >= 20 => {
            let flags = l4[13];
            let names: Vec<&str> = [
                (0x02, "SYN"),
                (0x10, "ACK"),
                (0x08, "PSH"),
                (0x01, "FIN"),
                (0x04, "RST"),
            ]
            .iter()
            .filter(|(m, _)| flags & m != 0)
            .map(|(_, n)| *n)
            .collect();
            FrameInfo {
                kind: FrameKind::Tcp,
                summary: format!(
                    "TCP {src}:{} > {dst}:{} [{}]{note}",
                    be16(&l4[0..2]),
                    be16(&l4[2..4]),
                    names.join(",")
                ),
                checksums_ok: header_ok,
            }
        }
        p => FrameInfo {
            kind: FrameKind::Ipv4Other,
            summary: format!("IPv4 proto={p} {src} > {dst}{note}"),
            checksums_ok: header_ok,
        },
    }
}

/// A classic little-endian, microsecond-resolution pcap file (link type
/// Ethernet) of `(time in ns, frame)` pairs.
pub fn write_pcap<'a>(frames: impl IntoIterator<Item = (u64, &'a [u8])>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0xA1B2_C3D4u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&65_535u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    for (t_ns, data) in frames {
        let sec = (t_ns / 1_000_000_000) as u32;
        let usec = ((t_ns % 1_000_000_000) / 1000) as u32;
        let len = data.len().min(65_535) as u32;
        out.extend_from_slice(&sec.to_le_bytes());
        out.extend_from_slice(&usec.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data[..len as usize]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUEST: [u8; 6] = [0x02, 0, 0, 0xaa, 0xbb, 0x01];
    const GW: [u8; 6] = [0x52, 0x55, 0x0a, 0, 2, 2];

    fn eth(dst: [u8; 6], src: [u8; 6], ty: u16, payload: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&dst);
        f.extend_from_slice(&src);
        f.extend_from_slice(&ty.to_be_bytes());
        f.extend_from_slice(payload);
        f
    }

    fn arp(op: u16, sha: [u8; 6], spa: [u8; 4], tha: [u8; 6], tpa: [u8; 4]) -> Vec<u8> {
        let mut p = vec![0, 1, 8, 0, 6, 4];
        p.extend_from_slice(&op.to_be_bytes());
        p.extend_from_slice(&sha);
        p.extend_from_slice(&spa);
        p.extend_from_slice(&tha);
        p.extend_from_slice(&tpa);
        p
    }

    fn ipv4(proto: u8, src: [u8; 4], dst: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut h = vec![0x45, 0, 0, 0, 0, 0, 0, 0, 64, proto, 0, 0];
        h.extend_from_slice(&src);
        h.extend_from_slice(&dst);
        let total = (20 + body.len()) as u16;
        h[2..4].copy_from_slice(&total.to_be_bytes());
        let c = internet_checksum(&h);
        h[10..12].copy_from_slice(&c.to_be_bytes());
        h.extend_from_slice(body);
        h
    }

    fn icmp_echo(ty: u8, ident: u16, seq: u16, data: &[u8]) -> Vec<u8> {
        let mut m = vec![ty, 0, 0, 0];
        m.extend_from_slice(&ident.to_be_bytes());
        m.extend_from_slice(&seq.to_be_bytes());
        m.extend_from_slice(data);
        let c = internet_checksum(&m);
        m[2..4].copy_from_slice(&c.to_be_bytes());
        m
    }

    #[test]
    fn checksum_matches_rfc1071_vectors() {
        // RFC 1071 section 3: the sum of 00 01 f2 03 f4 f5 f6 f7 is ddf2.
        let v = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(internet_checksum(&v), !0xddf2);
        // Odd length: the last byte is padded with a zero.
        assert_eq!(internet_checksum(&[0xff]), !0xff00);
        assert_eq!(internet_checksum(&[]), 0xffff);
        // A buffer carrying its own checksum sums to zero.
        let mut m = icmp_echo(8, 1, 2, b"abc");
        assert_eq!(internet_checksum(&m), 0);
        m[8] ^= 1;
        assert_ne!(internet_checksum(&m), 0);
    }

    #[test]
    fn arp_request_and_reply_are_summarised() {
        let req = eth(
            [0xff; 6],
            GUEST,
            0x0806,
            &arp(1, GUEST, [10, 0, 2, 15], [0; 6], [10, 0, 2, 2]),
        );
        let i = describe(&req);
        assert_eq!(i.kind, FrameKind::Arp);
        assert_eq!(i.summary, "ARP who-has 10.0.2.2 tell 10.0.2.15");
        assert_eq!(direction(&req), Direction::Tx);

        let rep = eth(
            GUEST,
            GW,
            0x0806,
            &arp(2, GW, [10, 0, 2, 2], GUEST, [10, 0, 2, 15]),
        );
        let i = describe(&rep);
        assert_eq!(i.summary, "ARP reply 10.0.2.2 is-at 52:55:0a:00:02:02");
        assert_eq!(direction(&rep), Direction::Rx);
    }

    #[test]
    fn icmp_echo_pair_is_classified_and_verified() {
        let data = [7u8; 32];
        let req = eth(
            GW,
            GUEST,
            0x0800,
            &ipv4(1, [10, 0, 2, 15], [10, 0, 2, 2], &icmp_echo(8, 0x2b2b, 3, &data)),
        );
        let i = describe(&req);
        assert_eq!(i.kind, FrameKind::IcmpEchoRequest);
        assert!(i.checksums_ok);
        assert_eq!(
            i.summary,
            "ICMP echo-request 10.0.2.15 > 10.0.2.2 id=0x2b2b seq=3 data=32"
        );
        let rep = eth(
            GUEST,
            GW,
            0x0800,
            &ipv4(1, [10, 0, 2, 2], [10, 0, 2, 15], &icmp_echo(0, 0x2b2b, 3, &data)),
        );
        assert_eq!(describe(&rep).kind, FrameKind::IcmpEchoReply);
    }

    #[test]
    fn corrupted_checksums_are_reported_not_hidden() {
        let mut f = eth(
            GW,
            GUEST,
            0x0800,
            &ipv4(1, [10, 0, 2, 15], [10, 0, 2, 2], &icmp_echo(8, 1, 1, b"xy")),
        );
        let last = f.len() - 1;
        f[last] ^= 0x40; // payload bit: the ICMP checksum no longer holds
        let i = describe(&f);
        assert_eq!(i.kind, FrameKind::IcmpEchoRequest);
        assert!(!i.checksums_ok);
        assert!(i.summary.ends_with("(bad ICMP checksum)"), "{}", i.summary);

        let mut g = eth(
            GW,
            GUEST,
            0x0800,
            &ipv4(1, [10, 0, 2, 15], [10, 0, 2, 2], &icmp_echo(8, 1, 1, b"xy")),
        );
        g[14 + 8] ^= 1; // TTL: the IPv4 header checksum no longer holds
        assert!(!describe(&g).checksums_ok);
    }

    #[test]
    fn malformed_frames_never_panic_and_become_other() {
        assert_eq!(describe(&[]).kind, FrameKind::Other);
        assert!(describe(&[1, 2, 3]).summary.contains("runt"));
        // ARP truncated, IPv4 with a bogus IHL, a total length past the frame.
        let short_arp = eth([0xff; 6], GUEST, 0x0806, &[0, 1, 8, 0, 6, 4, 0, 1]);
        assert!(describe(&short_arp).summary.starts_with("malformed ARP"));
        let mut bad = ipv4(1, [1, 1, 1, 1], [2, 2, 2, 2], &[0; 8]);
        bad[0] = 0x42; // IHL 2 words
        assert_eq!(describe(&eth(GW, GUEST, 0x0800, &bad)).kind, FrameKind::Other);
        let mut long = ipv4(1, [1, 1, 1, 1], [2, 2, 2, 2], &[0; 8]);
        long[2..4].copy_from_slice(&900u16.to_be_bytes());
        assert!(
            describe(&eth(GW, GUEST, 0x0800, &long))
                .summary
                .contains("inconsistent lengths")
        );
        // Every prefix of a valid frame decodes without panicking.
        let ok = eth(
            GW,
            GUEST,
            0x0800,
            &ipv4(17, [10, 0, 2, 15], [10, 0, 2, 2], &[0, 68, 0, 67, 0, 8, 0, 0]),
        );
        for n in 0..=ok.len() {
            let _ = describe(&ok[..n]);
        }
        assert_eq!(describe(&ok).kind, FrameKind::Udp);
        // Pseudo-random garbage.
        let mut x = 0x1234_5678u32;
        for _ in 0..500 {
            let len = (x % 90) as usize;
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (x >> 24) as u8
                })
                .collect();
            let _ = describe(&junk);
        }
    }

    #[test]
    fn ethernet_padding_after_the_ip_packet_is_ignored() {
        let mut f = eth(
            GW,
            GUEST,
            0x0800,
            &ipv4(1, [10, 0, 2, 15], [10, 0, 2, 2], &icmp_echo(8, 1, 1, b"")),
        );
        f.extend_from_slice(&[0xAA; 18]);
        let i = describe(&f);
        assert!(i.checksums_ok, "{}", i.summary);
        assert!(i.summary.ends_with("data=0"), "{}", i.summary);
    }

    #[test]
    fn tail_reads_records_across_partial_writes() {
        let dir = std::env::temp_dir().join(format!("zs-pcaptail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.pcap");
        let _ = std::fs::remove_file(&path);
        let mut t = PcapTail::new(path.clone());
        assert!(t.poll().is_empty(), "a missing file is not an error");

        let a = vec![1u8; 42];
        let b = vec![2u8; 60];
        let file = write_pcap([(1_500_000_000u64, &a[..]), (2_000_250_000u64, &b[..])]);
        // Header only, then half of the first record, then the rest.
        std::fs::write(&path, &file[..24]).unwrap();
        assert!(t.poll().is_empty());
        std::fs::write(&path, &file[..24 + 16 + 10]).unwrap();
        assert!(t.poll().is_empty());
        std::fs::write(&path, &file[..24 + 16 + 42 + 5]).unwrap();
        let got = t.poll();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, a);
        assert_eq!((got[0].ts_sec, got[0].ts_usec), (1, 500_000));
        std::fs::write(&path, &file).unwrap();
        let got = t.poll();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, b);
        assert_eq!((got[0].ts_sec, got[0].ts_usec), (2, 250));
        assert!(t.poll().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_is_not_followed() {
        let dir = std::env::temp_dir().join(format!("zs-pcapgarbage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("g.pcap");
        std::fs::write(&path, vec![0x55u8; 200]).unwrap();
        let mut t = PcapTail::new(path);
        assert!(t.poll().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn virtual_time_is_recovered_from_the_microsecond_field() {
        // Window (1_000_000 ns, 1_100_000 ns]; the frame was written at 1.05 ms.
        assert_eq!(resolve_virtual_ns(1050, 1_000_000, 1_100_000), 1_050_000);
        // Past the first virtual second the microsecond field wraps.
        let hi = 3_000_040_000; // 3.00004 s
        assert_eq!(resolve_virtual_ns(20, hi - 100_000, hi), 3_000_020_000);
        assert_eq!(resolve_virtual_ns(999_990, hi - 100_000, hi), 2_999_990_000);
        // A nonsense stamp is clamped into the window instead of trusted.
        assert_eq!(resolve_virtual_ns(5, 9_000_000, 9_100_000), 9_000_000);
    }

    #[test]
    fn pcap_writer_output_is_deterministic_and_well_formed() {
        let f = vec![0xabu8; 60];
        let a = write_pcap([(1_000_000_123u64, &f[..])]);
        assert_eq!(a, write_pcap([(1_000_000_123u64, &f[..])]));
        assert_eq!(&a[..4], &[0xd4, 0xc3, 0xb2, 0xa1]);
        assert_eq!(u32::from_le_bytes(a[20..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(a[24..28].try_into().unwrap()), 1); // sec
        assert_eq!(u32::from_le_bytes(a[28..32].try_into().unwrap()), 0); // usec
        assert_eq!(a.len(), 24 + 16 + 60);
    }

    #[test]
    fn config_validation_lists_every_problem() {
        assert!(NetConfig::default().problems().is_empty());
        let bad = NetConfig {
            mac: [0x01, 0, 0, 0, 0, 0],
            subnet: Ipv4Addr::new(10, 0, 2, 7),
            prefix: 24,
            gateway: Ipv4Addr::new(192, 168, 0, 1),
            restrict: true,
        };
        let f: Vec<&str> = bad.problems().iter().map(|p| p.0).collect();
        assert_eq!(f, vec!["mac", "subnet", "gateway"]);
        let tiny = NetConfig {
            prefix: 30,
            ..NetConfig::default()
        };
        assert_eq!(tiny.problems()[0].0, "subnet");
        let clash = NetConfig {
            gateway: Ipv4Addr::new(10, 0, 2, 3),
            ..NetConfig::default()
        };
        assert!(clash.problems()[0].1.contains("DNS"));
        let own = NetConfig {
            mac: [0x52, 0x55, 1, 2, 3, 4],
            ..NetConfig::default()
        };
        assert_eq!(own.problems()[0].0, "mac");
    }

    #[test]
    fn qemu_arguments_bind_gem0_and_mirror_to_the_capture_file() {
        let a = NetConfig::default().qemu_args(Path::new("C:/tmp/a,b/net.pcap"));
        assert_eq!(a[0], "-netdev");
        assert_eq!(
            a[1],
            "user,id=zsnet0,net=10.0.2.0/24,host=10.0.2.2,restrict=on"
        );
        assert_eq!(
            a[3],
            "nic,netdev=zsnet0,macaddr=00:0a:35:00:1e:53"
        );
        assert!(a[5].ends_with("file=C:/tmp/a,,b/net.pcap"), "{}", a[5]);
        let open = NetConfig {
            restrict: false,
            ..NetConfig::default()
        };
        assert!(!open.qemu_args(Path::new("x"))[1].contains("restrict"));
    }

    #[test]
    fn mac_and_cidr_parsers() {
        assert_eq!(parse_mac("00:0A:35:00:1e:53").unwrap(), DEFAULT_MAC);
        assert_eq!(parse_mac("00-0a-35-00-1e-53").unwrap(), DEFAULT_MAC);
        for bad in ["", "00:0a:35", "zz:0a:35:00:1e:53", "000a:35:00:1e:53:11"] {
            assert!(parse_mac(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_cidr("10.0.2.0/24").unwrap(),
            (Ipv4Addr::new(10, 0, 2, 0), 24)
        );
        for bad in ["10.0.2.0", "10.0.2/24", "10.0.2.0/x", "10.0.2.0/"] {
            assert!(parse_cidr(bad).is_err(), "{bad}");
        }
    }
}
