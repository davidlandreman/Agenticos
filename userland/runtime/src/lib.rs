//! `no_std` AgenticOS user runtime: Linux ABI stubs, startup parsing, and a
//! small `brk`-backed allocator for native Rust applications.

#![no_std]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use linked_list_allocator::LockedHeap;

pub const AT_FDCWD: i32 = -100;
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;
pub const F_OK: u32 = 0;
pub const X_OK: u32 = 1;
pub const WNOHANG: u32 = 1;

pub const GUI_ABI_VERSION: u32 = 1;
pub const GUI_PIXEL_FORMAT_XRGB8888: u32 = 1;
pub const GUI_NONBLOCK: u64 = 1;
pub const GUI_EVENT_KEY: u32 = 1;
pub const GUI_EVENT_MOUSE: u32 = 2;
pub const GUI_EVENT_RESIZE: u32 = 3;
pub const GUI_EVENT_CLOSE: u32 = 4;
pub const GUI_EVENT_FOCUS_CHANGE: u32 = 5;
pub const GUI_MOUSE_MOVE: u32 = 0;
pub const GUI_MOUSE_DOWN: u32 = 1;
pub const GUI_MOUSE_UP: u32 = 2;
pub const GUI_MOUSE_SCROLL: u32 = 3;
pub const GL_ABI_MAGIC: u32 = 0x314C_4741;
pub const GL_ABI_VERSION: u32 = 1;
pub const GL_DRAW_DEPTH_TEST: u32 = 1 << 0;
pub const GL_DRAW_CULL_BACK: u32 = 1 << 1;
pub const CLOCK_MONOTONIC: i32 = 1;

pub const KEY_ESCAPE: u32 = 37;
pub const KEY_ENTER: u32 = 38;
pub const KEY_SPACE: u32 = 39;
pub const KEY_TAB: u32 = 40;
pub const KEY_BACKSPACE: u32 = 41;
pub const KEY_DELETE: u32 = 42;
pub const KEY_LEFT: u32 = 43;
pub const KEY_RIGHT: u32 = 44;
pub const KEY_UP: u32 = 45;
pub const KEY_DOWN: u32 = 46;
pub const KEY_HOME: u32 = 47;
pub const KEY_END: u32 = 48;
pub const KEY_PAGE_UP: u32 = 49;
pub const KEY_PAGE_DOWN: u32 = 50;
pub const KEY_F2: u32 = 59;
pub const KEY_F5: u32 = 62;

const NR_READ: u64 = 0;
const NR_WRITE: u64 = 1;
const NR_CLOSE: u64 = 3;
const NR_FSTAT: u64 = 5;
const NR_LSEEK: u64 = 8;
const NR_BRK: u64 = 12;
const NR_ACCESS: u64 = 21;
const NR_NANOSLEEP: u64 = 35;
const NR_CLOCK_GETTIME: u64 = 228;
const NR_FORK: u64 = 57;
const NR_EXECVE: u64 = 59;
const NR_WAIT4: u64 = 61;
const NR_RMDIR: u64 = 84;
const NR_SYNC: u64 = 162;
const NR_EXIT_GROUP: u64 = 231;
const NR_GETDENTS64: u64 = 217;
const NR_OPENAT: u64 = 257;
const NR_NEWFSTATAT: u64 = 262;
const NR_FTRUNCATE: u64 = 77;
const NR_RENAME: u64 = 82;
const NR_MKDIR: u64 = 83;
const NR_UNLINK: u64 = 87;
const NR_GUI_LAUNCH: u64 = 5000;
const NR_GUI_WIN_CREATE: u64 = 5001;
const NR_GUI_WIN_PRESENT: u64 = 5002;
const NR_GUI_NEXT_EVENT: u64 = 5003;
const NR_GUI_WIN_DESTROY: u64 = 5004;
const NR_GUI_WIN_SET_TITLE: u64 = 5005;
const NR_GUI_GL_CONTEXT_CREATE: u64 = 5006;
const NR_GUI_GL_SUBMIT_FRAME: u64 = 5007;
const NR_GUI_GL_GET_INFO: u64 = 5008;
const NR_GUI_GL_CONTEXT_DESTROY: u64 = 5009;

#[inline]
unsafe fn syscall0(number: u64) -> i64 {
    let result: i64;
    core::arch::asm!(
        "syscall", inlateout("rax") number => result,
        out("rcx") _, out("r11") _, options(nostack)
    );
    result
}

