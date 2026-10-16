//! Standalone XSim runs (no PS, no DPI) and the parser for the compiler's messages.
//!
//! The co-simulation path (`XsimPl`) needs a testbench that calls `rp_open`. A *self-checking*
//! third-party design does not: it prints a verdict and calls `$finish`. [`run_standalone`]
//! compiles such a design with `xvlog` / `xvhdl`, elaborates it with `xelab`, runs it with
//! `xsim` and returns a [`StandaloneReport`]: what compiled, what ran, the simulator's output
//! and every message of the tools as a [`ToolDiagnostic`] naming **file, line and construct**
//! (prompt §9.1). This is what the universality corpus (`tests/rtl`, `docs/CORPUS.md`) uses.
//!
//! The parser ([`parse_diagnostics`]) is a pure function over the text the tools print; its unit
//! tests use messages captured from Vivado 2022.1.

use crate::{XsimError, fwd, hide_window, kill_tree};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use xsim_export::Lang;

/// How serious a tool message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagSeverity {
    /// `INFO:`
    Info,
    /// `WARNING:`
    Warning,
    /// `CRITICAL WARNING:`
    Critical,
    /// `ERROR:`
    Error,
    /// A run-time `Fatal:` (`$fatal`).
    Fatal,
}

impl fmt::Display for DiagSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DiagSeverity::Info => "info",
            DiagSeverity::Warning => "warning",
            DiagSeverity::Critical => "critical warning",
            DiagSeverity::Error => "error",
            DiagSeverity::Fatal => "fatal",
        })
    }
}

/// Which tool printed a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// `xvlog` (Verilog / SystemVerilog compile).
    Xvlog,
    /// `xvhdl` (VHDL compile).
    Xvhdl,
    /// `xelab` (elaboration and link).
    Xelab,
    /// `xsim` (the run).
    Xsim,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stage::Xvlog => "xvlog",
            Stage::Xvhdl => "xvhdl",
            Stage::Xelab => "xelab",
            Stage::Xsim => "xsim",
        })
    }
}

/// One message of the Vivado tools, with the place and the construct it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDiagnostic {
    /// Severity.
    pub severity: DiagSeverity,
    /// The tool.
    pub stage: Stage,
    /// The message code, `VRFC 10-2989` / `XSIM 43-3322`, if the tool printed one.
    pub code: Option<String>,
    /// The message text, without code and location.
    pub message: String,
    /// The source file, as the tool printed it (forward slashes).
    pub file: Option<String>,
    /// 1-based line.
    pub line: Option<u32>,
    /// The construct: the quoted name in the message (`'uvm_pkg'`, `<FDRE>`), or else the text of
    /// the source line, or else the message.
    pub construct: String,
}

impl ToolDiagnostic {
    /// `true` for errors and fatals.
    pub fn is_error(&self) -> bool {
        self.severity >= DiagSeverity::Error
    }

    /// The construct, or the message when the tool named none and the source was not read.
    pub fn construct_or_message(&self) -> &str {
        if self.construct.is_empty() {
            &self.message
        } else {
            &self.construct
        }
    }

    /// The final path component of [`ToolDiagnostic::file`].
    pub fn file_name(&self) -> Option<&str> {
        self.file
            .as_deref()
            .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
    }
}

impl fmt::Display for ToolDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.line) {
            (Some(file), Some(line)) => write!(f, "{file}:{line}: ")?,
            (Some(file), None) => write!(f, "{file}: ")?,
            _ => {}
        }
        write!(f, "{} {}", self.stage, self.severity)?;
        if let Some(c) = &self.code {
            write!(f, " [{c}]")?;
        }
        write!(f, ": {} (construct `{}`)", self.message, self.construct)
    }
}

/// Where the first `$finish` of a run happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinishInfo {
    /// Simulation time as printed (`5284 ns`).
    pub time: String,
    /// The `$finish` call's file.
    pub file: Option<String>,
    /// The `$finish` call's line.
    pub line: Option<u32>,
}

fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// The quoted name a message is about: `'x'`, `<x>`, `` `x' `` or `"x"`.
fn quoted_construct(message: &str) -> Option<String> {
    for (open, close) in [('\'', '\''), ('<', '>'), ('`', '\''), ('"', '"')] {
        if let Some(a) = message.find(open) {
            let rest = &message[a + open.len_utf8()..];
            if let Some(b) = rest.find(close) {
                let c = &rest[..b];
                if !c.is_empty() {
                    return Some(c.to_string());
                }
            }
        }
    }
    None
}

