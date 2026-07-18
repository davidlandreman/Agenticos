use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_int;
use core::{char, mem, ptr, slice, str};

use gui::{
    theme, Button, Canvas, CheckBox, MenuEntry, MenuEntryFlags, MenuPopup, MenuPopupAction,
    ProgressBar, RadioButton, FONT_LINE_HEIGHT,
};
use gui_core::{KeyInput, Modifiers, MouseButtons, PointerInput, PointerKind, Rect};

use crate::{
    GuiEvent, Surface, EVENT_KEY, EVENT_MOUSE, EVENT_RESIZE, EVENT_SETTINGS_CHANGED,
    EVENT_THEME_CHANGED, MOUSE_DOWN, MOUSE_MOVE, MOUSE_SCROLL, MOUSE_UP,
};

const ABI_VERSION: u32 = 1;
const MAX_MENU_NODES: usize = 256;
const MAX_TEXT_BYTES: usize = 1024 * 1024;

const ACTION_NONE: u32 = 0;
const ACTION_ACTIVATE: u32 = 1;
const ACTION_CANCEL: u32 = 2;
const ACTION_CHROME: u32 = 3;

const CHROME_MENU_BASE: i64 = 100;
const CHROME_BACK: i64 = 1;
const CHROME_FORWARD: i64 = 2;
const CHROME_RELOAD: i64 = 3;
const CHROME_GO: i64 = 4;
const MENU_HEIGHT: i32 = 26;
const TOOLBAR_HEIGHT: i32 = 38;
const STATUS_HEIGHT: i32 = 24;
const CHROME_TOP: i32 = MENU_HEIGHT + TOOLBAR_HEIGHT;
const MENU_LABELS: [&str; 6] = ["File", "View", "Link", "Downloads", "Setup", "Help"];

const NODE_LABEL: u32 = 1;
const NODE_CHECKBOX: u32 = 2;
const NODE_RADIO: u32 = 3;
const NODE_FIELD: u32 = 4;
const NODE_PASSWORD: u32 = 5;
const NODE_BUTTON: u32 = 6;
const NODE_DIALOG: u32 = 7;
const NODE_PROGRESS: u32 = 8;
const NODE_COMBO: u32 = 9;
const NODE_TEXTAREA: u32 = 10;
const NODE_SCROLLBAR: u32 = 11;
const NODE_TREE_ROW: u32 = 12;
const NODE_FOCUSED: u32 = 1 << 5;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UiText {
    pointer: *const u8,
    len: u32,
}

#[repr(C)]
pub struct UiNode {
    version: u32,
    byte_len: u32,
    id: u64,
    kind: u32,
    flags: u32,
    role: u32,
    group: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    value: i64,
    value_min: i64,
    value_max: i64,
    label: UiText,
    secondary: UiText,
    value_text: UiText,
}

#[repr(C)]
pub struct UiResult {
    version: u32,
    byte_len: u32,
    consumed: u32,
    repaint: u32,
    action_kind: u32,
    flags: u32,
    target: u64,
    value: i64,
}

impl UiResult {
    fn reset(&mut self) {
        *self = Self {
            version: ABI_VERSION,
            byte_len: mem::size_of::<Self>() as u32,
            consumed: 0,
            repaint: 0,
            action_kind: ACTION_NONE,
            flags: 0,
            target: 0,
            value: 0,
        };
    }
}

struct HostedMenu {
    id: u64,
    popup: MenuPopup,
    backing: Vec<u32>,
}

struct DialogNode {
    kind: u32,
    flags: u32,
    bounds: Rect,
    value: i64,
    value_min: i64,
    value_max: i64,
    group: u32,
    label: String,
}

struct HostedDialog {
    id: u64,
    bounds: Rect,
    nodes: Vec<DialogNode>,
    backing: Vec<u32>,
}

struct NativeUiHost {
    menu: Option<HostedMenu>,
    dialog: Option<HostedDialog>,
    chrome_visible: bool,
    status: String,
    location: String,
    chrome_pressed: Option<i64>,
}

struct HostCell(UnsafeCell<NativeUiHost>);

unsafe impl Sync for HostCell {}

