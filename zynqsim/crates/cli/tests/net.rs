//! End-to-end tests of the board's Ethernet through the `zynqsim` binary: the
//! Rust firmware gets an address by DHCP and pings the simulated gateway, the
//! host exports a `.pcap`, and the result is the same on every run.
//!
//! They need the Xilinx QEMU; without it they print `SKIPPED:` and return.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-cli-net-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn qemu_available() -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

fn example(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .join("project.toml")
        .to_string_lossy()
        .into_owned()
}

/// `(timestamp in microseconds, frame)` for every record of a classic
/// little-endian pcap file.
fn read_pcap(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
    assert_eq!(&bytes[..4], &[0xd4, 0xc3, 0xb2, 0xa1], "pcap magic");
    assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 1);
    let mut at = 24;
    let mut out = Vec::new();
    while at + 16 <= bytes.len() {
        let w = |o: usize| u32::from_le_bytes(bytes[at + o..at + o + 4].try_into().unwrap());
        let (sec, usec, len) = (u64::from(w(0)), u64::from(w(4)), w(8) as usize);
        out.push((
            sec * 1_000_000 + usec,
            bytes[at + 16..at + 16 + len].to_vec(),
        ));
        at += 16 + len;
    }
    assert_eq!(at, bytes.len(), "no trailing bytes");
    out
}

/// ICMP echo `(type, ident, seq, payload)` of an Ethernet+IPv4 frame, if it is one.
fn icmp_echo(f: &[u8]) -> Option<(u8, u16, u16, Vec<u8>)> {
    if f.len() < 42 || f[12..14] != [0x08, 0x00] || f[23] != 1 {
        return None;
    }
    let ihl = usize::from(f[14] & 0x0f) * 4;
    let icmp = &f[14 + ihl..];
    if !matches!(icmp[0], 0 | 8) {
        return None;
    }
    let total = usize::from(u16::from_be_bytes([f[16], f[17]]));
    let end = (14 + total).min(f.len());
    Some((
        icmp[0],
        u16::from_be_bytes([icmp[4], icmp[5]]),
        u16::from_be_bytes([icmp[6], icmp[7]]),
        f[14 + ihl + 8..end].to_vec(),
    ))
}

fn run_demo(extra: &[&str]) -> Value {
    let project = example("net-demo");
    let mut args = vec!["--json", "run", "--project", &project, "--for", "10ms"];
    args.extend_from_slice(extra);
    let o = zynqsim(&args);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    serde_json::from_str(&stdout(&o)).unwrap()
}

#[test]
fn net_demo_pings_the_gateway_and_exports_a_matching_pcap() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found (PATH, Xilinx install dirs or ZYNQSIM_QEMU)");
        return;
    }
    let d = temp("ping");
    let pcap = d.join("net.pcap");
    let v = run_demo(&["--pcap", pcap.to_str().unwrap()]);

    assert_eq!(v["backend"]["ps"], "qemu");
    assert_eq!(v["backend"]["fidelity"], "f0");
    let uart = v["uart"]["1"].as_str().unwrap();
    for seq in 0..4 {
        assert!(
            uart.contains(&format!("ping reply seq={seq} bytes=32 from=10.0.2.2")),
            "{uart}"
        );
    }
    assert!(
        uart.contains("DHCP: address=10.0.2.15/24 server=10.0.2.2"),
        "{uart}"
    );
    assert!(uart.contains("RESULT: ALL PASS"), "{uart}");

    // Counters: DISCOVER+OFFER+REQUEST+ACK, one ARP pair, four echo pairs.
    let n = &v["network"];
    assert_eq!(n["frames_tx"], 7, "{n}");
    assert_eq!(n["frames_rx"], 7, "{n}");
    assert_eq!(n["udp"], 4);
    assert_eq!(n["arp"], 2);
    assert_eq!(n["icmp"], 8);
    assert_eq!(n["bad_checksums"], 0);
    assert_eq!(n["mac"], "00:0a:35:00:1e:53");
    assert_eq!(n["restricted"], true);

    // The pcap holds the same fourteen frames, in time order, and every echo
    // request is followed by a reply with the same identifier, sequence number
    // and payload.
    let frames = read_pcap(&std::fs::read(&pcap).unwrap());
    assert_eq!(frames.len(), 14);
    assert!(frames.windows(2).all(|w| w[0].0 <= w[1].0));
    let echoes: Vec<_> = frames.iter().filter_map(|(_, f)| icmp_echo(f)).collect();
    assert_eq!(echoes.len(), 8);
    for pair in echoes.chunks(2) {
        let (req, rep) = (&pair[0], &pair[1]);
        assert_eq!((req.0, rep.0), (8, 0), "request then reply");
        assert_eq!((req.1, req.2), (rep.1, rep.2), "same id and seq");
        assert_eq!(req.3, rep.3, "payload echoed unchanged");
        assert_eq!(req.3.len(), 32);
    }
    let seqs: Vec<u16> = echoes.iter().step_by(2).map(|e| e.2).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3]);
    // The first frame is the DHCP DISCOVER from the board's MAC.
    let first = &frames[0].1;
    assert_eq!(&first[0..6], &[0xff; 6]);
    assert_eq!(&first[6..12], &[0x00, 0x0a, 0x35, 0x00, 0x1e, 0x53]);
}

