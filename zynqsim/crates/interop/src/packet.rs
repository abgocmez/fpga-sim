use alloc::vec::Vec;
use core::fmt;

/// Size of the common packet header on the wire.
pub const HEADER_LEN: usize = 20;
/// Size of the old busaccess layout (header included).
const BUS_V40_LEN: usize = 58;
/// Size of the extended busaccess layout (header included).
const BUS_EXT_LEN: usize = 80;
/// Size of a hello packet before the capability array.
const HELLO_LEN: usize = 32;
const INTERRUPT_LEN: usize = 41;
const SYNC_LEN: usize = 28;

/// Packet header flag bits.
pub mod flags {
    /// The receiver may ignore the packet if it does not understand it.
    pub const OPTIONAL: u32 = 1 << 0;
    /// The packet answers an earlier request with the same id.
    pub const RESPONSE: u32 = 1 << 1;
    /// The sender does not need a response.
    pub const POSTED: u32 = 1 << 2;
}

/// Capability numbers exchanged in hello packets.
pub mod cap {
    /// Extended busaccess header layout.
    pub const BUSACCESS_EXT_BASE: u32 = 1;
    /// Byte enables in busaccess packets.
    pub const BUSACCESS_EXT_BYTE_EN: u32 = 2;
    /// Wire updates honour the posted flag and are otherwise acknowledged.
    pub const WIRE_POSTED_UPDATES: u32 = 3;
    /// Address translation services.
    pub const ATS: u32 = 4;
}

/// Busaccess attribute bits.
pub mod attr {
    /// End of packet.
    pub const EOP: u64 = 1 << 0;
    /// Secure access.
    pub const SECURE: u64 = 1 << 1;
    /// The extended header layout is in use.
    pub const EXT_BASE: u64 = 1 << 2;
    /// Physical address.
    pub const PHYS_ADDR: u64 = 1 << 3;
    /// I/O address space.
    pub const IO_ACCESS: u64 = 1 << 4;
    /// Shift of the 4-bit response code inside the attributes.
    pub const RESP_SHIFT: u32 = 8;
    /// Mask of the response code.
    pub const RESP_MASK: u64 = 0xF << RESP_SHIFT;
}

/// Response codes carried in busaccess attributes.
pub mod resp {
    /// Success.
    pub const OK: u8 = 0;
    /// Generic bus error.
    pub const BUS_GENERIC_ERROR: u8 = 1;
    /// Address error (nothing decodes the address).
    pub const ADDR_ERROR: u8 = 2;
}

/// Packet command numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// No operation.
    Nop,
    /// Session setup.
    Hello,
    /// Configuration.
    Cfg,
    /// Bus read.
    Read,
    /// Bus write.
    Write,
    /// Interrupt / wire update.
    Interrupt,
    /// Time synchronisation.
    Sync,
    /// ATS request.
    AtsReq,
    /// ATS invalidation.
    AtsInv,
}

impl Cmd {
    fn from_u32(v: u32) -> Option<Cmd> {
        Some(match v {
            0 => Cmd::Nop,
            1 => Cmd::Hello,
            2 => Cmd::Cfg,
            3 => Cmd::Read,
            4 => Cmd::Write,
            5 => Cmd::Interrupt,
            6 => Cmd::Sync,
            7 => Cmd::AtsReq,
            8 => Cmd::AtsInv,
            _ => return None,
        })
    }
    fn to_u32(self) -> u32 {
        match self {
            Cmd::Nop => 0,
            Cmd::Hello => 1,
            Cmd::Cfg => 2,
            Cmd::Read => 3,
            Cmd::Write => 4,
            Cmd::Interrupt => 5,
            Cmd::Sync => 6,
            Cmd::AtsReq => 7,
            Cmd::AtsInv => 8,
        }
    }
}

/// The 20-byte common header (host byte order here, big-endian on the wire).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    /// Command number.
    pub cmd: u32,
    /// Payload length in bytes, not counting the header.
    pub len: u32,
    /// Request id, echoed in the response.
    pub id: u32,
    /// Flag bits, see [`flags`].
    pub flags: u32,
    /// Device number on the remote-port link.
    pub dev: u32,
}

