//! End-to-end tests of the board's microSD slot and QSPI flash (ADR-012):
//! host image tools, project validation and Rust firmware running on the real
//! Xilinx QEMU that reads the card through SDHCI + FAT and the flash through
//! the QSPI controller.
//!
//! Tests that need QEMU print `SKIPPED:` and return when it is not installed.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn zynqsim(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zynqsim"))
        .args(args)
        .output()
        .expect("zynqsim runs")
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zynqsim-storage-{tag}-{}", std::process::id()));
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
    if std::env::var("ZYNQSIM_FAST").as_deref() == Ok("1") {
        eprintln!("SKIPPED: fast mode (ZYNQSIM_FAST=1)");
        return false;
    }
    let o = zynqsim(&["--json", "inspect"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    v["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "qemu" && b["found"] == true)
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A path as TOML wants it (forward slashes, which Windows accepts too).
fn toml_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn elf(name: &str) -> String {
    toml_path(&root().join("fixtures/elf").join(name))
}

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811C_9DC5u32, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// Pseudo-random bytes (xorshift), so a misplaced cluster shows up.
fn noise(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 9) as u8
        })
        .collect()
}

/// A project that runs `firmware` with `[storage]` lines.
fn project(dir: &Path, firmware: &str, storage: &str) -> PathBuf {
    let p = dir.join("project.toml");
    std::fs::write(
        &p,
        format!(
            "schema_version = 1\n[board]\nname = \"zybo-z7-20\"\n[backends]\nps = \"qemu\"\n\
             pl = \"demo\"\n[sim]\nquantum_ns = 1000\n[boot]\nmode = \"elf\"\n\
             [cpu0]\nelf = \"{}\"\n[storage]\n{storage}\n",
            elf(firmware)
        ),
    )
    .unwrap();
    p
}

