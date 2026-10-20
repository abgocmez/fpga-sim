//! `sim_api::xsa::plan`: handoff to project mapping (no external tools needed).

use sim_api::xsa::{self, ImportOptions};
use sim_api::{Project, Severity};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-xsa-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn errors(p: &Project, base: &std::path::Path) -> Vec<(String, String)> {
    p.validate(base)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.field, d.message))
        .collect()
}

#[test]
fn the_gpio_design_maps_the_user_ip_and_places_the_gpio_model_next_to_it() {
    let out = temp("map");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_gpio.xsa"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    // The AXI GPIO becomes a vendor-ip model with the block design's parameters, next to the RTL.
    assert_eq!(plan.project.native["axi_gpio_0"], "axi_gpio");
    let np = &plan.project.native_params["axi_gpio_0"];
    assert_eq!(np["C_GPIO_WIDTH"].to_string(), "4");
    assert_eq!(np["C_ALL_OUTPUTS"].to_string(), "1");
    assert_eq!(np["C_INTERRUPT_PRESENT"].to_string(), "1");
    assert_eq!(np["ADDRESS_SIZE"].to_string(), "0x10000");
    assert_eq!(np["CLK_FREQ_HZ"].to_string(), "100000000");
    assert_eq!(plan.project.native_irq("axi_gpio_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("axi_gpio_0"),
        Some((0, 0x4120_0000))
    );
    // Four output bits: guessed to be the board LEDs, and the report says it is a guess.
    assert_eq!(plan.project.connect["axi_gpio_0.GPIO"], "board.leds");
    assert!(
        plan.warnings.iter().any(|w| w.contains("guess")),
        "{:?}",
        plan.warnings
    );

    // The generated project text round-trips and carries the maps.
    let p = Project::from_toml(&plan.project_toml).unwrap();
    assert_eq!(p, plan.project);
    assert_eq!(p.backends.pl, "xsim");
    let c = &p.connect;
    assert_eq!(c["u_math.S_AXI"], "M_AXI_GP0@0x43C00000");
    assert_eq!(c["axi_gpio_0.S_AXI"], "M_AXI_GP0@0x41200000");
    assert_eq!(c["axi_gpio_0.ip2intc_irpt"], "IRQ_F2P[0]");
    assert_eq!(c["u_math.S_AXI_ACLK"], "FCLK_CLK0");
    assert!(
        !c.keys().any(|k| k.starts_with("ps7_0_axi_periph")),
        "interconnect is wiring: {c:?}"
    );

    // Written to disk it validates clean (the ELF exists) and keeps the native model.
    plan.write(&out).unwrap();
    let loaded = Project::load(&out.join("project.toml")).unwrap();
    assert_eq!(loaded, plan.project);
    let e = errors(&loaded, &out);
    assert!(e.is_empty(), "{e:?}");

    // The report names the model and says what was and was not verified.
    assert!(!plan.report.contains("NOT RUNNABLE"));
    assert!(plan.report.contains("vendor-ip model `axi_gpio`"));
    assert!(plan.report.contains("PS7"));
    assert!(plan.report.contains("MIO use"));
    assert!(plan.report.contains("UART 1"));
    assert!(plan.report.contains("ps7_init.tcl"));
}

#[test]
fn an_ip_without_a_model_is_recorded_and_stops_the_run() {
    let out = temp("hwicap");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_hwicap.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert_eq!(plan.unmodelled.len(), 1);
    let u = &plan.unmodelled[0];
    assert_eq!(
        (u.instance.as_str(), u.vlnv.as_str()),
        ("axi_hwicap_0", "xilinx.com:ip:axi_hwicap:3.0")
    );
    assert_eq!(u.base.as_deref(), Some("0x43000000"));
    assert_eq!(u.size, Some(0x10000));
    assert_eq!(u.irq, vec![0]);
    assert!(!plan.runnable_design());
    assert!(plan.project.native.is_empty());

    plan.write(&out).unwrap();
    let loaded = Project::load(&out.join("project.toml")).unwrap();
    let e = errors(&loaded, &out);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].0, "xsa.unmodelled.axi_hwicap_0");
    assert!(e[0].1.contains("does not exist yet"), "{e:?}");
    assert!(e[0].1.contains("0x43000000"));
    let err = loaded.build_engine(&out).err().expect("must refuse");
    assert!(err.message.contains("axi_hwicap_0"), "{}", err.message);
    assert!(plan.report.contains("NOT RUNNABLE"));
    assert!(plan.report.contains("`axi_hwicap_0`"));
}

