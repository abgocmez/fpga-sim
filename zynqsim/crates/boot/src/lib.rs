//! Zynq-7000 BootROM image (`BOOT.BIN`) reader (prompt section 12, ADR-018).
//!
//! The format is the one `bootgen -arch zynq` writes and the BootROM reads (UG585, "Boot and
//! Configuration"):
//!
//! | Region | Content |
//! |---|---|
//! | `0x000..0x020` | eight ARM vectors (`b .`) |
//! | `0x020` | width-detection word `0xAA995566` |
//! | `0x024` | image identification `0x584C4E58` (bytes `XNLX`) |
//! | `0x028` | encryption status / key source (`0` = not encrypted) |
//! | `0x02C` | header version (`0x01010000`) |
//! | `0x030..0x044` | FSBL source offset, length, load address, execution address, total length, QSPI config word |
//! | `0x048` | checksum: inverted sum of the words `0x020..=0x044` |
//! | `0x098`, `0x09C` | image header table offset, partition header table offset |
//! | `0x0A0..0x8A0` | register initialisation table, up to 256 (address, value) pairs, ended by address `0xFFFFFFFF` |
//! | image header table | version, partition count, partition header offset (words), first image header offset (words), authentication certificate offset |
//! | image headers | next (words), partition header (words), reserved, partition count, name (big-endian packed, NUL ended) |
//! | partition headers (`0x40` bytes) | encrypted / unencrypted / total length (words), load, execution address, data offset (words), attributes, section count, checksum offset (words), image header offset (words), authentication certificate offset (words), 4 reserved words, header checksum (inverted sum of the 15 words before it) |
//!
//! Everything above was checked against images made by Vitis 2022.1 `bootgen` and its own
//! `-read` dump (`fixtures/boot/`). Fields that came from memory of UG585 and that no
//! available image exercises are marked `VERIFY:` where they are decoded.
//!
//! The reader is pure Rust, never panics on malformed input and reports what is wrong with
//! the byte offset of the offending field. It does **not** decrypt or authenticate: such
//! images are refused with an explanation.

mod loadable;
mod md5;

pub use loadable::{LoadPlan, Segment};
pub use md5::md5;

use serde::Serialize;
use std::fmt;

/// Width-detection word at `0x20`.
pub const WIDTH_DETECT: u32 = 0xAA99_5566;
/// Image identification word at `0x24`: the bytes `XNLX` in a little-endian file.
pub const SIGNATURE: u32 = 0x584C_4E58;
/// Size of the boot header including the register-initialisation table.
pub const BOOT_HEADER_LEN: usize = 0x8A0;
/// Offset of the register-initialisation table.
pub const REG_INIT_OFFSET: usize = 0xA0;
/// Most pairs the table can hold.
pub const REG_INIT_MAX: usize = 256;
/// Partition headers are this long.
pub const PARTITION_HEADER_LEN: usize = 0x40;
/// More partitions than this are refused as a corrupt count.
const MAX_PARTITIONS: u32 = 1024;

/// Why an image could not be read at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The file ends before a structure that has to be there.
    Truncated,
    /// Width-detection word or signature is wrong: not a Zynq-7000 boot image.
    NotABootImage,
    /// A table offset or count points outside the file or is inconsistent.
    Malformed,
}

/// A fatal problem: nothing about the image can be trusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BootError {
    /// Class of the problem.
    pub kind: ErrorKind,
    /// Byte offset in the file of the field at fault, when there is one.
    pub offset: Option<u64>,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(o) => write!(f, "{} (at file offset {o:#x})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for BootError {}

fn fatal(kind: ErrorKind, offset: usize, message: impl Into<String>) -> BootError {
    BootError {
        kind,
        offset: Some(offset as u64),
        message: message.into(),
    }
}

/// How serious an [`Issue`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing; the image still boots in ZynqSim.
    Warning,
    /// The BootROM would refuse the image, or ZynqSim cannot run it.
    Error,
}

/// One finding of the validation pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// Severity.
    pub severity: Severity,
    /// Byte offset of the field in the file, when there is one.
    pub offset: Option<u64>,
    /// Where: `boot_header.checksum`, `partition[2].data`, ...
    pub field: String,
    /// What is wrong, precisely.
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)?;
        if let Some(o) = self.offset {
            write!(f, " (file offset {o:#x})")?;
        }
        Ok(())
    }
}

