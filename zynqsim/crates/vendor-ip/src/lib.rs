//! Behavioural models of Xilinx IP, as native PL register blocks (prompt section 9.5).
//!
//! * [`gpio::AxiGpio`]: AXI GPIO 2.0 (PG144);
//! * [`timer::AxiTimer`]: AXI Timer 2.0 (PG079);
//! * [`uartlite::AxiUartLite`]: AXI UART Lite 2.0 (PG142);
//! * [`dma::AxiDma`]: AXI DMA 7.1 (PG021), direct register and scatter-gather mode;
//! * [`vdma::AxiVdma`]: AXI VDMA 6.3 (PG020), video frame buffer DMA (ADR-043);
//! * [`ethernetlite::AxiEthernetLite`]: AXI Ethernet Lite 3.0 (PG135);
//! * [`iic::AxiIic`]: AXI IIC 2.1 (PG090), dynamic-mode master on a Pmod port;
//! * [`bram::AxiBramCtrl`]: AXI BRAM Controller 4.1 (PG078) with its block memory;
//! * [`intc::AxiIntc`]: AXI Interrupt Controller 4.1 (PG099);
//! * [`spi::AxiQuadSpi`]: AXI Quad SPI 3.2 (PG153), standard SPI master with loopback;
//! * [`axis_fifo::AxisFifo`]: AXI4-Stream FIFO 4.2 (PG080), memory-mapped, stream loopback.
//! * [`clk_wiz::AxiClkWiz`]: Clocking Wizard 6.0 (PG065), registers and lock timing (ADR-042).
//!
//! Each model is a [`pl_native::RegisterBlock`], so it plugs into [`pl_native::NativePl`] at a
//! base address and drives one `IRQ_F2P` line through the lock-step `advance`. [`VendorPl`]
//! composes the models with an optional RTL design (XSim) on one PL: an access that lands in a
//! vendor window is served by the model, every other access goes to the RTL (ADR-016). It also
//! binds GPIO channels to the board (LEDs, switches, buttons, Pmod pins) and exposes the UART
//! Lite byte streams to the host.
//!
//! **Honesty note:** the register maps and reset values are written from the Xilinx product
//! guides as remembered; every register fact that was not checked against hardware or a Xilinx
//! simulation is marked `VERIFY:` in the module documentation.

pub mod axis_fifo;
pub mod bram;
pub mod cdma;
pub mod clk_wiz;
pub mod dma;
pub mod ethernetlite;
pub mod gpio;
pub mod iic;
pub mod intc;
mod params;
pub mod spi;
pub mod timer;
pub mod uartlite;
pub mod v_tc;
pub mod vdma;

pub use params::{Params, flag, number, parse_number, ranged};

pub use axis_fifo::{AxisFifo, AxisFifoConfig, AxisFifoHandle};
use bram::{AxiBramCtrl, BramConfig, BramHandle};
pub use cdma::{AxiCdma, CdmaConfig, CdmaHandle, CdmaStats};
pub use clk_wiz::{AxiClkWiz, ClkWizConfig, ClkWizHandle};
use dma::{AxiDma, DmaConfig, DmaHandle};
pub use ethernetlite::{
    AxiEthernetLite, EthernetLiteConfig, EthernetLiteHandle, EthernetLiteStats,
};
use gpio::{AxiGpio, GpioConfig, GpioHandle};
use iic::{AxiIic, I2cBus, IicConfig, IicHandle};
use intc::{AxiIntc, IntcConfig, IntcHandle};
use pl_api::{
    BoardOutput, BusError, BusResult, MasterAccess, MediaEvent, PinEvent, PinSink, PlTarget,
    PsPort, UartByte,
};
pub use pl_native::IrqDest;
use pl_native::{NativePl, RegisterBlock};
pub use spi::{AxiQuadSpi, SpiConfig, SpiHandle};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use timer::{AxiTimer, TimerConfig, TimerHandle};
use uartlite::{AxiUartLite, UartConfig, UartHandle};
pub mod ila;
pub use ila::{AxiIla, IlaConfig, IlaHandle, IlaStats};
pub mod uart16550;
pub use uart16550::{AxiUart16550, Uart16550Config, Uart16550Handle, Uart16550Stats};
pub mod v_axi4s_vid_out;
pub use v_axi4s_vid_out::{AxiVidOut, VidOutConfig, VidOutHandle, VidOutStats};
pub mod v_vid_in_axi4s;
pub use v_tc::{AxiVTc, VTcConfig, VTcHandle, VTcStats};
pub use v_vid_in_axi4s::{AxiVidIn, VidInConfig, VidInHandle, VidInStats};
pub use vdma::{AxiVdma, VdmaConfig, VdmaHandle};

/// Model names [`VendorPl::add`] accepts, by IP catalog name.
pub const MODELS: &[&str] = &[
    "axi_gpio",
    "axi_timer",
    "axi_uartlite",
    "axi_uart16550",
    "axi_dma",
    "axi_vdma",
    "axi_cdma",
    "axi_ethernetlite",
    "axi_iic",
    "axi_bram_ctrl",
    "axi_intc",
    "axi_quad_spi",
    "axi_fifo_mm_s",
    "clk_wiz",
    "v_tc",
    "v_axi4s_vid_out",
    "v_vid_in_axi4s",
    "ila",
];

/// True when `name` (`axi_gpio`) is a model of this library.
pub fn is_model(name: &str) -> bool {
    MODELS.contains(&name)
}

