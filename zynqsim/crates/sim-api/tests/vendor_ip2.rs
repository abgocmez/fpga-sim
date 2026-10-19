//! AXI DMA, AXI interrupt controller, AXI IIC and AXI BRAM controller (vendor-ip models, ADR-021)
//! under the real QEMU and the real firmware, through `examples/dma-loopback`,
//! `examples/intc-demo` and `examples/iic-bram-demo`.
//!
//! Needs the Xilinx QEMU; prints `SKIPPED:` otherwise.

use sim_api::{Command, Engine, Input, Project, Query, Response};
use std::path::PathBuf;

fn qemu_present() -> bool {
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    std::env::var_os("ZYNQSIM_QEMU").is_some()
        || sim_api::locate_binary("qemu-system-aarch64").is_some()
}

fn load(example: &str) -> (Project, PathBuf) {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(example)
        .join("project.toml");
    (
        Project::load(&file).unwrap(),
        file.parent().unwrap().to_path_buf(),
    )
}

fn engine(example: &str) -> Box<dyn Engine> {
    let (p, dir) = load(example);
    p.build_engine(&dir).unwrap_or_else(|e| panic!("{e}"))
}

fn run_us(e: &mut dyn Engine, us: u64) {
    e.command(Command::Run {
        duration_ps: us * 1_000_000,
    })
    .unwrap();
}

fn uart(e: &mut dyn Engine, port: u8) -> String {
    match e
        .query(
            &Query::UartRead {
                port,
                from: 0,
                max: 1 << 16,
            },
            false,
        )
        .unwrap()
    {
        Response::Uart { data, .. } => String::from_utf8_lossy(&data).into_owned(),
        r => panic!("{r:?}"),
    }
}

fn trace(e: &mut dyn Engine) -> Vec<sim_api::trace::TraceRecord> {
    match e
        .query(
            &Query::Trace {
                from: 0,
                max: 1_000_000,
            },
            false,
        )
        .unwrap()
    {
        Response::Trace { records, .. } => records,
        r => panic!("{r:?}"),
    }
}

#[test]
fn the_dma_copies_buffers_through_the_stream_loopback_and_interrupts() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine("dma-loopback");
    assert_eq!(e.status(false).pl_backend, "native");
    run_us(e.as_mut(), 4_000);
    let u = uart(e.as_mut(), 1);
    // Reset values, the 1 KiB copy checked word by word by the firmware, both channels idle.
    assert!(u.contains("after reset: mm2s sr=0x10001 (halted=1)"), "{u}");
    assert!(
        u.contains("copied 1024 bytes, 0 word(s) differ, s2mm length=1024"),
        "{u}"
    );
    assert!(u.contains("mm2s sr=0x10002 s2mm sr=0x10002 idle=1"), "{u}");
    // The error interrupt of a zero-length transfer, the soft reset, and the second copy.
    assert!(
        u.contains("zero length: err irq=true DMAIntErr=1 halted=1"),
        "{u}"
    );
    assert!(u.contains("copied 512 bytes, 0 word(s) differ"), "{u}");
    assert!(u.contains("RESULT: ALL PASS"), "{u}");

    // The DMA's own memory traffic is in the trace, on the HP0 port it was wired to: a read of
    // the source buffer (MM2S) and a write of the destination (S2MM), 1024 bytes each.
    let recs = trace(e.as_mut());
    let bus = |write: bool, len: u32| {
        recs.iter()
            .filter(|r| {
                matches!(&r.event, sim_api::trace::TraceEvent::Bus { port, write: w, len: l, ok, .. }
                    if port == "S_AXI_HP0" && *w == write && *l == len && *ok)
            })
            .count()
    };
    assert_eq!(bus(false, 1024), 1, "MM2S read");
    assert_eq!(bus(true, 1024), 1, "S2MM write");
    assert_eq!(bus(false, 512), 1);
    assert_eq!(bus(true, 512), 1);
}

#[test]
fn scatter_gather_descriptors_move_a_packet_through_the_loopback() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine("dma-sg");
    run_us(e.as_mut(), 3_000);
    let u = uart(e.as_mut(), 1);
    assert!(u.contains("sr after reset: 0x10009 (SGIncld=1)"), "{u}");
    // One interrupt per channel: IRQThreshold = 2 groups the two descriptors.
    assert!(u.contains("irqs: mm2s=1 s2mm=1 errors=0"), "{u}");
    // MM2S descriptors: Cmplt | transferred length. S2MM: SOF on the first, EOF on the last,
    // which received only the 16 bytes that were left of the packet.
    assert!(u.contains("tx status: 0x80000028 0x80000018"), "{u}");
    assert!(u.contains("rx status: 0x88000030 0x84000010"), "{u}");
    assert!(
        u.contains("data: 0 byte(s) differ; idle mm2s=1 s2mm=1"),
        "{u}"
    );
    assert!(u.contains("RESULT: ALL PASS"), "{u}");
}

