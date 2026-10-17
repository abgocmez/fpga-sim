//! `cargo xtask dist`: a release archive for this operating system and
//! architecture, and the check that it runs from a clean directory.
//!
//! The archive holds the three release binaries, the XSim DPI library in
//! `lib/`, the examples with the fixtures they refer to, the board files, the
//! docs, the licence and `INSTALL.txt`. Archiving uses the operating system's
//! own tools (`tar`; `tar.exe` and PowerShell on Windows) and the checksum comes
//! from `sha256sum`, `shasum` or `Get-FileHash`, so no archive or hash crate is
//! added to the workspace.

use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// The XSim DPI static library, under the name `find_dpi_lib` looks for.
pub const DPI_ARCHIVE: &str = "libxsim_dpi.a";

/// Arguments of `cargo xtask dpi`. The library gets its own target directory:
/// a normal workspace build compiles the same crate without exports.
pub const DPI_BUILD: &[&str] = &[
    "build",
    "--release",
    "-p",
    "xsim-dpi",
    "--features",
    "freestanding",
    "--target-dir",
    "target/dpi",
];

/// Arguments of `cargo xtask dpi-shared`: the shared-object DPI library
/// (`dpi-cdylib`), built in the normal target directory and then copied.
pub const DPI_SHARED_BUILD: &[&str] = &["build", "--release", "-p", "dpi-cdylib"];

/// The shared-object DPI library's file name: `zynqsim_dpi.dll`, `.so` or
/// `.dylib`. Cargo names the Unix files `libzynqsim_dpi.*`; the copy drops the `lib`.
pub fn dpi_shared_file() -> String {
    format!("zynqsim_dpi{}", env::consts::DLL_SUFFIX)
}

/// The binaries in the archive (file names without the platform suffix).
const PROGRAMS: [&str; 3] = ["zynqsim", "zynqsim-gui", "zynqsim-mcp"];

/// The simulated time of the clean-directory run: `--for 1ms`.
const DEMO_NOW_PS: u64 = 1_000_000_000;

/// The workspace root (`crates/xtask` is two levels below it).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Directory name of the archive: `zynqsim-<version>-<os>-<arch>`.
pub fn dir_name(version: &str, os: &str, arch: &str) -> String {
    format!("zynqsim-{version}-{os}-{arch}")
}

/// Archive file name: `.zip` on Windows, `.tar.gz` elsewhere.
pub fn archive_name(dir: &str, os: &str) -> String {
    if os == "windows" {
        format!("{dir}.zip")
    } else {
        format!("{dir}.tar.gz")
    }
}

/// The newest `zynqsim-*` archive in `dist`, if there is one.
pub fn newest_archive(dist: &Path) -> Option<PathBuf> {
    fs::read_dir(dist)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.starts_with("zynqsim-") && (n.ends_with(".zip") || n.ends_with(".tar.gz"))
        })
        .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// The `fixtures/` paths (relative to `fixtures/`) that `text` mentions: in a
/// project file (`"../../fixtures/rtl/x.v"`) or a comment (`fixtures/elf/x.elf`).
/// A mention ending in `/` or `.` is trimmed; `..` segments are refused.
pub fn fixture_refs_in(text: &str) -> BTreeSet<String> {
    const NEEDLE: &str = "fixtures/";
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let is_path = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/');
    let bytes = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(NEEDLE) {
        let start = from + i;
        let body = start + NEEDLE.len();
        let end = body + bytes[body..].iter().take_while(|&&b| is_path(b)).count();
        from = body;
        if start > 0 && is_word(bytes[start - 1]) {
            continue;
        }
        // Only ASCII bytes were matched, so these indices are char boundaries.
        let rel = text[body..end].trim_end_matches(['.', '/']);
        if !rel.is_empty() && !rel.split('/').any(|s| s.is_empty() || s == "..") {
            out.insert(rel.to_string());
        }
    }
    out
}

