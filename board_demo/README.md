# board_demo: virtual Zybo Z7-20 (no hardware needed)

Develop PS software and PL RTL together and *see* what they do: LEDs, switches,
buttons, an interactive UART shell and DDR contents as images, all in a browser.

| Piece | Role |
|---|---|
| `rtl/gpio_ctrl.v` | AXI4-Lite GPIO: 4 LEDs, 4 switches, 4 buttons (edge events + IRQ), tick counter |
| `../dma_demo/rtl/dma_xform.v` | image engine: `out = in*scale + offset`, optional clamp to 0..255, DMA through HP0, IRQ |
| `sim/cosim_top.sv` | board model: address decode (`0x43C00000` DMA, `0x43C10000` GPIO), HP0 -> QEMU DDR, IRQ_F2P[1:0], switches/buttons from the UI |
| `sw/main.c` | UART shell, GIC setup, button ISR, DMA ISR, test images |
| `../ui/` | browser UI: CPU/PL panels, virtual board, console, memory/image viewer |

## Run

```powershell
xsct board_demo\build_app.tcl            # -> out\board_test.elf (needs out\system_wrapper.xsa once)
powershell -ExecutionPolicy Bypass -File run_cosim_win.ps1 `
  -Elf out\board_test.elf `
  -Rtl dma_demo\rtl\dma_xform.v,board_demo\rtl\gpio_ctrl.v `
  -TopSv board_demo\sim\cosim_top.sv -RunName board_ui `
  -IcountShift 1 -QuantumNs 1000 -TimeoutS 3600 -Ui -SerialPort 5556
```

Open http://127.0.0.1:8765/ (the script opens it). Try in the console: `help`, `led 5`,
`img 1`, `bright 60`, `contrast 3`, `run 2 -100 1`. Click switches/buttons on the board;
use **Memory / image viewer** -> *source image* / *result image* to see the pixels the PL wrote.

## Verified (automated driver)

* UART shell round-trip, `led` -> LED pins in PL telemetry
* switches visible to firmware (`status`)
* `bright 60` and `contrast 3` on 64x64 images: **0 mismatches of 4096 pixels** vs a Python
  golden model, 256 read + 256 write bursts per run
* button press -> `IRQ_F2P[1]` -> GIC 62 -> ISR -> LED toggled

## Notes

* The simulator advances slowly in wall-clock terms (about 25 s per simulated ms), so
  the shell is responsive but not instant.
* Time is quantised at 1 us; CPU timing is an instruction-count model (see `../dma_demo/README.md`).
* `-TimeoutS` must cover how long you want to play; `end` in the console stops the run.
