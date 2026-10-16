//! Tests against a real `bootgen` 2022.1 image (`fixtures/boot/BOOT.BIN`, made from
//! `boot.bif`) and mutations of it.

use super::*;

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn real() -> Vec<u8> {
    fixture("boot/BOOT.BIN")
}

fn put(v: &mut [u8], off: usize, val: u32) {
    v[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn get(v: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([v[off], v[off + 1], v[off + 2], v[off + 3]])
}

fn fix_boot_checksum(v: &mut [u8]) {
    let c = inverted_sum(v, 0x20, 0x48);
    put(v, 0x48, c);
}

/// Partition header table of the fixture starts here (`pht_offset` printed by `bootgen -read`).
const PHT: usize = 0xC80;

fn fix_ph_checksum(v: &mut [u8], index: usize) {
    let o = PHT + index * PARTITION_HEADER_LEN;
    let c = inverted_sum(v, o, o + 0x3C);
    put(v, o + 0x3C, c);
}

fn has(img: &BootImage, sev: Severity, needle: &str) -> bool {
    img.issues
        .iter()
        .any(|i| i.severity == sev && i.to_string().contains(needle))
}

#[test]
fn a_real_bootgen_image_is_read_completely() {
    let bytes = real();
    let img = BootImage::parse(&bytes).expect("parses");
    assert!(img.issues.is_empty(), "{:#?}", img.issues);
    assert_eq!(img.file_size, bytes.len() as u64);
    let h = &img.header;
    assert_eq!(h.header_version, 0x0101_0000);
    assert_eq!(h.encryption_status, 0);
    assert_eq!((h.fsbl_offset, h.fsbl_length), (0x1700, 0x10));
    assert_eq!(
        (h.image_header_table_offset, h.partition_header_offset),
        (0x8C0, 0xC80)
    );
    assert_eq!(h.checksum, 0xFC19_4520, "value `bootgen -read` printed");
    assert_eq!(img.image_table.partition_count, 4);
    let names: Vec<&str> = img.images.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["fsbl_stub.elf", "dummy.bit", "app.elf"]);
    assert_eq!(img.images[2].partition_count, 2);
    assert_eq!(
        img.register_init,
        vec![
            RegInit {
                address: 0xF800_0008,
                value: 0xDF0D
            },
            RegInit {
                address: 0xF800_0120,
                value: 0x1F00_0200
            },
            RegInit {
                address: 0xF800_0170,
                value: 0x0010_0A00
            },
            RegInit {
                address: 0xF800_0240,
                value: 0
            },
        ]
    );
    let kinds: Vec<PartitionKind> = img.partitions.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        [
            PartitionKind::Fsbl,
            PartitionKind::Bitstream,
            PartitionKind::Application,
            PartitionKind::Application
        ]
    );
    let p = &img.partitions;
    assert_eq!(
        (p[0].length, p[0].data_offset, p[0].load_address),
        (16, 0x1700, 0)
    );
    assert_eq!((p[1].length, p[1].destination), (384, Destination::Pl));
    assert_eq!(p[1].sync_word_found, Some(true));
    // Values `bootgen -read` printed for app.elf.0 / app.elf.1 (attributes 0x1010 = MD5).
    assert_eq!(
        (
            p[2].load_address,
            p[2].exec_address,
            p[2].length,
            p[2].data_offset
        ),
        (0x0010_0000, 0x0010_0000, 2364, 0x18C0)
    );
    assert_eq!(
        (
            p[3].load_address,
            p[3].exec_address,
            p[3].length,
            p[3].data_offset
        ),
        (0x0010_093C, 0, 300, 0x2200)
    );
    assert_eq!(p[2].attributes, 0x1010);
    assert_eq!(p[2].checksum_type, ChecksumType::Md5);
    assert_eq!(p[2].checksum_ok, Some(true));
    assert_eq!(p[3].checksum_ok, Some(true));
    assert_eq!(p[2].image.as_deref(), Some("app.elf"));
    assert_eq!(
        img.entry_partition().map(|p| p.exec_address),
        Some(0x10_0000)
    );
    assert!(img.other_entries().is_empty());
}

#[test]
fn application_payloads_equal_the_elf_segments_bootgen_was_given() {
    let bytes = real();
    let img = BootImage::parse(&bytes).unwrap();
    let elf = fixture("elf/rust_hello.elf");
    // PT_LOAD #0 and #1 of rust_hello.elf: (file offset, size) = (65536, 2364), (67900, 300).
    assert_eq!(img.payload(&bytes, 2).unwrap(), &elf[65536..65536 + 2364]);
    assert_eq!(img.payload(&bytes, 3).unwrap(), &elf[67900..67900 + 300]);
    assert!(img.payload(&bytes, 9).is_err());
}

