//! The data model of a parsed hardware handoff.

use crate::catalog::IpClass;
use serde::Serialize;
use std::collections::BTreeMap;

/// What a source file in the archive is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// `.v`, `.vh`
    Verilog,
    /// `.sv`, `.svh`
    SystemVerilog,
    /// `.vhd`, `.vhdl`
    Vhdl,
    /// An `export_simulation` project file (`.prj`) or a plain file list (`.f`).
    FileList,
}

impl SourceKind {
    /// Classifies a lower-case file name by extension.
    pub fn from_name(lower: &str) -> Option<SourceKind> {
        let ext = lower.rsplit('.').next()?;
        Some(match ext {
            "v" | "vh" => SourceKind::Verilog,
            "sv" | "svh" => SourceKind::SystemVerilog,
            "vhd" | "vhdl" => SourceKind::Vhdl,
            "prj" | "f" => SourceKind::FileList,
            _ => return None,
        })
    }
}

/// An HDL source or file list from the archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceFile {
    /// Path inside the archive (forward slashes, no `..`).
    pub path: String,
    /// Kind of file.
    pub kind: SourceKind,
    /// The file's text (lossy UTF-8).
    #[serde(skip)]
    pub text: String,
}

/// One end of a connection: the instance and port on the other side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PortConnection {
    /// Instance name (a block-design cell, or `design_1_imp` for an external port).
    pub instance: String,
    /// Port name on that instance.
    pub port: String,
}

/// A module (IP instance) port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Port {
    /// Port name.
    pub name: String,
    /// `I`, `O` or `IO`.
    pub dir: String,
    /// Bit width (`|LEFT-RIGHT|+1`, 1 when the port has no range).
    pub width: u32,
    /// `SIGIS`: `clk`, `rst`, `INTERRUPT`, `undef`, ...
    pub sigis: Option<String>,
    /// `SENSITIVITY` of an interrupt (`LEVEL_HIGH`, `EDGE_RISING`, ...).
    pub sensitivity: Option<String>,
    /// `CLKFREQUENCY` in Hz of a clock port.
    pub clk_hz: Option<u64>,
    /// `POLARITY` of a reset port.
    pub polarity: Option<String>,
    /// What the port is connected to.
    pub connections: Vec<PortConnection>,
}

/// An AXI (or other) bus interface of a module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BusInterface {
    /// Interface name (`S_AXI`, `M_AXI_GP0`).
    pub name: String,
    /// `SLAVE`, `MASTER`, `INITIATOR`, `TARGET`.
    pub kind: String,
    /// `PROTOCOL` parameter (`AXI4LITE`, `AXI4`, `AXI3`) when present.
    pub protocol: Option<String>,
    /// `DATAWIDTH`.
    pub data_width: Option<u32>,
    /// The bus net the interface is on.
    pub bus_name: Option<String>,
    /// Interface VLNV (`xilinx.com:interface:aximm:1.0`).
    pub vlnv: Option<String>,
}

/// One `MEMRANGE` of a module's `MEMORYMAP`: a window the module (as a bus master) reaches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddrRange {
    /// The module that owns the `MEMORYMAP` (the master side), for example the PS7.
    pub master: String,
    /// `MASTERBUSINTERFACE` (`M_AXI_GP0`).
    pub master_interface: String,
    /// `INSTANCE`: the slave module reached.
    pub instance: String,
    /// `SLAVEBUSINTERFACE` of that slave.
    pub interface: String,
    /// `ADDRESSBLOCK` name.
    pub block: String,
    /// `BASEVALUE`.
    pub base: u64,
    /// `HIGHVALUE` (inclusive).
    pub high: u64,
    /// `MEMTYPE` (`REGISTER`, `MEMORY`).
    pub mem_type: Option<String>,
}

impl AddrRange {
    /// Size in bytes (`high - base + 1`).
    pub fn size(&self) -> u64 {
        self.high.saturating_sub(self.base).saturating_add(1)
    }
}

