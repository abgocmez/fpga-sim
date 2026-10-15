use crate::packet::{DecodeError, HEADER_LEN, Header, Packet, cap};
use crate::{VERSION_MAJOR, VERSION_MINOR};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Refuse packets larger than this; real traffic is bounded by 4 KiB bursts.
const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

/// Errors from a [`Session`].
#[derive(Debug)]
pub enum SessionError {
    /// An I/O error on the socket.
    Io(io::Error),
    /// A packet could not be decoded.
    Decode(DecodeError),
    /// The peer closed the connection.
    Closed,
    /// The peer sent something the protocol does not allow.
    Protocol(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "remote-port I/O error: {e}"),
            SessionError::Decode(e) => write!(f, "remote-port decode error: {e}"),
            SessionError::Closed => write!(f, "remote-port peer closed the connection"),
            SessionError::Protocol(m) => write!(f, "remote-port protocol error: {m}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<io::Error> for SessionError {
    fn from(e: io::Error) -> Self {
        SessionError::Io(e)
    }
}

impl From<DecodeError> for SessionError {
    fn from(e: DecodeError) -> Self {
        SessionError::Decode(e)
    }
}

/// A blocking remote-port connection over TCP.
///
/// The session tracks the peer's hello (version and capabilities) as packets
/// pass through [`Session::recv`], and hands out request ids.
pub struct Session {
    stream: TcpStream,
    peer_caps: Vec<u32>,
    peer_version: Option<(u16, u16)>,
    next_id: u32,
}

impl Session {
    /// Wraps an established stream.
    pub fn from_stream(stream: TcpStream) -> io::Result<Session> {
        stream.set_nodelay(true)?;
        Ok(Session {
            stream,
            peer_caps: Vec::new(),
            peer_version: None,
            next_id: 0,
        })
    }

    /// Connects to `addr`, retrying until `deadline` elapses. Useful because
    /// QEMU opens its listening socket a moment after it is spawned.
    pub fn connect(
        addr: impl ToSocketAddrs + Copy,
        deadline: Duration,
    ) -> Result<Session, SessionError> {
        let start = Instant::now();
        loop {
            match TcpStream::connect(addr) {
                Ok(s) => return Ok(Session::from_stream(s)?),
                Err(e) if start.elapsed() < deadline => {
                    let _ = e;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Sends our hello advertising `caps`.
    pub fn send_hello(&mut self, caps: &[u32]) -> Result<(), SessionError> {
        let id = self.alloc_id();
        self.send(&Packet::Hello {
            id,
            dev: 0,
            major: VERSION_MAJOR,
            minor: VERSION_MINOR,
            caps: caps.to_vec(),
        })
    }

    /// Closes both directions of the connection. The peer sees end of file.
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Allocates a fresh request id.
    pub fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// True when the peer advertised capability `c` in its hello.
    pub fn peer_has(&self, c: u32) -> bool {
        self.peer_caps.contains(&c)
    }

    /// True when the peer uses the extended busaccess layout.
    pub fn peer_ext_base(&self) -> bool {
        self.peer_has(cap::BUSACCESS_EXT_BASE)
    }

    /// The peer's protocol version, once its hello has been received.
    pub fn peer_version(&self) -> Option<(u16, u16)> {
        self.peer_version
    }

    /// Writes one packet.
    pub fn send(&mut self, p: &Packet) -> Result<(), SessionError> {
        self.stream.write_all(&p.encode())?;
        Ok(())
    }

    /// Reads one packet, blocking.
    pub fn recv(&mut self) -> Result<Packet, SessionError> {
        self.stream.set_read_timeout(None)?;
        self.read_packet()
    }

    /// Reads one packet if one arrives within `timeout`.
    ///
    /// The timeout only bounds the wait for the first byte; once a packet has
    /// started it is read to completion so framing is never lost.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Option<Packet>, SessionError> {
        let timeout = timeout.max(Duration::from_millis(1));
        self.stream.set_read_timeout(Some(timeout))?;
        let mut one = [0u8; 1];
        match self.stream.peek(&mut one) {
            Ok(0) => return Err(SessionError::Closed),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        }
        self.stream.set_read_timeout(None)?;
        self.read_packet().map(Some)
    }

    fn read_packet(&mut self) -> Result<Packet, SessionError> {
        let mut h = [0u8; HEADER_LEN];
        match self.stream.read_exact(&mut h) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(SessionError::Closed),
            Err(e) => return Err(e.into()),
        }
        let hdr = Header::parse(&h);
        if hdr.len > MAX_PAYLOAD {
            return Err(SessionError::Protocol(format!(
                "payload of {} bytes exceeds the {MAX_PAYLOAD} limit",
                hdr.len
            )));
        }
        let mut payload = vec![0u8; hdr.len as usize];
        self.stream.read_exact(&mut payload).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                SessionError::Closed
            } else {
                SessionError::Io(e)
            }
        })?;
        let p = Packet::decode(hdr, &payload)?;
        if let Packet::Hello {
            major, minor, caps, ..
        } = &p
        {
            self.peer_version = Some((*major, *minor));
            self.peer_caps = caps.clone();
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{BusAccess, BusCmd, flags};
    use std::net::TcpListener;

    fn pair() -> (Session, Session) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let c = std::thread::spawn(move || Session::connect(addr, Duration::from_secs(2)).unwrap());
        let (s, _) = l.accept().unwrap();
        (Session::from_stream(s).unwrap(), c.join().unwrap())
    }