/// The boot header (`0x000..0xA0`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BootHeader {
    /// Width-detection word (`0xAA995566`).
    pub width_detect: u32,
    /// Image identification word (`0x584C4E58`).
    pub signature: u32,
    /// Encryption status / key source; `0` means not encrypted.
    pub encryption_status: u32,
    /// Header version (`0x01010000`).
    pub header_version: u32,
    /// Where the FSBL image starts in the file.
    pub fsbl_offset: u32,
    /// FSBL length in bytes.
    pub fsbl_length: u32,
    /// FSBL load address (OCM).
    pub fsbl_load_address: u32,
    /// FSBL execution address.
    pub fsbl_exec_address: u32,
    /// Total FSBL length including checksum / authentication data.
    pub fsbl_total_length: u32,
    /// QSPI configuration word.
    pub qspi_config: u32,
    /// Checksum as stored.
    pub checksum: u32,
    /// Checksum the contents give.
    pub checksum_expected: u32,
    /// Offset of the image header table.
    pub image_header_table_offset: u32,
    /// Offset of the partition header table.
    pub partition_header_offset: u32,
}

/// One `(address, value)` write of the FSBL's register-initialisation table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RegInit {
    /// Register address (typically SLCR: `0xF8000xxx`).
    pub address: u32,
    /// Value to write.
    pub value: u32,
}

/// The image header table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImageHeaderTable {
    /// Version word (`0x01020000` from bootgen 2022.1).
    pub version: u32,
    /// Number of partition headers.
    pub partition_count: u32,
    /// Partition header table, bytes from the start of the file.
    pub partition_header_offset: u32,
    /// First image header, bytes from the start of the file.
    pub first_image_header_offset: u32,
    /// Authentication certificate (header), bytes; 0 = none.
    pub auth_cert_offset: u32,
}

/// One image header: a named input file of the `.bif`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImageHeader {
    /// Name as bootgen stored it (the file name in the `.bif`).
    pub name: String,
    /// File offset of the header.
    pub offset: u32,
    /// How many partitions belong to it (an ELF with several loadable segments has several).
    pub partition_count: u32,
}

/// What a partition is destined for (attribute bits 5:4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    /// Reserved / none.
    None,
    /// Processing system memory.
    Ps,
    /// Programmable logic configuration.
    Pl,
    /// Internal / other.
    Int,
}

/// How a partition's data is protected against corruption (attribute bits 14:12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumType {
    /// No checksum.
    None,
    /// MD5, the only algorithm the Zynq-7000 BootROM format has.
    Md5,
    /// Another value.
    Unknown(u8),
}

/// What ZynqSim makes of a partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionKind {
    /// The first stage boot loader (not run by ZynqSim).
    Fsbl,
    /// A PL bitstream (recognised and reported, never simulated).
    Bitstream,
    /// Data or code for the PS: a segment of an application ELF or a raw binary.
    Application,
    /// Anything else.
    Other,
}

/// One partition header with its decoded attributes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Partition {
    /// Position in the partition header table.
    pub index: usize,
    /// Name of the image (input file) it belongs to.
    pub image: Option<String>,
    /// What it is.
    pub kind: PartitionKind,
    /// File offset of its header.
    pub header_offset: u32,
    /// Encrypted data length in bytes (equals `length` when not encrypted).
    pub encrypted_length: u32,
    /// Data length in bytes.
    pub length: u32,
    /// Total length in bytes (data, padding, checksum).
    pub total_length: u32,
    /// Load address.
    pub load_address: u32,
    /// Execution address (0 for a segment that is not the entry).
    pub exec_address: u32,
    /// File offset of the data.
    pub data_offset: u32,
    /// Attribute word as stored.
    pub attributes: u32,
    /// Destination device.
    pub destination: Destination,
    /// Owner (attribute bits 17:16): 0 = FSBL, 1 = U-Boot. `VERIFY:` against UG585.
    pub owner: u8,
    /// Checksum type.
    pub checksum_type: ChecksumType,
    /// Section count as stored.
    pub section_count: u32,
    /// File offset of the 16-byte MD5, 0 when there is none.
    pub checksum_offset: u32,
    /// `Some(true)` when the stored MD5 matches the data, `Some(false)` when not.
    pub checksum_ok: Option<bool>,
    /// Authentication certificate offset (bytes), 0 = none.
    pub auth_cert_offset: u32,
    /// Header checksum as stored.
    pub header_checksum: u32,
    /// Header checksum the contents give.
    pub header_checksum_expected: u32,
    /// For a bitstream: the `0xAA995566` sync word was found in the first bytes.
    pub sync_word_found: Option<bool>,
}

