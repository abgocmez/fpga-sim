//! A minimal flattened-device-tree editor, enough to adapt the Xilinx co-sim
//! device tree for the board's QSPI flash.
//!
//! Two things in `zynq-arm-cosim.dtb` do not fit the Zybo Z7-20 and cannot be
//! changed from QEMU's command line:
//!
//! * the QSPI controller node lists only its register window, so the 32 MiB
//!   *linear* (memory-mapped read) window at `0xFC000000` exists in the device
//!   model but is never mapped. Appending a second `reg` pair maps it;
//! * the flash child node names a Micron `n25q128a13`, while the Zybo carries a
//!   Spansion S25FL128S. Rewriting `compatible` instantiates the closest QEMU
//!   model instead.
//!
//! The editor re-encodes the structure block only; strings, the reserved-memory
//! map and everything else are copied through unchanged. There is no `dtc`
//! on the machines this runs on, and the result is a pure function of the
//! input, so runs stay reproducible.

use std::fmt;

const MAGIC: u32 = 0xD00D_FEED;
const BEGIN_NODE: u32 = 1;
const END_NODE: u32 = 2;
const PROP: u32 = 3;
const NOP: u32 = 4;
const END: u32 = 9;
const HEADER_LEN: usize = 40;

/// Why a device tree could not be edited.
#[derive(Debug, PartialEq, Eq)]
pub enum DtbError {
    /// The blob is not a flattened device tree this editor understands.
    Malformed(&'static str),
    /// A node or property that has to be there is missing.
    Missing(String),
}

impl fmt::Display for DtbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DtbError::Malformed(m) => write!(f, "malformed device tree: {m}"),
            DtbError::Missing(m) => write!(f, "device tree has no {m}"),
        }
    }
}

impl std::error::Error for DtbError {}

fn be32(b: &[u8], at: usize) -> Result<u32, DtbError> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(DtbError::Malformed("truncated"))
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

/// The unit-less name of a node (`spi` of `spi@e000d000`) is not enough to
/// find the controller: match the full node name.
const QSPI_NODE: &str = "spi@e000d000";
const FLASH_NODE: &str = "flash@0";
/// Where the Zynq maps the QSPI linear window, and the model's size of it.
pub const LQSPI_BASE: u32 = 0xFC00_0000;
/// Size of the linear window (two 16 MiB chip selects).
pub const LQSPI_SIZE: u32 = 0x0200_0000;

/// What [`patch_qspi`] changed.
#[derive(Debug, PartialEq, Eq)]
pub struct Patched {
    /// The new blob.
    pub dtb: Vec<u8>,
    /// True when the linear window was added (it was missing).
    pub window_added: bool,
    /// True when the flash node's `compatible` was rewritten.
    pub flash_changed: bool,
}

