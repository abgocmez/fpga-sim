//! An RTL module behind an AXI4-Lite slave, as a native register block.
//!
//! [`RtlModule`] runs an elaborated design on the engine and drives its AXI4-Lite
//! slave port with proper VALID/READY handshakes: each register access is one
//! transaction of as many clock cycles as the design takes. Signals are sampled
//! just before each rising edge, the way a synchronous slave samples them. The
//! engine's clock is driven by the adapter (no free-running clock is needed), so the
//! engine's time advances by one period per cycle.
//!
//! The port prefix (`S_AXI`), clock and reset are found from the port names when
//! they are not given. Only the 32-bit data width is supported.

use crate::design::Design;
use crate::sim::Simulator;
use pl_native::RegisterBlock;

/// Clock period of the adapter's clock, in picoseconds (10 ns, 100 MHz).
const PERIOD_PS: u64 = 10_000;
/// Cycles one transaction may take before it is reported as stuck.
const TIMEOUT_CYCLES: u32 = 1_000;

/// An elaborated RTL design with an AXI4-Lite slave, usable as a [`RegisterBlock`].
pub struct RtlModule {
    sim: Simulator,
    name: String,
    prefix: String,
    clock: String,
    reset: Option<(String, bool)>,
    window: u64,
    last_error: Option<String>,
}

impl RtlModule {
    /// Wraps `design`, detecting the AXI4-Lite port as `S_AXI*` and the clock and
    /// reset as `S_AXI_ACLK` and `S_AXI_ARESETN` (active low).
    pub fn new(design: Design) -> Result<RtlModule, String> {
        let has = |n: &str| design.signal(n).is_some();
        let prefix = "S_AXI".to_string();
        let clock = format!("{prefix}_ACLK");
        if !has(&clock) || !has(&format!("{prefix}_AWVALID")) {
            return Err(format!(
                "no AXI4-Lite slave port `{prefix}_*` (need {clock} and {prefix}_AWVALID) in `{}`",
                design.top
            ));
        }
        let reset = if has(&format!("{prefix}_ARESETN")) {
            Some((format!("{prefix}_ARESETN"), true))
        } else {
            None
        };
        let name = design.top.clone();
        RtlModule::with_ports(design, &name, &prefix, &clock, reset)
    }

