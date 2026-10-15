//! microSD demo for the Zybo Z7-20's SD slot (PS SDIO0).
//!
//! 1. Initialises the card through the SDHCI controller (CMD0/8, ACMD41,
//!    CMD2/3/9/7/16) and reports what it found.
//! 2. Reads the MBR with CMD17 and, when there is a gap before the first
//!    partition, writes a block into the gap with CMD24 and reads it back.
//! 3. Mounts the first FAT partition with `embedded-sdmmc`, lists the root
//!    directory and one level of sub-directories, prints `HELLO.TXT` and a
//!    checksum of every file.
//!
//! The host builds the card (`zynqsim sd-image`, or `[storage] sd_dir`), so
//! the test can compare what appears on the UART with the files it put there.
#![no_std]
#![no_main]

mod sdhci;

use core::ops::ControlFlow;
use embedded_sdmmc::{
    DirEntry, LfnBuffer, Mode, ShortFileName, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use rt::println;
use sdhci::{Card, SdBlockDevice};

/// The card has no clock: a fixed time stamp for anything that is written.
struct Clock;

impl TimeSource for Clock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 56, // 2026
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

type Mgr<'a> = VolumeManager<SdBlockDevice<'a>, Clock, 4, 4, 1>;
type Dir<'a, 'b> = embedded_sdmmc::Directory<'a, SdBlockDevice<'b>, Clock, 4, 4, 1>;

/// FNV-1a over `data`, continuing from `h`.
fn fnv1a(mut h: u32, data: &[u8]) -> u32 {
    for &b in data {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    h
}

/// What is needed of a directory entry once the listing callback is over.
#[derive(Clone, Copy)]
struct Entry {
    name: ShortFileName,
    is_dir: bool,
    size: u32,
}

const MAX_ENTRIES: usize = 24;

fn fail(why: &str) -> ! {
    println!("RESULT: FAIL: {why}");
    loop {
        // SAFETY: parks the core; nothing else is left to do.
        unsafe { core::arch::asm!("wfi") }
    }
}

/// Prints bytes as text: CR LF for LF (a CR in the data is dropped), a dot for
/// anything unprintable.
fn put_text(data: &[u8]) {
    for &b in data {
        match b {
            b'\r' => {}
            b'\n' => {
                rt::putc(b'\r');
                rt::putc(b'\n');
            }
            0x20..=0x7E | b'\t' => rt::putc(b),
            _ => rt::putc(b'.'),
        }
    }
}

/// `/NAME/`, the prefix of a sub-directory's listing.
struct SubPath<'a>(&'a ShortFileName);

impl core::fmt::Display for SubPath<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "/{}/", self.0)
    }
}

/// Lists a directory, printing long names, and returns its entries.
fn list(
    dir: &Dir<'_, '_>,
    path: &dyn core::fmt::Display,
    out: &mut [Option<Entry>; MAX_ENTRIES],
) -> usize {
    let mut n = 0usize;
    let mut buf = [0u8; 64];
    let mut lfn = LfnBuffer::new(&mut buf);
    let r = dir.iterate_dir_lfn(&mut lfn, |e: &DirEntry, long: Option<&str>| {
        if e.attributes.is_volume() || e.name.base_name() == b"." || e.name.base_name() == b".." {
            return ControlFlow::Continue(());
        }
        if n < MAX_ENTRIES {
            out[n] = Some(Entry {
                name: e.name.clone(),
                is_dir: e.attributes.is_directory(),
                size: e.size,
            });
            n += 1;
        }
        match long {
            Some(l) => println!(
                "  {path}{}{} {} ({l})",
                e.name,
                if e.attributes.is_directory() { "/" } else { "" },
                e.size
            ),
            None => println!(
                "  {path}{}{} {}",
                e.name,
                if e.attributes.is_directory() { "/" } else { "" },
                e.size
            ),
        }
        ControlFlow::Continue(())
    });
    if let Err(e) = r {
        println!("directory listing failed: {e}");
        fail("directory listing");
    }
    n
}

