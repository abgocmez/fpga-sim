<!--
Integration note: `docs/TEST_YOUR_DESIGN.md` is not under version control in the branch this was
written on, so this section lives in its own file. Paste it into TEST_YOUR_DESIGN.md before
"Limits worth knowing" (it is written as a `##` section) and delete this file.
-->

## Network (PS Ethernet)

A project gets the Zybo's Ethernet (GEM0) only when it has a `[network]` section; without one the
NIC is unplugged. No PL is needed: this works with `pl = "demo"`.

```toml
[network]
mac = "00:0a:35:00:1e:53"     # optional: GEM0's address register at reset
subnet = "10.0.2.0/24"        # optional: the simulated LAN (prefix 8..27)
gateway = "10.0.2.2"          # optional: router and DHCP server, default subnet + 2
restrict = true               # default: the guest cannot reach the real network
```

The LAN is QEMU's user-mode stack: it answers ARP, ICMP echo (ping) and DHCP for the gateway
address by itself, so a firmware that brings the GEM up gets replies with no peer to write. Your
firmware drives the GEM at `0xE000B000` (see `firmware/net_demo/src/gem.rs` for a complete Rust
driver: MDIO PHY scan, descriptor rings, polled RX/TX under smoltcp). Print a result line on UART1
as usual.

```
zynqsim run --project mytest/project.toml --for 10ms --pcap net.pcap --json
```

* `--pcap` writes every frame the board sent or received, with virtual-time stamps; the same
  project gives the same bytes on every run, so a golden `.pcap` is a valid regression check.
* The JSON has a `network` object: frame and byte counters per direction and per protocol.
* MCP: `net_capture` returns the frames (paged) and the counters; `status_get` carries the
  counters. `net_send` is not available (ADR-011).
* Assert on `uart["1"]` and the counters, and check the order of the frames in the capture
  (`backend-qemu/tests/net.rs`, `crates/cli/tests/net.rs` show how).
* Wall-clock independent: frames are produced inside the guest's transmit instruction, so
  timestamps and round-trip times measured by the firmware are reproducible.
* Not simulated: the RTL8211E PHY and RGMII (QEMU's Marvell-like PHY model answers MDIO at
  address 7, and always reports link up), cable events, injection of host-made frames, custom peers.
  `restrict = false` lets the guest out to the host's network and is not reproducible.