impl Header {
    /// Parses the first [`HEADER_LEN`] bytes.
    pub fn parse(b: &[u8; HEADER_LEN]) -> Header {
        let u = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Header {
            cmd: u(0),
            len: u(4),
            id: u(8),
            flags: u(12),
            dev: u(16),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        for v in [self.cmd, self.len, self.id, self.flags, self.dev] {
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
}

/// Direction of a bus access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusCmd {
    /// Read.
    Read,
    /// Write.
    Write,
}

/// A read or write, request or response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusAccess {
    /// Read or write.
    pub cmd: BusCmd,
    /// Request id.
    pub id: u32,
    /// Header flag bits.
    pub flags: u32,
    /// Device number.
    pub dev: u32,
    /// Timestamp in nanoseconds of the sender's virtual time.
    pub timestamp: u64,
    /// Attribute bits (the response code lives in bits 11:8).
    pub attributes: u64,
    /// Address.
    pub addr: u64,
    /// Access length in bytes.
    pub len: u32,
    /// Beat width in bytes, 0 for unspecified.
    pub width: u32,
    /// Stream width in bytes; equals `len` for incrementing accesses.
    pub stream_width: u32,
    /// Master id (up to 64 bits).
    pub master_id: u64,
    /// Payload: write request data or read response data.
    pub data: Vec<u8>,
    /// Byte enables, empty when all bytes are enabled.
    pub byte_enable: Vec<u8>,
    /// Encode with the extended layout. Set on decode when the packet used it.
    pub ext: bool,
}

impl BusAccess {
    /// True for responses.
    pub fn is_response(&self) -> bool {
        self.flags & flags::RESPONSE != 0
    }
    /// True when no response is required.
    pub fn is_posted(&self) -> bool {
        self.flags & flags::POSTED != 0
    }
    /// The 4-bit response code.
    pub fn response_code(&self) -> u8 {
        ((self.attributes & attr::RESP_MASK) >> attr::RESP_SHIFT) as u8
    }
    /// Builds the response to this request: same id/dev/flags plus RESPONSE,
    /// the given code in the attributes, `data` for reads, and `timestamp`.
    pub fn response(&self, code: u8, data: Vec<u8>, timestamp: u64) -> BusAccess {
        BusAccess {
            cmd: self.cmd,
            id: self.id,
            flags: self.flags | flags::RESPONSE,
            dev: self.dev,
            timestamp,
            attributes: (self.attributes & !attr::RESP_MASK)
                | (u64::from(code) << attr::RESP_SHIFT),
            addr: self.addr,
            len: self.len,
            width: self.width,
            stream_width: self.stream_width,
            master_id: self.master_id,
            data: if self.cmd == BusCmd::Read {
                data
            } else {
                Vec::new()
            },
            byte_enable: Vec::new(),
            ext: self.ext,
        }
    }
}

/// An interrupt / wire update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interrupt {
    /// Request id.
    pub id: u32,
    /// Header flag bits.
    pub flags: u32,
    /// Device number.
    pub dev: u32,
    /// Timestamp in nanoseconds.
    pub timestamp: u64,
    /// Vector.
    pub vector: u64,
    /// Line number within the device.
    pub line: u32,
    /// New value.
    pub val: u8,
}

/// A time synchronisation packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sync {
    /// Request id.
    pub id: u32,
    /// Header flag bits.
    pub flags: u32,
    /// Device number.
    pub dev: u32,
    /// Timestamp in nanoseconds.
    pub timestamp: u64,
}

/// A decoded packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    /// Session setup with the sender's version and capabilities.
    Hello {
        /// Request id.
        id: u32,
        /// Device number.
        dev: u32,
        /// Protocol major version.
        major: u16,
        /// Protocol minor version.
        minor: u16,
        /// Capability numbers.
        caps: Vec<u32>,
    },
    /// Bus read or write.
    Bus(BusAccess),
    /// Interrupt or wire update.
    Interrupt(Interrupt),
    /// Time synchronisation.
    Sync(Sync),
    /// Any other command (cfg, ats, nop): header and raw payload.
    Other(Header, Vec<u8>),
}

