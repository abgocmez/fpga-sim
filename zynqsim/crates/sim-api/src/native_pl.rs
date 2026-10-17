//! The programmable logic of the native PS (ADR-035).
//!
//! The PL sits on the two PS master ports the system bus maps: `M_AXI_GP0` at
//! `0x4000_0000` and `M_AXI_GP1` at `0x8000_0000`, each 1 GiB. An access in a window goes to
//! the PL target ([`pl_api::PlTarget`], the same one the QEMU engine attaches); the PL's
//! interrupt outputs `IRQ_F2P[0..=15]` drive GIC SPIs 61 to 68 and 84 to 91, as on the Zynq.
//!
//! PL time is PS time: the bus ticks the PL with the CPU cycles, and a PL access is stamped with
//! the cycle of the instruction that made it. The PL sees the same instruction times in every
//! run, so the trace digest is reproducible.

use crate::bytelog::ByteLog;
use crate::error::ApiError;
use crate::native_ps::PS_PER_CYCLE;
use crate::pl_attach::{
    BusEvent, IrqLines, TracedPl, trace_leds, trace_master_accesses, trace_pl_uart,
    trace_pmod_pins, trace_ps_accesses,
};
use board_zybo::{BoardDef, BoardState};
use mem_sys::{BusFault, BusResult, Device, IrqInput, SystemBus};
use pl_api::{BusError, PlTarget};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use trace::TraceLog;

/// `M_AXI_GP0` base address and window size (1 GiB).
pub const GP0_BASE: u64 = 0x4000_0000;
/// `M_AXI_GP1` base address; its window is 1 GiB as well.
pub const GP1_BASE: u64 = 0x8000_0000;
/// Size of each GP window.
pub const GP_SIZE: u64 = 0x4000_0000;

/// The GIC interrupt ID of `IRQ_F2P[line]`: lines 0 to 7 are SPIs 61 to 68, lines 8 to 15 are
/// SPIs 84 to 91.
pub fn irq_spi(line: usize) -> u16 {
    if line < 8 {
        61 + line as u16
    } else {
        84 + (line as u16 - 8)
    }
}

/// The PL of the native PS: the target, its clock and the interrupt lines it drives.
pub struct PlState {
    pl: TracedPl,
    /// PS time (ns) the PL has reached: the last tick, or the cycle of the last access.
    now_ns: u64,
    /// CPU cycles the bus has ticked the PL with.
    cycles: u64,
    /// Levels of `IRQ_F2P[0..=15]` as the PL drives them.
    irq: [bool; 16],
}

/// The PL shared by the bus windows and the engine.
pub type PlHandle = Arc<Mutex<PlState>>;

/// Wraps `pl` for the native PS. Pin groups are given an empty set of Pmod devices, as the QEMU
/// engine gives its PL when no device is attached.
pub fn new_handle(mut pl: Box<dyn PlTarget + Send>) -> PlHandle {
    pl.set_pin_sink(Box::new(crate::pl_attach::BankSink(Arc::new(Mutex::new(
        devices::Bank::new(),
    )))));
    Arc::new(Mutex::new(PlState {
        pl: TracedPl::new(pl),
        now_ns: 0,
        cycles: 0,
        irq: [false; 16],
    }))
}

/// Locks the PL. A poisoned lock is taken anyway: the state is still a valid PL state.
pub fn lock(pl: &PlHandle) -> MutexGuard<'_, PlState> {
    pl.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PlState {
    /// Moves the PL's time forward to the CPU cycle `cycle` (never backwards).
    pub(crate) fn at_cycle(&mut self, cycle: u64) {
        self.now_ns = self.now_ns.max(cycle * PS_PER_CYCLE / 1000);
    }

    /// The bus ticked the PL with `cycles` CPU cycles: the PL catches up to the new time.
    fn tick(&mut self, cycles: u64) {
        self.cycles += cycles;
        self.at_cycle(self.cycles);
        let PlState {
            pl, now_ns, irq, ..
        } = self;
        pl.advance(*now_ns, &mut IrqLines { levels: irq });
    }

    /// The PS accesses the PL served and the events it reported, since the last call: the
    /// engine records them in its trace (`M_AXI_GP<n>`, Pmod pins, LEDs, UART Lite bytes).
    pub(crate) fn events_since_last_call(&mut self) -> Vec<BusEvent> {
        std::mem::take(&mut self.pl.events)
    }

    /// The target, for the engine's host inputs (board pins, UART Lite receive).
    pub(crate) fn target(&mut self) -> &mut TracedPl {
        &mut self.pl
    }
}

