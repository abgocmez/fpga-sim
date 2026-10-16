//! The shared object, loaded as a simulator loads it, speaks remote-port.
//!
//! The DPI state is one per process, so this file holds one test. A fake
//! remote-port peer (the `interop` crate, a QEMU stand-in) runs on its own thread
//! and checks every packet the library sends. The main thread plays the testbench:
//! it calls the exports through `extern "C"` function pointers with the DPI-C
//! argument types (`int` = i32, `longint` = i64, `chandle`/`string` = `const char*`,
//! `output int` = pointer). Each step says which side sends.
//!
//! The library has not been loaded by Questa, VCS, Xcelium, XSim-on-Linux or
//! Verilator here (none is installed); this test is the only load so far.

mod common;

use std::ffi::{c_char, CString};
use std::net::TcpListener;
use std::time::Duration;

use interop::{
    cap, flags, resp, BusAccess, BusCmd, Interrupt, Packet, Session, Sync, VERSION_MAJOR,
    VERSION_MINOR,
};

// Remote-port device numbers the library uses (xsim-dpi::core).
const DEV_WIRES_IN: u32 = 9;
const DEV_BOARD_IN: u32 = 12;
const DEV_BOARD_OUT: u32 = 13;
const DEV_CODEC_REG: u32 = 18;
const DEV_PIN_OUT: u32 = 19;
const DEV_PIN_IN: u32 = 20;
/// The PS slave port the PL master writes through (`S_AXI_HP0`).
const DEV_HP0: u32 = 2;
/// Device number of `M_AXI_GP0`, the PS-to-PL bus the testbench serves.
const DEV_GP0: u32 = 7;

const WAIT: Duration = Duration::from_secs(10);

// The DPI-C signatures, as a simulator calls them.
type Open = unsafe extern "C" fn(*const c_char) -> i32;
type Next = unsafe extern "C" fn(i64, i32, *mut i32, *mut i32, *mut i32, *mut i32) -> i32;
type DoneT = unsafe extern "C" fn(i32, i32, i64);
type Poll = unsafe extern "C" fn(i32) -> i32;
type Lockstep = unsafe extern "C" fn(i32);
type SyncTarget = unsafe extern "C" fn() -> i64;
type SyncAck = unsafe extern "C" fn(i64);
type MemPut = unsafe extern "C" fn(i32, i32);
type MemStart = unsafe extern "C" fn(i64, i32, i32, i32, i32) -> i32;
type MemWait = unsafe extern "C" fn(i32) -> i32;
type MemGet = unsafe extern "C" fn(i32, i32) -> i32;
type MemFree = unsafe extern "C" fn(i32);
type Irq = unsafe extern "C" fn(i64, i32, i32);
type Close = unsafe extern "C" fn();
type TelEvent = unsafe extern "C" fn(*const c_char);
type TelCtrl = unsafe extern "C" fn(*mut i32, *mut i32) -> i32;
type CodecReg = unsafe extern "C" fn(i32, i32, i64);
type PinOut = unsafe extern "C" fn(i32, i32, i64);
type PinIn = unsafe extern "C" fn(i32) -> i32;
type PinXchg = unsafe extern "C" fn(i32, i32, i64) -> i32;

/// The library and the symbols a testbench uses, looked up by name.
struct Dpi {
    _lib: libloading::Library,
    open: Open,
    next: Next,
    done_t: DoneT,
    poll: Poll,
    lockstep: Lockstep,
    sync_target: SyncTarget,
    sync_ack: SyncAck,
    mem_put: MemPut,
    mem_start: MemStart,
    mem_wait: MemWait,
    mem_get: MemGet,
    mem_free: MemFree,
    irq: Irq,
    close: Close,
    tel_event: TelEvent,
    tel_ctrl: TelCtrl,
    codec_reg: CodecReg,
    pin_out: PinOut,
    pin_in: PinIn,
    pin_xchg: PinXchg,
}

