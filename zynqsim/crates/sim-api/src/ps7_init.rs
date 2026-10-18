//! Replay of `ps7_init` operations and clock tree management.
//!
//! Applies the `mwr`, `mask_write`, and `mask_poll` sequences from `ps7_init.tcl`
//! before the guest executes its first instruction.

use crate::error::ApiError;
use crate::project::Project;
use mem_sys::{Bus, SystemBus};
use std::path::Path;
use xsa_import::ps7_init::{ClockTree, Ps7InitOp, derive_clock_tree, parse_ps7_init_tcl};

/// Maximum poll attempts before timing out on `mask_poll`.
pub const MAX_POLL_ITERATIONS: usize = 2000;

/// Resolves and loads `ps7_init` operations and derived clock tree for `project`.
///
/// Returns `Ok(Some((ops, clock_tree)))` if enabled and available,
/// `Ok(None)` if disabled or not present, or `Err` if explicitly requested but missing.
pub fn load_project_ps7_init(
    project: &Project,
    base: &Path,
) -> Result<Option<(Vec<Ps7InitOp>, Option<ClockTree>)>, ApiError> {
    let explicitly_disabled = project.boot.as_ref().and_then(|b| b.ps7_init) == Some(false);
    if explicitly_disabled {
        return Ok(None);
    }
    let explicitly_enabled = project.boot.as_ref().and_then(|b| b.ps7_init) == Some(true);

    // 1. Try reading from XSA if project specifies [xsa] source.
    if let Some(xsa_src) = project.xsa.as_ref().and_then(|x| x.source.as_ref()) {
        let xsa_path = if Path::new(xsa_src).is_absolute() {
            std::path::PathBuf::from(xsa_src)
        } else {
            base.join(xsa_src)
        };
        if xsa_path.is_file() {
            let handoff =
                xsa_import::read(&xsa_path).map_err(|e| ApiError::invalid_argument(e.0))?;
            if !handoff.archive.ps7_init_ops.is_empty() {
                return Ok(Some((
                    handoff.archive.ps7_init_ops,
                    handoff.archive.clock_tree,
                )));
            }
        }
    }

    // 2. Try reading from ps7_init.tcl directly in `base`.
    let tcl_path = base.join("ps7_init.tcl");
    if tcl_path.is_file() {
        let text = std::fs::read_to_string(&tcl_path)
            .map_err(|e| ApiError::io(format!("cannot read {}: {e}", tcl_path.display())))?;
        let ops = parse_ps7_init_tcl(&text).map_err(|e| ApiError::invalid_argument(e.0))?;
        let clock_tree = derive_clock_tree(&ops);
        return Ok(Some((ops, Some(clock_tree))));
    }

    if explicitly_enabled {
        return Err(ApiError::not_found(
            "ps7_init replay was explicitly requested in [boot], but neither ps7_init.tcl nor an XSA with ps7_init was found",
        ));
    }

    Ok(None)
}

/// Replays `ps7_init` operations onto Native PS `SystemBus`.
pub fn replay_native(bus: &mut SystemBus, ops: &[Ps7InitOp]) -> Result<(), ApiError> {
    for op in ops {
        match *op {
            Ps7InitOp::Write { addr, value } => {
                bus.write(addr as u64, 4, value as u64).map_err(|e| {
                    ApiError::internal(format!("ps7_init native write to {addr:#010x} failed: {e:?}"))
                })?;
            }
            Ps7InitOp::MaskWrite { addr, mask, value } => {
                let cur = bus.read(addr as u64, 4).map_err(|e| {
                    ApiError::internal(format!(
                        "ps7_init native read before mask_write on {addr:#010x} failed: {e:?}"
                    ))
                })? as u32;
                let new_val = (cur & !mask) | (value & mask);
                bus.write(addr as u64, 4, new_val as u64).map_err(|e| {
                    ApiError::internal(format!(
                        "ps7_init native mask_write to {addr:#010x} failed: {e:?}"
                    ))
                })?;
            }
            Ps7InitOp::MaskPoll { addr, mask } => {
                let mut satisfied = false;
                for _ in 0..MAX_POLL_ITERATIONS {
                    let cur = bus.read(addr as u64, 4).map_err(|e| {
                        ApiError::internal(format!(
                            "ps7_init native mask_poll read from {addr:#010x} failed: {e:?}"
                        ))
                    })? as u32;
                    if (cur & mask) != 0 {
                        satisfied = true;
                        break;
                    }
                }
                if !satisfied {
                    return Err(ApiError::internal(format!(
                        "ps7_init mask_poll timeout on register {addr:#010x} with mask {mask:#010x}"
                    )));
                }
            }
            Ps7InitOp::MaskDelay { .. } => {}
        }
    }
    Ok(())
}

