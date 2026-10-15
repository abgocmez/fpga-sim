//! The GUI's behaviour without any drawing code, so it can be tested headless.

use crate::loader::LoadJob;
use sim_api::board::{BoardDef, BoardState};
use sim_api::trace::TraceRecord;
use sim_api::{
    ApiError, AudioSpec, Capability, Command, Event, Input, Project, Ps, Query, Response,
    SimClient, Status, VideoSpec,
};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Console history kept per UART, in characters.
const CONSOLE_CAP: usize = 64 * 1024;
/// Trace records shown in the trace panel.
const TRACE_TAIL: usize = 40;

/// Playback speeds offered in the control bar: label and simulated
/// picoseconds per wall-clock second.
pub const SPEEDS: &[(&str, u64)] = &[
    ("1x real time", Ps::PER_S),
    ("10x", 10 * Ps::PER_S),
    ("100x", 100 * Ps::PER_S),
    ("1000x", 1000 * Ps::PER_S),
];

/// Most audio kept for the speaker while it is wanted (one second of frames).
const AUDIO_PENDING_CAP: usize = 48_000 * 2;
/// Least time between HDMI frame fetches.
const VIDEO_POLL: Duration = Duration::from_millis(100);

/// An HDMI TX frame, ready to upload as a texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frame counter since the start of the run.
    pub frame: u32,
    /// `width * height * 4` bytes, opaque.
    pub rgba: Vec<u8>,
}

/// Everything the UI shows and every action it can take.
pub struct Model {
    /// Control-plane handle.
    pub client: SimClient,
    /// Board description.
    pub def: BoardDef,
    /// Latest board state.
    pub state: BoardState,
    /// Latest status.
    pub status: Status,
    /// Target-to-host text per UART.
    pub console: [String; 2],
    /// Newest trace records, oldest first.
    pub trace_tail: Vec<TraceRecord>,
    /// Total trace records, for the panel header.
    pub trace_total: usize,
    /// The most recent error to show in the status bar.
    pub last_error: Option<String>,
    /// Index into [`SPEEDS`].
    pub speed: usize,
    /// Seed text box contents.
    pub seed_text: String,
    /// The newest complete HDMI TX frame, when the PL has an HDMI model.
    pub video: Option<VideoFrame>,
    /// Peak level (0..1) of the audio fetched by the latest refresh.
    pub audio_level: f32,
    /// Keep audio for the speaker ([`Model::take_audio`]).
    pub audio_wanted: bool,
    /// Counts engine swaps, so views can drop what they cached.
    pub generation: u32,
    /// Registers of each core while the guest is stopped under the debugger.
    pub cpu_regs: [Option<sim_api::CpuRegs>; 2],
    /// The newest picture of each attached Pmod device: `(port, kind, state,
    /// width, height, rgba)`.
    pub device_images: Vec<(u8, String, String, u32, u32, Vec<u8>)>,
    /// The quantities the host may set on each attached device (a sensor's
    /// temperature...): `(port, quantities)`.
    pub device_values: Vec<(u8, Vec<sim_api::DeviceValueInfo>)>,
    device_polled: Option<Instant>,
    /// An engine being built in the background.
    pub load: Option<LoadJob>,
    /// Why the last load failed.
    pub load_error: Option<String>,
    cursors: [u64; 2],
    events: Receiver<Event>,
    video_seen: u32,
    video_polled: Option<Instant>,
    audio_cursor: u64,
    audio_pending: Vec<i16>,
}

impl Model {
    /// Connects to a simulation and loads the first state.
    pub fn new(client: SimClient) -> Result<Model, ApiError> {
        let def = client.board_def()?;
        let state = client.board_state()?;
        let status = client.status()?;
        let events = client.subscribe()?;
        let mut m = Model {
            client,
            def,
            state,
            seed_text: status.seed.to_string(),
            status,
            console: [String::new(), String::new()],
            trace_tail: Vec::new(),
            trace_total: 0,
            last_error: None,
            speed: 0,
            video: None,
            audio_level: 0.0,
            audio_wanted: false,
            generation: 0,
            cpu_regs: [None, None],
            device_images: Vec::new(),
            device_values: Vec::new(),
            device_polled: None,
            load: None,
            load_error: None,
            cursors: [0; 2],
            events,
            video_seen: 0,
            video_polled: None,
            audio_cursor: 0,
            audio_pending: Vec::new(),
        };
        m.refresh();
        Ok(m)
    }

