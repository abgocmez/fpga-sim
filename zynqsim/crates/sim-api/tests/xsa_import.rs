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
    let out = temp("dma");
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
    let plan = xsa::plan(&fixtures().join("xsa/zs_math_dma.hwh"), &out, &opts).unwrap();
    assert_eq!(plan.testbench_module.as_deref(), Some("my_math_ip"));
    assert_eq!(plan.unmodelled.len(), 1);
    let u = &plan.unmodelled[0];
    assert_eq!(
        (u.instance.as_str(), u.vlnv.as_str()),
        ("axi_dma_0", "xilinx.com:ip:axi_dma:7.1")
    );
    assert_eq!(u.base.as_deref(), Some("0x40400000"));
    assert_eq!(u.size, Some(0x10000));
    assert_eq!(u.irq, vec![0]);
    assert!(!plan.runnable_design());
    assert!(plan.project.native.is_empty());

    plan.write(&out).unwrap();
    let loaded = Project::load(&out.join("project.toml")).unwrap();
    let e = errors(&loaded, &out);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].0, "xsa.unmodelled.axi_dma_0");
    assert!(e[0].1.contains("does not exist yet"), "{e:?}");
    assert!(e[0].1.contains("0x40400000"));
    let err = loaded.build_engine(&out).err().expect("must refuse");
    assert!(err.message.contains("axi_dma_0"), "{}", err.message);
    assert!(plan.report.contains("NOT RUNNABLE"));
    assert!(plan.report.contains("`axi_dma_0`"));
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
