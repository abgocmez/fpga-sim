//! The synchronous simulator: board, scheduler, UARTs, trace and recording,
//! driven by a small built-in *demo* model.
//!
//! The demo model stands in for the PS and PL backends until `backend-qemu` and
//! the RTL adapters exist (milestone P1). It is deliberately labelled
//! [`Fidelity::Demo`] everywhere: it is not a CPU or RTL simulation.
//!
//! Demo behaviour (all deterministic):
//! * LED0..LED2 follow SW0..SW2.
//! * LED3 is a heartbeat whose half-period is drawn from the seeded PRNG
//!   (400..600 ms), so the seed is observable in the trace.
//! * RGB LED5 green duty is the number of switches on, in quarters; RGB LED6
//!   blue is on while any button is held.
//! * Pressing BTN*n* sends `BTNn pressed\r\n` on UART1 at 115200 8N1 timing.
//! * Bytes written to a UART are echoed back with one character time of delay.

use crate::bytelog::ByteLog;
use crate::error::ApiError;
use crate::recording::{RecEntry, Recording, ReplayReport, SimConfig};
use crate::types::{Capability, Command, Fidelity, Input, Query, Response, Status, TraceFormat};
use board_zybo::{BoardDef, BoardState};
use serde::{Deserialize, Serialize};
use sim_core::{Prng, Ps, Scheduler};
use std::collections::BTreeMap;
use trace::{TraceEvent, TraceLog};

/// One character at 115200 baud, 8N1: 10 bit times, truncated to a picosecond.
const BYTE_TIME: Ps = Ps(86_805_555);
/// Heartbeat half-period range in picoseconds (400..600 ms).
const HEARTBEAT_MIN: u64 = 400 * Ps::PER_MS;
const HEARTBEAT_MAX: u64 = 600 * Ps::PER_MS;
/// Number of UARTs on the Zynq-7000 PS.
const UARTS: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum DemoEvent {
    Heartbeat,
    UartTx { port: u8, byte: u8 },
}

/// All mutable simulation state; this is what a snapshot captures.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    config: SimConfig,
    seed: u64,
    quantum_ps: u64,
    sched: Scheduler<DemoEvent>,
    board: BoardState,
    prng: Prng,
    trace: TraceLog,
    entries: Vec<RecEntry>,
    uart_tx: [ByteLog; UARTS],
    uart_rx: [ByteLog; UARTS],
    tx_free: [Ps; UARTS],
    heartbeat_on: bool,
}

/// The simulator. Single-threaded; wrap it in [`crate::SimClient`] for use
/// from a GUI or server.
pub struct Simulator {
    def: BoardDef,
    st: State,
    snapshots: BTreeMap<String, Vec<u8>>,
}

impl Simulator {
    /// Builds a simulator for a built-in board and powers it on at time zero.
    pub fn new(config: SimConfig) -> Result<Simulator, ApiError> {
        let def = BoardDef::builtin(&config.board).ok_or_else(|| {
            ApiError::not_found(format!("unknown board `{}`", config.board))
                .with_hint("built-in boards: zybo-z7-20")
        })?;
        Simulator::with_def(def, config)
    }

    /// Builds a simulator for an explicit board description.
    pub fn with_def(def: BoardDef, config: SimConfig) -> Result<Simulator, ApiError> {
        if config.quantum_ps == 0 {
            return Err(ApiError::invalid_argument("quantum_ps must be non-zero"));
        }
        let st = State {
            seed: config.seed,
            quantum_ps: config.quantum_ps,
            sched: Scheduler::new(),
            board: BoardState::new(&def),
            prng: Prng::new(config.seed),
            trace: TraceLog::new(),
            entries: Vec::new(),
            uart_tx: Default::default(),
            uart_rx: Default::default(),
            tx_free: [Ps::ZERO; UARTS],
            heartbeat_on: false,
            config,
        };
        let mut sim = Simulator {
            def,
            st,
            snapshots: BTreeMap::new(),
        };
        sim.power_on("power_on");
        Ok(sim)
    }

    /// The board description.
    pub fn def(&self) -> &BoardDef {
        &self.def
    }

    /// Current simulation time.
    pub fn now(&self) -> Ps {
        self.st.sched.now()
    }

    /// The trace so far.
    pub fn trace(&self) -> &TraceLog {
        &self.st.trace
    }

