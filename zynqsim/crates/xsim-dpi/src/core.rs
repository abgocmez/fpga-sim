//! Remote-port logic of the DPI library, independent of the OS socket layer.
//!
//! This is the Rust port of the prototype's `rp_dpi.c`. The testbench calls
//! these operations through DPI-C; each may pump incoming packets, so
//! concurrent Verilog processes can share the one connection. All entry points
//! are non-reentrant and single threaded.
//!
//! PS-to-PL accesses (the host plays "QEMU"):
//! * [`Dpi::next`] returns `1` when a 32-bit beat is ready, `2` when nothing is
//!   pending, `0` when the peer closed, `3` when a lock-step sync is pending.
//! * [`Dpi::done`] / [`Dpi::done_t`] complete the beat; the response is sent
//!   after the last beat of the bus access.
//!
//! PL-to-PS accesses: [`Dpi::mem_put`], [`Dpi::mem_start`], [`Dpi::mem_wait`],
//! [`Dpi::mem_get`], [`Dpi::mem_free`]; interrupts: [`Dpi::irq`].

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use interop::{
    cap, flags, resp, BusAccess, BusCmd, Header, Interrupt, Packet, Sync, HEADER_LEN,
    VERSION_MAJOR, VERSION_MINOR,
};

/// Largest single access handled, in bytes (4 KiB: an AXI burst boundary).
pub const DATA_MAX: usize = 4096;
const REQ_Q: usize = 8;
const NSLOT: usize = 4;
const RX_MAX: u32 = 64 * 1024;
/// Remote-port device number of the PL-to-PS interrupt wires.
pub const DEV_WIRES_IN: u32 = 9;
/// Device number of board input pins sent by the host (line 0 switches,
/// line 1 buttons).
pub const DEV_BOARD_IN: u32 = 12;
/// Device number of board output pins reported to the host (line 0 LEDs).
pub const DEV_BOARD_OUT: u32 = 13;
/// Video lines leaving the PL (HDMI TX): posted bus writes.
pub const DEV_VIDEO_OUT: u32 = 14;
/// Audio frames leaving the PL (codec DAC): posted bus writes.
pub const DEV_AUDIO_OUT: u32 = 15;
/// Video lines requested by the PL (HDMI RX): bus reads.
pub const DEV_VIDEO_IN: u32 = 16;
/// Audio frames requested by the PL (codec ADC): bus reads.
pub const DEV_AUDIO_IN: u32 = 17;
/// Codec register writes: interrupt packets, `line` = register, `vector` = value.
pub const DEV_CODEC_REG: u32 = 18;
/// Pin-group levels leaving the PL: interrupt packets, `line` = group, `val` =
/// eight pin levels.
pub const DEV_PIN_OUT: u32 = 19;
/// Pin-group input levels sent by the host: `line` = group, `val` = levels.
pub const DEV_PIN_IN: u32 = 20;
/// Audio frames sent per bus packet (and fetched per read).
const AUDIO_BLOCK: usize = 64;

/// Parses `tcp:a.b.c.d:port` or `a.b.c.d:port` (dotted IPv4 only) into the
/// four address octets and the port.
pub fn parse_endpoint(s: &[u8]) -> Option<([u8; 4], u16)> {
    let s = s.strip_prefix(b"tcp:").unwrap_or(s);
    let colon = s.iter().rposition(|&c| c == b':')?;
    let (host, port) = (&s[..colon], &s[colon + 1..]);
    if port.is_empty() {
        return None;
    }
    let mut port_v: u32 = 0;
    for &c in port {
        if !c.is_ascii_digit() {
            return None;
        }
        port_v = port_v * 10 + u32::from(c - b'0');
        if port_v > 65535 {
            return None;
        }
    }
    let mut octets = [0u8; 4];
    let mut n = 0;
    for part in host.split(|&c| c == b'.') {
        if n == 4 || part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut v: u32 = 0;
        for &c in part {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + u32::from(c - b'0');
        }
        if v > 255 {
            return None;
        }
        octets[n] = v as u8;
        n += 1;
    }
    (n == 4).then_some((octets, port_v as u16))
}

fn json_has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The unsigned integer following the first occurrence of `key`.
fn json_uint(hay: &[u8], key: &[u8]) -> Option<u64> {
    let at = hay.windows(key.len()).position(|w| w == key)? + key.len();
    let mut v: u64 = 0;
    let mut any = false;
    for &c in &hay[at..] {
        if !c.is_ascii_digit() {
            break;
        }
        v = v.wrapping_mul(10).wrapping_add(u64::from(c - b'0'));
        any = true;
    }
    any.then_some(v)
}

/// A byte stream to the host.
pub trait Transport {
    /// Writes all of `data`. False on error.
    fn send(&mut self, data: &[u8]) -> bool;
    /// Fills `buf` completely. False on EOF or error.
    fn recv_exact(&mut self, buf: &mut [u8]) -> bool;
    /// True when a read would not block (data, or EOF). `timeout_ms < 0`
    /// blocks until one of those.
    fn readable(&mut self, timeout_ms: i32) -> bool;
    /// Closes the stream.
    fn close(&mut self);
}

struct PsReq {
    req: BusAccess,
    off: u32,
    started: bool,
    t_first: u64,
    rdata: Vec<u8>,
}

#[derive(Default)]
struct Slot {
    used: bool,
    done: bool,
    ok: bool,
    is_write: bool,
    id: u32,
    len: u32,
    data: Vec<u8>,
}

/// One beat handed to the testbench.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beat {
    /// Word-aligned address.
    pub addr: u32,
    /// Write data (little-endian lanes).
    pub wdata: u32,
    /// Byte strobes, bit *n* enabling lane *n*.
    pub wstrb: u32,
    /// True for a write.
    pub is_write: bool,
}

/// Result of [`Dpi::next`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// The peer closed the connection.
    Closed,
    /// A beat is ready.
    Beat(Beat),
    /// Nothing pending right now.
    Idle,
    /// A sync is pending; advance PL time to [`Dpi::sync_target`].
    Sync,
}

