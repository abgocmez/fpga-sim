//! Summary of the AXI protocol-checker findings in a trace (ADR-025).
//!
//! The findings are trace markers (`protocol check <severity> at <t> ns: ...`, see
//! `pl_api::protocol`). The CLI reports their counts and fails a strict run on an error.

use pl_api::protocol::marker_severity;
use trace::{TraceEvent, TraceLog};

/// How many protocol-checker markers of each severity a trace holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProtocolSummary {
    /// Violations reported as warnings.
    pub warnings: usize,
    /// Violations reported as errors (a strict run).
    pub errors: usize,
    /// Informational findings (SLVERR/DECERR responses).
    pub infos: usize,
    /// The marker texts, in trace order.
    pub findings: Vec<String>,
}

/// The protocol-checker markers of `trace`.
pub fn protocol_summary(trace: &TraceLog) -> ProtocolSummary {
    let mut s = ProtocolSummary::default();
    for r in trace.records() {
        let TraceEvent::Marker { text } = &r.event else {
            continue;
        };
        match marker_severity(text) {
            Some("warning") => s.warnings += 1,
            Some("error") => s.errors += 1,
            Some("info") => s.infos += 1,
            _ => continue,
        }
        s.findings.push(text.clone());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_core::Ps;

    #[test]
    fn counts_each_severity_and_ignores_other_markers() {
        let mut log = TraceLog::new();
        log.push(
            Ps(1),
            TraceEvent::Marker {
                text: "qemu start elf=x".into(),
            },
        );
        log.push(
            Ps(2_000),
            TraceEvent::Marker {
                text: "protocol check warning at 2 ns: AXI4-Lite slave AW VALID_DROP detail=0x0"
                    .into(),
            },
        );
        log.push(
            Ps(3_000),
            TraceEvent::Marker {
                text: "protocol check info at 3 ns: AXI4 master R RESP detail=0x2".into(),
            },
        );
        log.push(
            Ps(4_000),
            TraceEvent::Marker {
                text: "protocol check error at 4 ns: AXI4 master AW CROSS_4KB detail=0xffc".into(),
            },
        );
        log.push(
            Ps(5_000),
            TraceEvent::Input {
                what: "switch 1 = on".into(),
            },
        );
        let s = protocol_summary(&log);
        assert_eq!((s.warnings, s.errors, s.infos), (1, 1, 1));
        assert_eq!(s.findings.len(), 3);
        assert!(s.findings[0].contains("VALID_DROP"));
    }

    #[test]
    fn an_empty_trace_has_no_findings() {
        assert_eq!(
            protocol_summary(&TraceLog::new()),
            ProtocolSummary::default()
        );
    }
}
