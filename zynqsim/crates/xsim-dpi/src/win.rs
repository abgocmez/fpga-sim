//! Windows glue: `kernel32` only, Winsock bound at run time, DPI exports.
//!
//! The DPI exports are not written here: `abi` generates them, and this module
//! supplies the platform part (`rp_open`'s endpoint, the allocator, Winsock). The
//! protocol logic is in `core.rs` and is tested on the host.

#![allow(unsafe_code, non_snake_case, clippy::missing_safety_doc)]

use crate::core::{parse_endpoint, Dpi, Transport};
use ::core::alloc::{GlobalAlloc, Layout};
use ::core::cell::UnsafeCell;
use ::core::ffi::c_void;

// ------------------------------------------------------------------ kernel32

extern "system" {
    fn GetProcessHeap() -> *mut c_void;
    fn HeapAlloc(heap: *mut c_void, flags: u32, bytes: usize) -> *mut c_void;
    fn HeapFree(heap: *mut c_void, flags: u32, p: *mut c_void) -> i32;
    fn HeapReAlloc(heap: *mut c_void, flags: u32, p: *mut c_void, bytes: usize) -> *mut c_void;
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetEnvironmentVariableA(name: *const u8, buf: *mut u8, size: u32) -> u32;
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        n: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}

// ------------------------------------------------- runtime support (no_std)

struct Heap;

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // HeapAlloc returns 16-byte aligned blocks on x86-64, enough for
        // everything this crate allocates.
        HeapAlloc(GetProcessHeap(), 0, l.size()) as *mut u8
    }
    unsafe fn dealloc(&self, p: *mut u8, _l: Layout) {
        HeapFree(GetProcessHeap(), 0, p as *mut c_void);
    }
    unsafe fn realloc(&self, p: *mut u8, _l: Layout, new: usize) -> *mut u8 {
        HeapReAlloc(GetProcessHeap(), 0, p as *mut c_void, new) as *mut u8
    }
}

#[global_allocator]
static ALLOCATOR: Heap = Heap;

#[panic_handler]
fn panic(_info: &::core::panic::PanicInfo) -> ! {
    log(b"xsim-dpi: internal error, aborting\n");
    unsafe { ExitProcess(70) }
}

// The prebuilt `core`/`alloc` reference these for unwinding, which this crate
// never does (`panic = "abort"`). Defining them satisfies the linker.
#[no_mangle]
pub extern "C" fn _Unwind_Resume() -> ! {
    unsafe { ExitProcess(71) }
}
#[no_mangle]
pub extern "C" fn rust_eh_personality() {}

fn log(msg: &[u8]) {
    unsafe {
        let h = GetStdHandle(0xFFFF_FFF4); // STD_ERROR_HANDLE
        let mut n = 0u32;
        WriteFile(
            h,
            msg.as_ptr(),
            msg.len() as u32,
            &mut n,
            ::core::ptr::null_mut(),
        );
    }
}

// ------------------------------------------------------------------ Winsock

type Socket = usize;
const INVALID_SOCKET: Socket = !0;

#[repr(C)]
struct SockAddrIn {
    family: u16,
    port: u16,
    addr: u32,
    zero: [u8; 8],
}

#[repr(C)]
struct FdSet {
    count: u32,
    sockets: [Socket; 64],
}

#[repr(C)]
struct TimeVal {
    sec: i32,
    usec: i32,
}

struct Ws {
    socket: extern "system" fn(i32, i32, i32) -> Socket,
    connect: extern "system" fn(Socket, *const SockAddrIn, i32) -> i32,
    send: extern "system" fn(Socket, *const u8, i32, i32) -> i32,
    recv: extern "system" fn(Socket, *mut u8, i32, i32) -> i32,
    closesocket: extern "system" fn(Socket) -> i32,
    select: extern "system" fn(i32, *mut FdSet, *mut FdSet, *mut FdSet, *const TimeVal) -> i32,
    setsockopt: extern "system" fn(Socket, i32, i32, *const u8, i32) -> i32,
}

unsafe fn proc_addr<T>(m: *mut c_void, name: &[u8]) -> Option<T> {
    let p = GetProcAddress(m, name.as_ptr());
    if p.is_null() {
        None
    } else {
        Some(::core::mem::transmute_copy::<*mut c_void, T>(&p))
    }
}

unsafe fn load_ws() -> Option<Ws> {
    let m = LoadLibraryA(c"ws2_32.dll".as_ptr().cast::<u8>());
    if m.is_null() {
        return None;
    }
    let startup: extern "system" fn(u16, *mut u8) -> i32 = proc_addr(m, b"WSAStartup\0")?;
    let mut wsadata = [0u8; 512];
    if startup(0x0202, wsadata.as_mut_ptr()) != 0 {
        return None;
    }
    Some(Ws {
        socket: proc_addr(m, b"socket\0")?,
        connect: proc_addr(m, b"connect\0")?,
        send: proc_addr(m, b"send\0")?,
        recv: proc_addr(m, b"recv\0")?,
        closesocket: proc_addr(m, b"closesocket\0")?,
        select: proc_addr(m, b"select\0")?,
        setsockopt: proc_addr(m, b"setsockopt\0")?,
    })
}

