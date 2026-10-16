//! AXI BRAM Controller (`xilinx.com:ip:axi_bram_ctrl:4.1`, Xilinx PG078) with the block memory
//! behind it (`blk_mem_gen`), as one memory window.
//!
//! The PS sees a plain memory: 32-bit reads and writes with byte strobes (sub-word accesses
//! reach the right lanes), little-endian, zero-filled at power-on. The window size is the
//! address segment of the block design (`ADDRESS_SIZE`), a power of two of at least 4 bytes;
//! `C_BRAM_CNT`-style multi-BRAM concatenation is hidden in that one size.
//!
//! Modelled: the memory itself. Not modelled: the ECC registers (the instance is rejected
//! with `C_ECC = 1`, because firmware that reads them would read nothing meaningful), the
//! block memory's second port (a PL design that shares the memory through port B has no
//! access to it here), initial contents from a `.coe` file (`C_INIT_FILE`), read latency and
//! the AXI4 burst protocol (accesses are one 32-bit beat each after the bridge splits them).
//!
//! `VERIFY:` items:
//! * Accesses beyond the end of the memory but inside the window cannot happen (the window is
//!   the memory); the real controller wraps in a larger segment. Here a window larger than the
//!   memory is rejected, so the question does not arise.
//! * Narrow writes with a zero strobe change nothing.

use crate::params::{Params, flag, ranged};
use pl_native::RegisterBlock;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The IP's configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BramConfig {
    /// Size of the memory and of the address window in bytes.
    pub size: u64,
}

impl BramConfig {
    /// Reads the configuration from IP parameters; `window` is the address segment.
    pub fn from_params(p: &Params, window: u64) -> Result<BramConfig, String> {
        if flag(p, "C_ECC", false)? {
            return Err(
                "ECC (C_ECC = 1) is not modelled: the ECC registers are absent".to_string(),
            );
        }
        ranged(p, "C_S_AXI_DATA_WIDTH", 32, 32, 1024)?;
        if window < 4 || !window.is_power_of_two() {
            return Err(format!(
                "the BRAM window ({window:#x} bytes) must be a power of two of at least 4"
            ));
        }
        if window > 1 << 24 {
            return Err(format!(
                "the BRAM window ({window:#x} bytes) is larger than the 16 MiB the model keeps"
            ));
        }
        Ok(BramConfig { size: window })
    }
}

struct State {
    mem: Vec<u8>,
}

/// An AXI BRAM controller with its memory as a [`RegisterBlock`].
pub struct AxiBramCtrl {
    name: String,
    st: Arc<Mutex<State>>,
}

/// A clonable handle to the memory contents: how the host (and tests) preload and inspect it.
#[derive(Clone)]
pub struct BramHandle {
    st: Arc<Mutex<State>>,
}

impl AxiBramCtrl {
    /// A zero-filled memory named `name` and the handle to its contents.
    pub fn new(name: &str, cfg: &BramConfig) -> (AxiBramCtrl, BramHandle) {
        let st = Arc::new(Mutex::new(State {
            mem: vec![0; cfg.size as usize],
        }));
        (
            AxiBramCtrl {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            BramHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl BramHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Size in bytes.
    pub fn size(&self) -> u64 {
        self.lock().mem.len() as u64
    }

    /// Reads `len` bytes at `offset` (clamped to the memory).
    pub fn read(&self, offset: u64, len: usize) -> Vec<u8> {
        let s = self.lock();
        let a = (offset as usize).min(s.mem.len());
        let b = (a + len).min(s.mem.len());
        s.mem[a..b].to_vec()
    }

    /// Writes `data` at `offset` (clamped to the memory).
    pub fn write(&self, offset: u64, data: &[u8]) {
        let mut s = self.lock();
        let a = (offset as usize).min(s.mem.len());
        let n = data.len().min(s.mem.len() - a);
        s.mem[a..a + n].copy_from_slice(&data[..n]);
    }
}

impl RegisterBlock for AxiBramCtrl {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().mem.len() as u64
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let s = self.lock();
        let a = offset as usize;
        if a + 4 > s.mem.len() {
            return 0;
        }
        u32::from_le_bytes([s.mem[a], s.mem[a + 1], s.mem[a + 2], s.mem[a + 3]])
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        let a = offset as usize;
        if a + 4 > s.mem.len() {
            return;
        }
        let b = value.to_le_bytes();
        for (lane, byte) in b.iter().enumerate() {
            if strobe & (1 << lane) != 0 {
                s.mem[a + lane] = *byte;
            }
        }
    }

    /// The memory is not reset by the PL reset: block RAM keeps its contents.
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn bram(size: u64) -> (AxiBramCtrl, BramHandle) {
        let cfg = BramConfig::from_params(&BTreeMap::new(), size).unwrap();
        AxiBramCtrl::new("bram", &cfg)
    }

    #[test]
    fn word_and_byte_access() {
        let (mut b, h) = bram(0x2000);
        assert_eq!(b.size(), 0x2000);
        assert_eq!(b.read32(0), 0);
        b.write32(0x10, 0x1122_3344, 0xF);
        assert_eq!(b.read32(0x10), 0x1122_3344);
        // A single-byte write on lane 2 only.
        b.write32(0x10, 0x00AA_0000, 0b0100);
        assert_eq!(b.read32(0x10), 0x11AA_3344);
        // Zero strobe: nothing.
        b.write32(0x10, 0xFFFF_FFFF, 0);
        assert_eq!(b.read32(0x10), 0x11AA_3344);
        // Last word.
        b.write32(0x1FFC, 0xDEAD_BEEF, 0xF);
        assert_eq!(h.read(0x1FFC, 8), vec![0xEF, 0xBE, 0xAD, 0xDE]);
        // The PL reset leaves the contents alone.
        b.reset();
        assert_eq!(b.read32(0x1FFC), 0xDEAD_BEEF);
    }

    #[test]
    fn host_preload_and_inspect() {
        let (mut b, h) = bram(0x100);
        h.write(4, &[1, 2, 3, 4, 5]);
        assert_eq!(b.read32(4), 0x0403_0201);
        assert_eq!(h.read(8, 100), [vec![5u8], vec![0; 99]].concat());
        h.write(0xFE, &[9, 9, 9, 9]);
        assert_eq!(h.read(0xFE, 4), vec![9, 9], "clamped to the memory");
    }

    #[test]
    fn configuration_errors() {
        let mut p = Params::new();
        assert!(BramConfig::from_params(&p, 3).is_err());
        assert!(
            BramConfig::from_params(&p, 0x3000)
                .unwrap_err()
                .contains("power of two")
        );
        assert!(
            BramConfig::from_params(&p, 1 << 25)
                .unwrap_err()
                .contains("16 MiB")
        );
        p.insert("C_ECC".into(), "1".into());
        assert!(
            BramConfig::from_params(&p, 0x1000)
                .unwrap_err()
                .contains("ECC")
        );
    }
}
