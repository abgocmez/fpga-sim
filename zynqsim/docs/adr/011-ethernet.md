# ADR-011: The board's Ethernet is QEMU's user-mode network, captured to `.pcap` by the host

Status: **Accepted**. Date: 2026-10-15.

## Context

Prompt §14/P2 asks for an "lwIP ping": firmware drives the PS Gigabit Ethernet (Cadence GEM0)
and gets an echo reply from a simulated peer. §6 names the mechanism for the QEMU phase
(`-nic user` with `.pcap` export), §11 wants link, counters, a packet list, `.pcap` export and
port forwarding, and §13 lists `net_send` / `net_capture`.

What was found on this machine (Xilinx QEMU 6.1 for Windows, Vitis 2022.1), by running it:

* The project already starts `-M arm-generic-fdt-7series` with `zynq-arm-cosim.dtb`. In that tree
  `ethernet@e000b000` (GEM0) is `status = "okay"` with a `marvell,88e1116` PHY node at MDIO
  address 7 and `local-mac-address = 00:0a:35:00:1e:53`; GEM1 is `disabled` in the tree but QEMU
  creates it anyway.
* Netdev backends are `socket`, `hubport`, `tap`, `user`. The first `-net nic,netdev=ID` binds to
  GEM0 (`info network` / `info qtree`: `ethernet@e000b000 ... netdev = "ID"`); `macaddr=` becomes
  the device's MAC and, as a side effect, the reset value of its address register.
* **Without any `-net` option QEMU attaches an implicit user-mode network to the GEM.** Every
  project this simulator has run so far therefore had a hidden, unrecorded NIC (and, with
  slirp's defaults, a path to the host's network).
* The GEM's internal PHY model answers at MDIO address 7 here (ID registers `0141:0e50`, a Marvell
  part), not at the Zybo's RTL8211E address.

## Options

| | A. `-netdev socket` + Rust peer in the orchestrator | B. `-netdev user` + `filter-dump` (chosen) | C. TAP |
|---|---|---|---|
| Admin rights / driver | none (loopback TCP, 127.0.0.1) | none | needs a TAP driver and admin |
| Who answers ARP / ping / DHCP | our code: must be written, extended per protocol | slirp, inside QEMU: ARP, ICMP echo, DHCP, DNS, TCP/UDP NAT, port forwarding | the real network |
| Frames visible to the host | yes, as they cross the socket | yes, through QEMU's `filter-dump` file | no (separate capture tool) |
| Delivery to the guest | **asynchronous**: the reply is read by QEMU's main loop at a host-scheduler-dependent moment, i.e. at a different guest instruction on every run | **synchronous**: slirp is called from the transmitting vCPU thread and its answer reaches the GEM before the guest's MMIO write returns | not reproducible |
| Host-side injection (`net_send`) | easy | not possible | possible, not reproducible |

Reasoning about A comes from QEMU's threading model (socket netdevs are serviced by the main
loop, the vCPU runs without the global lock under `-icount`); the socket variant was **not**
built, so its jitter is not measured here. B was measured: see below.

## Decision

* `[network]` in `project.toml` (and nothing else) attaches GEM0 to a user-mode network:

  ```
  -netdev user,id=zsnet0,net=10.0.2.0/24,host=10.0.2.2,restrict=on
  -net nic,netdev=zsnet0,macaddr=00:0a:35:00:1e:53
  -object filter-dump,id=zsdump0,netdev=zsnet0,file=<run dir>/net.pcap
  ```

  Keys: `enable`, `mac`, `subnet`, `gateway` (default: subnet + 2), `restrict` (default true).
  Without a `[network]` section (or with `enable = false`) the engine passes `-net none`, so
  **the NIC is genuinely unplugged** instead of silently using QEMU's implicit network.
* `restrict=on` is the default: slirp then answers only for itself (gateway ARP, ICMP echo,
  DHCP, with no router/DNS option) and the guest cannot reach the machine's real network. That
  is what keeps a run reproducible. `restrict = false` is allowed and flagged by a validation
  warning: traffic that leaves the simulation depends on the host.
* The host never sits in the data path. It **reads QEMU's capture at every sync boundary**,
  exactly like the `file:` UART: the transmit, slirp's answer and the capture write all happen
  inside one vCPU instruction, and the vCPU is stopped when a sync is held, so the file is
  complete for that quantum (`QemuMachine::net_take`, `backend-qemu::net::PcapTail`).
* **Timestamps.** QEMU writes `tv_sec = wall-clock start seconds + virtual seconds` and
  `tv_usec = virtual microseconds`; the file is therefore not byte-reproducible. The host
  recovers the virtual microsecond from the quantum window the record was found in
  (`resolve_virtual_ns`) and writes **its own** `.pcap` (`--pcap FILE`, `Query::NetPcap`) with
  virtual-time stamps, which is byte-identical between runs. Resolution is 1 us (QEMU's).
* **Direction** is by source MAC: slirp's own frames start `52:55` (QEMU's "special" Ethernet
  addresses); `[network] mac` is refused if it starts that way. Everything else was sent by the
  board.
