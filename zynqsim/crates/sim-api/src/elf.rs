//! A minimal ELF32 little-endian symbol reader, enough to turn addresses into
//! function names and back for the debugger. No DWARF.

use crate::error::ApiError;
use std::path::Path;

/// One function or object symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sym {
    /// Name.
    pub name: String,
    /// Start address (the Thumb bit is cleared).
    pub addr: u32,
    /// Size in bytes, 0 if unknown.
    pub size: u32,
}

/// The symbols of an ELF file, sorted by address.
#[derive(Clone, Debug, Default)]
pub struct Symbols {
    syms: Vec<Sym>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

impl Symbols {
    /// Reads `path`.
    pub fn load(path: &Path) -> Result<Symbols, ApiError> {
        let d = std::fs::read(path)
            .map_err(|e| ApiError::io(format!("cannot read {}: {e}", path.display())))?;
        Symbols::parse(&d).map_err(|m| {
            ApiError::invalid_argument(format!("{} is not a usable ELF: {m}", path.display()))
        })
    }

    /// Parses ELF bytes.
    pub fn parse(d: &[u8]) -> Result<Symbols, String> {
        if d.len() < 52 || &d[..4] != b"\x7fELF" {
            return Err("not an ELF file".to_string());
        }
        if d[4] != 1 || d[5] != 1 {
            return Err("only 32-bit little-endian ELF is supported".to_string());
        }
        let shoff = u32_at(d, 32).ok_or("truncated header")? as usize;
        let shentsize = u16_at(d, 46).ok_or("truncated header")? as usize;
        let shnum = u16_at(d, 48).ok_or("truncated header")? as usize;
        let mut syms = Vec::new();
        for i in 0..shnum {
            let sh = shoff + i * shentsize;
            let kind = u32_at(d, sh + 4).ok_or("truncated section table")?;
            if kind != 2 {
                continue; // SHT_SYMTAB
            }
            let off = u32_at(d, sh + 16).ok_or("truncated")? as usize;
            let size = u32_at(d, sh + 20).ok_or("truncated")? as usize;
            let link = u32_at(d, sh + 24).ok_or("truncated")? as usize;
            let strsh = shoff + link * shentsize;
            let stroff = u32_at(d, strsh + 16).ok_or("truncated")? as usize;
            for k in 0..size / 16 {
                let e = off + k * 16;
                let name_ix = u32_at(d, e).ok_or("truncated symbol table")? as usize;
                let value = u32_at(d, e + 4).ok_or("truncated symbol table")?;
                let sz = u32_at(d, e + 8).ok_or("truncated symbol table")?;
                let info = *d.get(e + 12).ok_or("truncated symbol table")?;
                let kind = info & 0xF;
                // FUNC and OBJECT only; skip section/file/notype symbols.
                if kind != 1 && kind != 2 {
                    continue;
                }
                let start = stroff + name_ix;
                let end = d[start..]
                    .iter()
                    .position(|&b| b == 0)
                    .map_or(d.len(), |n| start + n);
                let name = String::from_utf8_lossy(&d[start..end]).into_owned();
                if name.is_empty() {
                    continue;
                }
                syms.push(Sym {
                    name,
                    addr: value & !1,
                    size: sz,
                });
            }
        }
        syms.sort_by_key(|s| (s.addr, s.name.clone()));
        syms.dedup_by(|a, b| a.addr == b.addr && a.name == b.name);
        Ok(Symbols { syms })
    }

    /// Number of symbols.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.syms.len()
    }

    /// True when there are none.
    pub fn is_empty(&self) -> bool {
        self.syms.is_empty()
    }

    /// The symbol by exact name.
    pub fn by_name(&self, name: &str) -> Option<&Sym> {
        self.syms.iter().find(|s| s.name == name)
    }

    /// The symbol containing `addr` (or, for size-less symbols, the nearest
    /// one before it within 4 KiB), and the offset into it.
    pub fn containing(&self, addr: u32) -> Option<(&Sym, u32)> {
        let i = self.syms.partition_point(|s| s.addr <= addr);
        // Walk back over symbols at the same address to prefer sized ones.
        for s in self.syms[..i].iter().rev().take(8) {
            let off = addr - s.addr;
            let inside = if s.size > 0 {
                off < s.size
            } else {
                off < 0x1000
            };
            if inside {
                return Some((s, off));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(n: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/elf")
            .join(n)
    }

    #[test]
    fn symbols_of_a_real_firmware_resolve_both_ways() {
        let s = Symbols::load(&fixture("math_test.elf")).unwrap();
        assert!(s.len() > 20);
        let main = s.by_name("main").expect("main exists");
        assert_eq!(main.addr, 0x0010_0614);
        assert_eq!(main.size, 312);
        let (sym, off) = s.containing(0x0010_0614 + 20).unwrap();
        assert_eq!((sym.name.as_str(), off), ("main", 20));
        // Just past main's end is another function, not main.
        let (after, _) = s.containing(0x0010_0614 + 312).unwrap();
        assert_ne!(after.name, "main");
        assert!(s.containing(0xE000_1000).is_none());
        assert!(s.by_name("no_such_symbol").is_none());
    }

    #[test]
    fn non_elf_input_is_rejected_clearly() {
        assert!(Symbols::parse(b"hello").unwrap_err().contains("ELF"));
        let mut d = vec![0u8; 64];
        d[..4].copy_from_slice(b"\x7fELF");
        d[4] = 2; // 64-bit
        assert!(Symbols::parse(&d).unwrap_err().contains("32-bit"));
    }

    #[test]
    fn the_rust_firmware_resolves_too() {
        let s = Symbols::load(&fixture("rust_hello.elf")).unwrap();
        assert!(s.by_name("main").is_some(), "{} symbols", s.len());
    }
}
