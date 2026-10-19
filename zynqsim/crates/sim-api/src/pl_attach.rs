//! The programmable logic both PS engines attach (ADR-035).
//!
//! The Xilinx QEMU engine and the native PS place the same PL on the same `M_AXI_GP` windows:
//! native register-level models (`[native]`, the `vendor-ip` library and `pl-native`), an RTL
//! design run by the native RTL engine (`backends.pl = "rtl-native"`, ADR-034), or both. This
//! module builds that PL, wraps it so that every PS access is recorded, and turns what the PL
//! reports (bus accesses, LEDs, Pmod pins, UART Lite bytes) into trace records of the same kinds
//! on both engines.

use crate::bytelog::ByteLog;
use crate::error::ApiError;
use crate::tbgen;
use board_zybo::{BoardDef, BoardState};
use pl_api::{
    BoardOutput, BusError, BusResult, MasterAccess, MediaEvent, PinEvent, PinSink, PlTarget,
    PsMasterPort, PsPort, UartByte,
};
use pl_native::{MathIp, NativePl, RegisterBlock};
use rtl_engine::{Design, RtlModule};
use sim_core::Ps;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use trace::{TraceEvent, TraceLog};
use vendor_ip::{VendorInstance, VendorPl};

/// Names accepted for `[native]` models.
pub const NATIVE_MODELS: &[&str] = &[
    "my_math_ip",
    "axi_gpio",
    "axi_timer",
    "axi_uartlite",
    "axi_dma",
    "axi_iic",
    "axi_bram_ctrl",
    "axi_intc",
    "axi_quad_spi",
    "axi_fifo_mm_s",
];

/// A native PL model to place on a GP port.
#[derive(Clone, Debug)]
pub struct NativeInstance {
    /// Instance name from the project, for diagnostics.
    pub name: String,
    /// Model name (`my_math_ip`).
    pub model: String,
    /// `M_AXI_GP` port, 0 or 1.
    pub gp: u8,
    /// Base address.
    pub base: u64,
    /// Size of the address window; `None` uses the model's default (64 KiB).
    pub size: Option<u64>,
    /// `IRQ_F2P` bit the instance's interrupt output drives.
    pub irq: Option<u32>,
    /// Interrupt outputs by port name with their destination (`IRQ_F2P[n]`, or an interrupt
    /// controller input `axi_intc_0.intr[k]`); when not empty it replaces `irq`.
    pub irqs: Vec<(String, String)>,
    /// IP parameters by Vivado name (`C_GPIO_WIDTH`), plus `CLK_FREQ_HZ`.
    pub params: std::collections::BTreeMap<String, String>,
    /// Board bindings: GPIO interface (`GPIO`, `GPIO2`) to board signal (`board.leds`); the AXI
    /// IIC's `IIC` to a Pmod port.
    pub bindings: std::collections::BTreeMap<String, String>,
}

/// One slave behind an AXI4-Lite interface for native RTL simulation.
#[derive(Clone, Debug)]
pub struct RtlNativeSlaveSpec {
    /// Instance name in the project/block design (e.g. `u_math0`).
    pub instance: String,
    /// Top module name for this slave (e.g. `my_math_ip`).
    pub top: String,
    /// `M_AXI_GP` port, 0 or 1.
    pub gp: u8,
    /// Base address.
    pub base: u64,
    /// Interrupt outputs by port name and destination (`IRQ_F2P[n]`).
    pub irqs: Vec<(String, String)>,
}

/// An RTL design behind an AXI4-Lite slave on one GP port (`backends.pl = "rtl-native"`).
#[derive(Clone, Debug)]
pub struct RtlNativeSpec {
    /// Top module; the `[connect]` entries of the design are named after it (`<top>.S_AXI`).
    pub top: String,
    /// Source files, resolved.
    pub sources: Vec<PathBuf>,
    /// Preprocessor defines.
    pub defines: Vec<String>,
    /// `M_AXI_GP` port the slave is on.
    pub gp: u8,
    /// Base address of the slave.
    pub base: u64,
    /// Interrupt outputs by port name and destination (`IRQ_F2P[n]`), from `[connect]`. With
    /// none, the first interrupt output found by name (as the testbench generator finds them)
    /// drives `IRQ_F2P[0]`.
    pub irqs: Vec<(String, String)>,
    /// Optional list of multiple slaves for multi-slave designs. When not empty, each slave is
    /// elaborated and placed in NativePl.
    pub slaves: Vec<RtlNativeSlaveSpec>,
}