/// `path:line` at the end of a `[...]`; the path may contain `:` (drive letters).
fn trailing_location(rest: &str) -> Option<(String, String, u32)> {
    let rest = rest.trim_end();
    let body = rest.strip_suffix(']')?;
    let open = body.rfind(" [").map(|i| i + 1).or_else(|| {
        // `[file:line]` directly at the start of the remainder.
        body.starts_with('[').then_some(0)
    })?;
    let inner = &body[open + 1..];
    let (file, line) = inner.rsplit_once(':')?;
    let line: u32 = line.trim().parse().ok()?;
    let message = body[..open].trim_end().to_string();
    Some((message, file.to_string(), line))
}

/// `File "path" Line N : text` (XSIM 43-3980) or `"path" Line N. text` (XSIM 43-4287).
fn embedded_location(rest: &str) -> Option<(String, String, u32)> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("File ").unwrap_or(rest);
    let rest = rest.strip_prefix('"')?;
    let q = rest.find('"')?;
    let file = rest[..q].to_string();
    let after = rest[q + 1..]
        .trim_start()
        .strip_prefix("Line")?
        .trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    let line: u32 = digits.parse().ok()?;
    let text = after[digits.len()..]
        .trim_start_matches([' ', ':', '.'])
        .trim()
        .to_string();
    Some((text, file, line))
}

fn severity_of(prefix: &str) -> Option<DiagSeverity> {
    Some(match prefix {
        "INFO" => DiagSeverity::Info,
        "WARNING" => DiagSeverity::Warning,
        "CRITICAL WARNING" => DiagSeverity::Critical,
        "ERROR" => DiagSeverity::Error,
        _ => return None,
    })
}

/// Parses the text a tool printed into diagnostics. `INFO` lines are kept only when they carry
/// a location (the tools print hundreds of "analyzing module" lines). Unrecognised lines are
/// ignored, never guessed at.
pub fn parse_diagnostics(stage: Stage, text: &str) -> Vec<ToolDiagnostic> {
    let lines: Vec<&str> = text.lines().map(|l| strip_bom(l).trim_end()).collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // SEVERITY: [CODE] message [file:line]
        // SEVERITY: [CODE] File "file" Line N : message
        // SEVERITY: [CODE] "file" Line N. message
        // CRITICAL WARNING:message [file:line]
        if let Some((sev_text, rest)) = line.split_once(':') {
            if let Some(severity) = severity_of(sev_text) {
                let rest = rest.trim_start();
                let (code, body) = match rest.strip_prefix('[') {
                    Some(r) => match r.split_once(']') {
                        Some((c, b)) => (Some(c.to_string()), b.trim_start()),
                        None => (None, rest),
                    },
                    None => (None, rest),
                };
                let (message, file, line_no) = if let Some(t) = trailing_location(body) {
                    (t.0, Some(t.1), Some(t.2))
                } else if let Some(t) = embedded_location(body) {
                    (t.0, Some(t.1), Some(t.2))
                } else if let Some(t) = runtime_file_warning(body) {
                    (body.to_string(), Some(t.1), Some(t.2))
                } else {
                    (body.to_string(), None, None)
                };
                if severity == DiagSeverity::Info && file.is_none() {
                    continue;
                }
                // A `$readmemh` of a missing file names no construct in quotes: the file is it.
                let construct = runtime_file_warning(body)
                    .map(|t| t.0)
                    .or_else(|| quoted_construct(&message))
                    .unwrap_or_default();
                out.push(ToolDiagnostic {
                    severity,
                    stage,
                    code,
                    message,
                    file,
                    line: line_no,
                    construct,
                });
                continue;
            }
            // xsim run time: `Fatal: text` / `Error: text` / `Warning: text`, then
            // `Time: 5 ns  Iteration: 0  Process: ...  Scope: top  File: f Line: 3`.
            let rt = match sev_text {
                "Fatal" => Some(DiagSeverity::Fatal),
                "Error" => Some(DiagSeverity::Error),
                "Warning" => Some(DiagSeverity::Warning),
                _ => None,
            };
            if let (Some(severity), Some(next)) = (rt, lines.get(i + 1))
                && next.starts_with("Time:")
                && let Some((file, line_no)) = runtime_scope_location(next)
            {
                let message = rest.trim().to_string();
                out.push(ToolDiagnostic {
                    severity,
                    stage,
                    code: None,
                    construct: quoted_construct(&message).unwrap_or_default(),
                    message,
                    file: Some(file),
                    line: Some(line_no),
                });
                continue;
            }
        }
        // ld: `xsim_2.c:(.text+0x177): undefined reference to `c_add'`
        if let Some(a) = line.find("undefined reference to ") {
            let what = &line[a + "undefined reference to ".len()..];
            let name = what.trim_matches(['`', '\'', ' ']).to_string();
            out.push(ToolDiagnostic {
                severity: DiagSeverity::Error,
                stage,
                code: None,
                message: format!("undefined reference to `{name}'"),
                file: None,
                line: None,
                construct: name,
            });
        }
    }
    out
}

