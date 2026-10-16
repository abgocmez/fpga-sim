//! The Zynq-7000 physical address map as data.
//!
//! The tables are the source the docs and the tests print from. Addresses come from UG585
//! Table 2-1 and the chapter of each block, as remembered; an entry whose address is not
//! checked against the TRM says `VERIFY:` in its `note`.

/// What an entry of the map is on the simulated bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Memory the bus stores itself (DDR, OCM).
    Ram,
    /// A register block with a model.
    Peripheral,
    /// A named placeholder: reads zero, writes are ignored.
    Stub,
    /// A read-only memory image (the QSPI linear window).
    Rom,
    /// A PL-side AXI window. The PL is not on this bus.
    PlPort,
    /// Part of the map that has no model. Accesses are unmapped and logged.
    NotModelled,
}

impl Kind {
    /// A short label for tables.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Ram => "RAM",
            Kind::Peripheral => "peripheral",
            Kind::Stub => "stub (RAZ/WI)",
            Kind::Rom => "ROM image",
            Kind::PlPort => "PL port",
            Kind::NotModelled => "not modelled",
        }
    }
}

/// One named range of the physical address map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapEntry {
    /// Name used by the bus when a device is attached (`SystemBus::attach`) and in logs.
    pub name: &'static str,
    /// First address.
    pub base: u64,
    /// Length in bytes. For DDR this is the window; the configured size is smaller.
    pub size: u64,
    /// What the range is.
    pub kind: Kind,
    /// The model that serves it in this crate set, or "none".
    pub model: &'static str,
    /// Source and open questions.
    pub note: &'static str,
}

/// Last address + 1 of an entry.
pub const fn end(e: &MapEntry) -> u64 {
    e.base + e.size
}

/// The top-level physical map of the PS.
pub const ZYNQ_MAP: &[MapEntry] = &[
    MapEntry {
        name: "DDR",
        base: 0x0000_0000,
        size: 0x4000_0000,
        kind: Kind::Ram,
        model: "mem-sys::SparseRam (size from config, default 512 MiB)",
        note: "UG585 ch.2 (1 GB window). Zybo Z7-20 has 1 GB DDR3L; the default here is 512 MiB so tests stay small",
    },
    MapEntry {
        name: "M_AXI_GP0",
        base: 0x4000_0000,
        size: 0x4000_0000,
        kind: Kind::PlPort,
        model: "none (PL)",
        note: "UG585 Table 2-1",
    },
    MapEntry {
        name: "M_AXI_GP1",
        base: 0x8000_0000,
        size: 0x4000_0000,
        kind: Kind::PlPort,
        model: "none (PL)",
        note: "UG585 Table 2-1",
    },
    MapEntry {
        name: "IOP",
        base: 0xE000_0000,
        size: 0x0100_0000,
        kind: Kind::Peripheral,
        model: "UART0, UART1 (ps-periph); the rest unmapped, see IOP_BLOCKS",
        note: "UG585 Table 2-1: I/O peripherals 0xE000_0000-0xE0FF_FFFF",
    },
    MapEntry {
        name: "SMC_MEM",
        base: 0xE100_0000,
        size: 0x0100_0000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1 (SMC memory interfaces)",
    },
    MapEntry {
        name: "SLCR",
        base: 0xF800_0000,
        size: 0x1000,
        kind: Kind::Peripheral,
        model: "ps-periph::Slcr",
        note: "UG585 ch.4. Lock 0x004 / unlock 0x008 / lock status 0x00C",
    },
    MapEntry {
        name: "TTC0",
        base: 0xF800_1000,
        size: 0x1000,
        kind: Kind::Peripheral,
        model: "ps-periph::Ttc",
        note: "UG585 ch.12; counters 0..2 with SPI 42..44",
    },
    MapEntry {
        name: "TTC1",
        base: 0xF800_2000,
        size: 0x1000,
        kind: Kind::Peripheral,
        model: "ps-periph::Ttc",
        note: "UG585 ch.12; counters 0..2 with SPI 69..71",
    },
    MapEntry {
        name: "DMAC",
        base: 0xF800_3000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none (QEMU PL330 model in the QEMU backend)",
        note: "UG585 ch.12 (PL330 DMAC)",
    },
    MapEntry {
        name: "SWDT",
        base: 0xF800_5000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 ch.12 (system watchdog)",
    },
    MapEntry {
        name: "DDRC",
        base: 0xF800_6000,
        size: 0x1000,
        kind: Kind::Stub,
        model: "ps-periph::stub DDRC",
        note: "UG585 ch.10 (DDR controller). Reads zero; no timing or registers modelled",
    },
    MapEntry {
        name: "DEVCFG",
        base: 0xF800_7000,
        size: 0x1000,
        kind: Kind::Stub,
        model: "ps-periph::stub DEVCFG",
        note: "UG585 ch.6 (device config; XADC registers at 0xF8007100 are not modelled here). Reads zero",
    },
    MapEntry {
        name: "OCM",
        base: 0xFFFC_0000,
        size: 0x0004_0000,
        kind: Kind::Ram,
        model: "mem-sys::SystemBus (256 KiB on-chip RAM, high mapping)",
        note: "UG585 ch.4 (OCM_CFG in SLCR at 0x910). Only the reset mapping (high) is modelled; the low remap is not",
    },
    MapEntry {
        name: "MPCORE",
        base: 0xF8F0_0000,
        size: 0x3000,
        kind: Kind::Peripheral,
        model: "ps-periph::MpCore (SCU, GIC, timers, L2C stub); blocks in MPCORE_BLOCKS",
        note: "Cortex-A9 MPCore TRM; UG585 ch.7",
    },
    MapEntry {
        name: "QSPI_LINEAR",
        base: 0xFC00_0000,
        size: 0x0200_0000,
        kind: Kind::Rom,
        model: "mem-sys::ReadOnlyImage, when an image is configured; erased bytes read 0xFF",
        note: "UG585 ch.9 (QSPI linear address mode). Writes are ignored",
    },
];

