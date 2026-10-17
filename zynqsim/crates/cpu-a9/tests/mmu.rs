//! The ARMv7-A short-descriptor MMU: every descriptor type, the permission matrix, domains,
//! fault status codes, the TLB, CP15 maintenance and address translation, abort entry and
//! return through the core, and a property test against a reference walk written in this file.
//!
//! Descriptors are built here from the ARM ARM bit layouts (B3.5), not with the crate's own
//! helpers, so a wrong bit in `src/mmu.rs` shows up as a wrong number here.

use cpu_a9::mmu::{self, Mmu};
use cpu_a9::{Access, Cpu, Event, Mode, Regs, StepResult, TlbOp, Translate};
use ps_bus::{Bus, RamBus};

/// Physical RAM: 32 MiB at address 0 (vectors, code, data and the tables live here).
const RAM: usize = 0x0200_0000;
/// The TTBR0 tables: four 16 KiB tables in 0x0100_0000..0x0101_0000.
const TT0: u32 = 0x0100_0000;
/// The TTBR1 table (16 KiB).
const TT1: u32 = 0x0101_0000;
/// Second-level (coarse) tables, 1 KiB each, 256 of them.
const L2: u32 = 0x0110_0000;
/// SCTLR.M and SCTLR.XP.
const M: u32 = 1;
const XP: u32 = 1 << 23;
/// The code of the core-level programs runs from here (section 0 is identity mapped).
const CODE: u32 = 0x1000;

fn ram() -> RamBus {
    RamBus::new(0, RAM)
}

fn poke(bus: &mut RamBus, addr: u32, v: u32) {
    bus.write32(u64::from(addr), v).unwrap();
}

/// The attributes of a descriptor, with the AP field as AP[2:0] (APX is bit 2).
#[derive(Clone, Copy, Debug, Default)]
struct Attr {
    ap: u32,
    dom: u32,
    xn: bool,
    pxn: bool,
    tex: u32,
    c: bool,
    b: bool,
    ng: bool,
    s: bool,
    ns: bool,
}

/// The bits a section or supersection shares: AP[1:0] at 11:10, APX at 15, TEX at 14:12, XN
/// at 4, C at 3, B at 2, nG at 17, S at 16, NS at 19, PXN at 0.
fn section_attr_bits(a: Attr) -> u32 {
    ((a.ap & 3) << 10)
        | (((a.ap >> 2) & 1) << 15)
        | (a.tex << 12)
        | ((a.xn as u32) << 4)
        | ((a.c as u32) << 3)
        | ((a.b as u32) << 2)
        | ((a.ng as u32) << 17)
        | ((a.s as u32) << 16)
        | ((a.ns as u32) << 19)
        | (a.pxn as u32)
}

/// A first-level section (bits 1:0 = 10), domain at 8:5.
fn section(pa: u32, a: Attr) -> u32 {
    (pa & 0xFFF0_0000) | (a.dom << 5) | section_attr_bits(a) | 0b10
}

/// A first-level supersection (bit 18): PA[31:24], PA[35:32] at 23:20, PA[39:36] at 8:5.
fn supersection(pa: u64, a: Attr) -> u32 {
    let hi = (((pa >> 32) & 0xF) as u32) << 20;
    let top = (((pa >> 36) & 0xF) as u32) << 5;
    ((pa as u32) & 0xFF00_0000) | hi | top | (1 << 18) | section_attr_bits(a) | 0b10
}

/// A first-level coarse table descriptor (bits 1:0 = 01): base at 31:10, domain 8:5, NS 3,
/// PXN 2.
fn coarse(l2: u32, dom: u32, ns: bool, pxn: bool) -> u32 {
    (l2 & 0xFFFF_FC00) | (dom << 5) | ((ns as u32) << 3) | ((pxn as u32) << 2) | 0b01
}

/// A large page descriptor (bits 1:0 = 01): base 31:16, XN 15, TEX 14:12, nG 11, S 10, APX 9,
/// AP[1:0] 5:4, C 3, B 2.
fn large(pa: u32, a: Attr) -> u32 {
    (pa & 0xFFFF_0000)
        | ((a.xn as u32) << 15)
        | (a.tex << 12)
        | ((a.ng as u32) << 11)
        | ((a.s as u32) << 10)
        | (((a.ap >> 2) & 1) << 9)
        | ((a.ap & 3) << 4)
        | ((a.c as u32) << 3)
        | ((a.b as u32) << 2)
        | 0b01
}

/// A small page descriptor (bits 1:0 = 1x, XN in bit 0): base 31:12, nG 11, S 10, APX 9,
/// TEX 8:6, AP[1:0] 5:4, C 3, B 2.
fn small(pa: u32, a: Attr) -> u32 {
    (pa & 0xFFFF_F000)
        | ((a.ng as u32) << 11)
        | ((a.s as u32) << 10)
        | (((a.ap >> 2) & 1) << 9)
        | (a.tex << 6)
        | ((a.ap & 3) << 4)
        | ((a.c as u32) << 3)
        | ((a.b as u32) << 2)
        | 0b10
        | (a.xn as u32)
}

/// A small page with XP = 0 subpage permissions: four 2-bit AP fields at 5:4, 7:6, 9:8, 11:10.
fn small_subpages(pa: u32, sub: [u32; 4], xn: bool) -> u32 {
    (pa & 0xFFFF_F000)
        | (sub[0] << 4)
        | (sub[1] << 6)
        | (sub[2] << 8)
        | (sub[3] << 10)
        | 0b10
        | (xn as u32)
}

/// The first-level entry for `va` in the table at `tt`.
fn l1_addr(tt: u32, va: u32) -> u32 {
    tt + ((va >> 20) << 2)
}

/// Maps the megabyte at `va` to a section at `pa`.
fn map_section(bus: &mut RamBus, va: u32, pa: u32, a: Attr) {
    poke(bus, l1_addr(TT0, va), section(pa, a));
}

/// Points the megabyte of `va` at the coarse table of its index, and returns that table.
fn coarse_for(bus: &mut RamBus, va: u32, dom: u32) -> u32 {
    let l2 = L2 + ((va >> 20) & 0xFF) * 0x400;
    poke(bus, l1_addr(TT0, va), coarse(l2, dom, false, false));
    l2
}

/// Maps the 4 KiB page at `va` to `pa` with a small page.
fn map_small(bus: &mut RamBus, va: u32, pa: u32, a: Attr) {
    let l2 = coarse_for(bus, va, a.dom);
    poke(bus, l2 + (((va >> 12) & 0xFF) << 2), small(pa, a));
}

/// Maps the 64 KiB block at `va` (16-aligned) to `pa` with 16 identical large descriptors.
fn map_large(bus: &mut RamBus, va: u32, pa: u32, a: Attr) {
    let l2 = coarse_for(bus, va, a.dom);
    let first = ((va >> 12) & 0xFF) & !0xF;
    for i in 0..16 {
        poke(bus, l2 + ((first + i) << 2), large(pa, a));
    }
}

/// Registers for a translation under TTBR0 = `TT0`, TTBCR.N = 0, `dacr`.
fn regs(dacr: u32, privileged: bool) -> Regs {
    Regs {
        sctlr: M | XP,
        ttbr0: TT0,
        ttbr1: TT1,
        ttbcr: 0,
        dacr,
        asid: 0,
        privileged,
    }
}

/// Translates through the walker only (no TLB), as the tests' expected values expect.
fn xl(bus: &mut RamBus, r: &Regs, va: u32, acc: Access) -> Result<u64, u16> {
    mmu::translate_uncached(bus, r, va, acc).map(|t| t.pa)
}

