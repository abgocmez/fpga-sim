//! Interfaces between a PS engine and a PL implementation.
//!
//! A PS engine (QEMU today, a native Cortex-A9 later) calls a [`PlTarget`] for
//! every access that lands in the programmable logic, and gives it a
//! [`PsPort`] so the PL can master memory and raise interrupts. The PL may be
//! a native Rust model, an RTL simulator behind an adapter, or both at once;
//! the PS side cannot tell the difference.
//!
//! Times are nanoseconds of PS virtual time, as carried by remote-port.

use std::fmt;

pub mod media;
pub use media::{AudioSource, MediaEvent, VideoSource};

/// Why a bus access failed. Maps onto remote-port response codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    /// Nothing decodes the address (AXI DECERR).
    Decode,
    /// The target decoded the address and signalled an error (AXI SLVERR).
    Slave,
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BusError::Decode => write!(f, "address not decoded (DECERR)"),
            BusError::Slave => write!(f, "slave error (SLVERR)"),
        }
    }
}

impl std::error::Error for BusError {}

/// Result of a bus access.
pub type BusResult<T> = Result<T, BusError>;

/// A PL master port into the PS, numbered as the Xilinx co-simulation device
/// tree numbers them (prompt appendix §19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsMasterPort {
    /// `S_AXI_GP0`.
    SAxiGp0,
    /// `S_AXI_GP1`.
    SAxiGp1,
    /// `S_AXI_HP0` (high-performance port 0).
    SAxiHp0,
    /// `S_AXI_HP1`.
    SAxiHp1,
    /// `S_AXI_HP2`.
    SAxiHp2,
    /// `S_AXI_HP3`.
    SAxiHp3,
    /// `S_AXI_ACP`.
    SAxiAcp,
}

impl PsMasterPort {
    /// The remote-port device number of this port.
    pub fn device(self) -> u32 {
        match self {
            PsMasterPort::SAxiGp0 => 0,
            PsMasterPort::SAxiGp1 => 1,
            PsMasterPort::SAxiHp0 => 2,
            PsMasterPort::SAxiHp1 => 3,
            PsMasterPort::SAxiHp2 => 4,
            PsMasterPort::SAxiHp3 => 5,
            PsMasterPort::SAxiAcp => 6,
        }
    }
}

/// Remote-port device number of `M_AXI_GP0` (PS master into the PL).
pub const DEV_M_AXI_GP0: u32 = 7;
/// Remote-port device number of `M_AXI_GP1`.
pub const DEV_M_AXI_GP1: u32 = 8;
/// Remote-port device number of the PL-to-PS interrupt wires (`IRQ_F2P`).
pub const DEV_WIRES_PL_TO_PS: u32 = 9;
/// Remote-port device number of the PS-to-PL wires (resets).
pub const DEV_WIRES_PS_TO_PL: u32 = 10;
/// Device number the adapter uses to send board input pins (switches on
/// line 0, push buttons on line 1; bit n = component n of the board file, up to 8 bits in
/// the packet's `val`: ADR-022; the Zybo uses the low 4) to
/// the RTL testbench. Not a QEMU device: it lives between host and DPI.
pub const DEV_BOARD_IN: u32 = 12;
/// Device number the testbench side uses to report board output pins (LEDs on
/// line 0, bit n = LED n of the board file, up to 8 bits in `val`).
pub const DEV_BOARD_OUT: u32 = 13;
/// Device number the testbench side uses to report the levels of a *pin
/// group* (a Pmod port or the RGB LEDs): interrupt packets, `line` = group,
/// `val` = 8 pin levels, stamped with the PL time of the change. Bus *reads*
/// on the same device are the synchronous exchange (`zs_pin_xchg`): the
/// address carries `group | levels << 8` and the one-byte answer is what the
/// devices on the port drive into the PL's input pins (ADR-009).
pub const DEV_PIN_OUT: u32 = 19;
/// Device number the adapter uses to drive input pin groups (Pmod buttons...).
pub const DEV_PIN_IN: u32 = 20;
/// Pin group of the RGB LEDs: bits 0..2 = LD5 R,G,B and bits 3..5 = LD6 R,G,B.
pub const PIN_GROUP_RGB: u8 = 6;

