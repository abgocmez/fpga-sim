//! Loading a simulation from the GUI: an editable draft of a project, and an
//! engine build that runs on a worker thread so the window stays responsive
//! while XSim compiles the design.

use sim_api::{CpuSection, Engine, MediaSection, Project, RtlSection, Severity, SimClient};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

/// How the PL is provided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlKind {
    /// No PL (CPU-only firmware, or the demo engine).
    None,
    /// Native Rust models listed in the project's `[native]` table.
    Native,
    /// RTL simulated in Vivado XSim.
    Xsim,
}

/// The fields of a project the GUI edits. Everything else in the project
/// (`[connect]`, `[native]`, `[devices]`…) is carried over unchanged.
#[derive(Clone, Debug)]
pub struct ProjectDraft {
    /// The project this draft started from.
    pub base: Project,
    /// Directory relative paths resolve against.
    pub dir: PathBuf,
    /// Where the project came from, if it was loaded from a file.
    pub file: Option<PathBuf>,
    /// Use QEMU for the PS (else the built-in demo engine).
    pub use_qemu: bool,
    /// Core 0 ELF.
    pub cpu0: String,
    /// Core 1 ELF (empty = unused).
    pub cpu1: String,
    /// PL provider.
    pub pl: PlKind,
    /// RTL source files, one per line.
    pub rtl_files: String,
    /// RTL top module.
    pub rtl_top: String,
    /// UART1 over TCP (interactive) instead of a file.
    pub uart_tcp: bool,
    /// Lock-step quantum in nanoseconds.
    pub quantum_ns: String,
    /// HDMI RX source (`bars`, `gradient`, a PNG path…).
    pub video_in: String,
    /// LINE IN source (`silence`, `tone:HZ`, a WAV path…).
    pub audio_in: String,
}

impl ProjectDraft {
    /// A draft of the built-in demo project.
    pub fn demo() -> ProjectDraft {
        let base = Project::from_toml(sim_api::starter_project_toml())
            .unwrap_or_else(|e| unreachable!("the starter project parses: {}", e.message));
        ProjectDraft::from_project(base, PathBuf::from("."), None)
    }

    /// A draft of an existing project.
    pub fn from_project(base: Project, dir: PathBuf, file: Option<PathBuf>) -> ProjectDraft {
        let pl = match base.backends.pl.as_str() {
            "xsim" => PlKind::Xsim,
            "native" => PlKind::Native,
            _ => PlKind::None,
        };
        let media = base.media.clone().unwrap_or_default();
        ProjectDraft {
            use_qemu: base.backends.ps == "qemu",
            cpu0: base
                .cpu0
                .as_ref()
                .map(|c| c.elf.clone())
                .unwrap_or_default(),
            cpu1: base
                .cpu1
                .as_ref()
                .map(|c| c.elf.clone())
                .unwrap_or_default(),
            pl,
            rtl_files: base
                .rtl
                .as_ref()
                .map(|r| r.files.join("\n"))
                .unwrap_or_default(),
            rtl_top: base
                .rtl
                .as_ref()
                .and_then(|r| r.top.clone())
                .unwrap_or_default(),
            uart_tcp: base.host.get("uart1").is_some_and(|v| v == "tcp"),
            quantum_ns: base.sim.quantum_ns.to_string(),
            video_in: media.video_in.unwrap_or_default(),
            audio_in: media.audio_in.unwrap_or_default(),
            base,
            dir,
            file,
        }
    }

    /// Reads a project file into a draft.
    pub fn load(file: &Path) -> Result<ProjectDraft, String> {
        let p = Project::load(file).map_err(|e| e.message)?;
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(ProjectDraft::from_project(p, dir, Some(file.to_path_buf())))
    }

    /// The project this draft describes, or what is wrong with it.
    pub fn to_project(&self) -> Result<Project, String> {
        let mut p = self.base.clone();
        let non_empty = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        p.backends.ps = if self.use_qemu { "qemu" } else { "demo" }.to_string();
        p.backends.pl = match self.pl {
            PlKind::None => "demo",
            PlKind::Native => "native",
            PlKind::Xsim => "xsim",
        }
        .to_string();
        if self.use_qemu {
            // A BOOT.BIN project has no core 0 ELF: it comes from `[boot] file`.
            let boot_bin = p
                .boot
                .as_ref()
                .is_some_and(|b| sim_api::bootbin::is_boot_bin_mode(&b.mode));
            match non_empty(&self.cpu0) {
                Some(elf) => p.cpu0 = Some(CpuSection { elf }),
                None if boot_bin => p.cpu0 = None,
                None => return Err("the PS needs a core 0 ELF to run".to_string()),
            }
            p.cpu1 = non_empty(&self.cpu1).map(|elf| CpuSection { elf });
            if p.boot.is_none() {
                p.boot = Some(sim_api::BootSection {
                    mode: "elf".to_string(),
                    file: None,
                });
            }
        } else {
            p.cpu0 = None;
            p.cpu1 = None;
            p.boot = None;
        }
        if self.pl == PlKind::Xsim {
            let files: Vec<String> = self
                .rtl_files
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            if files.is_empty() {
                return Err("the PL needs at least one RTL source file".to_string());
            }
            let prev = p.rtl.take().unwrap_or(RtlSection {
                files: Vec::new(),
                top: None,
                defines: Vec::new(),
                waves: false,
            });
            p.rtl = Some(RtlSection {
                files,
                top: non_empty(&self.rtl_top),
                ..prev
            });
        } else {
            p.rtl = None;
        }
        if self.uart_tcp {
            p.host.insert("uart1".to_string(), "tcp".to_string());
        } else {
            p.host.remove("uart1");
        }
        p.sim.quantum_ns = self
            .quantum_ns
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|q| *q > 0)
            .ok_or_else(|| "the quantum must be a positive number of nanoseconds".to_string())?;
        let (v, a) = (non_empty(&self.video_in), non_empty(&self.audio_in));
        p.media = if v.is_some() || a.is_some() {
            Some(MediaSection {
                video_in: v,
                audio_in: a,
            })
        } else {
            None
        };
        Ok(p)
    }

    /// Every validation error of the draft, as lines of text.
    pub fn problems(&self) -> Vec<String> {
        match self.to_project() {
            Err(e) => vec![e],
            Ok(p) => p
                .validate(&self.dir)
                .into_iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| format!("{}: {}", d.field, d.message))
                .collect(),
        }
    }

    /// The project as `project.toml` text.
    pub fn to_toml(&self) -> Result<String, String> {
        let p = self.to_project()?;
        p.to_toml().map_err(|e| e.message)
    }
}

