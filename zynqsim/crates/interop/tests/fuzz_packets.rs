//! Seeded fuzzing of the remote-port packet decoder and the TCP session's
//! length check (spec §14.10: "fuzzing ... remote-port packets").
//!
//! Half of the inputs are random bytes, half are mutations of valid hello,
//! read, write, interrupt, sync and nop packets. A fixed-seed xorshift PRNG
//! (no dependency, as in `crates/boot`) makes every failure reproducible.
//! The properties: decoding never panics, decoded vectors are never longer
//! than the bytes they came from (so memory is bounded by the input), and the
//! session refuses a declared length above its cap before allocating for it.

use interop::{
    BusAccess, BusCmd, HEADER_LEN, Header, Interrupt, Packet, Session, SessionError, Sync, attr,
    cap, flags,
};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::panic::{self, AssertUnwindSafe};

/// Inputs per run.
const CASES: usize = 20_000;
/// The session's payload cap (`MAX_PAYLOAD` in `session.rs`).
const SESSION_CAP: u32 = 16 * 1024 * 1024;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Valid packets of every command the decoder knows, in both bus layouts.
fn valid_packets() -> Vec<Vec<u8>> {
    let bus = |cmd, ext, data: Vec<u8>, byte_enable: Vec<u8>| BusAccess {
        cmd,
        id: 9,
        flags: 0,
        dev: 1,
        timestamp: 123_456,
        attributes: attr::EXT_BASE | (3 << 4),
        addr: 0x4000_0000,
        len: data.len() as u32,
        width: 4,
        stream_width: 4,
        master_id: 0x1_0000_0002,
        data,
        byte_enable,
        ext,
    };
    let write_ext = bus(
        BusCmd::Write,
        true,
        vec![1, 2, 3, 4, 5, 6, 7, 8],
        vec![0xFF; 8],
    );
    let write_v40 = bus(BusCmd::Write, false, vec![9, 9, 9, 9], vec![]);
    let read_req = bus(BusCmd::Read, true, vec![], vec![]);
    let read_resp = read_req.response(0, vec![0xAA; 4], 777);
    let packets = vec![
        Packet::Hello {
            id: 1,
            dev: 0,
            major: 4,
            minor: 3,
            caps: vec![cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES, cap::ATS],
        },
        Packet::Hello {
            id: 2,
            dev: 0,
            major: 4,
            minor: 3,
            caps: vec![],
        },
        Packet::Bus(write_ext),
        Packet::Bus(write_v40),
        Packet::Bus(read_req),
        Packet::Bus(read_resp),
        Packet::Interrupt(Interrupt {
            id: 5,
            flags: flags::POSTED,
            dev: 0,
            timestamp: 42,
            vector: 0xDEAD_BEEF,
            line: 3,
            val: 1,
        }),
        Packet::Sync(Sync {
            id: 6,
            flags: 0,
            dev: 0,
            timestamp: 99,
        }),
        Packet::Other(
            Header {
                cmd: 0,
                len: 0,
                id: 7,
                flags: 0,
                dev: 0,
            },
            vec![],
        ),
    ];
    packets.iter().map(Packet::encode).collect()
}

/// Values that sit at offsets, counts and lengths in the packet layouts.
const HOT: &[u32] = &[
    0,
    1,
    3,
    4,
    8,
    20,
    21,
    24,
    32,
    41,
    58,
    60,
    64,
    80,
    0x7FFF_FFFF,
    0x8000_0000,
    u32::MAX,
    u32::MAX - 3,
];

fn mutate(rng: &mut Rng, base: &[u8]) -> Vec<u8> {
    let mut b = base.to_vec();
    for _ in 0..1 + rng.below(4) {
        match rng.below(6) {
            // Byte flip.
            0 if !b.is_empty() => {
                let i = rng.below(b.len());
                b[i] ^= 1 << rng.below(8);
            }
            // A 32-bit word at a random 4-byte-aligned offset gets a hot value.
            1 | 2 if b.len() >= 4 => {
                let i = rng.below(b.len() - 3);
                let v = if rng.below(2) == 0 {
                    HOT[rng.below(HOT.len())]
                } else {
                    rng.next() as u32
                };
                b[i..i + 4].copy_from_slice(&v.to_be_bytes());
            }
            // Truncation.
            3 => {
                let n = rng.below(b.len() + 1);
                b.truncate(n);
            }
            // Garbage appended.
            4 => {
                for _ in 0..rng.below(64) {
                    b.push(rng.next() as u8);
                }
            }
            // A random byte.
            _ if !b.is_empty() => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
            _ => {}
        }
    }
    b
}

fn random_buffer(rng: &mut Rng) -> Vec<u8> {
    let n = rng.below(200);
    let mut b: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
    // Often give the header a real command number so the layouts are exercised.
    if b.len() >= HEADER_LEN && rng.below(2) == 0 {
        b[..4].copy_from_slice(&(rng.below(10) as u32).to_be_bytes());
    }
    b
}

