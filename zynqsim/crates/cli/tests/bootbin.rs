//! `zynqsim boot-info` and `init --boot`, and a project booted from a real `bootgen` BOOT.BIN
//! (`fixtures/boot/BOOT.BIN`, ADR-018) against the same firmware booted as an ELF.
//!
//! The end-to-end test needs the Xilinx QEMU and prints `SKIPPED:` without it.

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
    let d = std::env::temp_dir().join(format!("zynqsim-cli-boot-{tag}-{}", std::process::id()));
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

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn boot_bin() -> PathBuf {
    root().join("fixtures/boot/BOOT.BIN")
}

fn s(p: &std::path::Path) -> &str {
    p.to_str().unwrap()
}

fn qemu_found() -> bool {
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

#[test]
fn boot_info_lists_the_partitions_of_a_real_image() {
    let o = zynqsim(&["boot-info", s(&boot_bin())]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let t = stdout(&o);
    for needle in [
        "fsbl_stub.elf",
        "dummy.bit",
        "bitstream",
        "application",
        "0xf8000120 <- 0x1f000200",
        "entry (core 0): 0x00100000",
        "runnable: yes",
        "not executed",
    ] {
        assert!(t.contains(needle), "missing `{needle}`:\n{t}");
    }

    let o = zynqsim(&["--json", "boot-info", s(&boot_bin())]);
    assert_eq!(o.status.code(), Some(0));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], true);
    assert_eq!(v["entry"], "0x00100000");
    assert_eq!(v["partitions"].as_array().unwrap().len(), 4);
    assert_eq!(v["partitions"][2]["exec_address"], "0x00100000");
    assert_eq!(v["partitions"][2]["checksum_ok"], true);
    assert_eq!(v["fsbl"]["executed"], false);
    assert_eq!(v["register_init_applied"], false);
}