/// `File NAME referenced on FILE at line N cannot be opened ...` ($readmemh at run time; 2022.1)
/// or `File NAME referenced from process on FILE at line N ...` (2023.1). Returns the name of the
/// file that could not be opened, and the place of the referencing statement or process.
fn runtime_file_warning(body: &str) -> Option<(String, String, u32)> {
    let name_start = body.strip_prefix("File ")?;
    let r = name_start.find(" referenced ")?;
    let name = name_start[..r].to_string();
    let after = &name_start[r + " referenced ".len()..];
    let on = after.find(" on ")? + " on ".len();
    let rest = &after[on..];
    let b = rest.find(" at line ")?;
    let file = rest[..b].to_string();
    let digits: String = rest[b + " at line ".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some((name, file, digits.parse().ok()?))
}

/// `File: path Line: N` of the `Time:` line after a run-time message.
fn runtime_scope_location(time_line: &str) -> Option<(String, u32)> {
    let a = time_line.find("File: ")? + "File: ".len();
    let rest = &time_line[a..];
    let (file, line) = rest.rsplit_once(" Line: ")?;
    Some((file.trim().to_string(), line.trim().parse().ok()?))
}

/// The first `$finish called at time : T : File "f" Line N` of a run's output.
pub fn parse_finish(text: &str) -> Option<FinishInfo> {
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("$finish called at time :") else {
            continue;
        };
        let (time, loc) = match rest.split_once(" : ") {
            Some((t, l)) => (t.trim().to_string(), Some(l)),
            None => (rest.trim().to_string(), None),
        };
        let (file, line_no) = match loc.and_then(embedded_location) {
            Some((_, f, l)) => (Some(f), Some(l)),
            None => (None, None),
        };
        return Some(FinishInfo {
            time,
            file,
            line: line_no,
        });
    }
    None
}

/// Fills in what a message did not say: the source text at `file:line` as the construct when the
/// message quotes no name, and, for a link error naming an undefined symbol, the first place in
/// `sources` that mentions it.
pub fn locate_constructs(diags: &mut [ToolDiagnostic], sources: &[PathBuf]) {
    for d in diags.iter_mut() {
        if d.file.is_none() && d.severity >= DiagSeverity::Error && !d.construct.is_empty() {
            'search: for s in sources {
                let Ok(text) = fs::read_to_string(s) else {
                    continue;
                };
                for (i, l) in text.lines().enumerate() {
                    if contains_word(l, &d.construct) {
                        d.file = Some(fwd(s));
                        d.line = Some(i as u32 + 1);
                        break 'search;
                    }
                }
            }
        }
        if d.construct.is_empty() {
            let from_source = d.file.as_deref().zip(d.line).and_then(|(f, n)| {
                let text = fs::read_to_string(f).ok()?;
                let l = text.lines().nth(n.saturating_sub(1) as usize)?;
                let t = l.trim();
                (!t.is_empty()).then(|| t.to_string())
            });
            d.construct = from_source.unwrap_or_else(|| d.message.clone());
        }
    }
}

