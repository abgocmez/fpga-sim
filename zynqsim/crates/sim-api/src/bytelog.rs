use serde::{Deserialize, Serialize};

/// Bytes of history retained per stream.
const LOG_CAP: usize = 1 << 20;

/// A bounded byte stream addressed by absolute offset.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ByteLog {
    /// Absolute offset of `data[0]`.
    pub(crate) base: u64,
    pub(crate) data: Vec<u8>,
}

impl ByteLog {
    pub(crate) fn push(&mut self, b: u8) {
        self.data.push(b);
        if self.data.len() > LOG_CAP {
            let drop = self.data.len() - LOG_CAP;
            self.data.drain(..drop);
            self.base += drop as u64;
        }
    }

    pub(crate) fn end(&self) -> u64 {
        self.base + self.data.len() as u64
    }

    /// Returns up to `max` bytes at or after `from` and the next offset.
    pub(crate) fn read(&self, from: u64, max: usize) -> (Vec<u8>, u64) {
        let start = from.max(self.base).min(self.end());
        let off = (start - self.base) as usize;
        let n = max.min(self.data.len() - off);
        (self.data[off..off + n].to_vec(), start + n as u64)
    }
}