    /// The board state.
    pub fn board(&self) -> &BoardState {
        &self.st.board
    }

    /// The lock-step quantum.
    pub fn quantum(&self) -> Ps {
        Ps(self.st.quantum_ps)
    }

    /// What the demo backend can do.
    pub fn capabilities() -> Vec<Capability> {
        vec![
            Capability::BoardIo,
            Capability::Uart,
            Capability::Trace,
            Capability::Snapshot,
            Capability::Replay,
            Capability::Screenshot,
        ]
    }

    /// Status, with `playing` supplied by the caller (the simulator itself is
    /// never "running" between calls).
    pub fn status(&self, playing: bool) -> Status {
        Status {
            now_ps: self.now().0,
            playing,
            quantum_ps: self.st.quantum_ps,
            seed: self.st.seed,
            board: self.def.name.clone(),
            ps_backend: "demo".to_string(),
            pl_backend: "demo".to_string(),
            fidelity: Fidelity::Demo,
            capabilities: Self::capabilities(),
            trace_len: self.st.trace.len(),
            trace_hash: self.st.trace.hash_string(),
            pending_events: self.st.sched.len(),
            devices: Vec::new(),
            debug: None,
            media: None,
        }
    }

    // ------------------------------------------------------------------ run

    /// Advances time by `d`, firing every event on the way.
    pub fn run_for(&mut self, d: Ps) {
        let target = self.now().saturating_add(d);
        self.run_until(target);
    }

    /// Advances time to `t`, firing every event due at or before `t`.
    /// Does nothing if `t` is not in the future.
    pub fn run_until(&mut self, t: Ps) {
        while let Some((_, ev)) = self.st.sched.pop_due(t) {
            self.handle(ev);
        }
        self.st.sched.advance_to(t);
    }

    /// Advances by one quantum.
    pub fn step(&mut self) {
        let q = self.quantum();
        self.run_for(q);
    }

    // --------------------------------------------------------------- inputs

    /// Applies an input at the current time and records it.
    pub fn apply(&mut self, input: Input) -> Result<(), ApiError> {
        self.validate_input(&input)?;
        let now = self.now();
        self.st.entries.push(RecEntry {
            t_ps: now.0,
            input: input.clone(),
        });
        match input {
            Input::SetSwitch { index, on } => {
                self.note(format!(
                    "switch {index} = {}",
                    if on { "on" } else { "off" }
                ));
                self.st.board.switches[index] = on;
                let name = format!("board.{}", self.def.switches[index].id);
                self.signal(name, u64::from(on), 1);
                self.update_outputs();
            }
            Input::SetButton { index, pressed } => {
                let was = self.st.board.buttons[index];
                self.note(format!(
                    "button {index} = {}",
                    if pressed { "pressed" } else { "released" }
                ));
                self.st.board.buttons[index] = pressed;
                let name = format!("board.{}", self.def.buttons[index].id);
                self.signal(name, u64::from(pressed), 1);
                self.update_outputs();
                if pressed && !was {
                    let msg = format!("BTN{index} pressed\r\n");
                    self.enqueue_tx(1, msg.as_bytes());
                }
            }
            Input::UartWrite { port, data } => {
                self.note(format!("uart{port} write {} byte(s)", data.len()));
                for &b in &data {
                    self.st.trace.push(
                        now,
                        TraceEvent::Uart {
                            port,
                            tx: false,
                            byte: b,
                        },
                    );
                    self.st.uart_rx[usize::from(port)].push(b);
                }
                self.enqueue_tx(port, &data);
            }
            Input::Reset { seed } => {
                if let Some(s) = seed {
                    self.st.seed = s;
                }
                self.st.prng = Prng::new(self.st.seed);
                self.st.board.reset();
                self.st.heartbeat_on = false;
                self.st.sched = Scheduler::new();
                self.st.sched.advance_to(now);
                self.st.tx_free = [now; UARTS];
                self.power_on("reset");
            }
        }
        Ok(())
    }