#[test]
fn two_processes_agree_on_digest_pcap_and_uart() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("det");
    let (a, b) = (d.join("a.pcap"), d.join("b.pcap"));
    let va = run_demo(&["--pcap", a.to_str().unwrap()]);
    let vb = run_demo(&["--pcap", b.to_str().unwrap()]);
    assert_eq!(va["trace_hash"], vb["trace_hash"]);
    assert_eq!(va["now_ps"], vb["now_ps"]);
    assert_eq!(va["uart"]["1"], vb["uart"]["1"], "UART incl. measured RTTs");
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "the exported capture is byte-identical"
    );
}

#[test]
fn a_network_session_records_and_replays_to_the_same_digest() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("rec");
    let rec = d.join("r.json");
    let v = run_demo(&["--record", rec.to_str().unwrap()]);
    let o = zynqsim(&["--json", "test", rec.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let r: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(r["matches"], true, "{r}");
    assert_eq!(r["trace_hash"], v["trace_hash"]);
}

#[test]
fn network_frames_are_in_the_trace_and_the_vcd() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("trace");
    let vcd = d.join("t.vcd");
    let perfetto = d.join("t.json");
    run_demo(&[
        "--vcd",
        vcd.to_str().unwrap(),
        "--perfetto",
        perfetto.to_str().unwrap(),
    ]);
    let vcd = std::fs::read_to_string(&vcd).unwrap();
    assert!(vcd.contains("net.tx_frames") && vcd.contains("net.rx_frames"));
    let p = std::fs::read_to_string(&perfetto).unwrap();
    assert!(p.contains("net tx 42B ARP who-has 10.0.2.2 tell 10.0.2.15"), "ARP marker");
    assert!(p.contains("ICMP echo-request 10.0.2.15 > 10.0.2.2 id=0x2b2b seq=3"));
}

#[test]
fn pcap_without_a_network_section_is_unsupported_not_silent() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("nonet");
    let o = zynqsim(&[
        "run",
        "--project",
        &example("rust-hello"),
        "--for",
        "2ms",
        "--pcap",
        d.join("x.pcap").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
    assert!(stderr(&o).contains("[network]"), "{}", stderr(&o));
    assert!(!d.join("x.pcap").exists());
}

#[test]
fn a_bad_network_section_is_rejected_with_every_field_listed() {
    let d = temp("bad");
    let p = d.join("project.toml");
    let fw = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/elf/rust_net.elf")
        .canonicalize()
        .unwrap();
    std::fs::write(
        &p,
        format!(
            "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"qemu\"\n\
             [cpu0]\nelf = {:?}\n[network]\nmac = \"zz:00\"\nsubnet = \"10.0.2.0/30\"\n\
             gateway = \"nope\"\n",
            fw.to_string_lossy()
        ),
    )
    .unwrap();
    let o = zynqsim(&["run", "--project", p.to_str().unwrap(), "--for", "1ms"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let e = stderr(&o);
    for field in ["network.mac", "network.subnet", "network.gateway"] {
        assert!(e.contains(field), "{field} missing in {e}");
    }
    // `inspect` lists them as findings without starting anything.
    let o = zynqsim(&["--json", "inspect", "--project", p.to_str().unwrap()]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(
        v.to_string().contains("network.mac"),
        "inspect output: {v}"
    );
}
