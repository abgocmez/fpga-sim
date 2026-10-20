# XSA import report: `design_1`

Source `fixtures/showcase/design.xsa`; Vivado 2022.1; device zynq 7z020 clg400; board digilentinc.com:zybo-z7-20:part0:1.2.

## Verdict

No IP instance is unmodelled. This is **not** a claim that the design runs correctly: nothing was run by the import. Run `zynqsim run --project project.toml` and read its output. The PS clock tree, MIO and `ps7_init` are not simulated (see *Not applied*).

Passive IP: 3 instance(s) (`ps7_0_axi_periph` AXI Interconnect, `rst_ps7_0_100M` Processor System Reset, `xlconcat_0` Concat) are recognised and carry no registers the firmware can reach, so they are mapped to nothing and are **not** errors.

## Processing system (PS7)

Instance `processing_system7_0` (`xilinx.com:ip:processing_system7:5.5`).

| FCLK | enabled | frequency | reset port |
|---|---|---|---|
| FCLK_CLK0 | yes | 100 MHz | yes |
| FCLK_CLK1 | no | - | no |
| FCLK_CLK2 | no | - | no |
| FCLK_CLK3 | no | - | no |

AXI ports: M_AXI_GP0 yes, M_AXI_GP1 no, S_AXI_GP0 no, S_AXI_GP1 no, S_AXI_HP0-3 no / no / no / no, S_AXI_ACP no.

IRQ_F2P: enabled, 6 bit(s) wide. EMIO GPIO: not used.

| Peripheral | enabled | IO |
|---|---|---|
| UART0 | no | - |
| UART1 | yes | MIO 48 .. 49 |
| ENET0 | yes | MIO 16 .. 27 |
| ENET1 | no | - |
| SD0 | yes | MIO 40 .. 45 |
| SD1 | no | - |
| USB0 | yes | MIO 28 .. 39 |
| USB1 | no | - |
| QSPI | yes | MIO 1 .. 6 |
| I2C0 | no | - |
| I2C1 | no | - |
| SPI0 | no | - |
| SPI1 | no | - |
| CAN0 | no | - |
| CAN1 | no | - |
| TTC0 | no | - |
| TTC1 | no | - |
| WDT | no | - |
| GPIO | yes | MIO |

MIO use (pin(signal)):

* GPIO: 0(gpio[0]), 7(gpio[7]), 9(gpio[9]), 10(gpio[10]), 11(gpio[11]), 12(gpio[12]), 13(gpio[13]), 14(gpio[14]), 15(gpio[15]), 50(gpio[50]), 51(gpio[51])
* Quad SPI Flash: 1(qspi0_ss_b), 2(qspi0_io[0]), 3(qspi0_io[1]), 4(qspi0_io[2]), 5(qspi0_io[3]/HOLD_B), 6(qspi0_sclk), 8(qspi_fbclk)
* Enet 0: 16(tx_clk), 17(txd[0]), 18(txd[1]), 19(txd[2]), 20(txd[3]), 21(tx_ctl), 22(rx_clk), 23(rxd[0]), 24(rxd[1]), 25(rxd[2]), 26(rxd[3]), 27(rx_ctl), 52(mdc), 53(mdio)
* USB 0: 28(data[4]), 29(dir), 30(stp), 31(nxt), 32(data[0]), 33(data[1]), 34(data[2]), 35(data[3]), 36(clk), 37(data[5]), 38(data[6]), 39(data[7])
* SD 0: 40(clk), 41(cmd), 42(data[0]), 43(data[1]), 44(data[2]), 45(data[3]), 47(cd)
* USB Reset: 46(reset)
* UART 1: 48(tx), 49(rx)

## PL instances