/// A PL window on the bus. The window with `lead` set ticks the PL and reports its interrupt
/// lines; the other only serves accesses (the bus gathers interrupt lines from every device, so
/// they are reported once).
pub struct GpWindow {
    gp: u8,
    base: u64,
    pl: PlHandle,
    lead: bool,
}

/// Maps a PL decode or slave error onto the bus fault of the window access.
fn fault(e: BusError, offset: u64) -> BusFault {
    match e {
        BusError::Decode => BusFault::Unmapped(offset),
        BusError::Slave => BusFault::BadAccess(offset),
    }
}

impl Device for GpWindow {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let addr = self.base + offset;
        let mut buf = [0u8; 8];
        let n = usize::from(size);
        let mut st = lock(&self.pl);
        let PlState {
            pl, now_ns, irq, ..
        } = &mut *st;
        pl.read(
            self.gp,
            addr,
            &mut buf[..n],
            *now_ns,
            &mut IrqLines { levels: irq },
        )
        .map_err(|e| fault(e, offset))?;
        Ok(u64::from_le_bytes(buf))
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let addr = self.base + offset;
        let bytes = value.to_le_bytes();
        let n = usize::from(size);
        let mut st = lock(&self.pl);
        let PlState {
            pl, now_ns, irq, ..
        } = &mut *st;
        pl.write(
            self.gp,
            addr,
            &bytes[..n],
            *now_ns,
            &mut IrqLines { levels: irq },
        )
        .map_err(|e| fault(e, offset))
    }

    fn tick(&mut self, cycles: u64) {
        if self.lead {
            lock(&self.pl).tick(cycles);
        }
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        if self.lead {
            let st = lock(&self.pl);
            for (line, level) in st.irq.iter().enumerate() {
                out.push(IrqInput {
                    id: irq_spi(line),
                    cpu: None,
                    level: *level,
                });
            }
        }
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        if self.lead {
            let st = lock(&self.pl);
            out.extend_from_slice(&st.now_ns.to_le_bytes());
            out.extend_from_slice(&st.cycles.to_le_bytes());
            let bits = st
                .irq
                .iter()
                .enumerate()
                .fold(0u16, |a, (i, &l)| a | (u16::from(l) << i));
            out.extend_from_slice(&bits.to_le_bytes());
        }
    }
}

/// Maps the two GP windows of the bus to the PL `pl`.
pub fn attach(bus: &mut SystemBus, pl: &PlHandle) -> Result<(), ApiError> {
    for (gp, base, name, lead) in [
        (0u8, GP0_BASE, "PL M_AXI_GP0", true),
        (1u8, GP1_BASE, "PL M_AXI_GP1", false),
    ] {
        bus.attach(
            name,
            base,
            GP_SIZE,
            Box::new(GpWindow {
                gp,
                base,
                pl: Arc::clone(pl),
                lead,
            }),
        )
        .map_err(|e| ApiError::internal(format!("the PL window {name}: {e}")))?;
    }
    Ok(())
}

/// Whether `addr` is in a GP window (the PL's address space).
pub fn in_gp_windows(addr: u64) -> bool {
    (GP0_BASE..GP1_BASE + GP_SIZE).contains(&addr)
}

