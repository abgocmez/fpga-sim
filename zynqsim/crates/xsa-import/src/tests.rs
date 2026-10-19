use super::*;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/xsa/zs_math_gpio.xsa")
}

#[test]
fn the_vivado_2022_1_fixture_is_read() {
    let h = read(&fixture()).expect("fixture");
    let d = &h.design;
    assert_eq!(d.name, "design_1");
    assert_eq!(d.vivado_version.as_deref(), Some("2022.1"));
    assert_eq!(d.device.as_deref(), Some("7z020"));
    assert_eq!(d.hwh_file.as_deref(), Some("design_1.hwh"));
    assert!(h.archive.ps7_init.iter().any(|n| n == "ps7_init.tcl"));
    assert_eq!(h.archive.ps7_init_ops.len(), 213);
    let ct = h.archive.clock_tree.as_ref().expect("clock tree");
    assert_eq!(ct.ps_clk_hz, 33_333_333);
    assert_eq!(ct.cpu_hz, 666_666_660);
    assert_eq!(ct.uart_hz, 99_999_999);
    assert!(!h.archive.has_bitstream);

    // PS7 configuration.
    let ps = d.ps7.as_ref().expect("ps7");
    assert_eq!(ps.instance, "processing_system7_0");
    assert!(ps.vlnv.starts_with("xilinx.com:ip:processing_system7:"));
    assert!(ps.fclk[0].enabled);
    assert_eq!(ps.fclk[0].hz(), Some(100_000_000));
    assert!(!ps.fclk[1].enabled);
    assert_eq!(ps.fclk[1].hz(), None);
    assert_eq!(ps.m_axi_gp, [true, false]);
    assert_eq!(ps.s_axi_hp, [false; 4]);
    assert!(!ps.s_axi_acp);
    assert!(ps.irq_f2p_enabled);
    assert_eq!(ps.irq_f2p_width, 1);
    let on = |n: &str| ps.peripherals.iter().find(|p| p.name == n).unwrap().clone();
    assert!(on("UART1").enabled);
    assert_eq!(on("UART1").io.as_deref(), Some("MIO 48 .. 49"));
    assert!(!on("UART0").enabled);
    assert!(on("ENET0").enabled);
    assert!(on("SD0").enabled);
    assert_eq!(on("SD0").io.as_deref(), Some("MIO 40 .. 45"));
    assert!(on("QSPI").enabled);
    assert_eq!(ps.mio.len(), 54);
    assert_eq!(ps.mio[48].peripheral, "UART 1");
    assert_eq!(ps.mio[48].signal, "tx");
    assert_eq!(ps.mio[1].peripheral, "Quad SPI Flash");

    // PL instances.
    let names: Vec<&str> = d.ips.iter().map(|i| i.instance.as_str()).collect();
    assert!(
        names.contains(&"u_math") && names.contains(&"axi_gpio_0"),
        "{names:?}"
    );
    let math = d.ip("u_math").unwrap();
    assert_eq!(math.vlnv, "xilinx.com:module_ref:my_math_ip:1.0");
    assert_eq!(math.modtype, "my_math_ip");
    assert_eq!(math.class, IpClass::UserRtl { packaged: false });
    assert_eq!(math.address.len(), 1);
    assert_eq!(math.address[0].base, 0x43C0_0000);
    assert_eq!(math.address[0].size(), 0x1000);
    assert_eq!(math.address[0].master_interface, "M_AXI_GP0");
    assert_eq!(math.address[0].interface, "S_AXI");
    let s_axi = math.interfaces.iter().find(|b| b.name == "S_AXI").unwrap();
    assert_eq!(s_axi.protocol.as_deref(), Some("AXI4LITE"));

    let gpio = d.ip("axi_gpio_0").unwrap();
    assert_eq!(gpio.vlnv, "xilinx.com:ip:axi_gpio:2.0");
    assert!(matches!(&gpio.class, IpClass::VendorIp { name, .. } if name == "axi_gpio"));
    assert_eq!(gpio.address[0].base, 0x4120_0000);
    assert_eq!(gpio.address[0].size(), 0x1_0000);
    assert_eq!(
        gpio.params.get("C_GPIO_WIDTH").map(String::as_str),
        Some("4")
    );

    // Infrastructure is classified, not reported as missing models.
    let xbar = d
        .ips
        .iter()
        .find(|i| i.modtype == "axi_interconnect")
        .unwrap();
    assert!(matches!(xbar.class, IpClass::Infrastructure { .. }));
    assert!(d.ips.iter().any(|i| i.modtype == "proc_sys_reset"));

    // Interrupt: axi_gpio_0.ip2intc_irpt -> IRQ_F2P[0], level high.
    assert_eq!(d.interrupts.len(), 1);
    let irq = &d.interrupts[0];
    assert_eq!(
        (irq.instance.as_str(), irq.port.as_str()),
        ("axi_gpio_0", "ip2intc_irpt")
    );
    assert_eq!((irq.bit, irq.width), (0, 1));
    assert_eq!(irq.sensitivity.as_deref(), Some("LEVEL_HIGH"));
    assert_eq!(IrqLink::gic_id(0), 61);

    // Clocks: both PL slaves run on FCLK_CLK0 at 100 MHz.
    let c = d.clocks_of("u_math").next().expect("clock");
    assert_eq!(c.port, "S_AXI_ACLK");
    assert_eq!(c.fclk, Some(0));
    assert_eq!(c.hz, Some(100_000_000));
    assert!(d.clocks_of("axi_gpio_0").any(|c| c.fclk == Some(0)));
}

