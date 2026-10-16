//! `BOOT.BIN` support (prompt section 12, ADR-018).
//!
//! The reader is the `boot` crate (re-exported here, so front ends keep depending on
//! `sim-api` only). This module is the glue to projects and engines:
//!
//! * [`validate_file`] feeds `Project::validate` for `[boot] mode = "boot_bin"`;
//! * [`stage_application`] turns the application partitions into one ELF in the run
//!   directory, which the QEMU engine loads like any `[cpu0] elf`;
//! * [`inspect`] is the JSON/text report behind `zynqsim boot-info` and MCP `boot_inspect`;
//! * [`plan_project`] is `zynqsim init --boot`.
//!
//! What is **not** done is stated in [`NOTES`], and repeated by every report.

pub use boot::*;

use crate::error::ApiError;
use crate::project::{BackendsSection, BoardSection, BootSection, CpuSection, Project, SimSection};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a BOOT.BIN boot does not do, in the words every report uses.
pub const NOTES: [&str; 4] = [
    "The FSBL partition is not executed: QEMU's Zynq machine starts the application directly, \
     so no ps7_init, DDR training or clock/MIO setup runs.",
    "The FSBL's register-init table is read and listed, never applied: PS clocks, MIO and \
     peripheral enables have no effect on the simulation.",
    "The PL bitstream partition is recognised and reported but cannot be simulated: the PL is \
     the project's RTL (or native models).",
    "Boot-ROM timing, the QSPI/SD read of the image, encryption and authentication are not \
     modelled; core 0 starts at the first application partition's execution address.",
];

/// `[boot] mode` values that select a boot image: `boot_bin` (prompt) and `bootbin` (alias).
pub fn is_boot_bin_mode(mode: &str) -> bool {
    mode == "boot_bin" || mode == "bootbin"
}

/// A boot image read from disk.
pub struct Loaded {
    /// The file.
    pub bytes: Vec<u8>,
    /// Its structure and validation findings.
    pub image: BootImage,
}

/// Reads and parses `path`. A damaged *structure* is an error here; per-field findings
/// stay in `image.issues`.
pub fn read_image(path: &Path) -> Result<Loaded, ApiError> {
    let bytes = std::fs::read(path)
        .map_err(|e| ApiError::io(format!("cannot read {}: {e}", path.display())))?;
    let image = BootImage::parse(&bytes)
        .map_err(|e| ApiError::invalid_argument(format!("{}: {e}", path.display())))?;
    Ok(Loaded { bytes, image })
}

/// Findings for project validation: `(is_error, message)`.
pub fn validate_file(path: &Path) -> Vec<(bool, String)> {
    match read_image(path) {
        Err(e) => vec![(true, e.message)],
        Ok(l) => {
            let mut v: Vec<(bool, String)> = l
                .image
                .issues
                .iter()
                .map(|i| (i.severity == Severity::Error, i.to_string()))
                .collect();
            if v.iter().all(|(e, _)| !e) {
                if let Err(e) = l.image.load_plan(&l.bytes) {
                    v.push((true, e.message));
                }
            }
            v
        }
    }
}

/// Writes the application partitions of `bin` as `bootbin-app.elf` into `run_dir` (created
/// if needed) and returns its path.
pub fn stage_application(bin: &Path, run_dir: &Path) -> Result<PathBuf, ApiError> {
    let l = read_image(bin)?;
    let plan = l
        .image
        .load_plan(&l.bytes)
        .map_err(|e| ApiError::invalid_argument(format!("{}: {e}", bin.display())))?;
    std::fs::create_dir_all(run_dir)
        .map_err(|e| ApiError::io(format!("cannot create {}: {e}", run_dir.display())))?;
    let out = run_dir.join("bootbin-app.elf");
    std::fs::write(&out, plan.to_elf())
        .map_err(|e| ApiError::io(format!("cannot write {}: {e}", out.display())))?;
    Ok(out)
}

/// The report behind `boot-info --json` and MCP `boot_inspect`.
pub fn inspect(path: &Path) -> Result<Value, ApiError> {
    let l = read_image(path)?;
    let img = &l.image;
    let plan = img.load_plan(&l.bytes);
    let hex = |v: u32| format!("{v:#010x}");
    let partitions: Vec<Value> = img
        .partitions
        .iter()
        .map(|p| {
            json!({
                "index": p.index,
                "image": p.image,
                "kind": p.kind,
                "destination": p.destination,
                "data_offset": p.data_offset,
                "length": p.length,
                "load_address": hex(p.load_address),
                "exec_address": hex(p.exec_address),
                "attributes": hex(p.attributes),
                "checksum": p.checksum_type,
                "checksum_ok": p.checksum_ok,
                "sync_word_found": p.sync_word_found,
            })
        })
        .collect();
    let regs: Vec<Value> = img
        .register_init
        .iter()
        .map(|r| json!({"address": hex(r.address), "value": hex(r.value)}))
        .collect();
    let issue_json = |sev: Severity| -> Vec<Value> {
        img.issues
            .iter()
            .filter(|i| i.severity == sev)
            .map(|i| json!({"field": i.field, "offset": i.offset, "message": i.message}))
            .collect()
    };
    let (runnable, entry, reason) = match &plan {
        Ok(p) => (true, Some(hex(p.entry)), Value::Null),
        Err(e) => (false, None, json!(e.to_string())),
    };
    Ok(json!({
        "file": path.display().to_string(),
        "size": img.file_size,
        "encrypted": img.header.encryption_status != 0,
        "header_version": hex(img.header.header_version),
        "fsbl": {
            "offset": img.header.fsbl_offset,
            "length": img.header.fsbl_length,
            "load_address": hex(img.header.fsbl_load_address),
            "exec_address": hex(img.header.fsbl_exec_address),
            "executed": false,
        },
        "register_init": regs,
        "register_init_applied": false,
        "images": img.images.iter().map(|i| &i.name).collect::<Vec<_>>(),
        "partitions": partitions,
        "bitstream_partitions": img.bitstreams().count(),
        "runnable": runnable,
        "entry": entry,
        "not_runnable_because": reason,
        "other_entries": img.other_entries().iter().map(|p| hex(p.exec_address)).collect::<Vec<_>>(),
        "errors": issue_json(Severity::Error),
        "warnings": issue_json(Severity::Warning),
        "notes": NOTES,
    }))
}