/// What `INSTALL.txt` says, for one archive.
fn install_txt(version: &str, os: &str, arch: &str) -> String {
    let exe = if os == "windows" { ".exe" } else { "" };
    let dot = if os == "windows" { "" } else { "./" };
    format!(
        "ZynqSim {version} ({os}-{arch})\n\
         \n\
         Contents\n\
         \x20 zynqsim{exe}        headless runner: run, test, trace, inspect, init\n\
         \x20 zynqsim-gui{exe}    desktop front end: project loader, board, waveforms\n\
         \x20 zynqsim-mcp{exe}    Model Context Protocol server on stdin/stdout\n\
         \x20 lib/{DPI_ARCHIVE}    XSim DPI library; found next to the programs, no setup\n\
         \x20 examples/          example projects (they refer to ../../fixtures/)\n\
         \x20 fixtures/          the files the examples use (RTL, firmware ELF, XSA, boot)\n\
         \x20 boards/            board descriptions (the built-in ones are compiled in)\n\
         \x20 docs/              status, test plan, design notes, ADRs, INSTALL.md\n\
         \x20 README.md, THIRD_PARTY.md, LICENSE\n\
         \n\
         Run from this folder, because the examples use relative paths:\n\
         \x20 {dot}zynqsim{exe} --version\n\
         \x20 {dot}zynqsim{exe} run --project examples/demo/project.toml --for 1ms --json\n\
         \n\
         The demo engine, the GUI and the MCP server need no other program.\n\
         Firmware needs Xilinx QEMU (ZYNQSIM_QEMU); RTL needs Vivado XSim (ZYNQSIM_VIVADO).\n\
         See docs/INSTALL.md for installing them and for troubleshooting.\n"
    )
}

/// `cargo xtask dpi-shared`: builds the shared-object DPI library and copies it to
/// `target/dpi/release/zynqsim_dpi.{dll,so,dylib}`.
pub fn run_dpi_shared(root: &Path) -> bool {
    match build_dpi_shared(root) {
        Ok(path) => {
            eprintln!("xtask: shared DPI library: {}", path.display());
            true
        }
        Err(e) => {
            eprintln!("xtask: dpi-shared failed: {e}");
            false
        }
    }
}

/// Builds `dpi-cdylib` and copies it into `target/dpi/release/`. Returns the copy.
fn build_dpi_shared(root: &Path) -> Result<PathBuf, String> {
    cargo(root, DPI_SHARED_BUILD)?;
    let built = root.join("target").join("release").join(format!(
        "{}zynqsim_dpi{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX
    ));
    let dest = root
        .join("target")
        .join("dpi")
        .join("release")
        .join(dpi_shared_file());
    copy_file(&built, &dest)?;
    Ok(dest)
}

/// `cargo xtask vpi`: builds the Icarus VPI module and copies it to `target/vpi/release/zynqsim.vpi`
/// (the name `vvp -mzynqsim` loads; ADR-037).
pub fn run_vpi(root: &Path) -> bool {
    match build_vpi(root) {
        Ok(path) => {
            eprintln!("xtask: Icarus VPI module: {}", path.display());
            true
        }
        Err(e) => {
            eprintln!("xtask: vpi failed: {e}");
            false
        }
    }
}

/// Builds `vpi-icarus` and copies the DLL to `target/vpi/release/zynqsim.vpi`. Returns the copy.
fn build_vpi(root: &Path) -> Result<PathBuf, String> {
    cargo(root, &["build", "--release", "-p", "vpi-icarus"])?;
    let built = root.join("target").join("release").join("zynqsim.dll");
    let dest = root
        .join("target")
        .join("vpi")
        .join("release")
        .join("zynqsim.vpi");
    copy_file(&built, &dest)?;
    Ok(dest)
}

/// Builds the release binaries and the DPI libraries, assembles the archive,
/// writes `SHA256SUMS` and checks the archive from a clean directory.
pub fn run(root: &Path) -> bool {
    match build_and_pack(root) {
        Ok(archive) => {
            eprintln!("xtask: dist ok: {}", archive.display());
            true
        }
        Err(e) => {
            eprintln!("xtask: dist failed: {e}");
            false
        }
    }
}