const SYNTH: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<EDKSYSTEM EDWVERSION="1.2" VIVADOVERSION="2022.1">
  <SYSTEMINFO ARCH="zynq" DEVICE="7z020" NAME="synth"/>
  <MODULES>
    <MODULE INSTANCE="ps" MODTYPE="processing_system7" VLNV="xilinx.com:ip:processing_system7:5.5" FULLNAME="/ps">
      <PARAMETERS>
        <PARAMETER NAME="PCW_USE_M_AXI_GP1" VALUE="1"/>
        <PARAMETER NAME="PCW_USE_S_AXI_HP0" VALUE="1"/>
        <PARAMETER NAME="PCW_USE_S_AXI_HP3" VALUE="1"/>
        <PARAMETER NAME="PCW_USE_S_AXI_ACP" VALUE="1"/>
        <PARAMETER NAME="PCW_USE_FABRIC_INTERRUPT" VALUE="1"/>
        <PARAMETER NAME="PCW_IRQ_F2P_INTR" VALUE="1"/>
        <PARAMETER NAME="PCW_EN_CLK0_PORT" VALUE="1"/>
        <PARAMETER NAME="PCW_EN_CLK2_PORT" VALUE="1"/>
        <PARAMETER NAME="PCW_FPGA0_PERIPHERAL_FREQMHZ" VALUE="125"/>
        <PARAMETER NAME="PCW_ACT_FPGA0_PERIPHERAL_FREQMHZ" VALUE="125.000000"/>
        <PARAMETER NAME="PCW_FPGA2_PERIPHERAL_FREQMHZ" VALUE="50"/>
        <PARAMETER NAME="PCW_GPIO_EMIO_GPIO_ENABLE" VALUE="1"/>
        <PARAMETER NAME="PCW_GPIO_EMIO_GPIO_WIDTH" VALUE="8"/>
        <PARAMETER NAME="PCW_UART0_PERIPHERAL_ENABLE" VALUE="1"/>
        <PARAMETER NAME="PCW_UART0_UART0_IO" VALUE="EMIO"/>
      </PARAMETERS>
      <PORTS>
        <PORT DIR="I" LEFT="3" NAME="IRQ_F2P" RIGHT="0" SENSITIVITY="LEVEL_HIGH" SIGIS="INTERRUPT"/>
        <PORT CLKFREQUENCY="125000000" DIR="O" NAME="FCLK_CLK0" SIGIS="clk"/>
      </PORTS>
      <BUSINTERFACES>
        <BUSINTERFACE BUSNAME="b" DATAWIDTH="32" NAME="M_AXI_GP1" TYPE="MASTER"/>
      </BUSINTERFACES>
      <MEMORYMAP>
        <MEMRANGE ADDRESSBLOCK="Reg" BASEVALUE="0x83C00000" HIGHVALUE="0x83C0FFFF" INSTANCE="u_a" MASTERBUSINTERFACE="M_AXI_GP1" MEMTYPE="REGISTER" SLAVEBUSINTERFACE="s_axi"/>
      </MEMORYMAP>
    </MODULE>
    <MODULE INSTANCE="cat" MODTYPE="xlconcat" VLNV="xilinx.com:ip:xlconcat:2.1" FULLNAME="/cat">
      <PORTS>
        <PORT DIR="I" LEFT="0" NAME="In0" RIGHT="0" SIGIS="undef">
          <CONNECTIONS><CONNECTION INSTANCE="u_a" PORT="done_irq"/></CONNECTIONS>
        </PORT>
        <PORT DIR="I" LEFT="1" NAME="In1" RIGHT="0" SIGIS="undef">
          <CONNECTIONS><CONNECTION INSTANCE="u_b" PORT="irq_vec"/></CONNECTIONS>
        </PORT>
        <PORT DIR="O" LEFT="2" NAME="dout" RIGHT="0" SIGIS="undef">
          <CONNECTIONS><CONNECTION INSTANCE="ps" PORT="IRQ_F2P"/></CONNECTIONS>
        </PORT>
      </PORTS>
    </MODULE>
    <MODULE INSTANCE="u_a" MODTYPE="fancy" VLNV="user.org:user:fancy:1.0" FULLNAME="/u_a">
      <PORTS>
        <PORT CLKFREQUENCY="125000000" DIR="I" NAME="clk" SIGIS="clk">
          <CONNECTIONS><CONNECTION INSTANCE="ps" PORT="FCLK_CLK0"/></CONNECTIONS>
        </PORT>
        <PORT DIR="O" NAME="done_irq" SENSITIVITY="EDGE_RISING" SIGIS="INTERRUPT">
          <CONNECTIONS><CONNECTION INSTANCE="cat" PORT="In0"/></CONNECTIONS>
        </PORT>
      </PORTS>
    </MODULE>
    <MODULE INSTANCE="u_b" MODTYPE="axi_vdma" VLNV="xilinx.com:ip:axi_vdma:6.3" FULLNAME="/u_b">
      <PORTS>
        <PORT DIR="O" LEFT="1" NAME="irq_vec" RIGHT="0" SIGIS="INTERRUPT">
          <CONNECTIONS><CONNECTION INSTANCE="cat" PORT="In1"/></CONNECTIONS>
        </PORT>
      </PORTS>
      <MEMORYMAP>
        <MEMRANGE ADDRESSBLOCK="HP0_DDR_LOWOCM" BASEVALUE="0x00000000" HIGHVALUE="0x3FFFFFFF" INSTANCE="ps" MASTERBUSINTERFACE="M_AXI" MEMTYPE="MEMORY" SLAVEBUSINTERFACE="S_AXI_HP0"/>
      </MEMORYMAP>
    </MODULE>
    <MODULE INSTANCE="u_c" MODTYPE="rgb2dvi" VLNV="digilentinc.com:user:rgb2dvi:1.4" FULLNAME="/u_c"/>
    <MODULE INSTANCE="u_d" MODTYPE="mystery" VLNV="xilinx.com:ip:mystery:1.0" FULLNAME="/u_d"/>
  </MODULES>
