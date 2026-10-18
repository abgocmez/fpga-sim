//! The Zynq-7000 PS as one bus: the devices of this crate attached at the addresses of
//! `mem_sys::ZYNQ_MAP`, with DDR and the on-chip RAM provided by the bus itself.

use crate::can::Can;
use crate::devcfg::DevCfg;
use crate::gpio::Gpio;
use crate::i2c::I2c;
use crate::mpcore::MpCore;
use crate::slcr::Slcr;
use crate::spi::Spi;
use crate::ttc::Ttc;
use crate::uart::{UART0_IRQ, UART1_IRQ, Uart};
use mem_sys::{
    DDR_DEFAULT, Device, MapError, ReadOnlyImage, Stub, SystemBus, UnmappedPolicy, find, shared,
};
use std::sync::{Arc, Mutex};

/// What to build the PS with.
pub struct PsConfig {
    /// DDR size in bytes (a multiple of 64 KiB, at most 1 GiB).
    pub ddr_size: u64,
    /// A flash image for the QSPI linear window, if any.
    pub qspi_image: Option<Vec<u8>>,
    /// What unmapped accesses do.
    pub policy: UnmappedPolicy,
    /// The UART reference clock in hertz.
    pub uart_ref_hz: u64,
}

impl Default for PsConfig {
    fn default() -> Self {
        PsConfig {
            ddr_size: DDR_DEFAULT,
            qspi_image: None,
            policy: UnmappedPolicy::Fault,
            uart_ref_hz: crate::clock::UART_REF_HZ,
        }
    }
}

/// Host handles to devices the bus owns: the UARTs (transmit log and receive input), the MPCore
/// region (watchdog reset requests), and the models that take host stimulus or show outputs.
pub struct PsHandles {
    /// UART0 (0xE0000000).
    pub uart0: Arc<Mutex<Uart>>,
    /// UART1 (0xE0001000).
    pub uart1: Arc<Mutex<Uart>>,
    /// The MPCore region, for `take_wdt_resets` (the watchdog's `WdtReset` events).
    pub mpcore: Arc<Mutex<MpCore>>,
    /// GPIO (0xE000A000): pin inputs and the output sink.
    pub gpio: Arc<Mutex<Gpio>>,
    /// SPI0 and SPI1 (0xE0006000, 0xE0007000): attach a slave with `set_peer`.
    pub spi: [Arc<Mutex<Spi>>; 2],
    /// I2C0 and I2C1 (0xE0004000, 0xE0005000): attach devices with `set_peer`.
    pub i2c: [Arc<Mutex<I2c>>; 2],
    /// CAN0 and CAN1 (0xE0008000, 0xE0009000).
    pub can: [Arc<Mutex<Can>>; 2],
    /// Device configuration and XADC (0xF8007000): the XADC's host temperature and voltage.
    pub devcfg: Arc<Mutex<DevCfg>>,
}

fn attach(bus: &mut SystemBus, name: &str, dev: Box<dyn Device + Send>) -> Result<(), MapError> {
    let e = find(name).unwrap_or_else(|| panic!("{name} is not in the address map"));
    bus.attach(e.name, e.base, e.size, dev)
}