/// `cargo xtask dist-verify [ARCHIVE]`: checks one archive (default: the
/// newest one in `dist/`) from a clean directory.
pub fn verify_cli(archive: Option<&str>) -> bool {
    let archive = match archive {
        Some(a) => PathBuf::from(a),
        None => match newest_archive(&workspace_root().join("dist")) {
            Some(a) => a,
            None => {
                eprintln!("xtask: no zynqsim-* archive in dist/; run `cargo xtask dist` first");
                return false;
            }
        },
    };
    match verify_archive(&archive) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("xtask: dist-verify failed: {e}");
            false
        }
    }
}

fn build_and_pack(root: &Path) -> Result<PathBuf, String> {
    cargo(
        root,
        &[
            "build",
            "--release",
            "-p",
            "zynqsim",
            "-p",
            "zynqsim-gui",
            "-p",
            "zynqsim-mcp",
        ],
    )?;
    cargo(root, DPI_BUILD)?;
    // The shared object is shipped when it builds; the archive is complete without it.
    let shared = match build_dpi_shared(root) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("xtask: shared DPI library left out of the archive: {e}");
            false
        }
    };

    let (os, arch) = (env::consts::OS, env::consts::ARCH);
    let name = dir_name(env!("CARGO_PKG_VERSION"), os, arch);
    let dist = root.join("dist");
    let stage = dist.join(&name);
    let _ = fs::remove_dir_all(&stage);
    assemble(root, &stage, shared)?;
    eprintln!("xtask: assembled {}", stage.display());

    let archive = dist.join(archive_name(&name, os));
    let _ = fs::remove_file(&archive);
    make_archive(&dist, &name, &archive)?;
    let hash = sha256(&archive)?;
    let file = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    fs::write(dist.join("SHA256SUMS"), format!("{hash}  {file}\n"))
        .map_err(|e| format!("cannot write SHA256SUMS: {e}"))?;
    let size = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
    eprintln!("xtask: {size} bytes, sha256 {hash}");

    verify_archive(&archive)?;
    Ok(archive)
}

fn cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    eprintln!("\n==> cargo {}", args.join(" "));
    let program = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

/// Copies the files of the archive into `stage`. `shared` says whether the
/// shared-object DPI library was built in this run.
fn assemble(root: &Path, stage: &Path, shared: bool) -> Result<(), String> {
    let exe = env::consts::EXE_SUFFIX;
    let release = root.join("target").join("release");
    for p in PROGRAMS {
        copy_file(
            &release.join(format!("{p}{exe}")),
            &stage.join(format!("{p}{exe}")),
        )?;
    }
    let dpi = root
        .join("target")
        .join("dpi")
        .join("release")
        .join(DPI_ARCHIVE);
    if !dpi.is_file() {
        return Err(format!(
            "{} was not produced; on Windows the XSim archive needs the GNU toolchain \
             (stable-x86_64-pc-windows-gnu)",
            dpi.display()
        ));
    }
    copy_file(&dpi, &stage.join("lib").join(DPI_ARCHIVE))?;
    if shared {
        let file = dpi_shared_file();
        let built = root.join("target").join("dpi").join("release").join(&file);
        copy_file(&built, &stage.join("lib").join(&file))?;
    }
    copy_tree(&root.join("examples"), &stage.join("examples"), &|_| true)?;
    copy_tree(&root.join("boards"), &stage.join("boards"), &|_| true)?;
    copy_tree(&root.join("docs"), &stage.join("docs"), &|p| {
        p.is_dir() || p.extension().is_some_and(|e| e == "md")
    })?;
    for f in ["README.md", "THIRD_PARTY.md"] {
        copy_file(&root.join(f), &stage.join(f))?;
    }
    for entry in fs::read_dir(root).map_err(|e| format!("cannot read {}: {e}", root.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && (name.starts_with("LICENSE") || name.starts_with("NOTICE")) {
            copy_file(&entry.path(), &stage.join(&name))?;
        }
    }
    copy_fixtures(root, stage)?;
    let (version, os, arch) = (
        env!("CARGO_PKG_VERSION"),
        env::consts::OS,
        env::consts::ARCH,
    );
    fs::write(stage.join("INSTALL.txt"), install_txt(version, os, arch))
        .map_err(|e| format!("cannot write INSTALL.txt: {e}"))
}

/// The fixtures that the examples mention, plus the fixtures README.
fn copy_fixtures(root: &Path, stage: &Path) -> Result<(), String> {
    let mut wanted = BTreeSet::from(["README.md".to_string()]);
    collect_fixture_refs(&root.join("examples"), &mut wanted)?;
    let mut copied = 0usize;
    for rel in &wanted {
        let src = root.join("fixtures").join(rel);
        let dst = stage.join("fixtures").join(rel);
        if src.is_dir() {
            copy_tree(&src, &dst, &|_| true)?;
            copied += 1;
        } else if src.is_file() {
            copy_file(&src, &dst)?;
            copied += 1;
        }
    }
    eprintln!("xtask: {copied} fixture files or directories referenced by examples");
    Ok(())
}

fn collect_fixture_refs(dir: &Path, out: &mut BTreeSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_fixture_refs(&path, out)?;
        } else if let Ok(text) = fs::read_to_string(&path) {
            out.extend(fixture_refs_in(&text));
        }
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.is_file() {
        return Err(format!("missing {}", src.display()));
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} to {}: {e}", src.display(), dst.display()))
}

