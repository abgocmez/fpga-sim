//! Network demo for the Zybo Z7's PS Ethernet (Cadence GEM0).
//!
//! 1. Finds the PHY over MDIO, resets it and waits for auto-negotiation.
//! 2. Programs the GEM descriptor rings and brings the MAC up (see `gem`).
//! 3. Runs a `smoltcp` interface (ARP + IPv4 + ICMP + DHCP), obtains an
//!    address by DHCP (falling back to a static one) and pings the gateway
//!    four times with a 32-byte payload, reporting every reply and its round
//!    trip in simulated time (read from the Cortex-A9 global timer).
//!
//! `smoltcp` is the Rust counterpart of the lwIP stack the Xilinx BSP uses.
//! The host sees every frame (`--pcap`, `net_capture`).
#![no_std]
#![no_main]

mod gem;

use gem::{Gem, bring_up_phy};
use rt::{println, read32, write32};
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::phy::Device;
use smoltcp::socket::{dhcpv4, icmp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address,
    Ipv4Cidr,
};

/// MAC used when the register holds none (real hardware resets it to zero;
/// the simulator initialises it from `[network] mac`). Xilinx OUI.
const DEFAULT_MAC: [u8; 6] = [0x00, 0x0a, 0x35, 0x00, 0x1e, 0x53];
/// Used when no DHCP server answers: QEMU's default user-mode network.
const FALLBACK_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
const FALLBACK_GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

const PINGS: u16 = 4;
const PAYLOAD: usize = 32;
const IDENT: u16 = 0x2b2b;
/// Gap between echo requests, in simulated microseconds.
const INTERVAL_US: u64 = 1_000;
/// Give up on DHCP / on the last reply after this long (simulated us).
const DHCP_TIMEOUT_US: u64 = 50_000;
const PING_TIMEOUT_US: u64 = 20_000;

/// Cortex-A9 global timer (private peripheral region).
const GT: usize = 0xF8F0_0200;
const GT_LO: usize = GT;
const GT_HI: usize = GT + 0x04;
const GT_CTRL: usize = GT + 0x08;
/// Global timer tick rate. The Xilinx QEMU 6.1 model counts one tick per
/// 10 ns of virtual time; measured by comparing the counter with the host's
/// virtual-time stamps of the frames (ADR-011). VERIFY: real silicon counts
/// at PERIPHCLK = CPU_3x2x (333.3 MHz with the stock Zybo clocking, UG585
/// clocking chapter), so a hardware build must change this constant.
const GT_HZ: u64 = 100_000_000;

fn ticks() -> u64 {
    loop {
        let hi = read32(GT_HI);
        let lo = read32(GT_LO);
        if read32(GT_HI) == hi {
            return (u64::from(hi) << 32) | u64::from(lo);
        }
    }
}

fn now_us() -> u64 {
    ticks() * 1_000_000 / GT_HZ
}

