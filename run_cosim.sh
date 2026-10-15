#!/usr/bin/env bash
# run_cosim.sh - QEMU (Cortex-A9 bare metal) <-> XSim (my_math_ip) co-simulation on Linux / WSL2.
# Same design as run_cosim_win.ps1: the PL side is a DPI-C remote-port client (sim/winport/rp_dpi.c).
# NOT YET RUN ON LINUX: only the Windows flow has been verified end to end.
#   ./run_cosim.sh            # batch, waveform database only
#   GUI=1 ./run_cosim.sh      # also open the XSim waveform viewer
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
XILINX_ROOT="${XILINX_ROOT:-/tools/Xilinx}"
VER="${VER:-2022.1}"
VIVADO_SETTINGS="${VIVADO_SETTINGS:-$XILINX_ROOT/Vivado/$VER/settings64.sh}"
VITIS_SETTINGS="${VITIS_SETTINGS:-$XILINX_ROOT/Vitis/$VER/settings64.sh}"
LIBSC="${LIBSC:-$HERE/third_party/libsystemctlm-soc}"
RUN="${RUN:-$HERE/build/cosim_run}"
ELF="${ELF:-$HERE/out/math_test.elf}"
QUANTUM_NS="${QUANTUM_NS:-1000000}"
GUI="${GUI:-0}"
TIMEOUT_S="${TIMEOUT_S:-300}"

die() { echo "ERROR: $*" >&2; exit 1; }

# shellcheck disable=SC1090
[ -f "$VIVADO_SETTINGS" ] && source "$VIVADO_SETTINGS" || die "no Vivado settings at $VIVADO_SETTINGS"
# shellcheck disable=SC1090
[ -f "$VITIS_SETTINGS" ]  && source "$VITIS_SETTINGS"  || die "no Vitis settings at $VITIS_SETTINGS"
for t in xvlog xsc xelab xsim; do command -v "$t" >/dev/null || die "$t not on PATH"; done

# ---- 1. Xilinx QEMU (Vitis ships one) and the co-sim device tree -------------
QEMU="${QEMU:-$(command -v qemu-system-aarch64 || true)}"
[ -x "$QEMU" ] || die "set QEMU=/path/to/Xilinx qemu-system-aarch64 (Vitis/PetaLinux build)"
"$QEMU" -M help | grep -q arm-generic-fdt-7series \
    || die "$QEMU lacks machine arm-generic-fdt-7series: not the Xilinx fork"
HW_DTB="${HW_DTB:-${XILINX_VITIS:-$XILINX_ROOT/Vitis/$VER}/data/emulation/dtbs/zynq/zynq-arm-cosim.dtb}"
[ -f "$HW_DTB" ] || die "co-sim DTB not found; set HW_DTB (looked for $HW_DTB)"
[ -f "$ELF" ]    || die "missing $ELF: run setup_bd.tcl then build_app.tcl first"

# ---- 2. libremote-port (protocol encode/decode, socket + safe IO helpers) ----
mkdir -p "$HERE/third_party" "$RUN"
[ -d "$LIBSC" ] || git clone --depth 1 https://github.com/Xilinx/libsystemctlm-soc "$LIBSC"
RP="$LIBSC/libremote-port"

# ---- 3. Build the XSim snapshot ----------------------------------------------
BUILD="$RUN/xsim"; rm -rf "$BUILD"; mkdir -p "$BUILD"; cd "$BUILD"
xvlog "$HERE/rtl/my_math_ip.v"
xvlog -sv "$HERE/sim/cosim_top.sv"
xsc --gcc_compile_options "-I$RP" \
    "$HERE/sim/winport/rp_dpi.c" "$RP/remote-port-proto.c" "$RP/remote-port-sk.c" "$RP/safeio.c"
xelab -debug typical -timescale 1ns/1ps -L work work.cosim_top \
      -sv_root xsim.dir/work/xsc -sv_lib dpi -s cosim_snap

# ---- 4. QEMU: PL remote-port over a unix chardev, then XSim -------------------
# The co-sim DTB has no chrdev-id, so the remote-port device is handed the chardev
# with -global. wait=on makes QEMU block until XSim connects.
SOCK="$RUN/pl-rp.sock"
rm -f "$SOCK" "$RUN/uart1.log"
cleanup() { kill "${QPID:-0}" "${XPID:-0}" 2>/dev/null || true; }
trap cleanup EXIT

"$QEMU" -M arm-generic-fdt-7series -display none \
    -hw-dtb "$HW_DTB" \
    -serial /dev/null -serial "file:$RUN/uart1.log" \
    -device "loader,file=$ELF,cpu-num=0" \
    -chardev "socket,id=pl-rp,path=$SOCK,server=on,wait=on" \
    -global remote-port.chardev=pl-rp \
    -sync-quantum "$QUANTUM_NS" \
    >"$RUN/qemu.log" 2>&1 &
QPID=$!

for _ in $(seq 1 50); do
    [ -S "$SOCK" ] && break
    kill -0 "$QPID" 2>/dev/null || { cat "$RUN/qemu.log"; die "QEMU exited early"; }
    sleep 0.2
done
[ -S "$SOCK" ] || { cat "$RUN/qemu.log"; die "QEMU did not create $SOCK"; }

export RP_SOCKET="unix:$SOCK"
if [ "$GUI" = "1" ]; then
    printf 'log_wave -recursive /cosim_top\nadd_wave -recursive /cosim_top\nrun all\n' >"$BUILD/wave.tcl"
    xsim cosim_snap -gui -tclbatch "$BUILD/wave.tcl" -wdb "$RUN/cosim.wdb" &
else
    printf 'log_wave -recursive /cosim_top\nrun all\nquit\n' >"$BUILD/wave.tcl"
    xsim cosim_snap -tclbatch "$BUILD/wave.tcl" -wdb "$RUN/cosim.wdb" &
fi
XPID=$!

# ---- 5. Wait for the firmware's end marker, then stop QEMU so XSim sees EOF --
for _ in $(seq 1 "$TIMEOUT_S"); do
    grep -q COSIM_DONE "$RUN/uart1.log" 2>/dev/null && break
    kill -0 "$QPID" 2>/dev/null || break
    sleep 1
done
sleep 1
kill "$QPID" 2>/dev/null || true
[ "$GUI" = "1" ] || wait "$XPID" 2>/dev/null || true

echo "---- UART1 ----"; cat "$RUN/uart1.log" 2>/dev/null || true
if grep -q "RESULT: ALL PASS" "$RUN/uart1.log" 2>/dev/null; then
    echo "CO-SIM PASS (waveforms: $RUN/cosim.wdb)"; rc=0
else
    echo "CO-SIM FAIL or timed out (see $RUN/qemu.log)"; rc=1
fi
[ "$GUI" = "1" ] && { echo "GUI open; close XSim to finish."; wait "$XPID" || true; }
exit $rc
