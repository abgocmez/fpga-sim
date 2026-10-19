//! Parser for `ps7_init.tcl` and PS clock tree derivation.

use crate::XsaError;
use std::collections::HashMap;

/// An operation from `ps7_init.tcl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ps7InitOp {
    /// Direct memory write: `mwr [-force] <addr> <val>`
    Write {
        /// Physical address to write.
        addr: u32,
        /// Word value to write.
        value: u32,
    },
    /// Masked write: `mask_write <addr> <mask impulse> <val>`
    MaskWrite {
        /// Physical address to update.
        addr: u32,
        /// Mask indicating which bits to replace.
        mask: u32,
        /// Value to merge under the mask.
        value: u32,
    },
    /// Masked poll: `mask_poll <addr> <mask impulse>`
    MaskPoll {
        /// Physical address to poll.
        addr: u32,
        /// Mask that must evaluate non-zero.
        mask: u32,
    },
    /// Delay: `mask_delay <addr> <cycles>`
    MaskDelay {
        /// Address associated with the delay/timer.
        addr: u32,
        /// Number of delay cycles.
        cycles: u32,
    },
}

/// The clock tree derived from SLCR clock and PLL registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClockTree {
    /// Input reference clock frequency (typically 33.333333 MHz).
    pub ps_clk_hz: u64,
    /// ARM PLL frequency in Hz.
    pub arm_pll_hz: u64,
    /// DDR PLL frequency in Hz.
    pub ddr_pll_hz: u64,
    /// I/O PLL frequency in Hz.
    pub io_pll_hz: u64,
    /// CPU frequency (6x4x) in Hz.
    pub cpu_hz: u64,
    /// Peripheral clock frequency (CPU_3x2x) in Hz.
    pub periph_hz: u64,
    /// TTC clock frequency (CPU_1x) in Hz.
    pub ttc_hz: u64,
    /// UART reference clock frequency in Hz.
    pub uart_hz: u64,
}

fn parse_u32(s: &str) -> Result<u32, XsaError> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(rest, 16).map_err(|e| XsaError(format!("bad hex number `{s}`: {e}")))
    } else if s.chars().all(|c| c.is_ascii_digit()) {
        s.parse::<u32>()
            .map_err(|e| XsaError(format!("bad dec number `{s}`: {e}")))
    } else {
        u32::from_str_radix(s, 16).map_err(|e| XsaError(format!("bad number `{s}`: {e}")))
    }
}