// ---------------------------------------------------------------------------------------------
// Descriptor types, addresses and offsets
// ---------------------------------------------------------------------------------------------

#[test]
fn a_section_maps_one_megabyte_with_its_offset() {
    let mut bus = ram();
    map_section(
        &mut bus,
        0x0034_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    let r = regs(0b01, true);
    assert_eq!(xl(&mut bus, &r, 0x0030_0000, Access::Read), Ok(0x0080_0000));
    assert_eq!(xl(&mut bus, &r, 0x0034_0000, Access::Read), Ok(0x0084_0000));
    assert_eq!(xl(&mut bus, &r, 0x0034_5678, Access::Read), Ok(0x0084_5678));
    assert_eq!(xl(&mut bus, &r, 0x003F_FFFF, Access::Read), Ok(0x008F_FFFF));
    let t = mmu::translate_uncached(&mut bus, &r, 0x0034_5678, Access::Read).unwrap();
    assert_eq!(t.mapping.size_log2, 20);
    assert!(!t.mapping.page);
    assert_eq!(
        t.mapping.va_base, 0x0030_0000,
        "section base is the megabyte"
    );
}

#[test]
fn a_supersection_maps_sixteen_megabytes_with_forty_bit_addresses() {
    // PA = 0x5_3800_0000: PA[31:24] = 0x38 at 31:24, PA[35:32] = 3 at 23:20, PA[39:36] = 5 at
    // 8:5 (so PA[39:32] is 0x53 and the domain field holds 0x5 as PA bits).
    let pa: u64 = (0x5 << 36) | (0x3 << 32) | 0x3800_0000;
    let entry = supersection(
        pa,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    let mut bus = ram();
    // The supersection at VA 0x4100_0000 is 16 identical first-level entries.
    for i in 0..16u32 {
        poke(&mut bus, l1_addr(TT0, 0x4100_0000) + i * 4, entry);
    }
    let r = regs(0b01, true);
    let t = mmu::translate_uncached(&mut bus, &r, 0x4123_4567, Access::Read).unwrap();
    assert_eq!(
        t.pa,
        pa | 0x0023_4567,
        "offset within 16 MiB, PA bits from the three fields"
    );
    assert_eq!(t.mapping.size_log2, 24);
    assert_eq!(t.mapping.domain, 0, "a supersection has domain 0");
    assert_eq!(t.mapping.va_base, 0x4100_0000);
    let t2 = mmu::translate_uncached(&mut bus, &r, 0x41FF_FFFF, Access::Read).unwrap();
    assert_eq!(t2.pa, pa | 0x00FF_FFFF);
}

#[test]
fn a_small_page_maps_four_kilobytes() {
    let mut bus = ram();
    map_small(
        &mut bus,
        0x0013_F000,
        0x0080_3000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    let r = regs(0b01, true);
    assert_eq!(xl(&mut bus, &r, 0x0013_F000, Access::Read), Ok(0x0080_3000));
    assert_eq!(xl(&mut bus, &r, 0x0013_F123, Access::Read), Ok(0x0080_3123));
    assert_eq!(xl(&mut bus, &r, 0x0013_FFFF, Access::Read), Ok(0x0080_3FFF));
    let t = mmu::translate_uncached(&mut bus, &r, 0x0013_F123, Access::Read).unwrap();
    assert_eq!(t.mapping.size_log2, 12);
    assert!(t.mapping.page);
    assert_eq!(t.mapping.sub_shift, 10);
}

#[test]
fn a_large_page_maps_sixty_four_kilobytes_and_neighbours_are_independent() {
    let mut bus = ram();
    map_large(
        &mut bus,
        0x0021_0000,
        0x0090_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    // The next 64 KiB block is unmapped (a translation fault), not part of the large page.
    let r = regs(0b01, true);
    assert_eq!(xl(&mut bus, &r, 0x0021_0000, Access::Read), Ok(0x0090_0000));
    assert_eq!(xl(&mut bus, &r, 0x0021_3456, Access::Read), Ok(0x0090_3456));
    assert_eq!(xl(&mut bus, &r, 0x0021_FFFF, Access::Read), Ok(0x0090_FFFF));
    assert_eq!(
        xl(&mut bus, &r, 0x0022_0000, Access::Read),
        Err(mmu::FS_TRANSLATION_PAGE),
        "the next block's second-level entry is empty"
    );
    let t = mmu::translate_uncached(&mut bus, &r, 0x0021_3456, Access::Read).unwrap();
    assert_eq!(t.mapping.size_log2, 16);
    assert_eq!(t.mapping.va_base, 0x0021_0000);
}

#[test]
fn texture_cacheability_and_sharing_bits_are_recorded_for_every_descriptor_type() {
    let mut bus = ram();
    let a = Attr {
        ap: 3,
        tex: 5,
        c: true,
        b: false,
        s: true,
        ng: true,
        ..Attr::default()
    };
    map_section(&mut bus, 0x0050_0000, 0x0000_0000, a);
    map_small(&mut bus, 0x0060_0000, 0x0080_0000, a);
    map_large(&mut bus, 0x0070_0000, 0x0090_0000, Attr { b: true, ..a });
    let r = regs(0b01, true);
    let s = mmu::translate_uncached(&mut bus, &r, 0x0050_0000, Access::Read)
        .unwrap()
        .mapping;
    assert_eq!((s.tex, s.c, s.b, s.s, s.ng), (5, true, false, true, true));
    let p = mmu::translate_uncached(&mut bus, &r, 0x0060_0000, Access::Read)
        .unwrap()
        .mapping;
    assert_eq!((p.tex, p.c, p.b, p.s, p.ng), (5, true, false, true, true));
    let l = mmu::translate_uncached(&mut bus, &r, 0x0070_0000, Access::Read)
        .unwrap()
        .mapping;
    assert_eq!((l.tex, l.c, l.b, l.s, l.ng), (5, true, true, true, true));
}

#[test]
fn execute_never_is_recorded_per_descriptor_type_and_pxn_for_sections_and_coarse_tables() {
    let mut bus = ram();
    map_section(
        &mut bus,
        0x0200_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            xn: true,
            pxn: true,
            ..Attr::default()
        },
    );
    map_small(
        &mut bus,
        0x0300_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            xn: true,
            ..Attr::default()
        },
    );
    map_large(
        &mut bus,
        0x0310_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            xn: true,
            ..Attr::default()
        },
    );
    let r = regs(0b01, true);
    let s = mmu::translate_uncached(&mut bus, &r, 0x0200_0000, Access::Read)
        .unwrap()
        .mapping;
    assert!(s.xn && s.pxn, "section: XN bit 4, PXN bit 0");
    let p = mmu::translate_uncached(&mut bus, &r, 0x0300_0000, Access::Read)
        .unwrap()
        .mapping;
    assert!(p.xn && !p.pxn, "small page: XN bit 0");
    let l = mmu::translate_uncached(&mut bus, &r, 0x0310_0000, Access::Read)
        .unwrap()
        .mapping;
    assert!(l.xn, "large page: XN bit 15");
    // A coarse table's PXN (bit 2) applies to every page under it.
    let l2 = coarse_for(&mut bus, 0x0320_0000, 0);
    poke(
        &mut bus,
        l1_addr(TT0, 0x0320_0000),
        coarse(l2, 0, false, true),
    );
    poke(
        &mut bus,
        l2,
        small(
            0x0080_0000,
            Attr {
                ap: 3,
                ..Attr::default()
            },
        ),
    );
    let c = mmu::translate_uncached(&mut bus, &r, 0x0320_0000, Access::Read)
        .unwrap()
        .mapping;
    assert!(c.pxn && !c.xn);
}

// ---------------------------------------------------------------------------------------------
// Faults: status codes
// ---------------------------------------------------------------------------------------------

#[test]
fn translation_faults_have_the_section_and_page_status_codes() {
    let mut bus = ram();
    let r = regs(0b01, true);
    assert_eq!(
        xl(&mut bus, &r, 0x0500_0000, Access::Read),
        Err(0x05),
        "empty first-level entry"
    );
    let l2 = coarse_for(&mut bus, 0x0500_0000, 0);
    assert_eq!(l2 % 0x400, 0);
    assert_eq!(
        xl(&mut bus, &r, 0x0500_0000, Access::Read),
        Err(0x07),
        "empty second-level entry"
    );
}

#[test]
fn walk_faults_are_external_aborts_on_the_first_and_second_level() {
    let mut bus = ram();
    // TTBR0 outside the 32 MiB RAM: the first-level read is an external abort.
    let r = Regs {
        ttbr0: 0x0400_0000,
        ..regs(0b01, true)
    };
    assert_eq!(xl(&mut bus, &r, 0x0000_1000, Access::Read), Err(0x0C));
    // A coarse entry whose table is outside RAM: the second-level read aborts.
    poke(
        &mut bus,
        l1_addr(TT0, 0x0600_0000),
        coarse(0x0400_0000, 0, false, false),
    );
    assert_eq!(
        xl(&mut bus, &regs(0b01, true), 0x0600_0000, Access::Read),
        Err(0x0E)
    );
}

#[test]
fn domain_fault_status_depends_on_the_domain_and_the_section_or_page_size() {
    let mut bus = ram();
    map_section(
        &mut bus,
        0x0700_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            dom: 5,
            ..Attr::default()
        },
    );
    map_small(
        &mut bus,
        0x0710_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            dom: 5,
            ..Attr::default()
        },
    );
    for dacr_field in 0..4u32 {
        let dacr = dacr_field << (2 * 5);
        let r = regs(dacr, true);
        let sec = mmu::translate_uncached(&mut bus, &r, 0x0700_0000, Access::Read);
        let pg = mmu::translate_uncached(&mut bus, &r, 0x0710_0000, Access::Read);
        match dacr_field {
            0 | 2 => {
                assert_eq!(sec, Err(0x09 | (5 << 4)), "section domain fault, domain 5");
                assert_eq!(pg, Err(0x0B | (5 << 4)), "page domain fault, domain 5");
            }
            1 => {
                assert!(
                    sec.is_ok() && pg.is_ok(),
                    "client: AP checked, AP allows this"
                );
            }
            _ => {
                assert!(sec.is_ok() && pg.is_ok(), "manager: no check");
            }
        }
    }
}

