//! Prompt §14.1: the DMA accelerator regression, checked against a **Rust
//! golden model** rather than only the firmware's own PASS prints.
//!
//! Real QEMU (two Cortex-A9 cores running the prototype firmware) drives the
//! RTL `dma_xform` in real XSim. The test sits between them: it watches the
//! PS's register writes (to learn each job's src/dst/len/scale/offset) and the
//! PL's DMA traffic (full data), and checks every word the engine wrote against
//! `dst[i] = src[i] * scale + offset` (wrapping 32-bit; clamped when SAT is set).
//!
//! Needs Xilinx QEMU and Vivado XSim; prints `SKIPPED:` otherwise.

use backend_qemu::{QemuConfig, QemuMachine, RunEnd};
use backend_rtl_xsim::{XsimConfig, XsimPl};
use pl_api::{BusResult, PlTarget, PsMasterPort, PsPort};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const DMA_BASE: u64 = 0x43C0_0000;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn find_qemu() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_QEMU") {
        return Some(PathBuf::from(p));
    }
    let root = PathBuf::from(r"C:\Xilinx\Vitis");
    std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| {
            e.path()
                .join("data")
                .join("emulation")
                .join("qemu_win")
                .join("qemu")
                .join("qemu-system-aarch64.exe")
        })
        .find(|p| p.is_file())
}

fn find_vivado() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_VIVADO") {
        return Some(PathBuf::from(p));
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(r"C:\Xilinx\Vivado")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("settings64.bat").is_file())
        .collect();
    v.sort();
    v.pop()
}

fn dpi_lib() -> PathBuf {
    let lib = workspace()
        .join("target")
        .join("dpi")
        .join("release")
        .join("libxsim_dpi.a");
    if !lib.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args([
                "build",
                "--release",
                "-p",
                "xsim-dpi",
                "--features",
                "freestanding",
            ])
            .current_dir(workspace())
            .status()
            .expect("cargo runs");
        assert!(st.success());
    }
    lib
}

fn work(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-dma-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

// ----------------------------------------------------------------------- golden

#[derive(Default)]
struct Job {
    src: u32,
    dst: u32,
    len: u32,
    scale: u32,
    offset: u32,
    sat: bool,
    /// Source words the PL has read since START, by byte address.
    seen: HashMap<u32, u32>,
}

#[derive(Default)]
struct Golden {
    job: Job,
    jobs: u32,
    words_checked: u64,
    mismatches: Vec<String>,
}

impl Golden {
    fn model(&self, x: u32) -> u32 {
        let raw = x.wrapping_mul(self.job.scale).wrapping_add(self.job.offset);
        if !self.job.sat {
            raw
        } else if raw & 0x8000_0000 != 0 {
            0
        } else if raw > 255 {
            255
        } else {
            raw
        }
    }

    fn reg_write(&mut self, off: u64, v: u32) {
        match off {
            0x00 => {
                self.job.sat = v & 4 != 0;
                if v & 1 != 0 {
                    self.jobs += 1;
                    self.job.seen.clear();
                }
            }
            0x08 => self.job.src = v,
            0x0C => self.job.dst = v,
            0x10 => self.job.len = v,
            0x14 => self.job.scale = v,
            0x18 => self.job.offset = v,
            _ => {}
        }
    }

    fn pl_read(&mut self, addr: u64, data: &[u8]) {
        for (i, w) in data.chunks_exact(4).enumerate() {
            let a = addr as u32 + 4 * i as u32;
            self.job
                .seen
                .insert(a, u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        }
    }

    fn pl_write(&mut self, addr: u64, data: &[u8]) {
        let (dst, src) = (self.job.dst, self.job.src);
        for (i, w) in data.chunks_exact(4).enumerate() {
            let a = addr as u32 + 4 * i as u32;
            let got = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            let in_range = a >= dst && a < dst.wrapping_add(self.job.len * 4);
            if !in_range {
                self.mismatches.push(format!(
                    "job {}: write outside dst range at {a:#x}",
                    self.jobs
                ));
                continue;
            }
            let src_a = src + (a - dst);
            match self.job.seen.get(&src_a) {
                None => self.mismatches.push(format!(
                    "job {}: wrote {a:#x} before reading {src_a:#x}",
                    self.jobs
                )),
                Some(&x) => {
                    let want = self.model(x);
                    self.words_checked += 1;
                    if got != want {
                        self.mismatches.push(format!(
                            "job {}: dst[{}] = {got:#x}, golden {want:#x} (in {x:#x}, *{} +{})",
                            self.jobs,
                            (a - dst) / 4,
                            self.job.scale,
                            self.job.offset
                        ));
                    }
                }
            }
        }
    }
}

/// Forwards PL master traffic to the real PS and shows it to the golden model.
struct Spy<'a> {
    ps: &'a mut dyn PsPort,
    g: &'a mut Golden,
}

impl PsPort for Spy<'_> {
    fn mem_read(&mut self, port: PsMasterPort, addr: u64, buf: &mut [u8]) -> BusResult<()> {
        self.ps.mem_read(port, addr, buf)?;
        self.g.pl_read(addr, buf);
        Ok(())
    }
    fn mem_write(&mut self, port: PsMasterPort, addr: u64, data: &[u8]) -> BusResult<()> {
        self.g.pl_write(addr, data);
        self.ps.mem_write(port, addr, data)
    }
    fn irq(&mut self, line: u32, level: bool, now_ns: u64) -> BusResult<()> {
        self.ps.irq(line, level, now_ns)
    }
}

