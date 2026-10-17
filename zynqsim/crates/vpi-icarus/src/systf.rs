//! The DPI-C functions of the generated testbenches, as Icarus system tasks and
//! functions. Each `$zs_<name>` has the argument and result types of the DPI import
//! `<name>` (`int` is 32 bits, `longint` 64 bits, `output int` is a variable the
//! call writes), and calls the same exported function of `xsim_dpi::abi` that the
//! archive and the shared object export. The logic is not repeated here.
//!
//! The wrappers in the testbench (`sim-api::tbgen`, under `` `ifdef ZS_ICARUS ``)
//! forward each DPI name to the `$zs_` system call, so one testbench text serves
//! both the DPI simulators and Icarus.

use std::ffi::{c_char, CStr};
use std::ptr;

use crate::abi;
use crate::vpi::{self, Handle, Value, ValueUnion, VecVal};

/// The arguments and result of the system call being executed.
pub struct Call {
    api: &'static vpi::Api,
    handle: Handle,
    args: Vec<Handle>,
}

impl Call {
    /// The call being executed: its handle and the handles of its arguments, in order.
    pub fn current() -> Call {
        let api = vpi::api();
        // SAFETY: called from a system task or function, where vvp answers these queries.
        unsafe {
            let handle = (api.handle)(vpi::SYS_TF_CALL, ptr::null_mut());
            let mut args = Vec::new();
            let it = (api.iterate)(vpi::ARGUMENT, handle);
            if !it.is_null() {
                loop {
                    let a = (api.scan)(it);
                    if a.is_null() {
                        break;
                    }
                    args.push(a);
                }
            }
            Call { api, handle, args }
        }
    }

    /// Reads the words of argument `i` (at most two, 64 bits); the number of words
    /// the argument has is returned with them. Missing arguments read as 0.
    fn words(&self, i: usize) -> ([VecVal; 2], usize) {
        let mut buf = [VecVal { aval: 0, bval: 0 }; 2];
        let Some(&arg) = self.args.get(i) else {
            return (buf, 0);
        };
        // SAFETY: `arg` is a live argument handle; the buffers hold the words.
        unsafe {
            // A `$time` argument reads as 0 in vvp (in every format), so the Icarus testbench
            // passes the time through a `longint` variable (see `sim-api::tbgen`).
            let size = (self.api.get)(vpi::SIZE, arg).max(1) as usize;
            let n = size.div_ceil(32).min(2);
            let mut v = Value {
                format: vpi::VECTOR_VAL,
                value: ValueUnion {
                    vector: buf.as_mut_ptr(),
                },
            };
            (self.api.get_value)(arg, &mut v);
            // vvp answers a vector with its own storage and puts that pointer in the value
            // (writing into the buffer we passed reads back as 0).
            let src = v.value.vector;
            if !src.is_null() {
                for (k, word) in buf.iter_mut().enumerate().take(n) {
                    *word = *src.add(k);
                }
            }
            (buf, n)
        }
    }

    /// Argument `i` as a 32-bit `int`.
    pub fn int(&self, i: usize) -> i32 {
        self.words(i).0[0].aval
    }

    /// Argument `i` as a 64-bit `longint`.
    pub fn long(&self, i: usize) -> i64 {
        let (b, n) = self.words(i);
        let lo = u64::from(b[0].aval as u32);
        let hi = if n > 1 { u64::from(b[1].aval as u32) } else { 0 };
        ((hi << 32) | lo) as i64
    }

    /// Argument `i` as a NUL-terminated byte string (its own copy).
    pub fn string(&self, i: usize) -> Vec<u8> {
        let Some(&arg) = self.args.get(i) else {
            return vec![0];
        };
        let mut v = Value {
            format: vpi::STRING_VAL,
            value: ValueUnion {
                misc: ptr::null_mut(),
            },
        };
        // SAFETY: `arg` is a live argument handle; vvp keeps the string until its next query,
        // and it is copied out here at once.
        unsafe {
            (self.api.get_value)(arg, &mut v);
            let p = v.value.str_;
            if p.is_null() {
                vec![0]
            } else {
                CStr::from_ptr(p).to_bytes_with_nul().to_vec()
            }
        }
    }

