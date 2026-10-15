//! A small client for QEMU's gdb remote serial protocol stub.
//!
//! Why this exists: with the remote-port lock-step, QEMU's vCPU thread holds
//! the global lock while it waits for a sync, so neither the monitor nor the
//! gdb stub can answer while the machine is merely *paused*. A stop caused by
//! the guest itself (breakpoint, watchpoint, single step) releases the lock,
//! and then the stub answers at once. This client therefore only speaks to a
//! machine that has stopped on its own, and sends fire-and-forget requests
//! (`Z0`...) otherwise.
//!
//! Protocol notes: packets are `$payload#cc` and every packet is acknowledged
//! with `+` (this QEMU does not offer `QStartNoAckMode`). With `-S` the stub
//! sends an unsolicited stop reply when the debugger attaches. Register block
//! layout of QEMU's Cortex-A9 stub (checked against a live session): words
//! 0..15 are r0..r15, word 40 is `fps`, word 41 is CPSR.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// What the guest stopped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    /// gdb thread number (1 = core 0, 2 = core 1).
    pub thread: u8,
    /// Signal number of the stop reply (5 = SIGTRAP).
    pub signal: u8,
    /// Set when a watchpoint caused the stop.
    pub watch_addr: Option<u32>,
}

/// Kinds of breakpoint the stub supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakKind {
    /// Execution breakpoint (`Z0`).
    Exec,
    /// Break on write (`Z2`).
    Write,
    /// Break on read (`Z3`).
    Read,
    /// Break on read or write (`Z4`).
    Access,
}

impl BreakKind {
    fn code(self) -> u8 {
        match self {
            BreakKind::Exec => 0,
            BreakKind::Write => 2,
            BreakKind::Read => 3,
            BreakKind::Access => 4,
        }
    }
}

/// Registers of one core as the stub reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Regs {
    /// r0..r15 (`r[13]` = sp, `r[14]` = lr, `r[15]` = pc).
    pub r: [u32; 16],
    /// Current program status register.
    pub cpsr: u32,
}

/// Errors from the debugger connection.
#[derive(Debug)]
pub enum GdbError {
    /// Socket error.
    Io(std::io::Error),
    /// No (valid) answer in time.
    Timeout,
    /// The stub answered with an error or something unexpected.
    Protocol(String),
}

impl std::fmt::Display for GdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GdbError::Io(e) => write!(f, "gdb connection: {e}"),
            GdbError::Timeout => write!(f, "the gdb stub did not answer in time"),
            GdbError::Protocol(m) => write!(f, "gdb stub: {m}"),
        }
    }
}

impl std::error::Error for GdbError {}

impl From<std::io::Error> for GdbError {
    fn from(e: std::io::Error) -> GdbError {
        GdbError::Io(e)
    }
}

/// A connection to the stub.
pub struct GdbClient {
    s: TcpStream,
    buf: Vec<u8>,
    /// Replies still owed for fire-and-forget requests; discarded on arrival.
    pending: usize,
}

fn checksum(p: &str) -> u8 {
    p.bytes().fold(0u8, |a, b| a.wrapping_add(b))
}

