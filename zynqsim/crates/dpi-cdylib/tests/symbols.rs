//! The exported symbol set is the set the generated testbenches import.
//!
//! The expected set is the union of the DPI-C imports in every `.sv` fixture and
//! in a testbench that `sim-api` generates for the AXI4-Lite fixture (the code path
//! `zynqsim init --ip` uses). The exact-equality check reads the export table of the
//! shared object (PE, so Windows only); the resolution check loads it on any platform.

mod common;

use std::collections::BTreeSet;

/// The imports of the generated testbench for `fixtures/rtl/math/my_math_ip.v`.
fn generated_testbench_imports() -> BTreeSet<String> {
    let src =
        std::fs::read_to_string(common::zynqsim_root().join("fixtures/rtl/math/my_math_ip.v"))
            .expect("the math IP fixture");
    let module = sim_api::parse_module(&src, None).expect("parses");
    let axi = sim_api::detect_axi_lite(&module).expect("an AXI4-Lite slave");
    let text = sim_api::generate_testbench(&module, &axi);
    common::dpi_imports(&text)
}

/// Every DPI name a testbench imports: the fixtures and the generated one.
fn expected_imports() -> BTreeSet<String> {
    let mut all = common::dpi_imports_under(&common::zynqsim_root().join("fixtures"));
    all.extend(generated_testbench_imports());
    all
}

#[test]
fn the_expected_import_set_is_the_documented_abi() {
    // 31 names: 17 rp_*, 3 tel_*, 11 zs_*. A change here is an ABI change.
    let expected = expected_imports();
    assert_eq!(expected.len(), 31, "imports: {expected:?}");
    assert!(expected
        .iter()
        .all(|n| { n.starts_with("rp_") || n.starts_with("zs_") || n.starts_with("tel_") }));
    // The static archive exports these too (`cargo xtask dpi`), so the testbench
    // works unchanged with either.
    for name in ["rp_open", "rp_done_t", "zs_pin_xchg", "tel_ctrl"] {
        assert!(expected.contains(name), "{name} is not imported");
    }
}

#[test]
fn every_imported_name_resolves_in_the_shared_object() {
    let path = common::require_cdylib();
    // SAFETY: loading the library runs its initialisers only; no DPI function is called.
    let lib = unsafe { libloading::Library::new(&path) }.expect("the shared object loads");
    for name in expected_imports() {
        let symbol = format!("{name}\0");
        // SAFETY: only the address is taken, never called.
        let found = unsafe { lib.get::<*const ()>(symbol.as_bytes()) };
        assert!(
            found.is_ok(),
            "{name} is not exported by {}",
            path.display()
        );
    }
}

#[cfg(windows)]
#[test]
fn the_export_table_equals_the_imported_set() {
    let path = common::require_cdylib();
    let bytes = std::fs::read(&path).expect("readable shared object");
    let exported = common::pe_exports(&bytes);
    assert_eq!(
        exported,
        expected_imports(),
        "exports of {} differ from the testbench imports",
        path.display()
    );
}