    fn validate_input(&self, input: &Input) -> Result<(), ApiError> {
        match input {
            Input::SetSwitch { index, .. } if *index >= self.def.switches.len() => {
                Err(ApiError::invalid_argument(format!(
                    "switch index {index} out of range 0..{}",
                    self.def.switches.len()
                )))
            }
            Input::SetButton { index, .. } if *index >= self.def.buttons.len() => {
                Err(ApiError::invalid_argument(format!(
                    "button index {index} out of range 0..{}",
                    self.def.buttons.len()
                )))
            }
            Input::UartWrite { port, .. } if usize::from(*port) >= UARTS => Err(
                ApiError::invalid_argument(format!("uart port {port} out of range 0..{UARTS}")),
            ),
            _ => Ok(()),
        }
    }

    // -------------------------------------------------------------- commands

    /// Executes a state-changing command.
    ///
    /// `Play` and `Pause` need the asynchronous server and are rejected here.
    pub fn command(&mut self, cmd: Command) -> Result<Response, ApiError> {
        match cmd {
            Command::Input(i) => {
                self.apply(i)?;
                Ok(Response::Ok)
            }
            Command::Run { duration_ps } => {
                self.run_for(Ps(duration_ps));
                Ok(Response::Time {
                    now_ps: self.now().0,
                })
            }
            Command::RunUntil { time_ps } => {
                self.run_until(Ps(time_ps));
                Ok(Response::Time {
                    now_ps: self.now().0,
                })
            }
            Command::Step => {
                self.step();
                Ok(Response::Time {
                    now_ps: self.now().0,
                })
            }
            Command::SetQuantum { quantum_ps } => {
                if quantum_ps == 0 || quantum_ps > Ps::PER_S {
                    return Err(ApiError::invalid_argument(
                        "quantum_ps must be between 1 ps and 1 s",
                    ));
                }
                self.st.quantum_ps = quantum_ps;
                Ok(Response::Ok)
            }
            Command::SnapshotSave { name } => {
                self.snapshot_save(&name)?;
                Ok(Response::Ok)
            }
            Command::SnapshotRestore { name } => {
                self.snapshot_restore(&name)?;
                Ok(Response::Ok)
            }
            Command::AttachDevice { .. } => Err(self.unsupported(
                "pmod devices",
                "the demo engine has no PL pins (use qemu + xsim with the pin-group DPI)",
            )),
            Command::SetBreakpoint { .. } => Err(self.unsupported(
                "breakpoints",
                "the demo engine runs no CPU (use the qemu backend with [debug])",
            )),
            Command::SetMediaSource { .. } => Err(self.unsupported(
                "media sources",
                "audio and video need a PL with the board's media models (qemu + xsim)",
            )),
            Command::MemWrite { .. } => {
                Err(self.unsupported("mem_write", "memory access needs a PS backend (qemu)"))
            }
            Command::Play { .. } | Command::Pause => Err(ApiError::unsupported(
                "continuous running needs the simulation server (SimClient)",
            )),
        }
    }

    /// Saves the full state under `name`, replacing any previous snapshot.
    pub fn snapshot_save(&mut self, name: &str) -> Result<(), ApiError> {
        if name.is_empty() {
            return Err(ApiError::invalid_argument("snapshot name is empty"));
        }
        let bytes = serde_json::to_vec(&self.st)
            .map_err(|e| ApiError::internal(format!("snapshot encode failed: {e}")))?;
        self.snapshots.insert(name.to_string(), bytes);
        Ok(())
    }

    /// Restores a saved state. The snapshot set itself is kept.
    pub fn snapshot_restore(&mut self, name: &str) -> Result<(), ApiError> {
        let bytes = self
            .snapshots
            .get(name)
            .ok_or_else(|| ApiError::not_found(format!("no snapshot named `{name}`")))?;
        self.st = serde_json::from_slice(bytes)
            .map_err(|e| ApiError::internal(format!("snapshot decode failed: {e}")))?;
        Ok(())
    }

    // --------------------------------------------------------------- queries