#[test]
fn the_load_plan_becomes_an_elf_with_the_same_segments() {
    let bytes = real();
    let img = BootImage::parse(&bytes).unwrap();
    let plan = img.load_plan(&bytes).unwrap();
    assert_eq!(plan.entry, 0x10_0000);
    assert_eq!(plan.segments.len(), 2);
    assert_eq!(plan.total_bytes(), 2364 + 300);
    let elf = plan.to_elf();
    assert_eq!(&elf[..4], b"\x7fELF");
    assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 40, "EM_ARM");
    assert_eq!(get(&elf, 24), 0x10_0000, "e_entry");
    let phnum = u16::from_le_bytes([elf[44], elf[45]]);
    assert_eq!(phnum, 2);
    for (i, s) in plan.segments.iter().enumerate() {
        let ph = 52 + i * 32;
        let off = get(&elf, ph + 4) as usize;
        assert_eq!(get(&elf, ph), 1, "PT_LOAD");
        assert_eq!(get(&elf, ph + 12), s.load_address, "p_paddr");
        assert_eq!(get(&elf, ph + 16) as usize, s.data.len());
        assert_eq!(&elf[off..off + s.data.len()], &s.data[..]);
    }
    assert_eq!(plan.to_elf(), elf, "deterministic");
}

#[test]
fn a_damaged_boot_header_is_reported_with_its_checksum() {
    let mut b = real();
    b[0x44] ^= 0x01; // QSPI config word, covered by the checksum
    let img = BootImage::parse(&b).unwrap();
    let e = img.errors();
    assert_eq!(e.len(), 1, "{e:#?}");
    assert_eq!(e[0].field, "boot_header.checksum");
    assert_eq!(e[0].offset, Some(0x48));
    assert!(e[0].message.contains("0xfc194520"), "{}", e[0].message);
    let err = img.load_plan(&b).unwrap_err();
    assert!(err.message.contains("boot_header.checksum"), "{err}");
}

#[test]
fn a_damaged_partition_header_names_the_partition() {
    let mut b = real();
    b[PHT + 2 * PARTITION_HEADER_LEN + 0x0C] ^= 0x10; // app.elf.0 load address
    let img = BootImage::parse(&b).unwrap();
    let e = img.errors();
    assert_eq!(e.len(), 1, "{e:#?}");
    assert!(
        e[0].field.starts_with("partition[2] (app.elf)"),
        "{}",
        e[0].field
    );
    assert_eq!(e[0].offset, Some((PHT + 2 * 0x40 + 0x3C) as u64));
}

#[test]
fn a_damaged_partition_payload_fails_its_md5() {
    let mut b = real();
    b[0x18C0 + 100] ^= 0xFF;
    let img = BootImage::parse(&b).unwrap();
    assert_eq!(img.partitions[2].checksum_ok, Some(false));
    assert_eq!(img.partitions[3].checksum_ok, Some(true));
    assert!(has(&img, Severity::Error, "MD5"));
    assert!(img.load_plan(&b).is_err());
}

#[test]
fn truncation_is_reported_at_every_level() {
    let b = real();
    for n in [0usize, 10, 0x27, 0x100, 0x89F] {
        let e = BootImage::parse(&b[..n]).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Truncated, "len {n}: {e}");
    }
    // Tables present, the file ends inside the partition table.
    let e = BootImage::parse(&b[..0xC90]).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Truncated, "{e}");
    assert!(e.message.contains("partition header table"), "{e}");
    // Headers complete, payload cut off: an error issue, not a failure to read.
    let img = BootImage::parse(&b[..0x1900]).unwrap();
    assert!(has(&img, Severity::Error, "runs past the end of the file"));
    assert!(img.load_plan(&b[..0x1900]).is_err());
}

#[test]
fn a_file_that_is_not_a_boot_image_says_so() {
    let mut b = real();
    put(&mut b, 0x24, 0x1234_5678);
    let e = BootImage::parse(&b).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotABootImage);
    assert_eq!(e.offset, Some(0x24));
    assert!(e.message.contains("XNLX"), "{e}");

    let mut b = real();
    put(&mut b, 0x20, 0);
    let e = BootImage::parse(&b).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotABootImage);
    assert!(e.message.contains("width-detection"), "{e}");

    // An ELF handed over by mistake.
    let e = BootImage::parse(&fixture("elf/rust_hello.elf")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotABootImage);
}

