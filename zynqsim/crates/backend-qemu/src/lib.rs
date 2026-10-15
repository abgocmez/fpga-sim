//! Adapter that runs Xilinx QEMU (Zynq-7000 machine) as the PS and attaches a
//! [`PlTarget`] to its remote-port interface.
//!
//! ## Time
//!
//! QEMU's virtual time is the simulation clock. With `-icount` it advances
//! by a fixed amount per instruction, independent of the host, and QEMU sends
//! a *sync* packet every quantum. It blocks until the sync is answered, so
//! **not answering a sync pauses the machine deterministically**.
//! [`QemuMachine::run_until`] answers syncs up to a target time and holds the
//! first one beyond it.
//!
//! ## What is and is not modelled
//!
//! Fidelity is F0 plus instruction-count timing (prompt §2.4/§5): QEMU is a
//! functional model, not a cycle-accurate Cortex-A9.

mod gdb;
pub use gdb::{BreakKind, GdbClient, GdbError, Regs, Stop};
pub mod net;

use interop::{
    BusAccess, BusCmd, Interrupt, Packet, Session, SessionError, Sync, cap, flags, resp,
};
use pl_api::{
    BusError, BusResult, DEV_M_AXI_GP0, DEV_M_AXI_GP1, DEV_WIRES_PL_TO_PS, DEV_WIRES_PS_TO_PL,
    PlTarget, PsMasterPort, PsPort,
};
use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How UART1 reaches the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartMode {
    /// QEMU appends target output to `uart1.log`. The write happens in the
    /// vCPU thread, so at a sync boundary every byte emitted before it is
    /// already in the file: the output is **deterministically** attributed to
    /// the right quantum. The host cannot send input.
    File,
    /// A TCP socket in both directions. Input works, but output timing is
    /// attributed on a best-effort basis (the host reads the socket
    /// asynchronously) and input delivery is not tied to virtual time.
    Tcp,
}

/// How to start QEMU.
#[derive(Clone, Debug)]
pub struct QemuConfig {
    /// `qemu-system-aarch64` (Xilinx build).
    pub qemu: PathBuf,
    /// Co-simulation device tree blob (`zynq-arm-cosim.dtb`).
    pub dtb: PathBuf,
    /// Software for core 0.
    pub elf0: PathBuf,
    /// Optional software for core 1.
    pub elf1: Option<PathBuf>,
    /// `-sync-quantum` in nanoseconds of virtual time.
    pub quantum_ns: u64,
    /// `-icount shift=N`: each instruction advances virtual time by `2^N` ns.
    /// `None` runs on the host clock, which is **not** deterministic.
    pub icount_shift: Option<u32>,
    /// Directory for `qemu.out.log` / `qemu.err.log` / `uart1.log`.
    pub work_dir: PathBuf,
    /// How UART1 is connected.
    pub uart: UartMode,
    /// How long to wait for QEMU to open its sockets.
    pub connect_timeout: Duration,
    /// Attach QEMU's gdb stub (`-gdb`) and start with the CPUs stopped
    /// (`-S`): breakpoints are planted before the first instruction, so every
    /// stop is a function of the guest alone. A machine under a debugger is
    /// **not** timing-reproducible: attaching shifts virtual time by up to a
    /// few microseconds and between runs by a few hundred nanoseconds.
    pub debug: bool,
    /// Attach GEM0 to a user-mode network and capture its frames
    /// (`work_dir/net.pcap`); see [`net`].
    pub net: Option<net::NetConfig>,
}

impl QemuConfig {
    /// A deterministic configuration: 1 us quantum, `-icount shift=1`.
    pub fn new(qemu: PathBuf, dtb: PathBuf, elf0: PathBuf, work_dir: PathBuf) -> QemuConfig {
        QemuConfig {
            qemu,
            dtb,
            elf0,
            elf1: None,
            quantum_ns: 1000,
            icount_shift: Some(1),
            work_dir,
            uart: UartMode::File,
            connect_timeout: Duration::from_secs(20),
            debug: false,
            net: None,
        }
    }

    /// Locates the co-simulation DTB for a Vitis QEMU on the Windows layout
    /// `<emulation>/qemu_win/qemu/qemu-system-aarch64.exe`.
    ///
    /// VERIFY: other platforms' Vitis layouts have not been checked; pass the
    /// DTB explicitly there.
    pub fn dtb_for(qemu: &Path) -> Option<PathBuf> {
        let emu = qemu.parent()?.parent()?.parent()?;
        let dtb = emu.join("dtbs").join("zynq").join("zynq-arm-cosim.dtb");
        dtb.is_file().then_some(dtb)
    }
}