    #[test]
    fn hello_exchange_records_capabilities_and_version() {
        let (mut a, mut b) = pair();
        a.send_hello(&[cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES])
            .unwrap();
        let p = b.recv().unwrap();
        assert!(matches!(
            p,
            Packet::Hello {
                major: 4,
                minor: 3,
                ..
            }
        ));
        assert!(b.peer_ext_base());
        assert!(b.peer_has(cap::WIRE_POSTED_UPDATES));
        assert!(!b.peer_has(cap::ATS));
        assert_eq!(b.peer_version(), Some((4, 3)));
    }

    #[test]
    fn bus_round_trip_and_ids() {
        let (mut a, mut b) = pair();
        let id0 = a.alloc_id();
        let id1 = a.alloc_id();
        assert_eq!((id0, id1), (0, 1));
        let w = BusAccess {
            cmd: BusCmd::Write,
            id: id1,
            flags: flags::POSTED,
            dev: 7,
            timestamp: 42,
            attributes: 0,
            addr: 0x43C0_0000,
            len: 4,
            width: 0,
            stream_width: 4,
            master_id: 0,
            data: vec![1, 2, 3, 4],
            byte_enable: vec![],
            ext: true,
        };
        a.send(&Packet::Bus(w.clone())).unwrap();
        match b.recv().unwrap() {
            Packet::Bus(got) => {
                assert_eq!(got.data, w.data);
                assert_eq!(got.addr, w.addr);
                assert!(got.is_posted());
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn timeout_close_and_garbage() {
        let (a, mut b) = pair();
        assert!(b.recv_timeout(Duration::from_millis(30)).unwrap().is_none());
        drop(a);
        assert!(matches!(b.recv(), Err(SessionError::Closed)));

        // An absurd length is refused rather than allocated.
        let (mut a, mut b) = pair();
        let mut raw = vec![0u8; HEADER_LEN];
        raw[4..8].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
        a.stream.write_all(&raw).unwrap();
        assert!(matches!(b.recv(), Err(SessionError::Protocol(_))));
    }

    #[test]
    fn connect_gives_up_after_the_deadline() {
        // Port 1 on loopback is essentially never listening.
        let r = Session::connect("127.0.0.1:1", Duration::from_millis(150));
        assert!(matches!(r, Err(SessionError::Io(_))));
    }
}