#[test]
fn a_manager_domain_skips_the_permission_check_and_a_client_does_not() {
    let mut bus = ram();
    // AP = 000 (no access for anyone) and XN: a client domain faults, a manager does not.
    map_section(
        &mut bus,
        0x0800_0000,
        0x0080_0000,
        Attr {
            ap: 0,
            xn: true,
            ..Attr::default()
        },
    );
    let client = regs(0b01, true);
    assert_eq!(xl(&mut bus, &client, 0x0800_0000, Access::Read), Err(0x0D));
    let manager = regs(0b11, false);
    assert_eq!(
        xl(&mut bus, &manager, 0x0800_0000, Access::Fetch),
        Ok(0x0080_0000)
    );
}

// ---------------------------------------------------------------------------------------------
// Permissions: the AP x privilege x access x XN matrix
// ---------------------------------------------------------------------------------------------

/// ARM ARM Table B3-6 for AP[2:0] with XP = 1, written as data: for each AP value the
/// read and write permissions of privileged and user accesses. AP 0b100 is UNPREDICTABLE and
/// is taken as no access.
const AP_TABLE: [[bool; 4]; 8] = [
    // priv read, priv write, user read, user write
    [false, false, false, false], // 000
    [true, true, false, false],   // 001
    [true, true, true, false],    // 010
    [true, true, true, true],     // 011
    [false, false, false, false], // 100 (unpredictable: none)
    [true, false, false, false],  // 101
    [true, false, true, false],   // 110
    [true, false, true, false],   // 111
];

/// The same for a subpage AP[1:0] with XP = 0 (ARMv5 encoding, S = R = 0).
const SUB_TABLE: [[bool; 4]; 4] = [
    [false, false, false, false],
    [true, true, false, false],
    [true, true, true, false],
    [true, true, true, true],
];

/// Expected result of one access, from the tables above and the execute-never bits.
fn expected(row: [bool; 4], privileged: bool, acc: Access, xn: bool, pxn: bool) -> bool {
    let (r, w) = if privileged {
        (row[0], row[1])
    } else {
        (row[2], row[3])
    };
    match acc {
        Access::Read => r,
        Access::Write => w,
        Access::Fetch => r && !xn && !(privileged && pxn),
    }
}