</EDKSYSTEM>
"#;

#[test]
fn a_synthetic_handoff_covers_concat_hp_acp_emio_and_classes() {
    let d = parse_hwh(SYNTH).unwrap();
    let ps = d.ps7.as_ref().unwrap();
    assert_eq!(ps.m_axi_gp, [false, true]);
    assert_eq!(ps.s_axi_hp, [true, false, false, true]);
    assert!(ps.s_axi_acp);
    assert_eq!(ps.irq_f2p_width, 4);
    assert_eq!(ps.fclk[0].hz(), Some(125_000_000));
    assert_eq!(ps.fclk[2].hz(), Some(50_000_000));
    assert_eq!(ps.emio_gpio_width, Some(8));
    let uart0 = ps.peripherals.iter().find(|p| p.name == "UART0").unwrap();
    assert!(uart0.enabled);
    assert_eq!(uart0.io.as_deref(), Some("EMIO"));
    assert!(ps.mio.is_empty(), "no MIO tree in this handoff");

    // xlconcat: u_a.done_irq is bit 0, u_b.irq_vec bits 1..2.
    let by = |i: &str| d.interrupts.iter().find(|l| l.instance == i).unwrap();
    assert_eq!((by("u_a").bit, by("u_a").width), (0, 1));
    assert_eq!((by("u_b").bit, by("u_b").width), (1, 2));
    assert_eq!(by("u_a").via, vec!["cat".to_string()]);
    assert_eq!(by("u_a").sensitivity.as_deref(), Some("EDGE_RISING"));
    assert_eq!(IrqLink::gic_id(8), 84);
    assert_eq!(IrqLink::gic_id(15), 91);

    assert_eq!(
        d.ip("u_a").unwrap().class,
        IpClass::UserRtl { packaged: true }
    );
    assert!(matches!(
        d.ip("u_b").unwrap().class,
        IpClass::VendorIp { .. }
    ));
    assert_eq!(d.ip("u_c").unwrap().class, IpClass::ThirdParty);
    assert_eq!(d.ip("u_d").unwrap().class, IpClass::UnknownVendorIp);
    assert!(matches!(
        d.ip("cat").unwrap().class,
        IpClass::Infrastructure { .. }
    ));

    let a = d.ip("u_a").unwrap();
    assert_eq!(a.address.len(), 1);
    assert_eq!(a.address[0].base, 0x83C0_0000);
    assert_eq!(a.address[0].master_interface, "M_AXI_GP1");
    assert_eq!(d.memory_map.len(), 2);
    assert_eq!(d.memory_map[0].master_interface, "M_AXI_GP1");
    assert_eq!(
        d.ip("u_b").unwrap().master_windows[0].interface,
        "S_AXI_HP0"
    );
}