impl Dpi {
    fn load() -> Dpi {
        let path = common::require_cdylib();
        // SAFETY: the library is the one this workspace builds; loading runs no DPI code.
        let lib = unsafe { libloading::Library::new(&path) }
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", path.display()));
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                let bytes = concat!($name, "\0").as_bytes();
                // SAFETY: the type is the DPI-C signature of the export.
                let s: libloading::Symbol<$ty> = unsafe { lib.get(bytes) }
                    .unwrap_or_else(|e| panic!("{} is missing: {e}", $name));
                *s
            }};
        }
        Dpi {
            open: sym!("rp_open", Open),
            next: sym!("rp_next", Next),
            done_t: sym!("rp_done_t", DoneT),
            poll: sym!("rp_poll", Poll),
            lockstep: sym!("rp_lockstep", Lockstep),
            sync_target: sym!("rp_sync_target", SyncTarget),
            sync_ack: sym!("rp_sync_ack", SyncAck),
            mem_put: sym!("rp_mem_put", MemPut),
            mem_start: sym!("rp_mem_start", MemStart),
            mem_wait: sym!("rp_mem_wait", MemWait),
            mem_get: sym!("rp_mem_get", MemGet),
            mem_free: sym!("rp_mem_free", MemFree),
            irq: sym!("rp_irq", Irq),
            close: sym!("rp_close", Close),
            tel_event: sym!("tel_event", TelEvent),
            tel_ctrl: sym!("tel_ctrl", TelCtrl),
            codec_reg: sym!("zs_codec_reg", CodecReg),
            pin_out: sym!("zs_pin_out", PinOut),
            pin_in: sym!("zs_pin_in", PinIn),
            pin_xchg: sym!("zs_pin_xchg", PinXchg),
            _lib: lib,
        }
    }
}

/// The next packet from the library, failing the test on timeout or EOF.
fn recv(sess: &mut Session, what: &str) -> Packet {
    match sess.recv_timeout(WAIT) {
        Ok(Some(p)) => p,
        other => panic!("peer expected {what}, got {other:?}"),
    }
}

fn bus_request(
    cmd: BusCmd,
    id: u32,
    dev: u32,
    addr: u64,
    timestamp: u64,
    data: Vec<u8>,
) -> BusAccess {
    let len = if data.is_empty() {
        4
    } else {
        data.len() as u32
    };
    BusAccess {
        cmd,
        id,
        flags: 0,
        dev,
        timestamp,
        attributes: 0,
        addr,
        len,
        width: 0,
        stream_width: len,
        master_id: 0,
        data,
        byte_enable: Vec::new(),
        ext: true,
    }
}