/// The IP parameters (Vivado names) a model reads, so a handoff importer can pick them out of
/// the block design.
pub fn param_names(model: &str) -> &'static [&'static str] {
    match model {
        "axi_gpio" => &[
            "C_GPIO_WIDTH",
            "C_GPIO2_WIDTH",
            "C_IS_DUAL",
            "C_ALL_INPUTS",
            "C_ALL_INPUTS_2",
            "C_ALL_OUTPUTS",
            "C_ALL_OUTPUTS_2",
            "C_DOUT_DEFAULT",
            "C_DOUT_DEFAULT_2",
            "C_TRI_DEFAULT",
            "C_TRI_DEFAULT_2",
            "C_INTERRUPT_PRESENT",
        ],
        "axi_timer" => &["C_COUNT_WIDTH", "C_ONE_TIMER_ONLY"],
        "axi_uartlite" => &["C_BAUDRATE", "C_DATA_BITS", "C_USE_PARITY"],
        "axi_dma" => &[
            "C_INCLUDE_SG",
            "C_INCLUDE_MM2S",
            "C_INCLUDE_S2MM",
            "C_SG_LENGTH_WIDTH",
            "C_M_AXI_MM2S_DATA_WIDTH",
            "C_M_AXI_S2MM_DATA_WIDTH",
            "C_ENABLE_MULTI_CHANNEL",
            "C_MICRO_DMA",
        ],
        "axi_vdma" => &[
            "C_INCLUDE_MM2S",
            "C_INCLUDE_S2MM",
            "C_NUM_FSTORES",
            "C_MM2S_LINEBUFFER_DEPTH",
            "C_S2MM_LINEBUFFER_DEPTH",
            "C_MM2S_GENLOCK_MODE",
            "C_S2MM_GENLOCK_MODE",
            "C_INCLUDE_SG",
            "C_M_AXI_MM2S_DATA_WIDTH",
            "C_M_AXI_S2MM_DATA_WIDTH",
            "STREAM_LOOPBACK",
            "PS_PORT",
        ],
        "axi_ethernetlite" => &[
            "C_INCLUDE_GLOBAL_BUFFERS",
            "C_INCLUDE_INTERNAL_LOOPBACK",
            "C_INCLUDE_MDIO",
            "C_TX_PINGPONG",
            "C_RX_PINGPONG",
            "LOOPBACK",
            "STREAM_LOOPBACK",
        ],
        "axi_iic" => &["C_SCL_CLK_FREQ_KHZ"],
        "axi_bram_ctrl" => &["C_ECC", "C_S_AXI_DATA_WIDTH"],
        "axi_intc" => &[
            "C_NUM_INTR_INPUTS",
            "C_KIND_OF_INTR",
            "C_KIND_OF_EDGE",
            "C_KIND_OF_LVL",
            "C_IRQ_IS_LEVEL",
            "C_HAS_FAST",
            "C_IVAR_RESET_VALUE",
            "C_HAS_CASC",
        ],
        "axi_quad_spi" => &[
            "C_FIFO_DEPTH",
            "C_SCK_RATIO",
            "C_NUM_SS_BITS",
            "C_NUM_TRANSFER_BITS",
            "C_SPI_MODE",
            "C_USE_STARTUP",
        ],
        "axi_fifo_mm_s" => &["C_TX_FIFO_DEPTH", "C_RX_FIFO_DEPTH", "C_INTERFACE_TYPE"],
        "clk_wiz" => &[
            "PRIM_IN_FREQ",
            "CLKOUT1_REQUESTED_OUT_FREQ",
            "USE_DYN_RECONFIG",
        ],
        "v_tc" => &[
            "C_GENERATE_EN",
            "C_DETECT_EN",
            "C_MAX_PIXELS",
            "C_MAX_LINES",
            "C_NUM_FSYNCS",
            "CLK_FREQ_HZ",
        ],
        "axi_cdma" => &[
            "C_INCLUDE_SG",
            "C_INCLUDE_DRE",
            "C_M_AXI_DATA_WIDTH",
            "C_M_AXI_ADDR_WIDTH",
            "C_M_AXI_MAX_BURST_LEN",
            "PS_PORT",
        ],
        "axi_uart16550" => &[
            "C_S_AXI_CLK_FREQ_HZ",
            "C_S_AXI_ACLK_FREQ_HZ",
            "C_IS_A_16550",
            "C_HAS_EXTERNAL_XIN",
        ],
        "v_axi4s_vid_out" => &[
            "C_PIXELS_PER_CLOCK",
            "C_COMPONENTS_PER_PIXEL",
            "C_S_AXIS_VIDEO_DATA_WIDTH",
            "C_ADDR_WIDTH",
            "C_HAS_ASYNC_CLK",
            "C_SYNC_LOCK_THRESHOLD",
            "CLK_FREQ_HZ",
        ],
        "v_vid_in_axi4s" => &[
            "C_PIXELS_PER_CLOCK",
            "C_COMPONENTS_PER_PIXEL",
            "C_M_AXIS_VIDEO_DATA_WIDTH",
            "C_ADDR_WIDTH",
            "C_HAS_ASYNC_CLK",
            "C_SYNC_LOCK_THRESHOLD",
            "CLK_FREQ_HZ",
        ],
        "ila" => &[
            "C_NUM_DATA_PROBE",
            "C_PROBE0_WIDTH",
            "C_DATA_DEPTH",
            "C_EN_STRG_QUAL",
            "CLK_FREQ_HZ",
        ],
        _ => &[],
    }
}

/// The interrupt output index of port `port` of `model` (the AXI DMA and VDMA have two outputs, the
/// other models one).
pub fn irq_output_index(model: &str, port: &str) -> usize {
    match (model, port) {
        ("axi_dma", "s2mm_introut") | ("axi_vdma", "s2mm_introut") => 1,
        _ => 0,
    }
}

/// Parses an interrupt destination of a `[connect]` entry: `IRQ_F2P[n]` or `<intc>.intr[k]`.
pub fn parse_irq_dest(text: &str) -> Option<IrqDest> {
    let t = text.trim();
    if let Some(n) = t.strip_prefix("IRQ_F2P[").and_then(|r| r.strip_suffix(']')) {
        return n.trim().parse().ok().map(IrqDest::F2p);
    }
    let (block, rest) = t.split_once('.')?;
    let k = rest.strip_prefix("intr[")?.strip_suffix(']')?;
    Some(IrqDest::Block {
        block: block.to_string(),
        input: k.trim().parse().ok()?,
    })
}

/// The pins of a Pmod port an I2C master uses (`PMOD_SCL_BIT`, `PMOD_SDA_BIT`), defaults 2 and 3
/// like the Digilent TMP2 module.
fn pmod_i2c_pins(p: &Params) -> Result<(u8, u8), String> {
    let scl = ranged(p, "PMOD_SCL_BIT", 2, 0, 7)? as u8;
    let sda = ranged(p, "PMOD_SDA_BIT", 3, 0, 7)? as u8;
    if scl == sda {
        return Err("PMOD_SCL_BIT and PMOD_SDA_BIT are the same pin".to_string());
    }
    Ok((scl, sda))
}

/// A board signal a GPIO channel can be bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardSignal {
    /// The user LEDs (output only; up to eight, bit n = LED n).
    Leds,
    /// The slide switches (input only; up to eight, bit n = switch n).
    Switches,
    /// The push buttons (input only; up to eight, bit n = button n).
    Buttons,
    /// A Pmod port, 0..=4 for JA..JE (eight pins, both directions).
    Pmod(u8),
    /// One slide switch, `board.swN` (bit N of the switches). Only a single-bit input such as a
    /// timer's `CAPTURETRIG` takes it; a GPIO channel takes a whole bus.
    Switch(u8),
    /// One push button, `board.btnN` (bit N of the buttons); see [`BoardSignal::Switch`].
    Button(u8),
}

impl BoardSignal {
    /// Parses `board.leds`, `board.sw` (`switches`), `board.btn` (`buttons`), `board.pmod_ja`..
    /// `board.pmod_je`, and the single bits `board.sw0`..`board.sw7`, `board.btn0`..`board.btn7`.
    pub fn parse(text: &str) -> Result<BoardSignal, String> {
        let t = text.trim().to_ascii_lowercase();
        let name = t.strip_prefix("board.").unwrap_or(&t);
        let bit = |prefix: &str| -> Option<u8> {
            let n = name.strip_prefix(prefix)?;
            match n.as_bytes() {
                [c @ b'0'..=b'7'] => Some(c - b'0'),
                _ => None,
            }
        };
        if let Some(n) = bit("sw") {
            return Ok(BoardSignal::Switch(n));
        }
        if let Some(n) = bit("btn") {
            return Ok(BoardSignal::Button(n));
        }
        match name {
            "leds" | "led" => Ok(BoardSignal::Leds),
            "sw" | "switches" | "switch" => Ok(BoardSignal::Switches),
            "btn" | "buttons" | "button" => Ok(BoardSignal::Buttons),
            p if p.starts_with("pmod_j") && p.len() == 7 => match p.as_bytes()[6] {
                c @ b'a'..=b'e' => Ok(BoardSignal::Pmod(c - b'a')),
                _ => Err(format!("`{text}`: Pmod ports are pmod_ja .. pmod_je")),
            },
            _ => Err(format!(
                "`{text}` is not a board signal; expected board.leds, board.sw, board.btn, \
                 board.swN, board.btnN (N = 0..7) or board.pmod_ja .. board.pmod_je"
            )),
        }
    }