struct WinSock {
    ws: &'static Ws,
    s: Socket,
}

impl Transport for WinSock {
    fn send(&mut self, data: &[u8]) -> bool {
        let mut off = 0;
        while off < data.len() {
            let n = (self.ws.send)(self.s, data[off..].as_ptr(), (data.len() - off) as i32, 0);
            if n <= 0 {
                return false;
            }
            off += n as usize;
        }
        true
    }

    fn recv_exact(&mut self, buf: &mut [u8]) -> bool {
        let mut off = 0;
        while off < buf.len() {
            let n = (self.ws.recv)(self.s, buf[off..].as_mut_ptr(), (buf.len() - off) as i32, 0);
            if n <= 0 {
                return false;
            }
            off += n as usize;
        }
        true
    }

    fn readable(&mut self, timeout_ms: i32) -> bool {
        let mut set = FdSet {
            count: 1,
            sockets: [0; 64],
        };
        set.sockets[0] = self.s;
        let r = if timeout_ms < 0 {
            (self.ws.select)(
                0,
                &mut set,
                ::core::ptr::null_mut(),
                ::core::ptr::null_mut(),
                ::core::ptr::null(),
            )
        } else {
            let tv = TimeVal {
                sec: timeout_ms / 1000,
                usec: (timeout_ms % 1000) * 1000,
            };
            (self.ws.select)(
                0,
                &mut set,
                ::core::ptr::null_mut(),
                ::core::ptr::null_mut(),
                &tv,
            )
        };
        r > 0
    }

    fn close(&mut self) {
        if self.s != INVALID_SOCKET {
            (self.ws.closesocket)(self.s);
            self.s = INVALID_SOCKET;
        }
    }
}

fn connect_tcp(descr: &[u8]) -> Option<WinSock> {
    static WS: Racy<Option<Ws>> = Racy(UnsafeCell::new(None));
    // SAFETY: single-threaded by the DPI contract.
    let ws: &'static Ws = unsafe {
        let slot = &mut *WS.0.get();
        if slot.is_none() {
            *slot = load_ws();
        }
        slot.as_ref()?
    };
    let (octets, port) = parse_endpoint(descr)?;
    // Network byte order in memory: the octets as written.
    let addr = u32::from_ne_bytes(octets);
    let s = (ws.socket)(2, 1, 6); // AF_INET, SOCK_STREAM, IPPROTO_TCP
    if s == INVALID_SOCKET {
        return None;
    }
    let sa = SockAddrIn {
        family: 2,
        port: port.to_be(),
        addr,
        zero: [0; 8],
    };
    if (ws.connect)(s, &sa, ::core::mem::size_of::<SockAddrIn>() as i32) != 0 {
        (ws.closesocket)(s);
        return None;
    }
    let one: u32 = 1;
    (ws.setsockopt)(s, 6, 1, &one as *const u32 as *const u8, 4); // TCP_NODELAY
    Some(WinSock { ws, s })
}

// ------------------------------------------------------------- global state

struct Racy<T>(UnsafeCell<T>);
// SAFETY: DPI calls are made from the simulator's single thread.
unsafe impl<T> ::core::marker::Sync for Racy<T> {}

static DPI: Racy<Option<Dpi<WinSock>>> = Racy(UnsafeCell::new(None));

fn dpi() -> &'static mut Dpi<WinSock> {
    // SAFETY: single-threaded by the DPI contract; never re-entered.
    unsafe {
        let slot = &mut *DPI.0.get();
        slot.get_or_insert_with(Dpi::new)
    }
}

/// `rp_open` on Windows: `given` is the endpoint text, or empty for `RP_SOCKET`.
fn open_endpoint(given: &[u8]) -> i32 {
    let mut env = [0u8; 256];
    let target: &[u8] = if given.is_empty() {
        // SAFETY: the buffer and the NUL-terminated name are valid for the call.
        let n = unsafe {
            GetEnvironmentVariableA(
                c"RP_SOCKET".as_ptr().cast::<u8>(),
                env.as_mut_ptr(),
                env.len() as u32,
            )
        };
        if n == 0 || n as usize >= env.len() {
            log(b"rp_open: RP_SOCKET is not set\n");
            return -1;
        }
        &env[..n as usize]
    } else {
        given
    };
    let Some(t) = connect_tcp(target) else {
        log(b"rp_open: cannot connect\n");
        return -1;
    };
    if dpi().open(t) {
        0
    } else {
        log(b"rp_open: hello failed\n");
        -1
    }
}

// The DPI-C exports (names and signatures shared with `dpi-cdylib`, see `abi`).
crate::export_dpi_abi!(dpi, open_endpoint);
