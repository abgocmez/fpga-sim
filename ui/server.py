#!/usr/bin/env python3
"""
server.py - web UI backend for the Zynq co-simulation (stdlib only).

Collects, once per poll:
  * PL telemetry         <run>/telemetry.json  (written by cosim_top.sv via tel_dpi.c)
  * PL event log         <run>/events.jsonl
  * both Cortex-A9 cores QEMU HMP monitor: `info cpus`, `cpu N` + `info registers`
  * shared mailbox       QEMU HMP `xp` of the mailbox in DDR (CPU1 progress, jobs)
  * UART console         <run>/uart1.log
and serves them as JSON at /api/state next to a static page (index.html).
Last good values are cached, so the page keeps showing the final state after the run.
"""
import argparse
import json
import os
import re
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
MAILBOX_ADDR = 0x01F00000
MBOX_FIELDS = ["magic", "jobs_total", "job_len", "cpu1_state", "cpu1_alive", "cpu1_job",
               "cpu1_words", "slot0_state", "slot1_state", "slot0_job", "slot1_job",
               "cpu0_job", "cpu0_jobs_done", "cpu0_errors"]
MODES = {0x10: "USR", 0x11: "FIQ", 0x12: "IRQ", 0x13: "SVC", 0x16: "MON", 0x17: "ABT",
         0x1A: "HYP", 0x1B: "UND", 0x1F: "SYS"}


# ----------------------------------------------------------------- QEMU monitor
class Monitor:
    """Minimal HMP client over TCP. One command at a time, reconnects on failure."""

    def __init__(self, port):
        self.port = port
        self.sock = None
        self.lock = threading.Lock()

    def _connect(self):
        s = socket.create_connection(("127.0.0.1", self.port), timeout=1.0)
        s.settimeout(1.0)
        self.sock = s
        self._read_until_prompt()

    def _read_until_prompt(self):
        buf = b""
        end = time.time() + 2.0
        while time.time() < end:
            try:
                chunk = self.sock.recv(65536)
            except socket.timeout:
                break
            if not chunk:
                raise ConnectionError("monitor closed")
            buf += chunk
            if buf.rstrip().endswith(b"(qemu)"):
                break
        return buf.decode("latin1")

    def cmd(self, text):
        with self.lock:
            if self.sock is None:
                self._connect()
            try:
                self.sock.sendall(text.encode() + b"\n")
                out = self._read_until_prompt()
            except Exception:
                try:
                    self.sock.close()
                except Exception:
                    pass
                self.sock = None
                raise
        # the monitor's line editor echoes the command with cursor escapes: strip them,
        # then drop the first line (the echo) and the trailing prompt
        out = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", out)
        parts = out.replace("\r", "").split("\n")[1:]
        return "\n".join(l for l in parts if l.strip() and "(qemu)" not in l)


def parse_regs(text):
    """`info registers` on ARM prints R00=.. R01=.. ... R15=.. PSR=.. lines."""
    regs = {}
    for name, val in re.findall(r"\b(R\d{2}|PSR)=([0-9a-fA-F]{8})\b", text):
        key = "CPSR" if name == "PSR" else "R%d" % int(name[1:])
        regs[key] = int(val, 16)
    return regs


def parse_xp(text, count):
    words = []
    for line in text.split("\n"):
        m = re.match(r"\s*[0-9a-fA-F]+:\s*(.*)", line)
        if m:
            words += [int(w, 16) for w in re.findall(r"0x([0-9a-fA-F]+)", m.group(1))]
    return words[:count]


# ------------------------------------------------------------------ ELF symbols
class Symbols:
    def __init__(self, elf):
        self.table = []
        if not elf or not os.path.exists(elf):
            return
        nm = self._find_nm()
        if not nm:
            return
        try:
            out = subprocess.run([nm, "-n", "--defined-only", elf], capture_output=True,
                                 text=True, timeout=20).stdout
        except Exception:
            return
        for line in out.splitlines():
            parts = line.split()
            if len(parts) == 3 and parts[1] in "tTwW":
                self.table.append((int(parts[0], 16), parts[2]))

    @staticmethod
    def _find_nm():
        root = os.environ.get("XILINX_VITIS", r"C:\Xilinx\Vitis\2022.1")
        for base, _, files in os.walk(os.path.join(root, "gnu", "aarch32")):
            if "arm-none-eabi-nm.exe" in files or "arm-none-eabi-nm" in files:
                return os.path.join(base, "arm-none-eabi-nm.exe" if "arm-none-eabi-nm.exe" in files
                                    else "arm-none-eabi-nm")
        return None

    def lookup(self, pc):
        best = None
        for addr, name in self.table:
            if addr <= pc:
                best = (addr, name)
            else:
                break
        if best and pc - best[0] < 0x4000:
            return f"{best[1]}+0x{pc - best[0]:x}"
        return ""