#[test]
fn boot_info_names_what_is_wrong_and_exits_2() {
    let d = temp("bad");
    let good = std::fs::read(boot_bin()).unwrap();

    let mut damaged = good.clone();
    damaged[0x44] ^= 1;
    let f = d.join("damaged.bin");
    std::fs::write(&f, &damaged).unwrap();
    let o = zynqsim(&["boot-info", s(&f)]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stdout(&o).contains("boot_header.checksum"),
        "{}",
        stdout(&o)
    );
    assert!(stdout(&o).contains("runnable: NO"), "{}", stdout(&o));

    let mut enc = good.clone();
    enc[0x28..0x2C].copy_from_slice(&0x3A5C_3C5Au32.to_le_bytes());
    let sum = !enc[0x20..0x48].chunks(4).fold(0u32, |a, w| {
        a.wrapping_add(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
    });
    enc[0x48..0x4C].copy_from_slice(&sum.to_le_bytes());
    let f = d.join("encrypted.bin");
    std::fs::write(&f, &enc).unwrap();
    let o = zynqsim(&["--json", "boot-info", s(&f)]);
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["encrypted"], true);
    assert_eq!(v["runnable"], false);
    assert!(
        v["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("encrypted boot is not supported"),
        "{v}"
    );

    let f = d.join("truncated.bin");
    std::fs::write(&f, &good[..0x500]).unwrap();
    let o = zynqsim(&["boot-info", s(&f)]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("boot header"), "{}", stderr(&o));

    let o = zynqsim(&["boot-info", s(&root().join("fixtures/elf/rust_hello.elf"))]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("not a Zynq BootROM image"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn init_boot_writes_a_valid_project_and_never_overwrites() {
    let d = temp("init");
    let o = zynqsim(&[
        "--json",
        "init",
        "--boot",
        s(&boot_bin()),
        "--out",
        s(&d.join("p")),
    ]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["runnable"], true);
    assert_eq!(v["boot"]["entry"], "0x00100000");
    let toml = std::fs::read_to_string(d.join("p/project.toml")).unwrap();
    assert!(toml.contains("mode = \"boot_bin\""), "{toml}");
    assert!(toml.contains("file = \"BOOT.BIN\""), "{toml}");
    assert!(toml.contains("not executed"), "{toml}");
    assert!(d.join("p/BOOT.BIN").is_file());
    assert_eq!(
        std::fs::read(d.join("p/BOOT.BIN")).unwrap(),
        std::fs::read(boot_bin()).unwrap()
    );
    let o = zynqsim(&["inspect", "--project", s(&d.join("p/project.toml"))]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));

    let again = zynqsim(&["init", "--boot", s(&boot_bin()), "--out", s(&d.join("p"))]);
    assert_eq!(again.status.code(), Some(2));
    assert!(stderr(&again).contains("refusing to overwrite"));

    // A damaged image creates nothing.
    let mut bad = std::fs::read(boot_bin()).unwrap();
    bad[0x44] ^= 1;
    let bf = d.join("bad.bin");
    std::fs::write(&bf, bad).unwrap();
    let o = zynqsim(&["init", "--boot", s(&bf), "--out", s(&d.join("q"))]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("boot_header.checksum"),
        "{}",
        stderr(&o)
    );
    assert!(!d.join("q/project.toml").exists());
}

#[test]
fn project_validation_reports_each_boot_problem_with_its_field() {
    let d = temp("validate");
    let write = |name: &str, body: &str| {
        let p = d.join(name);
        std::fs::write(&p, format!("schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"qemu\"\npl = \"demo\"\n{body}")).unwrap();
        p
    };
    let check = |p: &std::path::Path| -> Value {
        let o = zynqsim(&["--json", "inspect", "--project", s(p)]);
        serde_json::from_str(&stdout(&o)).unwrap_or_else(|_| panic!("{}", stderr(&o)))
    };
    let find = |v: &Value, field: &str| -> Option<(String, String)> {
        v["diagnostics"]
            .as_array()?
            .iter()
            .find(|d| d["field"] == field)
            .map(|d| {
                (
                    d["severity"].as_str().unwrap().to_string(),
                    d["message"].as_str().unwrap().to_string(),
                )
            })
    };

    // No file.
    let v = check(&write("nofile.toml", "[boot]\nmode = \"bootbin\"\n"));
    let (sev, m) = find(&v, "boot.file").expect("boot.file diagnosed");
    assert_eq!(sev, "error");
    assert!(m.contains("required"), "{m}");
    assert!(find(&v, "cpu0.elf").is_none(), "no ELF is needed: {v}");

    // Missing file.
    let v = check(&write(
        "missing.toml",
        "[boot]\nmode = \"boot_bin\"\nfile = \"nope.bin\"\n",
    ));
    assert!(find(&v, "boot.file").unwrap().1.contains("does not exist"));

    // A damaged file: the finding is carried with its offset.
    let mut bad = std::fs::read(boot_bin()).unwrap();
    bad[0x44] ^= 1;
    std::fs::write(d.join("bad.bin"), bad).unwrap();
    let p = write(
        "bad.toml",
        "[boot]\nmode = \"boot_bin\"\nfile = \"bad.bin\"\n",
    );
    let v = check(&p);
    let (sev, m) = find(&v, "boot.file").unwrap();
    assert_eq!(sev, "error");
    assert!(
        m.contains("boot_header.checksum") && m.contains("0x48"),
        "{m}"
    );
    let r = zynqsim(&["run", "--project", s(&p), "--for", "1ms"]);
    assert_eq!(r.status.code(), Some(1), "{}", stderr(&r));
    assert!(stderr(&r).contains("boot.file"), "{}", stderr(&r));

    // A cpu0 ELF next to a boot image is ignored, loudly.
    std::fs::copy(boot_bin(), d.join("BOOT.BIN")).unwrap();
    let v = check(&write(
        "both.toml",
        &format!(
            "[boot]\nmode = \"boot_bin\"\nfile = \"BOOT.BIN\"\n[cpu0]\nelf = \"{}\"\n",
            root()
                .join("fixtures/elf/rust_hello.elf")
                .to_str()
                .unwrap()
                .replace('\\', "/")
        ),
    ));
    let (sev, m) = find(&v, "cpu0.elf").unwrap();
    assert_eq!(sev, "warning");
    assert!(m.contains("ignored"), "{m}");
    assert!(find(&v, "boot.file").is_none(), "{v}");
}

fn run_uart(project: &std::path::Path) -> String {
    let r = zynqsim(&["--json", "run", "--project", s(project), "--for", "5ms"]);
    assert_eq!(r.status.code(), Some(0), "{} {}", stdout(&r), stderr(&r));
    let v: Value = serde_json::from_str(&stdout(&r)).unwrap();
    v["uart"]["1"].as_str().unwrap().to_string()
}

/// The firmware of `examples/rust-hello`, once as an ELF and once inside a BOOT.BIN that
/// bootgen made from that same ELF: the PS prints the same text.
#[test]
fn a_bootbin_project_prints_what_the_elf_prints() {
    if !qemu_found() {
        eprintln!("SKIPPED: needs the Xilinx QEMU");
        return;
    }
    let direct = run_uart(&root().join("examples/rust-hello/project.toml"));
    assert!(!direct.trim().is_empty(), "the ELF printed nothing");

    let d = temp("e2e");
    let o = zynqsim(&["init", "--boot", s(&boot_bin()), "--out", s(&d)]);
    assert_eq!(o.status.code(), Some(0), "{} {}", stdout(&o), stderr(&o));
    let booted = run_uart(&d.join("project.toml"));
    assert_eq!(booted, direct, "BOOT.BIN and ELF boots differ");
    eprintln!("UART (both routes): {booted:?}");
}