/// Builds the PL from its native models and, optionally, an RTL target that serves every
/// address the models do not decode. With neither, the PL is empty (every access is a decode
/// error).
pub fn build_pl(
    natives: &[NativeInstance],
    rtl: Option<Box<dyn PlTarget + Send>>,
) -> Result<Box<dyn PlTarget + Send>, ApiError> {
    if natives.is_empty() {
        return Ok(rtl.unwrap_or_else(|| Box::new(NativePl::new())));
    }
    // Native models (the prototype's math IP, the vendor-ip library), next to the RTL design
    // when there is one.
    let mut vp = VendorPl::new(rtl);
    let mut vendor: Vec<VendorInstance> = Vec::new();
    for n in natives {
        if n.gp > 1 {
            return Err(ApiError::invalid_argument(format!(
                "instance `{}`: M_AXI_GP{} does not exist",
                n.name, n.gp
            )));
        }
        if vendor_ip::is_model(&n.model) {
            let inst = VendorInstance {
                name: n.name.clone(),
                model: n.model.clone(),
                gp: n.gp,
                base: n.base,
                size: n.size,
                irq: n.irq,
                irqs: n.irqs.clone(),
                params: n.params.clone(),
                bindings: n.bindings.clone(),
            };
            vp.add(&inst).map_err(ApiError::invalid_argument)?;
            vendor.push(inst);
        } else {
            let block = model(&n.model).ok_or_else(|| {
                ApiError::invalid_argument(format!(
                    "unknown native model `{}` for instance `{}`; available: {}",
                    n.model,
                    n.name,
                    NATIVE_MODELS.join(", ")
                ))
            })?;
            vp.add_block(&n.name, n.gp, n.base, block)
                .map_err(ApiError::invalid_argument)?;
        }
    }
    vp.check_routes(&vendor)
        .map_err(ApiError::invalid_argument)?;
    Ok(Box::new(vp))
}

/// Builds a native model by name.
fn model(name: &str) -> Option<Box<dyn RegisterBlock>> {
    match name {
        "my_math_ip" => Some(Box::new(MathIp::new())),
        _ => None,
    }
}

/// The interrupt-output names of `top` that look like interrupts, found the way the testbench
/// generator finds them (`tbgen::detect_irqs`): a 1-bit output whose name says `irq`,
/// `interrupt` or `intr`. Empty when the module is not in `sources` or has no AXI4-Lite slave.
pub fn detect_irq_ports(sources: &[PathBuf], top: &str) -> Vec<String> {
    for f in sources {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let Ok(m) = tbgen::parse_module(&text, Some(top)) else {
            continue;
        };
        return match tbgen::detect_axi_lite(&m) {
            Ok(axi) => tbgen::detect_irqs(&m, &axi),
            Err(_) => Vec::new(),
        };
    }
    Vec::new()
}