/// Why a packet could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The payload is shorter than the command's fixed layout.
    Truncated {
        /// Command number.
        cmd: u32,
        /// Bytes needed (header excluded).
        need: usize,
        /// Bytes available.
        have: usize,
    },
    /// An offset inside the packet points outside it.
    BadOffset {
        /// What the offset was for.
        what: &'static str,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated { cmd, need, have } => {
                write!(
                    f,
                    "packet cmd {cmd} truncated: need {need} payload bytes, have {have}"
                )
            }
            DecodeError::BadOffset { what } => write!(f, "{what} offset points outside the packet"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}

fn be16(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}
fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn be64(b: &[u8], i: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[i..i + 8]);
    u64::from_be_bytes(a)
}

impl Packet {
    /// Decodes a packet from its header and the `hdr.len` payload bytes that
    /// followed it.
    pub fn decode(hdr: Header, payload: &[u8]) -> Result<Packet, DecodeError> {
        // `full` is the packet from the start of the header, so offsets stored
        // inside it (relative to the packet start) can be used directly.
        let mut full = Vec::with_capacity(HEADER_LEN + payload.len());
        hdr.write(&mut full);
        full.extend_from_slice(payload);
        let need = |n: usize| -> Result<(), DecodeError> {
            if payload.len() < n - HEADER_LEN {
                Err(DecodeError::Truncated {
                    cmd: hdr.cmd,
                    need: n - HEADER_LEN,
                    have: payload.len(),
                })
            } else {
                Ok(())
            }
        };

        match Cmd::from_u32(hdr.cmd) {
            Some(Cmd::Hello) => {
                need(HELLO_LEN - 8)?; // version only is mandatory
                let major = be16(&full, 20);
                let minor = be16(&full, 22);
                let mut caps = Vec::new();
                if full.len() >= HELLO_LEN {
                    let off = be32(&full, 24) as usize;
                    let n = usize::from(be16(&full, 28));
                    if n > 0 {
                        let end = off.checked_add(n * 4).filter(|e| *e <= full.len());
                        let end = end.ok_or(DecodeError::BadOffset {
                            what: "capabilities",
                        })?;
                        caps = full[off..end]
                            .chunks_exact(4)
                            .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                            .collect();
                    }
                }
                Ok(Packet::Hello {
                    id: hdr.id,
                    dev: hdr.dev,
                    major,
                    minor,
                    caps,
                })
            }
            Some(c @ (Cmd::Read | Cmd::Write)) => {
                need(BUS_V40_LEN)?;
                let attributes = be64(&full, 28);
                let mut master_id = u64::from(be16(&full, 56));
                let len = be32(&full, 44);
                let ext = attributes & attr::EXT_BASE != 0;
                let is_resp = hdr.flags & flags::RESPONSE != 0;
                let is_write = c == Cmd::Write;
                let (mut data, mut byte_enable) = (Vec::new(), Vec::new());
                let mut data_off = BUS_V40_LEN;
                if ext {
                    need(BUS_EXT_LEN)?;
                    master_id |= u64::from(be16(&full, 58)) << 16;
                    master_id |= u64::from(be32(&full, 60)) << 32;
                    data_off = be32(&full, 64) as usize;
                    let be_off = be32(&full, 72) as usize;
                    let be_len = be32(&full, 76) as usize;
                    if be_len > 0 {
                        let end = be_off.checked_add(be_len).filter(|e| *e <= full.len());
                        let end = end.ok_or(DecodeError::BadOffset {
                            what: "byte enable",
                        })?;
                        byte_enable = full[be_off..end].to_vec();
                    }
                }
                // Data follows a write request and a read response.
                let carries = (is_write && !is_resp) || (!is_write && is_resp);
                if carries && len > 0 {
                    let end = data_off
                        .checked_add(len as usize)
                        .filter(|e| *e <= full.len());
                    let end = end.ok_or(DecodeError::BadOffset { what: "data" })?;
                    data = full[data_off..end].to_vec();
                }
                Ok(Packet::Bus(BusAccess {
                    cmd: if is_write {
                        BusCmd::Write
                    } else {
                        BusCmd::Read
                    },
                    id: hdr.id,
                    flags: hdr.flags,
                    dev: hdr.dev,
                    timestamp: be64(&full, 20),
                    attributes,
                    addr: be64(&full, 36),
                    len,
                    width: be32(&full, 48),
                    stream_width: be32(&full, 52),
                    master_id,
                    data,
                    byte_enable,
                    ext,
                }))
            }
            Some(Cmd::Interrupt) => {
                need(INTERRUPT_LEN)?;
                Ok(Packet::Interrupt(Interrupt {
                    id: hdr.id,
                    flags: hdr.flags,
                    dev: hdr.dev,
                    timestamp: be64(&full, 20),
                    vector: be64(&full, 28),
                    line: be32(&full, 36),
                    val: full[40],
                }))
            }
            Some(Cmd::Sync) => {
                need(SYNC_LEN)?;
                Ok(Packet::Sync(Sync {
                    id: hdr.id,
                    flags: hdr.flags,
                    dev: hdr.dev,
                    timestamp: be64(&full, 20),
                }))
            }
            _ => Ok(Packet::Other(hdr, payload.to_vec())),
        }
    }

