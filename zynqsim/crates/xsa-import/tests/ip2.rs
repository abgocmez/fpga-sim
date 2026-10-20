//! The Vivado 2022.1 handoffs of the AXI DMA, interrupt controller, IIC and BRAM controller
//! designs (`fixtures/xsa/make_*_xsa.tcl`) and the catalog's passive IP.

use std::path::PathBuf;
use xsa_import::{IpClass, IpInstance, classify, read};

fn fx(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/xsa")
        .join(name)
}

#[test]
fn the_dma_fixture_has_its_hp_window_stream_loopback_and_two_interrupts() {
    let d = read(&fx("zs_dma_loopback.xsa")).expect("fixture").design;
    let ps = d.ps7.as_ref().unwrap();
    assert_eq!(ps.s_axi_hp, [true, false, false, false]);
    let dma = d.ip("axi_dma_0").unwrap();
    assert_eq!(dma.vlnv, "xilinx.com:ip:axi_dma:7.1");
    assert!(matches!(&dma.class, IpClass::VendorIp { name, .. } if name == "axi_dma"));
    assert_eq!(dma.address[0].base, 0x4040_0000);
    assert_eq!(dma.address[0].size(), 0x1_0000);
    assert_eq!(dma.params["C_INCLUDE_SG"], "0");
    assert_eq!(dma.params["C_SG_LENGTH_WIDTH"], "23");
    // The memory masters reach DDR through S_AXI_HP0.
    let hp: Vec<(&str, &str)> = dma
        .master_windows
        .iter()
        .map(|w| (w.master_interface.as_str(), w.interface.as_str()))
        .collect();
    assert_eq!(
        hp,
        vec![("M_AXI_MM2S", "S_AXI_HP0"), ("M_AXI_S2MM", "S_AXI_HP0")]
    );
    assert_eq!(dma.master_windows[0].instance, "processing_system7_0");
    assert_eq!(
        (dma.master_windows[0].base, dma.master_windows[0].high),
        (0, 0x3FFF_FFFF)
    );
    // The stream interfaces share one bus: a direct MM2S to S2MM loopback.
    let bus = |n: &str| {
        dma.interfaces
            .iter()
            .find(|i| i.name == n)
            .and_then(|i| i.bus_name.clone())
    };
    assert!(bus("M_AXIS_MM2S").is_some());
    assert_eq!(bus("M_AXIS_MM2S"), bus("S_AXIS_S2MM"));
    // Two interrupt outputs, through an xlconcat.
    let irq: Vec<(&str, u32)> = d
        .interrupts
        .iter()
        .map(|l| (l.port.as_str(), l.bit))
        .collect();
    assert_eq!(irq, vec![("mm2s_introut", 0), ("s2mm_introut", 1)]);
    assert!(d.intc_inputs.is_empty());
}

#[test]
fn the_fifo_variant_puts_stream_plumbing_between_the_two_stream_interfaces() {
    let d = read(&fx("zs_dma_fifo.xsa")).expect("fixture").design;
    let dma = d.ip("axi_dma_0").unwrap();
    let fifo = d.ip("axis_data_fifo_0").unwrap();
    assert!(
        matches!(fifo.class, IpClass::Infrastructure { .. }),
        "{:?}",
        fifo.class
    );
    let bus = |ip: &IpInstance, n: &str| {
        ip.interfaces
            .iter()
            .find(|i| i.name == n)
            .and_then(|i| i.bus_name.clone())
    };
    assert_ne!(bus(dma, "M_AXIS_MM2S"), bus(dma, "S_AXIS_S2MM"));
    assert_eq!(bus(dma, "M_AXIS_MM2S"), bus(fifo, "S_AXIS"));
    assert_eq!(bus(fifo, "M_AXIS"), bus(dma, "S_AXIS_S2MM"));
}

#[test]
fn the_intc_fixture_traces_interrupt_sources_through_the_concat_to_controller_inputs() {
    let d = read(&fx("zs_intc.xsa")).expect("fixture").design;
    let intc = d.ip("axi_intc_0").unwrap();
    assert_eq!(intc.vlnv, "xilinx.com:ip:axi_intc:4.1");
    assert_eq!(intc.address[0].base, 0x4180_0000);
    assert_eq!(intc.params["C_NUM_INTR_INPUTS"], "2");
    assert_eq!(
        intc.params["C_KIND_OF_INTR"].to_ascii_lowercase(),
        "0xfffffffc"
    );
    let inputs: Vec<(&str, &str, u32, &[String])> = d
        .intc_inputs
        .iter()
        .map(|l| {
            (
                l.link.instance.as_str(),
                l.link.port.as_str(),
                l.link.bit,
                l.link.via.as_slice(),
            )
        })
        .collect();
    let via = ["xlconcat_0".to_string()];
    assert_eq!(
        inputs,
        vec![
            ("axi_gpio_0", "ip2intc_irpt", 0, &via[..]),
            ("axi_timer_0", "interrupt", 1, &via[..]),
        ]
    );
    assert!(d.intc_inputs.iter().all(|l| l.intc == "axi_intc_0"));
    assert_eq!(d.intc_inputs_of("axi_timer_0").count(), 1);
    // Only the controller's own output reaches the PS.
    let ps: Vec<(&str, &str, u32)> = d
        .interrupts
        .iter()
        .map(|l| (l.instance.as_str(), l.port.as_str(), l.bit))
        .collect();
    assert_eq!(ps, vec![("axi_intc_0", "irq", 0)]);
}