/// Maps the QSPI linear window and, if `flash_compatible` is given, replaces
/// the flash child's `compatible` string (for example `spansion,s25fl129p0`).
///
/// Fails when the tree has no `spi@e000d000` node with a `reg`, or no
/// `flash@0` child when a new `compatible` was asked for.
pub fn patch_qspi(dtb: &[u8], flash_compatible: Option<&str>) -> Result<Patched, DtbError> {
    if dtb.len() < HEADER_LEN || be32(dtb, 0)? != MAGIC {
        return Err(DtbError::Malformed("bad magic"));
    }
    let total = be32(dtb, 4)? as usize;
    let off_struct = be32(dtb, 8)? as usize;
    let off_strings = be32(dtb, 12)? as usize;
    let size_strings = be32(dtb, 32)? as usize;
    let size_struct = be32(dtb, 36)? as usize;
    if total > dtb.len() || off_struct + size_struct > total || off_strings + size_strings > total {
        return Err(DtbError::Malformed("section outside the blob"));
    }
    let st = &dtb[off_struct..off_struct + size_struct];
    let strings = &dtb[off_strings..off_strings + size_strings];

    let mut out: Vec<u8> = Vec::with_capacity(st.len() + 64);
    let mut stack: Vec<String> = Vec::new();
    let (mut window_added, mut flash_changed) = (false, false);
    let mut pos = 0usize;
    loop {
        let tok = be32(st, pos)?;
        match tok {
            BEGIN_NODE => {
                let rest = st
                    .get(pos + 4..)
                    .ok_or(DtbError::Malformed("truncated node"))?;
                let nul = rest
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or(DtbError::Malformed("unterminated node name"))?;
                let name = String::from_utf8_lossy(&rest[..nul]).into_owned();
                let n = 4 + pad4(nul + 1);
                out.extend_from_slice(&st[pos..pos + n]);
                pos += n;
                stack.push(name);
            }
            END_NODE => {
                stack.pop();
                out.extend_from_slice(&st[pos..pos + 4]);
                pos += 4;
            }
            PROP => {
                let len = be32(st, pos + 4)? as usize;
                let name_off = be32(st, pos + 8)? as usize;
                let value = st
                    .get(pos + 12..pos + 12 + len)
                    .ok_or(DtbError::Malformed("truncated property"))?;
                let n = 12 + pad4(len);
                let pname = strings
                    .get(name_off..)
                    .and_then(|s| s.iter().position(|&b| b == 0).map(|e| &s[..e]))
                    .ok_or(DtbError::Malformed("property name outside strings"))?;
                let in_qspi = stack.last().is_some_and(|s| s == QSPI_NODE);
                let in_flash = stack.len() >= 2
                    && stack[stack.len() - 1] == FLASH_NODE
                    && stack[stack.len() - 2] == QSPI_NODE;
                if in_qspi && pname == b"reg" && len == 8 {
                    let mut v = value.to_vec();
                    v.extend_from_slice(&LQSPI_BASE.to_be_bytes());
                    v.extend_from_slice(&LQSPI_SIZE.to_be_bytes());
                    push_prop(&mut out, name_off, &v);
                    window_added = true;
                } else if in_qspi
                    && pname == b"reg"
                    && len == 16
                    && value[8..12] == LQSPI_BASE.to_be_bytes()
                {
                    // Already mapped: copy through.
                    out.extend_from_slice(&st[pos..pos + n]);
                } else if in_flash && pname == b"compatible" && flash_compatible.is_some() {
                    let mut v = flash_compatible.unwrap_or_default().as_bytes().to_vec();
                    v.push(0);
                    push_prop(&mut out, name_off, &v);
                    flash_changed = true;
                } else {
                    out.extend_from_slice(&st[pos..pos + n]);
                }
                pos += n;
            }
            NOP => {
                out.extend_from_slice(&st[pos..pos + 4]);
                pos += 4;
            }
            END => {
                out.extend_from_slice(&st[pos..pos + 4]);
                break;
            }
            _ => return Err(DtbError::Malformed("unknown structure token")),
        }
    }
    if !window_added && !has_window(dtb)? {
        return Err(DtbError::Missing(format!(
            "`{QSPI_NODE}` node with a `reg`"
        )));
    }
    if flash_compatible.is_some() && !flash_changed {
        return Err(DtbError::Missing(format!(
            "`{FLASH_NODE}` child of `{QSPI_NODE}` with a `compatible`"
        )));
    }

    // Reassemble: everything before the structure block, the new structure
    // block, then everything after the old one (strings and any tail), with
    // the header offsets that moved fixed up.
    let old_end = off_struct + size_struct;
    let mut res = Vec::with_capacity(dtb.len() + 64);
    res.extend_from_slice(&dtb[..off_struct]);
    res.extend_from_slice(&out);
    // Keep the next section 4-byte aligned (the structure block always is).
    res.extend_from_slice(&dtb[old_end..]);
    let delta = out.len() as i64 - size_struct as i64;
    let shift = |off: usize| -> u32 {
        if off >= old_end {
            (off as i64 + delta) as u32
        } else {
            off as u32
        }
    };
    let new_total = (total as i64 + delta) as u32;
    res[4..8].copy_from_slice(&new_total.to_be_bytes());
    res[12..16].copy_from_slice(&shift(off_strings).to_be_bytes());
    res[36..40].copy_from_slice(&(out.len() as u32).to_be_bytes());
    res.truncate(new_total as usize);
    Ok(Patched {
        dtb: res,
        window_added,
        flash_changed,
    })
}

fn push_prop(out: &mut Vec<u8>, name_off: usize, value: &[u8]) {
    out.extend_from_slice(&PROP.to_be_bytes());
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(&(name_off as u32).to_be_bytes());
    out.extend_from_slice(value);
    out.resize(out.len() + (pad4(value.len()) - value.len()), 0);
}