/// Errors from the QEMU adapter.
#[derive(Debug)]
pub enum QemuError {
    /// A file the configuration names does not exist.
    MissingFile(PathBuf),
    /// QEMU could not be started.
    Spawn(std::io::Error),
    /// QEMU exited; the tail of its stderr is included.
    Exited {
        /// Exit status text.
        status: String,
        /// Last lines of `qemu.err.log`.
        stderr: String,
    },
    /// A socket could not be set up.
    Io(std::io::Error),
    /// The remote-port session failed.
    Rp(SessionError),
    /// The wall-clock limit passed without reaching the target.
    Timeout {
        /// Simulated time QEMU had reached, in nanoseconds.
        now_ns: u64,
        /// The target it was asked to reach.
        target_ns: u64,
        /// What had been served so far, for diagnosing a stall.
        stats: Stats,
    },
    /// A memory access through the remote-port path failed.
    Memory(String),
    /// The debugger connection failed.
    Debug(GdbError),
}

impl fmt::Display for QemuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QemuError::MissingFile(p) => write!(f, "file not found: {}", p.display()),
            QemuError::Spawn(e) => write!(f, "cannot start QEMU: {e}"),
            QemuError::Exited { status, stderr } => {
                write!(f, "QEMU exited ({status}); stderr: {stderr}")
            }
            QemuError::Io(e) => write!(f, "QEMU socket error: {e}"),
            QemuError::Rp(e) => write!(f, "{e}"),
            QemuError::Timeout {
                now_ns,
                target_ns,
                stats,
            } => write!(
                f,
                "timed out waiting for QEMU: reached {now_ns} ns of {target_ns} ns ({} syncs, {} reads, {} writes, {} errors)",
                stats.syncs, stats.reads, stats.writes, stats.errors
            ),
            QemuError::Memory(m) => write!(f, "QEMU memory access: {m}"),
            QemuError::Debug(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for QemuError {}

impl From<SessionError> for QemuError {
    fn from(e: SessionError) -> Self {
        QemuError::Rp(e)
    }
}

/// Why [`QemuMachine::run_until`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEnd {
    /// QEMU reached the target virtual time and is paused at a sync.
    ReachedTime,
    /// QEMU closed the connection (it exited).
    PeerClosed,
    /// The guest stopped on a breakpoint, watchpoint or step under the
    /// debugger. Virtual time is frozen until [`QemuMachine::resume`].
    Stopped(Stop),
}

/// Where the debugged machine is in its run/stop cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DbgState {
    /// CPUs stopped: at start-up or on a stop.
    Stopped,
    /// Free running.
    Running,
    /// Stepping off the breakpoint at this address; it is re-planted after.
    StepOver(u32),
}

struct Dbg {
    client: GdbClient,
    state: DbgState,
    last: Option<Stop>,
    /// Execution breakpoints currently planted, for step-over on resume.
    exec: Vec<u32>,
}

/// Errors of the debugging calls.
#[derive(Debug)]
pub enum DebugError {
    /// The machine was not started with `QemuConfig::debug`.
    NotAttached,
    /// The machine has not stopped on its own: registers can only be read
    /// while it is stopped at a breakpoint, watchpoint or step.
    NotStopped,
    /// The stub failed.
    Gdb(GdbError),
}

impl fmt::Display for DebugError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DebugError::NotAttached => write!(f, "the machine was started without a debugger"),
            DebugError::NotStopped => write!(
                f,
                "the machine is not stopped at a breakpoint; registers are only available there"
            ),
            DebugError::Gdb(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DebugError {}

impl From<GdbError> for DebugError {
    fn from(e: GdbError) -> DebugError {
        DebugError::Gdb(e)
    }
}

/// Counters, for tests and the status view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Sync packets answered.
    pub syncs: u64,
    /// Bus reads served.
    pub reads: u64,
    /// Bus writes served.
    pub writes: u64,
    /// Accesses answered with an error.
    pub errors: u64,
    /// PS-to-PL wire updates received.
    pub wires: u64,
}