/// A PL interrupt source wired to `IRQ_F2P`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IrqLink {
    /// The module driving the line.
    pub instance: String,
    /// Its output port.
    pub port: String,
    /// First `IRQ_F2P` bit (0..15).
    pub bit: u32,
    /// Number of bits (1 for a scalar interrupt).
    pub width: u32,
    /// Sensitivity from the HWH, if present.
    pub sensitivity: Option<String>,
    /// Instances between the source and the PS (an `xlconcat`).
    pub via: Vec<String>,
}

impl IrqLink {
    /// The GIC interrupt ID of `IRQ_F2P[bit]` (Zynq-7000 TRM: `[7:0]` is 61..68, `[15:8]` 84..91).
    pub fn gic_id(bit: u32) -> u32 {
        if bit < 8 { 61 + bit } else { 84 + (bit - 8) }
    }
}

/// A PL clock input and where it comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClockLink {
    /// The module with the clock input.
    pub instance: String,
    /// Its clock port.
    pub port: String,
    /// Instance driving it.
    pub source_instance: Option<String>,
    /// Port driving it.
    pub source_port: Option<String>,
    /// `Some(n)` when it is the PS7's `FCLK_CLKn`.
    pub fclk: Option<u8>,
    /// Frequency in Hz (`CLKFREQUENCY`).
    pub hz: Option<u64>,
}

/// A PL reset input and where it comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResetLink {
    /// The module with the reset input.
    pub instance: String,
    /// Its reset port.
    pub port: String,
    /// Instance driving it.
    pub source_instance: Option<String>,
    /// Port driving it.
    pub source_port: Option<String>,
    /// `Some(n)` when it is the PS7's `FCLK_RESETn_N` directly.
    pub fclk_reset: Option<u8>,
}

/// One IP instance (anything but the PS7).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IpInstance {
    /// Instance name.
    pub instance: String,
    /// Hierarchical name (`/axi_gpio_0`).
    pub full_name: String,
    /// `VLNV` (`xilinx.com:ip:axi_gpio:2.0`, `xilinx.com:module_ref:my_ip:1.0`).
    pub vlnv: String,
    /// `MODTYPE` (the IP name, or the module name of a module reference).
    pub modtype: String,
    /// What ZynqSim knows about it.
    pub class: IpClass,
    /// `PARAMETERS`.
    pub params: BTreeMap<String, String>,
    /// Bus interfaces.
    pub interfaces: Vec<BusInterface>,
    /// Ports.
    pub ports: Vec<Port>,
    /// Address windows the PS reaches in this instance (from the PS7's memory map).
    pub address: Vec<AddrRange>,
    /// Windows this instance, as a master, reaches (for example the PS DDR through `S_AXI_HP0`).
    pub master_windows: Vec<AddrRange>,
}

/// `FCLK_CLKn` configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FclkInfo {
    /// The clock port is enabled (`PCW_EN_CLKn_PORT`).
    pub enabled: bool,
    /// Requested MHz (`PCW_FPGAn_PERIPHERAL_FREQMHZ`).
    pub requested_mhz: Option<f64>,
    /// Achieved MHz (`PCW_ACT_FPGAn_PERIPHERAL_FREQMHZ`).
    pub actual_mhz: Option<f64>,
    /// `CLKFREQUENCY` of the `FCLK_CLKn` port in Hz.
    pub port_hz: Option<u64>,
    /// `PCW_EN_RSTn_PORT`: `FCLK_RESETn_N` is enabled.
    pub reset_enabled: bool,
}

impl FclkInfo {
    /// The best known frequency in Hz when enabled.
    pub fn hz(&self) -> Option<u64> {
        if !self.enabled {
            return None;
        }
        self.port_hz
            .or_else(|| self.actual_mhz.map(|m| (m * 1e6).round() as u64))
            .or_else(|| self.requested_mhz.map(|m| (m * 1e6).round() as u64))
    }
}