#[test]
fn encrypted_images_are_refused_with_a_clear_message() {
    let mut b = real();
    put(&mut b, 0x28, 0x3A5C_3C5A);
    fix_boot_checksum(&mut b);
    let img = BootImage::parse(&b).unwrap();
    let e = img.errors();
    assert_eq!(e.len(), 1, "{e:#?}");
    assert!(e[0].message.contains("encrypted"), "{}", e[0].message);
    assert!(e[0].message.contains("BBRAM"), "{}", e[0].message);
    assert!(e[0].message.contains("not supported"), "{}", e[0].message);
    let err = img.load_plan(&b).unwrap_err();
    assert!(err.message.contains("encrypted"), "{err}");

    // A partition whose encrypted length differs from the plain length.
    let mut b = real();
    let o = PHT + 2 * PARTITION_HEADER_LEN;
    put(&mut b, o, 0x260);
    fix_ph_checksum(&mut b, 2);
    let img = BootImage::parse(&b).unwrap();
    assert!(has(&img, Severity::Error, "encrypted"));
    assert!(img.payload(&b, 2).is_err());
}

#[test]
fn authenticated_images_are_refused() {
    let mut b = real();
    put(&mut b, PHT + 2 * PARTITION_HEADER_LEN + 0x28, 0x700);
    fix_ph_checksum(&mut b, 2);
    let img = BootImage::parse(&b).unwrap();
    assert!(has(&img, Severity::Error, "authenticated"));

    let mut b = real();
    put(&mut b, 0x8C0 + 0x10, 0x700);
    let img = BootImage::parse(&b).unwrap();
    assert!(has(&img, Severity::Error, "authentication certificate"));
}

#[test]
fn an_image_without_a_ps_application_has_nothing_to_run() {
    let mut b = real();
    for i in [2, 3] {
        // destination PL: they become bitstream partitions
        put(&mut b, PHT + i * PARTITION_HEADER_LEN + 0x18, 0x20);
        fix_ph_checksum(&mut b, i);
    }
    let img = BootImage::parse(&b).unwrap();
    assert_eq!(img.applications().count(), 0);
    assert_eq!(img.bitstreams().count(), 3);
    let err = img.load_plan(&b).unwrap_err();
    assert!(err.message.contains("no application partition"), "{err}");
}

#[test]
fn an_application_without_an_entry_point_is_refused() {
    let mut b = real();
    put(&mut b, PHT + 2 * PARTITION_HEADER_LEN + 0x10, 0);
    fix_ph_checksum(&mut b, 2);
    let img = BootImage::parse(&b).unwrap();
    assert!(img.entry_partition().is_none());
    let err = img.load_plan(&b).unwrap_err();
    assert!(err.message.contains("entry point"), "{err}");
}

#[test]
fn the_description_lists_partitions_and_the_register_table() {
    let b = real();
    let text = BootImage::parse(&b).unwrap().describe();
    for needle in [
        "fsbl_stub.elf",
        "dummy.bit",
        "app.elf",
        "0xf8000120 <- 0x1f000200",
        "entry (core 0): 0x00100000",
        "md5 ok",
    ] {
        assert!(text.contains(needle), "missing `{needle}` in:\n{text}");
    }
}

#[test]
fn damaged_images_never_panic() {
    let base = real();
    let mut s: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for _ in 0..3000 {
        let mut b = base.clone();
        for _ in 0..(next() % 8) {
            let i = (next() % b.len() as u64) as usize;
            b[i] = next() as u8;
        }
        // Words that are offsets and counts get hit more often than a uniform flip would.
        let hot = [
            0x30,
            0x34,
            0x98,
            0x9C,
            0x8C0 + 4,
            0x8C0 + 8,
            0x8C0 + 12,
            0x900,
            0xC80 + 0x14,
        ];
        let i = hot[(next() % hot.len() as u64) as usize];
        put(&mut b, i, next() as u32);
        if next() % 3 == 0 {
            let n = (next() % b.len() as u64) as usize;
            b.truncate(n);
        }
        if let Ok(img) = BootImage::parse(&b) {
            let _ = img.describe();
            let _ = img.load_plan(&b);
            for i in 0..img.partitions.len() {
                let _ = img.payload(&b, i);
            }
        }
    }
}