/// The DPI library state.
pub struct Dpi<T: Transport> {
    t: Option<T>,
    tx_id: u32,
    peer_ext_base: bool,
    peer_wire_acks: bool,
    reqq: VecDeque<PsReq>,
    lockstep: bool,
    sync_pending: Option<(u32, u32, u64)>,
    irq_wait: Option<u32>,
    slots: [Slot; NSLOT],
    stage: Vec<u8>,
    /// Board input pins last sent by the host.
    switches: u8,
    buttons: u8,
    /// Latest host-driven levels per pin group.
    pins_in: [u8; 8],
    vtx: VideoTx,
    vrx_line: Vec<u8>,
    atx_buf: Vec<i16>,
    atx_first: u64,
    atx_count: u64,
    atx_t0: u64,
    arx_block: Vec<i16>,
    arx_block_start: u64,
    arx_index: u64,
}

/// Video-out assembly state.
#[derive(Default)]
struct VideoTx {
    frame: u32,
    y: u32,
    line: Vec<u8>,
    line_t: u64,
    in_active: bool,
    prev_vs: bool,
}

impl<T: Transport> Default for Dpi<T> {
    fn default() -> Self {
        Dpi::new()
    }
}

impl<T: Transport> Dpi<T> {
    /// A disconnected instance.
    pub fn new() -> Dpi<T> {
        Dpi {
            t: None,
            tx_id: 0,
            peer_ext_base: false,
            peer_wire_acks: false,
            reqq: VecDeque::new(),
            lockstep: false,
            sync_pending: None,
            irq_wait: None,
            slots: Default::default(),
            stage: vec![0; DATA_MAX],
            switches: 0,
            buttons: 0,
            pins_in: [0; 8],
            vtx: VideoTx::default(),
            vrx_line: Vec::new(),
            atx_buf: Vec::new(),
            atx_first: 0,
            atx_count: 0,
            atx_t0: 0,
            arx_block: Vec::new(),
            arx_block_start: u64::MAX,
            arx_index: 0,
        }
    }

    /// Adopts a connected transport and sends our hello. False on failure.
    pub fn open(&mut self, t: T) -> bool {
        self.t = Some(t);
        let id = self.alloc_id();
        let hello = Packet::Hello {
            id,
            dev: 0,
            major: VERSION_MAJOR,
            minor: VERSION_MINOR,
            caps: vec![cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES],
        };
        self.send(&hello)
    }

    /// Closes the connection.
    pub fn close(&mut self) {
        if let Some(t) = &mut self.t {
            t.close();
        }
        self.t = None;
    }

    /// True while connected.
    pub fn is_open(&self) -> bool {
        self.t.is_some()
    }

    /// Enables or disables lock-step: syncs are then answered by
    /// [`Dpi::sync_ack`] once the testbench has reached the target time.
    pub fn set_lockstep(&mut self, on: bool) {
        self.lockstep = on;
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.tx_id;
        self.tx_id = self.tx_id.wrapping_add(1);
        id
    }

    fn send(&mut self, p: &Packet) -> bool {
        match &mut self.t {
            Some(t) => t.send(&p.encode()),
            None => false,
        }
    }

    /// Reads and dispatches one packet. False on EOF or a malformed packet.
    fn pump_one(&mut self) -> bool {
        let Some(t) = &mut self.t else { return false };
        let mut h = [0u8; HEADER_LEN];
        if !t.recv_exact(&mut h) {
            return false;
        }
        let hdr = Header::parse(&h);
        if hdr.len > RX_MAX {
            return false;
        }
        let mut payload = vec![0u8; hdr.len as usize];
        if hdr.len > 0 && !t.recv_exact(&mut payload) {
            return false;
        }
        let Ok(pkt) = Packet::decode(hdr, &payload) else {
            return false;
        };
        self.handle(pkt);
        true
    }

    fn handle(&mut self, pkt: Packet) {
        match pkt {
            Packet::Hello { caps, .. } => {
                self.peer_ext_base = caps.contains(&cap::BUSACCESS_EXT_BASE);
                self.peer_wire_acks = caps.contains(&cap::WIRE_POSTED_UPDATES);
            }
            Packet::Bus(b) if b.is_response() => {
                if let Some(s) = self
                    .slots
                    .iter_mut()
                    .find(|s| s.used && !s.done && s.id == b.id)
                {
                    s.ok = b.response_code() == resp::OK;
                    if !s.is_write {
                        s.data = b.data;
                    }
                    s.done = true;
                }
            }
            Packet::Bus(b) => {
                let len = b.len as usize;
                if self.reqq.len() >= REQ_Q || len > DATA_MAX {
                    // Dropping a PS access hangs the PS; say so rather than
                    // silently losing it. (Cannot happen with one outstanding
                    // access per master.)
                    return;
                }
                let is_read = b.cmd == BusCmd::Read;
                self.reqq.push_back(PsReq {
                    rdata: if is_read { vec![0; len] } else { Vec::new() },
                    req: b,
                    off: 0,
                    started: false,
                    t_first: 0,
                });
            }
            Packet::Sync(s) => {
                if s.flags & flags::RESPONSE != 0 {
                    return;
                }
                if self.lockstep {
                    self.sync_pending = Some((s.id, s.dev, s.timestamp));
                } else {
                    self.send(&Packet::Sync(Sync {
                        flags: s.flags | flags::RESPONSE,
                        ..s
                    }));
                }
            }
            Packet::Interrupt(i) if i.dev == DEV_PIN_IN => {
                if let Some(slot) = self.pins_in.get_mut(i.line as usize) {
                    *slot = i.val;
                }
            }
            Packet::Interrupt(i) if i.dev == DEV_BOARD_IN => match i.line {
                0 => self.switches = i.val & 0xF,
                1 => self.buttons = i.val & 0xF,
                _ => {}
            },
            Packet::Interrupt(i) => {
                if i.flags & flags::RESPONSE != 0 {
                    if self.irq_wait == Some(i.id) {
                        self.irq_wait = None;
                    }
                } else if i.flags & flags::POSTED == 0 {
                    // PS-to-PL wire (reset...): acknowledge.
                    self.send(&Packet::Interrupt(Interrupt {
                        flags: i.flags | flags::RESPONSE,
                        ..i
                    }));
                }
            }
            Packet::Other(..) => {}
        }
    }

    /// Pumps at most one packet. `1` processed one, `0` none ready, `-1` EOF.
    pub fn poll(&mut self, timeout_ms: i32) -> i32 {
        let Some(t) = &mut self.t else { return -1 };
        if !t.readable(timeout_ms) {
            return 0;
        }
        if self.pump_one() {
            1
        } else {
            -1
        }
    }