#[test]
fn the_iic_bram_fixture_has_the_controller_and_its_block_memory() {
    let d = read(&fx("zs_iic_bram.xsa")).expect("fixture").design;
    let iic = d.ip("axi_iic_0").unwrap();
    assert_eq!(iic.address[0].base, 0x4160_0000);
    let bram = d.ip("axi_bram_ctrl_0").unwrap();
    assert_eq!(bram.address[0].base, 0x4000_0000);
    assert_eq!(bram.address[0].size(), 0x2000);
    assert_eq!(bram.params["C_S_AXI_DATA_WIDTH"], "32");
    // The memory behind the controller is passive.
    let mem = d.ips.iter().find(|i| i.modtype == "blk_mem_gen").unwrap();
    assert!(matches!(mem.class, IpClass::Infrastructure { .. }));
    assert!(mem.address.is_empty());
    // The IIC's SCL pin is connected to the block design's `iic_pmod_scl_*` port (the external
    // interface is named `iic_pmod_ja`; the signals inside it keep the `iic_pmod_scl_*` names).
    let scl = iic
        .ports
        .iter()
        .find(|p| p.name.starts_with("scl_"))
        .unwrap();
    assert!(
        scl.connections
            .iter()
            .any(|c| c.port.starts_with("iic_pmod_scl_")),
        "{:?}",
        scl.connections
    );
}

#[test]
fn plumbing_clock_wizards_and_block_memory_are_passive_ip() {
    use IpClass::*;
    for (vlnv, name) in [
        ("xilinx.com:ip:axis_data_fifo:2.0", "axis_data_fifo"),
        ("xilinx.com:ip:axis_switch:1.1", "axis_switch"),
        (
            "xilinx.com:ip:axis_dwidth_converter:1.1",
            "axis_dwidth_converter",
        ),
        (
            "xilinx.com:ip:axis_register_slice:1.1",
            "axis_register_slice",
        ),
        ("xilinx.com:ip:clk_wiz:6.0", "clk_wiz"),
        ("xilinx.com:ip:blk_mem_gen:8.4", "blk_mem_gen"),
        ("xilinx.com:ip:proc_sys_reset:5.0", "proc_sys_reset"),
        ("xilinx.com:ip:xlconstant:1.1", "xlconstant"),
        ("xilinx.com:ip:xlslice:1.0", "xlslice"),
        (
            "xilinx.com:ip:axi_protocol_converter:2.1",
            "axi_protocol_converter",
        ),
        ("xilinx.com:ip:smartconnect:1.0", "smartconnect"),
    ] {
        assert!(
            matches!(classify(vlnv, name), Infrastructure { .. }),
            "{name}"
        );
    }
    // IP with behaviour of its own is not passive, modelled or not.
    for (vlnv, name) in [
        ("xilinx.com:ip:axi_dma:7.1", "axi_dma"),
        ("xilinx.com:ip:axi_vdma:6.3", "axi_vdma"),
        ("xilinx.com:ip:axi_ethernetlite:3.0", "axi_ethernetlite"),
        ("xilinx.com:ip:axi_quad_spi:3.2", "axi_quad_spi"),
        ("xilinx.com:ip:v_tc:6.2", "v_tc"),
        ("xilinx.com:ip:axi_cdma:4.1", "axi_cdma"),
        ("xilinx.com:ip:axi_uart16550:2.0", "axi_uart16550"),
        ("xilinx.com:ip:v_axi4s_vid_out:4.0", "v_axi4s_vid_out"),
        ("xilinx.com:ip:v_vid_in_axi4s:4.0", "v_vid_in_axi4s"),
        ("xilinx.com:ip:ila:6.2", "ila"),
        ("xilinx.com:ip:vio:3.0", "vio"),
    ] {
        assert!(matches!(classify(vlnv, name), VendorIp { .. }), "{name}");
    }
}