    fn note(&mut self, r: Result<Response, ApiError>) {
        match r {
            Ok(_) => self.last_error = None,
            Err(e) => self.last_error = Some(e.message),
        }
    }

    /// Starts building the engine for `project` in the background; when it is
    /// ready, [`Model::refresh`] switches to it.
    pub fn start_load(&mut self, project: Project, dir: PathBuf, what: String) {
        self.load_error = None;
        self.load = Some(LoadJob::start(project, dir, what));
    }

    /// What is being built and for how long, while a load is in flight.
    pub fn loading(&self) -> Option<(&str, Duration)> {
        self.load
            .as_ref()
            .map(|j| (j.what.as_str(), j.since.elapsed()))
    }

    /// Replaces the running simulation with `client`'s, keeping UI settings.
    pub fn adopt(&mut self, client: SimClient) -> Result<(), ApiError> {
        let mut fresh = Model::new(client)?;
        fresh.speed = self.speed;
        fresh.audio_wanted = self.audio_wanted;
        fresh.generation = self.generation + 1;
        fresh.load = self.load.take();
        *self = fresh;
        Ok(())
    }

    fn poll_load(&mut self) {
        let Some(job) = &self.load else { return };
        match job.poll() {
            None => {}
            Some(Ok(client)) => {
                self.load = None;
                if let Err(e) = self.adopt(client) {
                    self.load_error = Some(e.message);
                }
            }
            Some(Err(e)) => {
                self.load = None;
                self.load_error = Some(e);
            }
        }
    }

    /// Whether the active PL has the board's HDMI and codec models.
    pub fn has_media(&self) -> bool {
        self.status.capabilities.contains(&Capability::Video)
            || self.status.capabilities.contains(&Capability::Audio)
    }

    /// Chooses the HDMI RX picture.
    pub fn set_video_source(&mut self, spec: VideoSpec) {
        let r = self.client.command(Command::SetMediaSource {
            video: Some(spec),
            audio: None,
        });
        self.note(r);
    }

    /// Chooses the codec LINE IN audio.
    pub fn set_audio_source(&mut self, spec: AudioSpec) {
        let r = self.client.command(Command::SetMediaSource {
            video: None,
            audio: Some(spec),
        });
        self.note(r);
    }

