//! The DPI-C export table, written once and expanded by each package that
//! exposes the library: the freestanding static archive (`win`, XSim on
//! Windows) and the shared-object package `dpi-cdylib` (Questa, VCS, Xcelium,
//! XSim on Linux, Verilator). The names and C signatures are the ones the
//! generated testbenches import (`sim-api::tbgen`, `fixtures/rtl`).
//!
//! [`export_dpi_abi!`] takes two things a package must supply: the accessor of
//! its single [`Dpi`](crate::core::Dpi) instance, and the `rp_open` body, the
//! only export that differs by platform (the endpoint text may be empty, which
//! means `RP_SOCKET`). Everything else is shared, so the signatures cannot drift
//! between packages.
//!
//! This is the FFI boundary: every `unsafe` here is a raw pointer read or write
//! from a C caller, or a C string walk. The protocol logic is in `core`.

use ::core::ffi::c_char;

/// The bytes of a NUL-terminated C string (empty for NULL).
///
/// # Safety
///
/// `p` must be NULL or point to a NUL-terminated string that stays valid for `'a`.
#[doc(hidden)]
pub unsafe fn cstr_bytes<'a>(p: *const c_char) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    ::core::slice::from_raw_parts(p as *const u8, n)
}

/// Expands the DPI-C exports of the library.
///
/// `$dpi` is a function returning `&'static mut Dpi<T>` for the package's one
/// instance (the DPI contract is single-threaded). `$open` takes the endpoint
/// text from `rp_open` (empty means the `RP_SOCKET` environment variable) and
/// returns the `rp_open` result: 0 on success, -1 on failure.
///
/// The generated functions are `#[no_mangle]` and `extern "C"`. The package must
/// be edition 2021 (edition 2024 would require `#[unsafe(no_mangle)]`).
#[macro_export]
macro_rules! export_dpi_abi {
    ($dpi:path, $open:path) => {
        /// `rp_open(descr)`: connects to the remote-port peer and sends our hello.
        #[no_mangle]
        pub unsafe extern "C" fn rp_open(descr: *const ::core::ffi::c_char) -> i32 {
            $open($crate::abi::cstr_bytes(descr))
        }

        /// `rp_next`: the next PS-to-PL beat, or 0 closed, 2 idle, 3 sync pending.
        #[no_mangle]
        pub unsafe extern "C" fn rp_next(
            now_ns: i64,
            idle_ms: i32,
            addr: *mut i32,
            wdata: *mut i32,
            wstrb: *mut i32,
            is_write: *mut i32,
        ) -> i32 {
            match $dpi().next(now_ns.max(0) as u64, idle_ms) {
                $crate::core::Next::Closed => 0,
                $crate::core::Next::Idle => 2,
                $crate::core::Next::Sync => 3,
                $crate::core::Next::Beat($crate::core::Beat {
                    addr: a,
                    wdata: w,
                    wstrb: s,
                    is_write: wr,
                }) => {
                    *addr = a as i32;
                    *wdata = w as i32;
                    *wstrb = s as i32;
                    *is_write = i32::from(wr);
                    1
                }
            }
        }

        /// `rp_done(rdata, ok)`: completes the current beat.
        #[no_mangle]
        pub extern "C" fn rp_done(rdata: i32, ok: i32) {
            $dpi().done(rdata as u32, ok != 0);
        }

        /// `rp_done_t(rdata, ok, now_ns)`: completes the beat, reporting PL latency.
        #[no_mangle]
        pub extern "C" fn rp_done_t(rdata: i32, ok: i32, now_ns: i64) {
            $dpi().done_t(rdata as u32, ok != 0, now_ns.max(0) as u64);
        }

        /// `rp_poll(timeout_ms)`: pumps at most one packet (1, 0 none, -1 closed).
        #[no_mangle]
        pub extern "C" fn rp_poll(timeout_ms: i32) -> i32 {
            $dpi().poll(timeout_ms)
        }

        /// `rp_lockstep(on)`: syncs are answered by `rp_sync_ack` when set.
        #[no_mangle]
        pub extern "C" fn rp_lockstep(on: i32) {
            $dpi().set_lockstep(on != 0);
        }

        /// `rp_sync_target()`: the PL time the peer asks for, or -1.
        #[no_mangle]
        pub extern "C" fn rp_sync_target() -> i64 {
            $dpi().sync_target()
        }

        /// `rp_sync_ack(now_ns)`: answers the pending sync.
        #[no_mangle]
        pub extern "C" fn rp_sync_ack(now_ns: i64) {
            $dpi().sync_ack(now_ns.max(0) as u64);
        }

        /// `rp_req_ts()`: the PS timestamp of the access at the head of the queue, or -1.
        #[no_mangle]
        pub extern "C" fn rp_req_ts() -> i64 {
            $dpi().req_ts()
        }

        /// `rp_mem_put(idx, word)`: stages word `idx` of a PL-to-PS write.
        #[no_mangle]
        pub extern "C" fn rp_mem_put(idx: i32, word: i32) {
            $dpi().mem_put(idx, word as u32);
        }

        /// `rp_mem_start(now_ns, dev, is_write, addr, len)`: a slot, or -1.
        #[no_mangle]
        pub extern "C" fn rp_mem_start(
            now_ns: i64,
            dev: i32,
            is_write: i32,
            addr: i32,
            len: i32,
        ) -> i32 {
            $dpi().mem_start(
                now_ns.max(0) as u64,
                dev as u32,
                is_write != 0,
                addr as u32,
                len,
            )
        }

        /// `rp_mem_poll(slot)`: 0 pending, 1 done, 2 error, -1 bad slot.
        #[no_mangle]
        pub extern "C" fn rp_mem_poll(slot: i32) -> i32 {
            $dpi().mem_poll(slot)
        }

        /// `rp_mem_wait(slot)`: blocks until the response; 1 ok, 2 error, -1 bad slot.
        #[no_mangle]
        pub extern "C" fn rp_mem_wait(slot: i32) -> i32 {
            $dpi().mem_wait(slot)
        }

        /// `rp_mem_get(slot, idx)`: word `idx` of a finished read.
        #[no_mangle]
        pub extern "C" fn rp_mem_get(slot: i32, idx: i32) -> i32 {
            $dpi().mem_get(slot, idx) as i32
        }

        /// `rp_mem_free(slot)`: releases a slot.
        #[no_mangle]
        pub extern "C" fn rp_mem_free(slot: i32) {
            $dpi().mem_free(slot);
        }

        /// `rp_irq(now_ns, line, val)`: drives `IRQ_F2P[line]`.
        #[no_mangle]
        pub extern "C" fn rp_irq(now_ns: i64, line: i32, val: i32) {
            $dpi().irq(now_ns.max(0) as u64, line as u32, val != 0);
        }

        /// `rp_close()`: closes the connection.
        #[no_mangle]
        pub extern "C" fn rp_close() {
            $dpi().close();
        }

        /// `tel_publish(json)`: discarded; the GUI reads the trace through the control plane.
        #[no_mangle]
        pub extern "C" fn tel_publish(_json: *const ::core::ffi::c_char) {}

        /// `tel_event(json)`: forwards LED changes to the host, drops the rest.
        #[no_mangle]
        pub unsafe extern "C" fn tel_event(json: *const ::core::ffi::c_char) {
            $dpi().telemetry_event($crate::abi::cstr_bytes(json), 0);
        }

        /// `tel_ctrl(&sw, &btn)`: hands the testbench the board's switches and buttons.
        #[no_mangle]
        pub unsafe extern "C" fn tel_ctrl(sw: *mut i32, btn: *mut i32) -> i32 {
            let (s, b) = $dpi().board_inputs();
            *sw = i32::from(s);
            *btn = i32::from(b);
            0
        }

        /// `zs_video_tx_pixel(de, vs, rgb, now_ns)`: one HDMI TX pixel-clock sample.
        #[no_mangle]
        pub extern "C" fn zs_video_tx_pixel(de: i32, vs: i32, rgb: i32, now_ns: i64) {
            $dpi().video_tx_pixel(de != 0, vs != 0, rgb as u32, now_ns.max(0) as u64);
        }

        /// `zs_video_rx_line(frame, y, w, h, now_ns)`: fetches an HDMI RX line; 0 on success.
        #[no_mangle]
        pub extern "C" fn zs_video_rx_line(frame: i32, y: i32, w: i32, h: i32, now_ns: i64) -> i32 {
            $dpi().video_rx_line(
                frame as u32,
                y as u32,
                w.max(0) as u32,
                h.max(0) as u32,
                now_ns.max(0) as u64,
            )
        }

        /// `zs_video_rx_pixel(x)`: pixel `x` of the line fetched by `zs_video_rx_line`.
        #[no_mangle]
        pub extern "C" fn zs_video_rx_pixel(x: i32) -> i32 {
            $dpi().video_rx_pixel(x.max(0) as u32) as i32
        }

        /// `zs_audio_tx_frame(left, right, now_ns)`: one stereo frame from the codec DAC.
        #[no_mangle]
        pub extern "C" fn zs_audio_tx_frame(left: i32, right: i32, now_ns: i64) {
            $dpi().audio_tx_frame(left as i16, right as i16, now_ns.max(0) as u64);
        }

        /// `zs_audio_flush()`: sends any buffered DAC frames.
        #[no_mangle]
        pub extern "C" fn zs_audio_flush() {
            $dpi().audio_flush();
        }

        /// `zs_audio_rx_frame(&left, &right, now_ns)`: the next stereo frame for the codec ADC.
        #[no_mangle]
        pub unsafe extern "C" fn zs_audio_rx_frame(left: *mut i32, right: *mut i32, now_ns: i64) {
            let (l, r) = $dpi().audio_rx_frame(now_ns.max(0) as u64);
            *left = i32::from(l);
            *right = i32::from(r);
        }

        /// `zs_codec_reg(reg, value, now_ns)`: a codec register write seen on the I2C pins.
        #[no_mangle]
        pub extern "C" fn zs_codec_reg(reg: i32, value: i32, now_ns: i64) {
            $dpi().codec_register(reg as u32, value as u32, now_ns.max(0) as u64);
        }

        /// `zs_protocol_check(port, channel, rule, info, detail, now_ns)`: an AXI
        /// protocol-checker finding (ADR-025).
        #[no_mangle]
        pub extern "C" fn zs_protocol_check(
            port: i32,
            channel: i32,
            rule: i32,
            info: i32,
            detail: i64,
            now_ns: i64,
        ) {
            $dpi().protocol_check(
                now_ns.max(0) as u64,
                port as u32,
                channel as u32,
                rule as u32,
                info != 0,
                detail as u64,
            );
        }

        /// `zs_pin_out(group, levels, now_ns)`: pin group `group` changed.
        #[no_mangle]
        pub extern "C" fn zs_pin_out(group: i32, levels: i32, now_ns: i64) {
            $dpi().pin_out(group as u32, levels as u32, now_ns.max(0) as u64);
        }

        /// `zs_pin_xchg(group, levels, now_ns)`: reports the levels and returns the
        /// levels the host's devices drive into the PL (blocks until the host answers).
        #[no_mangle]
        pub extern "C" fn zs_pin_xchg(group: i32, levels: i32, now_ns: i64) -> i32 {
            i32::from($dpi().pin_xchg(group as u32, levels as u32, now_ns.max(0) as u64))
        }

        /// `zs_pin_in(group)`: the host-driven levels of pin group `group`.
        #[no_mangle]
        pub extern "C" fn zs_pin_in(group: i32) -> i32 {
            i32::from($dpi().pin_in(group as u32))
        }
    };
}
