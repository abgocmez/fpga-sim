//! The address-translation hook. The MMU is a later task; until then every access is
//! physical = virtual through [`Identity`].

/// What an access is for, as the translation table walk and permission checks need it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// An instruction fetch.
    Fetch,
    /// A data read.
    Read,
    /// A data write.
    Write,
}

/// Maps a virtual address to a physical one. A failure is the data/prefetch fault status
/// (DFSR/IFSR encoding) the core reports.
pub trait Translate {
    /// Translates `va` for an `access`.
    fn translate(&mut self, va: u32, access: Access) -> Result<u64, u8>;
}

/// The default: physical address = virtual address, no faults.
#[derive(Clone, Copy, Debug, Default)]
pub struct Identity;

impl Translate for Identity {
    fn translate(&mut self, va: u32, _access: Access) -> Result<u64, u8> {
        Ok(u64::from(va))
    }
}
