//! End-to-end: the Rust firmware brings up the Cadence GEM and pings the
//! simulated network's gateway under the real Xilinx QEMU.
//!
//! Needs the Xilinx QEMU from a Vitis install; without it the tests print
//! `SKIPPED:` and return, so look for that word in `-- --nocapture` output.

use backend_qemu::net::{
    Direction, FrameKind, NetConfig, RawFrame, describe, direction, resolve_virtual_ns,
};
use backend_qemu::{QemuConfig, QemuMachine, RunEnd};
use pl_native::NativePl;
use std::path::PathBuf;
use std::time::Duration;

fn find_qemu() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZYNQSIM_QEMU") {
        return Some(PathBuf::from(p));
    }
    let mut roots = vec![PathBuf::from(r"C:\Xilinx\Vitis")];
    if let Some(v) = std::env::var_os("XILINX_VITIS") {
        roots.push(PathBuf::from(v));
    }
    for root in roots {
        let mut dirs = vec![root.clone()];
        if let Ok(rd) = std::fs::read_dir(&root) {
            dirs.extend(rd.flatten().map(|e| e.path()));
        }
        for d in dirs {
            let q = d
                .join("data")
                .join("emulation")
                .join("qemu_win")
                .join("qemu")
                .join("qemu-system-aarch64.exe");
            if q.is_file() {
                return Some(q);
            }
        }
    }
    None
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("elf")
        .join(name)
}