/// Builds the RTL design of `spec` on the native RTL engine: elaboration, simulation, and the
/// AXI4-Lite slave as a register block on its GP port, its interrupt output on `IRQ_F2P`.
pub fn rtl_native_target(spec: &RtlNativeSpec) -> Result<Box<dyn PlTarget + Send>, ApiError> {
    if !spec.slaves.is_empty() {
        let mut pl = NativePl::new();
        for slave in &spec.slaves {
            let design = Design::elaborate(&slave.top, &spec.sources, &spec.defines, &[]);
            let mut module = RtlModule::new(design).map_err(|e| {
                ApiError::invalid_argument(format!(
                    "backends.pl = \"rtl-native\": `{}` (instance `{}`): {e}",
                    slave.top, slave.instance
                ))
            })?;
            let mut irqs = slave.irqs.clone();
            if irqs.is_empty() {
                if let Some(p) = detect_irq_ports(&spec.sources, &slave.top).first() {
                    irqs.push((p.clone(), "IRQ_F2P[0]".to_string()));
                }
            }
            let bit = match irqs.as_slice() {
                [] => None,
                [(port, dest)] => {
                    let n = match vendor_ip::parse_irq_dest(dest) {
                        Some(pl_native::IrqDest::F2p(n)) if n <= 15 => n,
                        _ => {
                            return Err(ApiError::invalid_argument(format!(
                                "`{}.{port}` = `{dest}`: the RTL interrupt output goes to IRQ_F2P[0..15]",
                                slave.instance
                            )));
                        }
                    };
                    module.set_irq_output(port).map_err(|e| {
                        ApiError::invalid_argument(format!("`{}.{port}`: {e}", slave.instance))
                    })?;
                    Some(n)
                }
                _ => {
                    return Err(ApiError::invalid_argument(format!(
                        "`{}`: backends.pl = \"rtl-native\" supports one interrupt output; {} are connected",
                        slave.instance,
                        irqs.len()
                    )));
                }
            };
            pl.map_irq(slave.gp, slave.base, Box::new(module), bit);
        }
        return Ok(Box::new(pl));
    }

    let design = Design::elaborate(&spec.top, &spec.sources, &spec.defines, &[]);
    let mut module = RtlModule::new(design).map_err(|e| {
        ApiError::invalid_argument(format!("backends.pl = \"rtl-native\": `{}`: {e}", spec.top))
    })?;
    let mut irqs = spec.irqs.clone();
    if irqs.is_empty() {
        if let Some(p) = detect_irq_ports(&spec.sources, &spec.top).first() {
            irqs.push((p.clone(), "IRQ_F2P[0]".to_string()));
        }
    }
    let bit = match irqs.as_slice() {
        [] => None,
        [(port, dest)] => {
            let n = match vendor_ip::parse_irq_dest(dest) {
                Some(pl_native::IrqDest::F2p(n)) if n <= 15 => n,
                _ => {
                    return Err(ApiError::invalid_argument(format!(
                        "`{}.{port}` = `{dest}`: the RTL interrupt output goes to IRQ_F2P[0..15]",
                        spec.top
                    )));
                }
            };
            module
                .set_irq_output(port)
                .map_err(|e| ApiError::invalid_argument(format!("`{}.{port}`: {e}", spec.top)))?;
            Some(n)
        }
        _ => {
            return Err(ApiError::invalid_argument(format!(
                "`{}`: backends.pl = \"rtl-native\" supports one interrupt output; \
                 {} are connected",
                spec.top,
                irqs.len()
            )));
        }
    };
    let mut pl = NativePl::new();
    pl.map_irq(spec.gp, spec.base, Box::new(module), bit);
    Ok(Box::new(pl))
}

/// A PS access the PL served, as the engines record it (`M_AXI_GP<port>` in the trace).
pub(crate) struct BusEvent {
    pub t_ns: u64,
    pub port: u8,
    pub write: bool,
    pub addr: u64,
    pub len: u32,
    pub data: u64,
    pub ok: bool,
}

/// Wraps a PL target and remembers every PS access for the trace.
pub(crate) struct TracedPl {
    pub inner: Box<dyn PlTarget + Send>,
    pub events: Vec<BusEvent>,
}

impl TracedPl {
    pub fn new(inner: Box<dyn PlTarget + Send>) -> TracedPl {
        TracedPl {
            inner,
            events: Vec::new(),
        }
    }
}

fn first8(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    let n = b.len().min(8);
    a[..n].copy_from_slice(&b[..n]);
    u64::from_le_bytes(a)
}