/// Recursive copy of the entries of `src` that `keep` accepts (directories are
/// always descended into; `keep` is asked about files and directories).
fn copy_tree(src: &Path, dst: &Path, keep: &dyn Fn(&Path) -> bool) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("cannot read {}: {e}", src.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if !keep(&path) {
            continue;
        }
        let to = dst.join(path.file_name().unwrap_or_default());
        if path.is_dir() {
            copy_tree(&path, &to, keep)?;
        } else {
            copy_file(&path, &to)?;
        }
    }
    Ok(())
}

/// The archive, with the directory `dir` at its top level.
fn make_archive(dist: &Path, dir: &str, archive: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new(system_tar());
        c.args(["--format", "zip", "-cf"]);
        c
    } else {
        let mut c = Command::new("tar");
        c.arg("-czf");
        c
    };
    cmd.arg(archive).arg("-C").arg(dist).arg(dir);
    run_cmd(&mut cmd, "tar")
}

/// Windows ships bsdtar as `System32\tar.exe`, which writes zip files; a `tar`
/// earlier on PATH (Git's GNU tar) does not.
fn system_tar() -> PathBuf {
    env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("tar.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("tar"))
}

fn run_cmd(cmd: &mut Command, what: &str) -> Result<(), String> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{what} failed with {s}")),
        Err(e) => Err(format!("cannot run {what}: {e}")),
    }
}