# ----------------------------------------------------------------- UART link
class SerialLink:
    """TCP client of QEMU's UART1 chardev: console output and keyboard input."""

    def __init__(self, port, logpath):
        self.port = port
        self.logpath = logpath
        self.sock = None
        self.buf = ""
        self.lock = threading.Lock()
        threading.Thread(target=self._run, daemon=True).start()

    def _run(self):
        while self.sock is None:
            try:
                s = socket.create_connection(("127.0.0.1", self.port), timeout=1.0)
            except OSError:
                time.sleep(0.2)
                continue
            s.settimeout(0.5)
            self.sock = s
        with open(self.logpath, "wb") as log:
            while True:
                try:
                    d = self.sock.recv(4096)
                except socket.timeout:
                    continue
                except OSError:
                    break
                if not d:
                    break
                log.write(d)
                log.flush()
                with self.lock:
                    self.buf = (self.buf + d.decode("latin1"))[-8000:]
        self.sock = None

    def send(self, text):
        s = self.sock
        if s is None:
            return False
        try:
            s.sendall(text.encode("latin1", "replace"))
            return True
        except OSError:
            return False


# ------------------------------------------------------------------------ state
class State:
    def __init__(self, args):
        self.run = args.run
        self.mon = Monitor(args.monitor) if args.monitor else None
        self.serial = SerialLink(args.serial, os.path.join(args.run, "uart1.log")) if args.serial else None
        self.ctrl = (0, 0)
        self.sym = [Symbols(args.elf0), Symbols(args.elf1)]
        self.cpus = [dict(id=0, role="Control, DMA driver, UART"),
                     dict(id=1, role="Data producer (ping-pong buffers)")]
        self.mbox = None
        self.pl = {}
        self.qemu_ok = False
        self.lock = threading.Lock()
        self.events = []
        self.ev_pos = 0
        self.uart = ""
        self.t_start = time.time()
        threading.Thread(target=self._loop, daemon=True).start()

    def set_ctrl(self, sw, btn):
        """Virtual board inputs: read by the testbench from control.txt."""
        self.ctrl = (sw & 15, btn & 15)
        tmp = os.path.join(self.run, "control.txt.tmp")
        with open(tmp, "w") as f:
            f.write("%d %d\n" % self.ctrl)
        os.replace(tmp, os.path.join(self.run, "control.txt"))

    def read_mem(self, addr, length):
        """Physical memory of the guest through the QEMU monitor (pmemsave)."""
        if not self.mon:
            raise RuntimeError("no QEMU monitor")
        # unique file per request: QEMU may still hold the previous one open on Windows
        self._memseq = getattr(self, "_memseq", 0) + 1
        path = os.path.join(self.run, "mem_%d.bin" % self._memseq).replace("\\", "/")
        self.mon.cmd('pmemsave 0x%x %d "%s"' % (addr, length, path))
        end = time.time() + 5
        last = -1
        while time.time() < end:
            try:
                size = os.path.getsize(path)
            except OSError:
                size = -1
            if size >= length and size == last:      # complete and no longer growing
                break
            last = size
            time.sleep(0.05)
        with open(path, "rb") as f:
            data = f.read(length)
        for old in range(max(1, self._memseq - 3)):  # keep the run directory tidy
            try:
                os.remove(os.path.join(self.run, "mem_%d.bin" % (old + 1)))
            except OSError:
                pass
        return data

    def _read_file(self, name):
        try:
            with open(os.path.join(self.run, name), "rb") as f:
                return f.read().decode("utf8", "replace")
        except OSError:
            return None

    def _poll_files(self):
        t = self._read_file("telemetry.json")
        if t:
            try:
                self.pl = json.loads(t)
            except ValueError:
                pass
        try:
            with open(os.path.join(self.run, "events.jsonl"), "rb") as f:
                f.seek(self.ev_pos)
                data = f.read()
            nl = data.rfind(b"\n")
            if nl >= 0:
                self.ev_pos += nl + 1
                for line in data[:nl].decode("utf8", "replace").split("\n"):
                    try:
                        self.events.append(json.loads(line))
                    except ValueError:
                        pass
        except OSError:
            pass
        if self.serial is not None:
            with self.serial.lock:
                self.uart = self.serial.buf[-6000:]
        else:
            u = self._read_file("uart1.log")
            if u is not None:
                self.uart = u[-6000:]

    def _poll_qemu(self):
        if not self.mon:
            return
        try:
            for i in (0, 1):
                self.mon.cmd(f"cpu {i}")
                regs = parse_regs(self.mon.cmd("info registers"))
                if "R15" not in regs:
                    continue
                pc = regs["R15"]
                cpsr = regs.get("CPSR", 0)
                # QEMU leaves PC just after a WFI/WFE while the core is parked, so the word
                # before PC tells whether the core is waiting (ARM state, identity-mapped).
                prev = parse_xp(self.mon.cmd(f"xp /1xw 0x{(pc - 4) & 0xFFFFFFFF:08x}"), 1)
                wait = {0xE320F003: "WFI", 0xE320F002: "WFE"}.get(prev[0] if prev else 0)
                c = self.cpus[i]
                c.update(
                    pc=pc, sp=regs.get("R13", 0), lr=regs.get("R14", 0),
                    cpsr=cpsr, mode=MODES.get(cpsr & 0x1F, hex(cpsr & 0x1F)),
                    irq_masked=bool(cpsr & 0x80), waiting=wait or "",
                    halted=bool(wait), where=self.sym[i].lookup(pc),
                    regs={k: v for k, v in regs.items() if re.match(r"R\d+$", k)},
                )
            words = parse_xp(self.mon.cmd(f"xp /{len(MBOX_FIELDS)}xw 0x{MAILBOX_ADDR:08x}"),
                             len(MBOX_FIELDS))
            if len(words) == len(MBOX_FIELDS):
                self.mbox = dict(zip(MBOX_FIELDS, words))
            self.qemu_ok = True
        except Exception:
            self.qemu_ok = False

    def _loop(self):
        while True:
            try:
                with self.lock:
                    self._poll_files()
                    self._poll_qemu()
            except Exception as e:  # never kill the thread
                print("poll error:", e, file=sys.stderr)
            time.sleep(0.15)

    def snapshot(self, since):
        with self.lock:
            total = len(self.events)
            ev = self.events[since:since + 4000]
            return dict(
                total=since + len(ev),
                wall=round(time.time() - self.t_start, 1),
                qemu_ok=self.qemu_ok,
                ctrl=dict(sw=self.ctrl[0], btn=self.ctrl[1]),
                serial=bool(self.serial and self.serial.sock),
                cpus=self.cpus,
                mailbox=self.mbox,
                pl=self.pl,
                events=ev,
                uart=self.uart,
            )