static HOST: HostCell = HostCell(UnsafeCell::new(NativeUiHost {
    menu: None,
    dialog: None,
    chrome_visible: false,
    status: String::new(),
    location: String::new(),
    chrome_pressed: None,
}));

fn host() -> &'static mut NativeUiHost {
    // Links and its selector invoke the native host only on the process's
    // single UI thread. No host reference may escape an ABI call.
    unsafe { &mut *HOST.0.get() }
}

unsafe fn decode_text(text: UiText) -> Option<String> {
    if text.len == 0 {
        return Some(String::new());
    }
    if text.pointer.is_null() || text.len as usize > MAX_TEXT_BYTES {
        return None;
    }
    let bytes = slice::from_raw_parts(text.pointer, text.len as usize);
    Some(String::from(str::from_utf8(bytes).ok()?))
}

unsafe fn canvas(surface: *mut Surface) -> Option<Canvas> {
    let surface = surface.as_ref()?;
    Canvas::from_borrowed(surface.width, surface.height, surface.pixels)
}

unsafe fn copy_rect(surface: *mut Surface, bounds: Rect) -> Option<Vec<u32>> {
    let surface = surface.as_ref()?;
    if bounds.x < 0
        || bounds.y < 0
        || bounds.right() > surface.width as i32
        || bounds.bottom() > surface.height as i32
    {
        return None;
    }
    let mut backing = Vec::with_capacity(bounds.w as usize * bounds.h as usize);
    for y in bounds.y as usize..bounds.bottom() as usize {
        let start = y
            .checked_mul(surface.width as usize)?
            .checked_add(bounds.x as usize)?;
        backing.extend_from_slice(slice::from_raw_parts(
            surface.pixels.add(start),
            bounds.w as usize,
        ));
    }
    Some(backing)
}

unsafe fn restore_rect(surface: *mut Surface, bounds: Rect, backing: &[u32]) {
    let Some(surface) = surface.as_mut() else {
        return;
    };
    if bounds.x < 0
        || bounds.y < 0
        || bounds.right() > surface.width as i32
        || bounds.bottom() > surface.height as i32
        || backing.len() != bounds.w as usize * bounds.h as usize
    {
        return;
    }
    for row in 0..bounds.h as usize {
        let destination = surface
            .pixels
            .add((bounds.y as usize + row) * surface.width as usize + bounds.x as usize);
        ptr::copy_nonoverlapping(
            backing.as_ptr().add(row * bounds.w as usize),
            destination,
            bounds.w as usize,
        );
    }
}

unsafe fn copy_backing(surface: *mut Surface, menu: &MenuPopup) -> Option<Vec<u32>> {
    let surface = surface.as_ref()?;
    let bounds = menu.bounds;
    let mut backing = Vec::with_capacity(bounds.w as usize * bounds.h as usize);
    for y in bounds.y as usize..bounds.bottom() as usize {
        let start = y
            .checked_mul(surface.width as usize)?
            .checked_add(bounds.x as usize)?;
        let row = slice::from_raw_parts(surface.pixels.add(start), bounds.w as usize);
        backing.extend_from_slice(row);
    }
    Some(backing)
}

unsafe fn restore_backing(surface: *mut Surface, menu: &HostedMenu) {
    let Some(surface) = surface.as_mut() else {
        return;
    };
    let bounds = menu.popup.bounds;
    if bounds.right() > surface.width as i32 || bounds.bottom() > surface.height as i32 {
        return;
    }
    for row in 0..bounds.h as usize {
        let destination = surface
            .pixels
            .add((bounds.y as usize + row) * surface.width as usize + bounds.x as usize);
        ptr::copy_nonoverlapping(
            menu.backing.as_ptr().add(row * bounds.w as usize),
            destination,
            bounds.w as usize,
        );
    }
}

unsafe fn render(surface: *mut Surface, hosted: &HostedMenu) {
    let Some(mut canvas) = canvas(surface) else {
        return;
    };
    hosted.popup.draw(&mut canvas);
}