struct Checked {
    inner: XsimPl,
    g: Golden,
}

impl PlTarget for Checked {
    fn read(
        &mut self,
        gp: u8,
        addr: u64,
        buf: &mut [u8],
        now: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        let mut spy = Spy { ps, g: &mut self.g };
        self.inner.read(gp, addr, buf, now, &mut spy)
    }
    fn write(
        &mut self,
        gp: u8,
        addr: u64,
        data: &[u8],
        now: u64,
        ps: &mut dyn PsPort,
    ) -> BusResult<()> {
        if gp == 0 && data.len() == 4 && (DMA_BASE..DMA_BASE + 0x40).contains(&addr) {
            self.g.reg_write(
                addr - DMA_BASE,
                u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            );
        }
        let mut spy = Spy { ps, g: &mut self.g };
        self.inner.write(gp, addr, data, now, &mut spy)
    }
    fn wire(&mut self, line: u32, level: bool, now: u64) {
        self.inner.wire(line, level, now);
    }
    fn advance(&mut self, now: u64, ps: &mut dyn PsPort) {
        let mut spy = Spy { ps, g: &mut self.g };
        self.inner.advance(now, &mut spy);
    }
}

#[test]
fn dma_results_equal_the_rust_golden_model() {
    let (Some(qemu), Some(vivado)) = (find_qemu(), find_vivado()) else {
        eprintln!("SKIPPED: Xilinx QEMU and/or Vivado not found");
        return;
    };
    let dtb = QemuConfig::dtb_for(&qemu).expect("co-sim DTB next to QEMU");
    let fx = workspace().join("fixtures");

    let mut qcfg = QemuConfig::new(qemu, dtb, fx.join("elf").join("dma_test.elf"), work("qemu"));
    qcfg.elf1 = Some(fx.join("elf").join("dma_cpu1.elf"));
    let xcfg = XsimConfig {
        vivado_root: vivado,
        sources: vec![
            fx.join("rtl/dma/dma_xform.v"),
            fx.join("rtl/dma/cosim_top.sv"),
        ],
        top: "cosim_top".into(),
        defines: Vec::new(),
        dpi_lib: dpi_lib(),
        work_dir: work("xsim"),
        waves: false,
        connect_timeout: Duration::from_secs(300),
        cache_dir: None,
    };

    // XSim first: it is elaborated and waiting for the PS by the time QEMU runs.
    let xsim = XsimPl::start(&xcfg).unwrap_or_else(|e| panic!("XSim: {e}"));
    let mut pl = Checked {
        inner: xsim,
        g: Golden::default(),
    };
    let mut m = QemuMachine::spawn(&qcfg).expect("QEMU starts");

    let mut uart = String::new();
    let mut target = 0u64;
    loop {
        target += 100_000;
        match m
            .run_until(target, &mut pl, Duration::from_secs(300))
            .expect("run")
        {
            RunEnd::ReachedTime => {}
            RunEnd::PeerClosed => panic!("QEMU exited early; uart: {uart}"),
            RunEnd::Stopped(s) => panic!("stopped without a debugger: {s:?}"),
        }
        uart.push_str(&String::from_utf8_lossy(&m.uart_take()));
        if uart.contains("COSIM_DONE") {
            break;
        }
        assert!(target < 20_000_000, "firmware did not finish; uart: {uart}");
    }

    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");
    assert!(
        pl.g.mismatches.is_empty(),
        "golden model disagrees:\n{}",
        pl.g.mismatches.join("\n")
    );
    // 64 + 1000 + 200 + 1 words in the single-job cases, plus 6 jobs x 512.
    assert_eq!(pl.g.words_checked, 64 + 1000 + 200 + 1 + 6 * 512);
    assert!(pl.g.jobs >= 10, "jobs: {}", pl.g.jobs);
    let s = pl.inner.stats();
    assert!(s.irqs >= 2, "{s:?}");
    assert!(s.pl_reads > 0 && s.pl_writes > 0);
}