    /// Wraps `design` with explicit port names. `reset` is `(name, active_low)`.
    pub fn with_ports(
        design: Design,
        name: &str,
        prefix: &str,
        clock: &str,
        reset: Option<(String, bool)>,
    ) -> Result<RtlModule, String> {
        let aw = design
            .signal(&format!("{prefix}_AWADDR"))
            .map(|id| design.signals[id as usize].width)
            .ok_or_else(|| format!("no `{prefix}_AWADDR` port"))?;
        if let Some(id) = design.signal(&format!("{prefix}_WDATA"))
            && design.signals[id as usize].width != 32
        {
            return Err("only 32-bit AXI4-Lite data is supported".into());
        }
        let window = if aw >= 12 { 1u64 << aw.min(40) } else { 0x1000 };
        let mut sim = Simulator::new(design).map_err(|d| {
            d.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        for n in [clock.to_string(), format!("{prefix}_ARESETN")] {
            if sim.design().signal(&n).is_some() {
                let _ = sim.set(&n, 0);
            }
        }
        let mut m = RtlModule {
            sim,
            name: name.to_string(),
            prefix: prefix.to_string(),
            clock: clock.to_string(),
            reset,
            window: window.max(0x1000),
            last_error: None,
        };
        m.reset_hw()?;
        Ok(m)
    }

    /// The simulator, for inspection (signal values, the log, snapshots).
    pub fn sim(&self) -> &Simulator {
        &self.sim
    }

    /// The simulator, mutably.
    pub fn sim_mut(&mut self) -> &mut Simulator {
        &mut self.sim
    }

    /// The last error of a register access made through [`RegisterBlock`], if any.
    pub fn take_error(&mut self) -> Option<String> {
        self.last_error.take()
    }

    fn sig(&self, suffix: &str) -> String {
        format!("{}_{}", self.prefix, suffix)
    }

    fn drive(&mut self, name: &str, v: u128) -> Result<(), String> {
        self.sim.set(name, v)
    }

    fn get(&self, name: &str) -> u128 {
        self.sim.get(name).unwrap_or(0)
    }

    /// One full clock period: rising edge, then falling edge, with settling after each.
    fn cycle(&mut self) -> Result<(), String> {
        let clk = self.clock.clone();
        self.drive(&clk, 1)?;
        let t = self.sim.now_ps();
        self.sim.run_until(t).map_err(|e| e.to_string())?;
        self.drive(&clk, 0)?;
        self.sim
            .run_until(t + PERIOD_PS / 2)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Settles the combinational logic at the current time.
    fn settle(&mut self) -> Result<(), String> {
        let t = self.sim.now_ps();
        self.sim.run_until(t).map_err(|e| e.to_string())
    }

    /// Holds reset for two cycles (active level per the reset port), then releases it.
    pub fn reset_hw(&mut self) -> Result<(), String> {
        for n in [
            self.sig("AWVALID"),
            self.sig("WVALID"),
            self.sig("ARVALID"),
            self.sig("BREADY"),
            self.sig("RREADY"),
        ] {
            if self.sim.design().signal(&n).is_some() {
                self.drive(&n, 0)?;
            }
        }
        let Some((rst, active_low)) = self.reset.clone() else {
            return Ok(());
        };
        let asserted = if active_low { 0 } else { 1 };
        let released = 1 - asserted;
        self.drive(&rst, asserted)?;
        self.cycle()?;
        self.cycle()?;
        self.drive(&rst, released)?;
        self.cycle()
    }

    /// One AXI4-Lite write of `data` (lanes enabled by `strb`) to `addr`.
    pub fn write(&mut self, addr: u64, data: u32, strb: u8) -> Result<(), String> {
        let (awaddr, awvalid, wdata, wstrb, wvalid, bready) = (
            self.sig("AWADDR"),
            self.sig("AWVALID"),
            self.sig("WDATA"),
            self.sig("WSTRB"),
            self.sig("WVALID"),
            self.sig("BREADY"),
        );
        self.drive(&awaddr, addr as u128)?;
        self.drive(&wdata, data as u128)?;
        self.drive(&wstrb, (strb & 0xF) as u128)?;
        self.drive(&awvalid, 1)?;
        self.drive(&wvalid, 1)?;
        self.drive(&bready, 1)?;
        self.settle()?;
        let (mut aw_done, mut w_done) = (false, false);
        for _ in 0..TIMEOUT_CYCLES {
            let awready = self.get(&self.sig("AWREADY")) != 0;
            let wready = self.get(&self.sig("WREADY")) != 0;
            let bvalid = self.get(&self.sig("BVALID")) != 0;
            let resp = self.get(&self.sig("BRESP"));
            let aw_hs = !aw_done && awready && self.get(&awvalid) != 0;
            let w_hs = !w_done && wready && self.get(&wvalid) != 0;
            let b_hs = bvalid && self.get(&bready) != 0;
            self.cycle()?;
            if aw_hs {
                aw_done = true;
                self.drive(&awvalid, 0)?;
            }
            if w_hs {
                w_done = true;
                self.drive(&wvalid, 0)?;
            }
            if b_hs {
                self.drive(&bready, 0)?;
                self.settle()?;
                return if resp == 0 {
                    Ok(())
                } else {
                    Err(format!("write to {addr:#x} answered with BRESP {resp}"))
                };
            }
        }
        Err(format!(
            "write to {addr:#x} did not complete in {TIMEOUT_CYCLES} cycles"
        ))
    }

    /// One AXI4-Lite read of `addr`.
    pub fn read(&mut self, addr: u64) -> Result<u32, String> {
        let (araddr, arvalid, rready) =
            (self.sig("ARADDR"), self.sig("ARVALID"), self.sig("RREADY"));
        let rdata = self.sig("RDATA");
        self.drive(&araddr, addr as u128)?;
        self.drive(&arvalid, 1)?;
        self.drive(&rready, 1)?;
        self.settle()?;
        let mut ar_done = false;
        for _ in 0..TIMEOUT_CYCLES {
            let arready = self.get(&self.sig("ARREADY")) != 0;
            let rvalid = self.get(&self.sig("RVALID")) != 0;
            let resp = self.get(&self.sig("RRESP"));
            let data = self.get(&rdata) as u32;
            let ar_hs = !ar_done && arready && self.get(&arvalid) != 0;
            let r_hs = rvalid && self.get(&rready) != 0;
            self.cycle()?;
            if ar_hs {
                ar_done = true;
                self.drive(&arvalid, 0)?;
            }
            if r_hs {
                self.drive(&rready, 0)?;
                self.settle()?;
                return if resp == 0 {
                    Ok(data)
                } else {
                    Err(format!("read of {addr:#x} answered with RRESP {resp}"))
                };
            }
        }
        Err(format!(
            "read of {addr:#x} did not complete in {TIMEOUT_CYCLES} cycles"
        ))
    }
}

impl RegisterBlock for RtlModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        match self.read(offset) {
            Ok(v) => v,
            Err(e) => {
                self.last_error = Some(e);
                0
            }
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        if let Err(e) = self.write(offset, value, strobe) {
            self.last_error = Some(e);
        }
    }

    fn reset(&mut self) {
        if let Err(e) = self.reset_hw() {
            self.last_error = Some(e);
        }
    }
}