/// The peer side of the script. Each `recv` is what the library sent after the
/// main thread's call, and each `send` is what the testbench waits for.
fn peer(listener: TcpListener) {
    let (stream, _) = listener.accept().expect("the library connects");
    let mut sess = Session::from_stream(stream).expect("session");

    // Hello: the library advertises the extended busaccess layout and posted wires.
    match recv(&mut sess, "hello") {
        Packet::Hello {
            major, minor, caps, ..
        } => {
            assert_eq!((major, minor), (VERSION_MAJOR, VERSION_MINOR));
            assert!(caps.contains(&cap::BUSACCESS_EXT_BASE), "caps {caps:?}");
            assert!(caps.contains(&cap::WIRE_POSTED_UPDATES), "caps {caps:?}");
        }
        other => panic!("expected hello, got {other:?}"),
    }
    sess.send_hello(&[cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES])
        .expect("hello back");

    // Read: the PL serves a PS read and answers with the data, latency added.
    sess.send(&Packet::Bus(bus_request(
        BusCmd::Read,
        100,
        DEV_GP0,
        0x1000,
        5000,
        Vec::new(),
    )))
    .expect("read request");
    match recv(&mut sess, "the read response") {
        Packet::Bus(r) => {
            assert!(r.is_response() && r.id == 100, "{r:?}");
            assert_eq!(r.response_code(), resp::OK);
            assert_eq!(
                r.data,
                [0xEF, 0xBE, 0xAD, 0xDE],
                "read data is little-endian"
            );
            assert_eq!(r.timestamp, 6000, "5000 + (7000 - 6000) PL latency");
        }
        other => panic!("expected a response, got {other:?}"),
    }

    // Write: the PL accepts a PS write and acknowledges it.
    sess.send(&Packet::Bus(bus_request(
        BusCmd::Write,
        101,
        DEV_GP0,
        0x2000,
        8000,
        vec![0x78, 0x56, 0x34, 0x12],
    )))
    .expect("write request");
    match recv(&mut sess, "the write response") {
        Packet::Bus(r) => {
            assert!(r.is_response() && r.id == 101, "{r:?}");
            assert_eq!(r.response_code(), resp::OK);
        }
        other => panic!("expected a response, got {other:?}"),
    }

    // Pin event out: a posted pin-group update, no answer expected.
    match recv(&mut sess, "the pin update") {
        Packet::Interrupt(i) => {
            assert_eq!(
                (i.dev, i.line, i.val, i.timestamp),
                (DEV_PIN_OUT, 2, 0x5A, 9100)
            );
            assert!(i.flags & flags::POSTED != 0, "pin updates are posted");
        }
        other => panic!("expected a pin update, got {other:?}"),
    }

    // Pin event in: the host drives the levels of group 3.
    sess.send(&Packet::Interrupt(Interrupt {
        id: 300,
        flags: flags::POSTED,
        dev: DEV_PIN_IN,
        timestamp: 0,
        vector: 0,
        line: 3,
        val: 0xC3,
    }))
    .expect("pin input");

    // Pin exchange: the library asks what the devices drive, and waits for the answer.
    match recv(&mut sess, "the pin exchange request") {
        Packet::Bus(r) => {
            assert_eq!((r.cmd, r.dev, r.len), (BusCmd::Read, DEV_PIN_OUT, 1));
            assert_eq!(r.addr, 0x0F01, "group 1 in bits 7:0, levels 0x0F in 15:8");
            let answer = r.response(resp::OK, vec![0x0A], r.timestamp);
            sess.send(&Packet::Bus(answer)).expect("pin answer");
        }
        other => panic!("expected a pin exchange, got {other:?}"),
    }

    // Interrupt wire: with the wire acknowledgement negotiated, the library waits for it.
    match recv(&mut sess, "the wire update") {
        Packet::Interrupt(i) => {
            assert_eq!((i.dev, i.line, i.val), (DEV_WIRES_IN, 2, 1));
            assert_eq!(
                i.flags & flags::POSTED,
                0,
                "acknowledged updates are not posted"
            );
            sess.send(&Packet::Interrupt(Interrupt {
                flags: i.flags | flags::RESPONSE,
                ..i
            }))
            .expect("wire acknowledgement");
        }
        other => panic!("expected a wire update, got {other:?}"),
    }

    // Codec register write seen on the I2C pins.
    match recv(&mut sess, "the codec register") {
        Packet::Interrupt(i) => {
            assert_eq!((i.dev, i.line, i.vector), (DEV_CODEC_REG, 0xB, 0x1FF));
        }
        other => panic!("expected a codec register write, got {other:?}"),
    }

    // Telemetry LED event, from a JSON string.
    match recv(&mut sess, "the LED event") {
        Packet::Interrupt(i) => {
            assert_eq!((i.dev, i.val, i.timestamp), (DEV_BOARD_OUT, 5, 12000));
        }
        other => panic!("expected an LED event, got {other:?}"),
    }

    // Board inputs from the host: switches (line 0) and buttons (line 1).
    for (line, val) in [(0u32, 0x0B), (1, 0x02)] {
        sess.send(&Packet::Interrupt(Interrupt {
            id: 400 + line,
            flags: flags::POSTED,
            dev: DEV_BOARD_IN,
            timestamp: 0,
            vector: 0,
            line,
            val,
        }))
        .expect("board input");
    }

    // Lock-step: the host asks the PL to reach time 12000 and waits for the answer.
    sess.send(&Packet::Sync(Sync {
        id: 500,
        flags: 0,
        dev: 0,
        timestamp: 12000,
    }))
    .expect("sync");
    match recv(&mut sess, "the sync answer") {
        Packet::Sync(s) => {
            assert!(s.flags & flags::RESPONSE != 0, "{s:?}");
            assert_eq!((s.id, s.timestamp), (500, 12000));
        }
        other => panic!("expected a sync answer, got {other:?}"),
    }

    // PL-to-PS write of 8 bytes through the HP0 port, then a read of the same bytes.
    match recv(&mut sess, "the PL write to HP0") {
        Packet::Bus(r) => {
            assert_eq!(
                (r.cmd, r.dev, r.addr, r.len),
                (BusCmd::Write, DEV_HP0, 0x0010_0000, 8)
            );
            assert_eq!(r.data, [0xDD, 0xCC, 0xBB, 0xAA, 0x44, 0x33, 0x22, 0x11]);
            sess.send(&Packet::Bus(r.response(resp::OK, Vec::new(), r.timestamp)))
                .expect("write acknowledgement");
        }
        other => panic!("expected a PL write, got {other:?}"),
    }
    match recv(&mut sess, "the PL read from HP0") {
        Packet::Bus(r) => {
            assert_eq!(
                (r.cmd, r.dev, r.addr, r.len),
                (BusCmd::Read, DEV_HP0, 0x0010_0000, 8)
            );
            let data = vec![1, 2, 3, 4, 5, 6, 7, 8];
            sess.send(&Packet::Bus(r.response(resp::OK, data, r.timestamp)))
                .expect("read data");
        }
        other => panic!("expected a PL read, got {other:?}"),
    }

    // The library closes: the peer sees end of file.
    assert!(
        sess.recv_timeout(WAIT).is_err(),
        "after rp_close the peer must see the connection end"
    );
}

