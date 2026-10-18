//! Reads a Vivado hardware handoff: an `.xsa` archive or a bare `.hwh` file.
//!
//! The result is a [`Design`]: the Zynq PS7 instance and its configuration, every other IP
//! instance with its VLNV, address ranges, interrupt and clock connections, plus the HDL source
//! files and `ps7_init` files the archive holds. Nothing here knows about ZynqSim projects;
//! `sim-api::xsa` turns a [`Design`] into one.
//!
//! The HWH schema is Vivado's, not a published standard: every field read is listed in
//! `docs/adr/015-xsa-import.md`, and the reader is tolerant (a missing attribute is `None`, an
//! unknown element is ignored) so a newer Vivado degrades into fewer findings, not a failure.

mod catalog;
mod filelist;
mod hwh;
mod model;
pub mod ps7_init;

pub use catalog::{IpClass, classify};
pub use filelist::{FileListEntry, parse_filelist};
pub use hwh::parse_hwh;
pub use model::{
    AddrRange, BusInterface, ClockLink, Design, FclkInfo, IntcLink, IpInstance, IrqLink, MioPin,
    Port, PortConnection, Ps7, PsPeripheral, ResetLink, SourceFile, SourceKind,
};
pub use ps7_init::{ClockTree, Ps7InitOp, derive_clock_tree, parse_ps7_init_tcl};

use std::io::Read;
use std::path::Path;

/// Why a handoff could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XsaError(pub String);

impl std::fmt::Display for XsaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for XsaError {}

/// What was found in an `.xsa` archive besides the HWH.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Archive {
    /// Every entry name with its uncompressed size (empty for a bare `.hwh`).
    pub entries: Vec<(String, u64)>,
    /// HDL sources and file lists found in the archive.
    pub sources: Vec<SourceFile>,
    /// `ps7_init.*` files present (names only).
    pub ps7_init: Vec<String>,
    /// Parsed operations from `ps7_init.tcl` if present in the archive.
    pub ps7_init_ops: Vec<Ps7InitOp>,
    /// Derived clock tree if `ps7_init.tcl` was present and parsed.
    pub clock_tree: Option<ClockTree>,
    /// A bitstream (`.bit`) is part of the archive.
    pub has_bitstream: bool,
}

/// A hardware handoff and what accompanied it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Handoff {
    /// The parsed design.
    pub design: Design,
    /// Archive contents.
    pub archive: Archive,
}

const MAX_ENTRY: u64 = 64 * 1024 * 1024;

fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
}

/// Reads an `.xsa` (zip) or a `.hwh` (XML) file.
pub fn read(path: &Path) -> Result<Handoff, XsaError> {
    let bytes = std::fs::read(path)
        .map_err(|e| XsaError(format!("cannot read {}: {e}", path.display())))?;
    read_bytes(&bytes, &path.display().to_string())
}

/// Like [`read`] for bytes already in memory; `label` names them in messages.
pub fn read_bytes(bytes: &[u8], label: &str) -> Result<Handoff, XsaError> {
    if !is_zip(bytes) {
        let neither = || {
            XsaError(format!(
                "{label} is neither a zip archive (.xsa) nor UTF-8 XML (.hwh)"
            ))
        };
        let text = std::str::from_utf8(bytes).map_err(|_| neither())?;
        if !text
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with('<')
        {
            return Err(neither());
        }
        let design = parse_hwh(text).map_err(|e| XsaError(format!("{label}: {e}")))?;
        return Ok(Handoff {
            design,
            archive: Archive::default(),
        });
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| XsaError(format!("{label}: not a readable zip archive: {e}")))?;
    let mut archive = Archive::default();
    let mut hwh: Option<(String, String)> = None;
    for i in 0..zip.len() {
        let mut f = zip
            .by_index(i)
            .map_err(|e| XsaError(format!("{label}: entry {i}: {e}")))?;
        if f.is_dir() {
            continue;
        }
        // `enclosed_name` rejects absolute paths and `..` components (zip-slip).
        let Some(name) = f
            .enclosed_name()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
        else {
            return Err(XsaError(format!(
                "{label}: entry `{}` has an unsafe path",
                f.name()
            )));
        };
        let size = f.size();
        archive.entries.push((name.clone(), size));
        let lower = name.to_ascii_lowercase();
        let base = lower.rsplit('/').next().unwrap_or(&lower).to_string();
        if base.starts_with("ps7_init") {
            archive.ps7_init.push(name.clone());
        }
        if lower.ends_with(".bit") {
            archive.has_bitstream = true;
        }
        let kind = SourceKind::from_name(&lower);
        let wanted = lower.ends_with(".hwh") || kind.is_some() || base == "ps7_init.tcl";
        if !wanted {
            continue;
        }
        if size > MAX_ENTRY {
            return Err(XsaError(format!(
                "{label}: entry `{name}` is {size} bytes, over the {MAX_ENTRY} byte limit"
            )));
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)
            .map_err(|e| XsaError(format!("{label}: cannot read `{name}`: {e}")))?;
        let text = String::from_utf8_lossy(&buf).into_owned();
        if lower.ends_with(".hwh") {
            if hwh.is_none() {
                hwh = Some((name, text));
            }
        } else if base == "ps7_init.tcl" {
            if let Ok(ops) = parse_ps7_init_tcl(&text) {
                archive.clock_tree = Some(derive_clock_tree(&ops));
                archive.ps7_init_ops = ops;
            }
        } else if let Some(kind) = kind {
            archive.sources.push(SourceFile {
                path: name,
                kind,
                text,
            });
        }
    }
    let (hwh_name, text) = hwh.ok_or_else(|| {
        XsaError(format!(
            "{label}: the archive holds no .hwh file. A hardware platform written before the \
             block design's output products were generated has no handoff: run \
             `generate_target all` on the block design and write the XSA again"
        ))
    })?;
    let mut design = parse_hwh(&text).map_err(|e| XsaError(format!("{label}: {hwh_name}: {e}")))?;
    design.hwh_file = Some(hwh_name);
    Ok(Handoff { design, archive })
}

#[cfg(test)]
mod tests;
