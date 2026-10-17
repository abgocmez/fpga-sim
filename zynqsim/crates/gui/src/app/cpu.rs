//! The CPU registers section: flags and mode, the registers, and which ones
//! changed since the guest last ran.

use super::App;
use super::shell::Action;
use eframe::egui::{self, Color32, RichText};
use sim_api::CpuRegs;

const ACCENT: Color32 = Color32::from_rgb(90, 180, 255);
const CHANGED: Color32 = Color32::from_rgb(235, 160, 20);
const GRAY: Color32 = Color32::GRAY;

/// The CPSR's N, Z, C and V flags, in that order, with whether each is set.
pub fn cpsr_flags(cpsr: u32) -> [(char, bool); 4] {
    [
        ('N', cpsr & (1 << 31) != 0),
        ('Z', cpsr & (1 << 30) != 0),
        ('C', cpsr & (1 << 29) != 0),
        ('V', cpsr & (1 << 28) != 0),
    ]
}

/// The name of register `n` (`sp`, `lr`, `pc` for 13..=15).
pub fn reg_name(n: usize) -> String {
    match n {
        13 => "sp".to_string(),
        14 => "lr".to_string(),
        15 => "pc".to_string(),
        n => format!("r{n}"),
    }
}

/// The one-line summary of the section while it is closed.
pub fn summary(regs: Option<&CpuRegs>, debugger: bool) -> String {
    match regs {
        Some(r) => format!(
            "stopped at {}",
            r.pc_symbol
                .clone()
                .unwrap_or_else(|| format!("{:#010x}", r.r[15]))
        ),
        None if debugger => "running".to_string(),
        None => "no debugger".to_string(),
    }
}

impl App {
    /// Registers of core 0, and of core 1 when the project declares one. When
    /// core 0 has none because the QEMU run lacks the debugger, the debugger
    /// offer is drawn instead of the reason; returns the button pressed.
    pub(super) fn cpu_registers(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let cores = if self.model.cpu1_declared { 2 } else { 1 };
        let mut picked = None;
        for core in 0..cores {
            if self.model.cpu1_declared {
                ui.label(RichText::new(format!("CPU {core}")).strong());
            }
            if let Some(r) = self.model.cpu_regs[core].as_ref() {
                flags_row(ui, r.cpsr, &r.mode);
                if r.r.iter().all(|v| *v == 0) {
                    ui.label(
                        RichText::new("all registers are zero: nothing has run on this core")
                            .color(GRAY)
                            .small(),
                    );
                }
                if let Some(s) = &r.pc_symbol {
                    ui.label(RichText::new(format!("pc in {s}")).strong());
                }
                register_grid(ui, r, &self.model.cpu_changed[core]);
                ui.label(
                    RichText::new(format!("cpsr {:08x}", r.cpsr))
                        .monospace()
                        .small(),
                );
            } else if core == 0 && self.restart_offered() {
                picked = self.debugger_offer(ui);
            } else if core == 0 && self.model.status.ps_backend == "native" {
                ui.label(
                    RichText::new("running: the registers are read when the simulation is paused")
                        .color(GRAY)
                        .small(),
                );
            } else {
                ui.label(RichText::new(&self.cpu_reason[core]).color(GRAY).small());
            }
            if core + 1 < cores {
                ui.add_space(6.0);
            }
        }
        picked
    }
}

/// N Z C V as chips (set flags filled) and the processor mode.
fn flags_row(ui: &mut egui::Ui, cpsr: u32, mode: &str) {
    ui.horizontal_wrapped(|ui| {
        for (name, set) in cpsr_flags(cpsr) {
            let text = RichText::new(format!(" {name} ")).monospace().strong();
            let text = if set {
                text.background_color(ACCENT).color(Color32::BLACK)
            } else {
                text.color(GRAY)
            };
            ui.label(text)
                .on_hover_text(if set { "set" } else { "clear" });
        }
        ui.label(
            RichText::new(format!(" mode {mode} "))
                .small()
                .background_color(Color32::from_gray(70)),
        );
    });
}

/// The registers, four to a row, changed ones in a warning colour.
fn register_grid(ui: &mut egui::Ui, r: &CpuRegs, changed: &[bool; 16]) {
    egui::Grid::new(("regs", r.core))
        .num_columns(8)
        .spacing([8.0, 2.0])
        .show(ui, |ui| {
            for (i, value) in r.r.iter().enumerate() {
                ui.label(RichText::new(reg_name(i)).monospace().small());
                let text = RichText::new(format!("{value:08x}")).monospace().small();
                let text = if changed[i] {
                    text.color(CHANGED).strong()
                } else {
                    text
                };
                let resp = ui.label(text);
                if changed[i] {
                    resp.on_hover_text("changed since the guest last ran");
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::changed_registers;

    #[test]
    fn flags_come_from_the_top_four_bits_of_the_cpsr() {
        let f = cpsr_flags(0b1010 << 28);
        assert_eq!(f, [('N', true), ('Z', false), ('C', true), ('V', false)]);
        assert!(cpsr_flags(0).iter().all(|(_, set)| !set));
    }

    #[test]
    fn registers_that_changed_since_the_last_stop_are_marked() {
        let prev = [1u32; 16];
        let mut cur = prev;
        cur[0] = 2;
        cur[15] = 0x0010_058c;
        let c = changed_registers(Some(&prev), &cur);
        assert!(c[0] && c[15]);
        assert_eq!(c.iter().filter(|x| **x).count(), 2);
        assert_eq!(
            changed_registers(None, &cur),
            [false; 16],
            "no previous stop"
        );
    }

    #[test]
    fn register_names_and_summary() {
        assert_eq!(reg_name(13), "sp");
        assert_eq!(reg_name(14), "lr");
        assert_eq!(reg_name(15), "pc");
        assert_eq!(reg_name(3), "r3");
        assert_eq!(summary(None, false), "no debugger");
        assert_eq!(summary(None, true), "running");
        let r = CpuRegs {
            core: 0,
            r: [0; 16],
            cpsr: 0,
            mode: "svc".into(),
            pc_symbol: Some("main+0x4".into()),
            pc_source: None,
        };
        assert_eq!(summary(Some(&r), true), "stopped at main+0x4");
    }
}