    /// Answers a read-only query. `playing` is reported in [`Status`].
    pub fn query(&self, q: &Query, playing: bool) -> Result<Response, ApiError> {
        match q {
            Query::Status => Ok(Response::Status(Box::new(self.status(playing)))),
            Query::BoardDef => Ok(Response::BoardDef(Box::new(self.def.clone()))),
            Query::BoardState => Ok(Response::BoardState(self.st.board.clone())),
            Query::Pin { name } => self
                .st
                .board
                .pin_level(&self.def, name)
                .map(|level| Response::Pin {
                    name: name.clone(),
                    level,
                })
                .ok_or_else(|| {
                    ApiError::not_found(format!("no pin or control named `{name}`"))
                        .with_hint("try sw0..sw3, btn0..btn3, led0..led3, led5_r/g/b, JA1..JF10")
                }),
            Query::UartRead { port, from, max } => {
                let log = self
                    .st
                    .uart_tx
                    .get(usize::from(*port))
                    .ok_or_else(|| ApiError::invalid_argument(format!("no uart {port}")))?;
                let (data, next) = log.read(*from, *max);
                Ok(Response::Uart {
                    data,
                    next,
                    oldest: log.base,
                })
            }
            Query::Trace { from, max } => {
                let all = self.st.trace.records();
                let start = (*from).min(all.len());
                let end = start.saturating_add(*max).min(all.len());
                Ok(Response::Trace {
                    records: all[start..end].to_vec(),
                    next: end,
                    total: all.len(),
                    hash: self.st.trace.hash_string(),
                })
            }
            Query::TraceExport { format } => Ok(Response::Text(match format {
                TraceFormat::Vcd => trace::to_vcd(&self.st.trace),
                TraceFormat::Perfetto => trace::to_perfetto_json(&self.st.trace),
            })),
            Query::Recording => Ok(Response::Recording(Box::new(self.recording()))),
            Query::Snapshots => Ok(Response::Snapshots(
                self.snapshots.keys().cloned().collect(),
            )),
            Query::Screenshot { scale } => {
                if !scale.is_finite() || !(0.25..=4.0).contains(scale) {
                    return Err(ApiError::invalid_argument("scale must be within 0.25..=4"));
                }
                board_zybo::render_png(&self.def, &self.st.board, *scale)
                    .map(Response::Png)
                    .map_err(ApiError::internal)
            }
            Query::DeviceImage { .. } => Err(self.unsupported(
                "device_image",
                "the demo engine has no PL pins (use qemu + xsim with the pin-group DPI)",
            )),
            Query::Breakpoints | Query::Symbol { .. } | Query::Disasm { .. } => Err(self
                .unsupported(
                    "breakpoints and symbols",
                    "the demo engine runs no CPU (use the qemu backend with [debug])",
                )),
            Query::VideoFrame => Err(self.unsupported(
                "video_frame",
                "video needs a PL with an HDMI model (qemu + xsim)",
            )),
            Query::AudioCapture { .. } | Query::AudioFrom { .. } => Err(self.unsupported(
                "audio_capture",
                "audio needs a PL with the codec model (qemu + xsim)",
            )),
            Query::CpuRegs { .. } => Err(self.unsupported(
                "cpu_regs",
                "CPU state needs a PS backend (QEMU, milestone P1)",
            )),
            Query::MemRead { .. } => Err(self.unsupported(
                "mem_read",
                "physical memory access needs a PS backend (QEMU, milestone P1)",
            )),
            Query::PlSignal { .. } => Err(self.unsupported(
                "pl_signal_get",
                "PL signals need an RTL backend (Verilator/Icarus/XSim, milestone P1)",
            )),
        }
    }

    fn unsupported(&self, what: &str, why: &str) -> ApiError {
        ApiError::unsupported(format!(
            "`{what}` is not available on the `demo` backend: {why}"
        ))
    }

    // ------------------------------------------------------------- recording

    /// The recording of this session so far.
    pub fn recording(&self) -> Recording {
        Recording {
            version: 1,
            config: SimConfig {
                board: self.st.config.board.clone(),
                seed: self.st.config.seed,
                quantum_ps: self.st.config.quantum_ps,
            },
            entries: self.st.entries.clone(),
            end_ps: self.now().0,
            trace_hash: self.st.trace.hash_string(),
            journal: Vec::new(),
            project: None,
        }
    }

    /// Replays a recording on a fresh simulator and compares digests.
    ///
    /// Returns the replayed simulator (for trace export) and the report.
    pub fn replay(rec: &Recording) -> Result<(Simulator, ReplayReport), ApiError> {
        if rec.version != 1 {
            return Err(ApiError::unsupported(format!(
                "recording version {} is not supported (expected 1)",
                rec.version
            )));
        }
        let mut sim = Simulator::new(rec.config.clone())?;
        let mut last = 0u64;
        for e in &rec.entries {
            if e.t_ps < last {
                return Err(ApiError::invalid_argument(format!(
                    "recording entries are not in time order ({} after {last})",
                    e.t_ps
                )));
            }
            last = e.t_ps;
            sim.run_until(Ps(e.t_ps));
            sim.apply(e.input.clone())?;
        }
        sim.run_until(Ps(rec.end_ps));
        let report = ReplayReport {
            trace_hash: sim.st.trace.hash_string(),
            expected_hash: rec.trace_hash.clone(),
            matches: sim.st.trace.hash_string() == rec.trace_hash,
            end_ps: sim.now().0,
            trace_len: sim.st.trace.len(),
        };
        Ok((sim, report))
    }

