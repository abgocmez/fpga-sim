//! The prototype's `my_math_ip` (fixtures/rtl/math) behind an AXI4-Lite slave, driven
//! from Rust through `RtlModule`, compared with the golden register model that the
//! firmware tests use (`pl_native::MathIp`).

use pl_native::{MathIp, RegisterBlock};
use rtl_engine::{Design, RtlModule};
use std::path::PathBuf;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("rtl")
        .join(rel)
}

fn math_ip() -> RtlModule {
    let design = Design::elaborate("my_math_ip", &[fixture("math/my_math_ip.v")], &[], &[]);
    assert!(
        design.diagnostics.is_empty(),
        "my_math_ip must elaborate clean: {:?}",
        design.diagnostics
    );
    RtlModule::new(design).expect("AXI4-Lite slave detected")
}

const VECTORS: [u32; 10] = [
    0,
    1,
    0x15,
    0x1234_5678,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFF,
    0xDEAD_BEEF,
    0x0000_0001,
    0xCAFE_BABE,
];

#[test]
fn register_writes_and_reads_agree_with_the_golden_model() {
    let mut rtl = math_ip();
    let mut golden = MathIp::new();
    for v in VECTORS {
        rtl.write(0x0, v, 0xF).unwrap();
        golden.write32(0x0, v, 0xF);
        let r0 = rtl.read(0x0).unwrap();
        let r1 = rtl.read(0x4).unwrap();
        assert_eq!(r0, golden.read32(0x0), "reg0 after writing {v:#x}");
        assert_eq!(r1, golden.read32(0x4), "reg1 = 2 * {v:#x}");
        assert_eq!(r1, v.wrapping_mul(2));
    }
}

#[test]
fn byte_strobes_select_lanes_like_the_golden_model() {
    let mut rtl = math_ip();
    let mut golden = MathIp::new();
    rtl.write(0x0, 0x1111_1111, 0xF).unwrap();
    golden.write32(0x0, 0x1111_1111, 0xF);
    rtl.write(0x0, 0xFFFF_FFFF, 0b0010).unwrap();
    golden.write32(0x0, 0xFFFF_FFFF, 0b0010);
    assert_eq!(rtl.read(0x0).unwrap(), golden.read32(0x0));
    assert_eq!(rtl.read(0x0).unwrap(), 0x1111_FF11);
}

#[test]
fn unmapped_offsets_read_zero_and_writes_to_reg1_are_ignored() {
    let mut rtl = math_ip();
    rtl.write(0x0, 0x21, 0xF).unwrap();
    rtl.write(0x4, 0xDEAD_BEEF, 0xF).unwrap();
    assert_eq!(rtl.read(0x4).unwrap(), 0x42);
    assert_eq!(
        rtl.read(0x8).unwrap(),
        0,
        "unmapped offset inside the 16-byte window"
    );
}

/// Known difference from the golden model: the RTL's `S_AXI_AWADDR` is 4 bits wide
/// (`C_S_AXI_ADDR_WIDTH = 4`), so an offset past 0xF aliases into the window. The golden
/// `MathIp` answers 0 for 0x10; the RTL answers what 0x0 answers. Pinned here so the
/// difference stays visible (see `docs/CORPUS_NATIVE.md`).
#[test]
fn offsets_past_the_address_width_alias_as_the_rtl_decodes_them() {
    let mut rtl = math_ip();
    let mut golden = MathIp::new();
    rtl.write(0x0, 0x21, 0xF).unwrap();
    golden.write32(0x0, 0x21, 0xF);
    assert_eq!(rtl.read(0x10).unwrap(), rtl.read(0x0).unwrap());
    assert_eq!(golden.read32(0x10), 0);
}

#[test]
fn reset_clears_the_operand_register() {
    let mut rtl = math_ip();
    rtl.write(0x0, 0x77, 0xF).unwrap();
    rtl.reset();
    assert_eq!(rtl.read(0x0).unwrap(), 0);
    assert_eq!(rtl.read(0x4).unwrap(), 0);
}

#[test]
fn the_engine_clock_advances_one_period_per_cycle() {
    let mut rtl = math_ip();
    let t0 = rtl.sim().now_ps();
    rtl.write(0x0, 1, 0xF).unwrap();
    assert!(rtl.sim().now_ps() > t0, "transactions take clock cycles");
    assert!(rtl.take_error().is_none());
}

#[test]
fn the_adapter_is_a_register_block_with_a_window() {
    let rtl = math_ip();
    assert_eq!(rtl.name(), "my_math_ip");
    assert!(rtl.size() >= 0x10);
}