fn contains_word(line: &str, word: &str) -> bool {
    let is_id = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    let mut from = 0;
    while let Some(i) = line[from..].find(word) {
        let a = from + i;
        let b = a + word.len();
        let before = line[..a].chars().next_back().is_none_or(|c| !is_id(c));
        let after = line[b..].chars().next().is_none_or(|c| !is_id(c));
        if before && after {
            return true;
        }
        from = b;
    }
    false
}

/// One source file of a standalone run.
#[derive(Clone, Debug)]
pub struct StandaloneSource {
    /// The file.
    pub path: PathBuf,
    /// Language (`Verilog` is `xvlog`, `SystemVerilog` is `xvlog -sv`, the VHDL variants `xvhdl`).
    pub lang: Lang,
    /// Library it compiles into.
    pub library: String,
}

impl StandaloneSource {
    /// A file in library `work`, language from the extension (`.v` Verilog, `.sv`
    /// SystemVerilog, `.vhd` VHDL-93; anything else SystemVerilog).
    pub fn auto(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let lang = Lang::from_path(&path).unwrap_or(Lang::SystemVerilog);
        Self {
            path,
            lang,
            library: "work".to_string(),
        }
    }

    /// The same file with an explicit language.
    pub fn with_lang(mut self, lang: Lang) -> Self {
        self.lang = lang;
        self
    }
}

/// What to build and run, with no PS and no DPI.
#[derive(Clone, Debug)]
pub struct StandaloneConfig {
    /// Vivado install directory (contains `settings64.bat`).
    pub vivado_root: PathBuf,
    /// Sources in compile order, testbench included.
    pub sources: Vec<StandaloneSource>,
    /// `` `include `` directories (`-i`).
    pub include_dirs: Vec<PathBuf>,
    /// Preprocessor defines (`NAME` or `NAME=VALUE`).
    pub defines: Vec<String>,
    /// Libraries (`-L`), for example `uvm` or `unisims_ver`.
    pub libs: Vec<String>,
    /// The top unit (`tb`), or `lib.tb`.
    pub top: String,
    /// Further units elaborated with the top (for example `work.glbl`).
    pub extra_tops: Vec<String>,
    /// Extra `xvlog` arguments (for example `--relax`).
    pub xvlog_args: Vec<String>,
    /// Extra `xelab` arguments.
    pub xelab_args: Vec<String>,
    /// Files copied into the run directory (`$readmemh` data).
    pub data_files: Vec<PathBuf>,
    /// Scratch directory (products, logs); the simulator runs in it.
    pub work_dir: PathBuf,
    /// Wall-clock limit for the whole build and run.
    pub timeout: Duration,
}

impl StandaloneConfig {
    /// A configuration with the defaults: no includes, defines or libraries, two minutes.
    pub fn new(vivado_root: PathBuf, top: &str, work_dir: PathBuf) -> Self {
        Self {
            vivado_root,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            libs: Vec::new(),
            top: top.to_string(),
            extra_tops: Vec::new(),
            xvlog_args: Vec::new(),
            xelab_args: Vec::new(),
            data_files: Vec::new(),
            work_dir,
            timeout: Duration::from_secs(180),
        }
    }
}

/// How far a standalone run got, and everything the tools said.
#[derive(Clone, Debug)]
pub struct StandaloneReport {
    /// Every source compiled (`xvlog` and `xvhdl` exit 0).
    pub compiled: bool,
    /// `xelab` built the snapshot.
    pub elaborated: bool,
    /// `xsim` ran to the end of the simulation (it did not fail to start, die or time out).
    pub ran: bool,
    /// The first `$finish`, if the design called one.
    pub finish: Option<FinishInfo>,
    /// The wall limit was hit and the simulator was killed.
    pub timed_out: bool,
    /// The simulator's standard output.
    pub sim_output: String,
    /// Messages of all stages, in order, without bare `INFO` lines.
    pub diagnostics: Vec<ToolDiagnostic>,
    /// Wall time of the whole build and run.
    pub wall: Duration,
    /// The scratch directory (logs: `sa_xvlog.txt`, `sa_xvhdl.txt`, `sa_xelab.txt`, `sa_xsim.txt`).
    pub work_dir: PathBuf,
}