/// A running or finished engine build.
pub struct LoadJob {
    /// Description for the UI.
    pub what: String,
    /// When the build started.
    pub since: Instant,
    rx: Receiver<Result<Box<dyn Engine>, String>>,
}

impl LoadJob {
    /// Starts building `project` (relative paths resolve against `dir`).
    pub fn start(project: Project, dir: PathBuf, what: String) -> LoadJob {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = project.build_engine(&dir).map_err(|e| match e.hint {
                Some(h) => format!(
                    "{}
{h}",
                    e.message
                ),
                None => e.message,
            });
            let _ = tx.send(r);
        });
        LoadJob {
            what,
            since: Instant::now(),
            rx,
        }
    }

    /// The finished client, an error message, or `None` while still building.
    pub fn poll(&self) -> Option<Result<SimClient, String>> {
        match self.rx.try_recv() {
            Ok(Ok(engine)) => Some(Ok(SimClient::spawn_engine(engine))),
            Ok(Err(e)) => Some(Err(e)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the build thread ended".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(name)
            .join("project.toml")
    }

    #[test]
    fn a_loaded_project_round_trips_through_the_draft() {
        for name in ["board-demo", "audio-demo", "math-qemu", "dma-regression"] {
            let d = ProjectDraft::load(&example(name)).unwrap();
            let p = d.to_project().unwrap();
            assert_eq!(p, d.base, "{name}: an untouched draft is the same project");
            // And through TOML text.
            let text = d.to_toml().unwrap();
            assert_eq!(Project::from_toml(&text).unwrap(), p, "{name}");
        }
    }

    #[test]
    fn editing_the_ps_and_pl_separately_changes_only_that_part() {
        let mut d = ProjectDraft::load(&example("board-demo")).unwrap();
        let before = d.to_project().unwrap();
        d.cpu0 = "../../fixtures/elf/math_test.elf".to_string();
        let after = d.to_project().unwrap();
        assert_eq!(after.rtl, before.rtl, "PL untouched by a PS edit");
        assert_ne!(after.cpu0, before.cpu0);

        let mut d = ProjectDraft::load(&example("board-demo")).unwrap();
        d.rtl_files = "a.v\n\n  b.sv  ".to_string();
        d.rtl_top = "top".to_string();
        let p = d.to_project().unwrap();
        assert_eq!(p.rtl.as_ref().unwrap().files, vec!["a.v", "b.sv"]);
        assert_eq!(p.rtl.as_ref().unwrap().top.as_deref(), Some("top"));
        assert_eq!(p.cpu0, before.cpu0, "PS untouched by a PL edit");
    }

    #[test]
    fn invalid_drafts_say_what_is_wrong() {
        let mut d = ProjectDraft::load(&example("board-demo")).unwrap();
        d.cpu0 = "  ".to_string();
        assert!(d.problems()[0].contains("core 0 ELF"));
        d.cpu0 = "missing.elf".to_string();
        assert!(
            d.problems().iter().any(|p| p.contains("cpu0")),
            "{:?}",
            d.problems()
        );
        d.cpu0 = "../../fixtures/elf/board_test.elf".to_string();
        d.rtl_files.clear();
        assert!(d.problems()[0].contains("RTL source"));
        d.rtl_files = "x.v".to_string();
        d.quantum_ns = "zero".to_string();
        assert!(d.problems()[0].contains("quantum"));
    }

    #[test]
    fn switching_to_the_demo_engine_drops_cpu_and_pl() {
        let mut d = ProjectDraft::load(&example("board-demo")).unwrap();
        d.use_qemu = false;
        d.pl = PlKind::None;
        let p = d.to_project().unwrap();
        assert!(p.cpu0.is_none() && p.rtl.is_none());
        assert_eq!(p.backends.ps, "demo");
        assert!(d.problems().is_empty(), "{:?}", d.problems());
    }

    #[test]
    fn the_demo_draft_builds_a_working_client() {
        let d = ProjectDraft::demo();
        let job = LoadJob::start(d.to_project().unwrap(), d.dir.clone(), "demo".to_string());
        let client = loop {
            if let Some(r) = job.poll() {
                break r.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(client.status().unwrap().ps_backend, "demo");
    }

    #[test]
    fn a_failing_build_reports_the_error() {
        let mut d = ProjectDraft::load(&example("math-qemu")).unwrap();
        d.cpu0 = "no/such.elf".to_string();
        let job = LoadJob::start(d.to_project().unwrap(), d.dir.clone(), "x".to_string());
        let err = loop {
            if let Some(r) = job.poll() {
                break r.err().expect("must fail");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(err.contains("cpu0"), "{err}");
    }
}