/// Decodes one buffer the way the session does (header, then the bytes
/// that follow it) and checks what can be checked on the result. Returns
/// `Ok(true)` for a decoded packet, `Ok(false)` for an error.
fn decode_one(bytes: &[u8]) -> bool {
    if bytes.len() < HEADER_LEN {
        return false;
    }
    let mut h = [0u8; HEADER_LEN];
    h.copy_from_slice(&bytes[..HEADER_LEN]);
    let hdr = Header::parse(&h);
    let payload = &bytes[HEADER_LEN..];
    match Packet::decode(hdr, payload) {
        Ok(p) => {
            // Decoded data is copied out of the input, so it cannot be longer.
            match &p {
                Packet::Bus(b) => {
                    assert!(b.data.len() <= payload.len(), "data longer than input");
                    assert!(
                        b.byte_enable.len() <= payload.len(),
                        "byte enables longer than input"
                    );
                }
                Packet::Hello { caps, .. } => {
                    assert!(caps.len() * 4 <= payload.len(), "caps longer than input");
                }
                Packet::Other(_, raw) => {
                    assert_eq!(raw.len(), payload.len(), "raw payload changed length");
                }
                Packet::Interrupt(_) | Packet::Sync(_) => {}
            }
            // A decoded packet must also encode again without panicking.
            let _ = p.encode();
            true
        }
        Err(_) => false,
    }
}

#[test]
fn random_and_mutated_packets_never_panic_or_over_allocate() {
    let valid = valid_packets();
    // The unmutated packets decode: the base the mutations start from.
    for v in &valid {
        assert!(decode_one(v), "a valid packet does not decode: {}", hex(v));
    }

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut decoded = 0usize;
    let mut rejected = 0usize;
    for case in 0..CASES {
        let input = if case % 2 == 0 {
            random_buffer(&mut rng)
        } else {
            let base = &valid[rng.below(valid.len())];
            mutate(&mut rng, base)
        };
        match panic::catch_unwind(AssertUnwindSafe(|| decode_one(&input))) {
            Ok(true) => decoded += 1,
            Ok(false) => rejected += 1,
            Err(_) => panic!("case {case} panicked; input (hex): {}", hex(&input)),
        }
    }
    // Both outcomes must occur or the generator is not exercising the decoder.
    assert!(
        decoded > 1000 && rejected > 1000,
        "decoded={decoded} rejected={rejected}"
    );
}

/// A header with the given declared length, as the session reads it.
fn header_bytes(cmd: u32, len: u32) -> [u8; HEADER_LEN] {
    let mut out = [0u8; HEADER_LEN];
    for (i, v) in [cmd, len, 0, 0, 0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Opens a connected pair of sockets; returns (client writer, session on the other end).
fn connected_session() -> (TcpStream, Session) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let client = TcpStream::connect(listener.local_addr().unwrap()).expect("connect");
    let (server, _) = listener.accept().expect("accept");
    (client, Session::from_stream(server).expect("session"))
}

#[test]
fn a_declared_length_above_the_cap_is_refused_before_allocating() {
    for len in [SESSION_CAP + 1, 0x7FFF_FFFF, u32::MAX] {
        let (mut client, mut session) = connected_session();
        client
            .write_all(&header_bytes(4, len))
            .expect("send header");
        match session.recv() {
            Err(SessionError::Protocol(msg)) => {
                assert!(msg.contains("exceeds"), "{msg}");
            }
            other => panic!("len {len:#x}: expected a protocol refusal, got {other:?}"),
        }
    }
}

#[test]
fn a_length_within_the_cap_waits_for_the_bytes_and_reports_a_closed_peer() {
    // Declares 100 bytes, sends 10, then hangs up: the session must report the
    // close and must not decode a partial packet.
    let (mut client, mut session) = connected_session();
    client
        .write_all(&header_bytes(4, 100))
        .expect("send header");
    client.write_all(&[0u8; 10]).expect("send part");
    drop(client);
    assert!(
        matches!(session.recv(), Err(SessionError::Closed)),
        "a truncated payload is not a closed peer"
    );
}

#[test]
fn a_garbage_stream_is_an_error_not_a_panic() {
    let mut rng = Rng(0xC0FF_EE00_1234_5678);
    for _ in 0..200 {
        let bytes: Vec<u8> = (0..HEADER_LEN + rng.below(300))
            .map(|_| rng.next() as u8)
            .collect();
        let (mut client, mut session) = connected_session();
        client.write_all(&bytes).expect("send");
        drop(client);
        // Read until the stream ends; each answer must be a packet or an error.
        for _ in 0..8 {
            match panic::catch_unwind(AssertUnwindSafe(|| session.recv())) {
                Ok(Ok(_)) => {}
                Ok(Err(_)) => break,
                Err(_) => panic!("session panicked on input (hex): {}", hex(&bytes)),
            }
        }
    }
}