    /// Number of pins the signal carries.
    pub fn width(self) -> u32 {
        match self {
            BoardSignal::Pmod(_) => 8,
            BoardSignal::Switch(_) | BoardSignal::Button(_) => 1,
            _ => 4,
        }
    }
}

/// A timer's `CAPTURETRIG<ch>` bound to one board switch or button.
struct TriggerBind {
    handle: TimerHandle,
    ch: usize,
    sig: BoardSignal,
}

/// One vendor IP instance to place on the PL.
#[derive(Clone, Debug)]
pub struct VendorInstance {
    /// Instance name (block design cell).
    pub name: String,
    /// Model: one of [`MODELS`].
    pub model: String,
    /// `M_AXI_GP` port, 0 or 1.
    pub gp: u8,
    /// Base address.
    pub base: u64,
    /// Size of the address window; `None` uses 64 KiB, what Vivado assigns these IP by default.
    pub size: Option<u64>,
    /// `IRQ_F2P` bit the interrupt output drives.
    pub irq: Option<u32>,
    /// Interrupt outputs by port name with their destination text (`IRQ_F2P[3]` or
    /// `axi_intc_0.intr[1]`, see [`parse_irq_dest`]). When not empty it replaces `irq`; it is
    /// how the AXI DMA's two outputs and the routing into an interrupt controller are given.
    pub irqs: Vec<(String, String)>,
    /// IP parameters by Vivado name (`C_GPIO_WIDTH`), plus `CLK_FREQ_HZ`; for the AXI DMA
    /// `PS_PORT` (`S_AXI_HP0`) and `STREAM_LOOPBACK`, for the AXI IIC `PMOD_SCL_BIT` and
    /// `PMOD_SDA_BIT`.
    pub params: Params,
    /// Board bindings: GPIO channels (`GPIO` for channel 1, `GPIO2`) to `board.leds`..., and
    /// the AXI IIC's `IIC` to a Pmod port (`board.pmod_ja`).
    pub bindings: BTreeMap<String, String>,
}

struct GpioBind {
    handle: GpioHandle,
    /// Signal bound to each channel.
    sig: [Option<BoardSignal>; 2],
    /// Output level last reported per channel.
    last_out: [Option<u32>; 2],
}

struct UartBind {
    name: String,
    handle: UartHandle,
}

struct IicBind {
    name: String,
    handle: IicHandle,
    group: u8,
    scl_bit: u8,
    sda_bit: u8,
}

/// An AXI IIC master on two pins of a Pmod port: what it drives goes to the board's devices
/// through the pin sink, and their answer on SDA comes back in the same call.
struct PmodBus {
    sink: SharedSink,
    group: u8,
    scl_bit: u8,
    sda_bit: u8,
    events: Arc<Mutex<Vec<PinEvent>>>,
    last: Option<u8>,
}

impl I2cBus for PmodBus {
    fn drive(&mut self, t_ns: u64, scl: bool, sda: bool) -> bool {
        let mut levels = !(1u8 << self.scl_bit) & !(1u8 << self.sda_bit);
        levels |= u8::from(scl) << self.scl_bit | u8::from(sda) << self.sda_bit;
        if self.last != Some(levels) {
            self.last = Some(levels);
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(PinEvent {
                    t_ns,
                    group: self.group,
                    levels,
                });
        }
        let back = self.sink.pin_change(self.group, t_ns, levels);
        sda && back >> self.sda_bit & 1 != 0
    }
}

/// A PL made of vendor IP models and, optionally, an RTL design.
pub struct VendorPl {
    native: NativePl,
    rtl: Option<Box<dyn PlTarget + Send>>,
    gpios: Vec<GpioBind>,
    timers: Vec<(String, TimerHandle)>,
    triggers: Vec<TriggerBind>,
    uarts: Vec<UartBind>,
    dmas: Vec<(String, DmaHandle)>,
    vdmas: Vec<(String, VdmaHandle)>,
    ethernetlites: Vec<(String, EthernetLiteHandle)>,
    iics: Vec<IicBind>,
    brams: Vec<(String, BramHandle)>,
    intcs: Vec<(String, IntcHandle)>,
    spis: Vec<(String, SpiHandle)>,
    fifos: Vec<(String, AxisFifoHandle)>,
    clk_wizs: Vec<(String, ClkWizHandle)>,
    vtcs: Vec<(String, VTcHandle)>,
    cdmas: Vec<(String, CdmaHandle)>,
    uarts16550: Vec<(String, Uart16550Handle)>,
    vid_outs: Vec<(String, VidOutHandle)>,
    vid_ins: Vec<(String, VidInHandle)>,
    ilas: Vec<(String, IlaHandle)>,
    /// Pin changes of the I2C masters, stamped with their (possibly future) bus times; they
    /// are handed out once PL time has reached them.
    iic_events: Arc<Mutex<Vec<PinEvent>>>,
    board_out: Vec<BoardOutput>,
    pin_events: Vec<PinEvent>,
    sink: Option<SharedSink>,
    now_ns: u64,
}

/// A pin sink shared between the RTL adapter and the GPIO bindings.
#[derive(Clone)]
struct SharedSink(Arc<Mutex<Box<dyn PinSink>>>);

impl PinSink for SharedSink {
    fn pin_change(&mut self, group: u8, t_ns: u64, levels: u8) -> u8 {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pin_change(group, t_ns, levels)
    }

    fn video_line(&mut self, frame: u32, y: u32, w: u32, h: u32) -> Option<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .video_line(frame, y, w, h)
    }
}

impl VendorPl {
    /// A PL with no blocks yet; `rtl` is the design every unmapped access goes to.
    pub fn new(rtl: Option<Box<dyn PlTarget + Send>>) -> VendorPl {
        VendorPl {
            native: NativePl::new(),
            rtl,
            gpios: Vec::new(),
            timers: Vec::new(),
            triggers: Vec::new(),
            uarts: Vec::new(),
            dmas: Vec::new(),
            vdmas: Vec::new(),
            ethernetlites: Vec::new(),
            iics: Vec::new(),
            brams: Vec::new(),
            intcs: Vec::new(),
            spis: Vec::new(),
            fifos: Vec::new(),
            clk_wizs: Vec::new(),
            vtcs: Vec::new(),
            cdmas: Vec::new(),
            uarts16550: Vec::new(),
            vid_outs: Vec::new(),
            vid_ins: Vec::new(),
            ilas: Vec::new(),
            iic_events: Arc::new(Mutex::new(Vec::new())),
            board_out: Vec::new(),
            pin_events: Vec::new(),
            sink: None,
            now_ns: 0,
        }
    }