#[test]
fn the_shared_object_speaks_remote_port_through_the_c_abi() {
    let dpi = Dpi::load();

    // The endpoint grammar of xsim-dpi::core::parse_endpoint: optional `tcp:`,
    // dotted IPv4, a 16-bit port. Malformed text is refused before any connection.
    for text in [
        "not an endpoint",
        "tcp:127.0.0.1:99999",
        "127.0.0.1",
        "tcp:1.2.3:9",
        "tcp:300.0.0.1:9",
    ] {
        let bad = CString::new(text).expect("no NUL");
        assert_eq!(unsafe { (dpi.open)(bad.as_ptr()) }, -1, "rp_open({text})");
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("address").port();
    let peer_thread = std::thread::spawn(move || peer(listener));

    let endpoint = CString::new(format!("tcp:127.0.0.1:{port}")).expect("no NUL");
    assert_eq!(unsafe { (dpi.open)(endpoint.as_ptr()) }, 0, "rp_open");

    // Lock-step is on from the start: the peer's sync arrives after the LED event,
    // which is after this call, so the library holds it for `rp_sync_ack`.
    unsafe { (dpi.lockstep)(1) };

    // Hello: the peer's hello is pumped by rp_poll.
    assert_eq!(
        unsafe { (dpi.poll)(5000) },
        1,
        "rp_poll sees the peer's hello"
    );

    // Read transaction. The PL sees the beat, completes it with data, and the
    // response carries the PL latency (done_t at 7000, request at 6000).
    let (mut addr, mut wdata, mut wstrb, mut is_write) = (0i32, 0i32, 0i32, 0i32);
    let ready = unsafe { (dpi.next)(6000, 2000, &mut addr, &mut wdata, &mut wstrb, &mut is_write) };
    assert_eq!(ready, 1, "rp_next returns a beat");
    assert_eq!((addr, is_write, wstrb), (0x1000, 0, 0xF));
    unsafe { (dpi.done_t)(0xDEAD_BEEFu32 as i32, 1, 7000) };

    // Write transaction: the beat carries the data lanes, and rp_done acknowledges.
    let ready = unsafe { (dpi.next)(9000, 2000, &mut addr, &mut wdata, &mut wstrb, &mut is_write) };
    assert_eq!(ready, 1, "rp_next returns the write beat");
    assert_eq!(
        (addr, is_write, wdata, wstrb),
        (0x2000, 1, 0x1234_5678, 0xF)
    );
    unsafe { (dpi.done_t)(0, 1, 9000) };

    // Pin event out and in, and the blocking pin exchange.
    unsafe { (dpi.pin_out)(2, 0x5A, 9100) };
    assert_eq!(unsafe { (dpi.poll)(5000) }, 1, "rp_poll sees the pin input");
    assert_eq!(unsafe { (dpi.pin_in)(3) }, 0xC3, "zs_pin_in(3)");
    assert_eq!(unsafe { (dpi.pin_in)(2) }, 0, "group 2 was never driven in");
    assert_eq!(
        unsafe { (dpi.pin_xchg)(1, 0x0F, 10000) },
        0x0A,
        "zs_pin_xchg answer"
    );

    // Interrupt wire, acknowledged by the peer before rp_irq returns.
    unsafe { (dpi.irq)(11000, 2, 1) };
    unsafe { (dpi.codec_reg)(0x0B, 0x1FF, 11500) };

    // LED event from a C string, and the board inputs.
    let led = CString::new(r#"{"k":"led","v":5,"t":12000}"#).expect("no NUL");
    unsafe { (dpi.tel_event)(led.as_ptr()) };
    assert_eq!(unsafe { (dpi.poll)(5000) }, 1, "first board input");
    assert_eq!(unsafe { (dpi.poll)(5000) }, 1, "second board input");
    // Lock-step: the sync target is visible, and the ack goes back to the peer.
    // The sync is the next packet, so the blocking poll takes it in order. (The
    // library answers a sync at once unless lock-step is on, which is why it was
    // enabled before any traffic; `tel_ctrl` must not run first, since its
    // non-blocking pump could take the sync and leave nothing to poll.)
    assert_eq!(unsafe { (dpi.poll)(5000) }, 1, "rp_poll sees the sync");
    assert_eq!(unsafe { (dpi.sync_target)() }, 12000, "rp_sync_target");
    unsafe { (dpi.sync_ack)(12000) };

    let (mut sw, mut btn) = (0i32, 0i32);
    assert_eq!(unsafe { (dpi.tel_ctrl)(&mut sw, &mut btn) }, 0);
    assert_eq!(
        (sw, btn),
        (0x0B, 0x02),
        "tel_ctrl hands over switches and buttons"
    );

    // PL-to-PS through the memory master: a write, then a read of the same bytes.
    unsafe {
        (dpi.mem_put)(0, 0xAABB_CCDDu32 as i32);
        (dpi.mem_put)(1, 0x1122_3344);
    }
    let slot = unsafe { (dpi.mem_start)(13000, DEV_HP0 as i32, 1, 0x0010_0000, 8) };
    assert!(slot >= 0, "rp_mem_start (write) returns a slot");
    assert_eq!(unsafe { (dpi.mem_wait)(slot) }, 1, "rp_mem_wait (write)");
    unsafe { (dpi.mem_free)(slot) };

    let slot = unsafe { (dpi.mem_start)(14000, DEV_HP0 as i32, 0, 0x0010_0000, 8) };
    assert!(slot >= 0, "rp_mem_start (read) returns a slot");
    assert_eq!(unsafe { (dpi.mem_wait)(slot) }, 1, "rp_mem_wait (read)");
    assert_eq!(unsafe { (dpi.mem_get)(slot, 0) } as u32, 0x0403_0201);
    assert_eq!(unsafe { (dpi.mem_get)(slot, 1) } as u32, 0x0807_0605);
    unsafe { (dpi.mem_free)(slot) };

    unsafe { (dpi.close)() };
    peer_thread.join().expect("the peer's checks passed");
}