/// One property rewrite for [`patch_ps_blocks`]: the value of `prop` on the
/// node whose unit name is `node` (for example `spi@e0006000`) becomes `value`.
pub struct PropEdit {
    /// Unit name of the node, for example `spi@e0006000`.
    pub node: &'static str,
    /// Property to replace on that node.
    pub prop: &'static str,
    /// New string value (a NUL is appended).
    pub value: &'static str,
}

/// The PS blocks of the co-sim tree that this QEMU build does not instantiate
/// under the names the tree gives them (see docs/adr/024-ps-blocks.md). Each
/// entry replaces a `compatible` (the string QEMU's FDT table recognises) or a
/// `status` of a node that is already in the tree.
pub const PS_BLOCK_EDITS: &[PropEdit] = &[
    PropEdit {
        node: "gpio@e000a000",
        prop: "compatible",
        value: "xlnx,zynqmp-gpio",
    },
    PropEdit {
        node: "spi@e0006000",
        prop: "compatible",
        value: "cdns,spi-r1p6",
    },
    PropEdit {
        node: "spi@e0006000",
        prop: "status",
        value: "okay",
    },
    PropEdit {
        node: "spi@e0007000",
        prop: "compatible",
        value: "cdns,spi-r1p6",
    },
    PropEdit {
        node: "spi@e0007000",
        prop: "status",
        value: "okay",
    },
];

/// Applies [`PS_BLOCK_EDITS`]. Returns the new blob and the number of edits that
/// found their node and property (the tree is unchanged when that is zero).
pub fn patch_ps_blocks(dtb: &[u8]) -> Result<(Vec<u8>, usize), DtbError> {
    apply_prop_edits(dtb, PS_BLOCK_EDITS)
}

/// Edits that every run gets, whatever the project says: the Xilinx CAN
/// controller at `0xE0008000`, which this build maps under `xlnx,ps7-can-1.00.a`
/// (ADR-029). Applied by [`patch_default_blocks`].
pub const DEFAULT_EDITS: &[PropEdit] = &[PropEdit {
    node: "can@e0008000",
    prop: "compatible",
    value: "xlnx,ps7-can-1.00.a",
}];

/// Applies [`DEFAULT_EDITS`]. Returns the new blob and the number of edits
/// that found their node and property.
pub fn patch_default_blocks(dtb: &[u8]) -> Result<(Vec<u8>, usize), DtbError> {
    apply_prop_edits(dtb, DEFAULT_EDITS)
}

/// Replaces the properties `edits` name (see [`PropEdit`]). Returns the blob
/// and how many edits found their node and changed a value.
fn apply_prop_edits(dtb: &[u8], edits: &[PropEdit]) -> Result<(Vec<u8>, usize), DtbError> {
    let mut applied = vec![false; edits.len()];
    let out = edit_structure(dtb, |stack, pname, value| {
        let node = stack.last()?;
        let i = edits
            .iter()
            .position(|e| e.node == node.as_str() && e.prop.as_bytes() == pname)?;
        let mut v = edits[i].value.as_bytes().to_vec();
        v.push(0);
        if v == value {
            return None;
        }
        applied[i] = true;
        Some(v)
    })?;
    Ok((out, applied.iter().filter(|a| **a).count()))
}