| Instance | VLNV | Class | Address | Size | IRQ_F2P | Clock | Disposition |
|---|---|---|---|---|---|---|---|
| `axi_gpio_0` | `xilinx.com:ip:axi_gpio:2.0` | vendor IP: AXI GPIO | 0x41200000 | 0x10000 | [1] (GIC 62) | s_axi_aclk <- FCLK_CLK0 @ 100 MHz | vendor-ip model `axi_gpio` (native, register level; unverified register facts are marked VERIFY in the model docs) |
| `axi_gpio_1` | `xilinx.com:ip:axi_gpio:2.0` | vendor IP: AXI GPIO | 0x41210000 | 0x10000 | [2] (GIC 63) | s_axi_aclk <- FCLK_CLK0 @ 100 MHz | vendor-ip model `axi_gpio` (native, register level; unverified register facts are marked VERIFY in the model docs) |
| `axi_quad_spi_0` | `xilinx.com:ip:axi_quad_spi:3.2` | vendor IP: AXI Quad SPI | 0x41E00000 | 0x10000 | [5] (GIC 66) | ext_spi_clk <- FCLK_CLK0 @ 100 MHz; s_axi_aclk <- FCLK_CLK0 @ 100 MHz | vendor-ip model `axi_quad_spi` (native, register level; unverified register facts are marked VERIFY in the model docs) |
| `axi_timer_0` | `xilinx.com:ip:axi_timer:2.0` | vendor IP: AXI Timer | 0x42800000 | 0x10000 | [3] (GIC 64) | s_axi_aclk <- FCLK_CLK0 @ 100 MHz | vendor-ip model `axi_timer` (native, register level; unverified register facts are marked VERIFY in the model docs) |
| `axi_uartlite_0` | `xilinx.com:ip:axi_uartlite:2.0` | vendor IP: AXI UART Lite | 0x40600000 | 0x10000 | [4] (GIC 65) | s_axi_aclk <- FCLK_CLK0 @ 100 MHz | vendor-ip model `axi_uartlite` (native, register level; unverified register facts are marked VERIFY in the model docs) |
| `ps7_0_axi_periph` | `xilinx.com:ip:axi_interconnect:2.1` | passive IP: AXI Interconnect | - | - | - | ACLK <- FCLK_CLK0; S00_ACLK <- FCLK_CLK0; M00_ACLK <- FCLK_CLK0; M01_ACLK <- FCLK_CLK0; M02_ACLK <- FCLK_CLK0; M03_ACLK <- FCLK_CLK0; M04_ACLK <- FCLK_CLK0; M05_ACLK <- FCLK_CLK0 | passive: wiring only, not simulated (the windows and clocks it routes are listed on the endpoints) |
| `rst_ps7_0_100M` | `xilinx.com:ip:proc_sys_reset:5.0` | passive IP: Processor System Reset | - | - | - | slowest_sync_clk <- FCLK_CLK0 @ 100 MHz | passive: wiring only, not simulated (the windows and clocks it routes are listed on the endpoints) |
| `xlconcat_0` | `xilinx.com:ip:xlconcat:2.1` | passive IP: Concat | - | - | - | - | passive: wiring only, not simulated (the windows and clocks it routes are listed on the endpoints) |
| `zs_fir` | `xilinx.com:module_ref:zs_fir:1.0` | user RTL (module reference) | 0x43C00000 | 0x1000 | [0] (GIC 61) | S_AXI_ACLK <- FCLK_CLK0 @ 100 MHz | RTL in XSim (module `zs_fir`) |

## Address map (PS view)

| Base | Last | Size | Master port | Slave | Interface |
|---|---|---|---|---|---|
| 0x40600000 | 0x4060FFFF | 0x10000 | M_AXI_GP0 | `axi_uartlite_0` | S_AXI |
| 0x41200000 | 0x4120FFFF | 0x10000 | M_AXI_GP0 | `axi_gpio_0` | S_AXI |
| 0x41210000 | 0x4121FFFF | 0x10000 | M_AXI_GP0 | `axi_gpio_1` | S_AXI |
| 0x41E00000 | 0x41E0FFFF | 0x10000 | M_AXI_GP0 | `axi_quad_spi_0` | AXI_LITE |
| 0x42800000 | 0x4280FFFF | 0x10000 | M_AXI_GP0 | `axi_timer_0` | S_AXI |
| 0x43C00000 | 0x43C00FFF | 0x1000 | M_AXI_GP0 | `zs_fir` | S_AXI |

## Project

* PS backend `qemu`, PL backend `xsim`.
* `[rtl]`: top `cosim_top` (generated, `tb/cosim_top.sv`) instantiating `zs_fir` for block-design instance `zs_fir`; the testbench answers every address of the PS's M_AXI_GP0 window with that module, so the address map below is informational for the RTL path.
  * `../../fixtures/showcase/rtl/zs_fir.v`
  * `tb/zs_axi_checker.sv`
  * `tb/cosim_top.sv`
* RTL sources considered: fixtures/showcase/rtl/zs_fir.v.
* `[boot] mode = "boot_bin"`, `file = "BOOT.BIN"`: core 0 starts at the first application partition of the image. What a BOOT.BIN boot does **not** do:
  * The FSBL partition is not executed: QEMU's Zynq machine starts the application directly, so no ps7_init, DDR training or clock/MIO setup runs.
  * The FSBL's register-init table is read and listed, never applied: PS clocks, MIO and peripheral enables have no effect on the simulation.
  * The PL bitstream partition is recognised and reported but cannot be simulated: the PL is the project's RTL (or native models).
  * Boot-ROM timing, the QSPI/SD read of the image, encryption and authentication are not modelled; core 0 starts at the first application partition's execution address.