impl StandaloneReport {
    /// The first stage that failed, for tables: `xvlog`, `xvhdl`, `xelab`, `xsim` or `-`.
    pub fn failed_stage(&self) -> &'static str {
        if !self.compiled {
            if self
                .diagnostics
                .iter()
                .any(|d| d.stage == Stage::Xvhdl && d.is_error())
            {
                "xvhdl"
            } else {
                "xvlog"
            }
        } else if !self.elaborated {
            "xelab"
        } else if !self.ran {
            "xsim"
        } else {
            "-"
        }
    }

    /// The error and fatal diagnostics.
    pub fn errors(&self) -> impl Iterator<Item = &ToolDiagnostic> {
        self.diagnostics.iter().filter(|d| d.is_error())
    }

    /// Whether the simulator output contains `needle`.
    pub fn printed(&self, needle: &str) -> bool {
        self.sim_output.contains(needle)
    }
}

fn read_log(p: &Path) -> String {
    match fs::read(p) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => String::new(),
    }
}

/// The part of a failed build log worth showing: every error line, then the last 25 lines.
/// (The compilers print one `INFO` line per module, which pushes errors out of a plain tail.)
pub(crate) fn failure_log(path: &Path) -> String {
    let text = read_log(path);
    let lines: Vec<&str> = text.lines().collect();
    let tail_start = lines.len().saturating_sub(25);
    let mut out: Vec<&str> = lines[..tail_start]
        .iter()
        .copied()
        .filter(|l| {
            let l = strip_bom(l);
            l.starts_with("ERROR:") || l.starts_with("CRITICAL WARNING:")
        })
        .take(30)
        .collect();
    if !out.is_empty() {
        out.push("...");
    }
    out.extend_from_slice(&lines[tail_start..]);
    out.join("\n")
}