    /// Writes `value` into the variable passed as argument `i` (an `output int`).
    pub fn set_int(&self, i: usize, value: i32) {
        let Some(&arg) = self.args.get(i) else {
            return;
        };
        let mut v = Value {
            format: vpi::INT_VAL,
            value: ValueUnion { integer: value },
        };
        // SAFETY: `arg` is a live handle of a variable; no time is given (vpiNoDelay).
        unsafe {
            (self.api.put_value)(arg, &mut v, ptr::null_mut(), vpi::NO_DELAY);
        }
    }

    /// Sets the 32-bit result of this function call.
    pub fn ret_int(&self, value: i32) {
        self.put_result(&mut [VecVal {
            aval: value,
            bval: 0,
        }]);
    }

    /// Sets the 64-bit result of this function call.
    pub fn ret_long(&self, value: i64) {
        let v = value as u64;
        self.put_result(&mut [
            VecVal {
                aval: v as u32 as i32,
                bval: 0,
            },
            VecVal {
                aval: (v >> 32) as u32 as i32,
                bval: 0,
            },
        ]);
    }

    fn put_result(&self, words: &mut [VecVal]) {
        let mut v = Value {
            format: vpi::VECTOR_VAL,
            value: ValueUnion {
                vector: words.as_mut_ptr(),
            },
        };
        // SAFETY: the result handle of this call; the words live for the call.
        unsafe {
            (self.api.put_value)(self.handle, &mut v, ptr::null_mut(), vpi::NO_DELAY);
        }
    }
}

/// The result width of a function: `sizetf` for a 32-bit `int`.
extern "C" fn size32(_: *const c_char) -> i32 {
    32
}

/// The result width of a function: `sizetf` for a 64-bit `longint`.
extern "C" fn size64(_: *const c_char) -> i32 {
    64
}

/// What a DPI function returns, which decides how it is registered.
#[derive(Clone, Copy)]
pub enum Ret {
    /// `void`: a system task.
    Task,
    /// `int`: a system function of width 32.
    Int,
    /// `longint`: a system function of width 64.
    Long,
}

impl Ret {
    /// The `sizetf` callback for this result (none for a task).
    pub fn sizetf(self) -> Option<extern "C" fn(*const c_char) -> i32> {
        match self {
            Ret::Task => None,
            Ret::Int => Some(size32),
            Ret::Long => Some(size64),
        }
    }
}

/// One system task or function: its `$name`, result and call-time callback.
pub struct Def {
    /// The name with its `$`.
    pub name: &'static CStr,
    /// The DPI result type.
    pub ret: Ret,
    /// Runs one call.
    pub calltf: extern "C" fn(*const c_char) -> i32,
}

/// Defines a call-time callback: runs `$body` with the call's [`Call`], returns 0.
macro_rules! systf {
    ($name:ident, |$c:ident| $body:block) => {
        extern "C" fn $name(_data: *const c_char) -> i32 {
            let $c = Call::current();
            $body
            0
        }
    };
}

systf!(rp_open, |c| {
    let s = c.string(0);
    // SAFETY: `s` is NUL-terminated and lives for the call.
    let r = unsafe { abi::rp_open(s.as_ptr().cast()) };
    c.ret_int(r);
});

systf!(rp_next, |c| {
    let (mut addr, mut wdata, mut wstrb, mut is_write) = (0i32, 0i32, 0i32, 0i32);
    // SAFETY: the four pointers are valid locals.
    let r = unsafe {
        abi::rp_next(
            c.long(0),
            c.int(1),
            &mut addr,
            &mut wdata,
            &mut wstrb,
            &mut is_write,
        )
    };
    c.set_int(2, addr);
    c.set_int(3, wdata);
    c.set_int(4, wstrb);
    c.set_int(5, is_write);
    c.ret_int(r);
});

systf!(rp_done, |c| {
    abi::rp_done(c.int(0), c.int(1));
});

systf!(rp_done_t, |c| {
    abi::rp_done_t(c.int(0), c.int(1), c.long(2));
});

systf!(rp_poll, |c| {
    c.ret_int(abi::rp_poll(c.int(0)));
});

systf!(rp_lockstep, |c| {
    abi::rp_lockstep(c.int(0));
});