/// Parses a Tcl procedure body into a list of [`Ps7InitOp`].
fn parse_lines(body: &str, ops: &mut Vec<Ps7InitOp>) -> Result<(), XsaError> {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        match parts[0] {
            "mwr" => {
                let (addr_str, val_str) = if parts.len() >= 4 && parts[1] == "-force" {
                    (parts[2], parts[3])
                } else if parts.len() >= 3 {
                    (parts[1], parts[2])
                } else {
                    return Err(XsaError(format!("mwr has too few arguments: `{line}`")));
                };
                ops.push(Ps7InitOp::Write {
                    addr: parse_u32(addr_str)?,
                    value: parse_u32(val_str)?,
                });
            }
            "mask_write" => {
                if parts.len() < 4 {
                    return Err(XsaError(format!(
                        "mask_write has too few arguments: `{line}`"
                    )));
                }
                ops.push(Ps7InitOp::MaskWrite {
                    addr: parse_u32(parts[1])?,
                    mask: parse_u32(parts[2])?,
                    value: parse_u32(parts[3])?,
                });
            }
            "mask_poll" => {
                if parts.len() < 3 {
                    return Err(XsaError(format!(
                        "mask_poll has too few arguments: `{line}`"
                    )));
                }
                ops.push(Ps7InitOp::MaskPoll {
                    addr: parse_u32(parts[1])?,
                    mask: parse_u32(parts[2])?,
                });
            }
            "mask_delay" => {
                if parts.len() < 3 {
                    return Err(XsaError(format!(
                        "mask_delay has too few arguments: `{line}`"
                    )));
                }
                ops.push(Ps7InitOp::MaskDelay {
                    addr: parse_u32(parts[1])?,
                    cycles: parse_u32(parts[2])?,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Extracts Tcl procedures from `text`.
fn extract_procs(text: &str) -> HashMap<String, String> {
    let mut procs = HashMap::new();
    let mut rest = text;
    while let Some(proc_idx) = rest.find("proc ") {
        let after_proc = rest[proc_idx + 5..].trim_start();
        let name_end = after_proc
            .find(|c: char| c.is_whitespace() || c == '{')
            .unwrap_or(after_proc.len());
        let name = after_proc[..name_end].trim().to_string();
        let mut rem = after_proc[name_end..].trim_start();
        // Skip args: if it starts with '{', skip matching brace; else skip word
        if rem.starts_with('{') {
            let mut depth = 0;
            let mut arg_end = 0;
            for (i, b) in rem.bytes().enumerate() {
                if b == b'{' {
                    depth += 1;
                } else if b == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        arg_end = i + 1;
                        break;
                    }
                }
            }
            rem = rem[arg_end..].trim_start();
        } else {
            let word_end = rem
                .find(|c: char| c.is_whitespace() || c == '{')
                .unwrap_or(rem.len());
            rem = rem[word_end..].trim_start();
        }
        // Now find body brace '{'
        if rem.starts_with('{') {
            let mut depth = 0;
            let mut body_end = 0;
            for (i, b) in rem.bytes().enumerate() {
                if b == b'{' {
                    depth += 1;
                } else if b == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        body_end = i;
                        break;
                    }
                }
            }
            if depth == 0 && body_end > 0 {
                let body = &rem[1..body_end];
                procs.insert(name, body.to_string());
                rest = &rem[body_end + 1..];
                continue;
            }
        }
        rest = after_proc;
    }
    procs
}

/// Parses the full `ps7_init.tcl` text into an ordered sequence of initialization operations.
pub fn parse_ps7_init_tcl(text: &str) -> Result<Vec<Ps7InitOp>, XsaError> {
    let procs = extract_procs(text);
    let mut ops = Vec::new();

    let candidate_sets = [
        // 3.0 silicon (standard Zynq-7000 production)
        [
            "ps7_mio_init_data_3_0",
            "ps7_pll_init_data_3_0",
            "ps7_clock_init_data_3_0",
            "ps7_ddr_init_data_3_0",
            "ps7_peripherals_init_data_3_0",
            "ps7_post_config_3_0",
        ],
        // 2.0 silicon
        [
            "ps7_mio_init_data_2_0",
            "ps7_pll_init_data_2_0",
            "ps7_clock_init_data_2_0",
            "ps7_ddr_init_data_2_0",
            "ps7_peripherals_init_data_2_0",
            "ps7_post_config_2_0",
        ],
        // 1.0 silicon
        [
            "ps7_mio_init_data_1_0",
            "ps7_pll_init_data_1_0",
            "ps7_clock_init_data_1_0",
            "ps7_ddr_init_data_1_0",
            "ps7_peripherals_init_data_1_0",
            "ps7_post_config_1_0",
        ],
        // Unversioned
        [
            "ps7_mio_init_data",
            "ps7_pll_init_data",
            "ps7_clock_init_data",
            "ps7_ddr_init_data",
            "ps7_peripherals_init_data",
            "ps7_post_config",
        ],
    ];

    let mut found_proc_set = false;
    for candidates in candidate_sets {
        if candidates.iter().any(|name| procs.contains_key(*name)) {
            for name in candidates {
                if let Some(body) = procs.get(name) {
                    parse_lines(body, &mut ops)?;
                }
            }
            found_proc_set = true;
            break;
        }
    }

    if !found_proc_set {
        // Flat Tcl script without procedures.
        parse_lines(text, &mut ops)?;
    }

    Ok(ops)
}

/// Derives the PS clock tree from the SLCR register writes in `ops`.
pub fn derive_clock_tree(ops: &[Ps7InitOp]) -> ClockTree {
    let mut slcr: HashMap<u32, u32> = HashMap::new();
    // Default reset values for relevant SLCR registers.
    slcr.insert(0xF800_0100, 0x0002_8000); // ARM_PLL_CTRL (FDIV = 40)
    slcr.insert(0xF800_0104, 0x0002_0000); // DDR_PLL_CTRL (FDIV = 32)
    slcr.insert(0xF800_0108, 0x0001_E000); // IO_PLL_CTRL (FDIV = 30)
    slcr.insert(0xF800_0120, 0x1F00_0200); // ARM_CLK_CTRL (DIV = 2)
    slcr.insert(0xF800_0154, 0x0000_0A02); // UART_CLK_CTRL (DIV = 10)

    for op in ops {
        match *op {
            Ps7InitOp::Write { addr, value } => {
                if (0xF800_0100..=0xF800_01F0).contains(&addr) {
                    slcr.insert(addr, value);
                }
            }
            Ps7InitOp::MaskWrite { addr, mask, value } => {
                if (0xF800_0100..=0xF800_01F0).contains(&addr) {
                    let cur = slcr.get(&addr).copied().unwrap_or(0);
                    slcr.insert(addr, (cur & !mask) | (value & mask));
                }
            }
            Ps7InitOp::MaskPoll { .. } | Ps7InitOp::MaskDelay { .. } => {}
        }
    }

    let ps_clk_hz = 33_333_333u64;

    let arm_fdiv = ((slcr.get(&0xF800_0100).copied().unwrap_or(0x28000) >> 12) & 0x7F) as u64;
    let arm_pll_hz = ps_clk_hz * if arm_fdiv == 0 { 40 } else { arm_fdiv };

    let ddr_fdiv = ((slcr.get(&0xF800_0104).copied().unwrap_or(0x20000) >> 12) & 0x7F) as u64;
    let ddr_pll_hz = ps_clk_hz * if ddr_fdiv == 0 { 32 } else { ddr_fdiv };

    let io_fdiv = ((slcr.get(&0xF800_0108).copied().unwrap_or(0x1E000) >> 12) & 0x7F) as u64;
    let io_pll_hz = ps_clk_hz * if io_fdiv == 0 { 30 } else { io_fdiv };

    let arm_clk = slcr.get(&0xF800_0120).copied().unwrap_or(0x1F00_0200);
    let cpu_srcsel = (arm_clk >> 4) & 0x3;
    let cpu_pll = match cpu_srcsel {
        0 | 1 => arm_pll_hz,
        2 => ddr_pll_hz,
        _ => io_pll_hz,
    };
    let cpu_div = (((arm_clk >> 8) & 0x3F) as u64).max(1);
    let cpu_hz = cpu_pll / cpu_div;

    let periph_hz = cpu_hz / 2;
    let ttc_hz = cpu_hz / 6;

    let uart_clk = slcr
        .get(&0xF800_0154)
        .copied()
        .or_else(|| slcr.get(&0xF800_0140).copied())
        .unwrap_or(0x0A02);
    let uart_srcsel = (uart_clk >> 4) & 0x3;
    let uart_pll = match uart_srcsel {
        0 | 1 => io_pll_hz,
        2 => arm_pll_hz,
        _ => ddr_pll_hz,
    };
    let uart_div = (((uart_clk >> 8) & 0x3F) as u64).max(1);
    let uart_hz = uart_pll / uart_div;

    ClockTree {
        ps_clk_hz,
        arm_pll_hz,
        ddr_pll_hz,
        io_pll_hz,
        cpu_hz,
        periph_hz,
        ttc_hz,
        uart_hz,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcl_commands_and_comments() {
        let tcl = r#"
            # Header comment
            mwr -force 0xF8000008 0x0000DF0D
            mwr 0xF8000004 0x12345678
            mask_write 0xF8000100 0x0007F000 0x00028000
            mask_poll 0xF800010C 0x00000001
            mask_delay 0xF8000000 100
        "#;
        let ops = parse_ps7_init_tcl(tcl).expect("parse flat tcl");
        assert_eq!(
            ops,
            vec![
                Ps7InitOp::Write {
                    addr: 0xF800_0008,
                    value: 0x0000_DF0D,
                },
                Ps7InitOp::Write {
                    addr: 0xF800_0004,
                    value: 0x1234_5678,
                },
                Ps7InitOp::MaskWrite {
                    addr: 0xF800_0100,
                    mask: 0x0007_F000,
                    value: 0x0002_8000,
                },
                Ps7InitOp::MaskPoll {
                    addr: 0xF800_010C,
                    mask: 0x0000_0001,
                },
                Ps7InitOp::MaskDelay {
                    addr: 0xF800_0000,
                    cycles: 100,
                },
            ]
        );
    }

    #[test]
    fn parses_proc_sets_in_order() {
        let tcl = r#"
            proc ps7_clock_init_data_3_0 {} {
                mwr 0xF8000120 0x1F000200
            }
            proc ps7_pll_init_data_3_0 {} {
                mwr 0xF8000100 0x00028000
            }
        "#;
        let ops = parse_ps7_init_tcl(tcl).expect("parse procs");
        assert_eq!(ops.len(), 2);
        // ps7_pll_init_data_3_0 must come before ps7_clock_init_data_3_0
        assert_eq!(
            ops[0],
            Ps7InitOp::Write {
                addr: 0xF800_0100,
                value: 0x0002_8000
            }
        );
        assert_eq!(
            ops[1],
            Ps7InitOp::Write {
                addr: 0xF800_0120,
                value: 0x1F00_0200
            }
        );
    }

    #[test]
    fn derives_expected_clock_tree() {
        let ops = vec![
            Ps7InitOp::Write {
                addr: 0xF800_0100,
                value: 0x0002_8000,
            }, // ARM PLL FDIV = 40 -> 1333.33 MHz
            Ps7InitOp::Write {
                addr: 0xF800_0108,
                value: 0x0001_E000,
            }, // IO PLL FDIV = 30 -> 1000 MHz
            Ps7InitOp::Write {
                addr: 0xF800_0120,
                value: 0x1F00_0200,
            }, // ARM_CLK_CTRL: div=2, src=ARM PLL -> 666.66 MHz
            Ps7InitOp::Write {
                addr: 0xF800_0154,
                value: 0x0000_0A00,
            }, // UART_CLK_CTRL: div=10, src=IO PLL -> 100 MHz
        ];
        let ct = derive_clock_tree(&ops);
        assert_eq!(ct.ps_clk_hz, 33_333_333);
        assert_eq!(ct.arm_pll_hz, 1_333_333_320);
        assert_eq!(ct.io_pll_hz, 999_999_990);
        assert_eq!(ct.cpu_hz, 666_666_660);
        assert_eq!(ct.periph_hz, 333_333_330);
        assert_eq!(ct.ttc_hz, 111_111_110);
        assert_eq!(ct.uart_hz, 99_999_999);
    }
}