/// SHA-256 of a file as lower-case hex, from the system's own tool.
pub fn sha256(path: &Path) -> Result<String, String> {
    let out = if cfg!(windows) {
        let quoted = format!("'{}'", path.display().to_string().replace('\'', "''"));
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "(Get-FileHash -Algorithm SHA256 -LiteralPath {quoted}).Hash"
            ))
            .output()
            .map_err(|e| format!("cannot run powershell: {e}"))?
    } else {
        match Command::new("sha256sum").arg(path).output() {
            Ok(o) => o,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Command::new("shasum")
                .args(["-a", "256"])
                .arg(path)
                .output()
                .map_err(|e| format!("neither sha256sum nor shasum can be run: {e}"))?,
            Err(e) => return Err(format!("cannot run sha256sum: {e}")),
        }
    };
    if !out.status.success() {
        return Err(format!("checksum of {} failed", path.display()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let hash = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(hash)
    } else {
        Err(format!("no SHA-256 from the checksum tool: {text:?}"))
    }
}

/// The archive's top-level directory name (without `.zip` or `.tar.gz`).
fn archive_stem(archive: &Path) -> Option<String> {
    let name = archive.file_name()?.to_str()?;
    name.strip_suffix(".tar.gz")
        .or_else(|| name.strip_suffix(".zip"))
        .map(str::to_string)
}

/// Unpacks `archive` into a fresh temporary directory, runs the programs there
/// with a PATH that holds no toolchain, and removes the directory when every
/// check passes. On failure the directory is kept and named in the error.
pub fn verify_archive(archive: &Path) -> Result<(), String> {
    let name = archive_stem(archive)
        .ok_or_else(|| format!("{} is not a .zip or .tar.gz archive", archive.display()))?;
    let work = unique_temp_dir()?;
    match check_unpacked(archive, &name, &work) {
        Ok(steps) => {
            for s in &steps {
                eprintln!("verify: {s}");
            }
            let _ = fs::remove_dir_all(&work);
            eprintln!("verify: {} checks passed in a clean directory", steps.len());
            Ok(())
        }
        Err(e) => Err(format!("{e} (unpacked copy kept in {})", work.display())),
    }
}

fn check_unpacked(archive: &Path, name: &str, work: &Path) -> Result<Vec<String>, String> {
    extract(archive, work)?;
    let root = work.join(name);
    if !root.is_dir() {
        return Err(format!("the archive has no top-level {name}/ directory"));
    }
    let tmp = work.join("tmp");
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let exe = |p: &str| root.join(format!("{p}{}", env::consts::EXE_SUFFIX));
    let version = env!("CARGO_PKG_VERSION");
    let mut steps = Vec::new();

    let lib = root.join("lib").join(DPI_ARCHIVE);
    let lib_bytes = fs::read(&lib).map_err(|e| format!("{}: {e}", lib.display()))?;
    if !lib_bytes.windows(7).any(|w| w == b"rp_open") {
        return Err(format!("{} does not export rp_open", lib.display()));
    }
    steps.push("lib/libxsim_dpi.a is present and exports rp_open".to_string());

    // The shared object is optional in the archive; when present it must export the
    // same names (its export table holds them as plain strings).
    let shared = root.join("lib").join(dpi_shared_file());
    if shared.is_file() {
        let bytes = fs::read(&shared).map_err(|e| format!("{}: {e}", shared.display()))?;
        if !bytes.windows(7).any(|w| w == b"rp_open") {
            return Err(format!("{} does not export rp_open", shared.display()));
        }
        steps.push(format!(
            "lib/{} is present and exports rp_open",
            dpi_shared_file()
        ));
    }

    let out = clean_command(&exe("zynqsim"), &root, &tmp)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot start zynqsim: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || text != format!("zynqsim {version}") {
        return Err(format!("zynqsim --version gave {text:?} ({})", out.status));
    }
    steps.push(format!("zynqsim --version: {text}"));

    let out = clean_command(&exe("zynqsim"), &root, &tmp)
        .args([
            "run",
            "--project",
            "examples/demo/project.toml",
            "--for",
            "1ms",
            "--json",
        ])
        .output()
        .map_err(|e| format!("cannot start zynqsim run: {e}"))?;
    let report: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("zynqsim run --json printed no JSON ({}): {e}", out.status))?;
    if !out.status.success() || report["now_ps"] != json!(DEMO_NOW_PS) {
        return Err(format!("zynqsim run gave {} with {report}", out.status));
    }
    steps.push(format!(
        "zynqsim run --project examples/demo/project.toml --for 1ms --json: now_ps {}, trace_hash {}",
        report["now_ps"], report["trace_hash"]
    ));

    steps.push(mcp_handshake(&exe("zynqsim-mcp"), &root, &tmp)?);
    Ok(steps)
}

/// A child process with nothing inherited but what a clean machine has: a
/// PATH of system directories only, and a temporary directory of its own.
fn clean_command(program: &Path, cwd: &Path, tmp: &Path) -> Command {
    let mut c = Command::new(program);
    c.current_dir(cwd)
        .env_clear()
        .env("PATH", clean_path())
        .env("TEMP", tmp)
        .env("TMP", tmp)
        .env("TMPDIR", tmp);
    if let Some(sr) = env::var_os("SystemRoot") {
        c.env("SystemRoot", sr);
    }
    c
}