/// Compiles, elaborates and runs a self-contained design with XSim.
///
/// Tool failures (a syntax error, a missing module, a design that dies) are *results*, not
/// errors: they are in the report with their diagnostics. `Err` is for a missing file or an OS
/// failure.
pub fn run_standalone(cfg: &StandaloneConfig) -> Result<StandaloneReport, XsimError> {
    let settings = cfg.vivado_root.join("settings64.bat");
    for p in cfg
        .sources
        .iter()
        .map(|s| &s.path)
        .chain(&cfg.data_files)
        .chain([&settings])
    {
        if !p.is_file() {
            return Err(XsimError::MissingFile(p.clone()));
        }
    }
    if !cfg!(windows) {
        return Err(XsimError::NotConnected {
            detail: "the XSim adapter supports Windows only for now".to_string(),
        });
    }
    let started = Instant::now();
    fs::create_dir_all(&cfg.work_dir)?;
    let work = fs::canonicalize(&cfg.work_dir)?;
    let work = PathBuf::from(work.display().to_string().trim_start_matches(r"\\?\"));
    for d in &cfg.data_files {
        if let Some(n) = d.file_name() {
            fs::copy(d, work.join(n))?;
        }
    }
    for stale in [
        "sa_xvlog.txt",
        "sa_xvhdl.txt",
        "sa_xelab.txt",
        "sa_xsim.txt",
    ] {
        let _ = fs::remove_file(work.join(stale));
    }

    // Project files: one line per source, as Vivado's own scripts write them.
    let mut vlog = String::new();
    let mut vhdl = String::new();
    let mut opts = String::new();
    for i in &cfg.include_dirs {
        opts.push_str(&format!(" -i \"{}\"", fwd(i)));
    }
    for d in &cfg.defines {
        opts.push_str(&format!(" -d \"{d}\""));
    }
    for s in &cfg.sources {
        if s.lang.is_vhdl() {
            vhdl.push_str(&format!(
                "{} {} \"{}\"\n",
                s.lang.prj_word(),
                s.library,
                fwd(&s.path)
            ));
        } else {
            vlog.push_str(&format!(
                "{} {}{opts} \"{}\"\n",
                s.lang.prj_word(),
                s.library,
                fwd(&s.path)
            ));
        }
    }
    fs::write(work.join("sa_vlog.prj"), &vlog)?;
    fs::write(work.join("sa_vhdl.prj"), &vhdl)?;
    let libs: String = cfg.libs.iter().map(|l| format!(" -L {l}")).collect();
    let extra_v: String = cfg.xvlog_args.iter().map(|a| format!(" {a}")).collect();
    let extra_e: String = cfg.xelab_args.iter().map(|a| format!(" {a}")).collect();
    let tops: String = std::iter::once(&cfg.top)
        .chain(&cfg.extra_tops)
        .map(|t| {
            if t.contains('.') {
                format!(" {t}")
            } else {
                format!(" work.{t}")
            }
        })
        .collect();
    let mut bat = format!(
        "@echo off\r\ncall \"{}\"\r\ncd /d \"{}\"\r\nrmdir /s /q xsim.dir 2>nul\r\n",
        settings.display(),
        work.display()
    );
    if !vlog.is_empty() {
        bat.push_str(&format!(
            "call xvlog -prj sa_vlog.prj{libs}{extra_v} > sa_xvlog.txt 2>&1 || exit /b 1\r\n"
        ));
    }
    if !vhdl.is_empty() {
        bat.push_str("call xvhdl -prj sa_vhdl.prj > sa_xvhdl.txt 2>&1 || exit /b 2\r\n");
    }
    bat.push_str(&format!(
        "call xelab -debug typical -timescale 1ns/1ps -L work{libs}{extra_e}{tops} -s sa_snap \
         > sa_xelab.txt 2>&1 || exit /b 3\r\n"
    ));
    bat.push_str("call xsim sa_snap -tclbatch sa_run.tcl > sa_xsim.txt 2>&1 || exit /b 4\r\n");
    fs::write(work.join("sa_run.tcl"), "run all\r\nquit\r\n")?;
    let bat_path = work.join("sa_build_run.bat");
    fs::write(&bat_path, bat)?;

    let mut cmd = Command::new("cmd");
    cmd.arg("/C")
        .arg(&bat_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut timed_out = false;
    let code = loop {
        if let Some(st) = child.try_wait()? {
            break st.code();
        }
        if started.elapsed() > cfg.timeout {
            kill_tree(&mut child);
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    let xvlog = read_log(&work.join("sa_xvlog.txt"));
    let xvhdl = read_log(&work.join("sa_xvhdl.txt"));
    let xelab = read_log(&work.join("sa_xelab.txt"));
    let xsim = read_log(&work.join("sa_xsim.txt"));
    let mut diagnostics = Vec::new();
    diagnostics.extend(parse_diagnostics(Stage::Xvlog, &xvlog));
    diagnostics.extend(parse_diagnostics(Stage::Xvhdl, &xvhdl));
    diagnostics.extend(parse_diagnostics(Stage::Xelab, &xelab));
    diagnostics.extend(parse_diagnostics(Stage::Xsim, &xsim));
    let paths: Vec<PathBuf> = cfg.sources.iter().map(|s| s.path.clone()).collect();
    locate_constructs(&mut diagnostics, &paths);

    // An unexpected exit code means the batch died in a step it could not describe.
    let step_failed = |n: i32| code == Some(n);
    let compiled = !timed_out && !step_failed(1) && !step_failed(2) && code.is_some();
    let elaborated = compiled && !step_failed(3);
    let ran = elaborated && !step_failed(4) && code == Some(0);
    Ok(StandaloneReport {
        compiled,
        elaborated,
        ran,
        finish: parse_finish(&xsim),
        timed_out,
        sim_output: xsim,
        diagnostics,
        wall: started.elapsed(),
        work_dir: work,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Messages below are copied from Vivado 2022.1 runs; only the scratch directory in the paths
    // was shortened to `C:/w/`.

    fn one(stage: Stage, text: &str) -> ToolDiagnostic {
        let mut d = parse_diagnostics(stage, text);
        assert_eq!(d.len(), 1, "{d:?}");
        d.remove(0)
    }

    #[test]
    fn xvlog_error_with_trailing_location() {
        let d = one(
            Stage::Xvlog,
            "ERROR: [VRFC 10-2989] 'uvm_pkg' is not declared [C:/w/top.sv:1]\n",
        );
        assert_eq!(d.severity, DiagSeverity::Error);
        assert_eq!(d.code.as_deref(), Some("VRFC 10-2989"));
        assert_eq!(d.file.as_deref(), Some("C:/w/top.sv"));
        assert_eq!(d.line, Some(1));
        assert_eq!(d.construct, "uvm_pkg");
        assert_eq!(d.file_name(), Some("top.sv"));
    }

    #[test]
    fn missing_include_names_the_header() {
        let d = one(
            Stage::Xvlog,
            "ERROR: [VRFC 10-3195] cannot open include file 'no_such_header.vh' [C:/w/top.sv:1]",
        );
        assert_eq!(d.construct, "no_such_header.vh");
        assert_eq!(d.line, Some(1));
    }

    #[test]
    fn undefined_macro() {
        let d = one(
            Stage::Xvlog,
            "ERROR: [VRFC 10-3805] use of undefined macro 'DATA_WIDTH' [C:/w/top.sv:2]",
        );
        assert_eq!(d.construct, "DATA_WIDTH");
    }

    #[test]
    fn module_not_found_names_module_and_instance() {
        let d = one(
            Stage::Xvlog,
            "ERROR: [VRFC 10-2063] Module <FDRE> not found while processing module instance <ff> [C:/w/top.sv:4]",
        );
        assert_eq!(d.construct, "FDRE");
        assert_eq!(d.line, Some(4));
    }

    #[test]
    fn vhdl_2008_construct_has_no_quote_so_the_source_text_is_used() {
        let dir = std::env::temp_dir().join(format!("zs-sa-unit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("top.vhd");
        fs::write(&src, "a\nb\nc\nd\ne\nf\n  process (all) begin\n").unwrap();
        let text = format!(
            "ERROR: [VRFC 10-1449] this construct is only supported in VHDL 1076-2008 [{}:7]\n",
            fwd(&src)
        );
        let mut d = parse_diagnostics(Stage::Xvhdl, &text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].construct, "");
        locate_constructs(&mut d, std::slice::from_ref(&src));
        assert_eq!(d[0].construct, "process (all) begin");
        assert_eq!(d[0].line, Some(7));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn syntax_error_construct_is_the_token() {
        let d = one(
            Stage::Xvlog,
            "ERROR: [VRFC 10-4982] syntax error near ';' [C:/w/top.sv:4]",
        );
        assert_eq!(d.construct, ";");
    }

    #[test]
    fn info_lines_are_dropped_unless_they_have_a_location() {
        let text = "INFO: [VRFC 10-311] analyzing module dut\n\
                    INFO: [VRFC 10-8704] VHDL file 'C:/w/top.vhd' is ignored due to errors\n\
                    INFO: [VRFC 10-2263] Analyzing SystemVerilog file \"C:/w/top.sv\" into library work\n";
        assert!(parse_diagnostics(Stage::Xvhdl, text).is_empty());
    }

    #[test]
    fn critical_warning_without_code_is_the_encrypted_envelope() {
        let d = one(
            Stage::Xvlog,
            "CRITICAL WARNING:This program cannot decrypt an IEEE-1735 envelope without a Xilinx key. [C:/w/top.sv:2]",
        );
        assert_eq!(d.severity, DiagSeverity::Critical);
        assert_eq!(d.code, None);
        assert_eq!(d.file.as_deref(), Some("C:/w/top.sv"));
        assert_eq!(d.line, Some(2));
        assert!(d.message.contains("IEEE-1735"));
    }

    #[test]
    fn xelab_unsupported_feature_with_file_and_line_in_the_text() {
        let d = one(
            Stage::Xelab,
            "ERROR: [XSIM 43-3980] File \"C:/w/top.sv\" Line 3 : The SystemVerilog feature \"nettype\" is not supported yet for simulation.",
        );
        assert_eq!(d.code.as_deref(), Some("XSIM 43-3980"));
        assert_eq!(d.file.as_deref(), Some("C:/w/top.sv"));
        assert_eq!(d.line, Some(3));
        assert_eq!(d.construct, "nettype");
    }

    #[test]
    fn xelab_undefined_system_task() {
        let d = one(
            Stage::Xelab,
            "ERROR: [XSIM 43-4287] \"C:/w/top.sv\" Line 3. Undefined system task '$my_vpi_hello'",
        );
        assert_eq!(d.file.as_deref(), Some("C:/w/top.sv"));
        assert_eq!(d.line, Some(3));
        assert_eq!(d.construct, "$my_vpi_hello");
    }

    #[test]
    fn generic_interconnect() {
        let d = one(
            Stage::Xvlog,
            "ERROR: [VRFC 10-98] generic interconnect is not supported for static elaboration [C:/w/top.sv:3]",
        );
        assert_eq!(d.line, Some(3));
        assert_eq!(d.construct, "");
    }

    #[test]
    fn link_error_for_a_dpi_import_is_located_in_the_sources() {
        let dir = std::env::temp_dir().join(format!("zs-sa-link-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("top.sv");
        fs::write(
            &src,
            "module top;\n  import \"DPI-C\" function int c_add(input int a, input int b);\nendmodule\n",
        )
        .unwrap();
        let text = "xsim.dir/snap/obj/xsim_2.win64.obj:xsim_2.c:(.text+0x177): undefined reference to `c_add'\n\
                    collect2.exe: error: ld returned 1 exit status\n\
                    ERROR: [XSIM 43-3238] Failed to link the design.\n";
        let mut d = parse_diagnostics(Stage::Xelab, text);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].construct, "c_add");
        assert!(d[0].file.is_none());
        assert_eq!(d[1].code.as_deref(), Some("XSIM 43-3238"));
        locate_constructs(&mut d, std::slice::from_ref(&src));
        assert_eq!(d[0].line, Some(2));
        assert_eq!(d[0].file_name(), Some("top.sv"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn runtime_fatal_with_scope_line() {
        let text = "## run all\nFatal: deliberate failure\n\
                    Time: 5 ns  Iteration: 0  Process: /top/Initial2_0  Scope: top  File: C:/w/top.sv Line: 3\n\
                    $finish called at time : 5 ns\n";
        let d = one(Stage::Xsim, text);
        assert_eq!(d.severity, DiagSeverity::Fatal);
        assert_eq!(d.file.as_deref(), Some("C:/w/top.sv"));
        assert_eq!(d.line, Some(3));
        assert_eq!(d.message, "deliberate failure");
    }

    #[test]
    fn a_testbench_line_that_says_error_is_not_a_tool_message() {
        assert!(parse_diagnostics(Stage::Xsim, "Error: TC3 mismatch\nTC4 done\n").is_empty());
    }

    #[test]
    fn readmem_of_a_missing_file_warns_with_the_call_site() {
        let d = one(
            Stage::Xsim,
            "WARNING: File no_such_file.hex referenced on C:/w/top.sv at line 4 cannot be opened for reading. Please ensure that this file is available in the current working directory.",
        );
        assert_eq!(d.severity, DiagSeverity::Warning);
        assert_eq!(d.file.as_deref(), Some("C:/w/top.sv"));
        assert_eq!(d.line, Some(4));
        assert_eq!(d.construct, "no_such_file.hex");
    }

    #[test]
    fn readmem_of_a_missing_file_in_vivado_2023_1_names_the_process() {
        let d = one(
            Stage::Xsim,
            "WARNING: File no_such_file.hex referenced from process on C:/w/top.sv at line 4 cannot be opened for reading. Please ensure that this file is available in the current working directory.",
        );
        assert_eq!(d.file.as_deref(), Some("C:/w/top.sv"));
        assert_eq!(d.line, Some(4));
        assert_eq!(d.construct, "no_such_file.hex");
    }

    #[test]
    fn bom_and_crlf_are_tolerated() {
        let d = one(
            Stage::Xvlog,
            "\u{feff}ERROR: [VRFC 10-2989] 'b' is not declared [C:/w/top.sv:3]\r\n",
        );
        assert_eq!(d.construct, "b");
    }

    #[test]
    fn finish_info() {
        let f = parse_finish("x\n$finish called at time : 5284 ns : File \"C:/w/tb.v\" Line 712\n")
            .unwrap();
        assert_eq!(f.time, "5284 ns");
        assert_eq!(f.file.as_deref(), Some("C:/w/tb.v"));
        assert_eq!(f.line, Some(712));
        let g = parse_finish("$finish called at time : 5 ns\n").unwrap();
        assert_eq!(g.file, None);
        assert!(parse_finish("nothing").is_none());
    }

    #[test]
    fn display_names_file_line_and_construct() {
        let d = one(
            Stage::Xvlog,
            "ERROR: [VRFC 10-2989] 'b' is not declared [C:/w/top.sv:3]",
        );
        let s = d.to_string();
        assert!(
            s.starts_with("C:/w/top.sv:3: xvlog error [VRFC 10-2989]"),
            "{s}"
        );
        assert!(s.ends_with("(construct `b`)"), "{s}");
    }
}