impl PlTarget for TracedPl {
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        let r = self.inner.read(gp, addr, buf, now_ns, ps);
        self.events.push(BusEvent {
            t_ns: now_ns,
            port: gp,
            write: false,
            addr,
            len: buf.len() as u32,
            data: first8(buf),
            ok: r.is_ok(),
        });
        r
    }

    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        let r = self.inner.write(gp, addr, data, now_ns, ps);
        self.events.push(BusEvent {
            t_ns: now_ns,
            port: gp,
            write: true,
            addr,
            len: data.len() as u32,
            data: first8(data),
            ok: r.is_ok(),
        });
        r
    }

    fn wire(&mut self, line: u32, level: bool, now_ns: u64) {
        self.inner.wire(line, level, now_ns);
    }

    fn advance(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        self.inner.advance(now_ns, ps);
    }

    fn drain_master_log(&mut self) -> Vec<MasterAccess> {
        self.inner.drain_master_log()
    }

    fn drain_media(&mut self) -> Vec<MediaEvent> {
        self.inner.drain_media()
    }

    fn set_media_sources(
        &mut self,
        video: Option<pl_api::VideoSource>,
        audio: Option<pl_api::AudioSource>,
    ) {
        self.inner.set_media_sources(video, audio);
    }

    fn has_media(&self) -> bool {
        self.inner.has_media()
    }

    fn has_board_pins(&self) -> bool {
        self.inner.has_board_pins()
    }

    fn set_board_inputs(&mut self, switches: u8, buttons: u8, now_ns: u64) {
        self.inner.set_board_inputs(switches, buttons, now_ns);
    }

    fn drain_board_outputs(&mut self) -> Vec<BoardOutput> {
        self.inner.drain_board_outputs()
    }

    fn drain_protocol_findings(&mut self) -> Vec<pl_api::ProtocolFinding> {
        self.inner.drain_protocol_findings()
    }

    fn has_pin_groups(&self) -> bool {
        self.inner.has_pin_groups()
    }

    fn drain_pin_events(&mut self) -> Vec<PinEvent> {
        self.inner.drain_pin_events()
    }

    fn set_pin_inputs(&mut self, group: u8, levels: u8, now_ns: u64) {
        self.inner.set_pin_inputs(group, levels, now_ns);
    }

    fn set_pin_sink(&mut self, sink: Box<dyn PinSink>) {
        self.inner.set_pin_sink(sink);
    }

    fn uart_channels(&self) -> Vec<String> {
        self.inner.uart_channels()
    }

    fn drain_uart_tx(&mut self) -> Vec<UartByte> {
        self.inner.drain_uart_tx()
    }

    fn uart_rx(&mut self, channel: u8, data: &[u8], now_ns: u64) -> usize {
        self.inner.uart_rx(channel, data, now_ns)
    }

    fn finished(&self) -> bool {
        self.inner.finished()
    }
}

/// Gives the PL's pin groups access to the devices on the Pmod ports.
pub(crate) struct BankSink(pub Arc<Mutex<devices::Bank>>);

impl PinSink for BankSink {
    fn pin_change(&mut self, group: u8, t_ns: u64, levels: u8) -> u8 {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pins(group, t_ns, levels)
    }

    fn video_line(&mut self, frame: u32, y: u32, w: u32, h: u32) -> Option<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .video_line(frame, y, w, h)
    }
}

pub(crate) fn master_port_name(p: PsMasterPort) -> &'static str {
    match p {
        PsMasterPort::SAxiGp0 => "S_AXI_GP0",
        PsMasterPort::SAxiGp1 => "S_AXI_GP1",
        PsMasterPort::SAxiHp0 => "S_AXI_HP0",
        PsMasterPort::SAxiHp1 => "S_AXI_HP1",
        PsMasterPort::SAxiHp2 => "S_AXI_HP2",
        PsMasterPort::SAxiHp3 => "S_AXI_HP3",
        PsMasterPort::SAxiAcp => "S_AXI_ACP",
    }
}

/// Records the PS accesses the PL served (`M_AXI_GP<n>`), stamped with their PS time.
pub(crate) fn trace_ps_accesses(events: Vec<BusEvent>, trace: &mut TraceLog) {
    for e in events {
        trace.push(
            Ps(e.t_ns.saturating_mul(1000)),
            TraceEvent::Bus {
                port: format!("M_AXI_GP{}", e.port),
                write: e.write,
                addr: e.addr,
                len: e.len,
                data: e.data,
                ok: e.ok,
            },
        );
    }
}

/// Records the accesses the PL initiated itself (a DMA master), as `S_AXI_*` ports.
pub(crate) fn trace_master_accesses(accesses: Vec<MasterAccess>, trace: &mut TraceLog) {
    for m in accesses {
        trace.push(
            Ps(m.t_ns.saturating_mul(1000)),
            TraceEvent::Bus {
                port: master_port_name(m.port).to_string(),
                write: m.write,
                addr: m.addr,
                len: m.len,
                data: m.data,
                ok: m.ok,
            },
        );
    }
}

/// Shows the PL's LED outputs on the board: a `board.<led>` signal record per change.
pub(crate) fn trace_leds(
    outs: Vec<BoardOutput>,
    board: &mut BoardState,
    def: &BoardDef,
    trace: &mut TraceLog,
) {
    for o in outs {
        for i in 0..board.leds.len().min(board_zybo::MAX_BITS) {
            let duty = if o.leds & (1 << i) != 0 { 255 } else { 0 };
            if board.leds[i] != duty {
                board.leds[i] = duty;
                trace.push(
                    Ps(o.t_ns.saturating_mul(1000)),
                    TraceEvent::Signal {
                        name: format!("board.{}", def.leds[i].id),
                        value: u64::from(duty),
                        width: 8,
                    },
                );
            }
        }
    }
}