enum Uart {
    File {
        path: PathBuf,
        offset: u64,
    },
    Tcp {
        rx: Arc<Mutex<Vec<u8>>>,
        tx: TcpStream,
    },
}

/// A running QEMU instance with its remote-port session and UART1.
pub struct QemuMachine {
    child: Child,
    rp: Session,
    uart: Uart,
    net: Option<net::PcapTail>,
    pending: VecDeque<Packet>,
    held_sync: Option<Sync>,
    now_ns: u64,
    err_log: PathBuf,
    exited: bool,
    stats: Stats,
    dbg: Option<Dbg>,
    /// Port of the stub until `attach_debugger` connects.
    gdb_port: Option<u16>,
}

fn free_port() -> std::io::Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

impl QemuMachine {
    /// Starts QEMU and completes the remote-port handshake.
    pub fn spawn(cfg: &QemuConfig) -> Result<QemuMachine, QemuError> {
        for p in [&cfg.qemu, &cfg.dtb, &cfg.elf0] {
            if !p.is_file() {
                return Err(QemuError::MissingFile(p.clone()));
            }
        }
        if let Some(p) = &cfg.elf1 {
            if !p.is_file() {
                return Err(QemuError::MissingFile(p.clone()));
            }
        }
        std::fs::create_dir_all(&cfg.work_dir).map_err(QemuError::Io)?;
        let rp_port = free_port().map_err(QemuError::Io)?;
        let uart_port = free_port().map_err(QemuError::Io)?;
        let uart_log = cfg.work_dir.join("uart1.log");
        // A stale log from a previous run would be read as new output.
        let _ = std::fs::remove_file(&uart_log);

        // Same shape as the prototype's run_cosim_win.ps1. Windows QEMU has no
        // unix sockets, and the co-sim DTB has no chrdev-id, so the
        // remote-port device gets its chardev through -global.
        let mut args: Vec<String> = vec![
            "-M".into(),
            "arm-generic-fdt-7series".into(),
            "-display".into(),
            "none".into(),
            "-hw-dtb".into(),
            cfg.dtb.display().to_string(),
            "-serial".into(),
            "null".into(),
            "-serial".into(),
            match cfg.uart {
                UartMode::File => format!("file:{}", uart_log.display()),
                UartMode::Tcp => format!("tcp:127.0.0.1:{uart_port},server=on,wait=on"),
            },
            "-device".into(),
            format!("loader,file={},cpu-num=0", cfg.elf0.display()),
            "-chardev".into(),
            format!("socket,id=pl-rp,host=127.0.0.1,port={rp_port},server=on,wait=on"),
            "-global".into(),
            "remote-port.chardev=pl-rp".into(),
            "-sync-quantum".into(),
            cfg.quantum_ns.to_string(),
        ];
        if let Some(shift) = cfg.icount_shift {
            args.push("-icount".into());
            args.push(format!("shift={shift},align=off,sleep=off"));
        }
        let gdb_port = if cfg.debug {
            let p = free_port().map_err(QemuError::Io)?;
            args.push("-gdb".into());
            args.push(format!("tcp:127.0.0.1:{p}"));
            args.push("-S".into());
            Some(p)
        } else {
            None
        };
        if let Some(e) = &cfg.elf1 {
            args.push("-device".into());
            args.push(format!("loader,file={},cpu-num=1", e.display()));
        }
        let net_log = cfg.work_dir.join("net.pcap");
        let _ = std::fs::remove_file(&net_log);
        match &cfg.net {
            Some(n) => args.extend(n.qemu_args(&net_log)),
            // QEMU otherwise attaches an implicit user-mode network (and, with
            // `restrict` off, a path to the host's network) that the host
            // could neither see nor reproduce.
            None => args.extend(["-net".to_string(), "none".to_string()]),
        }

        let out_log = cfg.work_dir.join("qemu.out.log");
        let err_log = cfg.work_dir.join("qemu.err.log");
        let mut cmd = Command::new(&cfg.qemu);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(File::create(&out_log).map_err(QemuError::Io)?)
            .stderr(File::create(&err_log).map_err(QemuError::Io)?);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(QemuError::Spawn)?;

        // QEMU opens and waits on its sockets one at a time, in an order that
        // depends on its option processing (-chardev before -serial), so try
        // every socket until each has connected rather than assuming an order.
        let (uart, rp_stream) = match cfg.uart {
            UartMode::File => {
                let rp = match connect_all(&mut child, &[rp_port], cfg.connect_timeout, &err_log) {
                    Ok(mut v) => v.remove(0),
                    Err(e) => {
                        let _ = child.kill();
                        return Err(e);
                    }
                };
                (
                    Uart::File {
                        path: uart_log,
                        offset: 0,
                    },
                    rp,
                )
            }
            UartMode::Tcp => {
                let mut v = match connect_all(
                    &mut child,
                    &[uart_port, rp_port],
                    cfg.connect_timeout,
                    &err_log,
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = child.kill();
                        return Err(e);
                    }
                };
                let rp = v.remove(1);
                let uart_stream = v.remove(0);
                let rx = Arc::new(Mutex::new(Vec::new()));
                let rx2 = Arc::clone(&rx);
                let mut r = uart_stream.try_clone().map_err(QemuError::Io)?;
                std::thread::Builder::new()
                    .name("zynqsim-qemu-uart".into())
                    .spawn(move || {
                        let mut buf = [0u8; 1024];
                        while let Ok(n) = r.read(&mut buf) {
                            if n == 0 {
                                break;
                            }
                            if let Ok(mut v) = rx2.lock() {
                                v.extend_from_slice(&buf[..n]);
                            }
                        }
                    })
                    .map_err(QemuError::Io)?;
                (
                    Uart::Tcp {
                        rx,
                        tx: uart_stream,
                    },
                    rp,
                )
            }
        };
        let mut rp = Session::from_stream(rp_stream).map_err(QemuError::Io)?;
        rp.send_hello(&[cap::BUSACCESS_EXT_BASE, cap::WIRE_POSTED_UPDATES])?;
        Ok(QemuMachine {
            child,
            rp,
            uart,
            net: cfg.net.is_some().then(|| net::PcapTail::new(net_log)),
            pending: VecDeque::new(),
            held_sync: None,
            now_ns: 0,
            err_log,
            exited: false,
            stats: Stats::default(),
            dbg: None,
            gdb_port,
        })
    }

    /// Connects the debugger. Must be called once after `spawn` when
    /// `QemuConfig::debug` is set, before the first `run_until`. The CPUs are
    /// stopped (`-S`), which is when the stub answers.
    pub fn attach_debugger(&mut self, pl: &mut dyn PlTarget) -> Result<(), QemuError> {
        let Some(port) = self.gdb_port.take() else {
            return Ok(());
        };
        let _ = pl;
        let client = GdbClient::connect(port).map_err(QemuError::Debug)?;
        self.dbg = Some(Dbg {
            client,
            state: DbgState::Stopped,
            last: None,
            exec: Vec::new(),
        });
        Ok(())
    }

    /// QEMU virtual time in nanoseconds: the timestamp of the sync it is
    /// currently paused at, or of the latest access.
    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }

    /// Counters.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// True once QEMU has closed the connection.
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// Removes and returns UART1 output produced since the last call.
    pub fn uart_take(&mut self) -> Vec<u8> {
        match &mut self.uart {
            Uart::File { path, offset } => {
                let Ok(mut f) = File::open(path) else {
                    return Vec::new();
                };
                if f.seek(SeekFrom::Start(*offset)).is_err() {
                    return Vec::new();
                }
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).is_ok() {
                    *offset += buf.len() as u64;
                }
                buf
            }
            Uart::Tcp { rx, .. } => rx
                .lock()
                .map(|mut v| std::mem::take(&mut *v))
                .unwrap_or_default(),
        }
    }

    /// Frames GEM0 sent or received since the last call, in order. Empty
    /// without [`QemuConfig::net`]. Complete at every sync boundary: QEMU's
    /// network stack runs in the vCPU thread, which is stopped there.
    pub fn net_take(&mut self) -> Vec<net::RawFrame> {
        self.net
            .as_mut()
            .map(net::PcapTail::poll)
            .unwrap_or_default()
    }

    /// Sends bytes to UART1. Only available in [`UartMode::Tcp`].
    ///
    /// Delivery time relative to guest virtual time is **not** deterministic:
    /// the bytes reach QEMU's main loop on the host's schedule.
    pub fn uart_write(&mut self, data: &[u8]) -> std::io::Result<()> {
        match &mut self.uart {
            Uart::Tcp { tx, .. } => tx.write_all(data),
            Uart::File { .. } => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "UART input needs UartMode::Tcp; the deterministic file mode is output only",
            )),
        }
    }

    /// Serves the PL until QEMU's virtual time reaches `target_ns`.
    ///
    /// Gives up with [`QemuError::Timeout`] after `wall` of real time.
    pub fn run_until(
        &mut self,
        target_ns: u64,
        pl: &mut dyn PlTarget,
        wall: Duration,
    ) -> Result<RunEnd, QemuError> {
        if self.exited {
            return Ok(RunEnd::PeerClosed);
        }
        if let Some(d) = &self.dbg {
            if d.state == DbgState::Stopped {
                if let Some(stop) = d.last {
                    // Stopped on a breakpoint: nothing can run until `resume`.
                    return Ok(RunEnd::Stopped(stop));
                }
            }
        }
        let start = Instant::now();
        loop {
            if let Some(s) = self.held_sync.take() {
                if s.timestamp <= target_ns {
                    self.ack_sync(&s, pl)?;
                } else {
                    self.now_ns = self.now_ns.max(s.timestamp);
                    self.held_sync = Some(s);
                    return Ok(RunEnd::ReachedTime);
                }
            }
            let pkt = match self.pending.pop_front() {
                Some(p) => p,
                None => match self.rp.recv_timeout(Duration::from_millis(100)) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        self.check_alive()?;
                        if let Some(stop) = self.poll_debugger()? {
                            return Ok(RunEnd::Stopped(stop));
                        }
                        if start.elapsed() > wall {
                            return Err(self.timeout(target_ns));
                        }
                        continue;
                    }
                    Err(SessionError::Closed) => {
                        self.exited = true;
                        return Ok(RunEnd::PeerClosed);
                    }
                    Err(e) => return Err(e.into()),
                },
            };
            // Nothing happens beyond the target: a request stamped later than
            // it stays unanswered, which keeps QEMU blocked exactly as a held
            // sync does. A CPU polling a register would otherwise run on for as
            // long as it likes inside one call.
            let ts = match &pkt {
                Packet::Bus(b) if !b.is_response() => Some(b.timestamp),
                Packet::Interrupt(i) if i.flags & flags::RESPONSE == 0 => Some(i.timestamp),
                _ => None,
            };
            if let Some(ts) = ts.filter(|&t| t > target_ns) {
                self.now_ns = self.now_ns.max(ts);
                self.pending.push_front(pkt);
                return Ok(RunEnd::ReachedTime);
            }
            self.dispatch(pkt, target_ns, pl)?;
            if start.elapsed() > wall {
                return Err(self.timeout(target_ns));
            }
        }
    }

    fn timeout(&self, target_ns: u64) -> QemuError {
        QemuError::Timeout {
            now_ns: self.now_ns,
            target_ns,
            stats: self.stats,
        }
    }

    /// Looks for a stop reply from the debugger. A stop that only finishes a
    /// step off a breakpoint re-plants it and continues (not reported).
    fn poll_debugger(&mut self) -> Result<Option<Stop>, QemuError> {
        let Some(d) = &mut self.dbg else {
            return Ok(None);
        };
        if d.state == DbgState::Stopped {
            return Ok(None);
        }
        let Some(stop) = d.client.poll_stop().map_err(QemuError::Debug)? else {
            return Ok(None);
        };
        if let DbgState::StepOver(addr) = d.state {
            d.client
                .set_break(BreakKind::Exec, true, addr, 4)
                .map_err(QemuError::Debug)?;
            d.client.cont().map_err(QemuError::Debug)?;
            d.state = DbgState::Running;
            return Ok(None);
        }
        d.state = DbgState::Stopped;
        d.last = Some(stop);
        Ok(Some(stop))
    }

    /// Port of the gdb stub before `attach_debugger` has connected.
    pub fn debug_port(&self) -> Option<u16> {
        self.gdb_port
    }

    /// True when the machine was started under the debugger.
    pub fn debug_attached(&self) -> bool {
        self.dbg.is_some()
    }

    /// The stop the machine is sitting at, if it stopped on its own.
    pub fn debug_stop(&self) -> Option<Stop> {
        self.dbg
            .as_ref()
            .filter(|d| d.state == DbgState::Stopped)
            .and_then(|d| d.last)
    }

    /// Starts or continues the CPUs. From a breakpoint it first steps off it,
    /// as gdb does, so the same breakpoint does not fire again at once.
    pub fn resume(&mut self) -> Result<(), DebugError> {
        let d = self.dbg.as_mut().ok_or(DebugError::NotAttached)?;
        if d.state != DbgState::Stopped {
            return Ok(());
        }
        let thread = d.last.map_or(1, |s| s.thread);
        let at_break = match d.last {
            Some(_) => {
                let pc = d.client.regs(thread)?.r[15];
                d.exec.contains(&pc).then_some(pc)
            }
            None => None,
        };
        d.last = None;
        match at_break {
            Some(pc) => {
                d.client.set_break(BreakKind::Exec, false, pc, 4)?;
                d.client.step()?;
                d.state = DbgState::StepOver(pc);
            }
            None => {
                d.client.cont()?;
                d.state = DbgState::Running;
            }
        }
        Ok(())
    }

    /// Plants (`insert`) or removes a breakpoint or watchpoint. While the
    /// machine is stopped this is exact; otherwise the request is queued in
    /// the stub and takes effect as soon as its main loop runs, within the
    /// next quantum but at a wall-clock-dependent instant.
    pub fn debug_break(
        &mut self,
        kind: BreakKind,
        insert: bool,
        addr: u32,
        len: u32,
    ) -> Result<(), DebugError> {
        let d = self.dbg.as_mut().ok_or(DebugError::NotAttached)?;
        if d.state == DbgState::Stopped {
            d.client.set_break(kind, insert, addr, len)?;
        } else {
            let op = if insert { 'Z' } else { 'z' };
            let len = if kind == BreakKind::Exec {
                4
            } else {
                len.max(1)
            };
            let code = match kind {
                BreakKind::Exec => 0,
                BreakKind::Write => 2,
                BreakKind::Read => 3,
                BreakKind::Access => 4,
            };
            d.client
                .send_async(&format!("{op}{code},{addr:x},{len:x}"))?;
        }
        if kind == BreakKind::Exec {
            if insert {
                if !d.exec.contains(&addr) {
                    d.exec.push(addr);
                }
            } else {
                d.exec.retain(|&a| a != addr);
            }
        }
        Ok(())
    }

    /// Registers of `core` (0 or 1). Only while stopped.
    pub fn debug_regs(&mut self, core: u8) -> Result<Regs, DebugError> {
        let d = self.dbg.as_mut().ok_or(DebugError::NotAttached)?;
        if d.state != DbgState::Stopped || d.last.is_none() {
            return Err(DebugError::NotStopped);
        }
        Ok(d.client.regs(core + 1)?)
    }

    fn check_alive(&mut self) -> Result<(), QemuError> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.exited = true;
                Err(QemuError::Exited {
                    status: status.to_string(),
                    stderr: tail(&self.err_log),
                })
            }
            _ => Ok(()),
        }
    }

    fn ack_sync(&mut self, s: &Sync, pl: &mut dyn PlTarget) -> Result<(), QemuError> {
        self.now_ns = self.now_ns.max(s.timestamp);
        {
            let mut ps = PsCtx::new(&mut self.rp, &mut self.pending, self.now_ns);
            pl.advance(s.timestamp, &mut ps);
        }
        self.rp.send(&Packet::Sync(Sync {
            id: s.id,
            flags: s.flags | flags::RESPONSE,
            dev: s.dev,
            timestamp: s.timestamp,
        }))?;
        self.stats.syncs += 1;
        Ok(())
    }

    fn dispatch(
        &mut self,
        pkt: Packet,
        target_ns: u64,
        pl: &mut dyn PlTarget,
    ) -> Result<(), QemuError> {
        match pkt {
            Packet::Sync(s) if s.flags & flags::RESPONSE == 0 => {
                if s.timestamp <= target_ns {
                    self.ack_sync(&s, pl)?;
                } else {
                    self.held_sync = Some(s);
                }
            }
            Packet::Bus(b) if !b.is_response() => {
                self.now_ns = self.now_ns.max(b.timestamp);
                self.serve_bus(b, pl)?;
            }
            Packet::Interrupt(i) if i.flags & flags::RESPONSE == 0 => {
                self.now_ns = self.now_ns.max(i.timestamp);
                self.stats.wires += 1;
                pl.wire(i.line, i.val != 0, i.timestamp);
                if i.flags & flags::POSTED == 0 {
                    self.rp.send(&Packet::Interrupt(Interrupt {
                        flags: i.flags | flags::RESPONSE,
                        ..i
                    }))?;
                }
            }
            // Hello is recorded by the session; responses without a waiter,
            // cfg, ATS and nop carry nothing we act on.
            _ => {}
        }
        Ok(())
    }

    fn serve_bus(&mut self, b: BusAccess, pl: &mut dyn PlTarget) -> Result<(), QemuError> {
        let gp = match b.dev {
            DEV_M_AXI_GP0 => Some(0u8),
            DEV_M_AXI_GP1 => Some(1u8),
            _ => None,
        };
        let len = b.len as usize;
        let mut data = vec![0u8; if b.cmd == BusCmd::Read { len } else { 0 }];
        let result: BusResult<()> = match gp {
            None => Err(BusError::Decode),
            Some(gp) => {
                let mut ps = PsCtx::new(&mut self.rp, &mut self.pending, self.now_ns);
                match b.cmd {
                    BusCmd::Read => pl.read(gp, b.addr, &mut data, b.timestamp, &mut ps),
                    BusCmd::Write => {
                        if b.data.len() < len {
                            Err(BusError::Slave)
                        } else {
                            pl.write(gp, b.addr, &b.data[..len], b.timestamp, &mut ps)
                        }
                    }
                }
            }
        };
        match b.cmd {
            BusCmd::Read => self.stats.reads += 1,
            BusCmd::Write => self.stats.writes += 1,
        }
        let code = match result {
            Ok(()) => resp::OK,
            Err(e) => {
                self.stats.errors += 1;
                match e {
                    BusError::Decode => resp::ADDR_ERROR,
                    BusError::Slave => resp::BUS_GENERIC_ERROR,
                }
            }
        };
        if !b.is_posted() {
            let r = b.response(code, data, b.timestamp);
            self.rp.send(&Packet::Bus(r))?;
        }
        Ok(())
    }

    /// Reads guest physical memory through the remote-port memory path (the
    /// same path a PL DMA master uses, `S_AXI_HP0`).
    ///
    /// Works while the machine is paused at a sync, which is why this is not
    /// done through the QEMU monitor: with `-icount` the sync timer runs in
    /// the vCPU thread holding QEMU's global lock, so the monitor cannot
    /// answer while the machine is held.
    pub fn read_memory(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, QemuError> {
        const CHUNK: usize = 4096;
        if len > 64 * 1024 * 1024 {
            return Err(QemuError::Memory("read larger than 64 MiB".to_string()));
        }
        let mut out = vec![0u8; len];
        let mut ps = PsCtx::new(&mut self.rp, &mut self.pending, self.now_ns);
        for (i, chunk) in out.chunks_mut(CHUNK).enumerate() {
            let a = addr + (i * CHUNK) as u64;
            ps.mem_read(PsMasterPort::SAxiHp0, a, chunk)
                .map_err(|e| QemuError::Memory(format!("read at {a:#x}: {e}")))?;
        }
        Ok(out)
    }

    /// Writes guest physical memory through the same path as
    /// [`QemuMachine::read_memory`].
    pub fn write_memory(&mut self, addr: u64, data: &[u8]) -> Result<(), QemuError> {
        const CHUNK: usize = 4096;
        let mut ps = PsCtx::new(&mut self.rp, &mut self.pending, self.now_ns);
        for (i, chunk) in data.chunks(CHUNK).enumerate() {
            let a = addr + (i * CHUNK) as u64;
            ps.mem_write(PsMasterPort::SAxiHp0, a, chunk)
                .map_err(|e| QemuError::Memory(format!("write at {a:#x}: {e}")))?;
        }
        Ok(())
    }

    /// Stops QEMU. Safe to call more than once.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.exited = true;
    }
}

