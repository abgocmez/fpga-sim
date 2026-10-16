//! What ZynqSim loads from a boot image, and the ELF it is handed to QEMU as.
//!
//! The BootROM format stores an application ELF as raw loadable segments (load address,
//! execution address, bytes); the ELF container is gone. QEMU's `loader` device wants an
//! ELF, so the segments are written back into a minimal ELF32 little-endian ARM executable:
//! one `PT_LOAD` per segment at its load address, `e_entry` = the entry partition's
//! execution address, no symbols.

use serde::Serialize;

/// One loadable block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Segment {
    /// Partition it came from.
    pub partition: usize,
    /// Guest physical address.
    pub load_address: u32,
    /// Bytes.
    #[serde(skip)]
    pub data: Vec<u8>,
}

/// Memory to fill and the address core 0 starts at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LoadPlan {
    /// Execution address of core 0.
    pub entry: u32,
    /// Segments in partition order.
    pub segments: Vec<Segment>,
}

impl LoadPlan {
    /// Total payload bytes.
    pub fn total_bytes(&self) -> usize {
        self.segments.iter().map(|s| s.data.len()).sum()
    }

    /// A minimal ELF32 executable holding the segments (deterministic).
    pub fn to_elf(&self) -> Vec<u8> {
        const EHSIZE: usize = 52;
        const PHSIZE: usize = 32;
        let n = self.segments.len();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x7F, b'E', b'L', b'F', 1, 1, 1, 0]);
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        out.extend_from_slice(&40u16.to_le_bytes()); // EM_ARM
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&self.entry.to_le_bytes());
        out.extend_from_slice(&(EHSIZE as u32).to_le_bytes()); // e_phoff
        out.extend_from_slice(&0u32.to_le_bytes()); // e_shoff
        out.extend_from_slice(&0x0500_0200u32.to_le_bytes()); // EABI v5, soft-float ABI
        out.extend_from_slice(&(EHSIZE as u16).to_le_bytes());
        out.extend_from_slice(&(PHSIZE as u16).to_le_bytes());
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&40u16.to_le_bytes()); // e_shentsize
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        let mut offset = (EHSIZE + n * PHSIZE).next_multiple_of(4);
        for s in &self.segments {
            let len = s.data.len() as u32;
            out.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            out.extend_from_slice(&s.load_address.to_le_bytes()); // p_vaddr
            out.extend_from_slice(&s.load_address.to_le_bytes()); // p_paddr
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&7u32.to_le_bytes()); // RWX
            out.extend_from_slice(&4u32.to_le_bytes());
            offset += s.data.len().next_multiple_of(4);
        }
        for s in &self.segments {
            out.extend_from_slice(&s.data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        }
        out
    }
}
