#![no_std]
#![allow(
    clippy::missing_safety_doc,
    reason = "private C ABI functions share the pointer contracts declared by agenticos.c"
)]

use core::cmp::{max, min};
use core::ffi::{c_int, c_void};
use core::ptr;

const NR_READ: u64 = 0;
const NR_GUI_WIN_CREATE: u64 = 5001;
const NR_GUI_WIN_PRESENT: u64 = 5002;
const NR_GUI_WIN_DESTROY: u64 = 5004;
const NR_GUI_WIN_SET_TITLE: u64 = 5005;
const NR_GUI_EVENT_OPEN: u64 = 5011;

const EVENT_KEY: u32 = 1;
const EVENT_MOUSE: u32 = 2;
const EVENT_RESIZE: u32 = 3;
const EVENT_CLOSE: u32 = 4;
const EVENT_THEME_CHANGED: u32 = 6;
const EVENT_SETTINGS_CHANGED: u32 = 7;

const MOUSE_MOVE: u32 = 0;
const MOUSE_DOWN: u32 = 1;
const MOUSE_UP: u32 = 2;
const MOUSE_SCROLL: u32 = 3;

const KBD_ENTER: i32 = -0x100;
const KBD_BS: i32 = -0x101;
const KBD_TAB: i32 = -0x102;
const KBD_ESC: i32 = -0x103;
const KBD_LEFT: i32 = -0x104;
const KBD_RIGHT: i32 = -0x105;
const KBD_UP: i32 = -0x106;
const KBD_DOWN: i32 = -0x107;
const KBD_INS: i32 = -0x108;
const KBD_DEL: i32 = -0x109;
const KBD_HOME: i32 = -0x10a;
const KBD_END: i32 = -0x10b;
const KBD_PAGE_UP: i32 = -0x10c;
const KBD_PAGE_DOWN: i32 = -0x10d;
const KBD_F1: i32 = -0x120;
const KBD_CTRL_C: i32 = -0x200;
const KBD_CLOSE: i32 = -0x201;

const B_LEFT: i32 = 0;
const B_MIDDLE: i32 = 1;
const B_RIGHT: i32 = 2;
const B_WHEELUP: i32 = 8;
const B_WHEELDOWN: i32 = 9;
const B_WHEELLEFT: i32 = 12;
const B_WHEELRIGHT: i32 = 13;
const B_DOWN: i32 = 0;
const B_UP: i32 = 16;
const B_DRAG: i32 = 32;
const B_MOVE: i32 = 48;

extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(pointer: *mut c_void);
}