fn instant() -> Instant {
    Instant::from_micros(now_us() as i64)
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    println!("net demo: Cadence GEM0 + smoltcp (Rust lwIP counterpart), DHCP then ping");
    write32(GT_CTRL, 1);

    let Some(phy) = bring_up_phy() else {
        println!("PHY: not found on MDIO");
        println!("RESULT: FAIL");
        park();
    };
    println!(
        "PHY: addr={} id={:04x}:{:04x} link={}",
        phy.addr,
        phy.id.0,
        phy.id.1,
        if phy.link { "up" } else { "down" }
    );

    // The MAC register is zero on hardware; the simulator initialises it.
    let lo = read32(gem::GEM0 + 0x088);
    let hi = read32(gem::GEM0 + 0x08C);
    let mut mac = [
        lo as u8,
        (lo >> 8) as u8,
        (lo >> 16) as u8,
        (lo >> 24) as u8,
        hi as u8,
        (hi >> 8) as u8,
    ];
    if mac == [0; 6] {
        mac = DEFAULT_MAC;
    }
    println!(
        "MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let mut device = Gem::new(mac);
    let checksum = device.capabilities().checksum;

    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = 0x5a5a_2024;
    let mut iface = Interface::new(config, &mut device, instant());

    let mut rx_meta = [icmp::PacketMetadata::EMPTY; 4];
    let mut rx_data = [0u8; 512];
    let mut tx_meta = [icmp::PacketMetadata::EMPTY; 4];
    let mut tx_data = [0u8; 512];
    let ping_socket = icmp::Socket::new(
        icmp::PacketBuffer::new(&mut rx_meta[..], &mut rx_data[..]),
        icmp::PacketBuffer::new(&mut tx_meta[..], &mut tx_data[..]),
    );
    let mut storage = [SocketStorage::EMPTY; 2];
    let mut sockets = SocketSet::new(&mut storage[..]);
    let dhcp_handle = sockets.add(dhcpv4::Socket::new());
    let ping_handle = sockets.add(ping_socket);

    // Phase 1: DHCP. The simulated network's gateway is its DHCP server.
    let started = now_us();
    let mut gateway: Option<Ipv4Address> = None;
    while gateway.is_none() {
        let t = now_us();
        iface.poll(Instant::from_micros(t as i64), &mut device, &mut sockets);
        match sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll() {
            Some(dhcpv4::Event::Configured(lease)) => {
                // QEMU's restricted user-mode network leaves the router
                // option out; its DHCP server is the host address, which
                // answers ARP and ping, so that is the target then.
                let target = lease.router.unwrap_or(lease.server.address);
                let took = now_us() - started;
                match lease.router {
                    Some(r) => println!(
                        "DHCP: address={} server={} router={r} after {took}us",
                        lease.address, lease.server.address
                    ),
                    None => println!(
                        "DHCP: address={} server={} router=none after {took}us",
                        lease.address, lease.server.address
                    ),
                }
                iface.update_ip_addrs(|a| {
                    a.clear();
                    let _ = a.push(IpCidr::Ipv4(lease.address));
                });
                if let Some(r) = lease.router {
                    let _ = iface.routes_mut().add_default_ipv4_route(r);
                }
                gateway = Some(target);
            }
            Some(dhcpv4::Event::Deconfigured) | None => {}
        }
        if gateway.is_none() && t - started > DHCP_TIMEOUT_US {
            println!("DHCP: no answer, using static {FALLBACK_IP}/24");
            iface.update_ip_addrs(|a| {
                a.clear();
                let _ = a.push(IpCidr::Ipv4(Ipv4Cidr::new(FALLBACK_IP, 24)));
            });
            let _ = iface.routes_mut().add_default_ipv4_route(FALLBACK_GATEWAY);
            gateway = Some(FALLBACK_GATEWAY);
        }
        rt::delay_us(5);
    }
    let gateway = gateway.unwrap_or(FALLBACK_GATEWAY);
    println!("ping target {gateway}");

    // The classic ping pattern: 'a', 'b', ... wrapping after 'w'.
    let mut payload = [0u8; PAYLOAD];
    for (i, b) in payload.iter_mut().enumerate() {
        *b = b'a' + (i % 23) as u8;
    }

    // Phase 2: ping.
    let mut next_send = now_us();
    let mut sent_at = [0u64; PINGS as usize];
    let mut seq = 0u16;
    let mut replies = 0u16;
    let mut last_send = next_send;
    let mut payload_ok = true;
    loop {
        let t = now_us();
        iface.poll(Instant::from_micros(t as i64), &mut device, &mut sockets);
        let sock = sockets.get_mut::<icmp::Socket>(ping_handle);
        if !sock.is_open() {
            let _ = sock.bind(icmp::Endpoint::Ident(IDENT));
        }
        if sock.can_send() && seq < PINGS && t >= next_send {
            let repr = Icmpv4Repr::EchoRequest {
                ident: IDENT,
                seq_no: seq,
                data: &payload,
            };
            if let Ok(buf) = sock.send(repr.buffer_len(), IpAddress::Ipv4(gateway)) {
                let mut pkt = Icmpv4Packet::new_unchecked(buf);
                repr.emit(&mut pkt, &checksum);
                sent_at[seq as usize] = t;
                last_send = t;
                seq += 1;
                next_send = t + INTERVAL_US;
            }
        }
        while sock.can_recv() {
            let Ok((data, _from)) = sock.recv() else {
                break;
            };
            let Ok(pkt) = Icmpv4Packet::new_checked(data) else {
                continue;
            };
            let Ok(Icmpv4Repr::EchoReply {
                ident,
                seq_no,
                data,
            }) = Icmpv4Repr::parse(&pkt, &checksum)
            else {
                continue;
            };
            if ident != IDENT || seq_no >= PINGS {
                continue;
            }
            let rtt = now_us().saturating_sub(sent_at[seq_no as usize]);
            let same = data == payload;
            payload_ok &= same;
            replies += 1;
            println!(
                "ping reply seq={} bytes={} from={} rtt={}us payload={}",
                seq_no,
                data.len(),
                gateway,
                rtt,
                if same { "ok" } else { "MISMATCH" }
            );
        }
        if replies >= PINGS || (seq >= PINGS && t > last_send + PING_TIMEOUT_US) {
            break;
        }
        rt::delay_us(5);
    }

    println!(
        "net demo: {replies}/{PINGS} replies, frames tx={} rx={}",
        device.counters.tx_frames, device.counters.rx_frames
    );
    if replies == PINGS && payload_ok {
        println!("RESULT: ALL PASS");
    } else {
        println!("RESULT: FAIL");
    }
    park()
}

fn park() -> ! {
    loop {
        // SAFETY: parks the core; nothing wakes it.
        unsafe { core::arch::asm!("wfi") }
    }
}
