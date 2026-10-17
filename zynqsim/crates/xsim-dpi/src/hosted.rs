//! The hosted (std) transport and the one DPI instance of a shared object or a
//! VPI module: a TCP socket, the `RP_SOCKET` lookup and the single-instance cell.
//!
//! Shared by `dpi-cdylib` (Questa, VCS, Verilator, ...) and `vpi-icarus` (Icarus
//! Verilog through `vvp`), so the remote-port logic and the socket code exist once.
//! Each package expands [`export_dpi_abi!`](crate::export_dpi_abi) over [`dpi`] and
//! [`open_endpoint`]. Not compiled in the freestanding Windows archive (no std there).
//!
//! The cell is the same as the archive's: the DPI contract is single-threaded, one
//! instance per process, never re-entered. The `unsafe` is that cell.

use std::cell::UnsafeCell;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

use crate::core::{parse_endpoint, Dpi, Transport};

/// A remote-port connection over a TCP stream.
pub struct Socket(Option<TcpStream>);

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
pub fn dpi() -> &'static mut Dpi<Socket> {
    // SAFETY: single-threaded by the DPI contract, and never re-entered, so the
    // reference is the only live one.
    let slot = unsafe { &mut *DPI.0.get() };
    slot.get_or_insert_with(Dpi::new)
}

/// `rp_open` for hosted packages: `given` is the endpoint text (`tcp:a.b.c.d:port`
/// or `a.b.c.d:port`), or empty for the `RP_SOCKET` environment variable.
/// Returns 0 on success and -1 on failure, with the reason on stderr.
pub fn open_endpoint(given: &[u8]) -> i32 {
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