    /// Encodes the packet, including any data, byte enables and capabilities.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Packet::Hello {
                id,
                dev,
                major,
                minor,
                caps,
            } => {
                let len = HELLO_LEN - HEADER_LEN + caps.len() * 4;
                Header {
                    cmd: Cmd::Hello.to_u32(),
                    len: len as u32,
                    id: *id,
                    flags: 0,
                    dev: *dev,
                }
                .write(&mut out);
                out.extend_from_slice(&major.to_be_bytes());
                out.extend_from_slice(&minor.to_be_bytes());
                out.extend_from_slice(&(HELLO_LEN as u32).to_be_bytes()); // caps offset
                out.extend_from_slice(&(caps.len() as u16).to_be_bytes());
                out.extend_from_slice(&0u16.to_be_bytes()); // reserved
                for c in caps {
                    out.extend_from_slice(&c.to_be_bytes());
                }
            }
            Packet::Bus(b) => encode_bus(b, &mut out),
            Packet::Interrupt(i) => {
                Header {
                    cmd: Cmd::Interrupt.to_u32(),
                    len: (INTERRUPT_LEN - HEADER_LEN) as u32,
                    id: i.id,
                    flags: i.flags,
                    dev: i.dev,
                }
                .write(&mut out);
                out.extend_from_slice(&i.timestamp.to_be_bytes());
                out.extend_from_slice(&i.vector.to_be_bytes());
                out.extend_from_slice(&i.line.to_be_bytes());
                out.push(i.val);
            }
            Packet::Sync(s) => {
                Header {
                    cmd: Cmd::Sync.to_u32(),
                    len: (SYNC_LEN - HEADER_LEN) as u32,
                    id: s.id,
                    flags: s.flags,
                    dev: s.dev,
                }
                .write(&mut out);
                out.extend_from_slice(&s.timestamp.to_be_bytes());
            }
            Packet::Other(h, payload) => {
                let mut h = *h;
                h.len = payload.len() as u32;
                h.write(&mut out);
                out.extend_from_slice(payload);
            }
        }
        out
    }
}