fn config(tag: &str, net: NetConfig) -> Option<QemuConfig> {
    let qemu = find_qemu()?;
    let dtb = QemuConfig::dtb_for(&qemu)?;
    let d = std::env::temp_dir().join(format!("zynqsim-net-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let mut cfg = QemuConfig::new(qemu, dtb, fixture("rust_net.elf"), d);
    cfg.net = Some(net);
    Some(cfg)
}

struct Outcome {
    uart: String,
    /// `(virtual time in ns, frame)` in capture order.
    frames: Vec<(u64, Vec<u8>)>,
    end_ns: u64,
}

/// Runs in 100 us slices until the firmware reports a result, collecting the
/// frames each slice produced.
fn run_net(cfg: &QemuConfig) -> Outcome {
    run_net_in(cfg, 100_000)
}

fn run_net_in(cfg: &QemuConfig, slice_ns: u64) -> Outcome {
    let mut m = QemuMachine::spawn(cfg).expect("QEMU starts");
    let mut pl = NativePl::new();
    let mut uart = String::new();
    let mut frames = Vec::new();
    let mut lo = 0u64;
    let mut target = 0u64;
    loop {
        target += slice_ns;
        match m
            .run_until(target, &mut pl, Duration::from_secs(60))
            .expect("run")
        {
            RunEnd::ReachedTime => {}
            RunEnd::PeerClosed => panic!("QEMU exited early; uart so far: {uart:?}"),
        }
        let hi = m.now_ns();
        for RawFrame { ts_usec, data, .. } in m.net_take() {
            frames.push((resolve_virtual_ns(ts_usec, lo, hi), data));
        }
        lo = hi;
        uart.push_str(&String::from_utf8_lossy(&m.uart_take()));
        if uart.contains("RESULT:") {
            return Outcome {
                uart,
                frames,
                end_ns: hi,
            };
        }
        assert!(target < 300_000_000, "firmware did not finish: {uart:?}");
    }
}

#[test]
fn firmware_pings_the_gateway_and_every_frame_is_captured() {
    let Some(cfg) = config("ping", NetConfig::default()) else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let o = run_net(&cfg);
    eprintln!("{}", o.uart);
    for (t, f) in &o.frames {
        eprintln!("{t:>10} ns  {}", describe(f).summary);
    }
    for want in [
        "PHY: addr=",
        "link=up",
        "MAC: 00:0a:35:00:1e:53",
        "DHCP: address=10.0.2.15/24 server=10.0.2.2 router=none",
        "ping target 10.0.2.2",
        "ping reply seq=0 bytes=32",
        "ping reply seq=1 bytes=32",
        "ping reply seq=2 bytes=32",
        "ping reply seq=3 bytes=32",
        "net demo: 4/4 replies, frames tx=7 rx=7",
        "RESULT: ALL PASS",
    ] {
        assert!(o.uart.contains(want), "missing `{want}` in:\n{}", o.uart);
    }
    assert!(!o.uart.contains("DHCP: no answer"), "{}", o.uart);

    // DHCP DISCOVER/OFFER and REQUEST/ACK, ARP request/reply, then four echo
    // request/reply pairs.
    let kinds: Vec<FrameKind> = o.frames.iter().map(|(_, f)| describe(f).kind).collect();
    let mut want = vec![
        FrameKind::Udp,
        FrameKind::Udp,
        FrameKind::Udp,
        FrameKind::Udp,
        FrameKind::Arp,
        FrameKind::Arp,
    ];
    for _ in 0..4 {
        want.push(FrameKind::IcmpEchoRequest);
        want.push(FrameKind::IcmpEchoReply);
    }
    assert_eq!(
        kinds,
        want,
        "{:#?}",
        o.frames
            .iter()
            .map(|(t, f)| (t, describe(f).summary))
            .collect::<Vec<_>>()
    );
    for (i, (_, f)) in o.frames.iter().enumerate() {
        let info = describe(f);
        assert!(info.checksums_ok, "frame {i}: {}", info.summary);
        let expect = if i % 2 == 0 {
            Direction::Tx
        } else {
            Direction::Rx
        };
        assert_eq!(direction(f), expect, "frame {i}: {}", info.summary);
    }
    let sum = |i: usize| describe(&o.frames[i].1).summary;
    assert!(sum(0).starts_with("UDP 0.0.0.0:68 > 255.255.255.255:67"));
    assert!(sum(1).starts_with("UDP 10.0.2.2:67 > 255.255.255.255:68"));
    assert_eq!(sum(4), "ARP who-has 10.0.2.2 tell 10.0.2.15");
    assert_eq!(sum(5), "ARP reply 10.0.2.2 is-at 52:55:0a:00:02:02");
    assert_eq!(
        sum(7),
        "ICMP echo-reply 10.0.2.2 > 10.0.2.15 id=0x2b2b seq=0 data=32"
    );
    // Virtual time moves forward, and the reply follows its request closely
    // (it is produced inside the transmitting instruction).
    let times: Vec<u64> = o.frames.iter().map(|(t, _)| *t).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    assert!(*times.last().unwrap() <= o.end_ns);
}

#[test]
fn two_runs_produce_identical_frames_times_and_uart() {
    let (Some(a), Some(b)) = (
        config("det-a", NetConfig::default()),
        config("det-b", NetConfig::default()),
    ) else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let x = run_net(&a);
    let y = run_net(&b);
    assert_eq!(x.uart, y.uart, "UART (including the measured round trips)");
    assert_eq!(x.frames, y.frames, "frame bytes or virtual times diverged");
    assert_eq!(x.end_ns, y.end_ns);
}

/// The frames' virtual times come from QEMU's own stamps, so how the host
/// slices its run requests must not change them (unlike UART bytes, which
/// are attributed to the slice they were found in).
#[test]
fn frame_times_do_not_depend_on_how_the_run_is_sliced() {
    let (Some(a), Some(b)) = (
        config("slice-a", NetConfig::default()),
        config("slice-b", NetConfig::default()),
    ) else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let fine = run_net_in(&a, 7_000);
    let coarse = run_net_in(&b, 1_000_000);
    assert_eq!(fine.frames.len(), 14);
    assert_eq!(fine.frames, coarse.frames);
}

/// With no `[network]` the GEM has no netdev (`-net none`; QEMU would attach
/// an implicit user-mode network otherwise): nothing answers and nothing is
/// captured. QEMU's PHY model still reports link up, there being no cable
/// detection in it.
#[test]
fn an_unplugged_nic_gets_no_answers_and_captures_nothing() {
    let Some(mut cfg) = config("unplugged", NetConfig::default()) else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    cfg.net = None;
    let o = run_net_in(&cfg, 1_000_000);
    assert!(o.uart.contains("link=up"), "{}", o.uart);
    assert!(o.uart.contains("DHCP: no answer"), "{}", o.uart);
    assert!(o.uart.contains("0/4 replies"), "{}", o.uart);
    assert!(o.uart.contains("rx=0"), "nothing is delivered: {}", o.uart);
    assert!(o.uart.contains("RESULT: FAIL"), "{}", o.uart);
    assert!(o.frames.is_empty(), "{:?}", o.frames);
}

/// `restrict = false` changes what the DHCP server offers (a router and DNS)
/// and lets the guest reach the host's network; the gateway still answers
/// ping itself, so this does not depend on the real network being there.
#[test]
fn an_open_network_offers_a_router_and_the_gateway_still_answers() {
    let net = NetConfig {
        restrict: false,
        ..NetConfig::default()
    };
    let Some(cfg) = config("open", net) else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let o = run_net(&cfg);
    assert!(
        o.uart
            .contains("DHCP: address=10.0.2.15/24 server=10.0.2.2 router=10.0.2.2"),
        "{}",
        o.uart
    );
    assert!(o.uart.contains("RESULT: ALL PASS"), "{}", o.uart);
}

#[test]
fn a_custom_subnet_and_mac_reach_the_firmware_and_the_wire() {
    let net = NetConfig {
        mac: [0x02, 0x11, 0x22, 0x33, 0x44, 0x55],
        subnet: "192.168.50.0".parse().unwrap(),
        prefix: 24,
        gateway: "192.168.50.2".parse().unwrap(),
        restrict: true,
    };
    let Some(cfg) = config("custom", net) else {
        eprintln!("SKIPPED: Xilinx QEMU / co-sim DTB not found (set ZYNQSIM_QEMU)");
        return;
    };
    let o = run_net(&cfg);
    assert!(o.uart.contains("MAC: 02:11:22:33:44:55"), "{}", o.uart);
    assert!(
        o.uart
            .contains("DHCP: address=192.168.50.15/24 server=192.168.50.2"),
        "{}",
        o.uart
    );
    assert!(o.uart.contains("ping target 192.168.50.2"), "{}", o.uart);
    assert!(o.uart.contains("RESULT: ALL PASS"), "{}", o.uart);
    let arp = o
        .frames
        .iter()
        .map(|(_, f)| describe(f))
        .find(|i| i.kind == FrameKind::Arp)
        .expect("an ARP frame");
    assert_eq!(arp.summary, "ARP who-has 192.168.50.2 tell 192.168.50.15");
    // Every frame the board sent carries the configured MAC as its source.
    for (_, f) in o
        .frames
        .iter()
        .filter(|(_, f)| direction(f) == Direction::Tx)
    {
        assert_eq!(&f[6..12], &[0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
    }
}