unsafe fn render_dialog(surface: *mut Surface, dialog: &HostedDialog) {
    let Some(mut canvas) = canvas(surface) else {
        return;
    };
    let palette = theme::palette();
    theme::draw_menu_surface(
        &mut canvas,
        dialog.bounds.x,
        dialog.bounds.y,
        dialog.bounds.w,
        dialog.bounds.h,
    );
    for node in &dialog.nodes {
        let focused = node.flags & NODE_FOCUSED != 0;
        match node.kind {
            NODE_DIALOG => {
                canvas.draw_text(
                    node.bounds.x + 10,
                    node.bounds.y + 5,
                    &node.label,
                    palette.text,
                );
                canvas.horizontal_line(
                    node.bounds.x + 4,
                    node.bounds.y + 25,
                    node.bounds.w.saturating_sub(8),
                    palette.border,
                );
            }
            NODE_LABEL => {
                canvas.draw_text(node.bounds.x, node.bounds.y + 2, &node.label, palette.text)
            }
            NODE_CHECKBOX => {
                let checkbox = CheckBox::new("", node.bounds, node.value != 0);
                checkbox.draw(&mut canvas, focused);
            }
            NODE_RADIO => {
                let radio = RadioButton::new("", node.bounds, node.value != 0);
                radio.draw(&mut canvas, focused);
            }
            NODE_FIELD | NODE_PASSWORD => {
                theme::draw_field(
                    &mut canvas,
                    node.bounds.x,
                    node.bounds.y,
                    node.bounds.w,
                    node.bounds.h,
                    focused,
                );
                let shown = if node.kind == NODE_PASSWORD {
                    "*".repeat(node.label.chars().count())
                } else {
                    node.label.clone()
                };
                let old_clip = canvas.clip();
                canvas.set_clip(Some(node.bounds.inset(3)));
                canvas.draw_text(
                    node.bounds.x + 5,
                    node.bounds.y + (node.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
                    &shown,
                    palette.field_text,
                );
                canvas.set_clip(old_clip);
            }
            NODE_BUTTON => Button::new(
                &node.label,
                node.bounds.x,
                node.bounds.y,
                node.bounds.w,
                node.bounds.h,
            )
            .draw(&mut canvas, focused),
            NODE_PROGRESS => {
                let mut progress = ProgressBar::new(node.bounds, 1000);
                progress.set(node.value.max(0) as u64, 1000);
                progress.draw(&mut canvas);
            }
            NODE_TREE_ROW => {
                let selected = node.flags & NODE_FOCUSED != 0;
                canvas.fill_rect(
                    node.bounds.x,
                    node.bounds.y,
                    node.bounds.w,
                    node.bounds.h,
                    if selected {
                        palette.selection_bg
                    } else {
                        palette.field_bg
                    },
                );
                let indent = (node.group.min(24) as i32) * 14;
                let mut text_x = node.bounds.x + 5 + indent;
                if node.value & 1 != 0 {
                    canvas.draw_text(
                        text_x,
                        node.bounds.y + (node.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
                        if node.value & 2 != 0 { "v" } else { ">" },
                        if selected {
                            palette.selection_text
                        } else {
                            palette.field_text
                        },
                    );
                    text_x += 12;
                }
                if node.value & 4 != 0 {
                    canvas.draw_text(
                        text_x,
                        node.bounds.y + (node.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
                        "*",
                        if selected {
                            palette.selection_text
                        } else {
                            palette.field_text
                        },
                    );
                    text_x += 10;
                }
                let old_clip = canvas.clip();
                canvas.set_clip(Some(node.bounds.inset(2)));
                canvas.draw_text(
                    text_x,
                    node.bounds.y + (node.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
                    &node.label,
                    if selected {
                        palette.selection_text
                    } else {
                        palette.field_text
                    },
                );
                canvas.set_clip(old_clip);
            }
            NODE_SCROLLBAR => {
                let mut scrollbar = gui::Scrollbar::new(gui::Axis::Vertical, node.bounds);
                scrollbar.set_extents(node.value.max(1) as u32, node.value_min.max(1) as u32);
                scrollbar.set_offset(node.value_max.max(0) as u32);
                scrollbar.draw(&mut canvas);
            }
            _ => {}
        }
    }
}

unsafe fn render_embedded_control(surface: *mut Surface, node: &DialogNode) {
    let Some(mut canvas) = canvas(surface) else {
        return;
    };
    let palette = theme::palette();
    let focused = node.flags & NODE_FOCUSED != 0;
    match node.kind {
        NODE_CHECKBOX => CheckBox::new("", node.bounds, node.value != 0).draw(&mut canvas, focused),
        NODE_RADIO => RadioButton::new("", node.bounds, node.value != 0).draw(&mut canvas, focused),
        NODE_FIELD | NODE_PASSWORD | NODE_TEXTAREA => {
            theme::draw_field(
                &mut canvas,
                node.bounds.x,
                node.bounds.y,
                node.bounds.w,
                node.bounds.h,
                focused,
            );
            let shown = if node.kind == NODE_PASSWORD {
                "*".repeat(node.label.chars().count())
            } else {
                node.label.clone()
            };
            let old_clip = canvas.clip();
            canvas.set_clip(Some(node.bounds.inset(3)));
            let mut line_y = node.bounds.y + 3;
            for line in shown.split('\n') {
                canvas.draw_text(node.bounds.x + 5, line_y, line, palette.field_text);
                line_y += FONT_LINE_HEIGHT;
                if line_y >= node.bounds.bottom() {
                    break;
                }
            }
            canvas.set_clip(old_clip);
        }
        NODE_COMBO => {
            theme::draw_field(
                &mut canvas,
                node.bounds.x,
                node.bounds.y,
                node.bounds.w,
                node.bounds.h,
                focused,
            );
            canvas.draw_text(
                node.bounds.x + 5,
                node.bounds.y + (node.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
                &node.label,
                palette.field_text,
            );
            canvas.draw_text(
                node.bounds.right() - 14,
                node.bounds.y + (node.bounds.h as i32 - FONT_LINE_HEIGHT) / 2,
                "v",
                palette.text,
            );
        }
        NODE_BUTTON => Button::new(
            &node.label,
            node.bounds.x,
            node.bounds.y,
            node.bounds.w,
            node.bounds.h,
        )
        .draw(&mut canvas, focused),
        NODE_SCROLLBAR => {
            let axis = if node.flags & 1 != 0 {
                gui::Axis::Vertical
            } else {
                gui::Axis::Horizontal
            };
            let mut scrollbar = gui::Scrollbar::new(axis, node.bounds);
            scrollbar.set_extents(node.value.max(1) as u32, node.value_min.max(1) as u32);
            scrollbar.set_offset(node.value_max.max(0) as u32);
            scrollbar.draw(&mut canvas);
        }
        _ => {}
    }
}

unsafe fn render_chrome(surface: *mut Surface, host: &NativeUiHost) {
    if !host.chrome_visible {
        return;
    }
    let Some(mut canvas) = canvas(surface) else {
        return;
    };
    let palette = theme::palette();
    let width = canvas.width();
    let height = canvas.height();
    canvas.fill_rect(0, 0, width, MENU_HEIGHT as u32, palette.content_bg);
    canvas.horizontal_line(0, MENU_HEIGHT - 1, width, palette.border);
    let mut x = 8i32;
    for label in MENU_LABELS {
        let label_width = label.chars().count() as i32 * gui::FONT_CELL_WIDTH + 18;
        canvas.draw_text(x + 9, 5, label, palette.text);
        x += label_width;
    }
    canvas.fill_rect(
        0,
        MENU_HEIGHT,
        width,
        TOOLBAR_HEIGHT as u32,
        palette.content_bg,
    );
    canvas.horizontal_line(0, CHROME_TOP - 1, width, palette.border);
    Button::new("‹", 8, MENU_HEIGHT + 6, 30, 26).draw(&mut canvas, false);
    Button::new("›", 44, MENU_HEIGHT + 6, 30, 26).draw(&mut canvas, false);
    Button::new("Reload", 80, MENU_HEIGHT + 6, 72, 26).draw(&mut canvas, false);
    let go_width = 42u32;
    let location_x = 160;
    let location_width = width.saturating_sub(location_x as u32 + go_width + 14);
    theme::draw_field(
        &mut canvas,
        location_x,
        MENU_HEIGHT + 6,
        location_width,
        26,
        false,
    );
    let old_clip = canvas.clip();
    canvas.set_clip(Some(gui_core::Rect::new(
        location_x + 5,
        MENU_HEIGHT + 7,
        location_width.saturating_sub(10),
        24,
    )));
    canvas.draw_text(
        location_x + 6,
        MENU_HEIGHT + 10,
        &host.location,
        palette.field_text,
    );
    canvas.set_clip(old_clip);
    Button::new(
        "Go",
        width.saturating_sub(go_width + 8) as i32,
        MENU_HEIGHT + 6,
        go_width,
        26,
    )
    .draw(&mut canvas, false);
    if height >= STATUS_HEIGHT as u32 {
        let status_y = height as i32 - STATUS_HEIGHT;
        canvas.fill_rect(0, status_y, width, STATUS_HEIGHT as u32, palette.content_bg);
        canvas.horizontal_line(0, status_y, width, palette.border);
        let old_clip = canvas.clip();
        canvas.set_clip(Some(gui_core::Rect::new(
            6,
            status_y + 1,
            width.saturating_sub(12),
            STATUS_HEIGHT as u32 - 2,
        )));
        canvas.draw_text(
            8,
            status_y + (STATUS_HEIGHT - FONT_LINE_HEIGHT) / 2,
            &host.status,
            palette.text,
        );
        canvas.set_clip(old_clip);
    }
}

fn chrome_action_at(surface_width: u32, x: i32, y: i32) -> Option<i64> {
    if y >= 0 && y < MENU_HEIGHT {
        let mut left = 8i32;
        for (index, label) in MENU_LABELS.iter().enumerate() {
            let right = left + label.chars().count() as i32 * gui::FONT_CELL_WIDTH + 18;
            if x >= left && x < right {
                return Some(CHROME_MENU_BASE + index as i64);
            }
            left = right;
        }
    }
    if y >= MENU_HEIGHT + 6 && y < CHROME_TOP - 6 {
        if (8..38).contains(&x) {
            return Some(CHROME_BACK);
        }
        if (44..74).contains(&x) {
            return Some(CHROME_FORWARD);
        }
        if (80..152).contains(&x) {
            return Some(CHROME_RELOAD);
        }
        if x >= surface_width.saturating_sub(50) as i32 {
            return Some(CHROME_GO);
        }
    }
    None
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_menu_open(
    surface: *mut Surface,
    menu_id: u64,
    nodes: *const UiNode,
    count: usize,
    selected: i32,
    anchor_x: i32,
    anchor_y: i32,
) -> c_int {
    let Some(surface_ref) = surface.as_ref() else {
        return -1;
    };
    if menu_id == 0 || count == 0 || count > MAX_MENU_NODES || nodes.is_null() {
        return -1;
    }
    let host = host();
    if let Some(previous) = host.menu.take() {
        restore_backing(surface, &previous);
    }
    let mut entries = Vec::with_capacity(count);
    let mut total_text = 0usize;
    for node in slice::from_raw_parts(nodes, count) {
        if node.version != ABI_VERSION || node.byte_len as usize != mem::size_of::<UiNode>() {
            return -1;
        }
        total_text = match total_text
            .checked_add(node.label.len as usize)
            .and_then(|bytes| bytes.checked_add(node.secondary.len as usize))
        {
            Some(bytes) if bytes <= MAX_TEXT_BYTES => bytes,
            _ => return -1,
        };
        let Some(label) = decode_text(node.label) else {
            return -1;
        };
        let Some(secondary) = decode_text(node.secondary) else {
            return -1;
        };
        entries.push(MenuEntry::new(
            node.id,
            &label,
            &secondary,
            MenuEntryFlags(node.flags),
        ));
    }
    let popup = MenuPopup::new(
        entries,
        (selected >= 0).then_some(selected as usize),
        anchor_x,
        anchor_y,
        surface_ref.width,
        surface_ref.height,
    );
    let Some(backing) = copy_backing(surface, &popup) else {
        return -1;
    };
    let hosted = HostedMenu {
        id: menu_id,
        popup,
        backing,
    };
    render(surface, &hosted);
    host.menu = Some(hosted);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_close(surface: *mut Surface, target: u64) {
    let host = host();
    if host.menu.as_ref().map(|menu| menu.id) == Some(target) {
        if let Some(menu) = host.menu.take() {
            restore_backing(surface, &menu);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_dialog_update(
    surface: *mut Surface,
    dialog_id: u64,
    nodes: *const UiNode,
    count: usize,
    _selected: i32,
) -> c_int {
    if dialog_id == 0 || count == 0 || count > MAX_MENU_NODES || nodes.is_null() {
        return -1;
    }
    let mut copied = Vec::with_capacity(count);
    let mut bounds = None;
    let mut total_text = 0usize;
    for node in slice::from_raw_parts(nodes, count) {
        if node.version != ABI_VERSION || node.byte_len as usize != mem::size_of::<UiNode>() {
            return -1;
        }
        total_text = match total_text.checked_add(node.label.len as usize) {
            Some(bytes) if bytes <= MAX_TEXT_BYTES => bytes,
            _ => return -1,
        };
        let Some(label) = decode_text(node.label) else {
            return -1;
        };
        let node_bounds = Rect::new(node.x, node.y, node.width, node.height);
        if node.kind == NODE_DIALOG {
            bounds = Some(node_bounds);
        }
        copied.push(DialogNode {
            kind: node.kind,
            flags: node.flags,
            bounds: node_bounds,
            value: node.value,
            value_min: node.value_min,
            value_max: node.value_max,
            group: node.group,
            label,
        });
    }
    let Some(bounds) = bounds else { return -1 };
    let host = host();
    let backing = if let Some(previous) = host.dialog.take() {
        if previous.id == dialog_id && previous.bounds == bounds {
            previous.backing
        } else {
            restore_rect(surface, previous.bounds, &previous.backing);
            let Some(backing) = copy_rect(surface, bounds) else {
                return -1;
            };
            backing
        }
    } else {
        let Some(backing) = copy_rect(surface, bounds) else {
            return -1;
        };
        backing
    };
    let dialog = HostedDialog {
        id: dialog_id,
        bounds,
        nodes: copied,
        backing,
    };
    render_dialog(surface, &dialog);
    host.dialog = Some(dialog);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_dialog_close(surface: *mut Surface, dialog_id: u64) {
    let host = host();
    if host.dialog.as_ref().map(|dialog| dialog.id) == Some(dialog_id) {
        if let Some(dialog) = host.dialog.take() {
            restore_rect(surface, dialog.bounds, &dialog.backing);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_control_draw(surface: *mut Surface, node: *const UiNode) {
    let Some(node) = node.as_ref() else { return };
    if node.version != ABI_VERSION || node.byte_len as usize != mem::size_of::<UiNode>() {
        return;
    }
    let Some(label) = decode_text(node.label) else {
        return;
    };
    let copied = DialogNode {
        kind: node.kind,
        flags: node.flags,
        bounds: Rect::new(node.x, node.y, node.width, node.height),
        value: node.value,
        value_min: node.value_min,
        value_max: node.value_max,
        group: node.group,
        label,
    };
    render_embedded_control(surface, &copied);
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_render(surface: *mut Surface) {
    let host = host();
    render_chrome(surface, host);
    if let Some(dialog) = host.dialog.as_ref() {
        render_dialog(surface, dialog);
    }
    if let Some(menu) = host.menu.as_ref() {
        render(surface, menu);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_chrome_show(surface: *mut Surface, visible: u32) {
    let host = host();
    host.chrome_visible = visible != 0;
    if host.chrome_visible {
        render_chrome(surface, host);
    }
}

unsafe fn set_chrome_text(surface: *mut Surface, pointer: *const u8, len: usize, status: bool) {
    if len > MAX_TEXT_BYTES || (len != 0 && pointer.is_null()) {
        return;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        slice::from_raw_parts(pointer, len)
    };
    let Ok(text) = str::from_utf8(bytes) else {
        return;
    };
    let host = host();
    if status {
        host.status.clear();
        host.status.push_str(text);
    } else {
        host.location.clear();
        host.location.push_str(text);
    }
    render_chrome(surface, host);
    if let Some(menu) = host.menu.as_ref() {
        render(surface, menu);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_chrome_set_status(
    surface: *mut Surface,
    pointer: *const u8,
    len: usize,
) {
    set_chrome_text(surface, pointer, len, true);
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_chrome_set_location(
    surface: *mut Surface,
    pointer: *const u8,
    len: usize,
) {
    set_chrome_text(surface, pointer, len, false);
}

#[no_mangle]
pub unsafe extern "C" fn ag_ui_handle_event(
    surface: *mut Surface,
    event: *const GuiEvent,
    result: *mut UiResult,
) {
    let Some(result) = result.as_mut() else {
        return;
    };
    result.reset();
    let Some(event) = event.as_ref() else { return };
    let host = host();
    if host.menu.is_none()
        && host.dialog.is_none()
        && host.chrome_visible
        && event.kind == EVENT_MOUSE
    {
        let action = chrome_action_at(
            surface
                .as_ref()
                .map(|surface| surface.width)
                .unwrap_or_default(),
            event.payload[0] as i32,
            event.payload[1] as i32,
        );
        match event.payload[3] {
            MOUSE_DOWN if action.is_some() => {
                host.chrome_pressed = action;
                result.consumed = 1;
                result.repaint = 1;
                render_chrome(surface, host);
                return;
            }
            MOUSE_UP if host.chrome_pressed.is_some() => {
                let pressed = host.chrome_pressed.take();
                result.consumed = 1;
                result.repaint = 1;
                if pressed == action {
                    result.action_kind = ACTION_CHROME;
                    result.value = pressed.unwrap_or_default();
                }
                render_chrome(surface, host);
                return;
            }
            MOUSE_MOVE if action.is_some() => {
                result.consumed = 1;
                return;
            }
            _ => {}
        }
    }
    let Some(menu) = host.menu.as_mut() else {
        return;
    };
    let response = match event.kind {
        EVENT_MOUSE => {
            let mask = event.payload[2];
            let pointer = PointerInput {
                x: event.payload[0] as i32,
                y: event.payload[1] as i32,
                buttons: MouseButtons {
                    left: mask & 1 != 0,
                    right: mask & 2 != 0,
                    middle: mask & 4 != 0,
                },
                modifiers: Modifiers::default(),
                kind: match event.payload[3] {
                    MOUSE_MOVE => PointerKind::Move,
                    MOUSE_DOWN => PointerKind::Down,
                    MOUSE_UP => PointerKind::Up,
                    MOUSE_SCROLL => PointerKind::Scroll {
                        delta_x: event.payload[4] as i32,
                        delta_y: event.payload[5] as i32,
                    },
                    _ => return,
                },
                timestamp: 0,
            };
            menu.popup.handle_pointer(pointer)
        }
        EVENT_KEY => {
            let modifiers = event.payload[2];
            menu.popup.handle_key(KeyInput {
                key: event.payload[0],
                character: char::from_u32(event.payload[1]).unwrap_or('\0'),
                modifiers: Modifiers {
                    shift: modifiers & 1 != 0,
                    ctrl: modifiers & 2 != 0,
                    alt: modifiers & 4 != 0,
                },
                pressed: event.payload[3] != 0,
            })
        }
        EVENT_THEME_CHANGED | EVENT_SETTINGS_CHANGED => {
            result.consumed = 1;
            result.repaint = 1;
            render(surface, menu);
            return;
        }
        EVENT_RESIZE => {
            result.consumed = 0;
            result.repaint = 1;
            result.action_kind = ACTION_CANCEL;
            result.target = menu.id;
            return;
        }
        _ => return,
    };
    result.consumed = response.consumed as u32;
    result.repaint = response.repaint as u32;
    if let Some(action) = response.action {
        result.target = menu.id;
        match action {
            MenuPopupAction::Activate(entry) => {
                result.action_kind = ACTION_ACTIVATE;
                result.value = entry as i64;
            }
            MenuPopupAction::Cancel => result.action_kind = ACTION_CANCEL,
        }
    }
    if response.repaint {
        render(surface, menu);
    }
}