#[test]
fn a_bare_hwh_and_a_bom_are_accepted() {
    let with_bom = format!("\u{feff}{SYNTH}");
    let h = read_bytes(with_bom.as_bytes(), "x.hwh").unwrap();
    assert_eq!(h.design.name, "synth");
    assert!(h.archive.entries.is_empty());
}

#[test]
fn bad_inputs_are_explained() {
    let e = read_bytes(b"<foo/>", "a.hwh").unwrap_err();
    assert!(e.0.contains("expected <EDKSYSTEM>"), "{e}");
    let e = read_bytes(b"\xff\xfe not text", "a.bin").unwrap_err();
    assert!(e.0.contains("neither a zip"), "{e}");
    let e = read_bytes(b"<EDKSYSTEM><SYSTEMINFO/></EDKSYSTEM>", "a.hwh").unwrap_err();
    assert!(e.0.contains("no <MODULES>"), "{e}");
    let e = read_bytes(b"PK\x03\x04garbage", "a.xsa").unwrap_err();
    assert!(e.0.contains("zip"), "{e}");
    let e = read(std::path::Path::new("definitely/missing.xsa")).unwrap_err();
    assert!(e.0.contains("cannot read"), "{e}");
}

#[test]
fn a_zip_without_a_hwh_says_how_to_fix_it() {
    // A stored zip with one file: xsa.xml.
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(&mut buf);
        w.start_file(
            "xsa.xml",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        w.write_all(b"<Root/>").unwrap();
        w.finish().unwrap();
    }
    let e = read_bytes(buf.get_ref(), "empty.xsa").unwrap_err();
    assert!(
        e.0.contains("no .hwh") && e.0.contains("generate_target"),
        "{e}"
    );
}

#[test]
fn archive_sources_and_filelists_are_collected() {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (n, c) in [
            ("d.hwh", SYNTH),
            ("src/top.v", "module top; endmodule"),
            ("src/pkg.sv", "package p; endpackage"),
            (
                "sim/vlog.prj",
                "verilog xil_defaultlib \"../src/top.v\"\nsv work \"x.sv\" -i inc\n",
            ),
            ("ps7_init.tcl", "# init"),
            ("d.bit", "bits"),
        ] {
            w.start_file(n, o).unwrap();
            w.write_all(c.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }
    let h = read_bytes(buf.get_ref(), "s.xsa").unwrap();
    let names: Vec<&str> = h.archive.sources.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(names, ["src/top.v", "src/pkg.sv", "sim/vlog.prj"]);
    assert_eq!(h.archive.sources[0].kind, SourceKind::Verilog);
    assert_eq!(h.archive.sources[2].kind, SourceKind::FileList);
    assert!(h.archive.has_bitstream);
    assert_eq!(h.archive.ps7_init, ["ps7_init.tcl"]);
    let list = parse_filelist(&h.archive.sources[2].text);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].path, "../src/top.v");
    assert_eq!(list[0].language.as_deref(), Some("verilog"));
    assert_eq!(list[0].library.as_deref(), Some("xil_defaultlib"));
    assert_eq!(list[1].path, "x.sv");
}

#[test]
fn plain_file_lists_skip_options_and_comments() {
    let l = parse_filelist(
        "// header\n+incdir+inc\n-v lib.v\nrtl/a.v rtl/b.sv\n# c\n\"has space.v\"\n",
    );
    let p: Vec<&str> = l.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(p, ["rtl/a.v", "rtl/b.sv", "has space.v"]);
}

#[test]
fn zip_entries_with_parent_components_are_refused() {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("../evil.v", o).unwrap();
        w.write_all(b"module e; endmodule").unwrap();
        w.finish().unwrap();
    }
    let e = read_bytes(buf.get_ref(), "evil.xsa").unwrap_err();
    assert!(e.0.contains("unsafe path"), "{e}");
}

#[test]
fn catalog_classification() {
    use IpClass::*;
    assert_eq!(
        classify("xilinx.com:ip:processing_system7:5.5", "processing_system7"),
        Ps7
    );
    assert!(matches!(
        classify("xilinx.com:ip:axi_timer:2.0", "axi_timer"),
        VendorIp { .. }
    ));
    assert!(matches!(
        classify("xilinx.com:ip:axi_quad_spi:3.2", "axi_quad_spi"),
        VendorIp { ref name, .. } if name == "axi_quad_spi"
    ));
    assert!(matches!(
        classify("xilinx.com:ip:smartconnect:1.0", "smartconnect"),
        Infrastructure { .. }
    ));

    assert_eq!(
        classify("xilinx.com:module_ref:foo:1.0", "foo"),
        UserRtl { packaged: false }
    );
    assert_eq!(classify("acme.com:ip:foo:1.0", "foo"), ThirdParty);
}