impl Drop for QemuMachine {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The PL's view of the PS during a callback.
struct PsCtx<'a> {
    rp: &'a mut Session,
    pending: &'a mut VecDeque<Packet>,
    now_ns: u64,
}

impl<'a> PsCtx<'a> {
    fn new(rp: &'a mut Session, pending: &'a mut VecDeque<Packet>, now_ns: u64) -> PsCtx<'a> {
        PsCtx {
            rp,
            pending,
            now_ns,
        }
    }

    /// Sends a request and waits for its response, queueing anything else that
    /// arrives meanwhile (including new PS accesses) for the main loop.
    fn wait_response(&mut self, id: u32) -> Result<Packet, BusError> {
        loop {
            match self.rp.recv() {
                Ok(Packet::Bus(r)) if r.is_response() && r.id == id => {
                    return Ok(Packet::Bus(r));
                }
                Ok(Packet::Interrupt(r)) if r.flags & flags::RESPONSE != 0 && r.id == id => {
                    return Ok(Packet::Interrupt(r));
                }
                Ok(other) => self.pending.push_back(other),
                Err(_) => return Err(BusError::Slave),
            }
        }
    }

    fn bus(
        &mut self,
        cmd: BusCmd,
        port: PsMasterPort,
        addr: u64,
        len: usize,
        data: &[u8],
    ) -> BusResult<Vec<u8>> {
        let id = self.rp.alloc_id();
        let req = BusAccess {
            cmd,
            id,
            flags: 0,
            dev: port.device(),
            timestamp: self.now_ns,
            attributes: 0,
            addr,
            len: len as u32,
            width: 0,
            stream_width: len as u32,
            master_id: 0,
            data: data.to_vec(),
            byte_enable: Vec::new(),
            ext: self.rp.peer_ext_base(),
        };
        self.rp
            .send(&Packet::Bus(req))
            .map_err(|_| BusError::Slave)?;
        match self.wait_response(id)? {
            Packet::Bus(r) => match r.response_code() {
                resp::OK => Ok(r.data),
                resp::ADDR_ERROR => Err(BusError::Decode),
                _ => Err(BusError::Slave),
            },
            _ => Err(BusError::Slave),
        }
    }
}