/// Builds the PS bus: DDR and the on-chip RAM, UART0 and UART1, SLCR, TTC0 and TTC1, the
/// GPIO, SPI0/1, I2C0/1 and CAN0/1 controllers, the device configuration with the XADC, the DDRC
/// stub, the MPCore private region, and (if an image is given) the QSPI linear window. Blocks
/// listed in the map and not in this list are unmapped.
pub fn zynq_system(cfg: &PsConfig) -> Result<(SystemBus, PsHandles), MapError> {
    let mut bus = SystemBus::new(cfg.ddr_size)?;
    bus.set_policy(cfg.policy);
    let (u0, uart0) = shared(Uart::new(UART0_IRQ).with_ref_clock(cfg.uart_ref_hz));
    let (u1, uart1) = shared(Uart::new(UART1_IRQ).with_ref_clock(cfg.uart_ref_hz));
    attach(&mut bus, "UART0", Box::new(u0))?;
    attach(&mut bus, "UART1", Box::new(u1))?;
    attach(&mut bus, "SLCR", Box::new(Slcr::new()))?;
    attach(&mut bus, "TTC0", Box::new(Ttc::new(0)))?;
    attach(&mut bus, "TTC1", Box::new(Ttc::new(1)))?;
    attach(&mut bus, "DDRC", Box::new(Stub::new("DDRC")))?;
    let (dc, devcfg) = shared(DevCfg::new());
    attach(&mut bus, "DEVCFG", Box::new(dc))?;
    attach(&mut bus, "EFUSE", Box::new(Stub::new("EFUSE")))?;
    let (gp, gpio) = shared(Gpio::new());
    attach(&mut bus, "GPIO", Box::new(gp))?;
    let (s0, spi0) = shared(Spi::new(0));
    let (s1, spi1) = shared(Spi::new(1));
    attach(&mut bus, "SPI0", Box::new(s0))?;
    attach(&mut bus, "SPI1", Box::new(s1))?;
    let (i0, i2c0) = shared(I2c::new(0));
    let (i1, i2c1) = shared(I2c::new(1));
    attach(&mut bus, "I2C0", Box::new(i0))?;
    attach(&mut bus, "I2C1", Box::new(i1))?;
    let (c0, can0) = shared(Can::new(0));
    let (c1, can1) = shared(Can::new(1));
    attach(&mut bus, "CAN0", Box::new(c0))?;
    attach(&mut bus, "CAN1", Box::new(c1))?;
    let (mp, mpcore) = shared(MpCore::new());
    attach(&mut bus, "MPCORE", Box::new(mp))?;
    if let Some(img) = &cfg.qspi_image {
        attach(
            &mut bus,
            "QSPI_LINEAR",
            Box::new(ReadOnlyImage::new(img.clone())),
        )?;
    }
    let handles = PsHandles {
        uart0,
        uart1,
        mpcore,
        gpio,
        spi: [spi0, spi1],
        i2c: [i2c0, i2c1],
        can: [can0, can1],
        devcfg,
    };
    Ok((bus, handles))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_system_builds_and_the_map_addresses_work() {
        let (mut bus, _h) = zynq_system(&PsConfig::default()).unwrap();
        // SLCR lock status reads 1 at reset, through the bus.
        assert_eq!(mem_sys::Bus::read(&mut bus, 0xF800_000C, 4), Ok(1));
        // The MPCore timer count is reachable and the DDR is writable.
        assert_eq!(mem_sys::Bus::read(&mut bus, 0xF8F0_0600 + 4, 4), Ok(0));
        mem_sys::Bus::write(&mut bus, 0x1000, 4, 7).unwrap();
        assert_eq!(mem_sys::Bus::read(&mut bus, 0x1000, 4), Ok(7));
    }

    #[test]
    fn the_stubs_and_the_gaps_behave_as_documented() {
        let (mut bus, _h) = zynq_system(&PsConfig::default()).unwrap();
        assert_eq!(
            mem_sys::Bus::read(&mut bus, 0xF800_6000, 4),
            Ok(0),
            "DDRC stub"
        );
        assert_eq!(
            mem_sys::Bus::read(&mut bus, 0xF8F0_2000, 4),
            Ok(0),
            "L2C stub"
        );
        // The DMAC block is in the map but not modelled: unmapped and logged.
        assert!(mem_sys::Bus::read(&mut bus, 0xF800_3000, 4).is_err());
        assert_eq!(bus.unmapped_count(), 1);
    }

    #[test]
    fn a_qspi_image_reads_back_and_erased_bytes_are_ff() {
        let cfg = PsConfig {
            qspi_image: Some(vec![0x12, 0x34]),
            ..PsConfig::default()
        };
        let (mut bus, _h) = zynq_system(&cfg).unwrap();
        assert_eq!(mem_sys::Bus::read(&mut bus, 0xFC00_0000, 2), Ok(0x3412));
        assert_eq!(mem_sys::Bus::read(&mut bus, 0xFC00_0002, 1), Ok(0xFF));
    }

    #[test]
    fn uart_handles_reach_the_device_on_the_bus() {
        let (mut bus, h) = zynq_system(&PsConfig::default()).unwrap();
        mem_sys::Bus::write(&mut bus, 0xE000_0000, 4, 0x10 | 0x04).unwrap(); // enable TX and RX
        assert_eq!(h.uart0.lock().unwrap().inject_rx(b"k"), 1);
        assert_eq!(
            mem_sys::Bus::read(&mut bus, 0xE000_0030, 1),
            Ok(u64::from(b'k'))
        );
    }
}