fn clean_path() -> OsString {
    if cfg!(windows) {
        let sr = env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        let dirs = [PathBuf::from(&sr).join("System32"), PathBuf::from(&sr)];
        env::join_paths(dirs).unwrap_or_default()
    } else {
        OsString::from("/usr/bin:/bin")
    }
}

/// Starts `zynqsim-mcp` and checks its `initialize` reply.
fn mcp_handshake(program: &Path, cwd: &Path, tmp: &Path) -> Result<String, String> {
    let mut child = clean_command(program, cwd, tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start zynqsim-mcp: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin for zynqsim-mcp")?;
    let stdout = child.stdout.take().ok_or("no stdout for zynqsim-mcp")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let r = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = tx.send(r);
    });
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "xtask-dist-verify", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    let sent = writeln!(stdin, "{request}").and_then(|()| stdin.flush());
    let reply = match sent {
        Ok(()) => rx.recv_timeout(Duration::from_secs(60)).ok(),
        Err(_) => None,
    };
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let line = reply
        .ok_or("zynqsim-mcp did not answer initialize within 60 s")?
        .map_err(|e| format!("reading zynqsim-mcp: {e}"))?;
    let v: Value = serde_json::from_str(line.trim())
        .map_err(|e| format!("zynqsim-mcp replied with non-JSON {line:?}: {e}"))?;
    let server = v.pointer("/result/serverInfo/name").and_then(Value::as_str);
    let proto = v.pointer("/result/protocolVersion").and_then(Value::as_str);
    if server != Some("zynqsim-mcp") || proto.is_none() {
        return Err(format!("unexpected initialize reply: {v}"));
    }
    Ok(format!(
        "zynqsim-mcp initialize: serverInfo zynqsim-mcp, protocol {}",
        proto.unwrap_or_default()
    ))
}

fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    let is_zip = archive.extension().is_some_and(|e| e == "zip");
    if is_zip {
        // PowerShell's own unzip, so the check does not depend on the archiver
        // that wrote the file.
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
        let script = format!(
            "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
            q(archive),
            q(dest)
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(script);
        run_cmd(&mut cmd, "Expand-Archive")
    } else {
        let mut cmd = Command::new("tar");
        cmd.arg("-xzf").arg(archive).arg("-C").arg(dest);
        run_cmd(&mut cmd, "tar -xzf")
    }
}

fn unique_temp_dir() -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = env::temp_dir().join(format!("zynqsim-unpack-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_mentions_in_project_files_and_comments_are_found() {
        let text = concat!(
            "source = \"../../fixtures/handoff/design.xsa\"\n",
            "# see fixtures/rtl/pmod/spi_master.v. and `fixtures/xsa/`\n",
            "elf = \"../../fixtures/elf/rust_hello.elf\"\n",
            "not_a_fixture = \"myfixtures/x.v\"\n",
            "escape = \"../../fixtures/../secret\"\n",
        );
        let got: Vec<String> = fixture_refs_in(text).into_iter().collect();
        assert_eq!(
            got,
            vec![
                "elf/rust_hello.elf",
                "handoff/design.xsa",
                "rtl/pmod/spi_master.v",
                "xsa",
            ]
        );
    }

    #[test]
    fn names_follow_the_platform() {
        let dir = dir_name("0.1.0", "windows", "x86_64");
        assert_eq!(dir, "zynqsim-0.1.0-windows-x86_64");
        assert_eq!(archive_name(&dir, "windows"), format!("{dir}.zip"));
        assert_eq!(archive_name(&dir, "linux"), format!("{dir}.tar.gz"));
        assert_eq!(
            archive_stem(Path::new("/x/zynqsim-1-linux-x.tar.gz")).as_deref(),
            Some("zynqsim-1-linux-x")
        );
        assert_eq!(archive_stem(Path::new("out.rar")), None);
    }
}
