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

/// Initialisation sequence operations and optional derived clock tree.
pub type Ps7InitPayload = (Vec<Ps7InitOp>, Option<ClockTree>);

/// Resolves and loads `ps7_init` operations and derived clock tree for `project`.
///
/// Returns `Ok(Some((ops, clock_tree)))` if enabled and available,
/// `Ok(None)` if disabled or not present, or `Err` if explicitly requested but missing.
pub fn load_project_ps7_init(
    project: &Project,
    base: &Path,
) -> Result<Option<Ps7InitPayload>, ApiError> {
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
///
/// Unmapped peripherals in Native PS (such as QSPI, Ethernet, or USB registers that are not
/// modeled) are ignored.
pub fn replay_native(bus: &mut SystemBus, ops: &[Ps7InitOp]) -> Result<(), ApiError> {
    for op in ops {
        match *op {
            Ps7InitOp::Write { addr, value } => match bus.write(addr as u64, 4, value as u64) {
                Ok(()) => {}
                Err(mem_sys::BusFault::Unmapped(_)) => {}
                Err(e) => {
                    return Err(ApiError::internal(format!(
                        "ps7_init native write to {addr:#010x} failed: {e:?}"
                    )));
                }
            },
            Ps7InitOp::MaskWrite { addr, mask, value } => {
                let cur = match bus.read(addr as u64, 4) {
                    Ok(v) => v as u32,
                    Err(mem_sys::BusFault::Unmapped(_)) => continue,
                    Err(e) => {
                        return Err(ApiError::internal(format!(
                            "ps7_init native read before mask_write on {addr:#010x} failed: {e:?}"
                        )));
                    }
                };
                let new_val = (cur & !mask) | (value & mask);
                match bus.write(addr as u64, 4, new_val as u64) {
                    Ok(()) => {}
                    Err(mem_sys::BusFault::Unmapped(_)) => {}
                    Err(e) => {
                        return Err(ApiError::internal(format!(
                            "ps7_init native mask_write to {addr:#010x} failed: {e:?}"
                        )));
                    }
                }
            }
            Ps7InitOp::MaskPoll { addr, mask } => {
                let mut satisfied = false;
                for _ in 0..MAX_POLL_ITERATIONS {
                    let cur = match bus.read(addr as u64, 4) {
                        Ok(v) => v as u32,
                        Err(mem_sys::BusFault::Unmapped(_)) => {
                            satisfied = true;
                            break;
                        }
                        Err(e) => {
                            return Err(ApiError::internal(format!(
                                "ps7_init native mask_poll read from {addr:#010x} failed: {e:?}"
                            )));
                        }
                    };
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

/// An entry in the explicit QEMU skip allow-list for unmapped or non-decoded registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkippedRegisterRule {
    /// Start address (inclusive).
    pub start: u32,
    /// End address (inclusive).
    pub end: u32,
    /// Hardware block / peripheral name.
    pub block: &'static str,
    /// Rationale explaining why QEMU does not decode or model this range over Remote-Port HP0.
    pub reason: &'static str,
}

/// Explicit allow-list of register ranges skipped during QEMU ps7_init replay.
///
/// Over QEMU's Remote-Port HP0 memory path, accesses to unmapped blocks return DECERR.
/// Registers in this list are deliberately not decoded over HP0 or lack models in QEMU.
/// Any attempted access to an address outside SLCR (0xF800_0000..=0xF800_0FFF), RAM, or OCM
/// that is not in this explicit allow-list will fail with an informative error rather than being
/// silently ignored.
pub const QEMU_SKIPPED_REGISTER_ALLOW_LIST: &[SkippedRegisterRule] = &[
    SkippedRegisterRule {
        start: 0xF800_6000,
        end: 0xF800_6FFF,
        block: "DDRC",
        reason: "DDR controller registers not decoded over HP0 and not modelled in QEMU (DRAM is mapped as plain RAM)",
    },
    SkippedRegisterRule {
        start: 0xF800_7000,
        end: 0xF800_7FFF,
        block: "DEVCFG",
        reason: "Device configuration registers not decoded over HP0 and unmapped in QEMU FDT machine (ADR-029)",
    },
    SkippedRegisterRule {
        start: 0xE000_0000,
        end: 0xE000_1FFF,
        block: "UART",
        reason: "PS UART registers not decoded over HP0 bridge (guest initializes UART after boot)",
    },
    SkippedRegisterRule {
        start: 0xE000_2000,
        end: 0xE000_3FFF,
        block: "USB",
        reason: "PS USB controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE000_4000,
        end: 0xE000_5FFF,
        block: "I2C",
        reason: "PS I2C controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE000_6000,
        end: 0xE000_7FFF,
        block: "SPI",
        reason: "PS SPI controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE000_8000,
        end: 0xE000_9FFF,
        block: "CAN",
        reason: "PS CAN controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE000_A000,
        end: 0xE000_AFFF,
        block: "GPIO",
        reason: "PS GPIO registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE000_B000,
        end: 0xE000_CFFF,
        block: "GEM",
        reason: "PS Gigabit Ethernet controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE000_D000,
        end: 0xE000_DFFF,
        block: "QSPI",
        reason: "PS QSPI controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE000_E000,
        end: 0xE000_EFFF,
        block: "SMC",
        reason: "Static Memory Controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xE010_0000,
        end: 0xE010_1FFF,
        block: "SDIO",
        reason: "PS SDIO controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xF800_1000,
        end: 0xF800_2FFF,
        block: "TTC",
        reason: "PS Triple Timer Counter registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xF800_3000,
        end: 0xF800_3FFF,
        block: "DMAC",
        reason: "PS DMA controller registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xF800_5000,
        end: 0xF800_5FFF,
        block: "SWDT",
        reason: "PS System Watchdog Timer registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xF800_8000,
        end: 0xF800_BFFF,
        block: "AFI",
        reason: "AXI HP interface and configuration registers not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xF880_0000,
        end: 0xF88F_FFFF,
        block: "CoreSight",
        reason: "CoreSight debug and trace components not decoded over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xF8F0_0000,
        end: 0xF8F0_1FFF,
        block: "MPCore",
        reason: "ARM Cortex-A9 MPCore private peripheral registers (SCU, GIC, Timers) internal to CPU and not accessible over HP0 bridge",
    },
    SkippedRegisterRule {
        start: 0xF800_0B74,
        end: 0xF800_0B74,
        block: "DDRIOB_DCI",
        reason: "DDRIOB DCI calibration status register not modelled in QEMU",
    },
];

/// Returns the allow-list rule if `addr` is permitted to be skipped on QEMU, or `None`.
pub fn qemu_skipped_rule(addr: u32) -> Option<&'static SkippedRegisterRule> {
    QEMU_SKIPPED_REGISTER_ALLOW_LIST
        .iter()
        .find(|rule| (rule.start..=rule.end).contains(&addr))
}

/// Backwards-compatible check whether QEMU skips `addr`.
pub fn qemu_skips_addr(addr: u32) -> bool {
    qemu_skipped_rule(addr).is_some()
}

/// Whether `addr` is decoded and accessible over QEMU's Remote-Port HP0 memory path.
pub fn qemu_hp0_decodes(addr: u32) -> bool {
    (0xF800_0000..=0xF800_0FFF).contains(&addr)
        || (0x0000_0000..=0x3FFF_FFFF).contains(&addr)
        || (0xFFFC_0000..=0xFFFF_FFFF).contains(&addr)
}

/// A recorded operation that was skipped during QEMU replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedOpRecord {
    /// Zero-based index of the operation in the original sequence.
    pub op_index: usize,
    /// Physical address targeted by the operation.
    pub addr: u32,
    /// Name of the peripheral or block.
    pub block: &'static str,
    /// Documented rationale for skipping on QEMU.
    pub reason: &'static str,
}

/// Structured report of operations performed and skipped during QEMU ps7_init replay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QemuReplayReport {
    /// Number of operations applied to QEMU via Remote-Port.
    pub applied: usize,
    /// Operations skipped according to the explicit allow-list.
    pub skipped: Vec<SkippedOpRecord>,
}

/// Replays `ps7_init` operations on `QemuMachine` via Remote-Port memory access.
///
/// Registers in the explicit allow-list ([`QEMU_SKIPPED_REGISTER_ALLOW_LIST`]) are skipped
/// and recorded in the returned [`QemuReplayReport`] with their documented rationale.
/// Any operation targeting an address that is neither in the allow-list nor decoded by HP0
/// fails immediately with an informative error.
pub fn replay_qemu(
    machine: &mut backend_qemu::QemuMachine,
    ops: &[Ps7InitOp],
) -> Result<QemuReplayReport, ApiError> {
    let mut report = QemuReplayReport::default();
    for (op_index, op) in ops.iter().enumerate() {
        let addr = match *op {
            Ps7InitOp::Write { addr, .. }
            | Ps7InitOp::MaskWrite { addr, .. }
            | Ps7InitOp::MaskPoll { addr, .. } => addr,
            Ps7InitOp::MaskDelay { .. } => continue,
        };

        if let Some(rule) = qemu_skipped_rule(addr) {
            report.skipped.push(SkippedOpRecord {
                op_index,
                addr,
                block: rule.block,
                reason: rule.reason,
            });
            continue;
        }

        if !qemu_hp0_decodes(addr) {
            return Err(ApiError::invalid_argument(format!(
                "ps7_init operation #{op_index} targets register {addr:#010x} which is not decoded over QEMU Remote-Port HP0 and not in the explicit allow-list"
            )));
        }

        match *op {
            Ps7InitOp::Write { addr, value } => {
                machine
                    .write_memory(addr as u64, &value.to_le_bytes())
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "ps7_init QEMU write to {addr:#010x} failed: {e}"
                        ))
                    })?;
                report.applied += 1;
            }
            Ps7InitOp::MaskWrite { addr, mask, value } => {
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
                report.applied += 1;
            }
            Ps7InitOp::MaskPoll { addr, mask } => {
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
                report.applied += 1;
            }
            Ps7InitOp::MaskDelay { .. } => {}
        }
    }
    Ok(report)
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

    #[test]
    fn qemu_allow_list_identifies_skipped_ranges_with_reasons() {
        assert_eq!(qemu_skipped_rule(0xF800_6000).unwrap().block, "DDRC");
        assert_eq!(qemu_skipped_rule(0xF800_6054).unwrap().block, "DDRC");
        assert_eq!(qemu_skipped_rule(0xF800_7000).unwrap().block, "DEVCFG");
        assert_eq!(qemu_skipped_rule(0xE000_A000).unwrap().block, "GPIO");
        assert_eq!(qemu_skipped_rule(0xE000_2000).unwrap().block, "USB");
        assert_eq!(qemu_skipped_rule(0xE000_B000).unwrap().block, "GEM");
        assert_eq!(qemu_skipped_rule(0xE000_E000).unwrap().block, "SMC");
        assert_eq!(qemu_skipped_rule(0xF880_0000).unwrap().block, "CoreSight");
        assert_eq!(qemu_skipped_rule(0xF800_0B74).unwrap().block, "DDRIOB_DCI");

        assert_eq!(qemu_skipped_rule(0xE000_1034).unwrap().block, "UART");
        assert_eq!(qemu_skipped_rule(0xE000_4000).unwrap().block, "I2C");
        assert_eq!(qemu_skipped_rule(0xE000_6000).unwrap().block, "SPI");
        assert_eq!(qemu_skipped_rule(0xE000_8000).unwrap().block, "CAN");
        assert_eq!(qemu_skipped_rule(0xE000_D000).unwrap().block, "QSPI");
        assert_eq!(qemu_skipped_rule(0xE010_0000).unwrap().block, "SDIO");
        assert_eq!(qemu_skipped_rule(0xF800_1000).unwrap().block, "TTC");
        assert_eq!(qemu_skipped_rule(0xF800_5000).unwrap().block, "SWDT");

        // SLCR registers are NOT in the skip allow-list; they must be written!
        assert!(qemu_skipped_rule(0xF800_0008).is_none());
        assert!(qemu_skipped_rule(0xF800_0100).is_none());
        assert!(qemu_skipped_rule(0xF800_0120).is_none());

        // HP0 decodes SLCR, RAM, OCM
        assert!(qemu_hp0_decodes(0xF800_0008));
        assert!(qemu_hp0_decodes(0x0010_0000));
        assert!(qemu_hp0_decodes(0x0000_0000));
        assert!(qemu_hp0_decodes(0xFFFF_0000));
        assert!(!qemu_hp0_decodes(0xF800_6000));
        assert!(!qemu_hp0_decodes(0xDEAD_BEEF));
    }

    #[test]
    fn fixtures_handoff_xsa_all_ops_either_decoded_or_allowed_skip() {
        let fixtures_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("fixtures");
        if !fixtures_dir.is_dir() {
            return;
        }

        let xsa_paths = [
            fixtures_dir.join("handoff/design.xsa"),
            fixtures_dir.join("bsp/zs_ps_all.xsa"),
            fixtures_dir.join("xsa/zs_dma_fifo.xsa"),
            fixtures_dir.join("xsa/zs_dma_loopback.xsa"),
            fixtures_dir.join("xsa/zs_iic_bram.xsa"),
            fixtures_dir.join("xsa/zs_intc.xsa"),
            fixtures_dir.join("xsa/zs_math_gpio.xsa"),
            fixtures_dir.join("xsa/zs_math_only.xsa"),
            fixtures_dir.join("xsa/zs_two_math.xsa"),
            fixtures_dir.join("xsa/zs_vendor_ip.xsa"),
        ];

        for path in xsa_paths {
            assert!(path.is_file(), "Path does not exist: {}", path.display());
            let handoff = match xsa_import::read(&path) {
                Ok(h) => h,
                Err(e) => panic!("Failed to read {}: {:?}", path.display(), e),
            };
            for (idx, op) in handoff.archive.ps7_init_ops.iter().enumerate() {
                let addr = match op {
                    Ps7InitOp::Write { addr, .. } => *addr,
                    Ps7InitOp::MaskWrite { addr, .. } => *addr,
                    Ps7InitOp::MaskPoll { addr, .. } => *addr,
                    Ps7InitOp::MaskDelay { .. } => continue,
                };
                assert!(
                    qemu_hp0_decodes(addr) || qemu_skips_addr(addr),
                    "In {}, op #{idx} addr {addr:#010x} is neither decoded over HP0 nor in allow-list",
                    path.display()
                );
            }
        }
    }
}