/// A memory access the PL itself initiated (a PL master, for example a DMA
/// engine on `S_AXI_HP0`), for the trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MasterAccess {
    /// PS time of the access in nanoseconds.
    pub t_ns: u64,
    /// The PS port it went through.
    pub port: PsMasterPort,
    /// True for a write.
    pub write: bool,
    /// Address.
    pub addr: u64,
    /// Length in bytes.
    pub len: u32,
    /// The first up to 8 data bytes, little-endian.
    pub data: u64,
    /// False if the PS answered with an error.
    pub ok: bool,
}

/// The levels of a pin group changed (Pmod port `group` 0..=4 are JA..JE,
/// [`PIN_GROUP_RGB`] the RGB LEDs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinEvent {
    /// PL time of the change in nanoseconds.
    pub t_ns: u64,
    /// Pin group.
    pub group: u8,
    /// Levels of the group's eight pins, bit *n* = pin *n*.
    pub levels: u8,
}

/// A byte a PL-side UART (the AXI UART Lite model) put on its TX line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UartByte {
    /// PS time at which the character finished shifting out, in nanoseconds.
    pub t_ns: u64,
    /// Channel index, in the order [`PlTarget::uart_channels`] lists them.
    pub channel: u8,
    /// The byte.
    pub byte: u8,
}

/// The board side of the PL's Pmod pins: whatever is plugged into the ports.
///
/// The PL adapter calls it on its own thread of control, in PL-time order, the
/// moment a pin change arrives from the testbench (also while the PS is held
/// at a lock-step sync), so the answer can be returned to the testbench in
/// the same simulated instant.
pub trait PinSink: Send {
    /// The pins of `group` changed to `levels` at PL time `t_ns`. Returns the
    /// levels the board drives into the PL's input pins of that group (1 =
    /// released or high, see `devices::PinDrive`).
    fn pin_change(&mut self, group: u8, t_ns: u64, levels: u8) -> u8;

    /// The PL's video input asks for line `y` of frame `frame` (`w` pixels
    /// of a `h`-line picture). A board device that drives that input (a
    /// camera) answers with `3 * w` bytes of RGB; `None` leaves the host's
    /// video source (`[media] video_in`) in charge.
    fn video_line(&mut self, _frame: u32, _y: u32, _w: u32, _h: u32) -> Option<Vec<u8>> {
        None
    }
}

/// A change of the PL's board-facing output pins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardOutput {
    /// PS time of the change in nanoseconds.
    pub t_ns: u64,
    /// The user LEDs (at most eight), bit *n* = LED *n*.
    pub leds: u8,
}

/// What the PL can do to the PS during a callback.
pub trait PsPort {
    /// Reads `buf.len()` bytes of PS physical memory through `port`.
    /// Zero simulated time passes; the modelled latency is the caller's.
    fn mem_read(&mut self, port: PsMasterPort, addr: u64, buf: &mut [u8]) -> BusResult<()>;

    /// Writes `data` to PS physical memory through `port`.
    fn mem_write(&mut self, port: PsMasterPort, addr: u64, data: &[u8]) -> BusResult<()>;

    /// Drives `IRQ_F2P[line]` to `level` at PS time `now_ns`.
    fn irq(&mut self, line: u32, level: bool, now_ns: u64) -> BusResult<()>;
}

/// The PL as seen from the PS.
pub trait PlTarget {
    /// A read on `M_AXI_GP<gp>` at physical address `addr`, issued at PS time
    /// `now_ns`. Fills `buf`.
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()>;

    /// A write on `M_AXI_GP<gp>`.
    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()>;

    /// A PS-to-PL wire changed (for example `FCLK_RESET0_N`).
    fn wire(&mut self, _line: u32, _level: bool, _now_ns: u64) {}

    /// PS time has advanced to `now_ns`; the PL may catch up (lock-step).
    fn advance(&mut self, _now_ns: u64, _ps: &mut dyn PsPort) {}

