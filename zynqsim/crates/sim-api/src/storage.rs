//! Host-side storage tooling: the `[storage]` model, SD card and QSPI flash
//! image builders (ADR-012).
//!
//! Everything here runs on the host and is independent of any engine:
//!
//! * [`build_sd_image`] turns a folder into a raw SD card image (an MBR with
//!   one FAT16 or FAT32 partition, as a card formatted by a PC has);
//! * [`read_sd_image`] reads such an image back (used by tests and tools);
//! * [`build_qspi_image`] packs a file at an offset into an erased 16 MiB
//!   flash image.
//!
//! The FAT work is done by the `fatfs` crate; no C is involved. Images are
//! byte-for-byte reproducible: timestamps are fixed (the crate's `chrono`
//! feature is off, so it stamps 1980-01-01), the volume id and the MBR disk
//! signature are constants and directory entries are created in sorted order.

use crate::error::ApiError;
use fatfs::{FatType, FileSystem, FormatVolumeOptions, FsOptions};
use fscommon::StreamSlice;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Sector size of an SD card and of the FAT volumes built here.
pub const SD_SECTOR: usize = 512;
/// Size of the Zybo Z7-20's QSPI flash (S25FL128S, 128 Mbit).
pub const QSPI_BYTES: usize = 16 << 20;
/// First sector of the FAT partition (1 MiB aligned, as PC partitioners do).
pub const PARTITION_START: u32 = 2048;
/// Smallest and largest card image [`build_sd_image`] makes, in MiB.
pub const SD_MIN_MB: u32 = 8;
/// See [`SD_MIN_MB`]. The image is built in memory.
pub const SD_MAX_MB: u32 = 1024;
/// QSPI flash models the QEMU engine can be asked for (see ADR-012).
pub const QSPI_MODELS: &[&str] = &["s25fl129p0", "s25fl129p1", "n25q128a13"];
/// The default QSPI flash model.
pub const DEFAULT_QSPI_MODEL: &str = "s25fl129p0";

/// MBR disk signature written into images (arbitrary constant: "ZSIM").
const DISK_SIGNATURE: u32 = 0x4D49_535A;
/// FAT volume id written into images.
const VOLUME_ID: u32 = 0x5A53_494D;
/// MBR partition type ids (FAT16 and FAT32 with LBA addressing).
const MBR_FAT16_LBA: u8 = 0x0E;
const MBR_FAT32_LBA: u8 = 0x0C;

/// Which FAT variant to format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FatKind {
    /// FAT16 below 64 MiB, FAT32 from there on.
    #[default]
    Auto,
    /// FAT16.
    Fat16,
    /// FAT32 (needs at least 64 MiB so the volume has 65525 or more clusters).
    Fat32,
}

/// Options for [`build_sd_image`].
#[derive(Clone, Debug)]
pub struct SdImageOptions {
    /// Card size in MiB: a power of two from 8 to 1024 (QEMU's SD model
    /// refuses other sizes). `None` picks the smallest that holds the files.
    pub size_mb: Option<u32>,
    /// FAT variant.
    pub fat: FatKind,
    /// Volume label, up to 11 ASCII characters.
    pub label: String,
}

impl Default for SdImageOptions {
    fn default() -> Self {
        SdImageOptions {
            size_mb: None,
            fat: FatKind::Auto,
            label: "ZYNQSIM".to_string(),
        }
    }
}

/// One file of an SD image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdFile {
    /// Path inside the volume, `/`-separated, no leading slash.
    pub path: String,
    /// Contents.
    pub data: Vec<u8>,
}

fn io_err(what: &str, e: impl std::fmt::Display) -> ApiError {
    ApiError::io(format!("{what}: {e}"))
}

/// A directory tree read from the host, sorted so images are reproducible.
enum Node {
    Dir(String, Vec<Node>),
    File(String, PathBuf, u64),
}