    // -------------------------------------------------------------- internals

    fn note(&mut self, what: String) {
        let t = self.now();
        self.st.trace.push(t, TraceEvent::Input { what });
    }

    fn signal(&mut self, name: String, value: u64, width: u8) {
        let t = self.now();
        self.st
            .trace
            .push(t, TraceEvent::Signal { name, value, width });
    }

    /// Initial conditions at power-on and after reset.
    fn power_on(&mut self, reason: &str) {
        let t = self.now();
        let seed = self.st.seed;
        self.st.trace.push(
            t,
            TraceEvent::Marker {
                text: format!("{reason} seed={seed}"),
            },
        );
        // Declare every board signal at its initial value so waveforms start
        // defined rather than `x`.
        for i in 0..self.def.switches.len() {
            let name = format!("board.{}", self.def.switches[i].id);
            self.signal(name, 0, 1);
        }
        for i in 0..self.def.buttons.len() {
            let name = format!("board.{}", self.def.buttons[i].id);
            self.signal(name, 0, 1);
        }
        for i in 0..self.def.leds.len() {
            let name = format!("board.{}", self.def.leds[i].id);
            self.signal(name, 0, 8);
        }
        for i in 0..self.def.rgb_leds.len() {
            let name = format!("board.{}", self.def.rgb_leds[i].id);
            self.signal(name, 0, 24);
        }
        let half = self.st.prng.range(HEARTBEAT_MIN, HEARTBEAT_MAX);
        self.st.sched.schedule_in(Ps(half), 0, DemoEvent::Heartbeat);
        self.enqueue_tx(1, b"zynqsim demo ready\r\n");
    }

    /// Queues bytes on a UART transmitter at 115200 8N1 timing.
    fn enqueue_tx(&mut self, port: u8, bytes: &[u8]) {
        let p = usize::from(port);
        for &byte in bytes {
            let start = self.st.tx_free[p].max(self.now());
            let at = start.saturating_add(BYTE_TIME);
            self.st.tx_free[p] = at;
            // `at >= now` by construction.
            let _ = self
                .st
                .sched
                .schedule_at(at, 10, DemoEvent::UartTx { port, byte });
        }
    }

    fn handle(&mut self, ev: DemoEvent) {
        let t = self.now();
        match ev {
            DemoEvent::Heartbeat => {
                self.st.heartbeat_on = !self.st.heartbeat_on;
                self.update_outputs();
                let half = self.st.prng.range(HEARTBEAT_MIN, HEARTBEAT_MAX);
                self.st.sched.schedule_in(Ps(half), 0, DemoEvent::Heartbeat);
            }
            DemoEvent::UartTx { port, byte } => {
                self.st.trace.push(
                    t,
                    TraceEvent::Uart {
                        port,
                        tx: true,
                        byte,
                    },
                );
                self.st.uart_tx[usize::from(port)].push(byte);
            }
        }
    }