impl PsPort for PsCtx<'_> {
    fn mem_read(&mut self, port: PsMasterPort, addr: u64, buf: &mut [u8]) -> BusResult<()> {
        let d = self.bus(BusCmd::Read, port, addr, buf.len(), &[])?;
        if d.len() != buf.len() {
            return Err(BusError::Slave);
        }
        buf.copy_from_slice(&d);
        Ok(())
    }

    fn mem_write(&mut self, port: PsMasterPort, addr: u64, data: &[u8]) -> BusResult<()> {
        self.bus(BusCmd::Write, port, addr, data.len(), data)
            .map(|_| ())
    }

    fn irq(&mut self, line: u32, level: bool, now_ns: u64) -> BusResult<()> {
        let id = self.rp.alloc_id();
        let acked = self.rp.peer_has(cap::WIRE_POSTED_UPDATES);
        let pkt = Interrupt {
            id,
            // A peer that supports acknowledged updates is asked for one so the
            // interrupt is applied before PL time moves on (determinism).
            flags: if acked { 0 } else { flags::POSTED },
            dev: DEV_WIRES_PL_TO_PS,
            timestamp: now_ns,
            vector: 0,
            line,
            val: u8::from(level),
        };
        self.rp
            .send(&Packet::Interrupt(pkt))
            .map_err(|_| BusError::Slave)?;
        if acked {
            self.wait_response(id)?;
        }
        Ok(())
    }
}