    /// Removes and returns the PL-initiated memory accesses seen since the
    /// last call. Targets without PL masters return nothing.
    fn drain_master_log(&mut self) -> Vec<MasterAccess> {
        Vec::new()
    }

    /// True when this PL can receive board input pins and report board
    /// outputs (switches, buttons, LEDs).
    fn has_board_pins(&self) -> bool {
        false
    }

    /// Drives the board input pins: `switches` and `buttons` carry one bit per component
    /// of the board (bit n = component n, at most 8).
    /// Applied at PS time `now_ns`.
    fn set_board_inputs(&mut self, _switches: u8, _buttons: u8, _now_ns: u64) {}

    /// Removes and returns the board output changes seen since the last call.
    fn drain_board_outputs(&mut self) -> Vec<BoardOutput> {
        Vec::new()
    }

    /// True when the PL design reports Pmod/RGB pin groups.
    fn has_pin_groups(&self) -> bool {
        false
    }

    /// Removes and returns the pin-group changes seen since the last call.
    fn drain_pin_events(&mut self) -> Vec<PinEvent> {
        Vec::new()
    }

    /// Drives the input levels of a pin group (for example a Pmod button
    /// module) at PS time `now_ns`.
    fn set_pin_inputs(&mut self, _group: u8, _levels: u8, _now_ns: u64) {}

    /// Installs the board's response to pin changes: it is called for every
    /// pin-group event, and its answer is what a synchronous exchange
    /// returns to the testbench.
    fn set_pin_sink(&mut self, _sink: Box<dyn PinSink>) {}

    /// True when this PL carries the board's audio codec and HDMI models.
    fn has_media(&self) -> bool {
        false
    }

    /// Removes and returns the media events (audio frames, video lines, codec
    /// register writes) the PL's models produced since the last call.
    fn drain_media(&mut self) -> Vec<MediaEvent> {
        Vec::new()
    }

    /// Sets where the PL's HDMI RX and codec ADC get their data. Either may
    /// be left unchanged with `None`.
    fn set_media_sources(&mut self, _video: Option<VideoSource>, _audio: Option<AudioSource>) {}

    /// Instance names of the PL's host-visible UARTs (AXI UART Lite), in
    /// channel order.
    fn uart_channels(&self) -> Vec<String> {
        Vec::new()
    }

    /// Removes and returns the bytes the PL's UARTs transmitted since the
    /// last call.
    fn drain_uart_tx(&mut self) -> Vec<UartByte> {
        Vec::new()
    }

    /// The host sends `data` to the receiver of UART `channel`, starting at
    /// PS time `now_ns`. Returns the number of bytes accepted (the model's
    /// receive queue is bounded).
    fn uart_rx(&mut self, _channel: u8, _data: &[u8], _now_ns: u64) -> usize {
        0
    }

    /// True once the PL has ended the simulation (for example a testbench
    /// `$finish`). Accesses after that fail.
    fn finished(&self) -> bool {
        false
    }
}

/// A PL with nothing in it: every access is a decode error. Useful as the PS
/// engine's default and in tests.
pub struct EmptyPl;

impl PlTarget for EmptyPl {
    fn read(&mut self, _: u8, _: u64, _: &mut [u8], _: u64, _: &mut dyn PsPort) -> BusResult<()> {
        Err(BusError::Decode)
    }
    fn write(&mut self, _: u8, _: u64, _: &[u8], _: u64, _: &mut dyn PsPort) -> BusResult<()> {
        Err(BusError::Decode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_numbers_follow_the_cosim_tree() {
        assert_eq!(PsMasterPort::SAxiGp0.device(), 0);
        assert_eq!(PsMasterPort::SAxiHp0.device(), 2);
        assert_eq!(PsMasterPort::SAxiHp3.device(), 5);
        assert_eq!(PsMasterPort::SAxiAcp.device(), 6);
        assert_eq!((DEV_M_AXI_GP0, DEV_M_AXI_GP1), (7, 8));
    }
}