systf!(rp_sync_target, |c| {
    c.ret_long(abi::rp_sync_target());
});

systf!(rp_sync_ack, |c| {
    abi::rp_sync_ack(c.long(0));
});

systf!(rp_req_ts, |c| {
    c.ret_long(abi::rp_req_ts());
});

systf!(rp_mem_put, |c| {
    abi::rp_mem_put(c.int(0), c.int(1));
});

systf!(rp_mem_start, |c| {
    c.ret_int(abi::rp_mem_start(
        c.long(0),
        c.int(1),
        c.int(2),
        c.int(3),
        c.int(4),
    ));
});

systf!(rp_mem_poll, |c| {
    c.ret_int(abi::rp_mem_poll(c.int(0)));
});

systf!(rp_mem_wait, |c| {
    c.ret_int(abi::rp_mem_wait(c.int(0)));
});

systf!(rp_mem_get, |c| {
    c.ret_int(abi::rp_mem_get(c.int(0), c.int(1)));
});

systf!(rp_mem_free, |c| {
    abi::rp_mem_free(c.int(0));
});

systf!(rp_irq, |c| {
    abi::rp_irq(c.long(0), c.int(1), c.int(2));
});

systf!(rp_close, |_c| {
    abi::rp_close();
});

systf!(tel_publish, |c| {
    let s = c.string(0);
    abi::tel_publish(s.as_ptr().cast());
});

systf!(tel_event, |c| {
    let s = c.string(0);
    // SAFETY: NUL-terminated, alive for the call.
    unsafe { abi::tel_event(s.as_ptr().cast()) };
});

systf!(tel_ctrl, |c| {
    let (mut sw, mut btn) = (0i32, 0i32);
    // SAFETY: both pointers are valid locals.
    let r = unsafe { abi::tel_ctrl(&mut sw, &mut btn) };
    c.set_int(0, sw);
    c.set_int(1, btn);
    c.ret_int(r);
});

systf!(zs_video_tx_pixel, |c| {
    abi::zs_video_tx_pixel(c.int(0), c.int(1), c.int(2), c.long(3));
});

systf!(zs_video_rx_line, |c| {
    c.ret_int(abi::zs_video_rx_line(
        c.int(0),
        c.int(1),
        c.int(2),
        c.int(3),
        c.long(4),
    ));
});

systf!(zs_video_rx_pixel, |c| {
    c.ret_int(abi::zs_video_rx_pixel(c.int(0)));
});

systf!(zs_audio_tx_frame, |c| {
    abi::zs_audio_tx_frame(c.int(0), c.int(1), c.long(2));
});

systf!(zs_audio_flush, |_c| {
    abi::zs_audio_flush();
});

systf!(zs_audio_rx_frame, |c| {
    let (mut left, mut right) = (0i32, 0i32);
    // SAFETY: both pointers are valid locals.
    unsafe { abi::zs_audio_rx_frame(&mut left, &mut right, c.long(2)) };
    c.set_int(0, left);
    c.set_int(1, right);
});

systf!(zs_codec_reg, |c| {
    abi::zs_codec_reg(c.int(0), c.int(1), c.long(2));
});

systf!(zs_protocol_check, |c| {
    abi::zs_protocol_check(
        c.int(0),
        c.int(1),
        c.int(2),
        c.int(3),
        c.long(4),
        c.long(5),
    );
});

systf!(zs_pin_out, |c| {
    abi::zs_pin_out(c.int(0), c.int(1), c.long(2));
});

systf!(zs_pin_xchg, |c| {
    c.ret_int(abi::zs_pin_xchg(c.int(0), c.int(1), c.long(2)));
});

systf!(zs_pin_in, |c| {
    c.ret_int(abi::zs_pin_in(c.int(0)));
});