def make_handler(state):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def _send(self, code, body, ctype):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            path, _, query = self.path.partition("?")
            if path == "/api/state":
                since = 0
                m = re.search(r"since=(\d+)", query)
                if m:
                    since = int(m.group(1))
                self._send(200, json.dumps(state.snapshot(since)).encode(), "application/json")
            elif path == "/api/mem":
                q = dict(re.findall(r"(\w+)=([^&]+)", query))
                try:
                    addr = int(q.get("addr", "0"), 0)
                    length = min(int(q.get("len", "256")), 1 << 20)
                    data = state.read_mem(addr, length)
                    self._send(200, data, "application/octet-stream")
                except Exception as e:
                    self._send(500, str(e).encode(), "text/plain")
            elif path in ("/", "/index.html"):
                with open(os.path.join(HERE, "index.html"), "rb") as f:
                    self._send(200, f.read(), "text/html; charset=utf-8")
            else:
                self._send(404, b"not found", "text/plain")

        def do_POST(self):
            n = int(self.headers.get("Content-Length", "0"))
            try:
                body = json.loads(self.rfile.read(n) or b"{}")
            except ValueError:
                body = {}
            if self.path == "/api/ctrl":
                state.set_ctrl(int(body.get("sw", 0)), int(body.get("btn", 0)))
                self._send(200, b"{}", "application/json")
            elif self.path == "/api/uart":
                ok = state.serial.send(str(body.get("text", ""))) if state.serial else False
                self._send(200, json.dumps({"ok": ok}).encode(), "application/json")
            else:
                self._send(404, b"not found", "text/plain")

    return H


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True, help="co-sim run directory (telemetry, uart log)")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--monitor", type=int, default=0, help="QEMU HMP monitor TCP port")
    ap.add_argument("--serial", type=int, default=0, help="QEMU UART1 TCP port (interactive console)")
    ap.add_argument("--elf0", default="")
    ap.add_argument("--elf1", default="")
    args = ap.parse_args()
    st = State(args)
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(st))
    print(f"UI on http://127.0.0.1:{args.port}/", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