    // ------------------------------------------------------------ PS -> PL

    /// Hands out the next beat, pumping packets as needed.
    pub fn next(&mut self, now_ns: u64, idle_ms: i32) -> Next {
        loop {
            if let Some(r) = self.reqq.front_mut() {
                let a = r.req.addr + u64::from(r.off);
                if !r.started {
                    r.started = true;
                    r.t_first = now_ns;
                }
                let lane = (a & 3) as u32;
                let remaining = r.req.len - r.off;
                let n = (4 - lane).min(remaining);
                let is_write = r.req.cmd == BusCmd::Write;
                let mut w = 0u32;
                if is_write {
                    for i in 0..n {
                        let b = r.req.data.get((r.off + i) as usize).copied().unwrap_or(0);
                        w |= u32::from(b) << (8 * (lane + i));
                    }
                }
                return Next::Beat(Beat {
                    addr: (a & !3) as u32,
                    wdata: w,
                    wstrb: ((1u32 << n) - 1) << lane,
                    is_write,
                });
            }
            if self.sync_pending.is_some() {
                return Next::Sync;
            }
            match self.poll(idle_ms) {
                0 => return Next::Idle,
                1 => {}
                _ => return Next::Closed,
            }
        }
    }

    /// Completes the current beat; replies after the last beat.
    pub fn done(&mut self, rdata: u32, ok: bool) {
        let Some(r) = self.reqq.front_mut() else {
            return;
        };
        let a = r.req.addr + u64::from(r.off);
        let lane = (a & 3) as u32;
        let remaining = r.req.len - r.off;
        let n = (4 - lane).min(remaining);
        if r.req.cmd == BusCmd::Read {
            for i in 0..n {
                let idx = (r.off + i) as usize;
                if idx < r.rdata.len() {
                    r.rdata[idx] = (rdata >> (8 * (lane + i))) as u8;
                }
            }
        }
        r.off += n;
        if r.off >= r.req.len {
            if let Some(r) = self.reqq.pop_front() {
                if !r.req.is_posted() {
                    let code = if ok {
                        resp::OK
                    } else {
                        resp::BUS_GENERIC_ERROR
                    };
                    let ts = r.req.timestamp;
                    let rsp = r.req.response(code, r.rdata, ts);
                    self.send(&Packet::Bus(rsp));
                }
            }
        }
    }

    /// Like [`Dpi::done`] but reports the PL-side access latency to the PS by
    /// advancing the response timestamp.
    pub fn done_t(&mut self, rdata: u32, ok: bool, now_ns: u64) {
        if let Some(r) = self.reqq.front_mut() {
            let lat = now_ns.saturating_sub(r.t_first);
            r.req.timestamp = r.req.timestamp.saturating_add(lat);
        }
        self.done(rdata, ok);
    }

    /// PS timestamp of the access at the head of the queue, or -1.
    pub fn req_ts(&self) -> i64 {
        self.reqq
            .front()
            .map_or(-1, |r| r.req.timestamp.min(i64::MAX as u64) as i64)
    }

    // ----------------------------------------------------------- time lock

    /// PL time (ns) the PS wants us to reach before continuing, or -1.
    pub fn sync_target(&self) -> i64 {
        self.sync_pending
            .map_or(-1, |(_, _, ts)| ts.min(i64::MAX as u64) as i64)
    }

    /// Answers the pending sync with the PL time reached.
    pub fn sync_ack(&mut self, now_ns: u64) {
        if let Some((id, dev, _)) = self.sync_pending.take() {
            self.send(&Packet::Sync(Sync {
                id,
                flags: flags::RESPONSE,
                dev,
                timestamp: now_ns,
            }));
        }
    }

    // -------------------------------------------------------------- media

    /// One pixel-clock sample of the HDMI TX pins. Active pixels are gathered
    /// into a line and sent when `de` falls; a rising `vs` ends the frame.
    /// `vs` must be active-high (the testbench helper normalises polarity).
    pub fn video_tx_pixel(&mut self, de: bool, vs: bool, rgb: u32, now_ns: u64) {
        if vs && !self.vtx.prev_vs {
            if self.vtx.y > 0 {
                let frame = self.vtx.frame;
                let id = self.alloc_id();
                self.send(&Packet::Interrupt(Interrupt {
                    id,
                    flags: flags::POSTED,
                    dev: DEV_VIDEO_OUT,
                    timestamp: now_ns,
                    vector: u64::from(frame),
                    line: 0,
                    val: 1,
                }));
                self.vtx.frame = frame.wrapping_add(1);
            }
            self.vtx.y = 0;
        }
        self.vtx.prev_vs = vs;
        if de {
            if !self.vtx.in_active {
                self.vtx.in_active = true;
                self.vtx.line.clear();
                self.vtx.line_t = now_ns;
            }
            self.vtx
                .line
                .extend_from_slice(&[(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]);
        } else if self.vtx.in_active {
            self.vtx.in_active = false;
            let id = self.alloc_id();
            let len = self.vtx.line.len() as u32;
            let req = BusAccess {
                cmd: BusCmd::Write,
                id,
                flags: flags::POSTED,
                dev: DEV_VIDEO_OUT,
                timestamp: self.vtx.line_t,
                attributes: 0,
                addr: (u64::from(self.vtx.frame) << 16) | u64::from(self.vtx.y),
                len,
                width: 0,
                stream_width: len,
                master_id: 0,
                data: core::mem::take(&mut self.vtx.line),
                byte_enable: Vec::new(),
                ext: self.peer_ext_base,
            };
            self.send(&Packet::Bus(req));
            self.vtx.y += 1;
        }
    }

    /// Blocking read of `len` bytes from a host media source.
    fn host_read(
        &mut self,
        dev: u32,
        addr: u64,
        len: u32,
        hint: u32,
        now_ns: u64,
    ) -> Option<Vec<u8>> {
        if len == 0 || len as usize > DATA_MAX * 64 {
            return None;
        }
        let slot = self.slots.iter().position(|s| !s.used)?;
        let id = self.alloc_id();
        let req = BusAccess {
            cmd: BusCmd::Read,
            id,
            flags: 0,
            dev,
            timestamp: now_ns,
            attributes: 0,
            addr,
            len,
            width: hint,
            stream_width: len,
            master_id: 0,
            data: Vec::new(),
            byte_enable: Vec::new(),
            ext: self.peer_ext_base,
        };
        if !self.send(&Packet::Bus(req)) {
            return None;
        }
        self.slots[slot] = Slot {
            used: true,
            done: false,
            ok: false,
            is_write: false,
            id,
            len,
            data: Vec::new(),
        };
        while !self.slots[slot].done {
            if self.poll(-1) < 0 {
                self.slots[slot].used = false;
                return None;
            }
        }
        self.slots[slot].used = false;
        let ok = self.slots[slot].ok;
        let data = core::mem::take(&mut self.slots[slot].data);
        (ok && data.len() == len as usize).then_some(data)
    }

    /// Fetches line `y` of `frame` (`w` pixels of a `h`-line frame) from the
    /// host's video source. Returns 0 on success, -1 otherwise (the line then
    /// reads as black).
    pub fn video_rx_line(&mut self, frame: u32, y: u32, w: u32, h: u32, now_ns: u64) -> i32 {
        let addr = (u64::from(frame) << 16) | u64::from(y);
        match self.host_read(DEV_VIDEO_IN, addr, w * 3, h, now_ns) {
            Some(d) => {
                self.vrx_line = d;
                0
            }
            None => {
                self.vrx_line.clear();
                self.vrx_line.resize((w * 3) as usize, 0);
                -1
            }
        }
    }

    /// Pixel `x` of the fetched line as 0xRRGGBB.
    pub fn video_rx_pixel(&self, x: u32) -> u32 {
        let i = x as usize * 3;
        match self.vrx_line.get(i..i + 3) {
            Some(p) => (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]),
            None => 0,
        }
    }