/// Applies a change of a Pmod pin group the PL drives: the board's pin levels and a
/// `pmod.<port>` signal record. Returns false when the group is not a Pmod port.
pub(crate) fn trace_pmod_pins(
    ev: &PinEvent,
    board: &mut BoardState,
    def: &BoardDef,
    trace: &mut TraceLog,
) -> bool {
    let Some(row) = board.pmod_pins.get_mut(usize::from(ev.group)) else {
        return false;
    };
    for (i, p) in row.iter_mut().enumerate() {
        *p = if ev.levels >> i & 1 != 0 {
            board_zybo::PinLevel::High
        } else {
            board_zybo::PinLevel::Low
        };
    }
    let name = format!(
        "pmod.{}",
        def.pmods
            .get(usize::from(ev.group))
            .map_or_else(|| ev.group.to_string(), |p| p.id.to_lowercase())
    );
    trace.push(
        Ps(ev.t_ns.saturating_mul(1000)),
        TraceEvent::Signal {
            name,
            value: u64::from(ev.levels),
            width: 8,
        },
    );
    true
}

/// Moves the bytes of the PL's UART Lite instances into their logs (`uart_read` port `2 + n`)
/// and the trace (`uart<2 + n>` records), stamped with the time each character finished.
pub(crate) fn trace_pl_uart(bytes: Vec<UartByte>, logs: &mut Vec<ByteLog>, trace: &mut TraceLog) {
    for b in bytes {
        let ch = usize::from(b.channel);
        if logs.len() <= ch {
            logs.resize_with(ch + 1, ByteLog::default);
        }
        logs[ch].push(b.byte);
        trace.push(
            Ps(b.t_ns.saturating_mul(1000)),
            TraceEvent::Uart {
                port: 2 + b.channel,
                tx: true,
                byte: b.byte,
            },
        );
    }
}

/// The host sends `data` to PL UART Lite `port` (2 or more) at the current time. Refused when
/// the PL has no such instance or the receive queue is full (what fits is still delivered).
pub(crate) fn pl_uart_write(
    pl: &mut dyn PlTarget,
    port: u8,
    data: &[u8],
    now_ns: u64,
    now: Ps,
    trace: &mut TraceLog,
) -> Result<(), ApiError> {
    let ch = port - 2;
    if usize::from(ch) >= pl.uart_channels().len() {
        return Err(ApiError::invalid_argument(format!(
            "no uart {port}: the PL has {} UART Lite instance(s) (ports 2..)",
            pl.uart_channels().len()
        )));
    }
    let n = pl.uart_rx(ch, data, now_ns);
    trace.push(
        now,
        TraceEvent::Input {
            what: format!("uart{port} write {n} byte(s)"),
        },
    );
    if n < data.len() {
        return Err(ApiError::invalid_argument(format!(
            "uart{port}: only {n} of {} byte(s) fit the receive queue",
            data.len()
        )));
    }
    Ok(())
}

/// The two kinds of board push button and slide switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoardPin {
    Switch,
    Button,
}

/// Sets a board switch or button, traces it (`Input` and `board.<id>` signal) and returns
/// nothing else: the caller hands the new levels to the PL with [`push_board_inputs`].
pub(crate) fn set_board_pin(
    board: &mut BoardState,
    def: &BoardDef,
    trace: &mut TraceLog,
    now: Ps,
    pin: BoardPin,
    index: usize,
    on: bool,
) -> Result<(), ApiError> {
    let (what, len) = match pin {
        BoardPin::Switch => ("switch", board.switches.len()),
        BoardPin::Button => ("button", board.buttons.len()),
    };
    if index >= len {
        return Err(ApiError::invalid_argument(format!(
            "{what} index {index} out of range 0..{len}"
        )));
    }
    let id = match pin {
        BoardPin::Switch => {
            board.switches[index] = on;
            def.switches[index].id.clone()
        }
        BoardPin::Button => {
            board.buttons[index] = on;
            def.buttons[index].id.clone()
        }
    };
    let text = match pin {
        BoardPin::Switch => format!("switch {index} = {}", if on { "on" } else { "off" }),
        BoardPin::Button => format!(
            "button {index} = {}",
            if on { "pressed" } else { "released" }
        ),
    };
    trace.push(now, TraceEvent::Input { what: text });
    trace.push(
        now,
        TraceEvent::Signal {
            name: format!("board.{id}"),
            value: u64::from(on),
            width: 1,
        },
    );
    Ok(())
}