#[inline]
unsafe fn syscall1(number: u64, a1: u64) -> i64 {
    let result: i64;
    core::arch::asm!(
        "syscall", inlateout("rax") number => result, in("rdi") a1,
        out("rcx") _, out("r11") _, options(nostack)
    );
    result
}

#[inline]
unsafe fn syscall2(number: u64, a1: u64, a2: u64) -> i64 {
    let result: i64;
    core::arch::asm!(
        "syscall", inlateout("rax") number => result, in("rdi") a1, in("rsi") a2,
        out("rcx") _, out("r11") _, options(nostack)
    );
    result
}

#[inline]
unsafe fn syscall3(number: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let result: i64;
    core::arch::asm!(
        "syscall", inlateout("rax") number => result,
        in("rdi") a1, in("rsi") a2, in("rdx") a3,
        out("rcx") _, out("r11") _, options(nostack)
    );
    result
}

#[inline]
unsafe fn syscall4(number: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let result: i64;
    core::arch::asm!(
        "syscall", inlateout("rax") number => result,
        in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4,
        out("rcx") _, out("r11") _, options(nostack)
    );
    result
}

#[inline]
unsafe fn syscall5(number: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let result: i64;
    core::arch::asm!(
        "syscall", inlateout("rax") number => result,
        in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4, in("r8") a5,
        out("rcx") _, out("r11") _, options(nostack)
    );
    result
}

pub fn read(fd: i32, buffer: &mut [u8]) -> i64 {
    unsafe {
        syscall3(
            NR_READ,
            fd as u64,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
        )
    }
}

pub fn write(fd: i32, buffer: &[u8]) -> i64 {
    unsafe {
        syscall3(
            NR_WRITE,
            fd as u64,
            buffer.as_ptr() as u64,
            buffer.len() as u64,
        )
    }
}

pub unsafe fn print(ptr: *const u8, len: usize) -> i64 {
    syscall3(NR_WRITE, 1, ptr as u64, len as u64)
}

pub fn openat(dirfd: i32, path: &[u8], flags: u32, mode: u32) -> i64 {
    unsafe {
        syscall4(
            NR_OPENAT,
            dirfd as u64,
            path.as_ptr() as u64,
            flags as u64,
            mode as u64,
        )
    }
}

pub fn close(fd: i32) -> i64 {
    unsafe { syscall1(NR_CLOSE, fd as u64) }
}

pub fn lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    unsafe { syscall3(NR_LSEEK, fd as u64, offset as u64, whence as u64) }
}

pub fn fstat(fd: i32, stat: &mut LinuxStat) -> i64 {
    unsafe { syscall2(NR_FSTAT, fd as u64, stat as *mut _ as u64) }
}

pub fn newfstatat(dirfd: i32, path: &[u8], stat: &mut LinuxStat, flags: u32) -> i64 {
    unsafe {
        syscall4(
            NR_NEWFSTATAT,
            dirfd as u64,
            path.as_ptr() as u64,
            stat as *mut _ as u64,
            flags as u64,
        )
    }
}

pub fn access(path: &[u8], mode: u32) -> i64 {
    unsafe { syscall2(NR_ACCESS, path.as_ptr() as u64, mode as u64) }
}

pub fn getdents64(fd: i32, buffer: &mut [u8]) -> i64 {
    unsafe {
        syscall3(
            NR_GETDENTS64,
            fd as u64,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
        )
    }
}

pub fn mkdir(path: &[u8], mode: u32) -> i64 {
    unsafe { syscall2(NR_MKDIR, path.as_ptr() as u64, mode as u64) }
}

pub fn unlink(path: &[u8]) -> i64 {
    unsafe { syscall1(NR_UNLINK, path.as_ptr() as u64) }
}

pub fn rmdir(path: &[u8]) -> i64 {
    unsafe { syscall1(NR_RMDIR, path.as_ptr() as u64) }
}

pub fn rename(old_path: &[u8], new_path: &[u8]) -> i64 {
    unsafe {
        syscall2(
            NR_RENAME,
            old_path.as_ptr() as u64,
            new_path.as_ptr() as u64,
        )
    }
}

