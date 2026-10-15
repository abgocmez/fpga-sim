//! ARM and Thumb disassembly for the debugger (pure Rust, `yaxpeax-arm`).

use yaxpeax_arch::{Decoder, U8Reader};
use yaxpeax_arm::armv7::InstDecoder;

/// One disassembled instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Address of the instruction.
    pub addr: u32,
    /// Encoded size in bytes (2 or 4).
    pub size: u32,
    /// Raw bytes as lower-case hex, in memory order.
    pub hex: String,
    /// Assembly text, or a `.word`/`.short` directive for undecodable bytes.
    pub text: String,
}

/// Disassembles up to `count` instructions from `bytes` located at `base`.
/// Stops early when the bytes run out.
pub fn disassemble(bytes: &[u8], base: u32, thumb: bool, count: usize) -> Vec<Line> {
    let decoder = if thumb {
        InstDecoder::default_thumb()
    } else {
        InstDecoder::default()
    };
    let mut out = Vec::new();
    let mut off = 0usize;
    while out.len() < count && off < bytes.len() {
        let rest = &bytes[off..];
        let mut reader = U8Reader::new(rest);
        let (size, text) = match decoder.decode(&mut reader) {
            Ok(inst) => {
                let size = if thumb && !inst.wide() { 2 } else { 4 };
                (size, inst.to_string())
            }
            Err(_) => {
                // Not an instruction (data, or beyond the decoder): show it as data.
                if thumb && rest.len() >= 2 {
                    let h = u16::from_le_bytes([rest[0], rest[1]]);
                    (2, format!(".short {h:#06x}"))
                } else if rest.len() >= 4 {
                    let w = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                    (4, format!(".word {w:#010x}"))
                } else {
                    break;
                }
            }
        };
        if off + size > bytes.len() {
            break;
        }
        out.push(Line {
            addr: base.wrapping_add(off as u32),
            size: size as u32,
            hex: bytes[off..off + size]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            text,
        });
        off += size;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_instructions_decode_with_addresses_and_sizes() {
        // mov r0, #1 ; bx lr
        let code = [0x01, 0x00, 0xA0, 0xE3, 0x1E, 0xFF, 0x2F, 0xE1];
        let l = disassemble(&code, 0x0010_0000, false, 10);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].addr, l[0].size), (0x0010_0000, 4));
        assert_eq!(l[0].hex, "0100a0e3");
        assert_eq!(l[0].text, "mov r0, 0x1");
        assert_eq!(l[1].addr, 0x0010_0004);
        assert_eq!(l[1].text, "bx lr");
    }

    #[test]
    fn thumb_mixes_two_and_four_byte_instructions() {
        // movs r0, #1 (2 bytes) ; bl <+0> (4 bytes)
        let code = [0x01, 0x20, 0x00, 0xF0, 0x00, 0xF8];
        let l = disassemble(&code, 0x0400_0000, true, 10);
        assert_eq!(l.len(), 2, "{l:?}");
        assert_eq!((l[0].size, l[0].text.as_str()), (2, "movs r0, 0x1"));
        assert_eq!(l[1].size, 4);
        assert!(l[1].text.starts_with("bl"), "{}", l[1].text);
        assert_eq!(l[1].addr, 0x0400_0002);
    }

    #[test]
    fn count_and_truncation_are_respected() {
        let code = [0u8; 10];
        assert_eq!(disassemble(&code, 0, false, 1).len(), 1);
        // 10 bytes hold two whole ARM instructions, the last 2 bytes are dropped.
        assert_eq!(disassemble(&code, 0, false, 99).len(), 2);
        assert!(disassemble(&[], 0, false, 4).is_empty());
    }

    #[test]
    fn a_real_function_disassembles_without_data_lines() {
        // The first bytes of main() in math_test.elf: a push and register moves.
        let elf = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/elf/math_test.elf"),
        )
        .unwrap();
        // File offset of vaddr 0x100614 from the program headers.
        let u32_at = |o: usize| u32::from_le_bytes(elf[o..o + 4].try_into().unwrap());
        let (phoff, phentsize, phnum) = (
            u32_at(28) as usize,
            u16::from_le_bytes([elf[42], elf[43]]) as usize,
            u16::from_le_bytes([elf[44], elf[45]]) as usize,
        );
        let off = (0..phnum)
            .find_map(|i| {
                let ph = phoff + i * phentsize;
                let (kind, offset, vaddr, filesz) =
                    (u32_at(ph), u32_at(ph + 4), u32_at(ph + 8), u32_at(ph + 16));
                (kind == 1 && (vaddr..vaddr + filesz).contains(&0x0010_0614))
                    .then(|| (offset + (0x0010_0614 - vaddr)) as usize)
            })
            .expect("a PT_LOAD contains main");
        let l = disassemble(&elf[off..off + 64], 0x0010_0614, false, 16);
        assert_eq!(l.len(), 16);
        assert!(
            l.iter().all(|x| !x.text.starts_with('.')),
            "undecoded: {l:#?}"
        );
        assert!(
            l.iter()
                .any(|x| x.text.contains("push") || x.text.contains("stm")),
            "{l:#?}"
        );
    }
}