/// The text `zynqsim boot-info` prints.
pub fn inspect_text(path: &Path) -> Result<String, ApiError> {
    let l = read_image(path)?;
    let mut s = l.image.describe();
    match l.image.load_plan(&l.bytes) {
        Ok(p) => s.push_str(&format!(
            "runnable: yes ({} byte(s) in {} segment(s), core 0 starts at {:#010x})\n",
            p.total_bytes(),
            p.segments.len(),
            p.entry
        )),
        Err(e) => s.push_str(&format!("runnable: NO: {e}\n")),
    }
    for n in NOTES {
        s.push_str(&format!("note: {n}\n"));
    }
    Ok(s)
}

/// A project that boots a BOOT.BIN.
pub struct BootInit {
    /// `project.toml` text.
    pub project_toml: String,
    /// File name the image gets inside the project directory.
    pub image_name: String,
    /// The parsed image, for the caller's report.
    pub loaded: Loaded,
}

/// Plans `init --boot BIN`: the project refers to a copy of the image next to it.
/// Refuses an image ZynqSim could not run (the project would not validate either).
pub fn plan_project(bin: &Path) -> Result<BootInit, ApiError> {
    let loaded = read_image(bin)?;
    if let Err(e) = loaded.image.load_plan(&loaded.bytes) {
        return Err(ApiError::invalid_argument(format!(
            "{}: cannot create a project: {e}",
            bin.display()
        )));
    }
    let image_name = "BOOT.BIN".to_string();
    let project = Project {
        schema_version: 1,
        board: BoardSection::named("zybo-z7-20"),
        backends: BackendsSection {
            ps: "qemu".to_string(),
            // The bitstream cannot be simulated: add [rtl] (or [native]) to give the PL a
            // model. Without one the PL side is the built-in placeholder.
            pl: "demo".to_string(),
        },
        sim: SimSection::default(),
        boot: Some(BootSection {
            mode: "boot_bin".to_string(),
            file: Some(image_name.clone()),
        }),
        cpu0: None::<CpuSection>,
        cpu1: None,
        rtl: None,
        connect: BTreeMap::new(),
        xsa: None,
        devices: BTreeMap::new(),
        device_values: BTreeMap::new(),
        host: BTreeMap::new(),
        native: BTreeMap::new(),
        native_params: BTreeMap::new(),
        media: None,
        debug: None,
        network: None,
        storage: None,
    };
    let mut header = format!(
        "# Generated by `zynqsim init --boot` from {}.\n",
        bin.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    );
    for n in NOTES {
        header.push_str(&format!("# {n}\n"));
    }
    header.push_str(
        "# To simulate the PL, add an [rtl] section (and set backends.pl = \"xsim\") with the RTL\n\
         # the bitstream was built from.\n",
    );
    Ok(BootInit {
        project_toml: format!("{header}{}", project.to_toml()?),
        image_name,
        loaded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/boot/BOOT.BIN")
    }

    #[test]
    fn inspect_reports_partitions_and_what_is_not_done() {
        let v = inspect(&fixture()).unwrap();
        assert_eq!(v["runnable"], true);
        assert_eq!(v["entry"], "0x00100000");
        assert_eq!(v["partitions"].as_array().unwrap().len(), 4);
        assert_eq!(v["partitions"][1]["kind"], "bitstream");
        assert_eq!(v["fsbl"]["executed"], false);
        assert_eq!(v["register_init_applied"], false);
        assert_eq!(v["register_init"][1]["address"], "0xf8000120");
        assert!(v["errors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn staged_elf_is_a_loadable_arm_executable() {
        let dir = std::env::temp_dir().join(format!("zs-bootbin-{}", std::process::id()));
        let p = stage_application(&fixture(), &dir).unwrap();
        let elf = std::fs::read(&p).unwrap();
        assert_eq!(&elf[..4], b"\x7fELF");
        assert_eq!(
            u32::from_le_bytes([elf[24], elf[25], elf[26], elf[27]]),
            0x10_0000
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_plan_validates_and_refuses_unrunnable_images() {
        let init = plan_project(&fixture()).unwrap();
        assert!(init.project_toml.contains("mode = \"boot_bin\""));
        assert!(init.project_toml.contains("file = \"BOOT.BIN\""));
        assert!(init.project_toml.contains("not executed"));
        let dir = std::env::temp_dir().join(format!("zs-bootbin-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut bad = std::fs::read(fixture()).unwrap();
        bad[0x44] ^= 1;
        let p = dir.join("bad.bin");
        std::fs::write(&p, bad).unwrap();
        let e = plan_project(&p).err().unwrap();
        assert!(e.message.contains("checksum"), "{}", e.message);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
