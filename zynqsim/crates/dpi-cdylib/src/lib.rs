//! The DPI-C ABI of the remote-port peer as a shared object.
//!
//! `xsim-dpi` holds the protocol logic and the export table (`xsim_dpi::abi`).
//! This crate adds only what a shared object needs on top of them: the socket
//! (`std::net::TcpStream`, on every platform) and the `rp_open` endpoint lookup.
//! The export names and C signatures are the same as the static archive, so one
//! testbench imports either. Build with `cargo xtask dpi-shared`; the result is
//! `zynqsim_dpi.dll`, `libzynqsim_dpi.so` or `libzynqsim_dpi.dylib`.
//!
//! Like the archive, the library is single-threaded by the DPI contract: one
//! instance per process, never re-entered. It is built with `panic = "abort"`, so
//! a panic ends the simulator process rather than unwinding into it.
//!
//! This crate is an FFI boundary and must use `unsafe`, so it does not inherit
//! the workspace `unsafe_code = "forbid"` (same exemption as `xsim-dpi`). The
//! `unsafe` is the single-instance cell below and the export layer in
//! `xsim_dpi::abi`.

use std::cell::UnsafeCell;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

use xsim_dpi::core::{parse_endpoint, Dpi, Transport};

// The exported symbols. `#[no_mangle]` items are exported from the shared object
// whatever their visibility, so they live in a private module.
mod abi {
    use crate::{dpi, open_endpoint};

    xsim_dpi::export_dpi_abi!(dpi, open_endpoint);
}

/// A remote-port connection over a TCP stream.
struct Socket(Option<TcpStream>);

impl Transport for Socket {
    fn send(&mut self, data: &[u8]) -> bool {
        self.0.as_mut().is_some_and(|s| s.write_all(data).is_ok())
    }

    fn recv_exact(&mut self, buf: &mut [u8]) -> bool {
        self.0.as_mut().is_some_and(|s| s.read_exact(buf).is_ok())
    }

    fn readable(&mut self, timeout_ms: i32) -> bool {
        let Some(s) = self.0.as_ref() else {
            return false;
        };
        let mut probe = [0u8; 1];
        let peeked = if timeout_ms == 0 {
            if s.set_nonblocking(true).is_err() {
                return false;
            }
            let r = s.peek(&mut probe);
            let _ = s.set_nonblocking(false);
            r
        } else {
            // A negative timeout blocks until data or end of file.
            let limit = (timeout_ms > 0)
                .then(|| Duration::from_millis(u64::from(timeout_ms.unsigned_abs())));
            if s.set_read_timeout(limit).is_err() {
                return false;
            }
            let r = s.peek(&mut probe);
            let _ = s.set_read_timeout(None);
            r
        };
        match peeked {
            // Data, or end of file: the next `recv_exact` reports which.
            Ok(_) => true,
            Err(e) => !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        }
    }

    fn close(&mut self) {
        if let Some(s) = self.0.take() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// The one DPI instance. Racy by design: the DPI contract is single-threaded.
struct Racy<T>(UnsafeCell<T>);

// SAFETY: the simulator calls the DPI functions from one thread and never re-enters them.
unsafe impl<T> Sync for Racy<T> {}

static DPI: Racy<Option<Dpi<Socket>>> = Racy(UnsafeCell::new(None));

/// The one DPI instance, created on first use.
fn dpi() -> &'static mut Dpi<Socket> {
    // SAFETY: single-threaded by the DPI contract, and never re-entered, so the
    // reference is the only live one.
    let slot = unsafe { &mut *DPI.0.get() };
    slot.get_or_insert_with(Dpi::new)
}

/// `rp_open` for shared objects: `given` is the endpoint text (`tcp:a.b.c.d:port`
/// or `a.b.c.d:port`), or empty for the `RP_SOCKET` environment variable.
fn open_endpoint(given: &[u8]) -> i32 {
    let env_text;
    let target: &[u8] = if given.is_empty() {
        env_text = std::env::var("RP_SOCKET").unwrap_or_default();
        env_text.as_bytes()
    } else {
        given
    };
    let Some((octets, port)) = parse_endpoint(target) else {
        eprintln!("rp_open: bad endpoint (want tcp:a.b.c.d:port)");
        return -1;
    };
    let Ok(stream) = TcpStream::connect(SocketAddr::from((octets, port))) else {
        eprintln!("rp_open: cannot connect");
        return -1;
    };
    if stream.set_nodelay(true).is_err() {
        return -1;
    }
    if dpi().open(Socket(Some(stream))) {
        0
    } else {
        eprintln!("rp_open: hello failed");
        -1
    }
}