    /// Recomputes every LED from switches, buttons and the heartbeat, tracing
    /// only the outputs that changed.
    fn update_outputs(&mut self) {
        let on = |b: bool| if b { 255u8 } else { 0 };
        for i in 0..self.def.leds.len() {
            let duty = match i {
                0..=2 => on(self.st.board.switches.get(i).copied().unwrap_or(false)),
                3 => on(self.st.heartbeat_on),
                _ => 0,
            };
            if self.st.board.leds[i] != duty {
                self.st.board.leds[i] = duty;
                let name = format!("board.{}", self.def.leds[i].id);
                self.signal(name, u64::from(duty), 8);
            }
        }
        let n_on = self.st.board.switches.iter().filter(|&&s| s).count();
        let any_btn = self.st.board.buttons.iter().any(|&b| b);
        for i in 0..self.def.rgb_leds.len() {
            let rgb = match i {
                0 => [0, (n_on * 255 / 4) as u8, 0],
                1 => [0, 0, on(any_btn)],
                _ => [0, 0, 0],
            };
            if self.st.board.rgb[i] != rgb {
                self.st.board.rgb[i] = rgb;
                let name = format!("board.{}", self.def.rgb_leds[i].id);
                let v = (u64::from(rgb[0]) << 16) | (u64::from(rgb[1]) << 8) | u64::from(rgb[2]);
                self.signal(name, v, 24);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim() -> Simulator {
        Simulator::new(SimConfig::default()).unwrap()
    }

    fn uart1(s: &Simulator) -> String {
        match s
            .query(
                &Query::UartRead {
                    port: 1,
                    from: 0,
                    max: 4096,
                },
                false,
            )
            .unwrap()
        {
            Response::Uart { data, .. } => String::from_utf8(data).unwrap(),
            r => panic!("unexpected {r:?}"),
        }
    }

    #[test]
    fn banner_arrives_at_uart_timing() {
        let mut s = sim();
        s.run_for(Ps::from_ms(1)); // 20 chars * 86.8 us = 1.74 ms: not yet complete
        assert!(uart1(&s).len() < 20);
        s.run_for(Ps::from_ms(2));
        assert_eq!(uart1(&s), "zynqsim demo ready\r\n");
    }

    #[test]
    fn switches_drive_leds_and_rgb() {
        let mut s = sim();
        s.apply(Input::SetSwitch { index: 0, on: true }).unwrap();
        s.apply(Input::SetSwitch { index: 2, on: true }).unwrap();
        assert_eq!(s.board().leds[..3], [255, 0, 255]);
        assert_eq!(s.board().rgb[0], [0, 127, 0]);
        s.apply(Input::SetButton {
            index: 1,
            pressed: true,
        })
        .unwrap();
        assert_eq!(s.board().rgb[1], [0, 0, 255]);
        assert_eq!(
            s.query(
                &Query::Pin {
                    name: "led0".into()
                },
                false
            )
            .unwrap(),
            Response::Pin {
                name: "led0".into(),
                level: board_zybo::PinLevel::High
            }
        );
    }

    #[test]
    fn button_press_prints_once_and_echo_works() {
        let mut s = sim();
        s.run_for(Ps::from_ms(5)); // let the banner finish
        s.apply(Input::SetButton {
            index: 2,
            pressed: true,
        })
        .unwrap();
        s.apply(Input::SetButton {
            index: 2,
            pressed: false,
        })
        .unwrap();
        s.apply(Input::UartWrite {
            port: 1,
            data: b"hi".to_vec(),
        })
        .unwrap();
        s.run_for(Ps::from_ms(10));
        assert_eq!(uart1(&s), "zynqsim demo ready\r\nBTN2 pressed\r\nhi");
    }

    #[test]
    fn heartbeat_toggles_led3_and_seed_changes_trace() {
        let run = |seed| {
            let mut s = Simulator::new(SimConfig {
                seed,
                ..SimConfig::default()
            })
            .unwrap();
            s.run_for(Ps::from_s(2));
            s
        };
        let a = run(1);
        assert!(a.board().leds[3] == 0 || a.board().leds[3] == 255);
        let toggles = a
            .trace()
            .records()
            .iter()
            .filter(|r| matches!(&r.event, TraceEvent::Signal { name, .. } if name == "board.led3"))
            .count();
        // 2 s at 400..600 ms half-periods: 3 to 5 toggles, plus the initial 0.
        assert!((4..=6).contains(&toggles), "toggles = {toggles}");
        assert_eq!(a.trace().hash_string(), run(1).trace().hash_string());
        assert_ne!(a.trace().hash_string(), run(2).trace().hash_string());
    }

    #[test]
    fn rejects_bad_indices_without_recording_them() {
        let mut s = sim();
        let e = s
            .apply(Input::SetSwitch { index: 9, on: true })
            .unwrap_err();
        assert_eq!(e.code, crate::ErrorCode::InvalidArgument);
        assert!(
            s.apply(Input::UartWrite {
                port: 2,
                data: vec![]
            })
            .is_err()
        );
        assert!(s.recording().entries.is_empty());
    }

    #[test]
    fn snapshot_restore_rewinds_everything() {
        let mut s = sim();
        s.apply(Input::SetSwitch { index: 1, on: true }).unwrap();
        s.run_for(Ps::from_ms(3));
        s.snapshot_save("a").unwrap();
        let hash = s.trace().hash_string();
        let now = s.now();

        s.apply(Input::SetSwitch { index: 3, on: true }).unwrap();
        s.run_for(Ps::from_ms(700));
        assert_ne!(s.trace().hash_string(), hash);

        s.snapshot_restore("a").unwrap();
        assert_eq!(s.now(), now);
        assert_eq!(s.trace().hash_string(), hash);
        assert!(!s.board().switches[3]);
        assert_eq!(s.recording().entries.len(), 1);

        // Continuing after a restore matches an uninterrupted run.
        s.run_for(Ps::from_ms(700));
        let mut t = sim();
        t.apply(Input::SetSwitch { index: 1, on: true }).unwrap();
        t.run_for(Ps::from_ms(3));
        t.run_for(Ps::from_ms(700));
        assert_eq!(s.trace().hash_string(), t.trace().hash_string());
    }

    #[test]
    fn unknown_snapshot_and_unsupported_queries_are_structured_errors() {
        let mut s = sim();
        assert_eq!(
            s.snapshot_restore("nope").unwrap_err().code,
            crate::ErrorCode::NotFound
        );
        let e = s.query(&Query::CpuRegs { core: 0 }, false).unwrap_err();
        assert_eq!(e.code, crate::ErrorCode::Unsupported);
        assert!(e.message.contains("demo"));
        assert!(s.command(Command::Play { ps_per_second: 1 }).is_err());
    }

    #[test]
    fn replay_reproduces_a_session_exactly() {
        let mut s = sim();
        s.run_for(Ps::from_ms(1));
        s.apply(Input::SetSwitch { index: 0, on: true }).unwrap();
        s.run_for(Ps(123_456_789));
        s.apply(Input::UartWrite {
            port: 0,
            data: b"abc".to_vec(),
        })
        .unwrap();
        s.apply(Input::SetButton {
            index: 0,
            pressed: true,
        })
        .unwrap();
        s.run_for(Ps::from_s(1));
        s.apply(Input::Reset { seed: Some(9) }).unwrap();
        s.run_for(Ps::from_ms(900));
        let rec = s.recording();

        let (replayed, report) = Simulator::replay(&rec).unwrap();
        assert!(report.matches, "{report:?}");
        assert_eq!(replayed.trace().records(), s.trace().records());

        // A tampered recording must not silently match.
        let mut bad = rec.clone();
        bad.entries[0].t_ps += 1;
        let (_, r) = Simulator::replay(&bad).unwrap();
        assert!(!r.matches);
    }

    #[test]
    fn uart_log_reads_are_offset_addressed() {
        let mut s = sim();
        s.run_for(Ps::from_ms(5));
        let r = s
            .query(
                &Query::UartRead {
                    port: 1,
                    from: 5,
                    max: 4,
                },
                false,
            )
            .unwrap();
        match r {
            Response::Uart { data, next, oldest } => {
                assert_eq!(data, b"im d");
                assert_eq!(next, 9);
                assert_eq!(oldest, 0);
            }
            r => panic!("unexpected {r:?}"),
        }
    }
    /// Prompt §14.6: identical inputs, five runs, one digest, and the replay of
    /// the first run agrees with all of them.
    #[test]
    fn five_runs_one_digest_and_replay_agrees() {
        let run = || {
            let mut s = sim();
            s.apply(Input::SetSwitch { index: 3, on: true }).unwrap();
            s.run_for(Ps::from_ms(40));
            s.apply(Input::SetButton {
                index: 0,
                pressed: true,
            })
            .unwrap();
            s.run_for(Ps::from_ms(60));
            s.apply(Input::SetButton {
                index: 0,
                pressed: false,
            })
            .unwrap();
            s.apply(Input::UartWrite {
                port: 0,
                data: b"determinism".to_vec(),
            })
            .unwrap();
            s.run_for(Ps::from_s(3));
            s
        };
        let first = run();
        for _ in 0..4 {
            assert_eq!(run().trace().hash_string(), first.trace().hash_string());
        }
        let (_, report) = Simulator::replay(&first.recording()).unwrap();
        assert!(report.matches);
    }
}