impl Partition {
    /// Half-open byte range of the data in the file.
    pub fn data_range(&self) -> std::ops::Range<u64> {
        let s = u64::from(self.data_offset);
        s..s + u64::from(self.length)
    }
}

/// A parsed boot image: structure plus the findings of validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BootImage {
    /// File size in bytes.
    pub file_size: u64,
    /// Boot header.
    pub header: BootHeader,
    /// FSBL register-initialisation pairs, in file order.
    pub register_init: Vec<RegInit>,
    /// Image header table.
    pub image_table: ImageHeaderTable,
    /// Image headers in chain order.
    pub images: Vec<ImageHeader>,
    /// Partitions in header-table order.
    pub partitions: Vec<Partition>,
    /// Everything validation found, in the order found.
    pub issues: Vec<Issue>,
}

fn word(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn need_word(b: &[u8], off: usize, what: &str) -> Result<u32, BootError> {
    word(b, off).ok_or_else(|| {
        fatal(
            ErrorKind::Truncated,
            off,
            format!(
                "the file ends at {:#x}, before {what} (a 4-byte field at {off:#x})",
                b.len()
            ),
        )
    })
}

/// Inverted sum of the little-endian words `from..to` (byte offsets, `to` exclusive).
fn inverted_sum(b: &[u8], from: usize, to: usize) -> u32 {
    let mut s: u32 = 0;
    let mut o = from;
    while o + 4 <= to {
        s = s.wrapping_add(word(b, o).unwrap_or(0));
        o += 4;
    }
    !s
}

/// Image name: big-endian packed bytes, NUL ended.
fn read_name(b: &[u8], off: usize) -> String {
    let mut out = Vec::new();
    'words: for i in 0..64 {
        let Some(w) = word(b, off + i * 4) else { break };
        for byte in w.to_be_bytes() {
            if byte == 0 {
                break 'words;
            }
            out.push(byte);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn issue(
    v: &mut Vec<Issue>,
    severity: Severity,
    offset: Option<usize>,
    field: impl Into<String>,
    message: impl Into<String>,
) {
    v.push(Issue {
        severity,
        offset: offset.map(|o| o as u64),
        field: field.into(),
        message: message.into(),
    });
}

impl BootImage {
    /// Parses and validates `bytes`.
    ///
    /// `Err` means the image cannot be interpreted at all (too short, wrong magic, tables
    /// outside the file). A readable image with problems returns `Ok` with those problems
    /// in [`BootImage::issues`]; use [`BootImage::errors`] to see what makes it unusable.
    pub fn parse(bytes: &[u8]) -> Result<BootImage, BootError> {
        let len = bytes.len();
        if len < 0x28 {
            return Err(fatal(
                ErrorKind::Truncated,
                len,
                format!(
                    "the file is {len} bytes: shorter than the 0x28 bytes that hold the \
                     width-detection word and the signature, so it is not a boot image"
                ),
            ));
        }
        let width_detect = need_word(bytes, 0x20, "the width-detection word")?;
        if width_detect != WIDTH_DETECT {
            return Err(fatal(
                ErrorKind::NotABootImage,
                0x20,
                format!(
                    "width-detection word is {width_detect:#010x}, expected {WIDTH_DETECT:#010x}: \
                     not a Zynq BootROM image"
                ),
            ));
        }
        let signature = need_word(bytes, 0x24, "the image signature")?;
        if signature != SIGNATURE {
            return Err(fatal(
                ErrorKind::NotABootImage,
                0x24,
                format!(
                    "image signature is {signature:#010x} ({}), expected {SIGNATURE:#010x} \
                     (bytes `XNLX`): not a Zynq BootROM image",
                    String::from_utf8_lossy(&signature.to_le_bytes()).escape_default()
                ),
            ));
        }
        if len < BOOT_HEADER_LEN {
            return Err(fatal(
                ErrorKind::Truncated,
                len,
                format!(
                    "the file is {len} bytes but the boot header with its register-init table is \
                     {BOOT_HEADER_LEN:#x} bytes"
                ),
            ));
        }

        let mut issues = Vec::new();
        let w = |o: usize| word(bytes, o).unwrap_or(0);

        let checksum = w(0x48);
        let checksum_expected = inverted_sum(bytes, 0x20, 0x48);
        if checksum != checksum_expected {
            issue(
                &mut issues,
                Severity::Error,
                Some(0x48),
                "boot_header.checksum",
                format!(
                    "stored {checksum:#010x}, the words 0x20..0x48 give {checksum_expected:#010x} \
                     (inverted sum): the header was modified or damaged"
                ),
            );
        }
        let header = BootHeader {
            width_detect,
            signature,
            encryption_status: w(0x28),
            header_version: w(0x2C),
            fsbl_offset: w(0x30),
            fsbl_length: w(0x34),
            fsbl_load_address: w(0x38),
            fsbl_exec_address: w(0x3C),
            fsbl_total_length: w(0x40),
            qspi_config: w(0x44),
            checksum,
            checksum_expected,
            image_header_table_offset: w(0x98),
            partition_header_offset: w(0x9C),
        };
        if header.encryption_status != 0 {
            // VERIFY: key-source values 0xA5C3C5A3 (eFUSE) and 0x3A5C3C5A (BBRAM) from UG585.
            let src = match header.encryption_status {
                0xA5C3_C5A3 => "eFUSE key",
                0x3A5C_3C5A => "BBRAM key",
                _ => "unknown key source",
            };
            issue(
                &mut issues,
                Severity::Error,
                Some(0x28),
                "boot_header.encryption_status",
                format!(
                    "the image is encrypted ({:#010x}, {src}): ZynqSim has no AES-256/HMAC \
                     decryption and no key store, so encrypted boot is not supported; build an \
                     unencrypted image with bootgen",
                    header.encryption_status
                ),
            );
        }

        // Register initialisation table.
        let mut register_init = Vec::new();
        for i in 0..REG_INIT_MAX {
            let o = REG_INIT_OFFSET + i * 8;
            let a = w(o);
            if a == 0xFFFF_FFFF {
                break;
            }
            register_init.push(RegInit {
                address: a,
                value: w(o + 4),
            });
        }

        // Image header table.
        let iht = header.image_header_table_offset as usize;
        if iht < BOOT_HEADER_LEN || iht % 4 != 0 {
            return Err(fatal(
                ErrorKind::Malformed,
                0x98,
                format!(
                    "image header table offset {iht:#x} is inside the boot header \
                     (< {BOOT_HEADER_LEN:#x}) or not word aligned"
                ),
            ));
        }
        let ihv = need_word(bytes, iht, "the image header table")?;
        let image_table = ImageHeaderTable {
            version: ihv,
            partition_count: need_word(bytes, iht + 4, "the image header table's partition count")?,
            partition_header_offset: need_word(bytes, iht + 8, "the partition header offset")?
                .wrapping_mul(4),
            first_image_header_offset: need_word(bytes, iht + 12, "the first image header offset")?
                .wrapping_mul(4),
            auth_cert_offset: need_word(bytes, iht + 16, "the authentication offset")?
                .wrapping_mul(4),
        };
        if image_table.partition_count == 0 || image_table.partition_count > MAX_PARTITIONS {
            return Err(fatal(
                ErrorKind::Malformed,
                iht + 4,
                format!(
                    "partition count {} is not plausible (1..={MAX_PARTITIONS})",
                    image_table.partition_count
                ),
            ));
        }
        if image_table.partition_header_offset != header.partition_header_offset {
            issue(
                &mut issues,
                Severity::Warning,
                Some(iht + 8),
                "image_header_table.partition_header_offset",
                format!(
                    "the table says {:#x}, the boot header says {:#x}; the table's value is used",
                    image_table.partition_header_offset, header.partition_header_offset
                ),
            );
        }
        if image_table.auth_cert_offset != 0 {
            issue(
                &mut issues,
                Severity::Error,
                Some(iht + 16),
                "image_header_table.auth_cert_offset",
                "the image carries a header authentication certificate (RSA authenticated boot): \
                 ZynqSim does not verify signatures, so authenticated boot is not supported"
                    .to_string(),
            );
        }

        // Image headers (chain).
        let mut images: Vec<ImageHeader> = Vec::new();
        let mut off = image_table.first_image_header_offset as usize;
        while off != 0 && images.len() <= image_table.partition_count as usize {
            if images.iter().any(|i| i.offset as usize == off) {
                issue(
                    &mut issues,
                    Severity::Error,
                    Some(off),
                    "image_header",
                    "the image header chain loops back on itself",
                );
                break;
            }
            let next = word(bytes, off);
            let count = word(bytes, off + 12);
            let (Some(next), Some(count)) = (next, count) else {
                issue(
                    &mut issues,
                    Severity::Error,
                    Some(off),
                    "image_header",
                    format!(
                        "image header at {off:#x} lies outside the file ({} bytes)",
                        bytes.len()
                    ),
                );
                break;
            };
            images.push(ImageHeader {
                name: read_name(bytes, off + 16),
                offset: off as u32,
                partition_count: count,
            });
            off = (next as usize).wrapping_mul(4);
        }

        // Partition headers.
        let pht = image_table.partition_header_offset as usize;
        let n = image_table.partition_count as usize;
        let table_end = pht.saturating_add(n * PARTITION_HEADER_LEN);
        if pht < BOOT_HEADER_LEN || pht % 4 != 0 || table_end > len {
            return Err(fatal(
                ErrorKind::Truncated,
                pht.min(len),
                format!(
                    "the partition header table ({n} headers at {pht:#x}..{table_end:#x}) does \
                     not fit in the file ({len} bytes)"
                ),
            ));
        }
        let mut partitions = Vec::with_capacity(n);
        for index in 0..n {
            let ho = pht + index * PARTITION_HEADER_LEN;
            let v: Vec<u32> = (0..16).map(|i| w(ho + i * 4)).collect();
            let hdr_sum = inverted_sum(bytes, ho, ho + 0x3C);
            let image_name = images
                .iter()
                .find(|i| i.offset == v[9].wrapping_mul(4))
                .map(|i| i.name.clone());
            if v[15] != hdr_sum {
                issue(
                    &mut issues,
                    Severity::Error,
                    Some(ho + 0x3C),
                    format!(
                        "partition[{index}] ({}).header_checksum",
                        image_name.as_deref().unwrap_or("?")
                    ),
                    format!(
                        "stored {:#010x}, the header words give {hdr_sum:#010x}",
                        v[15]
                    ),
                );
            }
            let attributes = v[6];
            let destination = match (attributes >> 4) & 7 {
                1 => Destination::Ps,
                2 => Destination::Pl,
                3 => Destination::Int,
                _ => Destination::None,
            };
            let checksum_type = match (attributes >> 12) & 7 {
                0 => ChecksumType::None,
                1 => ChecksumType::Md5,
                x => ChecksumType::Unknown(x as u8),
            };
            let length = v[1].wrapping_mul(4);
            let mut p = Partition {
                index,
                image: image_name,
                kind: PartitionKind::Other,
                header_offset: ho as u32,
                encrypted_length: v[0].wrapping_mul(4),
                length,
                total_length: v[2].wrapping_mul(4),
                load_address: v[3],
                exec_address: v[4],
                data_offset: v[5].wrapping_mul(4),
                attributes,
                destination,
                owner: ((attributes >> 16) & 3) as u8,
                checksum_type,
                section_count: v[7],
                checksum_offset: v[8].wrapping_mul(4),
                checksum_ok: None,
                auth_cert_offset: v[10].wrapping_mul(4),
                header_checksum: v[15],
                header_checksum_expected: hdr_sum,
                sync_word_found: None,
            };
            let name = p.image.clone().unwrap_or_else(|| "?".to_string());
            let field = |s: &str| format!("partition[{index}] ({name}).{s}");

            // A partition header with all-zero lengths and destination is padding.
            let range = p.data_range();
            if range.end > len as u64 {
                issue(
                    &mut issues,
                    Severity::Error,
                    Some(p.data_offset as usize),
                    field("data"),
                    format!(
                        "data {:#x}..{:#x} runs past the end of the file ({len:#x} bytes): \
                         the image is truncated",
                        range.start, range.end
                    ),
                );
            }
            if p.encrypted_length != p.length {
                issue(
                    &mut issues,
                    Severity::Error,
                    Some(ho),
                    field("encrypted_length"),
                    format!(
                        "encrypted length {} differs from the data length {}: the partition is \
                         encrypted, which ZynqSim cannot decrypt",
                        p.encrypted_length, p.length
                    ),
                );
            }
            if p.auth_cert_offset != 0 {
                issue(
                    &mut issues,
                    Severity::Error,
                    Some(ho + 0x28),
                    field("auth_cert_offset"),
                    "the partition is authenticated (RSA): ZynqSim does not verify signatures, \
                     so authenticated boot is not supported",
                );
            }
            match p.checksum_type {
                ChecksumType::None => {}
                ChecksumType::Md5 => {
                    let stored = p.checksum_offset as usize;
                    let data = bytes.get(range.start as usize..range.end as usize);
                    let md5s = bytes.get(stored..stored.saturating_add(16));
                    match (data, md5s) {
                        (Some(d), Some(m)) if p.checksum_offset != 0 => {
                            let ok = md5(d)[..] == *m;
                            p.checksum_ok = Some(ok);
                            if !ok {
                                issue(
                                    &mut issues,
                                    Severity::Error,
                                    Some(stored),
                                    field("checksum"),
                                    "the MD5 stored for the partition does not match its data: \
                                     the partition is corrupt",
                                );
                            }
                        }
                        _ => issue(
                            &mut issues,
                            Severity::Error,
                            Some(stored.min(len)),
                            field("checksum"),
                            format!(
                                "MD5 checksum expected at {stored:#x} but it (or the data) lies \
                                 outside the file"
                            ),
                        ),
                    }
                }
                ChecksumType::Unknown(x) => issue(
                    &mut issues,
                    // VERIFY: Zynq-7000 only defines none and MD5.
                    Severity::Warning,
                    Some(ho + 0x18),
                    field("attributes"),
                    format!("checksum type {x} is not known; the checksum is not verified"),
                ),
            }
            partitions.push(p);
        }

        // Classification: the FSBL is the PS partition the boot header points at.
        let fsbl_idx = partitions.iter().position(|p| {
            p.destination == Destination::Ps
                && header.fsbl_length != 0
                && p.data_offset == header.fsbl_offset
        });
        for (i, p) in partitions.iter_mut().enumerate() {
            p.kind = if Some(i) == fsbl_idx {
                PartitionKind::Fsbl
            } else {
                match p.destination {
                    Destination::Ps => PartitionKind::Application,
                    Destination::Pl => PartitionKind::Bitstream,
                    _ => PartitionKind::Other,
                }
            };
            if p.kind == PartitionKind::Bitstream {
                let r = p.data_range();
                let head = bytes
                    .get(r.start as usize..(r.end.min(r.start + 256)) as usize)
                    .unwrap_or(&[]);
                let be = WIDTH_DETECT.to_be_bytes();
                let le = WIDTH_DETECT.to_le_bytes();
                let found = head.windows(4).any(|w| w == be || w == le);
                p.sync_word_found = Some(found);
                if !found {
                    issue(
                        &mut issues,
                        Severity::Warning,
                        Some(p.data_offset as usize),
                        format!("partition[{}].data", p.index),
                        "no bitstream sync word 0xAA995566 in the first 256 bytes: this does not \
                         look like a configuration bitstream",
                    );
                }
            }
        }
        if fsbl_idx.is_none() {
            issue(
                &mut issues,
                Severity::Warning,
                Some(0x30),
                "boot_header.fsbl_offset",
                if header.fsbl_length == 0 {
                    "the image has no FSBL (length 0); the BootROM could not start it".to_string()
                } else {
                    format!(
                        "no PS partition starts at the FSBL source offset {:#x}",
                        header.fsbl_offset
                    )
                },
            );
        }

        // Application partitions must not overlap, and the entry must lie in one.
        let mut apps: Vec<&Partition> = partitions
            .iter()
            .filter(|p| p.kind == PartitionKind::Application && p.length > 0)
            .collect();
        apps.sort_by_key(|p| p.load_address);
        for pair in apps.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if u64::from(a.load_address) + u64::from(a.length) > u64::from(b.load_address) {
                issue(
                    &mut issues,
                    Severity::Warning,
                    Some(b.header_offset as usize),
                    format!("partition[{}].load_address", b.index),
                    format!(
                        "overlaps partition {}: {:#x}..{:#x} against {:#x}",
                        a.index,
                        a.load_address,
                        u64::from(a.load_address) + u64::from(a.length),
                        b.load_address
                    ),
                );
            }
        }

        let image = BootImage {
            file_size: len as u64,
            header,
            register_init,
            image_table,
            images,
            partitions,
            issues,
        };
        let mut image = image;
        let entry_info = image
            .entry_partition()
            .map(|e| (e.index, e.exec_address, e.header_offset));
        if let Some((idx, exec, ho)) = entry_info {
            let entry = u64::from(exec);
            let inside = image.partitions.iter().any(|p| {
                p.kind == PartitionKind::Application
                    && entry >= u64::from(p.load_address)
                    && entry < u64::from(p.load_address) + u64::from(p.length)
            });
            if !inside {
                issue(
                    &mut image.issues,
                    Severity::Warning,
                    Some(ho as usize + 0x10),
                    format!("partition[{idx}].exec_address"),
                    format!(
                        "execution address {entry:#x} is outside every application partition \
                         (a handoff into memory the image does not fill)"
                    ),
                );
            }
        }
        Ok(image)
    }

    /// Findings that make the image unusable.
    pub fn errors(&self) -> Vec<&Issue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect()
    }

    /// Findings that do not.
    pub fn warnings(&self) -> Vec<&Issue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .collect()
    }

    /// The FSBL partition, when the boot header points at one.
    pub fn fsbl(&self) -> Option<&Partition> {
        self.partitions
            .iter()
            .find(|p| p.kind == PartitionKind::Fsbl)
    }

    /// PL bitstream partitions.
    pub fn bitstreams(&self) -> impl Iterator<Item = &Partition> {
        self.partitions
            .iter()
            .filter(|p| p.kind == PartitionKind::Bitstream)
    }

    /// PS application partitions (every loadable segment of every application ELF / binary).
    pub fn applications(&self) -> impl Iterator<Item = &Partition> {
        self.partitions
            .iter()
            .filter(|p| p.kind == PartitionKind::Application)
    }

    /// The partition core 0 starts at: the first application partition with a non-zero
    /// execution address.
    ///
    /// Which partition the real FSBL hands off to is FSBL logic, not part of the BootROM
    /// format (`VERIFY:`); this is the choice ZynqSim documents.
    pub fn entry_partition(&self) -> Option<&Partition> {
        self.applications().find(|p| p.exec_address != 0)
    }

    /// Every partition with a non-zero execution address beyond the first: other
    /// application images that are loaded but not started.
    pub fn other_entries(&self) -> Vec<&Partition> {
        self.applications()
            .filter(|p| p.exec_address != 0)
            .skip(1)
            .collect()
    }

    /// The data of partition `index` as `bytes` (the file this image was parsed from) holds it.
    pub fn payload<'a>(&self, bytes: &'a [u8], index: usize) -> Result<&'a [u8], BootError> {
        let p = self.partitions.get(index).ok_or_else(|| BootError {
            kind: ErrorKind::Malformed,
            offset: None,
            message: format!(
                "partition {index} does not exist ({} partitions)",
                self.partitions.len()
            ),
        })?;
        if p.encrypted_length != p.length {
            return Err(BootError {
                kind: ErrorKind::Malformed,
                offset: Some(u64::from(p.header_offset)),
                message: format!("partition {index} is encrypted and cannot be extracted"),
            });
        }
        let r = p.data_range();
        bytes
            .get(r.start as usize..r.end as usize)
            .ok_or_else(|| BootError {
                kind: ErrorKind::Truncated,
                offset: Some(r.start),
                message: format!(
                    "partition {index} data {:#x}..{:#x} lies outside the file ({} bytes)",
                    r.start,
                    r.end,
                    bytes.len()
                ),
            })
    }

    /// What ZynqSim would load and where it would start: refuses images with errors, images
    /// without an application and images with no entry point. `bytes` is the file.
    pub fn load_plan(&self, bytes: &[u8]) -> Result<LoadPlan, BootError> {
        let errs = self.errors();
        if !errs.is_empty() {
            let list: Vec<String> = errs.iter().map(|i| i.to_string()).collect();
            return Err(BootError {
                kind: ErrorKind::Malformed,
                offset: errs[0].offset,
                message: format!(
                    "the boot image has {} error(s): {}",
                    list.len(),
                    list.join("; ")
                ),
            });
        }
        let Some(entry) = self.entry_partition() else {
            let apps = self.applications().count();
            return Err(BootError {
                kind: ErrorKind::Malformed,
                offset: None,
                message: if apps == 0 {
                    "the boot image has no application partition for the PS (only an FSBL and/or \
                     a bitstream): there is nothing to run"
                        .to_string()
                } else {
                    format!(
                        "none of the {apps} application partition(s) has an execution address: \
                         the entry point is unknown"
                    )
                },
            });
        };
        let mut segments = Vec::new();
        for p in self.applications() {
            if p.length == 0 {
                continue;
            }
            segments.push(Segment {
                partition: p.index,
                load_address: p.load_address,
                data: self.payload(bytes, p.index)?.to_vec(),
            });
        }
        Ok(LoadPlan {
            entry: entry.exec_address,
            segments,
        })
    }

    /// A multi-line human description (`zynqsim boot-info`).
    pub fn describe(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let h = &self.header;
        let _ = writeln!(
            s,
            "boot image: {} bytes, header version {:#010x}, {}",
            self.file_size,
            h.header_version,
            if h.encryption_status == 0 {
                "not encrypted".to_string()
            } else {
                format!("ENCRYPTED ({:#010x})", h.encryption_status)
            }
        );
        let _ = writeln!(
            s,
            "fsbl: offset {:#x}, {} bytes, load {:#010x}, exec {:#010x} (not executed by ZynqSim)",
            h.fsbl_offset, h.fsbl_length, h.fsbl_load_address, h.fsbl_exec_address
        );
        let _ = writeln!(
            s,
            "register init: {} pair(s) (noted, not applied)",
            self.register_init.len()
        );
        for r in &self.register_init {
            let _ = writeln!(s, "  {:#010x} <- {:#010x}", r.address, r.value);
        }
        let _ = writeln!(s, "partitions: {}", self.partitions.len());
        for p in &self.partitions {
            let kind = match p.kind {
                PartitionKind::Fsbl => "fsbl",
                PartitionKind::Bitstream => "bitstream",
                PartitionKind::Application => "application",
                PartitionKind::Other => "other",
            };
            let _ = write!(
                s,
                "  [{}] {:<11} {:<16} offset {:#08x} len {:>7} load {:#010x} exec {:#010x}",
                p.index,
                kind,
                p.image.as_deref().unwrap_or("?"),
                p.data_offset,
                p.length,
                p.load_address,
                p.exec_address,
            );
            match p.checksum_ok {
                Some(true) => {
                    let _ = write!(s, " md5 ok");
                }
                Some(false) => {
                    let _ = write!(s, " md5 MISMATCH");
                }
                None => {}
            }
            let _ = writeln!(s);
        }
        if let Some(e) = self.entry_partition() {
            let _ = writeln!(
                s,
                "entry (core 0): {:#010x} (partition {})",
                e.exec_address, e.index
            );
        }
        for i in &self.issues {
            let sev = match i.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let _ = writeln!(s, "{sev}: {i}");
        }
        s
    }
}

#[cfg(test)]
mod tests;