/// Hands the board's switch and button levels to the PL (bit n = component n, at most 8).
pub(crate) fn push_board_inputs(pl: &mut dyn PlTarget, board: &BoardState, now_ns: u64) {
    let bits = |v: &[bool]| {
        // Bit n = component n of the board; a board file has at most 8 (ADR-022).
        v.iter()
            .take(board_zybo::MAX_BITS)
            .enumerate()
            .fold(0u8, |a, (i, &b)| a | (u8::from(b) << i))
    };
    pl.set_board_inputs(bits(&board.switches), bits(&board.buttons), now_ns);
}

/// A PL port with no PS memory behind it: the PL's DMA masters cannot reach memory through it
/// (`mem_*` are decode errors); interrupts are recorded for the engine.
pub(crate) struct IrqLines<'a> {
    pub levels: &'a mut [bool; 16],
}

impl PsPort for IrqLines<'_> {
    fn mem_read(&mut self, _: PsMasterPort, _: u64, _: &mut [u8]) -> BusResult<()> {
        Err(BusError::Decode)
    }

    fn mem_write(&mut self, _: PsMasterPort, _: u64, _: &[u8]) -> BusResult<()> {
        Err(BusError::Decode)
    }

    fn irq(&mut self, line: u32, level: bool, _now_ns: u64) -> BusResult<()> {
        match self.levels.get_mut(line as usize) {
            Some(l) => {
                *l = level;
                Ok(())
            }
            None => Err(BusError::Decode),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_native_model_is_built_by_name_and_unknown_names_are_refused() {
        let ok = NativeInstance {
            name: "u".into(),
            model: "my_math_ip".into(),
            gp: 0,
            base: 0x43C0_0000,
            size: None,
            irq: None,
            irqs: Vec::new(),
            params: Default::default(),
            bindings: Default::default(),
        };
        assert!(build_pl(std::slice::from_ref(&ok), None).is_ok());
        let spi = NativeInstance {
            model: "axi_quad_spi".into(),
            ..ok.clone()
        };
        assert!(build_pl(&[spi], None).is_ok());
        let fifo = NativeInstance {
            model: "axi_fifo_mm_s".into(),
            ..ok.clone()
        };
        assert!(build_pl(&[fifo], None).is_ok());
        let bad = NativeInstance {
            model: "not_a_model".into(),
            ..ok
        };
        let e = build_pl(&[bad], None).err().expect("refused");
        assert!(e.to_string().contains("not_a_model"), "{e}");
    }

    #[test]
    fn the_first_word_of_a_wide_access_is_recorded() {
        assert_eq!(first8(&[1, 0, 0, 0, 0, 0, 0, 0, 9]), 1);
        assert_eq!(first8(&[0xAA, 0xBB]), 0xBBAA);
    }

    #[test]
    fn an_irq_port_is_detected_by_name_in_the_top_module() {
        let dir = std::env::temp_dir().join(format!("zs-pl-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("top.v");
        std::fs::write(
            &file,
            "module t(input S_AXI_ACLK, input S_AXI_ARESETN, input S_AXI_AWVALID, \
             input [3:0] S_AXI_AWADDR, input [31:0] S_AXI_WDATA, input [3:0] S_AXI_WSTRB, \
             input S_AXI_WVALID, output S_AXI_AWREADY, output S_AXI_WREADY, output [1:0] S_AXI_BRESP, \
             output S_AXI_BVALID, input S_AXI_BREADY, input [3:0] S_AXI_ARADDR, \
             input S_AXI_ARVALID, output S_AXI_ARREADY, output [31:0] S_AXI_RDATA, \
             output [1:0] S_AXI_RRESP, output S_AXI_RVALID, input S_AXI_RREADY, \
             output irq_out); endmodule\n",
        )
        .unwrap();
        assert_eq!(
            detect_irq_ports(std::slice::from_ref(&file), "t"),
            vec!["irq_out".to_string()]
        );
        assert!(detect_irq_ports(std::slice::from_ref(&file), "missing").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