/// The blocks inside the MPCore region, with absolute addresses.
pub const MPCORE_BLOCKS: &[MapEntry] = &[
    MapEntry {
        name: "SCU",
        base: 0xF8F0_0000,
        size: 0x100,
        kind: Kind::Peripheral,
        model: "ps-periph::MpCore (control, config)",
        note: "Cortex-A9 MPCore TRM ch.2",
    },
    MapEntry {
        name: "GIC_CPU_IF",
        base: 0xF8F0_0100,
        size: 0x100,
        kind: Kind::Peripheral,
        model: "ps-periph::Gic (banked per core)",
        note: "PL390 TRM; ICCICR 0x00, ICCPMR 0x04, ICCIAR 0x0C, ICCEOIR 0x10",
    },
    MapEntry {
        name: "GLOBAL_TIMER",
        base: 0xF8F0_0200,
        size: 0x100,
        kind: Kind::Peripheral,
        model: "ps-periph::GlobalTimer (PPI 27 to both cores)",
        note: "Cortex-A9 MPCore TRM ch.4; the firmware checks in firmware/conf_scutimer",
    },
    MapEntry {
        name: "PRIVATE_TIMER",
        base: 0xF8F0_0600,
        size: 0x20,
        kind: Kind::Peripheral,
        model: "ps-periph::PrivateTimer (banked, PPI 29)",
        note: "Cortex-A9 MPCore TRM ch.4; LOAD 0x00, COUNTER 0x04, CONTROL 0x08, ISR 0x0C",
    },
    MapEntry {
        name: "WATCHDOG",
        base: 0xF8F0_0620,
        size: 0x20,
        kind: Kind::Peripheral,
        model: "ps-periph::Watchdog (banked, PPI 30; no reset is performed)",
        note: "Cortex-A9 MPCore TRM ch.4; LOAD 0x20, COUNTER 0x24, CONTROL 0x28, ISR 0x2C, RESET_STATUS 0x30, DISABLE 0x34",
    },
    MapEntry {
        name: "GIC_DIST",
        base: 0xF8F0_1000,
        size: 0x1000,
        kind: Kind::Peripheral,
        model: "ps-periph::Gic (96 IDs, 2 cores)",
        note: "PL390 TRM; ICDDCR 0x000, ICDISER 0x100, ICDIPR 0x400, ICDIPTR 0x800, ICDICFR 0xC00, ICDSGIR 0xF00",
    },
    MapEntry {
        name: "L2C",
        base: 0xF8F0_2000,
        size: 0x1000,
        kind: Kind::Stub,
        model: "ps-periph::MpCore stub (PL310 L2 cache controller)",
        note: "PL310 TRM. Reads zero, writes ignored; no cache model",
    },
];

