//! Behavioural models of Xilinx IP, as native PL register blocks (prompt section 9.5).
//!
//! * [`gpio::AxiGpio`]: AXI GPIO 2.0 (PG144);
//! * [`timer::AxiTimer`]: AXI Timer 2.0 (PG079);
//! * [`uartlite::AxiUartLite`]: AXI UART Lite 2.0 (PG142).
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

pub mod gpio;
mod params;
pub mod timer;
pub mod uartlite;

pub use params::{Params, flag, number, parse_number, ranged};

use gpio::{AxiGpio, GpioConfig, GpioHandle};
use pl_api::{
    BoardOutput, BusError, BusResult, MasterAccess, MediaEvent, PinEvent, PinSink, PlTarget,
    PsPort, UartByte,
};
use pl_native::{NativePl, RegisterBlock};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use timer::{AxiTimer, TimerConfig, TimerHandle};
use uartlite::{AxiUartLite, UartConfig, UartHandle};

/// Model names [`VendorPl::add`] accepts, by IP catalog name.
pub const MODELS: &[&str] = &["axi_gpio", "axi_timer", "axi_uartlite"];

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
        _ => &[],
    }
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
}

impl BoardSignal {
    /// Parses `board.leds`, `board.sw` (`switches`), `board.btn` (`buttons`), `board.pmod_ja`..
    /// `board.pmod_je`.
    pub fn parse(text: &str) -> Result<BoardSignal, String> {
        let t = text.trim().to_ascii_lowercase();
        let name = t.strip_prefix("board.").unwrap_or(&t);
        match name {
            "leds" | "led" => Ok(BoardSignal::Leds),
            "sw" | "switches" | "switch" => Ok(BoardSignal::Switches),
            "btn" | "buttons" | "button" => Ok(BoardSignal::Buttons),
            p if p.starts_with("pmod_j") && p.len() == 7 => match p.as_bytes()[6] {
                c @ b'a'..=b'e' => Ok(BoardSignal::Pmod(c - b'a')),
                _ => Err(format!("`{text}`: Pmod ports are pmod_ja .. pmod_je")),
            },
            _ => Err(format!(
                "`{text}` is not a board signal; expected board.leds, board.sw, board.btn \
                 or board.pmod_ja .. board.pmod_je"
            )),
        }
    }

    /// Number of pins the signal carries.
    pub fn width(self) -> u32 {
        match self {
            BoardSignal::Pmod(_) => 8,
            _ => 4,
        }
    }
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
    /// IP parameters by Vivado name (`C_GPIO_WIDTH`), plus `CLK_FREQ_HZ`.
    pub params: Params,
    /// Board bindings of GPIO channels: key `GPIO` (channel 1) or `GPIO2`, value `board.leds`...
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

/// A PL made of vendor IP models and, optionally, an RTL design.
pub struct VendorPl {
    native: NativePl,
    rtl: Option<Box<dyn PlTarget + Send>>,
    gpios: Vec<GpioBind>,
    timers: Vec<(String, TimerHandle)>,
    uarts: Vec<UartBind>,
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
            uarts: Vec::new(),
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
                    sig[ch] = Some(BoardSignal::parse(v).map_err(ctx)?);
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
                let (t, handle) = AxiTimer::new(&inst.name, cfg);
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
            other => {
                return Err(format!(
                    "instance `{}`: `{other}` is not a vendor-ip model; available: {}",
                    inst.name,
                    MODELS.join(", ")
                ));
            }
        };
        self.native.map_irq(inst.gp, inst.base, block, inst.irq);
        Ok(())
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
                    BoardSignal::Switches | BoardSignal::Buttons => {}
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
        self.rtl
            .as_mut()
            .map(|r| r.drain_master_log())
            .unwrap_or_default()
    }

    fn has_board_pins(&self) -> bool {
        self.rtl.as_ref().is_some_and(|r| r.has_board_pins())
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
            || self.gpios.iter().any(|g| {
                g.sig
                    .iter()
                    .flatten()
                    .any(|s| matches!(s, BoardSignal::Pmod(_)))
            })
    }

    fn drain_pin_events(&mut self) -> Vec<PinEvent> {
        let mut v = std::mem::take(&mut self.pin_events);
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
        self.sink = Some(shared);
    }

    fn has_media(&self) -> bool {
        self.rtl.as_ref().is_some_and(|r| r.has_media())
    }

    fn drain_media(&mut self) -> Vec<MediaEvent> {
        self.rtl
            .as_mut()
            .map(|r| r.drain_media())
            .unwrap_or_default()
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
            .add(&inst("c", "axi_dma", 0x4300_0000, None, &[]))
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