/// One MIO pin of the PS7.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MioPin {
    /// MIO number 0..53.
    pub index: u32,
    /// Peripheral from `PCW_MIO_TREE_PERIPHERALS` (`UART 1`, `GPIO`, ...).
    pub peripheral: String,
    /// Signal from `PCW_MIO_TREE_SIGNALS` (`tx`, `qspi0_ss_b`, ...).
    pub signal: String,
}

/// A PS peripheral and whether the design enables it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PsPeripheral {
    /// `UART0`, `ENET0`, `SD0`, `QSPI`, `GPIO`, ...
    pub name: String,
    /// Enabled in the PS7 configuration.
    pub enabled: bool,
    /// The IO selection (`MIO 48 .. 49`, `EMIO`), when enabled and given.
    pub io: Option<String>,
}

/// The PS7 instance and its configuration.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ps7 {
    /// Instance name (usually `processing_system7_0`).
    pub instance: String,
    /// VLNV.
    pub vlnv: String,
    /// `FCLK_CLK0..3`.
    pub fclk: [FclkInfo; 4],
    /// `M_AXI_GP0/1` enabled.
    pub m_axi_gp: [bool; 2],
    /// `S_AXI_GP0/1` enabled.
    pub s_axi_gp: [bool; 2],
    /// `S_AXI_HP0..3` enabled.
    pub s_axi_hp: [bool; 4],
    /// `S_AXI_ACP` enabled.
    pub s_axi_acp: bool,
    /// `IRQ_F2P` is used (fabric interrupts enabled).
    pub irq_f2p_enabled: bool,
    /// Width of `IRQ_F2P` (1..16).
    pub irq_f2p_width: u32,
    /// EMIO GPIO is enabled; its width.
    pub emio_gpio_width: Option<u32>,
    /// MIO use, one entry per pin (empty when the HWH has no MIO tree).
    pub mio: Vec<MioPin>,
    /// PS peripherals (enabled or not).
    pub peripherals: Vec<PsPeripheral>,
    /// Number of `PCW_*` parameters seen (the full set is not kept).
    pub param_count: usize,
}

/// A parsed hardware handoff.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Design {
    /// Block design name (`SYSTEMINFO NAME`).
    pub name: String,
    /// `VIVADOVERSION`.
    pub vivado_version: Option<String>,
    /// `TIMESTAMP`.
    pub timestamp: Option<String>,
    /// `ARCH` (`zynq`).
    pub arch: Option<String>,
    /// `DEVICE` (`7z020`).
    pub device: Option<String>,
    /// `PACKAGE`.
    pub package: Option<String>,
    /// `SPEEDGRADE`.
    pub speed_grade: Option<String>,
    /// `BOARD` part, when the design used one.
    pub board: Option<String>,
    /// Name of the `.hwh` inside the archive.
    pub hwh_file: Option<String>,
    /// The PS7, when the design has one.
    pub ps7: Option<Ps7>,
    /// Every other module.
    pub ips: Vec<IpInstance>,
    /// Every `MEMRANGE` of every module (PS to PL windows and PL master windows).
    pub memory_map: Vec<AddrRange>,
    /// PL interrupts wired to `IRQ_F2P`, resolved through `xlconcat`.
    pub interrupts: Vec<IrqLink>,
    /// PL clock inputs.
    pub clocks: Vec<ClockLink>,
    /// PL reset inputs.
    pub resets: Vec<ResetLink>,
    /// Things the reader noticed but could not resolve.
    pub notes: Vec<String>,
}

impl Design {
    /// The IP instance named `instance`.
    pub fn ip(&self, instance: &str) -> Option<&IpInstance> {
        self.ips.iter().find(|i| i.instance == instance)
    }

    /// Interrupt links of `instance`.
    pub fn irqs_of<'a>(&'a self, instance: &'a str) -> impl Iterator<Item = &'a IrqLink> + 'a {
        self.interrupts
            .iter()
            .filter(move |l| l.instance == instance)
    }

    /// Clock inputs of `instance`.
    pub fn clocks_of<'a>(&'a self, instance: &'a str) -> impl Iterator<Item = &'a ClockLink> + 'a {
        self.clocks.iter().filter(move |l| l.instance == instance)
    }
}