/// Returns whether QEMU's Remote-Port HP0 memory path does not decode `addr`.
///
/// In the Zynq-7000, HP ports primarily route to DDR and OCM. Over QEMU's Remote-Port HP0
/// bridge, accesses to unmapped or non-decoded blocks return DECERR:
/// - DDR controller registers (`0xF8006000..=0xF8006FFF`)
/// - Device configuration registers (`0xF8007000..=0xF8007FFF`)
/// - PS GPIO registers (`0xE000A000..=0xE000AFFF`)
/// - CoreSight debug components (`0xF8800000..=0xF88FFFFF`)
///
/// As documented in ADR-038, these registers are skipped during QEMU replay.
pub fn qemu_skips_addr(addr: u32) -> bool {
    (0xF800_6000..=0xF800_6FFF).contains(&addr)
        || (0xF800_7000..=0xF800_7FFF).contains(&addr)
        || (0xE000_A000..=0xE000_AFFF).contains(&addr)
        || (0xF880_0000..=0xF88F_FFFF).contains(&addr)
}

/// Replays `ps7_init` operations on `QemuMachine` via Remote-Port memory access.
///
/// Unmapped registers in QEMU's HP0 path are skipped per [`qemu_skips_addr`].
/// DDRIOB DCI calibration status (`0xF8000B74`) is not modeled in QEMU and is skipped.
pub fn replay_qemu(
    machine: &mut backend_qemu::QemuMachine,
    ops: &[Ps7InitOp],
) -> Result<(), ApiError> {
    for op in ops {
        match *op {
            Ps7InitOp::Write { addr, value } => {
                if qemu_skips_addr(addr) {
                    continue;
                }
                machine
                    .write_memory(addr as u64, &value.to_le_bytes())
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "ps7_init QEMU write to {addr:#010x} failed: {e}"
                        ))
                    })?;
            }
            Ps7InitOp::MaskWrite { addr, mask, value } => {
                if qemu_skips_addr(addr) {
                    continue;
                }
                let cur_bytes = machine.read_memory(addr as u64, 4).map_err(|e| {
                    ApiError::internal(format!(
                        "ps7_init QEMU read before mask_write on {addr:#010x} failed: {e}"
                    ))
                })?;
                let cur = u32::from_le_bytes(cur_bytes.try_into().unwrap_or([0; 4]));
                let new_val = (cur & !mask) | (value & mask);
                machine
                    .write_memory(addr as u64, &new_val.to_le_bytes())
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "ps7_init QEMU mask_write to {addr:#010x} failed: {e}"
                        ))
                    })?;
            }
            Ps7InitOp::MaskPoll { addr, mask } => {
                if qemu_skips_addr(addr) || addr == 0xF800_0B74 {
                    continue;
                }
                let mut satisfied = false;
                for _ in 0..MAX_POLL_ITERATIONS {
                    let cur_bytes = machine.read_memory(addr as u64, 4).map_err(|e| {
                        ApiError::internal(format!(
                            "ps7_init QEMU mask_poll read from {addr:#010x} failed: {e}"
                        ))
                    })?;
                    let cur = u32::from_le_bytes(cur_bytes.try_into().unwrap_or([0; 4]));
                    if (cur & mask) != 0 {
                        satisfied = true;
                        break;
                    }
                }
                if !satisfied {
                    return Err(ApiError::internal(format!(
                        "ps7_init mask_poll timeout on register {addr:#010x} with mask {mask:#010x}"
                    )));
                }
            }
            Ps7InitOp::MaskDelay { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps_periph::system::{PsConfig, zynq_system};

    #[test]
    fn replay_native_succeeds_for_slcr_and_ddrc() {
        let (mut bus, _) = zynq_system(&PsConfig::default()).unwrap();
        let ops = vec![
            Ps7InitOp::Write {
                addr: 0xF800_0008,
                value: 0x0000_DF0D,
            }, // unlock SLCR
            Ps7InitOp::MaskWrite {
                addr: 0xF800_0120,
                mask: 0x1F00_3F30,
                value: 0x1F00_0200,
            },
            Ps7InitOp::MaskPoll {
                addr: 0xF800_010C,
                mask: 0x0000_0001,
            }, // PLL status
            Ps7InitOp::MaskPoll {
                addr: 0xF800_0B74,
                mask: 0x0000_2000,
            }, // DCI status
            Ps7InitOp::MaskPoll {
                addr: 0xF800_6054,
                mask: 0x0000_0007,
            }, // DDRC status
        ];
        replay_native(&mut bus, &ops).expect("replay native");
        let val = bus.read(0xF800_0120, 4).unwrap() as u32;
        assert_eq!(val & 0x1F00_3F30, 0x1F00_0200);
    }

    #[test]
    fn replay_native_fails_when_mask_poll_never_completes() {
        let (mut bus, _) = zynq_system(&PsConfig::default()).unwrap();
        let ops = vec![Ps7InitOp::MaskPoll {
            addr: 0xF800_000C, // LOCKSTA
            mask: 0x0000_0002, // bit 1 is never set
        }];
        let err = replay_native(&mut bus, &ops).unwrap_err();
        assert!(
            err.message.contains("0xf800000c") || err.message.contains("0xF800000C"),
            "expected register address in error, got: {}",
            err.message
        );
        assert!(
            err.message.contains("mask_poll timeout"),
            "expected timeout message, got: {}",
            err.message
        );
    }
}