#[test]
fn the_uart16550_design_maps_the_uart16550_native_model() {
    let out = temp("uart16550_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_uart16550.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["axi_uart16550_0"], "axi_uart16550");
    assert_eq!(plan.project.native_irq("axi_uart16550_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("axi_uart16550_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_vid_out_design_maps_the_vid_out_native_model() {
    let out = temp("vid_out_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_vid_out.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["v_axi4s_vid_out_0"], "v_axi4s_vid_out");
    assert_eq!(plan.project.native_irq("v_axi4s_vid_out_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("v_axi4s_vid_out_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_vid_in_design_maps_the_vid_in_native_model() {
    let out = temp("vid_in_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_vid_in.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["v_vid_in_axi4s_0"], "v_vid_in_axi4s");
    assert_eq!(plan.project.native_irq("v_vid_in_axi4s_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("v_vid_in_axi4s_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_ila_design_maps_the_ila_native_model() {
    let out = temp("ila_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_ila.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["ila_0"], "ila");
    assert_eq!(plan.project.native_irq("ila_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("ila_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_vio_design_maps_the_vio_native_model() {
    let out = temp("vio_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_vio.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["vio_0"], "vio");
    assert_eq!(plan.project.native_irq("vio_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("vio_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_xadc_design_maps_the_xadc_native_model() {
    let out = temp("xadc_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_xadc.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["xadc_wiz_0"], "xadc_wiz");
    assert_eq!(plan.project.native_irq("xadc_wiz_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("xadc_wiz_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_vdma_design_maps_the_vdma_native_model() {
    let out = temp("vdma_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_vdma.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["axi_vdma_0"], "axi_vdma");
    assert_eq!(plan.project.native_irq("axi_vdma_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("axi_vdma_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_ethlite_design_maps_the_ethlite_native_model() {
    let out = temp("ethlite_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_ethlite.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(
        plan.project.native["axi_ethernetlite_0"],
        "axi_ethernetlite"
    );
    assert_eq!(plan.project.native_irq("axi_ethernetlite_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("axi_ethernetlite_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_vtc_design_maps_the_vtc_native_model() {
    let out = temp("vtc_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_vtc.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["v_tc_0"], "v_tc");
    assert_eq!(plan.project.native_irq("v_tc_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("v_tc_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn the_cdma_design_maps_the_cdma_native_model() {
    let out = temp("cdma_native");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_cdma.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert!(plan.runnable_design());
    assert_eq!(plan.project.native["axi_cdma_0"], "axi_cdma");
    assert_eq!(plan.project.native_irq("axi_cdma_0"), Some(0));
    assert_eq!(
        plan.project.native_placement("axi_cdma_0"),
        Some((0, 0x4300_0000))
    );
}

#[test]
fn a_user_rtl_only_design_validates_clean() {
    let out = temp("clean");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some(
            fixtures()
                .join("elf/math_test.elf")
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_only.xsa"), &out, &opts).unwrap();
    assert!(plan.runnable_design(), "{:?}", plan.unmodelled);
    plan.write(&out).unwrap();
    let loaded = Project::load(&out.join("project.toml")).unwrap();
    let e = errors(&loaded, &out);
    assert!(e.is_empty(), "{e:?}");
    assert!(plan.report.contains("not** a claim") || plan.report.contains("not a claim"));
    assert!(!plan.report.contains("NOT RUNNABLE"));
}

#[test]
fn a_missing_rtl_source_is_unmodelled_with_the_flag_to_pass_it() {
    let out = temp("nosrc");
    let plan = xsa::plan(
        &fixtures().join("xsa/zs_math_only.xsa"),
        &out,
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.unmodelled.len(), 1);
    assert_eq!(plan.unmodelled[0].instance, "u_math");
    assert!(plan.unmodelled[0].reason.contains("--rtl"));
    assert_eq!(plan.project.backends.pl, "demo");
    assert!(plan.project.rtl.is_none());
}

/// A hand-made handoff in Vivado's schema: an interrupt on bit 3 through an xlconcat and a
/// 125 MHz clock must reach the testbench.
#[test]
fn irq_bit_and_clock_frequency_reach_the_testbench() {
    let hwh = r#"<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<EDKSYSTEM EDWVERSION="1.2" VIVADOVERSION="2022.1">
  <SYSTEMINFO ARCH="zynq" DEVICE="7z020" NAME="hand"/>
  <MODULES>
    <MODULE INSTANCE="ps" MODTYPE="processing_system7" VLNV="xilinx.com:ip:processing_system7:5.5">
      <PARAMETERS>
        <PARAMETER NAME="PCW_USE_M_AXI_GP0" VALUE="1"/>
        <PARAMETER NAME="PCW_USE_FABRIC_INTERRUPT" VALUE="1"/>
        <PARAMETER NAME="PCW_IRQ_F2P_INTR" VALUE="1"/>
        <PARAMETER NAME="PCW_EN_CLK0_PORT" VALUE="1"/>
        <PARAMETER NAME="PCW_FPGA0_PERIPHERAL_FREQMHZ" VALUE="125"/>
      </PARAMETERS>
      <PORTS>
        <PORT DIR="I" LEFT="3" NAME="IRQ_F2P" RIGHT="0" SIGIS="INTERRUPT"/>
        <PORT CLKFREQUENCY="125000000" DIR="O" NAME="FCLK_CLK0" SIGIS="clk"/>
      </PORTS>
      <MEMORYMAP>
        <MEMRANGE ADDRESSBLOCK="reg0" BASEVALUE="0x43C10000" HIGHVALUE="0x43C10FFF" INSTANCE="u_irq" MASTERBUSINTERFACE="M_AXI_GP0" MEMTYPE="REGISTER" SLAVEBUSINTERFACE="S_AXI"/>
      </MEMORYMAP>
    </MODULE>
    <MODULE INSTANCE="cat" MODTYPE="xlconcat" VLNV="xilinx.com:ip:xlconcat:2.1">
      <PORTS>
        <PORT DIR="I" NAME="In0" SIGIS="undef"/>
        <PORT DIR="I" NAME="In1" SIGIS="undef"/>
        <PORT DIR="I" NAME="In2" SIGIS="undef"/>
        <PORT DIR="I" NAME="In3" SIGIS="undef">
          <CONNECTIONS><CONNECTION INSTANCE="u_irq" PORT="irq"/></CONNECTIONS>
        </PORT>
        <PORT DIR="O" LEFT="3" NAME="dout" RIGHT="0" SIGIS="undef">
          <CONNECTIONS><CONNECTION INSTANCE="ps" PORT="IRQ_F2P"/></CONNECTIONS>
        </PORT>
      </PORTS>
    </MODULE>
    <MODULE INSTANCE="u_irq" MODTYPE="irq_ip" VLNV="xilinx.com:module_ref:irq_ip:1.0">
      <PORTS>
        <PORT CLKFREQUENCY="125000000" DIR="I" NAME="S_AXI_ACLK" SIGIS="clk">
          <CONNECTIONS><CONNECTION INSTANCE="ps" PORT="FCLK_CLK0"/></CONNECTIONS>
        </PORT>
        <PORT DIR="O" NAME="irq" SENSITIVITY="LEVEL_HIGH" SIGIS="INTERRUPT">
          <CONNECTIONS><CONNECTION INSTANCE="cat" PORT="In3"/></CONNECTIONS>
        </PORT>
      </PORTS>
    </MODULE>
  </MODULES>
</EDKSYSTEM>
"#;
    let dir = temp("hand");
    let hwh_path = dir.join("hand.hwh");
    std::fs::write(&hwh_path, hwh).unwrap();
    let out = dir.join("proj");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/irq/irq_ip.v")],
        ..Default::default()
    };
    let plan = xsa::plan(&hwh_path, &out, &opts).unwrap();
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    let tb = String::from_utf8(
        plan.files
            .iter()
            .find(|(n, _)| n == "tb/cosim_top.sv")
            .unwrap()
            .1
            .clone(),
    )
    .unwrap();
    assert!(tb.contains("-> IRQ_F2P[3]"), "{tb}");
    assert!(tb.contains("rp_irq($time, 3, irq ? 1 : 0)"), "{tb}");
    assert!(tb.contains("always #4 aclk = ~aclk;   // 125 MHz"), "{tb}");
    assert_eq!(plan.project.connect["u_irq.irq"], "IRQ_F2P[3]");
    assert_eq!(plan.project.connect["u_irq.S_AXI"], "M_AXI_GP0@0x43C10000");
    assert!(
        plan.report.contains("GIC 64"),
        "bit 3 is GIC ID 64: {}",
        plan.report
    );
}

#[test]
fn designs_without_a_ps7_are_refused() {
    let dir = temp("nops");
    let p = dir.join("x.hwh");
    std::fs::write(
        &p,
        r#"<EDKSYSTEM><SYSTEMINFO NAME="x"/><MODULES>
           <MODULE INSTANCE="a" MODTYPE="axi_gpio" VLNV="xilinx.com:ip:axi_gpio:2.0"/>
           </MODULES></EDKSYSTEM>"#,
    )
    .unwrap();
    let e = xsa::plan(&p, &dir.join("o"), &ImportOptions::default())
        .err()
        .unwrap();
    assert!(e.message.contains("processing_system7"), "{}", e.message);
}

/// `--boot-bin` next to `--xsa` (ADR-019): one plan gives the XSA-derived PL mapping AND a
/// `[boot] mode = "boot_bin"` project; the image is validated and copied, `cpu0` is gone.
#[test]
fn a_boot_bin_next_to_an_xsa_gives_one_project_with_both() {
    let out = temp("bootbin");
    let bin = fixtures().join("boot/BOOT.BIN");
    let opts = ImportOptions {
        rtl: vec![fixtures().join("rtl/math/my_math_ip.v")],
        elf: Some("ignored.elf".to_string()),
        boot_bin: Some(bin.clone()),
        ..Default::default()
    };
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_gpio.xsa"), &out, &opts).unwrap();
    let boot = plan.project.boot.as_ref().unwrap();
    assert_eq!(boot.mode, "boot_bin");
    assert_eq!(boot.file.as_deref(), Some("BOOT.BIN"));
    assert!(plan.project.cpu0.is_none());
    // The PL mapping is the same as without the image.
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert_eq!(plan.project.native["axi_gpio_0"], "axi_gpio");
    assert!(
        !plan.project_toml.contains("[cpu0]"),
        "{}",
        plan.project_toml
    );
    assert!(
        plan.warnings.iter().any(|w| w.contains("--elf is ignored")),
        "{:?}",
        plan.warnings
    );
    assert!(plan.report.contains("boot_bin"), "{}", plan.report);
    assert!(plan.report.contains("not executed"), "{}", plan.report);
    assert!(!plan.report.contains("boots the ELF"), "{}", plan.report);

    plan.write(&out).unwrap();
    assert_eq!(
        std::fs::read(out.join("BOOT.BIN")).unwrap(),
        std::fs::read(&bin).unwrap()
    );
    let loaded = Project::load(&out.join("project.toml")).unwrap();
    assert_eq!(loaded, plan.project);
    let e = errors(&loaded, &out);
    assert!(e.is_empty(), "{e:?}");

    // A damaged image creates no plan at all.
    let mut bad = std::fs::read(&bin).unwrap();
    bad[0x44] ^= 1;
    let bad_path = out.join("bad.bin");
    std::fs::write(&bad_path, bad).unwrap();
    let err = xsa::plan(
        &fixtures().join("xsa/zs_math_gpio.xsa"),
        &out.join("q"),
        &ImportOptions {
            boot_bin: Some(bad_path),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.message.contains("checksum"), "{}", err.message);
}

#[test]
fn a_clock_wizard_with_an_axi_lite_window_is_mapped_as_a_native_model() {
    let dir = temp("clkwiz_plan");
    let hwh = dir.join("design.hwh");
    std::fs::write(
        &hwh,
        r#"<EDKSYSTEM EDWVERSION="1.2" VIVADOVERSION="2022.1">
  <SYSTEMINFO ARCH="zynq" DEVICE="7z020" NAME="synth"/>
  <MODULES>
    <MODULE INSTANCE="processing_system7_0" MODTYPE="processing_system7" VLNV="xilinx.com:ip:processing_system7:5.5">
      <PORTS>
        <PORT CLKFREQUENCY="100000000" DIR="O" NAME="FCLK_CLK0" SIGIS="clk"/>
      </PORTS>
      <BUSINTERFACES>
        <BUSINTERFACE BUSNAME="ps_gp0" DATAWIDTH="32" NAME="M_AXI_GP0" TYPE="MASTER"/>
      </BUSINTERFACES>
      <MEMORYMAP>
        <MEMRANGE ADDRESSBLOCK="Reg" BASEVALUE="0x43C00000" HIGHVALUE="0x43C0FFFF" INSTANCE="clk_wiz_0" MASTERBUSINTERFACE="M_AXI_GP0" MEMTYPE="REGISTER" SLAVEBUSINTERFACE="S_AXI_LITE"/>
      </MEMORYMAP>
    </MODULE>
    <MODULE INSTANCE="clk_wiz_0" MODTYPE="clk_wiz" VLNV="xilinx.com:ip:clk_wiz:6.0">
      <PARAMETERS>
        <PARAMETER NAME="PRIM_IN_FREQ" VALUE="125.000"/>
      </PARAMETERS>
      <BUSINTERFACES>
        <BUSINTERFACE BUSNAME="ps_gp0" DATAWIDTH="32" NAME="S_AXI_LITE" TYPE="SLAVE"/>
      </BUSINTERFACES>
    </MODULE>
  </MODULES>
</EDKSYSTEM>"#,
    )
    .unwrap();

    let out = dir.join("proj");
    let opts = ImportOptions {
        elf: Some("dummy.elf".to_string()),
        ..Default::default()
    };
    let plan = xsa::plan(&hwh, &out, &opts).unwrap();
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert_eq!(plan.project.native["clk_wiz_0"], "clk_wiz");
    assert_eq!(
        plan.project.native_placement("clk_wiz_0"),
        Some((0, 0x43C0_0000))
    );
}

#[test]
fn an_axi_quad_spi_instance_is_mapped_as_a_native_model() {
    let dir = temp("qspi_plan");
    let hwh = dir.join("design.hwh");
    std::fs::write(
        &hwh,
        r#"<EDKSYSTEM EDWVERSION="1.2" VIVADOVERSION="2022.1">
  <SYSTEMINFO ARCH="zynq" DEVICE="7z020" NAME="synth"/>
  <MODULES>
    <MODULE INSTANCE="processing_system7_0" MODTYPE="processing_system7" VLNV="xilinx.com:ip:processing_system7:5.5">
      <PORTS>
        <PORT CLKFREQUENCY="100000000" DIR="O" NAME="FCLK_CLK0" SIGIS="clk"/>
      </PORTS>
      <BUSINTERFACES>
        <BUSINTERFACE BUSNAME="ps_gp0" DATAWIDTH="32" NAME="M_AXI_GP0" TYPE="MASTER"/>
      </BUSINTERFACES>
      <MEMORYMAP>
        <MEMRANGE ADDRESSBLOCK="Reg" BASEVALUE="0x41E00000" HIGHVALUE="0x41E0FFFF" INSTANCE="axi_quad_spi_0" MASTERBUSINTERFACE="M_AXI_GP0" MEMTYPE="REGISTER" SLAVEBUSINTERFACE="AXI_LITE"/>
      </MEMORYMAP>
    </MODULE>
    <MODULE INSTANCE="axi_quad_spi_0" MODTYPE="axi_quad_spi" VLNV="xilinx.com:ip:axi_quad_spi:3.2">
      <PARAMETERS>
        <PARAMETER NAME="C_FIFO_DEPTH" VALUE="16"/>
        <PARAMETER NAME="C_NUM_SS_BITS" VALUE="1"/>
      </PARAMETERS>
      <PORTS>
        <PORT CLKFREQUENCY="100000000" DIR="I" NAME="ext_spi_clk" SIGIS="clk"/>
      </PORTS>
      <BUSINTERFACES>
        <BUSINTERFACE BUSNAME="ps_gp0" DATAWIDTH="32" NAME="AXI_LITE" TYPE="SLAVE"/>
      </BUSINTERFACES>
    </MODULE>
  </MODULES>
</EDKSYSTEM>"#,
    )
    .unwrap();

    let out = dir.join("proj");
    let opts = ImportOptions {
        elf: Some("dummy.elf".to_string()),
        ..Default::default()
    };
    let plan = xsa::plan(&hwh, &out, &opts).unwrap();
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert_eq!(plan.project.native["axi_quad_spi_0"], "axi_quad_spi");
    assert_eq!(
        plan.project.native_placement("axi_quad_spi_0"),
        Some((0, 0x41E0_0000))
    );
    let params = &plan.project.native_params["axi_quad_spi_0"];
    assert_eq!(params["C_FIFO_DEPTH"].to_string(), "16");
    assert_eq!(params["C_NUM_SS_BITS"].to_string(), "1");
}

/// A block design with the processing system and one AXI4-Stream FIFO. `extra_port` is an
/// optional `<PORT>` element for the FIFO (its stream wiring).
fn axi_fifo_hwh(tag: &str, extra_port: &str) -> PathBuf {
    let dir = temp(tag);
    let hwh = dir.join("design.hwh");
    std::fs::write(
        &hwh,
        format!(
            r#"<EDKSYSTEM EDWVERSION="1.2" VIVADOVERSION="2022.1">
  <SYSTEMINFO ARCH="zynq" DEVICE="7z020" NAME="synth"/>
  <MODULES>
    <MODULE INSTANCE="processing_system7_0" MODTYPE="processing_system7" VLNV="xilinx.com:ip:processing_system7:5.5">
      <PORTS>
        <PORT CLKFREQUENCY="100000000" DIR="O" NAME="FCLK_CLK0" SIGIS="clk"/>
      </PORTS>
      <BUSINTERFACES>
        <BUSINTERFACE BUSNAME="ps_gp0" DATAWIDTH="32" NAME="M_AXI_GP0" TYPE="MASTER"/>
      </BUSINTERFACES>
      <MEMORYMAP>
        <MEMRANGE ADDRESSBLOCK="Reg" BASEVALUE="0x43C00000" HIGHVALUE="0x43C0FFFF" INSTANCE="axi_fifo_mm_s_0" MASTERBUSINTERFACE="M_AXI_GP0" MEMTYPE="REGISTER" SLAVEBUSINTERFACE="S_AXI"/>
      </MEMORYMAP>
    </MODULE>
    <MODULE INSTANCE="axi_fifo_mm_s_0" MODTYPE="axi_fifo_mm_s" VLNV="xilinx.com:ip:axi_fifo_mm_s:4.2">
      <PARAMETERS>
        <PARAMETER NAME="C_TX_FIFO_DEPTH" VALUE="512"/>
        <PARAMETER NAME="C_RX_FIFO_DEPTH" VALUE="512"/>
      </PARAMETERS>
      <PORTS>
        <PORT CLKFREQUENCY="100000000" DIR="I" NAME="s_axi_aclk" SIGIS="clk"/>
        <PORT DIR="O" NAME="interrupt"/>
        {extra_port}
      </PORTS>
      <BUSINTERFACES>
        <BUSINTERFACE BUSNAME="ps_gp0" DATAWIDTH="32" NAME="S_AXI" TYPE="SLAVE"/>
      </BUSINTERFACES>
    </MODULE>
    <MODULE INSTANCE="xlconstant_0" MODTYPE="xlconstant" VLNV="xilinx.com:ip:xlconstant:1.1">
      <PORTS>
        <PORT DIR="O" NAME="dout"/>
      </PORTS>
    </MODULE>
  </MODULES>
</EDKSYSTEM>"#
        ),
    )
    .unwrap();
    hwh
}

#[test]
fn an_axi_fifo_mm_s_instance_is_mapped_as_a_native_model() {
    let hwh = axi_fifo_hwh("fifo_plan", "");
    let out = hwh.parent().unwrap().join("proj");
    let opts = ImportOptions {
        elf: Some("dummy.elf".to_string()),
        ..Default::default()
    };
    let plan = xsa::plan(&hwh, &out, &opts).unwrap();
    assert!(plan.unmodelled.is_empty(), "{:?}", plan.unmodelled);
    assert_eq!(plan.project.native["axi_fifo_mm_s_0"], "axi_fifo_mm_s");
    assert_eq!(
        plan.project.native_placement("axi_fifo_mm_s_0"),
        Some((0, 0x43C0_0000))
    );
    let params = &plan.project.native_params["axi_fifo_mm_s_0"];
    assert_eq!(params["C_TX_FIFO_DEPTH"].to_string(), "512");
    assert_eq!(params["C_RX_FIFO_DEPTH"].to_string(), "512");
}

#[test]
fn an_axi_fifo_whose_stream_is_wired_to_another_block_is_unmodelled() {
    let wired = r#"<PORT DIR="O" NAME="M_AXIS_TDATA"><CONNECTIONS><CONNECTION INSTANCE="xlconstant_0" PORT="dout"/></CONNECTIONS></PORT>"#;
    let hwh = axi_fifo_hwh("fifo_wired", wired);
    let out = hwh.parent().unwrap().join("proj");
    let opts = ImportOptions {
        elf: Some("dummy.elf".to_string()),
        ..Default::default()
    };
    let plan = xsa::plan(&hwh, &out, &opts).unwrap();
    let text = format!("{:?}", plan.unmodelled);
    assert!(text.contains("xlconstant_0.dout"), "{text}");
    assert!(!plan.project.native.contains_key("axi_fifo_mm_s_0"));
}
