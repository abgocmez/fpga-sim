//! AXI protocol-checker findings reported by an RTL testbench (ADR-025).
//!
//! The checkers are SystemVerilog modules (`fixtures/rtl/common/zs_axi_checker.sv`) that
//! `init --ip` instantiates in the generated testbench. They call the DPI function
//! `zs_protocol_check`, which sends one interrupt packet on [`DEV_PROTOCOL_CHECK`]:
//!
//! * `line` = `port << 12 | channel << 8 | rule`,
//! * `val` = 1 for an informational finding (SLVERR/DECERR), 0 for a protocol violation,
//! * `vector` = a detail (address, beat index, response code...),
//! * `timestamp` = simulation time in nanoseconds.
//!
//! The numbers below are the ones the SystemVerilog side uses; keep them in step.

/// Remote-port device number of the protocol-checker findings (testbench to host).
pub const DEV_PROTOCOL_CHECK: u32 = 21;

/// The AXI4-Lite slave the testbench drives (the IP's own interface).
pub const PORT_LITE_SLAVE: u8 = 0;
/// The AXI4 master the IP drives (memory accesses through the testbench's bridge).
pub const PORT_AXI4_MASTER: u8 = 1;

/// The five AXI channels, in the order of their codes.
pub const CHANNELS: [&str; 5] = ["AW", "W", "B", "AR", "R"];

/// Rule codes. `info` findings are not violations.
pub const RULE_VALID_DROP: u8 = 1;
/// The payload changed while VALID was high and READY low.
pub const RULE_PAYLOAD_UNSTABLE: u8 = 2;
/// VALID high while ARESETN is low (checked from the second reset cycle).
pub const RULE_RESET_VALID: u8 = 3;
/// An INCR burst crosses a 4 KB boundary.
pub const RULE_CROSS_4KB: u8 = 4;
/// WLAST or RLAST asserted before the last beat of the burst.
pub const RULE_LAST_EARLY: u8 = 5;
/// The last beat of the burst is not marked by WLAST or RLAST.
pub const RULE_LAST_MISSING: u8 = 6;
/// A WRAP burst starts at an address that is not aligned to the transfer size.
pub const RULE_WRAP_ALIGN: u8 = 7;
/// A WRAP burst whose length is not 2, 4, 8 or 16 beats.
pub const RULE_WRAP_LEN: u8 = 8;
/// AxSIZE is wider than the data bus.
pub const RULE_SIZE: u8 = 9;
/// SLVERR or DECERR on a response (informational only).
pub const RULE_RESP: u8 = 10;

/// Names of the rule codes, indexed by code (`RULES[0]` is unused).
pub const RULES: [&str; 11] = [
    "",
    "VALID_DROP",
    "PAYLOAD_UNSTABLE",
    "RESET_VALID",
    "CROSS_4KB",
    "LAST_EARLY",
    "LAST_MISSING",
    "WRAP_ALIGN",
    "WRAP_LEN",
    "SIZE",
    "RESP",
];

/// One finding of a protocol checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolFinding {
    /// Simulation time of the finding, in nanoseconds.
    pub t_ns: u64,
    /// [`PORT_LITE_SLAVE`] or [`PORT_AXI4_MASTER`].
    pub port: u8,
    /// Index into [`CHANNELS`].
    pub channel: u8,
    /// Rule code (`RULE_*`).
    pub rule: u8,
    /// True for an informational finding, false for a violation.
    pub info: bool,
    /// Rule-specific detail (address, beat index, response code).
    pub detail: u64,
}

impl ProtocolFinding {
    /// Decodes an interrupt packet of [`DEV_PROTOCOL_CHECK`].
    pub fn from_packet(t_ns: u64, line: u32, val: u8, vector: u64) -> ProtocolFinding {
        ProtocolFinding {
            t_ns,
            port: ((line >> 12) & 0xF) as u8,
            channel: ((line >> 8) & 0xF) as u8,
            rule: (line & 0xFF) as u8,
            info: val != 0,
            detail: vector,
        }
    }

    /// The line and value of the packet that carries this finding (the inverse of
    /// [`ProtocolFinding::from_packet`], for tests and for other host implementations).
    pub fn to_packet(&self) -> (u32, u8, u64) {
        let line = u32::from(self.port & 0xF) << 12
            | u32::from(self.channel & 0xF) << 8
            | u32::from(self.rule);
        (line, u8::from(self.info), self.detail)
    }