* `[native]`: 5 vendor-ip model(s) (`axi_gpio_0: axi_gpio`, `axi_gpio_1: axi_gpio`, `axi_quad_spi_0: axi_quad_spi`, `axi_timer_0: axi_timer`, `axi_uartlite_0: axi_uartlite`). They answer in their own address windows; every other PL access goes to the RTL (ADR-016). Interrupts reach the PS at the next lock-step sync (at most one quantum late).

`[connect]`:

* `axi_gpio_0.GPIO` = `board.leds`
* `axi_gpio_0.GPIO2` = `board.sw`
* `axi_gpio_0.S_AXI` = `M_AXI_GP0@0x41200000`
* `axi_gpio_0.ip2intc_irpt` = `IRQ_F2P[1]`
* `axi_gpio_0.s_axi_aclk` = `FCLK_CLK0`
* `axi_gpio_1.GPIO` = `board.btn`
* `axi_gpio_1.S_AXI` = `M_AXI_GP0@0x41210000`
* `axi_gpio_1.ip2intc_irpt` = `IRQ_F2P[2]`
* `axi_gpio_1.s_axi_aclk` = `FCLK_CLK0`
* `axi_quad_spi_0.AXI_LITE` = `M_AXI_GP0@0x41E00000`
* `axi_quad_spi_0.ext_spi_clk` = `FCLK_CLK0`
* `axi_quad_spi_0.ip2intc_irpt` = `IRQ_F2P[5]`
* `axi_quad_spi_0.s_axi_aclk` = `FCLK_CLK0`
* `axi_timer_0.S_AXI` = `M_AXI_GP0@0x42800000`
* `axi_timer_0.interrupt` = `IRQ_F2P[3]`
* `axi_timer_0.s_axi_aclk` = `FCLK_CLK0`
* `axi_uartlite_0.S_AXI` = `M_AXI_GP0@0x40600000`
* `axi_uartlite_0.interrupt` = `IRQ_F2P[4]`
* `axi_uartlite_0.s_axi_aclk` = `FCLK_CLK0`
* `zs_fir.S_AXI` = `M_AXI_GP0@0x43C00000`
* `zs_fir.S_AXI_ACLK` = `FCLK_CLK0`
* `zs_fir.irq` = `IRQ_F2P[0]`

## Not supported

Nothing is recorded as unmodelled.

## Not applied

* **PS7 configuration is read, not applied.** QEMU's Zynq machine has every PS peripheral; MIO routing, peripheral enables and the PS clock tree (PLLs, dividers) do not change what runs.
* **`ps7_init`**: `ps7_init.c`, `ps7_init.h`, `ps7_init.html`, `ps7_init.tcl`, `ps7_init_gpl.c`, `ps7_init_gpl.h` are in the archive but are not executed; the project starts the firmware directly (`boot.mode = "boot_bin"`; a BOOT.BIN's FSBL is not run either), so firmware that depends on state the FSBL would have set sees QEMU's defaults.
* IRQ_F2P[n] to GIC ID: bits 0-7 are 61-68 and 8-15 are 84-91 (Zynq-7000 TRM); QEMU's co-simulation device tree decides what the guest actually sees.

## Notes

* `axi_gpio_0.GPIO` (external `leds_4bits_tri_o`) is bound to board.leds by the port's name; change `[connect]` to rebind it
* `axi_gpio_0.GPIO2` (external `sws_4bits_tri_i`) is bound to board.sw by the port's name; change `[connect]` to rebind it
* `axi_gpio_1.GPIO` (external `btns_4bits_tri_i`) is bound to board.btn by the port's name; change `[connect]` to rebind it

## Archive contents

* `design_1.bda` (6995 bytes)
* `design_1.hwh` (369958 bytes)
* `hwdef.xml` (1270 bytes)
* `ps7_init.c` (542756 bytes)
* `ps7_init.h` (3795 bytes)
* `ps7_init.html` (2951775 bytes)
* `ps7_init.tcl` (35774 bytes)
* `ps7_init_gpl.c` (543373 bytes)
* `ps7_init_gpl.h` (4413 bytes)
* `xsa.json` (2682 bytes)
* `xsa.xml` (1244 bytes)