#[test]
fn the_permission_matrix_for_sections_and_small_pages_matches_the_tables() {
    let mut bus = ram();
    let mut checked = 0;
    for ap in 0..8u32 {
        for xn in [false, true] {
            for pxn in [false, true] {
                for privileged in [false, true] {
                    for acc in [Access::Read, Access::Write, Access::Fetch] {
                        // Section at 0x0900_0000 (fresh entry each time).
                        let a = Attr {
                            ap,
                            xn,
                            pxn,
                            ..Attr::default()
                        };
                        map_section(&mut bus, 0x0900_0000, 0x0080_0000, a);
                        let want = expected(AP_TABLE[ap as usize], privileged, acc, xn, pxn);
                        let got = xl(&mut bus, &regs(0b01, privileged), 0x0900_0000, acc).is_ok();
                        assert_eq!(
                            got, want,
                            "section ap={ap:03b} xn={xn} pxn={pxn} priv={privileged} {acc:?}"
                        );
                        // Small page, same attributes (no PXN: the coarse entry's PXN is 0).
                        map_small(&mut bus, 0x0910_0000, 0x0080_0000, Attr { pxn: false, ..a });
                        let want_page = expected(AP_TABLE[ap as usize], privileged, acc, xn, false);
                        let got_page =
                            xl(&mut bus, &regs(0b01, privileged), 0x0910_0000, acc).is_ok();
                        assert_eq!(
                            got_page, want_page,
                            "small page ap={ap:03b} xn={xn} priv={privileged} {acc:?}"
                        );
                        checked += 2;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 8 * 2 * 2 * 2 * 3 * 2);
}

#[test]
fn a_permission_fault_has_the_section_or_page_status_with_the_domain() {
    let mut bus = ram();
    map_section(
        &mut bus,
        0x0A00_0000,
        0x0080_0000,
        Attr {
            ap: 0b001,
            dom: 2,
            ..Attr::default()
        },
    );
    map_small(
        &mut bus,
        0x0A10_0000,
        0x0080_0000,
        Attr {
            ap: 0b001,
            dom: 2,
            ..Attr::default()
        },
    );
    let user = regs(0b01 << 4, false);
    assert_eq!(
        xl(&mut bus, &user, 0x0A00_0000, Access::Read),
        Err(0x0D | (2 << 4))
    );
    assert_eq!(
        xl(&mut bus, &user, 0x0A10_0000, Access::Read),
        Err(0x0F | (2 << 4))
    );
}

#[test]
fn xp_zero_uses_the_subpage_permissions_of_small_and_large_pages() {
    let mut bus = ram();
    let r_priv = |xp: bool| Regs {
        sctlr: M | if xp { XP } else { 0 },
        ..regs(0b01, true)
    };
    let r_user = |xp: bool| Regs {
        sctlr: M | if xp { XP } else { 0 },
        ..regs(0b01, false)
    };
    // One small page whose four 1 KiB subpages have each a different AP.
    let l2 = coarse_for(&mut bus, 0x0B00_0000, 0);
    let sub = [0u32, 1, 2, 3];
    poke(&mut bus, l2, small_subpages(0x0080_0000, sub, false));
    for (k, row) in SUB_TABLE.iter().enumerate() {
        let va = 0x0B00_0000 + (k as u32) * 0x400;
        for privileged in [false, true] {
            for acc in [Access::Read, Access::Write] {
                let want = expected(SUB_TABLE[sub[k] as usize], privileged, acc, false, false);
                let r = if privileged {
                    r_priv(false)
                } else {
                    r_user(false)
                };
                let got = xl(&mut bus, &r, va, acc).is_ok();
                assert_eq!(
                    got, want,
                    "subpage {k} priv={privileged} {acc:?} row={row:?}"
                );
            }
        }
    }
    // XP = 1 ignores the subpage fields: AP[1:0] is bits 5:4 (0b00) and APX is bit 9 (set by
    // the subpage 2 value), so AP = 0b100 (unpredictable, no access) for every subpage.
    assert_eq!(
        xl(&mut bus, &r_user(true), 0x0B00_0C00, Access::Read),
        Err(0x0F),
        "XP = 1: the page's single AP, here 0b100, gives no access"
    );
}

#[test]
fn the_large_page_subpage_index_is_bits_15_14_when_xp_is_zero() {
    let mut bus = ram();
    let l2 = coarse_for(&mut bus, 0x0C00_0000, 0);
    // Sixteen identical large descriptors: subpage AP0 = 00 (none), AP1 = 01 (priv RW),
    // AP2 = 10 (priv RW, user RO), AP3 = 11 (all). Bits 5:4, 7:6, 9:8, 11:10.
    let large_xp0 = (0x0080_0000 & 0xFFFF_0000) | (0b01 << 6) | (0b10 << 8) | (0b11 << 10) | 0b01;
    for i in 0..16 {
        poke(&mut bus, l2 + (i << 2), large_xp0);
    }
    let r = Regs {
        sctlr: M,
        ..regs(0b01, false)
    };
    for k in 0..4u32 {
        // Subpage k is 16 KiB: VA bits 15:14.
        let va = 0x0C00_0000 + k * 0x4000 + 0x123;
        let want = expected(SUB_TABLE[k as usize], false, Access::Read, false, false);
        assert_eq!(
            xl(&mut bus, &r, va, Access::Read).is_ok(),
            want,
            "large subpage {k}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Table base selection (TTBCR.N) and the TLB-less walk
// ---------------------------------------------------------------------------------------------

#[test]
fn ttbcr_n_selects_ttbr1_for_the_upper_addresses_only() {
    let mut bus = ram();
    const TT1_ALT: u32 = 0x0101_4000;
    // TTBR0 maps 0x8000_0000 region to PA 0x0080_0000; TTBR1 maps it to PA 0x0090_0000.
    poke(
        &mut bus,
        l1_addr(TT0, 0x8000_0000),
        section(
            0x0080_0000,
            Attr {
                ap: 3,
                ..Attr::default()
            },
        ),
    );
    poke(
        &mut bus,
        l1_addr(TT1_ALT, 0x8000_0000),
        section(
            0x0090_0000,
            Attr {
                ap: 3,
                ..Attr::default()
            },
        ),
    );
    poke(
        &mut bus,
        l1_addr(TT0, 0x0000_1000),
        section(
            0x00A0_0000,
            Attr {
                ap: 3,
                ..Attr::default()
            },
        ),
    );
    poke(
        &mut bus,
        l1_addr(TT1_ALT, 0x0000_1000),
        section(
            0x00B0_0000,
            Attr {
                ap: 3,
                ..Attr::default()
            },
        ),
    );
    // TTBR1 also maps 0xE000_0000 (VA[31:29] = 111) to PA 0x0090_0000.
    poke(
        &mut bus,
        l1_addr(TT1_ALT, 0xE000_0000),
        section(
            0x0090_0000,
            Attr {
                ap: 3,
                ..Attr::default()
            },
        ),
    );
    let mut r = Regs {
        ttbr1: TT1_ALT,
        ttbcr: 1,
        ..regs(0b01, true)
    };
    // N = 1: VA[31] set uses TTBR1, clear uses TTBR0 (offset 0x1000 inside the section).
    assert_eq!(xl(&mut bus, &r, 0x8000_0000, Access::Read), Ok(0x0090_0000));
    assert_eq!(xl(&mut bus, &r, 0x0000_1000, Access::Read), Ok(0x00A0_1000));
    // N = 0: everything uses TTBR0.
    r.ttbcr = 0;
    assert_eq!(xl(&mut bus, &r, 0x8000_0000, Access::Read), Ok(0x0080_0000));
    // N = 3: TTBR0 is aligned to 2^(14-3) = 2 KiB, so its low 11 bits are ignored. Addresses
    // with VA[31:29] non-zero use TTBR1: 0x2000_0000 (001) has no entry in TTBR1.
    r.ttbcr = 3;
    r.ttbr0 = TT0 | 0x7FF;
    assert_eq!(
        xl(&mut bus, &r, 0x0000_1000, Access::Read),
        Ok(0x00A0_1000),
        "TTBR0 low bits ignored"
    );
    assert_eq!(
        xl(&mut bus, &r, 0x2000_0000, Access::Read),
        Err(0x05),
        "001 uses TTBR1: empty entry"
    );
    assert_eq!(
        xl(&mut bus, &r, 0xE000_0000, Access::Read),
        Ok(0x0090_0000),
        "111 uses TTBR1"
    );
}

// ---------------------------------------------------------------------------------------------
// The TLB
// ---------------------------------------------------------------------------------------------

#[test]
fn the_tlb_serves_the_old_walk_until_it_is_invalidated() {
    let mut bus = ram();
    map_small(
        &mut bus,
        0x0D00_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    let r = regs(0b01, true);
    let mut tlb = Mmu::new();
    assert_eq!(
        tlb.translate(&mut bus, &r, 0x0D00_0010, Access::Read),
        Ok(0x0080_0010)
    );
    assert_eq!(tlb.tlb_len(), 1);
    // Change the page; the TLB still answers with the old frame.
    map_small(
        &mut bus,
        0x0D00_0000,
        0x0090_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    assert_eq!(
        tlb.translate(&mut bus, &r, 0x0D00_0010, Access::Read),
        Ok(0x0080_0010),
        "stale until invalidated"
    );
    tlb.invalidate(TlbOp::Mva {
        va: 0x0D00_0FFF,
        asid: 0,
    });
    assert_eq!(tlb.tlb_len(), 0);
    assert_eq!(
        tlb.translate(&mut bus, &r, 0x0D00_0010, Access::Read),
        Ok(0x0090_0010),
        "TLBIMVA on the page"
    );
}

#[test]
fn tlb_invalidate_operations_remove_what_their_operand_names() {
    let mut bus = ram();
    map_small(
        &mut bus,
        0x0E00_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            ng: true,
            ..Attr::default()
        },
    );
    map_small(
        &mut bus,
        0x0E10_0000,
        0x0081_0000,
        Attr {
            ap: 3,
            ng: false,
            ..Attr::default()
        },
    );
    let mut asid1 = regs(0b01, true);
    asid1.asid = 1;
    let mut asid2 = asid1;
    asid2.asid = 2;
    let mut tlb = Mmu::new();
    // Non-global entry tagged ASID 1, global entry.
    tlb.translate(&mut bus, &asid1, 0x0E00_0000, Access::Read)
        .unwrap();
    tlb.translate(&mut bus, &asid1, 0x0E10_0000, Access::Read)
        .unwrap();
    assert_eq!(tlb.tlb_len(), 2);
    // A global entry is shared: after the tables change, ASID 2 still sees the old frame.
    map_small(
        &mut bus,
        0x0E10_0000,
        0x0082_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    assert_eq!(
        tlb.translate(&mut bus, &asid2, 0x0E10_0000, Access::Read),
        Ok(0x0081_0000),
        "global entry hit"
    );
    // The non-global entry is tagged ASID 1: ASID 2 misses and walks (sees the new frame).
    map_small(
        &mut bus,
        0x0E00_0000,
        0x0083_0000,
        Attr {
            ap: 3,
            ng: true,
            ..Attr::default()
        },
    );
    assert_eq!(
        tlb.translate(&mut bus, &asid2, 0x0E00_0000, Access::Read),
        Ok(0x0083_0000),
        "other ASID misses"
    );
    assert_eq!(
        tlb.translate(&mut bus, &asid1, 0x0E00_0000, Access::Read),
        Ok(0x0080_0000),
        "ASID 1 still hits its old entry"
    );
    // TLBIASID(1) drops the ASID 1 entry and keeps the global one.
    tlb.invalidate(TlbOp::Asid { asid: 1 });
    assert_eq!(
        tlb.translate(&mut bus, &asid1, 0x0E00_0000, Access::Read),
        Ok(0x0083_0000),
        "re-walked"
    );
    assert_eq!(
        tlb.translate(&mut bus, &asid1, 0x0E10_0000, Access::Read),
        Ok(0x0081_0000),
        "global kept"
    );
    // TLBIMVAA removes the page for every ASID, global or not.
    tlb.invalidate(TlbOp::MvaAll { va: 0x0E10_0000 });
    map_small(
        &mut bus,
        0x0E10_0000,
        0x0084_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    assert_eq!(
        tlb.translate(&mut bus, &asid1, 0x0E10_0000, Access::Read),
        Ok(0x0084_0000)
    );
    // TLBIALL empties the TLB.
    tlb.invalidate(TlbOp::All);
    assert_eq!(tlb.tlb_len(), 0);
}

#[test]
fn permissions_are_checked_on_every_access_and_faults_are_not_cached() {
    let mut bus = ram();
    map_small(
        &mut bus,
        0x0F00_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            dom: 4,
            ..Attr::default()
        },
    );
    let mut tlb = Mmu::new();
    let r_client = regs(0b01 << 8, true);
    assert_eq!(
        tlb.translate(&mut bus, &r_client, 0x0F00_0000, Access::Read),
        Ok(0x0080_0000)
    );
    // Revoking the domain (DACR, no TLB operation) takes effect at once: the entry is cached,
    // but the domain check runs on every access.
    let r_none = regs(0b00 << 8, true);
    assert_eq!(
        tlb.translate(&mut bus, &r_none, 0x0F00_0000, Access::Read),
        Err(0x0B | (4 << 4))
    );
    // A fault is not cached: fix the table and the next access sees the fix with no TLBI.
    let mut bus2 = ram();
    let mut tlb2 = Mmu::new();
    assert_eq!(
        tlb2.translate(&mut bus2, &regs(0b01, true), 0x0F10_0000, Access::Read),
        Err(0x05)
    );
    map_small(
        &mut bus2,
        0x0F10_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    assert_eq!(
        tlb2.translate(&mut bus2, &regs(0b01, true), 0x0F10_0000, Access::Read),
        Ok(0x0080_0000)
    );
}

#[test]
fn with_sctlr_m_clear_the_mmu_is_the_identity_and_ignores_the_tables() {
    let mut bus = ram();
    map_small(
        &mut bus,
        0x0000_1000,
        0x0080_0000,
        Attr {
            ap: 0,
            ..Attr::default()
        },
    );
    let mut tlb = Mmu::new();
    let flat = Regs {
        sctlr: 0,
        ..regs(0, false)
    };
    assert_eq!(
        tlb.translate(&mut bus, &flat, 0x0000_1000, Access::Fetch),
        Ok(0x0000_1000)
    );
    assert_eq!(tlb.tlb_len(), 0, "no TLB fill while the MMU is off");
}

// ---------------------------------------------------------------------------------------------
// The core: programs with CP15 maintenance, abort entry and return
// ---------------------------------------------------------------------------------------------

/// MCR p15, 0, Rt, CRn, CRm, opc2.
const fn mcr(crn: u32, crm: u32, opc2: u32, rt: u32) -> u32 {
    0xEE00_0F10 | (crn << 16) | (rt << 12) | (opc2 << 5) | crm
}
/// MRC p15, 0, Rt, CRn, CRm, opc2.
const fn mrc(crn: u32, crm: u32, opc2: u32, rt: u32) -> u32 {
    0xEE10_0F10 | (crn << 16) | (rt << 12) | (opc2 << 5) | crm
}
/// LDR Rt, [Rn, #off].
const fn ldr(rt: u32, rn: u32, off: u32) -> u32 {
    0xE590_0000 | (rn << 16) | (rt << 12) | off
}
/// STR Rt, [Rn, #off].
const fn str_(rt: u32, rn: u32, off: u32) -> u32 {
    0xE580_0000 | (rn << 16) | (rt << 12) | off
}
/// LDRT Rt, [Rn] (P = 0, W = 1).
const fn ldrt(rt: u32, rn: u32) -> u32 {
    0xE4B0_0000 | (rn << 16) | (rt << 12)
}
/// STRT Rt, [Rn] (P = 0, W = 1).
const fn strt(rt: u32, rn: u32) -> u32 {
    0xE4A0_0000 | (rn << 16) | (rt << 12)
}
/// MOVW Rd, #imm16.
const fn movw(rd: u32, imm: u32) -> u32 {
    0xE300_0000 | ((imm >> 12) << 16) | (rd << 12) | (imm & 0xFFF)
}
/// MOVT Rd, #imm16.
const fn movt(rd: u32, imm: u32) -> u32 {
    0xE340_0000 | ((imm >> 12) << 16) | (rd << 12) | (imm & 0xFFF)
}
/// The two-word load of a 32-bit constant.
fn load32(rd: u32, v: u32) -> [u32; 2] {
    [movw(rd, v & 0xFFFF), movt(rd, v >> 16)]
}
/// MOV Rd, #imm8.
const fn mov(rd: u32, imm: u32) -> u32 {
    0xE3A0_0000 | (rd << 12) | imm
}
/// ADD Rd, Rn, Rm.
const fn add_rr(rd: u32, rn: u32, rm: u32) -> u32 {
    0xE080_0000 | (rn << 16) | (rd << 12) | rm
}
/// SUBS PC, LR, #8 (return from a data abort and retry the instruction).
const RETRY_DATA: u32 = 0xE25E_F008;
/// SUBS PC, LR, #4 (return from a prefetch abort and retry the instruction).
const RETRY_PREFETCH: u32 = 0xE25E_F004;
/// BX Rm.
const fn bx(rm: u32) -> u32 {
    0xE12F_FF10 | rm
}
/// B . (branch to itself).
const SPIN: u32 = 0xEAFF_FFFE;

/// A core with 32 MiB of RAM, the vector table at 0 (data abort and prefetch abort return by
/// retrying), and identity sections for the RAM. The MMU is off until a program turns it on.
struct Machine {
    cpu: Cpu,
    bus: RamBus,
}

fn machine(program: &[u32]) -> Machine {
    let mut bus = ram();
    for mb in 0..32u32 {
        map_section(
            &mut bus,
            mb << 20,
            mb << 20,
            Attr {
                ap: 3,
                ..Attr::default()
            },
        );
    }
    poke(&mut bus, 0x0C, RETRY_PREFETCH);
    poke(&mut bus, 0x10, RETRY_DATA);
    for (i, w) in program.iter().enumerate() {
        poke(&mut bus, CODE + 4 * i as u32, *w);
    }
    let mut cpu = Cpu::new(0);
    cpu.reset(CODE);
    Machine { cpu, bus }
}

impl Machine {
    fn step(&mut self) -> StepResult {
        self.cpu.step(&mut self.bus)
    }

    /// Steps until the PC is `pc`, with no event on the way.
    fn run_to(&mut self, pc: u32) {
        for _ in 0..10_000 {
            if self.cpu.pc() == pc {
                return;
            }
            match self.step() {
                StepResult::Executed { event: None, .. } => {}
                other => panic!(
                    "unexpected {other:?} at {:#x}, wanted {pc:#x}",
                    self.cpu.pc()
                ),
            }
        }
        panic!("pc never reached {pc:#x}");
    }

    /// Steps once; the step must execute and report an event.
    fn event(&mut self) -> Event {
        match self.step() {
            StepResult::Executed { event: Some(e), .. } => e,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    fn cp15(&self, crn: u8, crm: u8, opc2: u8) -> u32 {
        self.cpu.cp15(0, crn, crm, opc2).unwrap()
    }
}

/// The word address of instruction `i` of a program at [`CODE`].
fn at(i: usize) -> u32 {
    CODE + 4 * i as u32
}

/// Turns the MMU on with TTBR0 = [`TT0`], DACR = `dacr`, and SCTLR = reset | M | XP (the
/// Cortex-A9 reset SCTLR already has XP set).
fn mmu_on(dacr: u32) -> Vec<u32> {
    let mut p = Vec::new();
    p.extend(load32(0, TT0));
    p.push(mcr(2, 0, 0, 0)); // TTBR0
    p.extend(load32(0, dacr));
    p.push(mcr(3, 0, 0, 0)); // DACR
    p.extend(load32(0, 0x00C5_0079)); // reset | M
    p.push(mcr(1, 0, 0, 0)); // SCTLR
    p.push(mcr(8, 7, 0, 0)); // TLBIALL
    p.push(0xF57F_F06F); // ISB
    p
}

#[test]
fn an_identity_mapped_program_runs_with_the_mmu_on() {
    // Sum 1..=10 with the MMU on, every fetch translated through the sections.
    let mut p = mmu_on(0b01);
    let start = p.len();
    p.push(mov(1, 0));
    p.push(mov(2, 10));
    p.push(add_rr(1, 1, 2)); // loop
    p.push(0xE252_2001); // SUBS r2, r2, #1
    p.push(0x1AFF_FFFC); // BNE loop
    p.push(SPIN);
    let mut m = machine(&p);
    m.run_to(at(start));
    m.run_to(at(p.len() - 1));
    assert_eq!(m.cpu.reg(1), 55);
    assert_eq!(m.cp15(1, 0, 0) & M, M, "SCTLR.M set");
}

#[test]
fn ldrt_and_strt_are_translated_with_user_permissions() {
    // A page only privileged code may access (AP = 001).
    // Program: MMU on, then LDR (privileged, must work), LDRT, STRT, spin.
    let mut p = mmu_on(0b01);
    let first = p.len();
    p.extend(load32(4, 0x0200_0000));
    p.push(ldr(3, 4, 0));
    p.push(ldrt(5, 4));
    p.push(strt(5, 4));
    p.push(SPIN);
    let mut m = machine(&p);
    map_small(
        &mut m.bus,
        0x0200_0000,
        0x0080_0000,
        Attr {
            ap: 0b001,
            ..Attr::default()
        },
    );
    poke(&mut m.bus, 0x0080_0000, 0x1234_5678);
    // The privileged load works.
    m.run_to(at(first + 2));
    m.run_to(at(first + 3));
    assert_eq!(m.cpu.reg(3), 0x1234_5678, "a privileged LDR reads the page");
    // LDRT: a permission fault at the page, data abort, DFAR = the address.
    assert_eq!(m.event(), Event::DataAbort { addr: 0x0200_0000 });
    assert_eq!(
        m.cp15(5, 0, 0),
        0x0F,
        "DFSR: permission fault, page, domain 0, read"
    );
    assert_eq!(m.cp15(6, 0, 0), 0x0200_0000, "DFAR");
    assert_eq!(m.cpu.mode(), Mode::Abort);
    // Back in SVC at the STRT, the same fault on a write sets WnR.
    m.cpu
        .set_cpsr(Mode::Supervisor.bits() | (1 << 7) | (1 << 6));
    m.cpu.set_reg(15, at(first + 4));
    assert_eq!(m.event(), Event::DataAbort { addr: 0x0200_0000 });
    assert_eq!(
        m.cp15(5, 0, 0),
        0x0F | 0x800,
        "DFSR: the same fault on a write (WnR)"
    );
}

#[test]
fn a_translation_fault_takes_a_data_abort_and_the_retry_succeeds_after_the_table_is_fixed() {
    let mut p = mmu_on(0b01);
    let start = p.len();
    p.extend(load32(4, 0x0200_0000));
    p.push(ldr(3, 4, 0)); // faults first
    p.push(SPIN);
    let mut m = machine(&p);
    poke(&mut m.bus, 0x0080_0000, 0xBEEF);
    m.run_to(at(start));
    m.run_to(at(start + 2));
    let faulting = at(start + 2);
    assert_eq!(m.event(), Event::DataAbort { addr: 0x0200_0000 });
    assert_eq!(m.cp15(5, 0, 0), 0x05, "DFSR: translation fault, section");
    assert_eq!(m.cp15(6, 0, 0), 0x0200_0000, "DFAR");
    assert_eq!(
        m.cpu.reg(14),
        faulting + 8,
        "LR of a data abort is the instruction + 8"
    );
    assert_eq!(m.cpu.mode(), Mode::Abort);
    // The handler fixes the table (a small page) and returns with SUBS PC, LR, #8.
    map_small(
        &mut m.bus,
        0x0200_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    m.run_to(faulting); // the vector returns to the faulting instruction
    m.run_to(at(start + 3)); // the retried load completes
    assert_eq!(
        m.cpu.reg(3),
        0xBEEF,
        "the retried load reads the fixed page"
    );
}

#[test]
fn a_domain_fault_through_the_core_reports_the_domain_in_dfsr() {
    // Section 0x0200_0000 in domain 1, which DACR leaves at no access.
    let mut p = mmu_on(0b01);
    p.extend(load32(4, 0x0200_0000));
    p.push(ldr(3, 4, 0));
    let mut m = machine(&p);
    map_section(
        &mut m.bus,
        0x0200_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            dom: 1,
            ..Attr::default()
        },
    );
    m.run_to(at(p.len() - 1));
    let e = m.event();
    assert_eq!(e, Event::DataAbort { addr: 0x0200_0000 });
    assert_eq!(
        m.cp15(5, 0, 0),
        0x09 | (1 << 4),
        "section domain fault, domain 1"
    );
}

#[test]
fn an_execute_never_page_takes_a_prefetch_abort_at_the_branch_target() {
    let mut p = mmu_on(0b01);
    p.extend(load32(0, 0x0210_0000));
    p.push(bx(0));
    let mut m = machine(&p);
    map_section(
        &mut m.bus,
        0x0210_0000,
        0x0090_0000,
        Attr {
            ap: 3,
            xn: true,
            ..Attr::default()
        },
    );
    m.run_to(at(p.len() - 1));
    // The BX executes; the fetch at its target is the one that faults.
    assert!(matches!(m.step(), StepResult::Executed { event: None, .. }));
    assert_eq!(m.event(), Event::PrefetchAbort { addr: 0x0210_0000 });
    assert_eq!(m.cp15(5, 0, 1), 0x0D, "IFSR: permission fault, section");
    assert_eq!(m.cp15(6, 0, 2), 0x0210_0000, "IFAR");
    assert_eq!(
        m.cpu.reg(14),
        0x0210_0004,
        "LR of a prefetch abort is the address + 4"
    );
}

#[test]
fn alignment_faults_with_sctlr_a_on_report_wnr_for_stores() {
    // MMU off, SCTLR.A on: an unaligned word access is an alignment fault.
    let mut p = Vec::new();
    p.extend(load32(0, 0x00C5_007A)); // reset | A
    p.push(mcr(1, 0, 0, 0));
    p.extend(load32(4, 0x0000_2001));
    p.push(ldr(3, 4, 0));
    let mut m = machine(&p);
    m.run_to(at(p.len() - 1));
    assert_eq!(m.event(), Event::DataAbort { addr: 0x2001 });
    assert_eq!(m.cp15(5, 0, 0), 0x01, "DFSR: alignment");
    let mut q = Vec::new();
    q.extend(load32(0, 0x00C5_007A));
    q.push(mcr(1, 0, 0, 0));
    q.extend(load32(4, 0x0000_2001));
    q.push(str_(3, 4, 0));
    let mut m = machine(&q);
    m.run_to(at(q.len() - 1));
    assert_eq!(m.event(), Event::DataAbort { addr: 0x2001 });
    assert_eq!(m.cp15(5, 0, 0), 0x01 | 0x800, "DFSR: alignment on a write");
}

#[test]
fn a_remapped_page_is_seen_only_after_tlb_maintenance() {
    let mut p = mmu_on(0b01);
    p.extend(load32(4, 0x0200_0000));
    let base = p.len();
    p.push(ldr(3, 4, 0)); // maps through the TLB
    p.push(ldr(5, 4, 0)); // still the old frame
    p.push(mcr(8, 7, 1, 4)); // TLBIMVA VA in r4
    p.push(ldr(6, 4, 0)); // the new frame
    p.push(SPIN);
    let mut m = machine(&p);
    map_small(
        &mut m.bus,
        0x0200_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    poke(&mut m.bus, 0x0080_0000, 1);
    poke(&mut m.bus, 0x0081_0000, 2);
    m.run_to(at(base));
    m.run_to(at(base + 1));
    assert_eq!(m.cpu.reg(3), 1);
    map_small(
        &mut m.bus,
        0x0200_0000,
        0x0081_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    m.run_to(at(base + 2));
    assert_eq!(m.cpu.reg(5), 1, "stale translation until TLBIMVA");
    m.run_to(at(base + 4));
    assert_eq!(m.cpu.reg(6), 2, "after TLBIMVA the new frame is used");
}

#[test]
fn ats_writes_par_for_a_mapped_and_for_an_unmapped_address() {
    let mut p = mmu_on(0b01);
    p.extend(load32(4, 0x0200_0000));
    p.push(mcr(7, 8, 0, 4)); // ATS1CPR
    p.push(mrc(7, 4, 0, 5)); // PAR
    p.extend(load32(4, 0x0300_0000));
    p.push(mcr(7, 8, 0, 4)); // ATS1CPR on an empty entry
    p.push(mrc(7, 4, 0, 6));
    p.push(SPIN);
    let mut m = machine(&p);
    map_small(
        &mut m.bus,
        0x0200_0000,
        0x0080_0000,
        Attr {
            ap: 3,
            ..Attr::default()
        },
    );
    let base = p.len() - 1;
    m.run_to(at(base));
    assert_eq!(
        m.cpu.reg(5),
        0x0080_0000,
        "PAR: the physical frame, F clear"
    );
    assert_eq!(
        m.cpu.reg(6),
        1 | (0x05 << 1),
        "PAR: F set, FST = translation fault"
    );
}

// ---------------------------------------------------------------------------------------------
// Property test against an independent reference walk
// ---------------------------------------------------------------------------------------------

/// A reference translation written from the ARM ARM text, with a different structure from
/// `src/mmu.rs`: bit fields are extracted by one helper, the permission table is data, and
/// the domain and permission decisions are made in the order the ARM ARM gives them.
mod reference {
    use super::*;

    /// Bits `hi:lo` of `v`.
    fn bits(v: u32, hi: u32, lo: u32) -> u32 {
        (v >> lo) & ((1u32 << (hi - lo + 1)) - 1)
    }

    /// The translation registers and the configuration the reference needs.
    pub struct Cfg {
        pub ttbr0: u32,
        pub ttbr1: u32,
        pub ttbcr: u32,
        pub dacr: u32,
        pub xp: bool,
    }

    /// Returns the physical address, or the DFSR status of the fault (domain in 7:4).
    pub fn translate(
        bus: &mut RamBus,
        c: &Cfg,
        va: u32,
        acc: Access,
        privileged: bool,
    ) -> Result<u64, u16> {
        let n = bits(c.ttbcr, 2, 0);
        // TTBR1 when N > 0 and VA[31:32-N] is not zero; otherwise TTBR0[31:14-N].
        let upper = n > 0 && bits(va, 31, 32 - n) != 0;
        let (ttbr, lowest) = if upper {
            (c.ttbr1, 14)
        } else {
            (c.ttbr0, 14 - n)
        };
        let table = (ttbr >> lowest) << lowest;
        let d1 = bus
            .read32(u64::from(table + ((va >> 20) << 2)))
            .map_err(|_| 0x0Cu16)?;
        match bits(d1, 1, 0) {
            0b00 => Err(0x05),
            0b01 => {
                let dom = bits(d1, 8, 5);
                let l2 = (d1 & 0xFFFF_FC00) + (((va >> 12) & 0xFF) << 2);
                let d2 = bus.read32(u64::from(l2)).map_err(|_| 0x0Eu16)?;
                let pxn = bits(d1, 2, 2) == 1;
                match bits(d2, 1, 0) {
                    0b00 => Err(0x07),
                    0b01 => {
                        // Large page: XN 15, APX 9, AP 5:4; subpage fields 5:4, 7:6, 9:8, 11:10.
                        let sub = (va >> 14) & 3;
                        let ap_xp0 = bits(d2, 5 + 2 * sub, 4 + 2 * sub);
                        let ap = if c.xp {
                            (bits(d2, 9, 9) << 2) | bits(d2, 5, 4)
                        } else {
                            ap_xp0
                        };
                        let xn = bits(d2, 15, 15) == 1;
                        check_perm(c, dom, ap, xn, pxn, true, acc, privileged)?;
                        Ok(u64::from(d2 & 0xFFFF_0000) | u64::from(va & 0xFFFF))
                    }
                    _ => {
                        let sub = (va >> 10) & 3;
                        let ap_xp0 = bits(d2, 5 + 2 * sub, 4 + 2 * sub);
                        let ap = if c.xp {
                            (bits(d2, 9, 9) << 2) | bits(d2, 5, 4)
                        } else {
                            ap_xp0
                        };
                        let xn = bits(d2, 0, 0) == 1;
                        check_perm(c, dom, ap, xn, pxn, true, acc, privileged)?;
                        Ok(u64::from(d2 & 0xFFFF_F000) | u64::from(va & 0xFFF))
                    }
                }
            }
            _ => {
                // With XP = 0 a section's AP is the two-bit field alone (APX is not used).
                let section_ap = if c.xp {
                    (bits(d1, 15, 15) << 2) | bits(d1, 11, 10)
                } else {
                    bits(d1, 11, 10)
                };
                if bits(d1, 18, 18) == 1 {
                    // Supersection: domain 0, PA bits in 23:20 and 8:5.
                    let ap = section_ap;
                    check_perm(
                        c,
                        0,
                        ap,
                        bits(d1, 4, 4) == 1,
                        bits(d1, 0, 0) == 1,
                        false,
                        acc,
                        privileged,
                    )?;
                    let pa = u64::from(d1 & 0xFF00_0000)
                        | (u64::from(bits(d1, 23, 20)) << 32)
                        | (u64::from(bits(d1, 8, 5)) << 36);
                    Ok(pa | u64::from(va & 0x00FF_FFFF))
                } else {
                    check_perm(
                        c,
                        bits(d1, 8, 5),
                        section_ap,
                        bits(d1, 4, 4) == 1,
                        bits(d1, 0, 0) == 1,
                        false,
                        acc,
                        privileged,
                    )?;
                    Ok(u64::from(d1 & 0xFFF0_0000) | u64::from(va & 0x000F_FFFF))
                }
            }
        }
    }

    /// The domain check, then the permission check, for a region. `ap` is AP[2:0] when XP = 1;
    /// with XP = 0 the caller already picked the subpage's AP[1:0] (the high bit is 0).
    #[allow(clippy::too_many_arguments)]
    fn check_perm(
        c: &Cfg,
        dom: u32,
        ap: u32,
        xn: bool,
        pxn: bool,
        page: bool,
        acc: Access,
        privileged: bool,
    ) -> Result<(), u16> {
        let (dom_fs, perm_fs) = if page {
            (0x0Bu16, 0x0Fu16)
        } else {
            (0x09u16, 0x0Du16)
        };
        let d = bits(c.dacr, 2 * dom + 1, 2 * dom);
        let dfield = (dom as u16) << 4;
        match d {
            0b00 | 0b10 => return Err(dom_fs | dfield),
            0b11 => return Ok(()),
            _ => {}
        }
        // (priv read, priv write, user read, user write) per AP value, ARM ARM Table B3-6.
        const TABLE: [[u8; 4]; 8] = [
            [0, 0, 0, 0],
            [1, 1, 0, 0],
            [1, 1, 1, 0],
            [1, 1, 1, 1],
            [0, 0, 0, 0],
            [1, 0, 0, 0],
            [1, 0, 1, 0],
            [1, 0, 1, 0],
        ];
        let row = TABLE[ap as usize];
        let (read_ok, write_ok) = if privileged {
            (row[0], row[1])
        } else {
            (row[2], row[3])
        };
        let ok = match acc {
            Access::Read => read_ok == 1,
            Access::Write => write_ok == 1,
            Access::Fetch => read_ok == 1 && !xn && !(privileged && pxn),
        };
        if ok { Ok(()) } else { Err(perm_fs | dfield) }
    }
}

/// xorshift64*: deterministic, no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

#[test]
fn random_tables_agree_with_the_reference_walk() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut bus = ram();
    let mut walks_ok = 0u32;
    let mut walks_fault = 0u32;
    let mut mmu = Mmu::new();
    for iteration in 0..3000u32 {
        // Random first-level entries in the TTBR0 and TTBR1 tables. Each word is random except
        // for the bits that select its type: a fault, a coarse table in the L2 area (with random
        // domain, NS and PXN), or a section or supersection (bit 1 set, bit 18 random).
        // Blocks of 16 entries are written whole, so a supersection (16 MiB) or a large page
        // (64 KiB) is 16 identical descriptors, as the architecture requires. Single entries
        // are random and independent.
        for _ in 0..256 {
            let base = if rng.below(4) == 0 { TT1 } else { TT0 };
            let block = base + rng.below(256) * 64;
            if rng.below(8) == 0 {
                // A supersection: bit 18 set, bit 1 set; the other bits are random.
                let w = rng.next() | 0b10 | (1 << 18);
                for k in 0..16 {
                    poke(&mut bus, block + 4 * k, w);
                }
            } else {
                for k in 0..16 {
                    let w = rng.next();
                    let v = match rng.below(8) {
                        0 => 0,
                        1..=3 => {
                            let l2 = L2 + rng.below(256) * 0x400;
                            // Bits 9:5 (domain 8:5), 3 (NS), 2 (PXN); bit 4 must be zero.
                            (l2 & 0xFFFF_FC00) | (w & 0x3E0) | (w & 0xC) | 0b01
                        }
                        _ => (w | 0b10) & !(1 << 18),
                    };
                    poke(&mut bus, block + 4 * k, v);
                }
            }
        }
        // Second-level blocks of 16 entries: a large page (bits 1:0 = 01, replicated) or 16
        // random entries that are faults or small pages.
        for _ in 0..512 {
            let block = L2 + rng.below(4096) * 64;
            if rng.below(4) == 0 {
                let w = (rng.next() & 0xFFFF_FFFC) | 0b01;
                for k in 0..16 {
                    poke(&mut bus, block + 4 * k, w);
                }
            } else {
                for k in 0..16 {
                    let w = rng.next();
                    let v = match rng.below(4) {
                        0 => 0,
                        _ => (w & 0xFFFF_FFFC) | 0b10 | (w & 1),
                    };
                    poke(&mut bus, block + 4 * k, v);
                }
            }
        }
        let ttbr0 = TT0 | (rng.next() & 0x3FFF);
        let ttbr1 = TT1 | (rng.next() & 0x3FFF);
        let n = rng.below(8);
        let xp = rng.below(4) != 0;
        let dacr = rng.next();
        let asid = rng.below(4) as u8;
        // The TLB must not hold anything from an older table.
        mmu.invalidate(TlbOp::All);
        let cfg = reference::Cfg {
            ttbr0,
            ttbr1,
            ttbcr: n,
            dacr,
            xp,
        };
        for _ in 0..16 {
            let va = if rng.below(2) == 0 {
                rng.next()
            } else {
                (rng.below(4096) << 20) | (rng.next() & 0x000F_FFFF)
            };
            let acc = match rng.below(3) {
                0 => Access::Fetch,
                1 => Access::Read,
                _ => Access::Write,
            };
            let privileged = rng.below(2) == 0;
            let regs = Regs {
                sctlr: M | if xp { XP } else { 0 },
                ttbr0,
                ttbr1,
                ttbcr: n,
                dacr,
                asid,
                privileged,
            };
            let want = reference::translate(&mut bus, &cfg, va, acc, privileged);
            let got = mmu::translate_uncached(&mut bus, &regs, va, acc).map(|t| t.pa);
            assert_eq!(
                got, want,
                "iteration {iteration}: va={va:#010x} acc={acc:?} priv={privileged} ttbcr.N={n} xp={xp} dacr={dacr:#010x} ttbr0={ttbr0:#010x} ttbr1={ttbr1:#010x}"
            );
            // The TLB agrees with the walk when nothing changed.
            let cached = mmu.translate(&mut bus, &regs, va, acc);
            assert_eq!(cached, want, "TLB at iteration {iteration}, va={va:#010x}");
            if want.is_ok() {
                walks_ok += 1;
            } else {
                walks_fault += 1;
            }
        }
    }
    // The generator must reach successes and every kind of failure, or the test proves little.
    assert!(walks_ok > 1000, "successful translations: {walks_ok}");
    assert!(walks_fault > 1000, "faulting translations: {walks_fault}");
}