    /// Name of the port, as the trace shows it.
    pub fn port_name(&self) -> &'static str {
        match self.port {
            PORT_LITE_SLAVE => "AXI4-Lite slave",
            PORT_AXI4_MASTER => "AXI4 master",
            _ => "unknown port",
        }
    }

    /// Name of the channel (`AW`, `W`, `B`, `AR`, `R`), or `?`.
    pub fn channel_name(&self) -> &'static str {
        CHANNELS
            .get(usize::from(self.channel))
            .copied()
            .unwrap_or("?")
    }

    /// Name of the rule, or `UNKNOWN_RULE_<code>`.
    pub fn rule_name(&self) -> String {
        match RULES.get(usize::from(self.rule)) {
            Some(name) if self.rule != 0 => (*name).to_string(),
            _ => format!("UNKNOWN_RULE_{}", self.rule),
        }
    }

    /// The severity word: `error` for a violation when `strict`, `warning` for a
    /// violation otherwise, `info` for an informational finding.
    pub fn severity(&self, strict: bool) -> &'static str {
        match (self.info, strict) {
            (true, _) => "info",
            (false, true) => "error",
            (false, false) => "warning",
        }
    }

    /// The trace marker text. It starts with [`MARKER_PREFIX`] and the severity, and
    /// carries the simulation time, port, channel, rule and detail:
    /// `protocol check error at 1250 ns: AXI4 master W LAST_MISSING detail=0x3`.
    pub fn marker(&self, strict: bool) -> String {
        format!(
            "{MARKER_PREFIX} {} at {} ns: {} {} {} detail=0x{:x}",
            self.severity(strict),
            self.t_ns,
            self.port_name(),
            self.channel_name(),
            self.rule_name(),
            self.detail
        )
    }
}

/// The start of every protocol-checker trace marker.
pub const MARKER_PREFIX: &str = "protocol check";

/// The severity of a protocol-checker trace marker (`info`, `warning` or `error`), or
/// `None` when the text is some other marker.
pub fn marker_severity(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(MARKER_PREFIX)?.strip_prefix(' ')?;
    let word = rest.split(' ').next()?;
    matches!(word, "info" | "warning" | "error").then_some(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_round_trip_keeps_every_field() {
        let f = ProtocolFinding {
            t_ns: 1250,
            port: PORT_AXI4_MASTER,
            channel: 1,
            rule: RULE_LAST_MISSING,
            info: false,
            detail: 3,
        };
        let (line, val, vector) = f.to_packet();
        assert_eq!(ProtocolFinding::from_packet(1250, line, val, vector), f);
        assert_eq!(line, 0x1106);
    }

    #[test]
    fn names_and_severity() {
        let f = ProtocolFinding::from_packet(15, 0x0003, 0, 0);
        assert_eq!(f.port_name(), "AXI4-Lite slave");
        assert_eq!(f.channel_name(), "AW");
        assert_eq!(f.rule_name(), "RESET_VALID");
        assert_eq!(f.severity(false), "warning");
        assert_eq!(f.severity(true), "error");
        let info = ProtocolFinding::from_packet(9, 0x040A, 1, 2);
        assert_eq!(info.severity(true), "info");
        assert_eq!(info.channel_name(), "R");
        let odd = ProtocolFinding::from_packet(0, 0xFF77, 0, 0);
        assert_eq!(odd.rule_name(), "UNKNOWN_RULE_119");
        assert_eq!(odd.channel_name(), "?");
        assert_eq!(odd.port_name(), "unknown port");
    }

    #[test]
    fn marker_text_and_its_severity() {
        let f = ProtocolFinding {
            t_ns: 1250,
            port: PORT_AXI4_MASTER,
            channel: 1,
            rule: RULE_LAST_MISSING,
            info: false,
            detail: 3,
        };
        assert_eq!(
            f.marker(false),
            "protocol check warning at 1250 ns: AXI4 master W LAST_MISSING detail=0x3"
        );
        assert_eq!(marker_severity(&f.marker(true)), Some("error"));
        assert_eq!(
            marker_severity("protocol check info at 1 ns: x"),
            Some("info")
        );
        assert_eq!(marker_severity("protocol check fatal"), None);
        assert_eq!(marker_severity("protocol checks"), None);
        assert_eq!(marker_severity("qemu start elf=x"), None);
        assert_eq!(marker_severity("protocol check"), None);
    }
}