fn connect_all(
    child: &mut Child,
    ports: &[u16],
    timeout: Duration,
    err_log: &Path,
) -> Result<Vec<TcpStream>, QemuError> {
    let start = Instant::now();
    let mut conns: Vec<Option<TcpStream>> = ports.iter().map(|_| None).collect();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(QemuError::Exited {
                status: status.to_string(),
                stderr: tail(err_log),
            });
        }
        for (slot, port) in conns.iter_mut().zip(ports) {
            if slot.is_none() {
                if let Ok(s) = TcpStream::connect(("127.0.0.1", *port)) {
                    let _ = s.set_nodelay(true);
                    *slot = Some(s);
                }
            }
        }
        if conns.iter().all(Option::is_some) {
            return Ok(conns.into_iter().flatten().collect());
        }
        if start.elapsed() > timeout {
            return Err(QemuError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "QEMU did not accept connections on ports {ports:?}; stderr: {}",
                    tail(err_log)
                ),
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Last few hundred bytes of a log, for error messages.
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let start = text.len().saturating_sub(600);
    let mut s = start;
    while !text.is_char_boundary(s) {
        s += 1;
    }
    text[s..].trim().to_string()
}

#[doc(hidden)]
pub const _PS_TO_PL_DEVICE: u32 = DEV_WIRES_PS_TO_PL;