/// Rewrites the properties for which `edit` returns a new value, copying every
/// other token through. `edit` gets the stack of node names, the property name
/// and its value. Only the structure block changes; the header is fixed up.
fn edit_structure(
    dtb: &[u8],
    mut edit: impl FnMut(&[String], &[u8], &[u8]) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, DtbError> {
    if dtb.len() < HEADER_LEN || be32(dtb, 0)? != MAGIC {
        return Err(DtbError::Malformed("bad magic"));
    }
    let total = be32(dtb, 4)? as usize;
    let off_struct = be32(dtb, 8)? as usize;
    let off_strings = be32(dtb, 12)? as usize;
    let size_strings = be32(dtb, 32)? as usize;
    let size_struct = be32(dtb, 36)? as usize;
    if total > dtb.len() || off_struct + size_struct > total || off_strings + size_strings > total {
        return Err(DtbError::Malformed("section outside the blob"));
    }
    let st = &dtb[off_struct..off_struct + size_struct];
    let strings = &dtb[off_strings..off_strings + size_strings];
    let mut out: Vec<u8> = Vec::with_capacity(st.len() + 64);
    let mut stack: Vec<String> = Vec::new();
    let mut pos = 0usize;
    loop {
        match be32(st, pos)? {
            BEGIN_NODE => {
                let rest = st
                    .get(pos + 4..)
                    .ok_or(DtbError::Malformed("truncated node"))?;
                let nul = rest
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or(DtbError::Malformed("unterminated node name"))?;
                let n = 4 + pad4(nul + 1);
                out.extend_from_slice(&st[pos..pos + n]);
                stack.push(String::from_utf8_lossy(&rest[..nul]).into_owned());
                pos += n;
            }
            END_NODE => {
                stack.pop();
                out.extend_from_slice(&st[pos..pos + 4]);
                pos += 4;
            }
            PROP => {
                let len = be32(st, pos + 4)? as usize;
                let name_off = be32(st, pos + 8)? as usize;
                let value = st
                    .get(pos + 12..pos + 12 + len)
                    .ok_or(DtbError::Malformed("truncated property"))?;
                let n = 12 + pad4(len);
                let pname = strings
                    .get(name_off..)
                    .and_then(|s| s.iter().position(|&b| b == 0).map(|e| &s[..e]))
                    .ok_or(DtbError::Malformed("property name outside strings"))?;
                match edit(&stack, pname, value) {
                    Some(v) => push_prop(&mut out, name_off, &v),
                    None => out.extend_from_slice(&st[pos..pos + n]),
                }
                pos += n;
            }
            NOP => {
                out.extend_from_slice(&st[pos..pos + 4]);
                pos += 4;
            }
            END => {
                out.extend_from_slice(&st[pos..pos + 4]);
                break;
            }
            _ => return Err(DtbError::Malformed("unknown structure token")),
        }
    }
    let old_end = off_struct + size_struct;
    let mut res = Vec::with_capacity(dtb.len() + 64);
    res.extend_from_slice(&dtb[..off_struct]);
    res.extend_from_slice(&out);
    res.extend_from_slice(&dtb[old_end..]);
    let delta = out.len() as i64 - size_struct as i64;
    let shift = |off: usize| -> u32 {
        if off >= old_end {
            (off as i64 + delta) as u32
        } else {
            off as u32
        }
    };
    let new_total = (total as i64 + delta) as u32;
    res[4..8].copy_from_slice(&new_total.to_be_bytes());
    res[12..16].copy_from_slice(&shift(off_strings).to_be_bytes());
    res[36..40].copy_from_slice(&(out.len() as u32).to_be_bytes());
    res.truncate(new_total as usize);
    Ok(res)
}

/// Unit name of the Cortex-A9 global timer in the co-sim tree (`0xF8F00200`).
pub const GLOBAL_TIMER_NODE: &str = "timer@f8f00200";

/// Sets the `u32` property `name` of the node whose unit name is `node` to
/// `value`, adding it when the node does not have it. The name is appended to
/// the strings block when the tree does not have it yet, which needs the
/// strings block to be the last section of the blob (true for `dtc` output).
///
/// Used for `clock-frequency` on the A9 global timer (see `[ps] cpu_clock`,
/// ADR-029): QEMU's global timer takes its rate from that property, and not
/// from its `clocks` phandle, which this build ignores.
pub fn set_u32_property(
    dtb: &[u8],
    node: &str,
    name: &str,
    value: u32,
) -> Result<Vec<u8>, DtbError> {
    if dtb.len() < HEADER_LEN || be32(dtb, 0)? != MAGIC {
        return Err(DtbError::Malformed("bad magic"));
    }
    let total = be32(dtb, 4)? as usize;
    let off_struct = be32(dtb, 8)? as usize;
    let off_strings = be32(dtb, 12)? as usize;
    let size_strings = be32(dtb, 32)? as usize;
    let size_struct = be32(dtb, 36)? as usize;
    let old_end = off_struct + size_struct;
    if total > dtb.len() || old_end > off_strings || off_strings + size_strings > total {
        return Err(DtbError::Malformed("section outside the blob"));
    }
    let st = &dtb[off_struct..old_end];
    let strings = &dtb[off_strings..off_strings + size_strings];

    // The property name: an existing string, or one appended to the block.
    let needle = [name.as_bytes(), &[0u8]].concat();
    let found = (0..strings.len())
        .find(|&i| strings[i..].starts_with(&needle) && (i == 0 || strings[i - 1] == 0));
    let (name_off, strings_out) = match found {
        Some(i) => (i, strings.to_vec()),
        None => {
            if off_strings + size_strings != total {
                return Err(DtbError::Malformed("strings block is not the last section"));
            }
            let mut s = strings.to_vec();
            s.extend_from_slice(&needle);
            (size_strings, s)
        }
    };
    let exists = property(dtb, &[node], name)?.is_some();

    let mut out: Vec<u8> = Vec::with_capacity(st.len() + 32);
    let mut stack: Vec<String> = Vec::new();
    let mut pos = 0usize;
    let mut done = false;
    loop {
        match be32(st, pos)? {
            BEGIN_NODE => {
                let rest = st
                    .get(pos + 4..)
                    .ok_or(DtbError::Malformed("truncated node"))?;
                let nul = rest
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or(DtbError::Malformed("unterminated node name"))?;
                let n = 4 + pad4(nul + 1);
                out.extend_from_slice(&st[pos..pos + n]);
                let unit = String::from_utf8_lossy(&rest[..nul]).into_owned();
                if !exists && !done && unit == node {
                    push_prop(&mut out, name_off, &value.to_be_bytes());
                    done = true;
                }
                stack.push(unit);
                pos += n;
            }
            END_NODE => {
                stack.pop();
                out.extend_from_slice(&st[pos..pos + 4]);
                pos += 4;
            }
            PROP => {
                let len = be32(st, pos + 4)? as usize;
                let old_off = be32(st, pos + 8)? as usize;
                let n = 12 + pad4(len);
                if stack.last().is_some_and(|s| s == node) {
                    let pname = strings
                        .get(old_off..)
                        .and_then(|s| s.iter().position(|&b| b == 0).map(|e| &s[..e]))
                        .ok_or(DtbError::Malformed("property name outside strings"))?;
                    if pname == name.as_bytes() {
                        push_prop(&mut out, name_off, &value.to_be_bytes());
                        done = true;
                        pos += n;
                        continue;
                    }
                }
                out.extend_from_slice(&st[pos..pos + n]);
                pos += n;
            }
            NOP => {
                out.extend_from_slice(&st[pos..pos + 4]);
                pos += 4;
            }
            END => {
                out.extend_from_slice(&st[pos..pos + 4]);
                break;
            }
            _ => return Err(DtbError::Malformed("unknown structure token")),
        }
    }
    if !done {
        return Err(DtbError::Missing(format!("`{node}` node")));
    }

    let mut res = Vec::with_capacity(total + out.len() + strings_out.len());
    res.extend_from_slice(&dtb[..off_struct]);
    res.extend_from_slice(&out);
    res.extend_from_slice(&dtb[old_end..off_strings]);
    res.extend_from_slice(&strings_out);
    res.extend_from_slice(&dtb[off_strings + size_strings..total]);
    let new_off_strings = off_strings + out.len() - size_struct;
    let new_total = res.len() as u32;
    res[4..8].copy_from_slice(&new_total.to_be_bytes());
    res[12..16].copy_from_slice(&(new_off_strings as u32).to_be_bytes());
    res[32..36].copy_from_slice(&(strings_out.len() as u32).to_be_bytes());
    res[36..40].copy_from_slice(&(out.len() as u32).to_be_bytes());
    Ok(res)
}

/// True when the QSPI node already has the linear window in its `reg`.
fn has_window(dtb: &[u8]) -> Result<bool, DtbError> {
    Ok(property(dtb, &[QSPI_NODE], "reg")?
        .is_some_and(|v| v.len() >= 16 && v[8..12] == LQSPI_BASE.to_be_bytes()))
}

/// The value of property `name` of the node whose name chain ends with `path`
/// (for example `["spi@e000d000", "flash@0"]`), if present.
pub fn property(dtb: &[u8], path: &[&str], name: &str) -> Result<Option<Vec<u8>>, DtbError> {
    if dtb.len() < HEADER_LEN || be32(dtb, 0)? != MAGIC {
        return Err(DtbError::Malformed("bad magic"));
    }
    let off_struct = be32(dtb, 8)? as usize;
    let off_strings = be32(dtb, 12)? as usize;
    let size_strings = be32(dtb, 32)? as usize;
    let size_struct = be32(dtb, 36)? as usize;
    let st = dtb
        .get(off_struct..off_struct + size_struct)
        .ok_or(DtbError::Malformed("structure block outside the blob"))?;
    let strings = dtb
        .get(off_strings..off_strings + size_strings)
        .ok_or(DtbError::Malformed("strings block outside the blob"))?;
    let mut stack: Vec<String> = Vec::new();
    let mut pos = 0usize;
    loop {
        match be32(st, pos)? {
            BEGIN_NODE => {
                let rest = &st[pos + 4..];
                let nul = rest
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or(DtbError::Malformed("unterminated node name"))?;
                stack.push(String::from_utf8_lossy(&rest[..nul]).into_owned());
                pos += 4 + pad4(nul + 1);
            }
            END_NODE => {
                stack.pop();
                pos += 4;
            }
            PROP => {
                let len = be32(st, pos + 4)? as usize;
                let name_off = be32(st, pos + 8)? as usize;
                let value = st
                    .get(pos + 12..pos + 12 + len)
                    .ok_or(DtbError::Malformed("truncated property"))?;
                let pname = strings
                    .get(name_off..)
                    .and_then(|s| s.iter().position(|&b| b == 0).map(|e| &s[..e]))
                    .ok_or(DtbError::Malformed("property name outside strings"))?;
                // `path` names the node's last components, so a caller need not
                // know the parent chain (`amba` in the Xilinx trees).
                if pname == name.as_bytes()
                    && stack.len() > path.len()
                    && stack[stack.len() - path.len()..]
                        .iter()
                        .zip(path)
                        .all(|(a, b)| a == b)
                {
                    return Ok(Some(value.to_vec()));
                }
                pos += 12 + pad4(len);
            }
            NOP => pos += 4,
            END => return Ok(None),
            _ => return Err(DtbError::Malformed("unknown structure token")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a small tree: root { amba { spi@e000d000 { reg, flash@0 { compatible } } } }.
    fn sample(reg_cells: usize) -> Vec<u8> {
        let strings = b"reg\0compatible\0status\0";
        let (s_reg, s_compat, s_status) = (0u32, 4u32, 15u32);
        let mut st: Vec<u8> = Vec::new();
        let node = |st: &mut Vec<u8>, name: &str| {
            st.extend_from_slice(&BEGIN_NODE.to_be_bytes());
            st.extend_from_slice(name.as_bytes());
            st.push(0);
            while st.len() % 4 != 0 {
                st.push(0);
            }
        };
        let prop = |st: &mut Vec<u8>, name_off: u32, v: &[u8]| {
            st.extend_from_slice(&PROP.to_be_bytes());
            st.extend_from_slice(&(v.len() as u32).to_be_bytes());
            st.extend_from_slice(&name_off.to_be_bytes());
            st.extend_from_slice(v);
            while st.len() % 4 != 0 {
                st.push(0);
            }
        };
        node(&mut st, "");
        node(&mut st, "amba");
        node(&mut st, "spi@e000d000");
        let mut reg = Vec::new();
        reg.extend_from_slice(&0xE000_D000u32.to_be_bytes());
        reg.extend_from_slice(&0x1000u32.to_be_bytes());
        if reg_cells == 4 {
            reg.extend_from_slice(&LQSPI_BASE.to_be_bytes());
            reg.extend_from_slice(&LQSPI_SIZE.to_be_bytes());
        }
        prop(&mut st, s_reg, &reg);
        prop(&mut st, s_status, b"okay\0");
        node(&mut st, "flash@0");
        prop(&mut st, s_compat, b"micron,n25q128a13\0");
        st.extend_from_slice(&END_NODE.to_be_bytes()); // flash
        st.extend_from_slice(&END_NODE.to_be_bytes()); // spi
        st.extend_from_slice(&END_NODE.to_be_bytes()); // amba
        st.extend_from_slice(&END_NODE.to_be_bytes()); // root
        st.extend_from_slice(&END.to_be_bytes());

        let off_rsv = HEADER_LEN;
        let off_struct = off_rsv + 16;
        let off_str = off_struct + st.len();
        let total = off_str + strings.len();
        let mut b = Vec::new();
        for v in [
            MAGIC,
            total as u32,
            off_struct as u32,
            off_str as u32,
            off_rsv as u32,
            17,
            16,
            0,
            strings.len() as u32,
            st.len() as u32,
        ] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend_from_slice(&[0u8; 16]); // empty reservation map
        b.extend_from_slice(&st);
        b.extend_from_slice(strings);
        b
    }

    #[test]
    fn the_linear_window_is_appended_and_the_flash_is_renamed() {
        let src = sample(2);
        assert_eq!(
            property(&src, &["amba", QSPI_NODE], "reg")
                .unwrap()
                .unwrap()
                .len(),
            8
        );
        let p = patch_qspi(&src, Some("spansion,s25fl129p0")).unwrap();
        assert!(p.window_added && p.flash_changed);
        let reg = property(&p.dtb, &["amba", QSPI_NODE], "reg")
            .unwrap()
            .unwrap();
        assert_eq!(reg.len(), 16);
        assert_eq!(&reg[8..12], &LQSPI_BASE.to_be_bytes());
        assert_eq!(&reg[12..16], &LQSPI_SIZE.to_be_bytes());
        // The register window itself is untouched.
        assert_eq!(
            &reg[..8],
            &property(&src, &["amba", QSPI_NODE], "reg")
                .unwrap()
                .unwrap()[..]
        );
        let compat = property(&p.dtb, &["amba", QSPI_NODE, FLASH_NODE], "compatible")
            .unwrap()
            .unwrap();
        assert_eq!(compat, b"spansion,s25fl129p0\0");
        // Other properties survive, the header is consistent.
        assert_eq!(
            property(&p.dtb, &["amba", QSPI_NODE], "status")
                .unwrap()
                .unwrap(),
            b"okay\0"
        );
        assert_eq!(be32(&p.dtb, 4).unwrap() as usize, p.dtb.len());
    }

    #[test]
    fn patching_is_idempotent_and_keeps_the_flash_when_not_asked() {
        let src = sample(2);
        let once = patch_qspi(&src, None).unwrap();
        assert!(once.window_added && !once.flash_changed);
        let twice = patch_qspi(&once.dtb, None).unwrap();
        assert!(!twice.window_added);
        assert_eq!(twice.dtb, once.dtb);
        let compat = property(&once.dtb, &["amba", QSPI_NODE, FLASH_NODE], "compatible")
            .unwrap()
            .unwrap();
        assert_eq!(compat, b"micron,n25q128a13\0");
        // An already-mapped tree is accepted as it is.
        let mapped = sample(4);
        assert!(!patch_qspi(&mapped, None).unwrap().window_added);
    }

    #[test]
    fn missing_pieces_and_garbage_are_reported() {
        assert!(matches!(
            patch_qspi(b"nope", None),
            Err(DtbError::Malformed(_))
        ));
        let mut bad = sample(2);
        bad[0] = 0;
        assert!(patch_qspi(&bad, None).is_err());
        // No flash child: asking for a rename fails.
        let mut no_flash = sample(2);
        // Rename the child so it does not match.
        let at = no_flash.windows(7).position(|w| w == b"flash@0").unwrap();
        no_flash[at] = b'x';
        let e = patch_qspi(&no_flash, Some("spansion,s25fl129p0")).unwrap_err();
        assert!(e.to_string().contains("flash@0"), "{e}");
    }

    /// The real co-simulation tree of the Xilinx install, when there is one.
    #[test]
    fn the_real_cosim_tree_is_patched() {
        let candidates = [r"C:\Xilinx\Vitis\2022.1\data\emulation\dtbs\zynq\zynq-arm-cosim.dtb"];
        let Some(path) = candidates
            .iter()
            .find(|p| std::path::Path::new(p).is_file())
        else {
            eprintln!("SKIPPED: the Xilinx co-simulation device tree is not installed");
            return;
        };
        let src = std::fs::read(path).unwrap();
        let p = patch_qspi(&src, Some("spansion,s25fl129p0")).unwrap();
        assert!(p.window_added && p.flash_changed);
        let reg = property(&p.dtb, &["amba", QSPI_NODE], "reg")
            .unwrap()
            .unwrap();
        assert_eq!(reg.len(), 16);
        assert_eq!(&reg[8..12], &LQSPI_BASE.to_be_bytes());
        // The rest of the tree is intact: every other property is unchanged.
        let sd = property(&src, &["amba", "sdhci@e0100000"], "reg").unwrap();
        assert_eq!(
            sd,
            property(&p.dtb, &["amba", "sdhci@e0100000"], "reg").unwrap()
        );
        assert!(sd.is_some());
    }

    /// The PS block edits land on the real tree, and nothing else moves.
    #[test]
    fn a_u32_property_is_set_on_the_real_global_timer_and_nothing_else_moves() {
        let path = r"C:\Xilinx\Vitis\2022.1\data\emulation\dtbs\zynq\zynq-arm-cosim.dtb";
        if !std::path::Path::new(path).is_file() {
            eprintln!("SKIPPED: the Xilinx co-simulation device tree is not installed");
            return;
        }
        let src = std::fs::read(path).unwrap();
        let rate = 333_333_333u32;
        let out = set_u32_property(&src, GLOBAL_TIMER_NODE, "clock-frequency", rate).unwrap();
        let get = |d: &[u8], unit: &str, prop: &str| property(d, &["amba", unit], prop).unwrap();
        // The new property has the value; the existing `clocks` is kept.
        assert_eq!(
            get(&out, GLOBAL_TIMER_NODE, "clock-frequency"),
            Some(rate.to_be_bytes().to_vec())
        );
        assert_eq!(
            get(&out, GLOBAL_TIMER_NODE, "clocks"),
            get(&src, GLOBAL_TIMER_NODE, "clocks")
        );
        // Other nodes keep their values, and the header is consistent.
        assert_eq!(
            get(&out, "spi@e0006000", "compatible"),
            get(&src, "spi@e0006000", "compatible")
        );
        assert_eq!(be32(&out, 4).unwrap() as usize, out.len());
        // Setting the same value again changes nothing (and needs no new name).
        let again = set_u32_property(&out, GLOBAL_TIMER_NODE, "clock-frequency", rate).unwrap();
        assert_eq!(again, out);
        // A different value replaces the property in place.
        let other = set_u32_property(&out, GLOBAL_TIMER_NODE, "clock-frequency", 1000).unwrap();
        assert_eq!(
            get(&other, GLOBAL_TIMER_NODE, "clock-frequency"),
            Some(1000u32.to_be_bytes().to_vec())
        );
        assert_eq!(other.len(), out.len());
        // A node that is not in the tree is reported.
        assert!(set_u32_property(&src, "timer@deadbeef", "clock-frequency", 1).is_err());
    }

    #[test]
    fn the_default_can_edit_is_applied_to_the_real_cosim_tree() {
        let path = r"C:\Xilinx\Vitis\2022.1\data\emulation\dtbs\zynq\zynq-arm-cosim.dtb";
        if !std::path::Path::new(path).is_file() {
            eprintln!("SKIPPED: the Xilinx co-simulation device tree is not installed");
            return;
        }
        let src = std::fs::read(path).unwrap();
        let (out, n) = patch_default_blocks(&src).unwrap();
        assert_eq!(n, DEFAULT_EDITS.len(), "the CAN0 node is in the tree");
        assert_eq!(
            property(&out, &["amba", "can@e0008000"], "compatible")
                .unwrap()
                .unwrap(),
            b"xlnx,ps7-can-1.00.a\0"
        );
        // The CAN1 node and the other nodes are as shipped.
        assert_eq!(
            property(&out, &["amba", "can@e0009000"], "compatible").unwrap(),
            property(&src, &["amba", "can@e0009000"], "compatible").unwrap()
        );
        let (again, _) = patch_default_blocks(&out).unwrap();
        assert_eq!(again, out);
    }

    #[test]
    fn the_ps_block_edits_are_applied_to_the_real_cosim_tree() {
        let path = r"C:\Xilinx\Vitis\2022.1\data\emulation\dtbs\zynq\zynq-arm-cosim.dtb";
        if !std::path::Path::new(path).is_file() {
            eprintln!("SKIPPED: the Xilinx co-simulation device tree is not installed");
            return;
        }
        let src = std::fs::read(path).unwrap();
        let (out, n) = patch_ps_blocks(&src).unwrap();
        assert_eq!(n, PS_BLOCK_EDITS.len(), "every edit finds its node");
        let compat = |unit: &str| {
            property(&out, &["amba", unit], "compatible")
                .unwrap()
                .unwrap()
        };
        assert_eq!(compat("gpio@e000a000"), b"xlnx,zynqmp-gpio\0");
        assert_eq!(compat("spi@e0006000"), b"cdns,spi-r1p6\0");
        assert_eq!(compat("spi@e0007000"), b"cdns,spi-r1p6\0");
        assert_eq!(
            property(&out, &["amba", "spi@e0006000"], "status")
                .unwrap()
                .unwrap(),
            b"okay\0"
        );
        // An untouched node is byte-identical.
        assert_eq!(
            property(&src, &["amba", "serial@e0001000"], "compatible").unwrap(),
            property(&out, &["amba", "serial@e0001000"], "compatible").unwrap()
        );
        assert_eq!(be32(&out, 4).unwrap() as usize, out.len());
        // Applying the edits to an already patched tree changes nothing.
        let (again, _) = patch_ps_blocks(&out).unwrap();
        assert_eq!(again, out);
    }
}