impl GdbClient {
    /// Connects to `127.0.0.1:port` and selects thread 1, which also proves
    /// the stub answers (it does while the CPUs are stopped with `-S`).
    pub fn connect(port: u16) -> Result<GdbClient, GdbError> {
        // QEMU opens the stub while it initialises; on a loaded machine the
        // first attempt can come before that.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let s = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => break s,
                Err(e)
                    if e.kind() == std::io::ErrorKind::ConnectionRefused
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e.into()),
            }
        };
        s.set_nodelay(true)?;
        let mut c = GdbClient {
            s,
            buf: Vec::new(),
            pending: 0,
        };
        let r = c.command("Hg1", Duration::from_secs(10))?;
        if r != "OK" {
            return Err(GdbError::Protocol(format!("Hg1 answered `{r}`")));
        }
        Ok(c)
    }

    fn send(&mut self, payload: &str) -> Result<(), GdbError> {
        let msg = format!("${payload}#{:02x}", checksum(payload));
        self.s.write_all(msg.as_bytes())?;
        Ok(())
    }

    /// Reads one packet payload, or `None` on timeout. Acks (`+`/`-`) are
    /// skipped; bad checksums are ignored.
    fn read_packet(&mut self, timeout: Duration) -> Result<Option<String>, GdbError> {
        let deadline = Instant::now() + timeout;
        loop {
            // A complete packet already buffered?
            if let Some(start) = self.buf.iter().position(|&b| b == b'$') {
                if let Some(rel) = self.buf[start..].iter().position(|&b| b == b'#') {
                    let hash = start + rel;
                    if self.buf.len() >= hash + 3 {
                        let payload =
                            String::from_utf8_lossy(&self.buf[start + 1..hash]).into_owned();
                        self.buf.drain(..hash + 3);
                        self.s.write_all(b"+")?;
                        return Ok(Some(payload));
                    }
                }
            } else {
                self.buf.clear(); // only acks so far
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.s
                .set_read_timeout(Some(left.min(Duration::from_millis(50))))?;
            let mut tmp = [0u8; 2048];
            match self.s.read(&mut tmp) {
                Ok(0) => return Err(GdbError::Protocol("connection closed".into())),
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Sends a request and waits for its reply (the machine must be stopped).
    pub fn command(&mut self, payload: &str, timeout: Duration) -> Result<String, GdbError> {
        self.send(payload)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.read_packet(left)? {
                None => return Err(GdbError::Timeout),
                Some(p) => {
                    // A reply owed to an earlier fire-and-forget request.
                    if self.pending > 0 && (p == "OK" || p.starts_with('E')) {
                        self.pending -= 1;
                        continue;
                    }
                    // A stop reply (the attach notification) is not an answer.
                    if parse_stop(&p).is_some() {
                        continue;
                    }
                    return Ok(p);
                }
            }
        }
    }

    /// Sends a request whose reply is not waited for. Used while the machine
    /// is paused at a sync (the stub only answers once it runs again).
    pub fn send_async(&mut self, payload: &str) -> Result<(), GdbError> {
        self.send(payload)?;
        self.pending += 1;
        Ok(())
    }

    /// Continues execution (`c`); no reply until the next stop.
    pub fn cont(&mut self) -> Result<(), GdbError> {
        self.send("c")
    }

    /// Single-steps the current thread (`s`); the stop reply arrives later.
    pub fn step(&mut self) -> Result<(), GdbError> {
        self.send("s")
    }

    /// A stop reply that has arrived, if any (non-blocking).
    pub fn poll_stop(&mut self) -> Result<Option<Stop>, GdbError> {
        loop {
            match self.read_packet(Duration::from_millis(1))? {
                None => return Ok(None),
                Some(p) => {
                    if self.pending > 0 && (p == "OK" || p.starts_with('E')) {
                        self.pending -= 1;
                        continue;
                    }
                    if let Some(stop) = parse_stop(&p) {
                        return Ok(Some(stop));
                    }
                    // Anything else unsolicited is not a stop: ignore.
                }
            }
        }
    }

    /// Plants or removes a breakpoint (`insert` false removes), synchronously.
    pub fn set_break(
        &mut self,
        kind: BreakKind,
        insert: bool,
        addr: u32,
        len: u32,
    ) -> Result<(), GdbError> {
        let op = if insert { 'Z' } else { 'z' };
        let len = if kind == BreakKind::Exec {
            4
        } else {
            len.max(1)
        };
        let r = self.command(
            &format!("{op}{},{addr:x},{len:x}", kind.code()),
            Duration::from_secs(5),
        )?;
        if r == "OK" {
            Ok(())
        } else {
            Err(GdbError::Protocol(format!(
                "request refused ({r}); `{kind:?}` at {addr:#x}"
            )))
        }
    }

    /// Reads the registers of gdb thread `thread` (1 = core 0).
    pub fn regs(&mut self, thread: u8) -> Result<Regs, GdbError> {
        let r = self.command(&format!("Hg{thread}"), Duration::from_secs(5))?;
        if r != "OK" {
            return Err(GdbError::Protocol(format!("no thread {thread} ({r})")));
        }
        let g = self.command("g", Duration::from_secs(5))?;
        parse_regs(&g)
    }
}

fn parse_stop(p: &str) -> Option<Stop> {
    let b = p.as_bytes();
    if b.len() < 3 || !(b[0] == b'T' || b[0] == b'S') {
        return None;
    }
    let signal = u8::from_str_radix(p.get(1..3)?, 16).ok()?;
    let mut thread = 1;
    let mut watch_addr = None;
    if b[0] == b'T' {
        for field in p[3..].split(';') {
            if let Some(t) = field.strip_prefix("thread:") {
                // `thread:01` or `thread:p01.01`.
                let id = t.rsplit('.').next().unwrap_or(t);
                thread = u8::from_str_radix(id, 16).unwrap_or(1);
            } else if let Some(a) = field
                .strip_prefix("watch:")
                .or_else(|| field.strip_prefix("rwatch:"))
                .or_else(|| field.strip_prefix("awatch:"))
            {
                watch_addr = u32::from_str_radix(a, 16).ok();
            }
        }
    }
    Some(Stop {
        thread,
        signal,
        watch_addr,
    })
}

fn parse_regs(hex: &str) -> Result<Regs, GdbError> {
    if hex.len() < 42 * 8 || hex.starts_with('E') {
        return Err(GdbError::Protocol(format!(
            "short register block ({} hex digits)",
            hex.len()
        )));
    }
    let word = |i: usize| -> Result<u32, GdbError> {
        let s = hex
            .get(i * 8..i * 8 + 8)
            .ok_or_else(|| GdbError::Protocol("short register block".into()))?;
        let b = u32::from_str_radix(s, 16)
            .map_err(|_| GdbError::Protocol(format!("bad register data `{s}`")))?;
        Ok(b.swap_bytes()) // little-endian target
    };
    let mut r = [0u32; 16];
    for (i, v) in r.iter_mut().enumerate() {
        *v = word(i)?;
    }
    Ok(Regs { r, cpsr: word(41)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_replies_are_parsed() {
        assert_eq!(
            parse_stop("T05thread:01;"),
            Some(Stop {
                thread: 1,
                signal: 5,
                watch_addr: None
            })
        );
        assert_eq!(
            parse_stop("T05thread:02;watch:43c00004;"),
            Some(Stop {
                thread: 2,
                signal: 5,
                watch_addr: Some(0x43c0_0004)
            })
        );
        assert_eq!(parse_stop("S05").map(|s| s.signal), Some(5));
        assert_eq!(parse_stop("OK"), None);
        assert_eq!(parse_stop("E01"), None);
    }

    #[test]
    fn the_register_block_is_decoded_little_endian() {
        // Words: r0 = 0xf8000008, pc (r15) = 0x0010058c, cpsr at word 41.
        let mut words = [0u32; 48];
        words[0] = 0xf800_0008;
        words[13] = 0x0010_c000;
        words[15] = 0x0010_058c;
        words[41] = 0x6000_00df;
        let hex: String = words
            .iter()
            .map(|w| format!("{:08x}", w.swap_bytes()))
            .collect();
        let r = parse_regs(&hex).unwrap();
        assert_eq!(r.r[0], 0xf800_0008);
        assert_eq!(r.r[13], 0x0010_c000);
        assert_eq!(r.r[15], 0x0010_058c);
        assert_eq!(r.cpsr, 0x6000_00df);
        assert!(parse_regs("E01").is_err());
        assert!(parse_regs("00").is_err());
    }

    #[test]
    fn checksums_match_the_protocol() {
        assert_eq!(checksum("OK"), 0x9a);
        assert_eq!(checksum("g"), 0x67);
    }
}