fn encode_bus(b: &BusAccess, out: &mut Vec<u8>) {
    let carries =
        (b.cmd == BusCmd::Write && !b.is_response()) || (b.cmd == BusCmd::Read && b.is_response());
    let data: &[u8] = if carries { &b.data } else { &[] };
    let cmd = match b.cmd {
        BusCmd::Read => Cmd::Read,
        BusCmd::Write => Cmd::Write,
    };

    let fixed = if b.ext { BUS_EXT_LEN } else { BUS_V40_LEN };
    // Data first, then byte enables, mirroring the C encoder.
    let hsize = data.len() + if b.ext { b.byte_enable.len() } else { 0 };
    Header {
        cmd: cmd.to_u32(),
        len: (fixed - HEADER_LEN + hsize) as u32,
        id: b.id,
        flags: b.flags,
        dev: b.dev,
    }
    .write(out);
    let attributes = if b.ext {
        b.attributes | attr::EXT_BASE
    } else {
        b.attributes & !attr::EXT_BASE
    };
    out.extend_from_slice(&b.timestamp.to_be_bytes());
    out.extend_from_slice(&attributes.to_be_bytes());
    out.extend_from_slice(&b.addr.to_be_bytes());
    out.extend_from_slice(&b.len.to_be_bytes());
    out.extend_from_slice(&b.width.to_be_bytes());
    out.extend_from_slice(&b.stream_width.to_be_bytes());
    out.extend_from_slice(&((b.master_id & 0xFFFF) as u16).to_be_bytes());
    if b.ext {
        out.extend_from_slice(&(((b.master_id >> 16) & 0xFFFF) as u16).to_be_bytes());
        out.extend_from_slice(&((b.master_id >> 32) as u32).to_be_bytes());
        out.extend_from_slice(&(BUS_EXT_LEN as u32).to_be_bytes()); // data_offset
        out.extend_from_slice(&0u32.to_be_bytes()); // next_offset
        out.extend_from_slice(&((BUS_EXT_LEN + data.len()) as u32).to_be_bytes());
        out.extend_from_slice(&(b.byte_enable.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(data);
    if b.ext {
        out.extend_from_slice(&b.byte_enable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn decode_all(bytes: &[u8]) -> Packet {
        let mut h = [0u8; HEADER_LEN];
        h.copy_from_slice(&bytes[..HEADER_LEN]);
        let hdr = Header::parse(&h);
        assert_eq!(hdr.len as usize, bytes.len() - HEADER_LEN, "len field");
        Packet::decode(hdr, &bytes[HEADER_LEN..]).unwrap()
    }

    // The expected strings below were produced by the C reference encoder
    // (libsystemctlm-soc `remote-port-proto.c`) for the same inputs.

    #[test]
    fn hello_matches_c_reference() {
        let p = Packet::Hello {
            id: 1,
            dev: 0,
            major: 4,
            minor: 3,
            caps: vec![cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES],
        };
        let b = p.encode();
        assert_eq!(
            hex(&b),
            "00000001000000140000000100000000000000000004000300000020000200000000000100000003"
        );
        assert_eq!(decode_all(&b), p);
    }

    #[test]
    fn write_request_ext_matches_c_reference() {
        let p = Packet::Bus(BusAccess {
            cmd: BusCmd::Write,
            id: 7,
            flags: 0,
            dev: 7,
            timestamp: 0x1122_3344_5566_7788,
            attributes: 0,
            addr: 0x43C0_0004,
            len: 4,
            width: 0,
            stream_width: 4,
            master_id: 0x0000_0001_0002_0003,
            data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            byte_enable: vec![],
            ext: true,
        });
        let b = p.encode();
        let c_header = "0000000400000040000000070000000000000007112233445566778800000000000000040000000043c00004000000040000000000000004000300020000000100000050000000000000005400000000";
        assert_eq!(hex(&b[..80]), c_header);
        assert_eq!(&b[80..], &[0xDE, 0xAD, 0xBE, 0xEF]);
        let mut want = p.clone();
        if let Packet::Bus(w) = &mut want {
            w.attributes |= attr::EXT_BASE;
        }
        assert_eq!(decode_all(&b), want);
    }

    #[test]
    fn old_layout_read_matches_c_reference() {
        let req = BusAccess {
            cmd: BusCmd::Read,
            id: 3,
            flags: 0,
            dev: 2,
            timestamp: 5,
            attributes: 0,
            addr: 0x1000,
            len: 8,
            width: 0,
            stream_width: 8,
            master_id: 9,
            data: vec![],
            byte_enable: vec![],
            ext: false,
        };
        let b = Packet::Bus(req.clone()).encode();
        assert_eq!(
            hex(&b),
            "00000003000000260000000300000000000000020000000000000005000000000000000000000000000010000000000800000000000000080009"
        );
        assert_eq!(decode_all(&b), Packet::Bus(req));
    }

    #[test]
    fn read_response_ext_matches_c_reference() {
        let req = BusAccess {
            cmd: BusCmd::Read,
            id: 3,
            flags: 0,
            dev: 2,
            timestamp: 5,
            attributes: attr::EXT_BASE,
            addr: 0x1000,
            len: 8,
            width: 0,
            stream_width: 8,
            master_id: 9,
            data: vec![],
            byte_enable: vec![],
            ext: true,
        };
        let r = req.response(resp::BUS_GENERIC_ERROR, vec![1, 2, 3, 4, 5, 6, 7, 8], 6);
        let b = Packet::Bus(r.clone()).encode();
        assert_eq!(b.len(), 80 + 8);
        assert_eq!(
            hex(&b[..80]),
            "0000000300000044000000030000000200000002000000000000000600000000000001040000000000001000000000080000000000000008000900000000000000000050000000000000005800000000"
        );
        assert_eq!(&b[80..], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(decode_all(&b), Packet::Bus(r));
    }

    #[test]
    fn interrupt_and_sync_match_c_reference() {
        let i = Packet::Interrupt(Interrupt {
            id: 9,
            flags: flags::POSTED,
            dev: 9,
            timestamp: 1000,
            vector: 0,
            line: 3,
            val: 1,
        });
        let b = i.encode();
        assert_eq!(
            hex(&b),
            "000000050000001500000009000000040000000900000000000003e800000000000000000000000301"
        );
        assert_eq!(decode_all(&b), i);

        let s = Packet::Sync(Sync {
            id: 4,
            flags: flags::RESPONSE,
            dev: 5,
            timestamp: 123_456,
        });
        let b = s.encode();
        assert_eq!(
            hex(&b),
            "0000000600000008000000040000000200000005000000000001e240"
        );
        assert_eq!(decode_all(&b), s);
        assert_eq!(unhex(&hex(&b)), b);
    }

    #[test]
    fn response_helpers() {
        let req = BusAccess {
            cmd: BusCmd::Write,
            id: 1,
            flags: flags::POSTED,
            dev: 7,
            timestamp: 1,
            attributes: attr::EXT_BASE,
            addr: 0,
            len: 4,
            width: 0,
            stream_width: 4,
            master_id: 0,
            data: vec![0; 4],
            byte_enable: vec![],
            ext: true,
        };
        assert!(req.is_posted() && !req.is_response());
        let r = req.response(resp::ADDR_ERROR, vec![], 9);
        assert!(r.is_response());
        assert_eq!(r.response_code(), resp::ADDR_ERROR);
        assert!(r.data.is_empty(), "write responses carry no data");
        assert_eq!(r.timestamp, 9);
    }

    #[test]
    fn truncated_and_bad_offsets_are_errors_not_panics() {
        let hdr = Header {
            cmd: 4,
            len: 10,
            id: 0,
            flags: 0,
            dev: 0,
        };
        assert!(matches!(
            Packet::decode(hdr, &[0; 10]),
            Err(DecodeError::Truncated { .. })
        ));

        // A write whose data offset points past the end.
        let mut b = Packet::Bus(BusAccess {
            cmd: BusCmd::Write,
            id: 0,
            flags: 0,
            dev: 0,
            timestamp: 0,
            attributes: 0,
            addr: 0,
            len: 4,
            width: 0,
            stream_width: 4,
            master_id: 0,
            data: vec![0; 4],
            byte_enable: vec![],
            ext: true,
        })
        .encode();
        b[64..68].copy_from_slice(&1000u32.to_be_bytes());
        let mut h = [0u8; HEADER_LEN];
        h.copy_from_slice(&b[..HEADER_LEN]);
        assert!(matches!(
            Packet::decode(Header::parse(&h), &b[HEADER_LEN..]),
            Err(DecodeError::BadOffset { .. })
        ));
    }

    #[test]
    fn unknown_commands_pass_through() {
        let h = Header {
            cmd: 99,
            len: 3,
            id: 1,
            flags: 0,
            dev: 0,
        };
        match Packet::decode(h, &[1, 2, 3]).unwrap() {
            Packet::Other(hh, p) => {
                assert_eq!(hh.cmd, 99);
                assert_eq!(p, vec![1, 2, 3]);
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn random_bytes_never_panic() {
        // xorshift, no dependency
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..5000 {
            let n = (next() % 120) as usize;
            let payload: Vec<u8> = (0..n).map(|_| next() as u8).collect();
            let hdr = Header {
                cmd: (next() % 12) as u32,
                len: n as u32,
                id: 0,
                flags: (next() % 8) as u32,
                dev: 0,
            };
            let _ = Packet::decode(hdr, &payload);
        }
    }
}