/// Records what the PL reported since the last call, in the order the QEMU engine records it:
/// PS accesses, Pmod pin changes, LEDs, PL-initiated accesses and UART Lite bytes.
pub fn flush(
    pl: &PlHandle,
    trace: &mut TraceLog,
    board: &mut BoardState,
    def: &BoardDef,
    uarts: &mut Vec<ByteLog>,
) {
    let mut st = lock(pl);
    trace_ps_accesses(st.events_since_last_call(), trace);
    let pins = st.pl.drain_pin_events();
    for ev in &pins {
        // Groups that are not Pmod ports (the RGB LEDs, the camera) have no board row here.
        trace_pmod_pins(ev, board, def, trace);
    }
    trace_leds(st.pl.drain_board_outputs(), board, def, trace);
    trace_master_accesses(st.pl.drain_master_log(), trace);
    trace_pl_uart(st.pl.drain_uart_tx(), uarts, trace);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem_sys::Bus;
    use pl_native::{MathIp, NativePl};

    #[test]
    fn the_gic_numbers_follow_the_zynq_map() {
        assert_eq!(irq_spi(0), 61);
        assert_eq!(irq_spi(7), 68);
        assert_eq!(irq_spi(8), 84);
        assert_eq!(irq_spi(15), 91);
    }

    #[test]
    fn the_windows_are_contiguous_and_cover_both_gp_ports() {
        assert_eq!(GP0_BASE + GP_SIZE, GP1_BASE);
        assert!(in_gp_windows(0x4000_0000) && in_gp_windows(0xBFFF_FFFF));
        assert!(!in_gp_windows(0x3FFF_FFFF) && !in_gp_windows(0xC000_0000));
    }

    #[test]
    fn a_native_model_answers_on_gp0_through_the_bus_and_is_traced() {
        let mut pl = NativePl::new();
        pl.map(0, 0x43C0_0000, Box::new(MathIp::new()));
        let handle = new_handle(Box::new(pl));
        let mut bus = SystemBus::new(0x2000_0000).unwrap();
        attach(&mut bus, &handle).unwrap();
        bus.write(0x43C0_0000, 4, 21).unwrap();
        assert_eq!(bus.read(0x43C0_0004, 4).unwrap(), 42);
        let ev = lock(&handle).events_since_last_call();
        assert_eq!(ev.len(), 2);
        assert!(ev.iter().all(|e| e.port == 0 && e.ok));
        // An address in the window that no PL block decodes is a PL decode error, and an
        // address outside both windows and DDR is unmapped on the bus.
        assert!(bus.read(0x4000_0000, 4).is_err());
        assert!(bus.read(0x3000_0000, 4).is_err());
        let ev = lock(&handle).events_since_last_call();
        assert_eq!(ev.len(), 1, "only the GP0 access reached the PL");
        assert!(!ev[0].ok);
    }

    #[test]
    fn my_math_ip_on_the_rtl_engine_answers_on_the_ps_bus() {
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/rtl/math/my_math_ip.v");
        let spec = crate::RtlNativeSpec {
            top: "my_math_ip".into(),
            sources: vec![src],
            defines: Vec::new(),
            gp: 0,
            base: 0x43C0_0000,
            irqs: Vec::new(),
        };
        let target = crate::pl_attach::rtl_native_target(&spec).expect("elaborates");
        let handle = new_handle(target);
        let mut bus = SystemBus::new(0x2000_0000).unwrap();
        attach(&mut bus, &handle).unwrap();
        bus.write(0x43C0_0000, 4, 21).unwrap();
        assert_eq!(bus.read(0x43C0_0004, 4).unwrap(), 42, "reg1 = reg0 * 2");
        assert_eq!(bus.read(0x43C0_0000, 4).unwrap(), 21);
        let ev = lock(&handle).events_since_last_call();
        assert_eq!(ev.len(), 3);
    }

    #[test]
    fn the_pl_clock_follows_the_cpu_cycles_the_bus_ticks() {
        let handle = new_handle(Box::new(NativePl::new()));
        let mut bus = SystemBus::new(0x2000_0000).unwrap();
        attach(&mut bus, &handle).unwrap();
        bus.tick(1000);
        assert_eq!(lock(&handle).now_ns, 1500);
        // An access earlier than the bus clock does not move the PL backwards.
        lock(&handle).at_cycle(10);
        assert_eq!(lock(&handle).now_ns, 1500);
    }
}