/// Every DPI function the testbenches import, with its `$zs_` name.
pub const TABLE: &[Def] = &[
    Def { name: c"$zs_rp_open", ret: Ret::Int, calltf: rp_open },
    Def { name: c"$zs_rp_next", ret: Ret::Int, calltf: rp_next },
    Def { name: c"$zs_rp_done", ret: Ret::Task, calltf: rp_done },
    Def { name: c"$zs_rp_done_t", ret: Ret::Task, calltf: rp_done_t },
    Def { name: c"$zs_rp_poll", ret: Ret::Int, calltf: rp_poll },
    Def { name: c"$zs_rp_lockstep", ret: Ret::Task, calltf: rp_lockstep },
    Def { name: c"$zs_rp_sync_target", ret: Ret::Long, calltf: rp_sync_target },
    Def { name: c"$zs_rp_sync_ack", ret: Ret::Task, calltf: rp_sync_ack },
    Def { name: c"$zs_rp_req_ts", ret: Ret::Long, calltf: rp_req_ts },
    Def { name: c"$zs_rp_mem_put", ret: Ret::Task, calltf: rp_mem_put },
    Def { name: c"$zs_rp_mem_start", ret: Ret::Int, calltf: rp_mem_start },
    Def { name: c"$zs_rp_mem_poll", ret: Ret::Int, calltf: rp_mem_poll },
    Def { name: c"$zs_rp_mem_wait", ret: Ret::Int, calltf: rp_mem_wait },
    Def { name: c"$zs_rp_mem_get", ret: Ret::Int, calltf: rp_mem_get },
    Def { name: c"$zs_rp_mem_free", ret: Ret::Task, calltf: rp_mem_free },
    Def { name: c"$zs_rp_irq", ret: Ret::Task, calltf: rp_irq },
    Def { name: c"$zs_rp_close", ret: Ret::Task, calltf: rp_close },
    Def { name: c"$zs_tel_publish", ret: Ret::Task, calltf: tel_publish },
    Def { name: c"$zs_tel_event", ret: Ret::Task, calltf: tel_event },
    Def { name: c"$zs_tel_ctrl", ret: Ret::Int, calltf: tel_ctrl },
    Def { name: c"$zs_video_tx_pixel", ret: Ret::Task, calltf: zs_video_tx_pixel },
    Def { name: c"$zs_video_rx_line", ret: Ret::Int, calltf: zs_video_rx_line },
    Def { name: c"$zs_video_rx_pixel", ret: Ret::Int, calltf: zs_video_rx_pixel },
    Def { name: c"$zs_audio_tx_frame", ret: Ret::Task, calltf: zs_audio_tx_frame },
    Def { name: c"$zs_audio_flush", ret: Ret::Task, calltf: zs_audio_flush },
    Def { name: c"$zs_audio_rx_frame", ret: Ret::Task, calltf: zs_audio_rx_frame },
    Def { name: c"$zs_codec_reg", ret: Ret::Task, calltf: zs_codec_reg },
    Def { name: c"$zs_protocol_check", ret: Ret::Task, calltf: zs_protocol_check },
    Def { name: c"$zs_pin_out", ret: Ret::Task, calltf: zs_pin_out },
    Def { name: c"$zs_pin_xchg", ret: Ret::Int, calltf: zs_pin_xchg },
    Def { name: c"$zs_pin_in", ret: Ret::Int, calltf: zs_pin_in },
];

#[cfg(test)]
mod tests {
    use super::TABLE;
    use std::collections::BTreeSet;

    /// Every system task or function is registered once, with its `$zs_` name.
    #[test]
    fn the_names_are_unique_and_prefixed() {
        let names: BTreeSet<String> = TABLE
            .iter()
            .map(|d| d.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), TABLE.len());
        assert_eq!(TABLE.len(), 31, "the DPI export table has 31 functions");
        assert!(names.iter().all(|n| n.starts_with("$zs_")));
    }

    /// Every `$zs_` call of the math testbench (the fixture the Icarus example runs) is registered.
    #[test]
    fn the_math_testbench_calls_are_all_registered() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/rtl/math/cosim_top.sv");
        let text = std::fs::read_to_string(path).unwrap();
        let mut calls = 0;
        for part in text.split("$zs_").skip(1) {
            let name: String = part
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            calls += 1;
            let full = format!("$zs_{name}");
            assert!(
                TABLE.iter().any(|d| d.name.to_bytes() == full.as_bytes()),
                "{full} is not registered"
            );
        }
        assert!(calls >= 4, "the testbench calls the DPI functions");
    }
}