pub fn ftruncate(fd: i32, length: i64) -> i64 {
    unsafe { syscall2(NR_FTRUNCATE, fd as u64, length as u64) }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

pub fn nanosleep(request: &Timespec, remaining: Option<&mut Timespec>) -> i64 {
    unsafe {
        syscall2(
            NR_NANOSLEEP,
            request as *const _ as u64,
            remaining.map_or(0, |value| value as *mut _ as u64),
        )
    }
}

pub fn clock_gettime(clock_id: i32, time: &mut Timespec) -> i64 {
    unsafe { syscall2(NR_CLOCK_GETTIME, clock_id as u64, time as *mut _ as u64) }
}

pub fn brk(address: usize) -> i64 {
    unsafe { syscall1(NR_BRK, address as u64) }
}

pub fn sync() -> i64 {
    unsafe { syscall0(NR_SYNC) }
}

pub fn fork() -> i64 {
    unsafe { syscall0(NR_FORK) }
}

pub fn execve(path: &[u8], argv: &[*const u8], envp: &[*const u8]) -> i64 {
    unsafe {
        syscall3(
            NR_EXECVE,
            path.as_ptr() as u64,
            argv.as_ptr() as u64,
            envp.as_ptr() as u64,
        )
    }
}

pub fn wait4(pid: i32, status: Option<&mut u32>, options: u32) -> i64 {
    unsafe {
        syscall4(
            NR_WAIT4,
            pid as u64,
            status.map_or(0, |value| value as *mut _ as u64),
            options as u64,
            0,
        )
    }
}

pub unsafe fn exit(code: i64) -> ! {
    core::arch::asm!("syscall", in("rax") NR_EXIT_GROUP, in("rdi") code, options(nostack, noreturn));
}

pub unsafe fn gui_launch(ptr: *const u8, len: usize) -> i64 {
    syscall2(NR_GUI_LAUNCH, ptr as u64, len as u64)
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuiEvent {
    pub kind: u32,
    pub window: u32,
    pub payload: [u32; 6],
}

const _: [(); 32] = [(); core::mem::size_of::<GuiEvent>()];

pub fn gui_win_create(width: u32, height: u32, title: &str, flags: u64) -> i64 {
    unsafe {
        syscall5(
            NR_GUI_WIN_CREATE,
            width as u64,
            height as u64,
            title.as_ptr() as u64,
            title.len() as u64,
            flags,
        )
    }
}

pub fn gui_win_present(handle: u32, pixels: &[u32], width: u32, height: u32) -> i64 {
    unsafe {
        syscall5(
            NR_GUI_WIN_PRESENT,
            handle as u64,
            pixels.as_ptr() as u64,
            width as u64,
            height as u64,
            width as u64 * 4,
        )
    }
}

pub fn gui_next_event(event: &mut GuiEvent, flags: u64) -> i64 {
    unsafe {
        syscall3(
            NR_GUI_NEXT_EVENT,
            event as *mut _ as u64,
            core::mem::size_of::<GuiEvent>() as u64,
            flags,
        )
    }
}

pub fn gui_win_destroy(handle: u32) -> i64 {
    unsafe { syscall1(NR_GUI_WIN_DESTROY, handle as u64) }
}

pub fn gui_win_set_title(handle: u32, title: &str) -> i64 {
    unsafe {
        syscall3(
            NR_GUI_WIN_SET_TITLE,
            handle as u64,
            title.as_ptr() as u64,
            title.len() as u64,
        )
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GlFrameHeader {
    pub magic: u32,
    pub version: u32,
    pub byte_len: u32,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub viewport_x: u32,
    pub viewport_y: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub clear_color: [f32; 4],
    pub clear_depth: f32,
    pub draw_count: u32,
    pub vertex_count: u32,
    pub draw_offset: u32,
    pub vertex_offset: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GlDraw {
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub flags: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GlVertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GlInfo {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub max_packet_bytes: u32,
    pub max_draws: u32,
    pub max_vertices: u32,
    pub supported_draw_flags: u32,
    pub last_error: i32,
    pub last_submitted_serial: u64,
    pub last_completed_serial: u64,
}

const _: [(); 88] = [(); core::mem::size_of::<GlFrameHeader>()];
const _: [(); 16] = [(); core::mem::size_of::<GlDraw>()];
const _: [(); 32] = [(); core::mem::size_of::<GlVertex>()];
const _: [(); 48] = [(); core::mem::size_of::<GlInfo>()];

pub fn gui_gl_context_create(window: u32, flags: u64) -> i64 {
    unsafe { syscall2(NR_GUI_GL_CONTEXT_CREATE, window as u64, flags) }
}

pub fn gui_gl_submit_frame(context: u32, packet: &[u8], flags: u64) -> i64 {
    unsafe {
        syscall4(
            NR_GUI_GL_SUBMIT_FRAME,
            context as u64,
            packet.as_ptr() as u64,
            packet.len() as u64,
            flags,
        )
    }
}

pub fn gui_gl_get_info(context: u32, info: &mut GlInfo) -> i64 {
    unsafe {
        syscall3(
            NR_GUI_GL_GET_INFO,
            context as u64,
            info as *mut _ as u64,
            core::mem::size_of::<GlInfo>() as u64,
        )
    }
}

pub fn gui_gl_context_destroy(context: u32) -> i64 {
    unsafe { syscall1(NR_GUI_GL_CONTEXT_DESTROY, context as u64) }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: u64,
    pub st_mtime: i64,
    pub st_mtime_nsec: u64,
    pub st_ctime: i64,
    pub st_ctime_nsec: u64,
    pub __unused: [i64; 3],
}

const _: [(); 144] = [(); core::mem::size_of::<LinuxStat>()];

pub struct Startup<'a> {
    pub argv: &'a [*const u8],
    pub envp: &'a [*const u8],
}

pub unsafe fn startup_from_stack(stack: *const u64) -> Startup<'static> {
    let argc = core::ptr::read(stack) as usize;
    let argv_ptr = stack.add(1) as *const *const u8;
    let argv = core::slice::from_raw_parts(argv_ptr, argc);
    let env_ptr = argv_ptr.add(argc + 1);
    let mut envc = 0usize;
    while envc < 4096 && !core::ptr::read(env_ptr.add(envc)).is_null() {
        envc += 1;
    }
    Startup {
        argv,
        envp: core::slice::from_raw_parts(env_ptr, envc),
    }
}

pub unsafe fn argv0_from_stack(stack: *const u64) -> (*const u8, usize) {
    let startup = startup_from_stack(stack);
    let Some(&pointer) = startup.argv.first() else {
        return (core::ptr::null(), 0);
    };
    if pointer.is_null() {
        return (core::ptr::null(), 0);
    }
    let mut len = 0usize;
    while len < 4096 && core::ptr::read(pointer.add(len)) != 0 {
        len += 1;
    }
    (pointer, len)
}

struct BrkAllocator {
    heap: LockedHeap,
    initialized: AtomicBool,
    end: AtomicUsize,
}

impl BrkAllocator {
    const CHUNK: usize = 64 * 1024;

    const fn new() -> Self {
        Self {
            heap: LockedHeap::empty(),
            initialized: AtomicBool::new(false),
            end: AtomicUsize::new(0),
        }
    }

    unsafe fn grow(&self, minimum: usize) -> bool {
        let current = brk(0);
        if current < 0 {
            return false;
        }
        let current = current as usize;
        let amount = align_up(minimum.max(Self::CHUNK), 4096);
        if !self.initialized.load(Ordering::Acquire) {
            let start = align_up(current, core::mem::align_of::<usize>());
            let new_end = match start.checked_add(amount) {
                Some(value) => value,
                None => return false,
            };
            if brk(new_end) != new_end as i64 {
                return false;
            }
            self.heap.lock().init(start as *mut u8, amount);
            self.end.store(new_end, Ordering::Release);
            self.initialized.store(true, Ordering::Release);
            return true;
        }
        let old_end = self.end.load(Ordering::Acquire).max(current);
        let Some(new_end) = old_end.checked_add(amount) else {
            return false;
        };
        if brk(new_end) != new_end as i64 {
            return false;
        }
        self.heap.lock().extend(amount);
        self.end.store(new_end, Ordering::Release);
        true
    }
}

unsafe impl GlobalAlloc for BrkAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.initialized.load(Ordering::Acquire) && !self.grow(layout.size() + layout.align()) {
            return core::ptr::null_mut();
        }
        let mut pointer = GlobalAlloc::alloc(&self.heap, layout);
        if pointer.is_null() && self.grow(layout.size() + layout.align()) {
            pointer = GlobalAlloc::alloc(&self.heap, layout);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        GlobalAlloc::dealloc(&self.heap, pointer, layout);
    }
}

const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[global_allocator]
static USER_ALLOCATOR: BrkAllocator = BrkAllocator::new();

#[alloc_error_handler]
fn allocation_error(_layout: Layout) -> ! {
    unsafe { exit(127) }
}

#[allow(dead_code)]
fn _keep_syscall0_used() {
    let _ = syscall0;
}