fn scan(dir: &Path, name: String, depth: usize) -> Result<Node, ApiError> {
    if depth > 16 {
        return Err(ApiError::invalid_argument(format!(
            "{}: directories are nested deeper than 16 levels",
            dir.display()
        )));
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| io_err(&format!("cannot read {}", dir.display()), e))?
        .collect::<Result<_, _>>()
        .map_err(|e| io_err(&format!("cannot read {}", dir.display()), e))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut out = Vec::new();
    for e in entries {
        let path = e.path();
        let child = e.file_name().to_string_lossy().into_owned();
        let meta = std::fs::metadata(&path)
            .map_err(|er| io_err(&format!("cannot stat {}", path.display()), er))?;
        if meta.is_dir() {
            out.push(scan(&path, child, depth + 1)?);
        } else if meta.is_file() {
            out.push(Node::File(child, path, meta.len()));
        }
    }
    Ok(Node::Dir(name, out))
}

fn content_bytes(n: &Node) -> u64 {
    match n {
        Node::File(_, _, len) => len.div_ceil(512) * 512 + 512,
        Node::Dir(_, kids) => 1024 + kids.iter().map(content_bytes).sum::<u64>(),
    }
}

fn write_tree<T: fatfs::ReadWriteSeek>(
    dir: &fatfs::Dir<'_, T>,
    kids: &[Node],
) -> Result<(), ApiError> {
    for k in kids {
        match k {
            Node::Dir(name, sub) => {
                let d = dir
                    .create_dir(name)
                    .map_err(|e| io_err(&format!("cannot create directory `{name}`"), e))?;
                write_tree(&d, sub)?;
            }
            Node::File(name, path, _) => {
                let data = std::fs::read(path)
                    .map_err(|e| io_err(&format!("cannot read {}", path.display()), e))?;
                let mut f = dir
                    .create_file(name)
                    .map_err(|e| io_err(&format!("cannot create file `{name}`"), e))?;
                f.write_all(&data)
                    .map_err(|e| io_err(&format!("cannot write file `{name}`"), e))?;
                f.flush()
                    .map_err(|e| io_err(&format!("cannot write file `{name}`"), e))?;
            }
        }
    }
    Ok(())
}

fn label_bytes(label: &str) -> Result<[u8; 11], ApiError> {
    let up = label.to_ascii_uppercase();
    if up.len() > 11
        || !up
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b' ')
    {
        return Err(ApiError::invalid_argument(format!(
            "volume label `{label}` must be at most 11 characters from A-Z 0-9 _ - and space"
        )));
    }
    let mut out = [b' '; 11];
    out[..up.len()].copy_from_slice(up.as_bytes());
    Ok(out)
}

