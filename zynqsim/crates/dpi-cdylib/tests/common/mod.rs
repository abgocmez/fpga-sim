//! Helpers shared by the ABI tests: where the shared object is, the DPI names
//! the generated testbenches import, and the export table of a Windows image.

// Each test binary uses a different subset of these.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The zynqsim root (the directory with `fixtures/` and `crates/`).
pub fn zynqsim_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The shared object of this test profile: the one `cargo build -p dpi-cdylib`
/// writes into the same target directory (`ZYNQSIM_DPI_CDYLIB` overrides it).
pub fn cdylib_path() -> PathBuf {
    if let Some(p) = std::env::var_os("ZYNQSIM_DPI_CDYLIB") {
        return PathBuf::from(p);
    }
    // The test executable is target/<profile>/deps/<name>-<hash>[.exe].
    let exe = std::env::current_exe().expect("the test executable's path");
    let profile = exe
        .parent()
        .and_then(Path::parent)
        .expect("target/<profile>/deps layout");
    profile.join(format!(
        "{}zynqsim_dpi{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ))
}

/// The shared object. `cargo test` does not build a `cdylib` target, so when it is
/// missing this builds it into the same target directory and profile first.
pub fn require_cdylib() -> PathBuf {
    let path = cdylib_path();
    if !path.is_file() && std::env::var_os("ZYNQSIM_DPI_CDYLIB").is_none() {
        build_cdylib(&path);
    }
    assert!(
        path.is_file(),
        "the shared object is missing at {}; build it with `cargo build -p dpi-cdylib` \
         (or `cargo xtask dpi-shared`), or set ZYNQSIM_DPI_CDYLIB",
        path.display()
    );
    path
}

fn build_cdylib(path: &Path) {
    let profile_dir = path.parent().expect("target/<profile>");
    let profile = profile_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let target_dir = profile_dir.parent().expect("target directory");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(zynqsim_root())
        .args(["build", "-p", "dpi-cdylib", "--target-dir"])
        .arg(target_dir);
    match profile {
        "debug" => {}
        "release" => {
            cmd.arg("--release");
        }
        other => panic!("cannot build the shared object for profile `{other}`"),
    }
    let status = cmd.status().expect("cargo runs");
    assert!(status.success(), "cargo build -p dpi-cdylib failed");
}

/// Every `import "DPI-C" ... name(...)` in SystemVerilog text.
pub fn dpi_imports(text: &str) -> BTreeSet<String> {
    text.match_indices("import \"DPI-C\"")
        .filter_map(|(at, _)| {
            let rest = &text[at..];
            let decl = &rest[..rest.find(';')?];
            let open = decl.find('(')?;
            decl[..open].split_whitespace().last().map(str::to_string)
        })
        .collect()
}

/// The DPI imports of every `.sv` file under `dir`, recursively.
pub fn dpi_imports_under(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        panic!("cannot read {}", dir.display());
    };
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            out.extend(dpi_imports_under(&path));
        } else if path.extension().is_some_and(|e| e == "sv") {
            let text = std::fs::read_to_string(&path).expect("readable SystemVerilog");
            out.extend(dpi_imports(&text));
        }
    }
    out
}

/// The names in the export table of a PE32+ image (a Windows DLL).
pub fn pe_exports(bytes: &[u8]) -> BTreeSet<String> {
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
    let u32_at =
        |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().expect("four bytes")) as usize;
    let pe = u32_at(0x3c);
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0", "not a PE image");
    let sections = u16_at(pe + 6);
    let opt = pe + 24;
    assert_eq!(u16_at(opt), 0x20b, "not a PE32+ image");
    let export_dir_rva = u32_at(opt + 112);
    let section_table = opt + u16_at(pe + 20);

    // RVA to file offset, through the section table.
    let offset_of = |rva: usize| {
        (0..sections).find_map(|i| {
            let s = section_table + 40 * i;
            let (vsize, va, raw_size, raw) = (
                u32_at(s + 8),
                u32_at(s + 12),
                u32_at(s + 16),
                u32_at(s + 20),
            );
            let size = vsize.max(raw_size);
            (rva >= va && rva < va + size).then(|| rva - va + raw)
        })
    };
    let Some(dir) = offset_of(export_dir_rva).filter(|_| export_dir_rva != 0) else {
        return BTreeSet::new();
    };
    let count = u32_at(dir + 24);
    let names = offset_of(u32_at(dir + 32)).expect("export name table");
    (0..count)
        .map(|i| {
            let name = offset_of(u32_at(names + 4 * i)).expect("export name");
            let end = bytes[name..].iter().position(|&b| b == 0).expect("NUL") + name;
            String::from_utf8_lossy(&bytes[name..end]).into_owned()
        })
        .collect()
}
