#![no_main]
// Entry point: a remote-port packet (20-byte header, then the payload) ->
// Packet::decode -> encode. Same properties as crates/interop/tests/fuzz_packets.rs.

use interop::{HEADER_LEN, Header, Packet};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < HEADER_LEN {
        return;
    }
    let mut h = [0u8; HEADER_LEN];
    h.copy_from_slice(&data[..HEADER_LEN]);
    if let Ok(packet) = Packet::decode(Header::parse(&h), &data[HEADER_LEN..]) {
        let _ = packet.encode();
    }
});