    /// Takes the audio fetched since the last call (interleaved stereo, 48 kHz).
    pub fn take_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.audio_pending)
    }

    /// Fetches the pictures of attached devices, at most ten times a second.
    fn refresh_devices(&mut self) {
        if self.status.devices.is_empty() {
            self.device_images.clear();
            self.device_values.clear();
            return;
        }
        if self
            .device_polled
            .is_some_and(|t| t.elapsed() < Duration::from_millis(100))
        {
            return;
        }
        self.device_polled = Some(Instant::now());
        let ports: Vec<u8> = self.status.devices.iter().map(|d| d.port).collect();
        let mut images = Vec::new();
        let mut values = Vec::new();
        for port in ports {
            if let Ok(Response::DeviceState { values: v, .. }) =
                self.client.query(Query::DeviceState { port })
            {
                if !v.is_empty() {
                    values.push((port, v));
                }
            }
            if let Ok(Response::DeviceImage {
                kind,
                png,
                description,
                ..
            }) = self.client.query(Query::DeviceImage { port })
            {
                if let Ok((w, h, rgb)) = sim_api::board::decode_png_rgb(&png) {
                    let rgba = rgb
                        .chunks_exact(3)
                        .flat_map(|p| [p[0], p[1], p[2], 255])
                        .collect();
                    images.push((port, kind, description, w, h, rgba));
                }
            }
        }
        self.device_images = images;
        self.device_values = values;
    }

    /// Sets a physical quantity of the device on Pmod port `port` (a
    /// sensor's temperature); the firmware sees it at its next read.
    pub fn set_device_value(&mut self, port: u8, name: &str, value: f64) {
        let r = self.client.command(Command::SetDeviceValue {
            port,
            name: name.to_string(),
            value,
        });
        // Show the new value at once, not after the next poll.
        if r.is_ok() {
            if let Some((_, vs)) = self.device_values.iter_mut().find(|(p, _)| *p == port) {
                if let Some(v) = vs.iter_mut().find(|v| v.name == name) {
                    v.value = value.clamp(v.min, v.max);
                }
            }
        }
        self.note(r);
    }

    /// Reads the registers when (and only when) the guest is stopped.
    fn refresh_cpu(&mut self) {
        let stopped = self.status.debug.as_ref().is_some_and(|d| d.stopped);
        if !stopped {
            self.cpu_regs = [None, None];
            return;
        }
        for core in 0..2u8 {
            self.cpu_regs[usize::from(core)] = match self.client.query(Query::CpuRegs { core }) {
                Ok(Response::CpuRegs(r)) => Some(r),
                _ => None,
            };
        }
    }

    /// Fetches new HDMI frames and DAC audio when the status says there are some.
    fn refresh_media(&mut self) {
        let Some(m) = self.status.media.clone() else {
            return;
        };
        if m.video_frames != self.video_seen
            && self.video_polled.is_none_or(|t| t.elapsed() >= VIDEO_POLL)
        {
            self.video_polled = Some(Instant::now());
            if let Ok(Response::Video { frame, png, .. }) = self.client.query(Query::VideoFrame) {
                if let Ok((width, height, rgb)) = sim_api::board::decode_png_rgb(&png) {
                    let rgba = rgb
                        .chunks_exact(3)
                        .flat_map(|p| [p[0], p[1], p[2], 255])
                        .collect();
                    self.video = Some(VideoFrame {
                        width,
                        height,
                        frame,
                        rgba,
                    });
                }
            }
            self.video_seen = m.video_frames;
        }
        if m.audio_frames > self.audio_cursor {
            if let Ok(Response::Audio {
                first_frame,
                frames,
                wav,
                peak,
                ..
            }) = self.client.query(Query::AudioFrom {
                from_frame: self.audio_cursor,
                max_frames: 48_000,
            }) {
                self.audio_cursor = first_frame + frames;
                self.audio_level = peak as f32 / 32768.0;
                if self.audio_wanted {
                    if let Ok((samples, _rate)) = sim_api::parse_wav(&wav) {
                        self.audio_pending.extend_from_slice(&samples);
                        let over = self.audio_pending.len().saturating_sub(AUDIO_PENDING_CAP);
                        if over > 0 {
                            self.audio_pending.drain(..over & !1);
                        }
                    }
                }
            }
        } else if m.audio_frames == 0 {
            self.audio_cursor = 0;
        }
    }

    /// Pulls the latest state. Cheap enough to call every frame.
    pub fn refresh(&mut self) {
        self.poll_load();
        let mut restored = false;
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Restored => restored = true,
                Event::Fault { message } => self.last_error = Some(message),
                _ => {}
            }
        }
        if restored {
            self.console = [String::new(), String::new()];
            self.cursors = [0; 2];
        }
        if let Ok(s) = self.client.status() {
            self.status = s;
        }
        if let Ok(b) = self.client.board_state() {
            self.state = b;
        }
        for port in 0..2u8 {
            let from = self.cursors[usize::from(port)];
            if let Ok(Response::Uart { data, next, .. }) = self.client.query(Query::UartRead {
                port,
                from,
                max: 16 * 1024,
            }) {
                if !data.is_empty() {
                    let c = &mut self.console[usize::from(port)];
                    c.push_str(&String::from_utf8_lossy(&data));
                    if c.len() > CONSOLE_CAP {
                        let mut cut = c.len() - CONSOLE_CAP;
                        while !c.is_char_boundary(cut) {
                            cut += 1;
                        }
                        c.drain(..cut);
                    }
                }
                self.cursors[usize::from(port)] = next;
            }
        }
        self.refresh_media();
        self.refresh_cpu();
        self.refresh_devices();
        let total = self.status.trace_len;
        let from = total.saturating_sub(TRACE_TAIL);
        if let Ok(Response::Trace { records, .. }) = self.client.query(Query::Trace {
            from,
            max: TRACE_TAIL,
        }) {
            self.trace_tail = records;
            self.trace_total = total;
        }
    }

    /// Flips a slide switch.
    pub fn toggle_switch(&mut self, index: usize) {
        let on = !self.state.switches.get(index).copied().unwrap_or(false);
        let r = self
            .client
            .command(Command::Input(Input::SetSwitch { index, on }));
        self.note(r);
    }

    /// Presses or releases a button.
    pub fn set_button(&mut self, index: usize, pressed: bool) {
        let r = self
            .client
            .command(Command::Input(Input::SetButton { index, pressed }));
        self.note(r);
    }

    /// Sends text to the target on a UART, optionally followed by CR LF.
    pub fn send_uart(&mut self, port: u8, text: &str, crlf: bool) {
        let mut data = text.as_bytes().to_vec();
        if crlf {
            data.extend_from_slice(b"\r\n");
        }
        if data.is_empty() {
            return;
        }
        let r = self
            .client
            .command(Command::Input(Input::UartWrite { port, data }));
        self.note(r);
    }

    /// Starts continuous running at the selected speed.
    pub fn play(&mut self) {
        let r = self.client.command(Command::Play {
            ps_per_second: SPEEDS[self.speed.min(SPEEDS.len() - 1)].1,
        });
        self.note(r);
    }

    /// Stops continuous running.
    pub fn pause(&mut self) {
        let r = self.client.command(Command::Pause);
        self.note(r);
    }

    /// Advances by one quantum.
    pub fn step(&mut self) {
        let r = self.client.command(Command::Step);
        self.note(r);
    }

    /// Advances by a fixed duration.
    pub fn run_for(&mut self, duration_ps: u64) {
        let r = self.client.command(Command::Run { duration_ps });
        self.note(r);
    }

    /// Resets the system, re-seeding from the seed box when it holds a number.
    pub fn reset(&mut self) {
        let seed = self.seed_text.trim().parse::<u64>().ok();
        let r = self.client.command(Command::Input(Input::Reset { seed }));
        self.console = [String::new(), String::new()];
        self.cursors = [0; 2];
        self.note(r);
    }

    /// Saves the quick snapshot.
    pub fn snapshot_save(&mut self) {
        let r = self.client.command(Command::SnapshotSave {
            name: "quick".to_string(),
        });
        self.note(r);
    }

    /// Restores the quick snapshot.
    pub fn snapshot_restore(&mut self) {
        let r = self.client.command(Command::SnapshotRestore {
            name: "quick".to_string(),
        });
        self.note(r);
        self.refresh();
    }

    /// Reads guest physical memory.
    pub fn mem_read(&self, addr: u64, len: u32) -> Result<Vec<u8>, String> {
        match self.client.query(Query::MemRead { addr, len }) {
            Ok(Response::Memory { data, .. }) => Ok(data),
            Ok(other) => Err(format!("unexpected response {other:?}")),
            Err(e) => Err(e.message),
        }
    }

    /// Whether the active engine can read memory.
    pub fn can_read_memory(&self) -> bool {
        self.status
            .capabilities
            .contains(&sim_api::Capability::MemoryAccess)
    }

    /// Why a CPU panel has nothing to show, in the backend's own words.
    pub fn cpu_unavailable_reason(&self, core: u8) -> String {
        match self.client.query(Query::CpuRegs { core }) {
            Err(e) => e.message,
            Ok(_) => "CPU state is available".to_string(),
        }
    }

    /// Why the PL panel has nothing to show.
    pub fn pl_unavailable_reason(&self) -> String {
        match self.client.query(Query::PlSignal {
            name: "top".to_string(),
        }) {
            Err(e) => e.message,
            Ok(_) => "PL signals are available".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_api::SimConfig;

    fn model() -> Model {
        Model::new(SimClient::spawn(SimConfig::default()).unwrap()).unwrap()
    }

    #[test]
    fn starts_with_board_status_and_honest_fidelity() {
        let m = model();
        assert_eq!(m.def.name, "zybo-z7-20");
        assert_eq!(m.status.ps_backend, "demo");
        assert_eq!(m.state.switches.len(), 4);
        assert!(m.trace_total > 0 && !m.trace_tail.is_empty());
    }

    #[test]
    fn device_values_go_through_the_simulation_and_errors_are_shown() {
        let mut m = model();
        // The demo engine has no Pmod pins: the request reaches it and its
        // explanation is surfaced, nothing is invented.
        m.set_device_value(0, "temperature", 21.0);
        let e = m.last_error.clone().expect("an explanation");
        assert!(e.contains("pmod devices"), "{e}");
        assert!(m.device_values.is_empty());
        m.refresh();
        assert!(m.device_values.is_empty() && m.device_images.is_empty());
    }

    #[test]
    fn switch_and_button_actions_reach_the_simulation() {
        let mut m = model();
        m.toggle_switch(1);
        m.refresh();
        assert!(m.state.switches[1]);
        assert_eq!(m.state.leds[1], 255);
        m.toggle_switch(1);
        m.refresh();
        assert!(!m.state.switches[1]);

        m.set_button(0, true);
        m.refresh();
        assert!(m.state.buttons[0]);
        m.set_button(0, false);
        assert!(m.last_error.is_none());
    }

    #[test]
    fn console_accumulates_incrementally_and_survives_restore() {
        let mut m = model();
        m.run_for(5 * Ps::PER_MS);
        m.refresh();
        assert_eq!(m.console[1], "zynqsim demo ready\r\n");
        m.refresh();
        assert_eq!(
            m.console[1], "zynqsim demo ready\r\n",
            "no duplicate delivery"
        );

        m.send_uart(1, "x", true);
        m.run_for(5 * Ps::PER_MS);
        m.refresh();
        assert!(m.console[1].ends_with("x\r\n"));

        m.snapshot_save();
        m.run_for(Ps::PER_S);
        m.snapshot_restore();
        // After restore the console is rebuilt from the restored stream.
        assert!(m.console[1].starts_with("zynqsim demo ready"));
        assert!(m.console[1].ends_with("x\r\n"));
    }

    #[test]
    fn errors_are_surfaced_not_swallowed() {
        let mut m = model();
        m.toggle_switch(40);
        assert!(
            m.last_error
                .as_deref()
                .is_some_and(|e| e.contains("out of range"))
        );
        m.last_error = None;
        m.set_button(40, true);
        assert!(
            m.last_error
                .as_deref()
                .is_some_and(|e| e.contains("out of range"))
        );
        m.toggle_switch(0);
        assert!(m.last_error.is_none(), "a later success clears the error");
        assert!(m.cpu_unavailable_reason(0).contains("demo"));
        assert!(m.pl_unavailable_reason().contains("RTL backend"));
    }

    fn tools_present() -> bool {
        let qemu = std::env::var_os("ZYNQSIM_QEMU").is_some()
            || sim_api::locate_binary("qemu-system-aarch64").is_some();
        let vivado =
            std::env::var_os("ZYNQSIM_VIVADO").is_some() || sim_api::find_vivado().is_some();
        let dpi = sim_api::find_dpi_lib().is_some();
        qemu && vivado && dpi
    }

    /// The whole GUI path for the audio demo: build in the background, swap the
    /// engine in, run, and receive the codec's audio and the board media state.
    #[test]
    fn a_project_loads_in_the_background_and_its_audio_reaches_the_model() {
        if !tools_present() {
            eprintln!("SKIPPED: needs QEMU, Vivado XSim and the DPI library");
            return;
        }
        let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/audio-demo/project.toml");
        let draft = crate::loader::ProjectDraft::load(&file).unwrap();
        let mut m = model();
        assert_eq!(m.status.ps_backend, "demo");
        assert!(!m.has_media());
        m.audio_wanted = true;
        m.start_load(
            draft.to_project().unwrap(),
            draft.dir.clone(),
            "audio demo".to_string(),
        );
        assert!(m.loading().is_some());
        // The window stays usable while XSim compiles: refresh keeps working.
        let start = std::time::Instant::now();
        while m.generation == 0 {
            m.refresh();
            assert!(m.load_error.is_none(), "{:?}", m.load_error);
            assert!(start.elapsed().as_secs() < 240, "load did not finish");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(m.loading().is_none());
        assert_eq!(m.status.ps_backend, "qemu");
        assert_eq!(m.status.pl_backend, "xsim");
        assert!(m.has_media());
        assert!(m.audio_wanted, "UI settings survive the swap");

        m.run_for(90 * Ps::PER_MS);
        m.refresh();
        let media = m.status.media.clone().expect("media status");
        assert_eq!(media.codec_writes, 8, "{media:?}");
        assert!(media.audio_frames >= 2000, "{media:?}");
        assert!(m.audio_level > 0.3, "level {}", m.audio_level);
        let pcm = m.take_audio();
        assert!(pcm.len() >= 4000, "{} samples", pcm.len());
        assert!(pcm.iter().any(|&s| s.abs() > 10_000));
        assert!(m.take_audio().is_empty(), "taken once");
        assert!(
            m.console[1].contains("RESULT: ALL PASS"),
            "{}",
            m.console[1]
        );
    }

    #[test]
    fn hdmi_frames_reach_the_model_as_rgba() {
        if !tools_present() {
            eprintln!("SKIPPED: needs QEMU, Vivado XSim and the DPI library");
            return;
        }
        let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/hdmi-demo/project.toml");
        let draft = crate::loader::ProjectDraft::load(&file).unwrap();
        let mut m = model();
        m.start_load(
            draft.to_project().unwrap(),
            draft.dir.clone(),
            "hdmi".into(),
        );
        let start = std::time::Instant::now();
        while m.generation == 0 {
            m.refresh();
            assert!(m.load_error.is_none(), "{:?}", m.load_error);
            assert!(start.elapsed().as_secs() < 240, "load did not finish");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        m.run_for(40 * Ps::PER_MS);
        std::thread::sleep(std::time::Duration::from_millis(150)); // fetch throttle
        m.refresh();
        let v = m.video.clone().expect("a frame arrived");
        assert_eq!((v.width, v.height), (320, 240));
        assert_eq!(v.rgba.len(), 320 * 240 * 4);
        // The firmware's test card: red grows with x, green with y.
        let px = |x: usize, y: usize| {
            let i = (y * 320 + x) * 4;
            (v.rgba[i], v.rgba[i + 1], v.rgba[i + 2], v.rgba[i + 3])
        };
        assert_eq!(px(0, 0), (0, 0, 0, 255));
        assert_eq!(px(319, 0), (254, 0, 63, 255));
        assert_eq!(px(0, 239), (0, 253, 239, 255));

        // Switch the HDMI RX picture from the model, as the panel does.
        m.set_video_source(sim_api::VideoSpec::Gradient {
            width: 320,
            height: 240,
        });
        assert!(m.last_error.is_none(), "{:?}", m.last_error);
    }

    #[test]
    fn registers_appear_in_the_model_when_the_guest_stops() {
        let qemu = std::env::var_os("ZYNQSIM_QEMU").is_some()
            || sim_api::locate_binary("qemu-system-aarch64").is_some();
        if !qemu {
            eprintln!("SKIPPED: needs the Xilinx QEMU");
            return;
        }
        let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/math-debug/project.toml");
        let draft = crate::loader::ProjectDraft::load(&file).unwrap();
        let mut m = model();
        m.start_load(draft.to_project().unwrap(), draft.dir.clone(), "dbg".into());
        let start = std::time::Instant::now();
        while m.generation == 0 {
            m.refresh();
            assert!(m.load_error.is_none(), "{:?}", m.load_error);
            assert!(start.elapsed().as_secs() < 60, "load did not finish");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(m.cpu_regs[0].is_none(), "nothing before the first stop");
        m.run_for(Ps::PER_MS);
        m.refresh();
        let r = m.cpu_regs[0].clone().expect("registers while stopped");
        assert_eq!(r.r[15], 0x0010_058c);
        assert_eq!(r.pc_symbol.as_deref(), Some("Xil_Out32"));
        assert!(m.status.debug.as_ref().unwrap().stopped);
        // Continuing clears them.
        m.run_for(Ps::PER_MS);
        m.refresh();
        assert_eq!(m.cpu_regs[0].as_ref().unwrap().r[0], 0xF800_0900);
    }

    #[test]
    fn reset_uses_the_seed_box() {
        let mut m = model();
        m.seed_text = "42".to_string();
        m.reset();
        m.refresh();
        assert_eq!(m.status.seed, 42);
        m.seed_text = "not a number".to_string();
        m.reset();
        m.refresh();
        assert_eq!(m.status.seed, 42, "unparseable seed keeps the current one");
    }
}
