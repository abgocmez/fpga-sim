//! What ZynqSim knows about an IP by its VLNV.

use serde::Serialize;

/// How an IP instance is treated when a project is made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum IpClass {
    /// The Zynq-7000 processing system: ZynqSim's PS side.
    Ps7,
    /// The user's own RTL: a module reference (`xilinx.com:module_ref:*`) or a packaged custom
    /// IP. Simulated from its sources.
    UserRtl {
        /// `true` for a packaged IP (`user.org:user:*`, `xilinx.com:user:*`), `false` for a
        /// module reference.
        packaged: bool,
    },
    /// A Xilinx IP in the catalog. The `vendor-ip` library models only some of them (AXI GPIO,
    /// AXI Timer, AXI UART Lite); the importer decides by name which ones run.
    VendorIp {
        /// Catalog name (`axi_gpio`).
        name: String,
        /// Human title (`AXI GPIO`).
        title: String,
    },
    /// Wiring and housekeeping IP that has no software-visible behaviour of its own in this
    /// flow (interconnects, reset generators, concatenations).
    Infrastructure {
        /// Human title.
        title: String,
    },
    /// A Xilinx IP the catalog does not list: it has no model.
    UnknownVendorIp,
    /// An IP from another vendor (for example a Digilent core): no model, sources unknown.
    ThirdParty,
}

/// Catalog of Xilinx IP the `vendor-ip` library is meant to model (prompt section 9.5); which
/// of them it does model is `vendor_ip::MODELS`.
const VENDOR: &[(&str, &str)] = &[
    ("axi_gpio", "AXI GPIO"),
    ("axi_timer", "AXI Timer"),
    ("axi_uartlite", "AXI UART Lite"),
    ("axi_uart16550", "AXI UART 16550"),
    ("axi_dma", "AXI DMA"),
    ("axi_vdma", "AXI VDMA"),
    ("axi_cdma", "AXI CDMA"),
    ("axi_iic", "AXI IIC"),
    ("axi_quad_spi", "AXI Quad SPI"),
    ("axi_bram_ctrl", "AXI BRAM Controller"),
    ("blk_mem_gen", "Block Memory Generator"),
    ("axi_intc", "AXI Interrupt Controller"),
    ("axi_fifo_mm_s", "AXI4-Stream FIFO (memory mapped)"),
    ("axis_data_fifo", "AXI4-Stream Data FIFO"),
    ("axis_switch", "AXI4-Stream Switch"),
    ("axis_dwidth_converter", "AXI4-Stream Data Width Converter"),
    ("clk_wiz", "Clocking Wizard"),
    ("v_tc", "Video Timing Controller"),
    ("v_axi4s_vid_out", "AXI4-Stream to Video Out"),
    ("v_vid_in_axi4s", "Video In to AXI4-Stream"),
    ("ila", "Integrated Logic Analyzer"),
    ("vio", "Virtual Input/Output"),
    ("xadc_wiz", "XADC Wizard"),
    ("axi_ethernetlite", "AXI Ethernet Lite"),
    ("axi_hwicap", "AXI HWICAP"),
    ("axi_perf_mon", "AXI Performance Monitor"),
];

/// Xilinx IP that only connects things.
const INFRA: &[(&str, &str)] = &[
    ("axi_interconnect", "AXI Interconnect"),
    ("smartconnect", "AXI SmartConnect"),
    ("proc_sys_reset", "Processor System Reset"),
    ("xlconcat", "Concat"),
    ("xlslice", "Slice"),
    ("xlconstant", "Constant"),
    ("util_vector_logic", "Utility Vector Logic"),
    ("util_reduced_logic", "Utility Reduced Logic"),
    ("axi_protocol_converter", "AXI Protocol Converter"),
    ("axi_register_slice", "AXI Register Slice"),
    ("axi_clock_converter", "AXI Clock Converter"),
    ("axi_data_fifo", "AXI Data FIFO"),
    ("axi_crossbar", "AXI Crossbar"),
    ("axi_dwidth_converter", "AXI Data Width Converter"),
    ("axi_mem_intercon", "AXI Memory Interconnect"),
];

/// Classifies an instance from its `VLNV` (`vendor:library:name:version`) and `MODTYPE`.
pub fn classify(vlnv: &str, modtype: &str) -> IpClass {
    let mut parts = vlnv.split(':');
    let vendor = parts.next().unwrap_or("");
    let library = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or(modtype);
    if name == "processing_system7" {
        return IpClass::Ps7;
    }
    if library == "module_ref" {
        return IpClass::UserRtl { packaged: false };
    }
    if library == "user" && (vendor == "xilinx.com" || vendor == "user.org") {
        return IpClass::UserRtl { packaged: true };
    }
    if vendor == "xilinx.com" && library == "ip" {
        if let Some((_, t)) = INFRA.iter().find(|(n, _)| *n == name) {
            return IpClass::Infrastructure {
                title: (*t).to_string(),
            };
        }
        if let Some((n, t)) = VENDOR.iter().find(|(n, _)| *n == name) {
            return IpClass::VendorIp {
                name: (*n).to_string(),
                title: (*t).to_string(),
            };
        }
        return IpClass::UnknownVendorIp;
    }
    IpClass::ThirdParty
}