#[repr(C)]
pub struct Surface {
    pub handle: u32,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels: *mut u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GuiEvent {
    pub kind: u32,
    pub window: u32,
    pub payload: [u32; 6],
}

#[repr(C)]
pub struct MappedEvent {
    /// 0 ignored, 1 key, 2 mouse, 3 resize, 4 close, 5 redraw.
    pub kind: u32,
    pub key: i32,
    pub flags: i32,
    pub x: i32,
    pub y: i32,
    pub buttons: i32,
    pub width: u32,
    pub height: u32,
}

#[inline]
unsafe fn syscall1(number: u64, a1: u64) -> i64 {
    let result: i64;
    core::arch::asm!("syscall", inlateout("rax") number as i64 => result, in("rdi") a1,
        lateout("rcx") _, lateout("r11") _, options(nostack));
    result
}

#[inline]
unsafe fn syscall3(number: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let result: i64;
    core::arch::asm!("syscall", inlateout("rax") number as i64 => result, in("rdi") a1,
        in("rsi") a2, in("rdx") a3, lateout("rcx") _, lateout("r11") _, options(nostack));
    result
}

#[inline]
unsafe fn syscall5(number: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let result: i64;
    core::arch::asm!("syscall", inlateout("rax") number as i64 => result, in("rdi") a1,
        in("rsi") a2, in("rdx") a3, in("r10") a4, in("r8") a5,
        lateout("rcx") _, lateout("r11") _, options(nostack));
    result
}

unsafe fn allocate_pixels(width: u32, height: u32) -> *mut u32 {
    let Some(count) = (width as usize).checked_mul(height as usize) else {
        return ptr::null_mut();
    };
    let Some(bytes) = count.checked_mul(4) else {
        return ptr::null_mut();
    };
    let pixels = malloc(bytes).cast::<u32>();
    if !pixels.is_null() {
        ptr::write_bytes(pixels, 0xff, count);
    }
    pixels
}

#[no_mangle]
pub unsafe extern "C" fn ag_surface_create(
    width: u32,
    height: u32,
    title: *const u8,
    title_len: usize,
) -> *mut Surface {
    let handle = syscall5(
        NR_GUI_WIN_CREATE,
        width as u64,
        height as u64,
        title as u64,
        title_len as u64,
        0,
    );
    if handle < 0 {
        return ptr::null_mut();
    }
    let pixels = allocate_pixels(width, height);
    if pixels.is_null() {
        let _ = syscall1(NR_GUI_WIN_DESTROY, handle as u64);
        return ptr::null_mut();
    }
    let raw = malloc(core::mem::size_of::<Surface>()).cast::<Surface>();
    if raw.is_null() {
        free(pixels.cast());
        let _ = syscall1(NR_GUI_WIN_DESTROY, handle as u64);
        return ptr::null_mut();
    }
    raw.write(Surface {
        handle: handle as u32,
        width,
        height,
        stride: width as usize * 4,
        pixels,
    });
    raw
}

#[no_mangle]
pub unsafe extern "C" fn ag_surface_destroy(surface: *mut Surface) {
    if surface.is_null() {
        return;
    }
    let _ = syscall1(NR_GUI_WIN_DESTROY, (*surface).handle as u64);
    free((*surface).pixels.cast());
    free(surface.cast());
}

#[no_mangle]
pub unsafe extern "C" fn ag_surface_resize(
    surface: *mut Surface,
    width: u32,
    height: u32,
) -> c_int {
    if surface.is_null() || width == 0 || height == 0 {
        return -1;
    }
    let pixels = allocate_pixels(width, height);
    if pixels.is_null() {
        return -1;
    }
    free((*surface).pixels.cast());
    (*surface).pixels = pixels;
    (*surface).width = width;
    (*surface).height = height;
    (*surface).stride = width as usize * 4;
    0
}

#[no_mangle]
pub unsafe extern "C" fn ag_surface_present(surface: *mut Surface) -> i64 {
    if surface.is_null() {
        return -1;
    }
    syscall5(
        NR_GUI_WIN_PRESENT,
        (*surface).handle as u64,
        (*surface).pixels as u64,
        (*surface).width as u64,
        (*surface).height as u64,
        (*surface).stride as u64,
    )
}

#[no_mangle]
pub unsafe extern "C" fn ag_surface_set_title(
    surface: *mut Surface,
    title: *const u8,
    len: usize,
) -> i64 {
    if surface.is_null() {
        return -1;
    }
    syscall3(
        NR_GUI_WIN_SET_TITLE,
        (*surface).handle as u64,
        title as u64,
        len as u64,
    )
}

#[no_mangle]
pub unsafe extern "C" fn ag_event_open(flags: u64) -> i64 {
    syscall1(NR_GUI_EVENT_OPEN, flags)
}

#[no_mangle]
pub unsafe extern "C" fn ag_event_read(fd: c_int, events: *mut GuiEvent, count: usize) -> i64 {
    syscall3(
        NR_READ,
        fd as u64,
        events as u64,
        (count * core::mem::size_of::<GuiEvent>()) as u64,
    )
}

#[no_mangle]
pub unsafe extern "C" fn ag_bitmap_alloc(width: i32, height: i32) -> *mut u32 {
    if width <= 0 || height <= 0 {
        return ptr::null_mut();
    }
    allocate_pixels(width as u32, height as u32)
}

#[no_mangle]
pub unsafe extern "C" fn ag_bitmap_free(pixels: *mut u32) {
    if !pixels.is_null() {
        free(pixels.cast())
    }
}

fn clipped_rect(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    clip: [i32; 4],
    size: [u32; 2],
) -> Option<[usize; 4]> {
    let left = max(max(x1, clip[0]), 0);
    let top = max(max(y1, clip[1]), 0);
    let right = min(min(x2, clip[2]), size[0] as i32);
    let bottom = min(min(y2, clip[3]), size[1] as i32);
    (left < right && top < bottom).then_some([
        left as usize,
        top as usize,
        right as usize,
        bottom as usize,
    ])
}

#[no_mangle]
pub unsafe extern "C" fn ag_fill(
    surface: *mut Surface,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: u32,
    cx1: i32,
    cy1: i32,
    cx2: i32,
    cy2: i32,
) {
    if surface.is_null() {
        return;
    }
    let Some(r) = clipped_rect(
        x1,
        y1,
        x2,
        y2,
        [cx1, cy1, cx2, cy2],
        [(*surface).width, (*surface).height],
    ) else {
        return;
    };
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            *(*surface).pixels.add(y * (*surface).width as usize + x) = color;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ag_draw_bitmap(
    surface: *mut Surface,
    bitmap: *const u32,
    bw: i32,
    bh: i32,
    x: i32,
    y: i32,
    cx1: i32,
    cy1: i32,
    cx2: i32,
    cy2: i32,
) {
    if surface.is_null() || bitmap.is_null() || bw <= 0 || bh <= 0 {
        return;
    }
    let Some(r) = clipped_rect(
        x,
        y,
        x + bw,
        y + bh,
        [cx1, cy1, cx2, cy2],
        [(*surface).width, (*surface).height],
    ) else {
        return;
    };
    for dy in r[1]..r[3] {
        let sy = (dy as i32 - y) as usize;
        for dx in r[0]..r[2] {
            let sx = (dx as i32 - x) as usize;
            *(*surface).pixels.add(dy * (*surface).width as usize + dx) =
                *bitmap.add(sy * bw as usize + sx);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ag_scroll(
    surface: *mut Surface,
    dx: i32,
    dy: i32,
    cx1: i32,
    cy1: i32,
    cx2: i32,
    cy2: i32,
) {
    if surface.is_null() || (dx == 0 && dy == 0) {
        return;
    }
    let Some(r) = clipped_rect(
        cx1,
        cy1,
        cx2,
        cy2,
        [0, 0, cx2, cy2],
        [(*surface).width, (*surface).height],
    ) else {
        return;
    };
    let width = (*surface).width as usize;
    if dy > 0 {
        for y in (r[1]..r[3]).rev() {
            copy_scroll_row((*surface).pixels, width, r, y, dx, dy);
        }
    } else {
        for y in r[1]..r[3] {
            copy_scroll_row((*surface).pixels, width, r, y, dx, dy);
        }
    }
}

unsafe fn copy_scroll_row(
    pixels: *mut u32,
    width: usize,
    r: [usize; 4],
    y: usize,
    dx: i32,
    dy: i32,
) {
    let sy = y as i32 - dy;
    if sy < r[1] as i32 || sy >= r[3] as i32 {
        return;
    }
    if dx > 0 {
        for x in (r[0]..r[2]).rev() {
            copy_scroll_pixel(pixels, width, r, x, y, sy, dx);
        }
    } else {
        for x in r[0]..r[2] {
            copy_scroll_pixel(pixels, width, r, x, y, sy, dx);
        }
    }
}

unsafe fn copy_scroll_pixel(
    pixels: *mut u32,
    width: usize,
    r: [usize; 4],
    x: usize,
    y: usize,
    sy: i32,
    dx: i32,
) {
    let sx = x as i32 - dx;
    if sx >= r[0] as i32 && sx < r[2] as i32 {
        *pixels.add(y * width + x) = *pixels.add(sy as usize * width + sx as usize);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ag_map_event(
    event: *const GuiEvent,
    mapped: *mut MappedEvent,
    previous_buttons: *mut u32,
) {
    if event.is_null() || mapped.is_null() || previous_buttons.is_null() {
        return;
    }
    ptr::write_bytes(mapped, 0, 1);
    let event = &*event;
    match event.kind {
        EVENT_KEY if event.payload[3] != 0 => {
            let modifiers = event.payload[2] & 7;
            let key = map_key(event.payload[0], event.payload[1], modifiers);
            if key != 0 {
                (*mapped).kind = 1;
                (*mapped).key = key;
                (*mapped).flags = if key == KBD_CTRL_C {
                    0
                } else {
                    modifiers as i32
                };
            }
        }
        EVENT_MOUSE => map_mouse(event, &mut *mapped, &mut *previous_buttons),
        EVENT_RESIZE => {
            (*mapped).kind = 3;
            (*mapped).width = event.payload[0];
            (*mapped).height = event.payload[1];
        }
        EVENT_CLOSE => {
            (*mapped).kind = 4;
            (*mapped).key = KBD_CLOSE;
        }
        EVENT_THEME_CHANGED | EVENT_SETTINGS_CHANGED => (*mapped).kind = 5,
        _ => {}
    }
}

fn map_key(code: u32, character: u32, modifiers: u32) -> i32 {
    if (code == 3 && modifiers & 2 != 0) || (code == 61 && modifiers & 4 != 0) {
        return KBD_CTRL_C;
    }
    match code {
        37 => KBD_ESC,
        38 => KBD_ENTER,
        40 => KBD_TAB,
        41 => KBD_BS,
        42 => KBD_DEL,
        43 => KBD_LEFT,
        44 => KBD_RIGHT,
        45 => KBD_UP,
        46 => KBD_DOWN,
        47 => KBD_HOME,
        48 => KBD_END,
        49 => KBD_PAGE_UP,
        50 => KBD_PAGE_DOWN,
        51 => KBD_INS,
        58..=69 => KBD_F1 - (code as i32 - 58),
        _ if character != 0 => character as i32,
        _ => 0,
    }
}

fn first_button(mask: u32) -> i32 {
    if mask & 1 != 0 {
        B_LEFT
    } else if mask & 4 != 0 {
        B_MIDDLE
    } else {
        B_RIGHT
    }
}

fn map_mouse(event: &GuiEvent, out: &mut MappedEvent, previous: &mut u32) {
    let current = event.payload[2] & 7;
    let action = event.payload[3];
    out.kind = 2;
    out.x = event.payload[0] as i32;
    out.y = event.payload[1] as i32;
    out.buttons = match action {
        MOUSE_DOWN => first_button(current & !*previous) | B_DOWN,
        MOUSE_UP => first_button(*previous & !current) | B_UP,
        MOUSE_MOVE if current != 0 => first_button(current) | B_DRAG,
        MOUSE_MOVE => B_MOVE,
        MOUSE_SCROLL => {
            let dx = event.payload[4] as i32;
            let dy = event.payload[5] as i32;
            if dy < 0 {
                B_WHEELUP | B_MOVE
            } else if dy > 0 {
                B_WHEELDOWN | B_MOVE
            } else if dx < 0 {
                B_WHEELLEFT | B_MOVE
            } else {
                B_WHEELRIGHT | B_MOVE
            }
        }
        _ => B_MOVE,
    };
    *previous = current;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop()
    }
}