    /// One stereo frame from the codec DAC. Frames are sent in blocks.
    pub fn audio_tx_frame(&mut self, left: i16, right: i16, now_ns: u64) {
        if self.atx_buf.is_empty() {
            self.atx_first = self.atx_count;
            self.atx_t0 = now_ns;
        }
        self.atx_buf.push(left);
        self.atx_buf.push(right);
        self.atx_count += 1;
        if self.atx_buf.len() >= AUDIO_BLOCK * 2 {
            self.audio_flush();
        }
    }

    /// Sends any buffered DAC frames.
    pub fn audio_flush(&mut self) {
        if self.atx_buf.is_empty() {
            return;
        }
        let mut data = Vec::with_capacity(self.atx_buf.len() * 2);
        for s in &self.atx_buf {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let len = data.len() as u32;
        let id = self.alloc_id();
        let req = BusAccess {
            cmd: BusCmd::Write,
            id,
            flags: flags::POSTED,
            dev: DEV_AUDIO_OUT,
            timestamp: self.atx_t0,
            attributes: 0,
            addr: self.atx_first,
            len,
            width: 0,
            stream_width: len,
            master_id: 0,
            data,
            byte_enable: Vec::new(),
            ext: self.peer_ext_base,
        };
        self.send(&Packet::Bus(req));
        self.atx_buf.clear();
    }

    /// The next stereo frame for the codec ADC, from the host's audio source.
    pub fn audio_rx_frame(&mut self, now_ns: u64) -> (i16, i16) {
        let idx = self.arx_index;
        self.arx_index += 1;
        let block = idx / AUDIO_BLOCK as u64 * AUDIO_BLOCK as u64;
        if self.arx_block_start != block {
            let len = (AUDIO_BLOCK * 4) as u32;
            self.arx_block = match self.host_read(DEV_AUDIO_IN, block, len, 0, now_ns) {
                Some(d) => d
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect(),
                None => vec![0; AUDIO_BLOCK * 2],
            };
            self.arx_block_start = block;
        }
        let i = ((idx - block) * 2) as usize;
        (
            self.arx_block.get(i).copied().unwrap_or(0),
            self.arx_block.get(i + 1).copied().unwrap_or(0),
        )
    }

    /// Reports a codec register write seen on the I2C pins.
    pub fn codec_register(&mut self, reg: u32, value: u32, now_ns: u64) {
        let id = self.alloc_id();
        self.send(&Packet::Interrupt(Interrupt {
            id,
            flags: flags::POSTED,
            dev: DEV_CODEC_REG,
            timestamp: now_ns,
            vector: u64::from(value & 0x1FF),
            line: reg & 0xF,
            val: 0,
        }));
    }

    // --------------------------------------------------------- board pins

    /// The board input pins as `(switches, buttons)`. Pumps any packets that
    /// are already waiting so a freshly sent input is seen without needing a
    /// PS access, and returns the latest values.
    pub fn board_inputs(&mut self) -> (u8, u8) {
        // Bounded: a sync or bus request found here is queued for `next`.
        for _ in 0..16 {
            if self.poll(0) != 1 {
                break;
            }
        }
        (self.switches, self.buttons)
    }

    /// Reports the levels of pin group `group` to the host, stamped `now_ns`.
    pub fn pin_out(&mut self, group: u32, levels: u32, now_ns: u64) {
        let id = self.alloc_id();
        self.send(&Packet::Interrupt(Interrupt {
            id,
            flags: flags::POSTED,
            dev: DEV_PIN_OUT,
            timestamp: now_ns,
            vector: 0,
            line: group & 0xFF,
            val: (levels & 0xFF) as u8,
        }));
    }

    /// Reports the levels of pin group `group` and waits for the host's
    /// answer: the levels the devices on the port drive into the PL's input
    /// pins of that group (1 = released). No simulated time passes, so the
    /// answer is in effect in the instant of the change (ADR-009). A host
    /// that is gone, or has no device model, answers `0xFF`.
    pub fn pin_xchg(&mut self, group: u32, levels: u32, now_ns: u64) -> u8 {
        let addr = u64::from(group & 0xFF) | u64::from(levels & 0xFF) << 8;
        match self.host_read(DEV_PIN_OUT, addr, 1, 0, now_ns) {
            Some(d) if d.len() == 1 => d[0],
            _ => 0xFF,
        }
    }

    /// The latest host-driven levels of pin group `group`. Pumps packets that
    /// are already waiting, like [`Dpi::board_inputs`].
    pub fn pin_in(&mut self, group: u32) -> u8 {
        for _ in 0..16 {
            if self.poll(0) != 1 {
                break;
            }
        }
        self.pins_in.get(group as usize).copied().unwrap_or(0)
    }

    /// Handles a testbench telemetry event. Only `{"k":"led","v":N,"t":T}` is
    /// meaningful: it reports the LED pins to the host. Everything else is
    /// dropped (the GUI reads the trace through the control plane instead).
    pub fn telemetry_event(&mut self, json: &[u8], now_ns: u64) {
        if !json_has(json, b"\"k\":\"led\"") {
            return;
        }
        let Some(v) = json_uint(json, b"\"v\":") else {
            return;
        };
        let t = json_uint(json, b"\"t\":").unwrap_or(now_ns);
        let id = self.alloc_id();
        self.send(&Packet::Interrupt(Interrupt {
            id,
            flags: flags::POSTED,
            dev: DEV_BOARD_OUT,
            timestamp: t,
            vector: 0,
            line: 0,
            val: (v & 0xF) as u8,
        }));
    }

    // ----------------------------------------------------------- PL -> PS

    /// Stages word `idx` of a write.
    pub fn mem_put(&mut self, idx: i32, word: u32) {
        if idx >= 0 && (idx as usize + 1) * 4 <= DATA_MAX {
            let o = idx as usize * 4;
            self.stage[o..o + 4].copy_from_slice(&word.to_le_bytes());
        }
    }

    /// Starts a read or write of `len` bytes. Returns a slot or -1.
    pub fn mem_start(&mut self, now_ns: u64, dev: u32, is_write: bool, addr: u32, len: i32) -> i32 {
        if len <= 0 || len as usize > DATA_MAX {
            return -1;
        }
        let Some(slot) = self.slots.iter().position(|s| !s.used) else {
            return -1;
        };
        let id = self.alloc_id();
        let len = len as u32;
        let req = BusAccess {
            cmd: if is_write {
                BusCmd::Write
            } else {
                BusCmd::Read
            },
            id,
            flags: 0,
            dev,
            timestamp: now_ns,
            attributes: 0,
            addr: u64::from(addr),
            len,
            width: 0,
            stream_width: len,
            master_id: 0,
            data: if is_write {
                self.stage[..len as usize].to_vec()
            } else {
                Vec::new()
            },
            byte_enable: Vec::new(),
            ext: self.peer_ext_base,
        };
        if !self.send(&Packet::Bus(req)) {
            return -1;
        }
        self.slots[slot] = Slot {
            used: true,
            done: false,
            ok: false,
            is_write,
            id,
            len,
            data: Vec::new(),
        };
        slot as i32
    }

    /// `0` pending, `1` done ok, `2` done with error, `-1` EOF / bad slot.
    pub fn mem_poll(&mut self, slot: i32) -> i32 {
        let Some(i) = self.slot_index(slot) else {
            return -1;
        };
        if !self.slots[i].done && self.poll(0) < 0 {
            return -1;
        }
        let s = &self.slots[i];
        match (s.done, s.ok) {
            (false, _) => 0,
            (true, true) => 1,
            (true, false) => 2,
        }
    }

    /// Blocks (real time, no PL time advance) until the response arrives. This
    /// is what keeps simulated time independent of host speed: the modelled
    /// latency is added by the testbench.
    pub fn mem_wait(&mut self, slot: i32) -> i32 {
        let Some(i) = self.slot_index(slot) else {
            return -1;
        };
        while !self.slots[i].done {
            if self.poll(-1) < 0 {
                return -1;
            }
        }
        if self.slots[i].ok {
            1
        } else {
            2
        }
    }

    /// Word `idx` of a finished read.
    pub fn mem_get(&self, slot: i32, idx: i32) -> u32 {
        let Some(i) = self.slot_index(slot) else {
            return 0;
        };
        let s = &self.slots[i];
        if idx < 0 || (idx as usize + 1) * 4 > s.len as usize {
            return 0;
        }
        let o = idx as usize * 4;
        match s.data.get(o..o + 4) {
            Some(b) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            None => 0,
        }
    }

    /// Releases a slot.
    pub fn mem_free(&mut self, slot: i32) {
        if let Some(i) = self.slot_index(slot) {
            self.slots[i].used = false;
        }
    }

    fn slot_index(&self, slot: i32) -> Option<usize> {
        let i = usize::try_from(slot).ok()?;
        (i < NSLOT && self.slots[i].used).then_some(i)
    }

    /// Drives `IRQ_F2P[line]`. With an acknowledging peer, waits (real time)
    /// until it has applied the wire so delivery is deterministic.
    pub fn irq(&mut self, now_ns: u64, line: u32, val: bool) {
        let id = self.alloc_id();
        let acked = self.peer_wire_acks;
        self.send(&Packet::Interrupt(Interrupt {
            id,
            flags: if acked { 0 } else { flags::POSTED },
            dev: DEV_WIRES_IN,
            timestamp: now_ns,
            vector: 0,
            line,
            val: u8::from(val),
        }));
        if acked {
            self.irq_wait = Some(id);
            while self.irq_wait.is_some() {
                if self.poll(-1) < 0 {
                    break;
                }
            }
            self.irq_wait = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interop::{attr, Session};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    /// A std TCP stream as the DPI transport.
    struct StdTransport(TcpStream);

    impl Transport for StdTransport {
        fn send(&mut self, data: &[u8]) -> bool {
            use std::io::Write;
            self.0.write_all(data).is_ok()
        }
        fn recv_exact(&mut self, buf: &mut [u8]) -> bool {
            use std::io::Read;
            let _ = self.0.set_read_timeout(None);
            self.0.read_exact(buf).is_ok()
        }
        fn readable(&mut self, timeout_ms: i32) -> bool {
            let t = if timeout_ms < 0 {
                None
            } else {
                Some(Duration::from_millis(timeout_ms.max(1) as u64))
            };
            let _ = self.0.set_read_timeout(t);
            let mut b = [0u8; 1];
            // Ok(n): data, or Ok(0) = EOF; both mean a read will not block.
            self.0.peek(&mut b).is_ok()
        }
        fn close(&mut self) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }

    /// DPI client wired to a host-side `Session` (the "QEMU" role).
    fn pair() -> (Dpi<StdTransport>, Session) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let c = TcpStream::connect(addr).unwrap();
        c.set_nodelay(true).unwrap();
        let (s, _) = l.accept().unwrap();
        let mut host = Session::from_stream(s).unwrap();
        let mut dpi = Dpi::new();
        assert!(dpi.open(StdTransport(c)));
        // Host side: read the DPI hello, then answer with ours.
        assert!(matches!(host.recv().unwrap(), Packet::Hello { .. }));
        host.send_hello(&[cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES])
            .unwrap();
        // Consume the peer's hello so its capabilities are known, as happens
        // in a real run before the first interrupt.
        assert_eq!(dpi.poll(2000), 1);
        (dpi, host)
    }

    fn bus(host: &mut Session, cmd: BusCmd, addr: u64, data: &[u8], len: u32, ts: u64) -> u32 {
        let id = host.alloc_id();
        host.send(&Packet::Bus(BusAccess {
            cmd,
            id,
            flags: 0,
            dev: 7,
            timestamp: ts,
            attributes: attr::EXT_BASE,
            addr,
            len,
            width: 0,
            stream_width: len,
            master_id: 0,
            data: data.to_vec(),
            byte_enable: Vec::new(),
            ext: true,
        }))
        .unwrap();
        id
    }

    #[test]
    fn write_beats_follow_lanes_and_respond_after_the_last() {
        let (mut dpi, mut host) = pair();
        // 6 bytes at 0x...2: lanes 2,3 then lanes 0..3 of the next word.
        let id = bus(
            &mut host,
            BusCmd::Write,
            0x43C0_0002,
            &[1, 2, 3, 4, 5, 6],
            6,
            100,
        );
        let Next::Beat(b0) = dpi.next(0, 1000) else {
            panic!("expected a beat")
        };
        assert_eq!(b0.addr, 0x43C0_0000);
        assert!(b0.is_write);
        assert_eq!(b0.wstrb, 0b1100);
        assert_eq!(b0.wdata, 0x0201_0000);
        dpi.done(0, true);
        assert!(host
            .recv_timeout(Duration::from_millis(50))
            .unwrap()
            .is_none());
        let Next::Beat(b1) = dpi.next(0, 1000) else {
            panic!("expected a second beat")
        };
        assert_eq!((b1.addr, b1.wstrb), (0x43C0_0004, 0b1111));
        assert_eq!(b1.wdata, u32::from_le_bytes([3, 4, 5, 6]));
        dpi.done(0, true);
        match host.recv().unwrap() {
            Packet::Bus(r) => {
                assert!(r.is_response());
                assert_eq!(r.id, id);
                assert_eq!(r.response_code(), resp::OK);
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn read_assembles_bytes_from_lanes_and_reports_latency() {
        let (mut dpi, mut host) = pair();
        let id = bus(&mut host, BusCmd::Read, 0x43C0_0004, &[], 4, 1000);
        let Next::Beat(b) = dpi.next(2000, 1000) else {
            panic!()
        };
        assert!(!b.is_write);
        assert_eq!(dpi.req_ts(), 1000);
        dpi.done_t(0xDEAD_BEEF, true, 2160); // 160 ns of PL time elapsed
        match host.recv().unwrap() {
            Packet::Bus(r) => {
                assert_eq!(r.id, id);
                assert_eq!(r.data, 0xDEAD_BEEFu32.to_le_bytes());
                assert_eq!(r.timestamp, 1000 + 160);
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn posted_writes_get_no_response_and_errors_are_reported() {
        let (mut dpi, mut host) = pair();
        let id = host.alloc_id();
        host.send(&Packet::Bus(BusAccess {
            cmd: BusCmd::Write,
            id,
            flags: flags::POSTED,
            dev: 7,
            timestamp: 5,
            attributes: attr::EXT_BASE,
            addr: 0x43C0_0000,
            len: 4,
            width: 0,
            stream_width: 4,
            master_id: 0,
            data: vec![0; 4],
            byte_enable: Vec::new(),
            ext: true,
        }))
        .unwrap();
        assert!(matches!(dpi.next(0, 1000), Next::Beat(_)));
        dpi.done(0, true);
        assert!(host
            .recv_timeout(Duration::from_millis(50))
            .unwrap()
            .is_none());

        bus(&mut host, BusCmd::Read, 0x43C0_0000, &[], 4, 6);
        assert!(matches!(dpi.next(0, 1000), Next::Beat(_)));
        dpi.done(0, false);
        match host.recv().unwrap() {
            Packet::Bus(r) => assert_eq!(r.response_code(), resp::BUS_GENERIC_ERROR),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn lockstep_holds_the_sync_until_acked() {
        let (mut dpi, mut host) = pair();
        dpi.set_lockstep(true);
        host.send(&Packet::Sync(Sync {
            id: 77,
            flags: 0,
            dev: 7,
            timestamp: 5000,
        }))
        .unwrap();
        assert_eq!(dpi.next(0, 1000), Next::Sync);
        assert_eq!(dpi.sync_target(), 5000);
        assert!(host
            .recv_timeout(Duration::from_millis(50))
            .unwrap()
            .is_none());
        dpi.sync_ack(5000);
        match host.recv().unwrap() {
            Packet::Sync(s) => {
                assert_eq!((s.id, s.timestamp), (77, 5000));
                assert_ne!(s.flags & flags::RESPONSE, 0);
            }
            p => panic!("{p:?}"),
        }
        assert_eq!(dpi.sync_target(), -1);
    }

    #[test]
    fn without_lockstep_syncs_are_answered_immediately() {
        let (mut dpi, mut host) = pair();
        host.send(&Packet::Sync(Sync {
            id: 1,
            flags: 0,
            dev: 7,
            timestamp: 42,
        }))
        .unwrap();
        assert_eq!(dpi.next(0, 50), Next::Idle);
        assert!(matches!(host.recv().unwrap(), Packet::Sync(s) if s.timestamp == 42));
    }

    #[test]
    fn pl_master_read_and_write_round_trip() {
        let (mut dpi, mut host) = pair();
        let t = std::thread::spawn(move || {
            // Host plays the PS memory: answer one read, then accept one write.
            match host.recv().unwrap() {
                Packet::Bus(r) if r.cmd == BusCmd::Read => {
                    assert_eq!((r.dev, r.addr, r.len), (2, 0x0100_0000, 8));
                    let rsp = r.response(resp::OK, vec![1, 0, 0, 0, 2, 0, 0, 0], r.timestamp);
                    host.send(&Packet::Bus(rsp)).unwrap();
                }
                p => panic!("{p:?}"),
            }
            match host.recv().unwrap() {
                Packet::Bus(w) if w.cmd == BusCmd::Write => {
                    assert_eq!(w.data, vec![9, 0, 0, 0, 8, 0, 0, 0]);
                    host.send(&Packet::Bus(w.response(resp::OK, vec![], w.timestamp)))
                        .unwrap();
                }
                p => panic!("{p:?}"),
            }
            host
        });

        let s = dpi.mem_start(10, 2, false, 0x0100_0000, 8);
        assert!(s >= 0);
        assert_eq!(dpi.mem_wait(s), 1);
        assert_eq!(
            (dpi.mem_get(s, 0), dpi.mem_get(s, 1), dpi.mem_get(s, 2)),
            (1, 2, 0)
        );
        dpi.mem_free(s);

        dpi.mem_put(0, 9);
        dpi.mem_put(1, 8);
        let w = dpi.mem_start(20, 2, true, 0x0100_0100, 8);
        assert_eq!(dpi.mem_wait(w), 1);
        dpi.mem_free(w);
        assert_eq!(dpi.mem_poll(w), -1, "freed slots are invalid");
        let _ = t.join().unwrap();
    }

    #[test]
    fn irq_waits_for_the_acknowledgement() {
        let (mut dpi, mut host) = pair();
        let t = std::thread::spawn(move || {
            match host.recv().unwrap() {
                Packet::Interrupt(i) => {
                    assert_eq!(
                        (i.dev, i.line, i.val, i.timestamp),
                        (DEV_WIRES_IN, 3, 1, 777)
                    );
                    assert_eq!(
                        i.flags & flags::POSTED,
                        0,
                        "acked peers are asked for an ack"
                    );
                    host.send(&Packet::Interrupt(Interrupt {
                        flags: i.flags | flags::RESPONSE,
                        ..i
                    }))
                    .unwrap();
                }
                p => panic!("{p:?}"),
            }
            host
        });
        dpi.irq(777, 3, true); // returns only after the ack
        let _ = t.join().unwrap();
    }

    #[test]
    fn board_inputs_arrive_and_led_events_are_reported() {
        let (mut dpi, mut host) = pair();
        for (line, val) in [(0u32, 0b0110u8), (1, 0b0001)] {
            host.send(&Packet::Interrupt(Interrupt {
                id: 0,
                flags: flags::POSTED,
                dev: DEV_BOARD_IN,
                timestamp: 10,
                vector: 0,
                line,
                val,
            }))
            .unwrap();
        }
        // The values are visible after pumping; give the loopback a moment.
        let mut got = (0, 0);
        for _ in 0..200 {
            got = dpi.board_inputs();
            if got == (0b0110, 0b0001) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, (0b0110, 0b0001));

        dpi.telemetry_event(br#"{"t":1234,"k":"led","v":5}"#, 99);
        match host.recv().unwrap() {
            Packet::Interrupt(i) => {
                assert_eq!(
                    (i.dev, i.line, i.val, i.timestamp),
                    (DEV_BOARD_OUT, 0, 5, 1234)
                );
            }
            p => panic!("{p:?}"),
        }
        // Other telemetry is not forwarded.
        dpi.telemetry_event(br#"{"t":1,"k":"ps_wr","a":1,"d":2}"#, 0);
        assert!(host
            .recv_timeout(Duration::from_millis(50))
            .unwrap()
            .is_none());
    }

    #[test]
    fn pin_groups_are_reported_and_driven() {
        let (mut dpi, mut host) = pair();
        // Outputs: one posted packet per call, stamped with the given time.
        dpi.pin_out(2, 0xA5, 4321);
        dpi.pin_out(6, 0b10_0001, 5000);
        for (group, levels, t) in [(2u32, 0xA5u8, 4321u64), (6, 0b10_0001, 5000)] {
            match host.recv().unwrap() {
                Packet::Interrupt(i) => {
                    assert_eq!(
                        (i.dev, i.line, i.val, i.timestamp),
                        (DEV_PIN_OUT, group, levels, t)
                    );
                    assert_eq!(i.flags & flags::POSTED, flags::POSTED);
                }
                p => panic!("{p:?}"),
            }
        }
        // Inputs: the latest value per group, 0 for a group never driven.
        for (line, val) in [(1u32, 0x0Fu8), (1, 0xF0), (3, 0x11)] {
            host.send(&Packet::Interrupt(Interrupt {
                id: 0,
                flags: flags::POSTED,
                dev: DEV_PIN_IN,
                timestamp: 10,
                vector: 0,
                line,
                val,
            }))
            .unwrap();
        }
        let mut got = (0, 0, 0);
        for _ in 0..200 {
            got = (dpi.pin_in(1), dpi.pin_in(3), dpi.pin_in(0));
            if got == (0xF0, 0x11, 0) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, (0xF0, 0x11, 0));
        assert_eq!(dpi.pin_in(99), 0, "unknown groups read as zero");
    }

    #[test]
    fn pin_exchange_blocks_for_the_host_answer() {
        let (mut dpi, mut host) = pair();
        let t = std::thread::spawn(move || {
            serve_read(&mut host, DEV_PIN_OUT, |addr, len, _| {
                assert_eq!(len, 1);
                // group 3, levels 0b1010_0110: answer with the complement.
                assert_eq!(addr, 3 | 0b1010_0110 << 8);
                vec![!0b1010_0110]
            });
            // The second exchange is answered with the wrong length.
            serve_read(&mut host, DEV_PIN_OUT, |_, _, _| vec![]);
            host
        });
        assert_eq!(dpi.pin_xchg(3, 0b1010_0110, 55), 0b0101_1001);
        assert_eq!(
            dpi.pin_xchg(0, 0, 56),
            0xFF,
            "a bad answer reads as released"
        );
        let _ = t.join().unwrap();
    }

    #[test]
    fn pin_exchanges_stay_in_order_in_bulk() {
        let (mut dpi, mut host) = pair();
        const N: u32 = 2000;
        let t = std::thread::spawn(move || {
            for _ in 0..N {
                serve_read(&mut host, DEV_PIN_OUT, |addr, _, _| {
                    // Answer with the levels sent, inverted.
                    vec![!((addr >> 8) as u8)]
                });
            }
            host
        });
        for i in 0..N {
            let lv = i % 251;
            assert_eq!(dpi.pin_xchg(1, lv, u64::from(i)), !(lv as u8));
        }
        let _ = t.join().unwrap();
    }

    /// Answers one host-side read with `data` produced by `f(addr, len)`.
    fn serve_read(host: &mut Session, dev: u32, f: impl Fn(u64, u32, u32) -> Vec<u8>) {
        match host.recv().unwrap() {
            Packet::Bus(r) if r.cmd == BusCmd::Read => {
                assert_eq!(r.dev, dev);
                let d = f(r.addr, r.len, r.width);
                host.send(&Packet::Bus(r.response(resp::OK, d, r.timestamp)))
                    .unwrap();
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn video_out_sends_lines_and_frame_ends() {
        let (mut dpi, mut host) = pair();
        // Two frames of 3 lines x 2 pixels; vs pulses between them.
        let mut t = 0u64;
        for frame in 0..2u32 {
            dpi.video_tx_pixel(false, true, 0, t); // vs rises: end of previous frame
            t += 10;
            dpi.video_tx_pixel(false, false, 0, t);
            for y in 0..3u32 {
                for x in 0..2u32 {
                    t += 10;
                    let rgb = (frame << 16) | (y << 8) | x;
                    dpi.video_tx_pixel(true, false, rgb, t);
                }
                t += 10;
                dpi.video_tx_pixel(false, false, 0, t); // de falls: line done
            }
        }
        dpi.video_tx_pixel(false, true, 0, t + 10); // closes frame 1
        let mut lines = Vec::new();
        let mut ends = Vec::new();
        for _ in 0..8 {
            match host.recv().unwrap() {
                Packet::Bus(b) => {
                    assert_eq!((b.dev, b.cmd), (DEV_VIDEO_OUT, BusCmd::Write));
                    lines.push((b.addr >> 16, b.addr & 0xFFFF, b.data));
                }
                Packet::Interrupt(i) => {
                    assert_eq!(i.dev, DEV_VIDEO_OUT);
                    ends.push(i.vector);
                }
                p => panic!("{p:?}"),
            }
        }
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], (0, 0, vec![0, 0, 0, 0, 0, 1]));
        assert_eq!(lines[5], (1, 2, vec![1, 2, 0, 1, 2, 1]));
        assert_eq!(ends, vec![0, 1], "one end marker per completed frame");
    }

    #[test]
    fn video_in_fetches_a_line_and_serves_pixels() {
        let (mut dpi, mut host) = pair();
        let t = std::thread::spawn(move || {
            serve_read(&mut host, DEV_VIDEO_IN, |addr, len, h| {
                assert_eq!((addr >> 16, addr & 0xFFFF, len, h), (3, 7, 12, 240));
                (0..len as u8).collect()
            });
            host
        });
        assert_eq!(dpi.video_rx_line(3, 7, 4, 240, 55), 0);
        assert_eq!(dpi.video_rx_pixel(0), 0x00_01_02);
        assert_eq!(dpi.video_rx_pixel(3), 0x09_0A_0B);
        assert_eq!(dpi.video_rx_pixel(99), 0, "out of range reads as black");
        let _ = t.join().unwrap();
    }

    #[test]
    fn audio_out_is_batched_and_flushable() {
        let (mut dpi, mut host) = pair();
        for i in 0..70i16 {
            dpi.audio_tx_frame(i, -i, 1000 + i as u64);
        }
        match host.recv().unwrap() {
            Packet::Bus(b) => {
                assert_eq!((b.dev, b.addr, b.len), (DEV_AUDIO_OUT, 0, 64 * 4));
                assert_eq!(b.timestamp, 1000);
                assert_eq!(&b.data[4..8], &[1, 0, 0xFF, 0xFF], "frame 1 = (1, -1)");
            }
            p => panic!("{p:?}"),
        }
        dpi.audio_flush(); // the 6 remaining frames
        match host.recv().unwrap() {
            Packet::Bus(b) => assert_eq!((b.addr, b.len), (64, 6 * 4)),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn audio_in_fetches_blocks_on_demand() {
        let (mut dpi, mut host) = pair();
        let t = std::thread::spawn(move || {
            for expect_block in [0u64, 64] {
                serve_read(&mut host, DEV_AUDIO_IN, move |addr, len, _| {
                    assert_eq!((addr, len), (expect_block, 64 * 4));
                    (0..64u64)
                        .flat_map(|i| {
                            let f = (addr + i) as i16;
                            [f.to_le_bytes(), (-f).to_le_bytes()]
                        })
                        .flatten()
                        .collect()
                });
            }
            host
        });
        for i in 0..65i16 {
            assert_eq!(dpi.audio_rx_frame(0), (i, -i));
        }
        let _ = t.join().unwrap();
    }

    #[test]
    fn codec_register_writes_are_reported() {
        let (mut dpi, mut host) = pair();
        dpi.codec_register(4, 0x1FF, 77);
        match host.recv().unwrap() {
            Packet::Interrupt(i) => {
                assert_eq!(
                    (i.dev, i.line, i.vector, i.timestamp),
                    (DEV_CODEC_REG, 4, 0x1FF, 77)
                );
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn json_helpers() {
        assert!(json_has(b"{\"k\":\"led\"}", b"\"k\":\"led\""));
        assert_eq!(json_uint(b"{\"t\":42,\"v\":7}", b"\"v\":"), Some(7));
        assert_eq!(json_uint(b"{\"v\":}", b"\"v\":"), None);
        assert_eq!(json_uint(b"nothing", b"\"v\":"), None);
    }

    #[test]
    fn endpoint_parsing() {
        assert_eq!(
            parse_endpoint(b"tcp:127.0.0.1:9750"),
            Some(([127, 0, 0, 1], 9750))
        );
        assert_eq!(parse_endpoint(b"10.0.0.7:1"), Some(([10, 0, 0, 7], 1)));
        for bad in [
            &b"tcp:localhost:1"[..],
            b"tcp:1.2.3:4",
            b"tcp:1.2.3.4.5:4",
            b"tcp:1.2.3.4:",
            b"tcp:1.2.3.4:99999",
            b"tcp:256.0.0.1:1",
            b"",
        ] {
            assert!(parse_endpoint(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn peer_close_is_reported() {
        let (mut dpi, host) = pair();
        drop(host);
        assert_eq!(dpi.next(0, 100), Next::Closed);
        assert_eq!(dpi.poll(100), -1);
    }
}
