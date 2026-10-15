//! Xilinx *remote-port* protocol in safe Rust: packet encoding and decoding,
//! plus a blocking TCP session with capability negotiation.
//!
//! This is the wire protocol QEMU's `remote-port` device speaks to the PL side
//! (protocol version 4.3, as in `libsystemctlm-soc`). Everything here is pure
//! data handling with no `unsafe` and no dependency; the encodings are tested
//! byte for byte against the C reference implementation.
//!
//! All numeric fields on the wire are big-endian.
//!
//! The packet codec is `no_std` + `alloc` so the same code runs inside the
//! simulator-side DPI library (which cannot link `std`). The TCP [`Session`]
//! needs the default `std` feature.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod packet;
#[cfg(feature = "std")]
mod session;

pub use packet::{
    BusAccess, BusCmd, Cmd, DecodeError, HEADER_LEN, Header, Interrupt, Packet, Sync, attr, cap,
    flags, resp,
};
#[cfg(feature = "std")]
pub use session::{Session, SessionError};

/// Protocol major version spoken by this crate.
pub const VERSION_MAJOR: u16 = 4;
/// Protocol minor version spoken by this crate.
pub const VERSION_MINOR: u16 = 3;