    fn check_window(&self, name: &str, gp: u8, base: u64, size: u64) -> Result<(), String> {
        if gp > 1 {
            return Err(format!("instance `{name}`: M_AXI_GP{gp} does not exist"));
        }
        if size == 0 {
            return Err(format!("instance `{name}`: empty address window"));
        }
        for (g, b, s, other) in self.native.windows() {
            if g == gp && base < b + s && b < base + size {
                return Err(format!(
                    "instance `{name}` at {base:#x}+{size:#x} overlaps `{other}` at {b:#x}"
                ));
            }
        }
        Ok(())
    }

    /// Places an arbitrary register block (the `my_math_ip` native model).
    pub fn add_block(
        &mut self,
        name: &str,
        gp: u8,
        base: u64,
        block: Box<dyn RegisterBlock>,
    ) -> Result<(), String> {
        self.check_window(name, gp, base, block.size())?;
        self.native.map(gp, base, block);
        Ok(())
    }

    /// Builds and places a vendor IP instance.
    pub fn add(&mut self, inst: &VendorInstance) -> Result<(), String> {
        let size = inst.size.unwrap_or(0x1_0000);
        self.check_window(&inst.name, inst.gp, inst.base, size)?;
        if let Some(b) = inst.irq {
            if b > 15 {
                return Err(format!(
                    "instance `{}`: IRQ_F2P[{b}] does not exist (0..=15)",
                    inst.name
                ));
            }
        }
        let ctx = |e: String| format!("instance `{}` ({}): {e}", inst.name, inst.model);
        let block: Box<dyn RegisterBlock> = match inst.model.as_str() {
            "axi_gpio" => {
                let cfg = GpioConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (g, handle) = AxiGpio::new(&inst.name, cfg);
                let mut sig = [None; 2];
                for (k, v) in &inst.bindings {
                    let ch = match k.as_str() {
                        "GPIO" => 0,
                        "GPIO2" => 1,
                        other => {
                            return Err(ctx(format!(
                                "binding `{other}` is not a GPIO interface (GPIO or GPIO2)"
                            )));
                        }
                    };
                    if ch >= handle.config().channels() {
                        return Err(ctx(format!("{k} is bound but the IP has one channel")));
                    }
                    let s = BoardSignal::parse(v).map_err(ctx)?;
                    if matches!(s, BoardSignal::Switch(_) | BoardSignal::Button(_)) {
                        return Err(ctx(format!(
                            "{k} is bound to `{v}`, a single bit: a GPIO channel takes the whole \
                             bus (board.sw or board.btn); single bits are for CAPTURETRIG<n>"
                        )));
                    }
                    sig[ch] = Some(s);
                }
                self.gpios.push(GpioBind {
                    handle,
                    sig,
                    last_out: [None; 2],
                });
                Box::new(g)
            }
            "axi_timer" => {
                let cfg = TimerConfig::from_params(&inst.params, size).map_err(ctx)?;
                let one_timer = cfg.one_timer_only;
                let (t, handle) = AxiTimer::new(&inst.name, cfg);
                for (k, v) in &inst.bindings {
                    let ch = match k.as_str() {
                        "CAPTURETRIG0" => 0,
                        "CAPTURETRIG1" if !one_timer => 1,
                        other => {
                            return Err(ctx(format!(
                                "binding `{other}` is not a timer capture input (CAPTURETRIG0{})",
                                if one_timer { "" } else { " or CAPTURETRIG1" }
                            )));
                        }
                    };
                    let sig = BoardSignal::parse(v).map_err(ctx)?;
                    if !matches!(sig, BoardSignal::Switch(_) | BoardSignal::Button(_)) {
                        return Err(ctx(format!(
                            "{k} is bound to `{v}`: a capture trigger takes one switch or button \
                             (board.swN or board.btnN)"
                        )));
                    }
                    self.triggers.push(TriggerBind {
                        handle: handle.clone(),
                        ch,
                        sig,
                    });
                }
                self.timers.push((inst.name.clone(), handle));
                Box::new(t)
            }
            "axi_uartlite" => {
                let cfg = UartConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (u, handle) = AxiUartLite::new(&inst.name, cfg);
                self.uarts.push(UartBind {
                    name: inst.name.clone(),
                    handle,
                });
                Box::new(u)
            }
            "axi_dma" => {
                let cfg = DmaConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (d, handle) = AxiDma::new(&inst.name, cfg);
                self.dmas.push((inst.name.clone(), handle));
                Box::new(d)
            }
            "axi_vdma" => {
                let cfg = VdmaConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (v, handle) = AxiVdma::new(&inst.name, cfg);
                self.vdmas.push((inst.name.clone(), handle));
                Box::new(v)
            }
            "axi_ethernetlite" => {
                let cfg = EthernetLiteConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (e, handle) = AxiEthernetLite::new(&inst.name, cfg);
                self.ethernetlites.push((inst.name.clone(), handle));
                Box::new(e)
            }
            "axi_iic" => {
                let cfg = IicConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (i, handle) = AxiIic::new(&inst.name, cfg);
                for k in inst.bindings.keys() {
                    if k != "IIC" {
                        return Err(ctx(format!("binding `{k}` is not an IIC interface (IIC)")));
                    }
                }
                if let Some(v) = inst.bindings.get("IIC") {
                    let group = match BoardSignal::parse(v).map_err(ctx)? {
                        BoardSignal::Pmod(g) => g,
                        _ => {
                            return Err(ctx(format!(
                                "IIC is bound to `{v}`: an I2C master needs a Pmod port \
                                 (board.pmod_ja .. board.pmod_je)"
                            )));
                        }
                    };
                    let (scl_bit, sda_bit) = pmod_i2c_pins(&inst.params).map_err(ctx)?;
                    self.iics.push(IicBind {
                        name: inst.name.clone(),
                        handle,
                        group,
                        scl_bit,
                        sda_bit,
                    });
                }
                Box::new(i)
            }
            "axi_bram_ctrl" => {
                let cfg = BramConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (b, handle) = AxiBramCtrl::new(&inst.name, &cfg);
                self.brams.push((inst.name.clone(), handle));
                Box::new(b)
            }
            "axi_intc" => {
                let cfg = IntcConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (c, handle) = AxiIntc::new(&inst.name, cfg);
                self.intcs.push((inst.name.clone(), handle));
                Box::new(c)
            }
            "axi_quad_spi" => {
                let cfg = SpiConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (s, handle) = AxiQuadSpi::new(&inst.name, cfg);
                self.spis.push((inst.name.clone(), handle));
                Box::new(s)
            }
            "axi_fifo_mm_s" => {
                let cfg = AxisFifoConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (f, handle) = AxisFifo::new(&inst.name, cfg);
                self.fifos.push((inst.name.clone(), handle));
                Box::new(f)
            }
            "clk_wiz" => {
                let cfg = ClkWizConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (c, handle) = AxiClkWiz::new(&inst.name, cfg);
                self.clk_wizs.push((inst.name.clone(), handle));
                Box::new(c)
            }
            "v_tc" => {
                let cfg = VTcConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (v, handle) = AxiVTc::new(&inst.name, cfg);
                self.vtcs.push((inst.name.clone(), handle));
                Box::new(v)
            }
            "axi_cdma" => {
                let cfg = CdmaConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (c, handle) = AxiCdma::new(&inst.name, cfg);
                self.cdmas.push((inst.name.clone(), handle));
                Box::new(c)
            }
            "axi_uart16550" => {
                let cfg = Uart16550Config::from_params(&inst.params, size).map_err(ctx)?;
                let (u, handle) = AxiUart16550::new(&inst.name, cfg);
                self.uarts16550.push((inst.name.clone(), handle));
                Box::new(u)
            }
            "v_axi4s_vid_out" => {
                let cfg = VidOutConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (v, handle) = AxiVidOut::new(&inst.name, cfg);
                self.vid_outs.push((inst.name.clone(), handle));
                Box::new(v)
            }
            "v_vid_in_axi4s" => {
                let cfg = VidInConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (v, handle) = AxiVidIn::new(&inst.name, cfg);
                self.vid_ins.push((inst.name.clone(), handle));
                Box::new(v)
            }
            "ila" => {
                let cfg = IlaConfig::from_params(&inst.params, size).map_err(ctx)?;
                let (i, handle) = AxiIla::new(&inst.name, cfg);
                self.ilas.push((inst.name.clone(), handle));
                Box::new(i)
            }
            other => {
                return Err(format!(
                    "instance `{}`: `{other}` is not a vendor-ip model; available: {}",
                    inst.name,
                    MODELS.join(", ")
                ));
            }
        };
        if inst.irqs.is_empty() {
            self.native.map_irq(inst.gp, inst.base, block, inst.irq);
        } else {
            self.native.map(inst.gp, inst.base, block);
            for (port, dest) in &inst.irqs {
                let d = parse_irq_dest(dest).ok_or_else(|| {
                    format!(
                        "instance `{}`: interrupt destination `{dest}` of `{port}` is neither \
                         IRQ_F2P[n] nor <controller>.intr[k]",
                        inst.name
                    )
                })?;
                if let IrqDest::F2p(b) = &d {
                    if *b > 15 {
                        return Err(format!(
                            "instance `{}`: IRQ_F2P[{b}] does not exist (0..=15)",
                            inst.name
                        ));
                    }
                }
                self.native
                    .route_irq(&inst.name, irq_output_index(&inst.model, port), d);
            }
        }
        Ok(())
    }