/// The I/O peripheral blocks inside the IOP window (UG585 Table 2-1).
pub const IOP_BLOCKS: &[MapEntry] = &[
    MapEntry {
        name: "UART0",
        base: 0xE000_0000,
        size: 0x1000,
        kind: Kind::Peripheral,
        model: "ps-periph::Uart (SPI 59)",
        note: "xuartps",
    },
    MapEntry {
        name: "UART1",
        base: 0xE000_1000,
        size: 0x1000,
        kind: Kind::Peripheral,
        model: "ps-periph::Uart (SPI 82)",
        note: "xuartps",
    },
    MapEntry {
        name: "USB0",
        base: 0xE000_2000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "USB1",
        base: 0xE000_3000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "I2C0",
        base: 0xE000_4000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "I2C1",
        base: 0xE000_5000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "SPI0",
        base: 0xE000_6000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "SPI1",
        base: 0xE000_7000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "CAN0",
        base: 0xE000_8000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "CAN1",
        base: 0xE000_9000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "GPIO",
        base: 0xE000_A000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "GEM0",
        base: 0xE000_B000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none (QEMU)",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "GEM1",
        base: 0xE000_C000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none (QEMU)",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "QSPI_CTRL",
        base: 0xE000_D000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "SMC_CTRL",
        base: 0xE000_E000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "SDIO0",
        base: 0xE010_0000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
    MapEntry {
        name: "SDIO1",
        base: 0xE010_1000,
        size: 0x1000,
        kind: Kind::NotModelled,
        model: "none",
        note: "VERIFY: UG585 Table 2-1",
    },
];

/// Looks an entry up by name in the three tables.
pub fn find(name: &str) -> Option<&'static MapEntry> {
    ZYNQ_MAP
        .iter()
        .chain(MPCORE_BLOCKS)
        .chain(IOP_BLOCKS)
        .find(|e| e.name == name)
}

/// Renders a table of entries as Markdown, for the docs and for `describe`.
pub fn render_markdown(entries: &[MapEntry]) -> String {
    let mut s = String::from("| Name | Base | End (incl.) | Size | Kind | Model | Note |\n");
    s.push_str("|---|---|---|---|---|---|---|\n");
    for e in entries {
        s.push_str(&format!(
            "| {} | 0x{:08X} | 0x{:08X} | {} | {} | {} | {} |\n",
            e.name,
            e.base,
            end(e) - 1,
            human_size(e.size),
            e.kind.label(),
            e.model,
            e.note
        ));
    }
    s
}

/// Formats a byte count as KiB, MiB or GiB.
fn human_size(bytes: u64) -> String {
    if bytes >= 1 << 30 && bytes % (1 << 30) == 0 {
        format!("{} GiB", bytes >> 30)
    } else if bytes >= 1 << 20 && bytes % (1 << 20) == 0 {
        format!("{} MiB", bytes >> 20)
    } else if bytes == 0 {
        "0 B".to_string()
    } else {
        format!("{} KiB", bytes >> 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_entries_do_not_overlap() {
        for (i, a) in ZYNQ_MAP.iter().enumerate() {
            for b in &ZYNQ_MAP[i + 1..] {
                let overlap = a.base < end(b) && b.base < end(a);
                assert!(!overlap, "{} overlaps {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn mpcore_blocks_lie_inside_the_mpcore_region() {
        let region = find("MPCORE").unwrap();
        for b in MPCORE_BLOCKS {
            assert!(b.base >= region.base && end(b) <= end(region), "{}", b.name);
        }
    }

    #[test]
    fn named_addresses_match_the_firmware_constants() {
        // firmware/rt and firmware/conf_* use these literals.
        assert_eq!(find("UART1").unwrap().base, 0xE000_1000);
        assert_eq!(find("SLCR").unwrap().base, 0xF800_0000);
        assert_eq!(find("TTC1").unwrap().base, 0xF800_2000);
        assert_eq!(find("PRIVATE_TIMER").unwrap().base, 0xF8F0_0600);
        assert_eq!(find("GLOBAL_TIMER").unwrap().base, 0xF8F0_0200);
        assert_eq!(find("GIC_DIST").unwrap().base, 0xF8F0_1000);
        assert_eq!(find("GIC_CPU_IF").unwrap().base, 0xF8F0_0100);
        assert_eq!(find("L2C").unwrap().base, 0xF8F0_2000);
        assert_eq!(find("OCM").unwrap().base, 0xFFFC_0000);
    }

    #[test]
    fn markdown_table_has_one_row_per_entry() {
        let t = render_markdown(ZYNQ_MAP);
        assert_eq!(t.lines().count(), ZYNQ_MAP.len() + 2);
        assert!(t.contains("| SLCR | 0xF8000000 | 0xF8000FFF | 4 KiB |"));
    }
}