/// Runs a project and returns the parsed `--json` result.
fn run_json(project: &Path, duration: &str) -> Value {
    let o = zynqsim(&[
        "--json",
        "run",
        "--project",
        &project.to_string_lossy(),
        "--for",
        duration,
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    serde_json::from_str(&stdout(&o)).unwrap()
}

fn uart(v: &Value) -> String {
    v["uart"]["1"].as_str().unwrap().replace("\r\n", "\n")
}

// ------------------------------------------------------------ host tooling

#[test]
fn image_tools_build_card_and_flash_images_and_refuse_bad_sizes() {
    let d = temp("tools");
    let files = d.join("files");
    std::fs::create_dir_all(files.join("SUB")).unwrap();
    std::fs::write(files.join("A.TXT"), b"alpha").unwrap();
    std::fs::write(files.join("SUB/B.BIN"), noise(5000, 3)).unwrap();

    let sd = d.join("sd.img");
    let o = zynqsim(&[
        "--json",
        "sd-image",
        "--dir",
        &files.to_string_lossy(),
        "--out",
        &sd.to_string_lossy(),
        "--size-mb",
        "16",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["bytes"], 16 << 20);
    assert_eq!(v["fat"], "FAT16");
    assert_eq!(v["files"], 2);
    // MBR signature and partition type of a card a PC would have formatted.
    let img = std::fs::read(&sd).unwrap();
    assert_eq!(&img[510..512], &[0x55, 0xAA]);
    assert_eq!(img[446 + 4], 0x0E);

    // Sizes QEMU's SD model would reject, and FAT32 on a card that is too small.
    for size in ["10", "4", "2048"] {
        let o = zynqsim(&[
            "sd-image",
            "--dir",
            &files.to_string_lossy(),
            "--out",
            &d.join("bad.img").to_string_lossy(),
            "--size-mb",
            size,
        ]);
        assert_eq!(o.status.code(), Some(2), "size {size}");
        assert!(stderr(&o).contains("power of two"), "{}", stderr(&o));
    }
    let o = zynqsim(&[
        "sd-image",
        "--dir",
        &files.to_string_lossy(),
        "--out",
        &d.join("bad.img").to_string_lossy(),
        "--size-mb",
        "8",
        "--fat",
        "32",
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("64 MiB"), "{}", stderr(&o));

    let data = d.join("data.bin");
    std::fs::write(&data, noise(300, 4)).unwrap();
    let flash = d.join("flash.bin");
    let o = zynqsim(&[
        "--json",
        "qspi-image",
        "--file",
        &data.to_string_lossy(),
        "--offset",
        "0x100000",
        "--out",
        &flash.to_string_lossy(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let f = std::fs::read(&flash).unwrap();
    assert_eq!(f.len(), 16 << 20);
    assert!(f[..0x10_0000].iter().all(|&b| b == 0xFF));
    assert_eq!(&f[0x10_0000..0x10_0000 + 300], &noise(300, 4)[..]);
    // Past the end of the flash.
    let o = zynqsim(&[
        "qspi-image",
        "--file",
        &data.to_string_lossy(),
        "--offset",
        "0xFFFFF0",
        "--out",
        &d.join("bad.bin").to_string_lossy(),
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("past the 16 MiB flash"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn a_project_that_references_a_missing_or_wrong_image_fails_with_a_clear_error() {
    let d = temp("missing");
    // Validation happens before any tool is started, so this needs no QEMU.
    let p = project(
        &d,
        "rust_sd.elf",
        "sd_image = \"nope.img\"\nqspi_image = \"nope.bin\"\n",
    );
    let o = zynqsim(&["run", "--project", &p.to_string_lossy(), "--for", "1ms"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let e = stderr(&o);
    assert!(
        e.contains("storage.sd_image: `nope.img` does not exist"),
        "{e}"
    );
    assert!(
        e.contains("storage.qspi_image: `nope.bin` does not exist"),
        "{e}"
    );

    // Present but unusable: not a power of two / not 16 MiB.
    std::fs::write(d.join("odd.img"), vec![0u8; 10 << 20]).unwrap();
    std::fs::write(d.join("short.bin"), vec![0xFFu8; 1 << 20]).unwrap();
    let p = project(
        &d,
        "rust_sd.elf",
        "sd_image = \"odd.img\"\nqspi_image = \"short.bin\"\n",
    );
    let o = zynqsim(&["run", "--project", &p.to_string_lossy(), "--for", "1ms"]);
    assert_eq!(o.status.code(), Some(1));
    let e = stderr(&o);
    assert!(e.contains("power-of-two"), "{e}");
    assert!(e.contains("exactly 16777216 bytes"), "{e}");

    // `inspect` lists the same findings without starting anything.
    let o = zynqsim(&["inspect", "--project", &p.to_string_lossy()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("storage.sd_image"), "{}", stdout(&o));
}

// ---------------------------------------------------------------------- SD

/// The files of the test card: (path on the card, contents).
fn card_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "HELLO.TXT",
            b"Hello from the host!\nThe quick brown fox jumps over 13 lazy dogs.\n\
              Line three: 0123456789 ABCDEF.\n"
                .to_vec(),
        ),
        // Eighty clusters' worth of a FAT16 volume this size: the chain has to
        // be followed.
        ("BIG.BIN", noise(40_000, 11)),
        ("A long file name.txt", b"long name".to_vec()),
        ("DOCS/NOTES.TXT", b"in a sub-directory\n".to_vec()),
    ]
}

fn build_card(d: &Path) -> PathBuf {
    let folder = d.join("card");
    for (path, data) in card_files() {
        let p = folder.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
    let img = d.join("sd.img");
    let o = zynqsim(&[
        "sd-image",
        "--dir",
        &folder.to_string_lossy(),
        "--out",
        &img.to_string_lossy(),
        "--size-mb",
        "16",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    img
}

/// Rust firmware initialises the card through SDHCI, mounts the FAT volume
/// with `embedded-sdmmc` and prints a file the host put there.
#[test]
fn sd_demo_reads_a_file_from_the_card_through_sdhci_and_fat() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("sd");
    let img = build_card(&d);
    let before = std::fs::read(&img).unwrap();
    let p = project(&d, "rust_sd.elf", "sd_image = \"sd.img\"\n");

    let v = run_json(&p, "10ms");
    let u = uart(&v);
    let blocks = before.len() / 512;
    assert!(
        u.contains(&format!("card: SDSC {blocks} blocks ({} KiB)", blocks / 2)),
        "capacity from the CSD must equal the image size: {u}"
    );
    assert!(
        u.contains("mbr: signature 0xaa55, partition 1 type 0x0e start 2048"),
        "{u}"
    );
    assert!(
        u.contains("raw write: block 1 written and read back, 512 bytes equal"),
        "{u}"
    );
    // The exact content of HELLO.TXT, between the markers.
    let hello = String::from_utf8(card_files()[0].1.clone()).unwrap();
    assert!(
        u.contains(&format!(
            "---- HELLO.TXT ----\n{hello}---- end HELLO.TXT ----"
        )),
        "{u}"
    );
    // Every file was read back: length and FNV-1a over the file contents.
    for (path, data) in card_files() {
        if path.starts_with("A long") {
            // Listed with its long name; its 8.3 alias is the crate's choice.
            assert!(u.contains("(A long file name.txt)"), "{u}");
            assert!(
                u.contains(&format!("size={} fnv1a={:#010x}", data.len(), fnv1a(&data))),
                "{u}"
            );
            continue;
        }
        let short = path.rsplit('/').next().unwrap();
        let shown = if path.contains('/') {
            format!("DOCS/{short}")
        } else {
            short.to_string()
        };
        assert!(
            u.contains(&format!(
                "FILE {shown} size={} fnv1a={:#010x}",
                data.len(),
                fnv1a(&data)
            )),
            "{path}: {u}"
        );
    }
    assert!(u.contains("4 file(s) read"), "{u}");
    assert!(u.contains("RESULT: ALL PASS"), "{u}");

    // The status says what is attached, and read-only means read-only: the
    // firmware wrote a block (above), yet the host file is byte-identical.
    assert_eq!(v["storage"]["sd"]["source"], "sd.img");
    assert_eq!(v["storage"]["sd"]["bytes"], 16 << 20);
    assert_eq!(v["storage"]["sd"]["writable"], false);
    assert_eq!(std::fs::read(&img).unwrap(), before);

    // Same inputs, same trace.
    let again = run_json(&p, "10ms");
    assert_eq!(again["trace_hash"], v["trace_hash"]);
    assert_eq!(uart(&again), u);
}

#[test]
fn writable_storage_changes_the_image_and_refuses_what_cannot_be_repeated() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("sd-rw");
    let img = build_card(&d);
    let before = std::fs::read(&img).unwrap();
    let p = project(
        &d,
        "rust_sd.elf",
        "sd_image = \"sd.img\"\nwritable = true\n",
    );

    // `--record` needs a repeatable session; a writable card is not one.
    let o = zynqsim(&[
        "run",
        "--project",
        &p.to_string_lossy(),
        "--for",
        "10ms",
        "--record",
        &d.join("r.json").to_string_lossy(),
    ]);
    assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
    assert!(stderr(&o).contains("writable"), "{}", stderr(&o));
    // The refusal came after the run: the card was written.
    let after_refused = std::fs::read(&img).unwrap();
    assert_ne!(after_refused, before);

    std::fs::write(&img, &before).unwrap();
    let v = run_json(&p, "10ms");
    assert!(uart(&v).contains("RESULT: ALL PASS"), "{}", uart(&v));
    assert_eq!(v["storage"]["sd"]["writable"], true);
    let after = std::fs::read(&img).unwrap();
    assert_eq!(after.len(), before.len());
    // Exactly block 1 changed, to the firmware's pattern.
    let want: Vec<u8> = (0..512u32)
        .map(|i| i.wrapping_mul(5).wrapping_add(0x33) as u8)
        .collect();
    assert_eq!(&after[512..1024], &want[..], "block 1");
    assert_eq!(&after[..512], &before[..512]);
    assert_eq!(&after[1024..], &before[1024..]);
}

/// The card can also come straight from a folder (`sd_dir`), which is how the
/// example project avoids committing an image.
#[test]
fn the_sd_demo_example_builds_its_card_from_a_folder() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let p = root().join("examples/sd-demo/project.toml");
    let v = run_json(&p, "10ms");
    let u = uart(&v);
    let hello = std::fs::read_to_string(root().join("examples/sd-demo/sdcard/HELLO.TXT")).unwrap();
    let hello = hello.replace("\r\n", "\n");
    assert!(
        u.contains(&format!(
            "---- HELLO.TXT ----\n{hello}---- end HELLO.TXT ----"
        )),
        "{u}"
    );
    assert!(u.contains("/DOCS/NOTES.TXT"), "{u}");
    assert!(u.contains("RESULT: ALL PASS"), "{u}");
    assert_eq!(v["storage"]["sd"]["source"], "dir:sdcard");
}

// -------------------------------------------------------------------- QSPI

/// The flash image of the QSPI tests: noise at 0x100000, erased elsewhere.
fn flash_with_data(d: &Path) -> (PathBuf, Vec<u8>) {
    let data = noise(256, 21);
    let src = d.join("data.bin");
    std::fs::write(&src, &data).unwrap();
    let flash = d.join("flash.bin");
    let o = zynqsim(&[
        "qspi-image",
        "--file",
        &src.to_string_lossy(),
        "--offset",
        "0x100000",
        "--out",
        &flash.to_string_lossy(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    (flash, data)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Rust firmware reads the JEDEC ID and the data through the controller's
/// FIFOs and again through the linear window, then erases and programs.
#[test]
fn qspi_demo_reads_the_jedec_id_and_flash_data_in_fifo_and_linear_mode() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("qspi");
    let (flash, data) = flash_with_data(&d);
    let before = std::fs::read(&flash).unwrap();
    let p = project(&d, "rust_qspi.elf", "qspi_image = \"flash.bin\"\n");

    let v = run_json(&p, "10ms");
    let u = uart(&v);
    // Spansion S25FL128S family: manufacturer 01, type 20, capacity 18.
    assert!(u.contains("RDID: 01 20 18 4d 00"), "{u}");
    assert!(
        u.contains("JEDEC: manufacturer 0x01 type 0x20 capacity code 0x18 = 16777216 bytes"),
        "{u}"
    );
    let want = hex(&data[..64]);
    assert!(
        u.contains(&format!("FIFO read @0x00100000: {want}\n")),
        "{u}"
    );
    assert!(
        u.contains(&format!("LINEAR read @0xfc100000: {want}\n")),
        "{u}"
    );
    assert!(u.contains("FIFO and linear reads agree (64 bytes)"), "{u}");
    assert!(
        u.contains("ERASE sector at 0x00100000: 64 bytes read back as 0xFF"),
        "{u}"
    );
    assert!(
        u.contains("PROGRAM 16 bytes at 0x00100000, read back: a0a1a2a3a4a5a6a7a8a9aaabacadaeaf"),
        "{u}"
    );
    assert!(u.contains("RESULT: ALL PASS"), "{u}");

    // The firmware erased and programmed the flash; the host file is intact.
    assert_eq!(v["storage"]["qspi"]["source"], "flash.bin");
    assert_eq!(v["storage"]["qspi"]["model"], "s25fl129p0");
    assert_eq!(v["storage"]["qspi"]["writable"], false);
    assert_eq!(std::fs::read(&flash).unwrap(), before);

    let again = run_json(&p, "10ms");
    assert_eq!(again["trace_hash"], v["trace_hash"]);
}

/// With `writable = true` the erase and program reach the file.
#[test]
fn writable_flash_is_programmed_in_the_image_file() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    let d = temp("qspi-rw");
    let (flash, _data) = flash_with_data(&d);
    let before = std::fs::read(&flash).unwrap();
    let p = project(
        &d,
        "rust_qspi.elf",
        "qspi_image = \"flash.bin\"\nwritable = true\n",
    );
    let v = run_json(&p, "10ms");
    assert!(uart(&v).contains("RESULT: ALL PASS"), "{}", uart(&v));
    let after = std::fs::read(&flash).unwrap();
    assert_eq!(after.len(), before.len());
    let pat: Vec<u8> = (0..16u8).map(|i| 0xA0 + i).collect();
    // The erased sector (256 KiB for this model) holds the pattern at its
    // start and is 0xFF behind it; everything outside it is unchanged.
    let sector = 0x10_0000..0x10_0000 + (256 << 10);
    assert_eq!(&after[sector.start..sector.start + 16], &pat[..]);
    assert!(
        after[sector.start + 16..sector.end]
            .iter()
            .all(|&b| b == 0xFF)
    );
    assert_eq!(&after[..sector.start], &before[..sector.start]);
    assert_eq!(&after[sector.end..], &before[sector.end..]);
}

#[test]
fn the_qspi_demo_example_and_each_flash_model_report_their_own_id() {
    if !qemu_available() {
        eprintln!("SKIPPED: Xilinx QEMU not found");
        return;
    }
    // The example builds its flash from a data file.
    let p = root().join("examples/qspi-demo/project.toml");
    let v = run_json(&p, "10ms");
    let u = uart(&v);
    let data = std::fs::read(root().join("examples/qspi-demo/flash-data.bin")).unwrap();
    assert!(
        u.contains(&format!("FIFO read @0x00100000: {}\n", hex(&data[..64]))),
        "{u}"
    );
    assert!(u.contains("RESULT: ALL PASS"), "{u}");
    assert_eq!(v["storage"]["qspi"]["source"], "file:flash-data.bin");

    // Each selectable part answers with its own manufacturer.
    let d = temp("models");
    let (flash, _) = flash_with_data(&d);
    let _ = flash;
    for (model, id_prefix) in [
        ("s25fl129p0", "RDID: 01 20 18 4d 00"),
        ("s25fl129p1", "RDID: 01 20 18 4d 01"),
        ("n25q128a13", "RDID: 20 ba 18"),
    ] {
        let p = project(
            &d,
            "rust_qspi.elf",
            &format!("qspi_image = \"flash.bin\"\nqspi_model = \"{model}\"\n"),
        );
        let v = run_json(&p, "10ms");
        let u = uart(&v);
        assert!(u.contains(id_prefix), "{model}: {u}");
        assert!(u.contains("RESULT: ALL PASS"), "{model}: {u}");
        assert_eq!(v["storage"]["qspi"]["model"], model);
    }
}