    /// Checks that every interrupt routed to a controller names an `axi_intc` instance and an
    /// input it has. Call after all instances were added.
    pub fn check_routes(&self, instances: &[VendorInstance]) -> Result<(), String> {
        for i in instances {
            for (port, dest) in &i.irqs {
                if let Some(IrqDest::Block { block, input }) = parse_irq_dest(dest) {
                    let Some((_, h)) = self.intcs.iter().find(|(n, _)| *n == block) else {
                        return Err(format!(
                            "instance `{}`: `{port}` is routed to `{block}`, which is not an \
                             axi_intc instance",
                            i.name
                        ));
                    };
                    let n = h.config().inputs;
                    if input >= n {
                        return Err(format!(
                            "instance `{}`: `{port}` is routed to `{block}.intr[{input}]` but \
                             the controller has {n} input(s)",
                            i.name
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The DMA handle of instance `name`.
    pub fn dma(&self, name: &str) -> Option<DmaHandle> {
        self.dmas
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The VDMA handle of instance `name`.
    pub fn vdma(&self, name: &str) -> Option<VdmaHandle> {
        self.vdmas
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The Ethernet Lite handle of instance `name`.
    pub fn ethernetlite(&self, name: &str) -> Option<EthernetLiteHandle> {
        self.ethernetlites
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The BRAM handle of instance `name`.
    pub fn bram(&self, name: &str) -> Option<BramHandle> {
        self.brams
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The interrupt controller handle of instance `name`.
    pub fn intc(&self, name: &str) -> Option<IntcHandle> {
        self.intcs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The IIC handle of instance `name` (only instances bound to a Pmod port).
    pub fn iic(&self, name: &str) -> Option<IicHandle> {
        self.iics
            .iter()
            .find(|i| i.name == name)
            .map(|i| i.handle.clone())
    }

    /// The AXI4-Stream FIFO handle of instance `name`.
    pub fn fifo(&self, name: &str) -> Option<AxisFifoHandle> {
        self.fifos
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The SPI handle of instance `name`.
    pub fn spi(&self, name: &str) -> Option<SpiHandle> {
        self.spis
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The Clocking Wizard handle of instance `name`.
    pub fn clk_wiz(&self, name: &str) -> Option<ClkWizHandle> {
        self.clk_wizs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The Video Timing Controller handle of instance `name`.
    pub fn v_tc(&self, name: &str) -> Option<VTcHandle> {
        self.vtcs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The AXI CDMA handle of instance `name`.
    pub fn cdma(&self, name: &str) -> Option<CdmaHandle> {
        self.cdmas
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The AXI UART 16550 handle of instance `name`.
    pub fn uart16550(&self, name: &str) -> Option<Uart16550Handle> {
        self.uarts16550
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The AXI4-Stream to Video Out handle of instance `name`.
    pub fn vid_out(&self, name: &str) -> Option<VidOutHandle> {
        self.vid_outs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The Video In to AXI4-Stream handle of instance `name`.
    pub fn vid_in(&self, name: &str) -> Option<VidInHandle> {
        self.vid_ins
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The Integrated Logic Analyzer handle of instance `name`.
    pub fn ila(&self, name: &str) -> Option<IlaHandle> {
        self.ilas
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.clone())
    }

    /// The timer handles, by instance name.
    pub fn timers(&self) -> &[(String, TimerHandle)] {
        &self.timers
    }

    /// The GPIO handle of the `i`-th GPIO instance added.
    pub fn gpio(&self, i: usize) -> Option<GpioHandle> {
        self.gpios.get(i).map(|g| g.handle.clone())
    }

    /// The UART handle of channel `i`.
    pub fn uart(&self, i: usize) -> Option<UartHandle> {
        self.uarts.get(i).map(|u| u.handle.clone())
    }

    /// Reports output changes of bound GPIO channels to the board and feeds back what the
    /// board drives into the channel's input pins.
    fn pump(&mut self, now_ns: u64) {
        let mut feedback: Vec<(usize, usize, u32)> = Vec::new();
        for (gi, g) in self.gpios.iter_mut().enumerate() {
            for ch in 0..2 {
                let Some(sig) = g.sig[ch] else { continue };
                let out = g.handle.pins_out(ch);
                if g.last_out[ch] == Some(out) {
                    continue;
                }
                g.last_out[ch] = Some(out);
                match sig {
                    BoardSignal::Leds => self.board_out.push(BoardOutput {
                        t_ns: now_ns,
                        leds: (out & 0xFF) as u8,
                    }),
                    BoardSignal::Pmod(group) => {
                        let levels = (out & 0xFF) as u8;
                        self.pin_events.push(PinEvent {
                            t_ns: now_ns,
                            group,
                            levels,
                        });
                        if let Some(sink) = &mut self.sink {
                            let back = sink.pin_change(group, now_ns, levels);
                            feedback.push((gi, ch, u32::from(back)));
                        }
                    }
                    BoardSignal::Switches
                    | BoardSignal::Buttons
                    | BoardSignal::Switch(_)
                    | BoardSignal::Button(_) => {}
                }
            }
        }
        for (gi, ch, v) in feedback {
            self.gpios[gi].handle.set_input(ch, v);
        }
    }

    fn routed_to_native(&self, gp: u8, addr: u64) -> bool {
        self.native.decodes(gp, addr) || self.rtl.is_none()
    }
}

impl PlTarget for VendorPl {
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.now_ns = self.now_ns.max(now_ns);
        if self.routed_to_native(gp, addr) {
            let r = self.native.read(gp, addr, buf, now_ns, ps);
            self.pump(now_ns);
            r
        } else if let Some(rtl) = &mut self.rtl {
            rtl.read(gp, addr, buf, now_ns, ps)
        } else {
            Err(BusError::Decode)
        }
    }

    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now_ns: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        self.now_ns = self.now_ns.max(now_ns);
        if self.routed_to_native(gp, addr) {
            let r = self.native.write(gp, addr, data, now_ns, ps);
            self.pump(now_ns);
            r
        } else if let Some(rtl) = &mut self.rtl {
            rtl.write(gp, addr, data, now_ns, ps)
        } else {
            Err(BusError::Decode)
        }
    }

    fn wire(&mut self, line: u32, level: bool, now_ns: u64) {
        self.native.wire(line, level, now_ns);
        if let Some(rtl) = &mut self.rtl {
            rtl.wire(line, level, now_ns);
        }
    }

    fn advance(&mut self, now_ns: u64, ps: &mut dyn PsPort) {
        self.now_ns = self.now_ns.max(now_ns);
        self.native.advance(now_ns, ps);
        self.pump(now_ns);
        if let Some(rtl) = &mut self.rtl {
            rtl.advance(now_ns, ps);
        }
    }

    fn drain_master_log(&mut self) -> Vec<MasterAccess> {
        let mut v: Vec<MasterAccess> = self.dmas.iter().flat_map(|(_, d)| d.take_log()).collect();
        if let Some(r) = self.rtl.as_mut() {
            v.extend(r.drain_master_log());
        }
        v.sort_by_key(|a| a.t_ns);
        v
    }

    fn has_board_pins(&self) -> bool {
        self.rtl.as_ref().is_some_and(|r| r.has_board_pins())
            || !self.triggers.is_empty()
            || self.gpios.iter().any(|g| {
                g.sig.iter().flatten().any(|s| {
                    matches!(
                        s,
                        BoardSignal::Leds | BoardSignal::Switches | BoardSignal::Buttons
                    )
                })
            })
    }

    fn set_board_inputs(&mut self, switches: u8, buttons: u8, now_ns: u64) {
        for t in &self.triggers {
            let level = match t.sig {
                BoardSignal::Switch(n) => switches >> n & 1 != 0,
                BoardSignal::Button(n) => buttons >> n & 1 != 0,
                _ => false,
            };
            t.handle.capture_input(t.ch, level, now_ns);
        }
        for g in &self.gpios {
            for ch in 0..2 {
                match g.sig[ch] {
                    Some(BoardSignal::Switches) => {
                        g.handle.set_input(ch, u32::from(switches));
                    }
                    Some(BoardSignal::Buttons) => g.handle.set_input(ch, u32::from(buttons)),
                    _ => {}
                }
            }
        }
        if let Some(rtl) = &mut self.rtl {
            rtl.set_board_inputs(switches, buttons, now_ns);
        }
    }

    fn drain_board_outputs(&mut self) -> Vec<BoardOutput> {
        let mut v = std::mem::take(&mut self.board_out);
        if let Some(rtl) = &mut self.rtl {
            v.extend(rtl.drain_board_outputs());
            v.sort_by_key(|o| o.t_ns);
        }
        v
    }

    fn has_pin_groups(&self) -> bool {
        self.rtl.as_ref().is_some_and(|r| r.has_pin_groups())
            || !self.iics.is_empty()
            || self.gpios.iter().any(|g| {
                g.sig
                    .iter()
                    .flatten()
                    .any(|s| matches!(s, BoardSignal::Pmod(_)))
            })
    }

    fn drain_pin_events(&mut self) -> Vec<PinEvent> {
        let mut v = std::mem::take(&mut self.pin_events);
        {
            // I2C bus times run ahead of PS time; hand out what has happened.
            let mut ev = self
                .iic_events
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let now = self.now_ns;
            let (due, later): (Vec<PinEvent>, Vec<PinEvent>) =
                ev.drain(..).partition(|e| e.t_ns <= now);
            *ev = later;
            v.extend(due);
        }
        if let Some(rtl) = &mut self.rtl {
            v.extend(rtl.drain_pin_events());
            v.sort_by_key(|e| e.t_ns);
        }
        v
    }

    fn set_pin_inputs(&mut self, group: u8, levels: u8, now_ns: u64) {
        for g in &self.gpios {
            for ch in 0..2 {
                if g.sig[ch] == Some(BoardSignal::Pmod(group)) {
                    g.handle.set_input(ch, u32::from(levels));
                }
            }
        }
        if let Some(rtl) = &mut self.rtl {
            rtl.set_pin_inputs(group, levels, now_ns);
        }
    }

    fn set_pin_sink(&mut self, sink: Box<dyn PinSink>) {
        let shared = SharedSink(Arc::new(Mutex::new(sink)));
        if let Some(rtl) = &mut self.rtl {
            rtl.set_pin_sink(Box::new(shared.clone()));
        }
        for i in &self.iics {
            i.handle.set_bus(Box::new(PmodBus {
                sink: shared.clone(),
                group: i.group,
                scl_bit: i.scl_bit,
                sda_bit: i.sda_bit,
                events: Arc::clone(&self.iic_events),
                last: None,
            }));
        }
        self.sink = Some(shared);
    }

    fn has_media(&self) -> bool {
        self.rtl.as_ref().is_some_and(|r| r.has_media())
            || !self.vid_outs.is_empty()
            || !self.vid_ins.is_empty()
    }

    fn drain_media(&mut self) -> Vec<MediaEvent> {
        let mut events = self
            .rtl
            .as_mut()
            .map(|r| r.drain_media())
            .unwrap_or_default();
        for (_, vo) in &self.vid_outs {
            events.extend(vo.drain_media());
        }
        for (_, vi) in &self.vid_ins {
            events.extend(vi.drain_media());
        }
        events
    }

    fn set_media_sources(
        &mut self,
        video: Option<pl_api::VideoSource>,
        audio: Option<pl_api::AudioSource>,
    ) {
        if let Some(rtl) = &mut self.rtl {
            rtl.set_media_sources(video, audio);
        }
    }

    fn uart_channels(&self) -> Vec<String> {
        self.uarts.iter().map(|u| u.name.clone()).collect()
    }

    fn drain_uart_tx(&mut self) -> Vec<UartByte> {
        let now = self.now_ns;
        let mut out = Vec::new();
        for (i, u) in self.uarts.iter().enumerate() {
            for (t_ns, byte) in u.handle.take_tx(now) {
                out.push(UartByte {
                    t_ns,
                    channel: i as u8,
                    byte,
                });
            }
        }
        out.sort_by_key(|b| (b.t_ns, b.channel));
        out
    }

    fn uart_rx(&mut self, channel: u8, data: &[u8], now_ns: u64) -> usize {
        self.uarts
            .get(usize::from(channel))
            .map_or(0, |u| u.handle.push_rx(data, now_ns))
    }

    fn finished(&self) -> bool {
        self.rtl.as_ref().is_some_and(|r| r.finished())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pl_api::PsMasterPort;

    #[derive(Default)]
    struct Ps {
        irqs: Vec<(u32, bool, u64)>,
    }
    impl PsPort for Ps {
        fn mem_read(&mut self, _: PsMasterPort, _: u64, _: &mut [u8]) -> BusResult<()> {
            Err(BusError::Decode)
        }
        fn mem_write(&mut self, _: PsMasterPort, _: u64, _: &[u8]) -> BusResult<()> {
            Err(BusError::Decode)
        }
        fn irq(&mut self, line: u32, level: bool, now_ns: u64) -> BusResult<()> {
            self.irqs.push((line, level, now_ns));
            Ok(())
        }
    }

    fn params(p: &[(&str, &str)]) -> Params {
        p.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn inst(
        name: &str,
        model: &str,
        base: u64,
        irq: Option<u32>,
        p: &[(&str, &str)],
    ) -> VendorInstance {
        VendorInstance {
            name: name.to_string(),
            model: model.to_string(),
            gp: 0,
            base,
            size: None,
            irq,
            irqs: Vec::new(),
            params: params(p),
            bindings: BTreeMap::new(),
        }
    }

    fn rd(pl: &mut VendorPl, ps: &mut Ps, addr: u64, t: u64) -> u32 {
        let mut b = [0u8; 4];
        pl.read(0, addr, &mut b, t, ps).unwrap();
        u32::from_le_bytes(b)
    }

    fn wr(pl: &mut VendorPl, ps: &mut Ps, addr: u64, v: u32, t: u64) {
        pl.write(0, addr, &v.to_le_bytes(), t, ps).unwrap();
    }

    #[test]
    fn board_signals_parse() {
        assert_eq!(BoardSignal::parse("board.leds"), Ok(BoardSignal::Leds));
        assert_eq!(BoardSignal::parse("SW"), Ok(BoardSignal::Switches));
        assert_eq!(
            BoardSignal::parse("board.pmod_jc"),
            Ok(BoardSignal::Pmod(2))
        );
        assert!(BoardSignal::parse("board.pmod_jz").is_err());
        assert!(BoardSignal::parse("hdmi").is_err());
        assert_eq!(BoardSignal::Pmod(0).width(), 8);
        // Single bits for capture triggers.
        assert_eq!(BoardSignal::parse("board.btn0"), Ok(BoardSignal::Button(0)));
        assert_eq!(BoardSignal::parse("board.sw7"), Ok(BoardSignal::Switch(7)));
        assert_eq!(BoardSignal::parse("board.btn"), Ok(BoardSignal::Buttons));
        assert!(BoardSignal::parse("board.btn8").is_err());
        assert!(BoardSignal::parse("board.sw").is_ok());
        assert_eq!(BoardSignal::Button(3).width(), 1);
    }

    #[test]
    fn timer_capture_inputs_are_bound_to_board_bits() {
        let mut pl = VendorPl::new(None);
        let mut timer = inst("tmr", "axi_timer", 0x4280_0000, None, &[]);
        timer
            .bindings
            .insert("CAPTURETRIG0".into(), "board.btn0".into());
        timer
            .bindings
            .insert("CAPTURETRIG1".into(), "board.sw2".into());
        pl.add(&timer).unwrap();
        assert!(pl.has_board_pins(), "a capture input is a board pin");
        let mut ps = Ps::default();
        // Timer 0 in capture mode from 0 at t = 0.
        wr(&mut pl, &mut ps, 0x4280_0004, 0, 0);
        wr(&mut pl, &mut ps, 0x4280_0000, 1 << 5, 0);
        wr(
            &mut pl,
            &mut ps,
            0x4280_0000,
            (1 << 0) | (1 << 3) | (1 << 7),
            0,
        );
        // Button 0 pressed at 1 us: the counter (100 clocks) is captured.
        pl.set_board_inputs(0, 0b0001, 1_000);
        assert_eq!(rd(&mut pl, &mut ps, 0x4280_0004, 1_000), 100);
        // Button 0 held and button 1 (not bound) pressed: no second edge on CAPTURETRIG0.
        pl.set_board_inputs(0, 0b0011, 1_500);
        assert_eq!(pl.timers()[0].1.captures(0), 1, "held high: one edge only");
        // Switch 2 drives CAPTURETRIG1, but timer 1 is not armed: nothing is captured.
        pl.set_board_inputs(0b0100, 0, 2_000);
        assert_eq!(pl.timers()[0].1.captures(0), 1);
        assert_eq!(pl.timers()[0].1.captures(1), 0, "timer 1 is not armed");
        // A GPIO channel cannot take a single bit.
        let mut g = inst("g", "axi_gpio", 0x4120_0000, None, &[("C_GPIO_WIDTH", "4")]);
        g.bindings.insert("GPIO".into(), "board.btn0".into());
        let e = VendorPl::new(None).add(&g).unwrap_err();
        assert!(e.contains("single bit"), "{e}");
        // A timer cannot take a whole bus, nor a Pmod pin, nor a register it does not have.
        let mut bad = inst("t2", "axi_timer", 0x4290_0000, None, &[]);
        bad.bindings
            .insert("CAPTURETRIG0".into(), "board.btn".into());
        assert!(VendorPl::new(None).add(&bad).is_err());
        let mut bad = inst("t3", "axi_timer", 0x4290_0000, None, &[]);
        bad.bindings
            .insert("GENERATEOUT0".into(), "board.btn0".into());
        assert!(
            VendorPl::new(None)
                .add(&bad)
                .unwrap_err()
                .contains("GENERATEOUT0")
        );
    }

    #[test]
    fn gpio_leds_switches_and_interrupt_through_the_pl() {
        let mut pl = VendorPl::new(None);
        let mut leds = inst(
            "gpio_led",
            "axi_gpio",
            0x4120_0000,
            None,
            &[("C_GPIO_WIDTH", "4"), ("C_ALL_OUTPUTS", "1")],
        );
        leds.bindings.insert("GPIO".into(), "board.leds".into());
        let mut sw = inst(
            "gpio_sw",
            "axi_gpio",
            0x4121_0000,
            Some(2),
            &[
                ("C_GPIO_WIDTH", "4"),
                ("C_ALL_INPUTS", "1"),
                ("C_INTERRUPT_PRESENT", "1"),
            ],
        );
        sw.bindings.insert("GPIO".into(), "board.sw".into());
        pl.add(&leds).unwrap();
        pl.add(&sw).unwrap();
        assert!(pl.has_board_pins());
        assert!(!pl.has_pin_groups());
        let mut ps = Ps::default();

        wr(&mut pl, &mut ps, 0x4120_0000, 0b1010, 100);
        let o = pl.drain_board_outputs();
        assert_eq!(
            o,
            vec![BoardOutput {
                t_ns: 100,
                leds: 0b1010
            }]
        );
        // Writing the same value again reports nothing.
        wr(&mut pl, &mut ps, 0x4120_0000, 0b1010, 110);
        assert!(pl.drain_board_outputs().is_empty());

        // Interrupt: enable, flip a switch, the line follows at the next advance.
        wr(&mut pl, &mut ps, 0x4121_0128, 1, 200);
        wr(&mut pl, &mut ps, 0x4121_011C, 0x8000_0000, 210);
        pl.set_board_inputs(0b0100, 0, 300);
        // A board input has no bus access to ride on: the line follows at the next advance.
        assert!(ps.irqs.is_empty());
        pl.advance(1_000, &mut ps);
        assert_eq!(ps.irqs, vec![(2, true, 1_000)]);
        assert_eq!(rd(&mut pl, &mut ps, 0x4121_0000, 1_050), 0b0100);
        // Acknowledge (toggle on write): the line falls with the access, not a quantum later.
        wr(&mut pl, &mut ps, 0x4121_0120, 1, 1_100);
        assert_eq!(ps.irqs.last(), Some(&(2, false, 1_100)));
        pl.advance(2_000, &mut ps);
        assert_eq!(ps.irqs.len(), 2);
        // Unmapped addresses without an RTL design are decode errors.
        let mut b = [0u8; 4];
        assert_eq!(
            pl.read(0, 0x4300_0000, &mut b, 0, &mut ps),
            Err(BusError::Decode)
        );
    }

    #[test]
    fn eight_bit_boards_reach_the_gpio_in_full() {
        // The ZedBoard's 8 LEDs and 8 switches: nothing is cut to four bits (ADR-022).
        let mut pl = VendorPl::new(None);
        let mut leds = inst(
            "gpio_led",
            "axi_gpio",
            0x4120_0000,
            None,
            &[("C_GPIO_WIDTH", "8"), ("C_ALL_OUTPUTS", "1")],
        );
        leds.bindings.insert("GPIO".into(), "board.leds".into());
        let mut sw = inst(
            "gpio_sw",
            "axi_gpio",
            0x4121_0000,
            None,
            &[("C_GPIO_WIDTH", "8"), ("C_ALL_INPUTS", "1")],
        );
        sw.bindings.insert("GPIO".into(), "board.sw".into());
        pl.add(&leds).unwrap();
        pl.add(&sw).unwrap();
        let mut ps = Ps::default();
        wr(&mut pl, &mut ps, 0x4120_0000, 0xA5, 100);
        assert_eq!(
            pl.drain_board_outputs(),
            vec![BoardOutput {
                t_ns: 100,
                leds: 0xA5
            }]
        );
        pl.set_board_inputs(0b1001_0000, 0, 300);
        assert_eq!(rd(&mut pl, &mut ps, 0x4121_0000, 400), 0x90);
    }

    #[test]
    fn timer_interrupt_and_uart_bytes() {
        let mut pl = VendorPl::new(None);
        pl.add(&inst("tmr", "axi_timer", 0x4280_0000, Some(0), &[]))
            .unwrap();
        pl.add(&inst(
            "uart",
            "axi_uartlite",
            0x4060_0000,
            Some(1),
            &[("C_BAUDRATE", "1000000")],
        ))
        .unwrap();
        assert_eq!(pl.uart_channels(), vec!["uart".to_string()]);
        let mut ps = Ps::default();
        // Timer: down count 99 clocks + 1 = 1 us, interrupt enabled.
        wr(&mut pl, &mut ps, 0x4280_0004, 99, 0);
        wr(&mut pl, &mut ps, 0x4280_0000, 1 << 5, 0);
        wr(&mut pl, &mut ps, 0x4280_0000, 0x80 | 0x40 | 0x10 | 0x2, 0);
        pl.advance(500, &mut ps);
        assert!(ps.irqs.is_empty());
        pl.advance(1_000, &mut ps);
        assert_eq!(ps.irqs, vec![(0, true, 1_000)]);
        // UART: transmit "ok" at 1 Mbaud, the bytes appear with completion stamps.
        wr(&mut pl, &mut ps, 0x4060_0004, u32::from(b'o'), 2_000);
        wr(&mut pl, &mut ps, 0x4060_0004, u32::from(b'k'), 2_000);
        pl.advance(40_000, &mut ps);
        let tx = pl.drain_uart_tx();
        assert_eq!(
            tx,
            vec![
                UartByte {
                    t_ns: 12_000,
                    channel: 0,
                    byte: b'o'
                },
                UartByte {
                    t_ns: 22_000,
                    channel: 0,
                    byte: b'k'
                },
            ]
        );
        // Receive.
        assert_eq!(pl.uart_rx(0, b"Z", 50_000), 1);
        assert_eq!(pl.uart_rx(3, b"Z", 50_000), 0, "no such channel");
        pl.advance(70_000, &mut ps);
        assert_eq!(rd(&mut pl, &mut ps, 0x4060_0008, 70_000) & 1, 1);
        assert_eq!(rd(&mut pl, &mut ps, 0x4060_0000, 70_000), u32::from(b'Z'));
    }

    #[test]
    fn overlapping_windows_and_bad_instances_are_errors() {
        let mut pl = VendorPl::new(None);
        pl.add(&inst("a", "axi_gpio", 0x4120_0000, None, &[]))
            .unwrap();
        let e = pl
            .add(&inst("b", "axi_timer", 0x4120_8000, None, &[]))
            .unwrap_err();
        assert!(e.contains("overlaps"), "{e}");
        let e = pl
            .add(&inst("c", "axi_hwicap", 0x4300_0000, None, &[]))
            .unwrap_err();
        assert!(e.contains("not a vendor-ip model"), "{e}");
        let e = pl
            .add(&inst(
                "d",
                "axi_gpio",
                0x4130_0000,
                None,
                &[("C_GPIO_WIDTH", "99")],
            ))
            .unwrap_err();
        assert!(e.contains("C_GPIO_WIDTH"), "{e}");
        let mut v = inst("e", "axi_gpio", 0x4140_0000, Some(16), &[]);
        assert!(pl.add(&v).unwrap_err().contains("IRQ_F2P[16]"));
        v.irq = None;
        v.bindings.insert("GPIO2".into(), "board.leds".into());
        assert!(pl.add(&v).unwrap_err().contains("one channel"));
    }

    /// An RTL stand-in: answers everything outside the vendor windows with 0xAA.
    struct Rtl {
        reads: Vec<u64>,
    }
    impl PlTarget for Rtl {
        fn read(
            &mut self,
            _: u8,
            a: u64,
            buf: &mut [u8],
            _: u64,
            _: &mut dyn PsPort,
        ) -> BusResult<()> {
            self.reads.push(a);
            buf.fill(0xAA);
            Ok(())
        }
        fn write(&mut self, _: u8, _: u64, _: &[u8], _: u64, _: &mut dyn PsPort) -> BusResult<()> {
            Ok(())
        }
    }

    #[test]
    fn unmapped_accesses_go_to_the_rtl_design() {
        let mut pl = VendorPl::new(Some(Box::new(Rtl { reads: Vec::new() })));
        pl.add(&inst(
            "g",
            "axi_gpio",
            0x4120_0000,
            None,
            &[("C_GPIO_WIDTH", "4"), ("C_ALL_OUTPUTS", "1")],
        ))
        .unwrap();
        let mut ps = Ps::default();
        assert_eq!(rd(&mut pl, &mut ps, 0x43C0_0000, 0), 0xAAAA_AAAA);
        wr(&mut pl, &mut ps, 0x4120_0000, 5, 0);
        assert_eq!(rd(&mut pl, &mut ps, 0x4120_0000, 0), 5);
    }
}