/// The master boot record: one bootable-flag-less partition of `type_id`.
fn mbr(type_id: u8, total_sectors: u32) -> [u8; SD_SECTOR] {
    let mut s = [0u8; SD_SECTOR];
    s[440..444].copy_from_slice(&DISK_SIGNATURE.to_le_bytes());
    let e = &mut s[446..462];
    e[0] = 0x00; // not bootable
    e[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]); // CHS: use LBA
    e[4] = type_id;
    e[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    e[8..12].copy_from_slice(&PARTITION_START.to_le_bytes());
    e[12..16].copy_from_slice(&(total_sectors - PARTITION_START).to_le_bytes());
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

fn try_build(
    tree: &[Node],
    size_mb: u32,
    fat: FatKind,
    label: [u8; 11],
) -> Result<Vec<u8>, BuildFailure> {
    let total = (size_mb as usize) << 20;
    let total_sectors = (total / SD_SECTOR) as u32;
    let part_bytes = (total - PARTITION_START as usize * SD_SECTOR) as u64;
    let mut image = vec![0u8; total];
    {
        let cur = Cursor::new(&mut image[..]);
        let slice = StreamSlice::new(
            cur,
            u64::from(PARTITION_START) * SD_SECTOR as u64,
            u64::from(PARTITION_START) * SD_SECTOR as u64 + part_bytes,
        )
        .map_err(|e| BuildFailure::Other(io_err("cannot place the partition", e)))?;
        let mut slice = slice;
        let mut opts = FormatVolumeOptions::new()
            .volume_id(VOLUME_ID)
            .volume_label(label);
        match fat {
            FatKind::Fat32 => opts = opts.fat_type(FatType::Fat32),
            FatKind::Fat16 => opts = opts.fat_type(FatType::Fat16),
            FatKind::Auto if size_mb >= 64 => opts = opts.fat_type(FatType::Fat32),
            FatKind::Auto => opts = opts.fat_type(FatType::Fat16),
        }
        fatfs::format_volume(&mut slice, opts)
            .map_err(|e| BuildFailure::Other(io_err("cannot format the volume", e)))?;
        slice
            .seek(SeekFrom::Start(0))
            .map_err(|e| BuildFailure::Other(io_err("cannot seek", e)))?;
        let fs = FileSystem::new(slice, FsOptions::new())
            .map_err(|e| BuildFailure::Other(io_err("cannot open the new volume", e)))?;
        let got = fs.fat_type();
        let want = match fat {
            FatKind::Fat16 => Some(FatType::Fat16),
            FatKind::Fat32 => Some(FatType::Fat32),
            FatKind::Auto => None,
        };
        if got == FatType::Fat12 || want.is_some_and(|w| w != got) {
            return Err(BuildFailure::WrongFat(got));
        }
        if let Err(e) = write_tree(&fs.root_dir(), tree) {
            // `create_*`/`write_all` report a full volume as an I/O error.
            return Err(if e.message.contains("No space left") {
                BuildFailure::NoSpace(e)
            } else {
                BuildFailure::Other(e)
            });
        }
        fs.unmount()
            .map_err(|e| BuildFailure::Other(io_err("cannot finish the volume", e)))?;
        let type_id = if got == FatType::Fat32 {
            MBR_FAT32_LBA
        } else {
            MBR_FAT16_LBA
        };
        image[..SD_SECTOR].copy_from_slice(&mbr(type_id, total_sectors));
    }
    Ok(image)
}

enum BuildFailure {
    /// The files did not fit.
    NoSpace(ApiError),
    /// The size does not give the FAT variant that was asked for.
    WrongFat(FatType),
    Other(ApiError),
}

/// Builds an SD card image holding the files of `dir` (recursively).
///
/// The card is `size_mb` MiB (a power of two, 8 to 1024) or, when `None`, the
/// smallest such size that holds the files. Fails with a message naming the
/// problem when the files do not fit, or `--fat 32` is asked for a card below
/// 64 MiB.
pub fn build_sd_image(dir: &Path, opts: &SdImageOptions) -> Result<Vec<u8>, ApiError> {
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let label = label_bytes(&opts.label)?;
    let root = scan(dir, String::new(), 0)?;
    let Node::Dir(_, tree) = &root else {
        unreachable!("scan returns a directory")
    };
    let need = content_bytes(&root) + (PARTITION_START as u64) * SD_SECTOR as u64;

    if let Some(mb) = opts.size_mb {
        check_card_size(mb)?;
        return match try_build(tree, mb, opts.fat, label) {
            Ok(i) => Ok(i),
            Err(BuildFailure::NoSpace(e)) => Err(ApiError::invalid_argument(format!(
                "the files do not fit on a {mb} MiB card: {}",
                e.message
            ))
            .with_hint("use a larger --size-mb or leave it out to size the card automatically")),
            Err(BuildFailure::WrongFat(t)) => Err(ApiError::invalid_argument(format!(
                "a {mb} MiB card cannot be formatted as the requested FAT variant (got {t:?}); \
                 FAT32 needs at least 64 MiB, FAT16 at most about 2 GiB"
            ))),
            Err(BuildFailure::Other(e)) => Err(e),
        };
    }
    let start = if opts.fat == FatKind::Fat32 {
        64
    } else {
        SD_MIN_MB
    };
    let mut mb = start;
    while mb <= SD_MAX_MB {
        // Skip sizes that cannot hold the files (the estimate is generous, so
        // a real attempt still decides).
        if (u64::from(mb) << 20) >= need {
            match try_build(tree, mb, opts.fat, label) {
                Ok(i) => return Ok(i),
                Err(BuildFailure::NoSpace(_) | BuildFailure::WrongFat(_)) => {}
                Err(BuildFailure::Other(e)) => return Err(e),
            }
        }
        mb *= 2;
    }
    Err(ApiError::invalid_argument(format!(
        "the files in {} need more than a {SD_MAX_MB} MiB card",
        dir.display()
    )))
}

/// Writes [`build_sd_image`]'s result to `out`.
pub fn write_sd_image(dir: &Path, out: &Path, opts: &SdImageOptions) -> Result<u64, ApiError> {
    let bytes = build_sd_image(dir, opts)?;
    std::fs::write(out, &bytes)
        .map_err(|e| io_err(&format!("cannot write {}", out.display()), e))?;
    Ok(bytes.len() as u64)
}

/// Checks a card size in MiB: power of two, 8 to 1024.
pub fn check_card_size(mb: u32) -> Result<(), ApiError> {
    if !mb.is_power_of_two() || !(SD_MIN_MB..=SD_MAX_MB).contains(&mb) {
        return Err(ApiError::invalid_argument(format!(
            "SD card size {mb} MiB must be a power of two from {SD_MIN_MB} to {SD_MAX_MB} \
             (QEMU's SD card model rejects other sizes)"
        )));
    }
    Ok(())
}

fn walk<T: fatfs::ReadWriteSeek>(
    dir: &fatfs::Dir<'_, T>,
    prefix: &str,
    out: &mut Vec<SdFile>,
) -> Result<(), ApiError> {
    for e in dir.iter() {
        let e = e.map_err(|e| io_err("cannot read a directory", e))?;
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if e.is_dir() {
            walk(&e.to_dir(), &path, out)?;
        } else {
            let mut data = Vec::new();
            e.to_file()
                .read_to_end(&mut data)
                .map_err(|er| io_err(&format!("cannot read `{path}`"), er))?;
            out.push(SdFile { path, data });
        }
    }
    Ok(())
}

/// Reads every file of the first FAT partition of an SD image, sorted by path.
pub fn read_sd_image(image: &[u8]) -> Result<Vec<SdFile>, ApiError> {
    if image.len() < SD_SECTOR || image[510] != 0x55 || image[511] != 0xAA {
        return Err(ApiError::invalid_argument(
            "not an SD card image: no MBR signature in sector 0",
        ));
    }
    let start = u64::from(u32::from_le_bytes(
        image[454..458].try_into().unwrap_or([0; 4]),
    ));
    let count = u64::from(u32::from_le_bytes(
        image[458..462].try_into().unwrap_or([0; 4]),
    ));
    let (from, to) = (start * 512, (start + count) * 512);
    if count == 0 || to > image.len() as u64 {
        return Err(ApiError::invalid_argument(
            "the first partition is empty or lies outside the image",
        ));
    }
    let slice = StreamSlice::new(Cursor::new(image.to_vec()), from, to)
        .map_err(|e| io_err("cannot open the partition", e))?;
    let fs = FileSystem::new(slice, FsOptions::new())
        .map_err(|e| io_err("the first partition is not a FAT volume", e))?;
    let mut out = Vec::new();
    walk(&fs.root_dir(), "", &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The FAT variant of the first partition of an SD image.
pub fn sd_image_fat_type(image: &[u8]) -> Result<&'static str, ApiError> {
    let start = u64::from(u32::from_le_bytes(
        image
            .get(454..458)
            .and_then(|s| s.try_into().ok())
            .unwrap_or([0; 4]),
    ));
    let slice = StreamSlice::new(Cursor::new(image.to_vec()), start * 512, image.len() as u64)
        .map_err(|e| io_err("cannot open the partition", e))?;
    let fs = FileSystem::new(slice, FsOptions::new())
        .map_err(|e| io_err("the first partition is not a FAT volume", e))?;
    Ok(match fs.fat_type() {
        FatType::Fat12 => "FAT12",
        FatType::Fat16 => "FAT16",
        FatType::Fat32 => "FAT32",
    })
}

/// Packs `file` at byte `offset` of an erased (`0xFF`) flash image of
/// `size_mb` MiB (16 for the board's flash; QEMU's models need exactly that).
pub fn build_qspi_image(file: &[u8], offset: u64, size_mb: u32) -> Result<Vec<u8>, ApiError> {
    if size_mb == 0 || size_mb > 64 {
        return Err(ApiError::invalid_argument(format!(
            "flash size {size_mb} MiB is out of range (1..=64)"
        )));
    }
    let size = (size_mb as usize) << 20;
    let end = offset
        .checked_add(file.len() as u64)
        .ok_or_else(|| ApiError::invalid_argument("offset + length overflows"))?;
    if end > size as u64 {
        return Err(ApiError::invalid_argument(format!(
            "{} byte(s) at offset {offset:#x} end at {end:#x}, past the {size_mb} MiB flash",
            file.len()
        )));
    }
    let mut img = vec![0xFFu8; size];
    img[offset as usize..end as usize].copy_from_slice(file);
    Ok(img)
}

/// Reads a whole file for packing; a helper shared by the front ends.
pub fn write_qspi_image(
    file: &Path,
    offset: u64,
    size_mb: u32,
    out: &Path,
) -> Result<u64, ApiError> {
    let data =
        std::fs::read(file).map_err(|e| io_err(&format!("cannot read {}", file.display()), e))?;
    let img = build_qspi_image(&data, offset, size_mb)?;
    std::fs::write(out, &img).map_err(|e| io_err(&format!("cannot write {}", out.display()), e))?;
    Ok(img.len() as u64)
}

/// Facts about an image file that validation and the status report share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageFacts {
    /// Size of the file in bytes.
    pub bytes: u64,
}

/// Looks at an SD image file: it must exist, be a multiple of 512 bytes and a
/// power-of-two size (QEMU's SD model requires it).
pub fn check_sd_file(path: &Path) -> Result<ImageFacts, String> {
    let meta = std::fs::metadata(path).map_err(|_| "does not exist".to_string())?;
    if !meta.is_file() {
        return Err("is not a file".to_string());
    }
    let bytes = meta.len();
    if bytes < SD_SECTOR as u64 || !bytes.is_power_of_two() {
        return Err(format!(
            "is {bytes} bytes; QEMU's SD card model needs a power-of-two size of at least 512 \
             bytes (build one with `zynqsim sd-image`, or `qemu-img resize`)"
        ));
    }
    Ok(ImageFacts { bytes })
}

/// Looks at a QSPI image file: it must exist and be exactly 16 MiB.
pub fn check_qspi_file(path: &Path) -> Result<ImageFacts, String> {
    let meta = std::fs::metadata(path).map_err(|_| "does not exist".to_string())?;
    if !meta.is_file() {
        return Err("is not a file".to_string());
    }
    let bytes = meta.len();
    if bytes != QSPI_BYTES as u64 {
        return Err(format!(
            "is {bytes} bytes; the flash models need exactly {QSPI_BYTES} bytes (16 MiB; \
             build one with `zynqsim qspi-image`)"
        ));
    }
    Ok(ImageFacts { bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "zynqsim-storage-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Bytes that are not periodic in the cluster size, so a misplaced
    /// cluster shows up.
    fn pattern(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 7) as u8
            })
            .collect()
    }

    fn sample_tree(d: &Path) -> Vec<SdFile> {
        std::fs::create_dir_all(d.join("SUB/DEEP")).unwrap();
        let files = vec![
            ("HELLO.TXT", b"Hello from the SD card.\n".to_vec()),
            ("readme with spaces.txt", b"long name".to_vec()),
            ("SUB/DATA.BIN", pattern(70_000, 1)),
            ("SUB/DEEP/EMPTY.DAT", Vec::new()),
            ("SUB/DEEP/z.bin", pattern(513, 2)),
        ];
        let mut want = Vec::new();
        for (p, data) in files {
            std::fs::write(d.join(p), &data).unwrap();
            want.push(SdFile {
                path: p.to_string(),
                data,
            });
        }
        want.sort_by(|a, b| a.path.cmp(&b.path));
        want
    }

    #[test]
    fn sd_image_round_trips_files_and_directories() {
        let d = temp("rt");
        let want = sample_tree(&d);
        let img = build_sd_image(&d, &SdImageOptions::default()).unwrap();
        assert!(img.len().is_power_of_two());
        assert_eq!(img.len(), 8 << 20);
        assert_eq!(&img[510..512], &[0x55, 0xAA]);
        assert_eq!(img[446 + 4], MBR_FAT16_LBA);
        assert_eq!(read_sd_image(&img).unwrap(), want);
        assert_eq!(sd_image_fat_type(&img).unwrap(), "FAT16");
    }

    #[test]
    fn sd_image_is_reproducible() {
        let d = temp("repro");
        sample_tree(&d);
        let a = build_sd_image(&d, &SdImageOptions::default()).unwrap();
        let b = build_sd_image(&d, &SdImageOptions::default()).unwrap();
        assert!(a == b, "two builds of the same folder differ");
    }

    #[test]
    fn fat32_cards_are_fat32_and_small_ones_say_why_not() {
        let d = temp("f32");
        let want = sample_tree(&d);
        let opts = SdImageOptions {
            size_mb: Some(64),
            fat: FatKind::Fat32,
            ..SdImageOptions::default()
        };
        let img = build_sd_image(&d, &opts).unwrap();
        assert_eq!(img.len(), 64 << 20);
        assert_eq!(img[446 + 4], MBR_FAT32_LBA);
        assert_eq!(sd_image_fat_type(&img).unwrap(), "FAT32");
        assert_eq!(read_sd_image(&img).unwrap(), want);

        let small = SdImageOptions {
            size_mb: Some(8),
            fat: FatKind::Fat32,
            ..SdImageOptions::default()
        };
        let e = build_sd_image(&d, &small).unwrap_err();
        assert!(e.message.contains("64 MiB"), "{}", e.message);
    }

    #[test]
    fn card_size_rules_are_enforced() {
        let d = temp("size");
        std::fs::write(d.join("A.TXT"), b"a").unwrap();
        for mb in [0, 4, 10, 100, 2048] {
            let o = SdImageOptions {
                size_mb: Some(mb),
                ..SdImageOptions::default()
            };
            let e = build_sd_image(&d, &o).unwrap_err();
            assert!(e.message.contains("power of two"), "{mb}: {}", e.message);
        }
        std::fs::write(d.join("BIG.BIN"), pattern(9 << 20, 3)).unwrap();
        let tight = SdImageOptions {
            size_mb: Some(8),
            ..SdImageOptions::default()
        };
        let e = build_sd_image(&d, &tight).unwrap_err();
        assert!(e.message.contains("do not fit"), "{}", e.message);
        // Automatic sizing grows the card instead.
        let img = build_sd_image(&d, &SdImageOptions::default()).unwrap();
        assert_eq!(img.len(), 16 << 20);
    }

    #[test]
    fn labels_and_missing_folders_are_rejected() {
        let d = temp("label");
        let o = SdImageOptions {
            label: "THIS LABEL IS TOO LONG".to_string(),
            ..SdImageOptions::default()
        };
        assert!(build_sd_image(&d, &o).is_err());
        assert!(build_sd_image(&d.join("nope"), &SdImageOptions::default()).is_err());
    }

    #[test]
    fn qspi_image_packs_a_file_at_an_offset() {
        let data = pattern(1000, 9);
        let img = build_qspi_image(&data, 0x10_0000, 16).unwrap();
        assert_eq!(img.len(), QSPI_BYTES);
        assert!(img[..0x10_0000].iter().all(|&b| b == 0xFF));
        assert_eq!(&img[0x10_0000..0x10_0000 + 1000], &data[..]);
        assert!(img[0x10_0000 + 1000..].iter().all(|&b| b == 0xFF));
        // Exactly at the end fits, one byte more does not.
        assert!(build_qspi_image(&[1, 2], (QSPI_BYTES - 2) as u64, 16).is_ok());
        let e = build_qspi_image(&[1, 2, 3], (QSPI_BYTES - 2) as u64, 16).unwrap_err();
        assert!(e.message.contains("past the 16 MiB flash"), "{}", e.message);
        assert!(build_qspi_image(&[1], u64::MAX, 16).is_err());
    }

    #[test]
    fn image_file_checks_state_the_rule_that_failed() {
        let d = temp("chk");
        let sd = d.join("sd.img");
        std::fs::write(&sd, vec![0u8; 10 << 20]).unwrap();
        let e = check_sd_file(&sd).unwrap_err();
        assert!(e.contains("power-of-two"), "{e}");
        std::fs::write(&sd, vec![0u8; 16 << 20]).unwrap();
        assert_eq!(check_sd_file(&sd).unwrap().bytes, 16 << 20);
        assert!(
            check_sd_file(&d.join("missing.img"))
                .unwrap_err()
                .contains("does not exist")
        );

        let q = d.join("q.bin");
        std::fs::write(&q, vec![0xFFu8; 8 << 20]).unwrap();
        let e = check_qspi_file(&q).unwrap_err();
        assert!(e.contains("exactly 16777216"), "{e}");
        std::fs::write(&q, vec![0xFFu8; QSPI_BYTES]).unwrap();
        assert!(check_qspi_file(&q).is_ok());
    }
}