/// Reads a whole file, returning its FNV-1a checksum and length.
fn read_file(dir: &Dir<'_, '_>, name: &ShortFileName, show: bool) -> (u32, u32) {
    let f = match dir.open_file_in_dir(name, Mode::ReadOnly) {
        Ok(f) => f,
        Err(e) => {
            println!("open {name} failed: {e}");
            fail("open file");
        }
    };
    let mut h = 0x811C_9DC5u32;
    let mut total = 0u32;
    let mut buf = [0u8; 512];
    if show {
        println!("---- {name} ----");
    }
    while !f.is_eof() {
        let n = match f.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                println!("read {name} failed: {e}");
                fail("read file");
            }
        };
        if n == 0 {
            break;
        }
        h = fnv1a(h, &buf[..n]);
        total += n as u32;
        if show {
            put_text(&buf[..n]);
        }
    }
    if show {
        println!("---- end {name} ----");
    }
    (h, total)
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("sd demo: SDHCI (PS SDIO0) + FAT through embedded-sdmmc");
    let card = Card::new();
    let info = match card.init() {
        Ok(i) => i,
        Err(e) => {
            println!("card init failed: {e}");
            fail("card init");
        }
    };
    println!(
        "card: {} {} blocks ({} KiB), ocr {:#010x}, rca {:#06x}, mid {:#04x}, product {}",
        if info.high_capacity { "SDHC" } else { "SDSC" },
        info.blocks,
        info.blocks / 2,
        info.ocr,
        info.rca,
        info.manufacturer,
        core::str::from_utf8(&info.product).unwrap_or("?")
    );

    // Block 0: the master boot record.
    let mut sector = [0u8; 512];
    if let Err(e) = card.read_block(0, &mut sector) {
        println!("read of block 0 failed: {e}");
        fail("read block 0");
    }
    let sig = u16::from_le_bytes([sector[510], sector[511]]);
    let ptype = sector[446 + 4];
    let start = u32::from_le_bytes([sector[454], sector[455], sector[456], sector[457]]);
    let count = u32::from_le_bytes([sector[458], sector[459], sector[460], sector[461]]);
    println!("mbr: signature {sig:#06x}, partition 1 type {ptype:#04x} start {start} sectors {count}");
    if sig != 0xAA55 {
        fail("no MBR signature");
    }

    // A write into the gap before the first partition, read back.
    if start > 1 {
        let mut pat = [0u8; 512];
        for (i, b) in pat.iter_mut().enumerate() {
            *b = (i as u32).wrapping_mul(5).wrapping_add(0x33) as u8;
        }
        if let Err(e) = card.write_block(1, &pat) {
            println!("write of block 1 failed: {e}");
            fail("write block 1");
        }
        let mut back = [0u8; 512];
        if let Err(e) = card.read_block(1, &mut back) {
            println!("read-back of block 1 failed: {e}");
            fail("read back block 1");
        }
        if back != pat {
            fail("block 1 read back differs from what was written");
        }
        println!("raw write: block 1 written and read back, 512 bytes equal");
    }

    // The FAT volume.
    let mgr: Mgr = VolumeManager::new(SdBlockDevice(&card), Clock);
    let volume = match mgr.open_volume(VolumeIdx(0)) {
        Ok(v) => v,
        Err(e) => {
            println!("mount failed: {e:?}");
            fail("mount");
        }
    };
    let root = match volume.open_root_dir() {
        Ok(d) => d,
        Err(e) => {
            println!("open root directory failed: {e}");
            fail("open root");
        }
    };
    println!("mounted volume 0, root directory:");
    let mut entries = [None; MAX_ENTRIES];
    let n = list(&root, &"/", &mut entries);
    println!("{n} entr(ies) in /");

    let mut files = 0u32;
    for e in entries.iter().take(n).flatten() {
        if e.is_dir {
            let sub = match root.open_dir(&e.name) {
                Ok(d) => d,
                Err(err) => {
                    println!("open directory {} failed: {err}", e.name);
                    fail("open directory");
                }
            };
            let mut inner = [None; MAX_ENTRIES];
            let m = list(&sub, &SubPath(&e.name), &mut inner);
            for f in inner.iter().take(m).flatten() {
                if !f.is_dir {
                    let (h, len) = read_file(&sub, &f.name, false);
                    println!("FILE {}/{} size={len} fnv1a={h:#010x}", e.name, f.name);
                    files += 1;
                }
            }
        } else {
            let show = e.name.base_name() == b"HELLO" && e.name.extension() == b"TXT";
            let (h, len) = read_file(&root, &e.name, show);
            println!("FILE {} size={len} fnv1a={h:#010x}", e.name);
            files += 1;
            if len != e.size {
                fail("file length differs from the directory entry");
            }
        }
    }
    println!("{files} file(s) read");
    println!("RESULT: ALL PASS");
    loop {
        // SAFETY: parks the core.
        unsafe { core::arch::asm!("wfi") }
    }
}