* **Trace.** Each frame becomes a `Marker` (`net tx 74B ICMP echo-request 10.0.2.15 > 10.0.2.2
  id=0x2b2b seq=0 data=32 fnv=1a2b3c4d`) and a `Signal` (`net.tx_frames` / `net.rx_frames`),
  stamped with the frame's virtual time. The marker carries a hash of the whole frame, so the
  trace digest covers frame **content, order and time**.
* `Capability::Network` is advertised when a NIC is attached. `Query::NetStats` (counters:
  frames/bytes per direction, ARP/ICMP/UDP/TCP/other, bad checksums), `Query::NetCapture`
  (paged frames with summaries; the newest 8192 are retained, counters and the trace cover all)
  and `Query::NetPcap` serve the front ends: CLI `run --pcap`, MCP `net_capture` and
  `status_get`, GUI side panel.
* **Firmware** (`firmware/net_demo`, Rust `no_std`): a small Cadence GEM driver (MDIO PHY scan
  as the Xilinx lwIP port does, reset and auto-negotiation, descriptor rings in DDR, polled
  RX/TX) under **smoltcp** (ARP, IPv4, ICMP echo, DHCPv4 client). smoltcp is the Rust
  counterpart of the lwIP stack of the Xilinx BSP; lwIP itself is C and this project authors no
  C. The firmware gets an address by DHCP, falls back to 10.0.2.15/24 after 50 ms of silence,
  pings the DHCP server (or router) four times with 32 bytes and prints
  `ping reply seq=N bytes=M ... rtt=Tus`. The round trip is measured with the Cortex-A9 global
  timer.

## Evidence

* `backend-qemu/tests/net.rs`: the demo exchanges 14 frames (DISCOVER, OFFER, REQUEST, ACK, ARP
  request/reply, 4 echo request/reply pairs), all checksums verify, and **two runs give the
  same frame bytes, the same virtual timestamps and the same UART text including the measured
  round trips**.
* `zynqsim/tests/net.rs`: two separate processes give the same trace digest, the same final
  time, the same UART and a byte-identical `.pcap`; record and replay reproduce the digest.
* `mcp/tests/net.rs`: snapshot, restore and `session_replay` reproduce the digest of a network
  session.

## Consequences and limits

* **Record/replay and snapshots** need no new journal entries: the network has no host-side
  input, so a session is still its command journal, and a restore re-executes it and must
  reproduce the digest (which now includes the frames). A different frame order or timing is
  caught as a digest mismatch, as for any other divergence.
* **No injection.** `net_send` answers `unsupported` with the reason. There is no way to hand
  slirp a frame, and a frame fed from outside (A, or a hub) would reach the GEM at a
  host-dependent instruction. If injection is wanted later, the cost is: a `-netdev socket`
  behind a `hubport` with slirp, a Rust peer on that socket, and documenting that only the
  *content and order* of such frames are reproducible, not their arrival instruction.
* **No custom peer** (UDP echo, fault injection, latency, packet loss): that needs the same
  socket peer. slirp's behaviour is what it is.
* **Not the real PHY.** The firmware talks to QEMU's Marvell-like PHY model; the board's
  RTL8211E, RGMII timing and the MIO pin mux are not simulated. QEMU's PHY reports link up even
  with no network attached (there is no cable model), so `link=up` in the UART line says
  nothing about a cable. GEM1 is created by QEMU but wired to nothing; its "no peer" warning in
  `qemu.err.log` is expected.
* **Global timer rate.** In this QEMU the A9 global timer counts 100 MHz (one tick per 10 ns of
  virtual time, measured against the frame timestamps), while real silicon counts PERIPHCLK =
  CPU_3x2x (333 MHz at the stock clocking; VERIFY UG585). `firmware/net_demo` hard-codes the
  QEMU value (`GT_HZ`); a hardware build must change it. The SLCR the model exposes (ARM PLL
  FDIV 26, divisor 2) would suggest 216 MHz, so it is not a usable source either.
* **GEM register values and descriptor layouts** were written from memory of `xemacps_hw.h` /
  UG585 (`VERIFY:`); what is verified is that QEMU's `cadence_gem` model accepts them. Interrupt
  driven RX/TX, jumbo frames, checksum offload, priority queues and the second GEM are not
  exercised.
* **Unmodified Xilinx lwIP/xemacps applications were not run**: their sources are C and are not
  in this repository.
* Third-party code in the firmware workspace (`firmware/Cargo.lock`): smoltcp 0.12 (0BSD),
  heapless, hash32, byteorder, managed, bitflags, cfg-if, stable_deref_trait. `THIRD_PARTY.md`
  is generated from the host workspace only and does not list them.
* With `restrict = false` the guest can reach the host's network (slirp NAT, host ICMP, DNS);
  frames then depend on the host and are not reproducible. Not tested beyond "DHCP offers a
  router and the gateway still answers ping".