#[test]
fn the_dma_session_is_deterministic() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut hashes = Vec::new();
    for _ in 0..2 {
        let mut e = engine("dma-loopback");
        run_us(e.as_mut(), 3_000);
        hashes.push((e.status(false).trace_hash, uart(e.as_mut(), 1)));
    }
    assert_eq!(hashes[0], hashes[1]);
    assert!(hashes[0].1.contains("ALL PASS"), "{}", hashes[0].1);
}

#[test]
fn the_interrupt_controller_serves_the_timer_and_the_gpio_through_one_irq_line() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine("intc-demo");
    run_us(e.as_mut(), 3_000);
    let u = uart(e.as_mut(), 1);
    // Software interrupt through ISR (MER.ME only), then the hardware path: three timer
    // interrupts on input 1, all served from IRQ_F2P[0] through IVR/IAR.
    assert!(
        u.contains("software interrupt: taken=true isr after ack=0x0 mer=0x1"),
        "{u}"
    );
    assert!(u.contains("ier=0x3 mer=0x3"), "{u}");
    assert!(
        u.contains("timer ticks=3 (via input 1) ipr=0x0 errors=0"),
        "{u}"
    );
    assert!(u.contains("ier after CIE=0x1"), "{u}");
    assert!(u.contains("RESULT: ALL PASS"), "{u}");

    // A switch changes: the GPIO interrupt arrives on input 0 and is served.
    e.command(Command::Input(Input::SetSwitch { index: 1, on: true }))
        .unwrap();
    run_us(e.as_mut(), 600);
    e.command(Command::Input(Input::SetSwitch {
        index: 1,
        on: false,
    }))
    .unwrap();
    run_us(e.as_mut(), 600);
    let u = uart(e.as_mut(), 1);
    assert!(u.contains("gpio interrupt #1: switches=0x2"), "{u}");
    assert!(u.contains("gpio interrupt #2: switches=0x0"), "{u}");
}

#[test]
fn the_iic_master_reads_the_tmp2_model_and_the_bram_holds_data() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let mut e = engine("iic-bram-demo");
    run_us(e.as_mut(), 8_000);
    let u = uart(e.as_mut(), 1);
    assert!(u.contains("bram: 2048 words, 0 differ"), "{u}");
    assert!(u.contains("bram test ok"), "{u}");
    assert!(u.contains("iic: sr=0xc0"), "{u}");
    assert!(u.contains("ID 0xCB"), "{u}");
    // 23.5 degC in the ADT7420's 13-bit format: 23.5 / 0.0625 = 376 -> 376 << 3 = 0x0BC0.
    assert!(
        u.contains("T raw=0x0BC0 mC=23500 (rx irqs=1, bytes in fifo=2)"),
        "{u}"
    );
    assert!(
        u.contains("absent device: tx error irqs=1 bus busy=0"),
        "{u}"
    );
    assert!(u.contains("RESULT: ALL PASS"), "{u}");

    // The bus pins the master drove reached the Pmod port: JA saw SCL and SDA move.
    let recs = trace(e.as_mut());
    let ja = recs
        .iter()
        .filter(|r| {
            matches!(&r.event, sim_api::trace::TraceEvent::Signal { name, .. } if name.starts_with("pmod.ja"))
        })
        .count();
    assert!(ja > 100, "pins on JA: {ja}");
}

#[test]
fn the_sensor_value_the_host_sets_is_what_the_iic_master_reads() {
    if !qemu_present() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let (mut p, dir) = load("iic-bram-demo");
    p.device_values
        .get_mut("pmod_ja")
        .unwrap()
        .insert("temperature".to_string(), -12.25);
    let mut e = p.build_engine(&dir).unwrap_or_else(|e| panic!("{e}"));
    run_us(e.as_mut(), 8_000);
    let u = uart(e.as_mut(), 1);
    // -12.25 degC = -196 / 16: raw 0xF9E0 in the 13-bit format.
    assert!(u.contains("T raw=0xF9E0 mC=-12250"), "{u}");
    assert!(u.contains("RESULT: ALL PASS"), "{u}");
}

#[test]
fn the_axi_quad_spi_model_runs_loopback_transfers_and_interrupts() {
    let (p, dir) = load("spi-demo");
    let mut e = p.build_engine(&dir).unwrap_or_else(|e| panic!("{e}"));
    run_us(e.as_mut(), 20_000);
    let u = uart(e.as_mut(), 1);
    assert!(u.contains("RESULT: ALL PASS"), "{u}");
}

#[test]
fn the_axi_fifo_model_loops_packets_back_and_raises_interrupts() {
    let (p, dir) = load("axis-fifo-demo");
    let mut e = p.build_engine(&dir).unwrap_or_else(|e| panic!("{e}"));
    run_us(e.as_mut(), 20_000);
    let u = uart(e.as_mut(), 1);
    for line in [
        "packet 1: 4 words looped back",
        "packet 2: 6 bytes looped back",
        "tx reset ok",
        "rx reset ok",
        "srr reset ok",
        "RESULT: ALL PASS",
    ] {
        assert!(u.contains(line), "UART1 lacks `{line}`:\n{u}");
    }
}
