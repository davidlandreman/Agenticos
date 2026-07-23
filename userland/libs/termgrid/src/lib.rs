//! Terminal cell renderer.
//!
//! Paints a [`vte::Screen`] viewport into a caller-owned little-endian
//! XRGB8888 buffer, honoring per-cell foreground/background color
//! (`ColorSpec` resolved through `vte::colors`), the `REVERSE`/`UNDERLINE`
//! attributes, and the block caret. This mirrors the kernel `TextWindow` paint
//! path but runs in ring-3 and writes the surface the app hands to
//! `gui_win_present`.
//!
//! The `ColorSpec::Default` background is rendered with the caller-supplied
//! `default_bg` word so the app can substitute the active theme's translucent
//! content-well color, exactly like `sync_text_window_from_screen`'s
//! `bg_is_default` bit in the kernel.

#![no_std]

extern crate alloc;

pub mod font;

pub use font::TermFont;

use vte::screen::attrs;
use vte::{Screen, ColorSpec};

/// Default JetBrains Mono size the kernel terminal renders at.
pub const DEFAULT_FONT_PX: u16 = 16;

/// Parameters for one full grid repaint.
pub struct RenderParams {
    /// Framebuffer width in pixels (row stride == width).
    pub width_px: usize,
    /// Framebuffer height in pixels.
    pub height_px: usize,
    /// XRGB8888 word painted where a cell's background is `ColorSpec::Default`.
    pub default_bg: u32,
    /// Whether the window is focused (caret only renders when focused).
    pub focused: bool,
    /// Caret blink phase — `true` shows the caret this frame.
    pub caret_on: bool,
}

/// Repaint the whole grid into `fb` (length must be `width_px * height_px`).
pub fn render(font: &mut TermFont, screen: &Screen, fb: &mut [u32], p: &RenderParams) {
    let cell_w = font.cell_width() as usize;
    let line_h = font.line_height() as usize;
    let ascent = font.ascent();
    let stride = p.width_px;

    let rows = screen.rows().min(p.height_px / line_h);
    let cols = screen.cols().min(p.width_px / cell_w);

    let caret = screen.caret();
    // The caret's row refers to the live buffer; hide it while the user
    // is scrolled back into history.
    let show_caret = p.focused && p.caret_on && caret.visible && screen.view_offset() == 0;

    for r in 0..rows {
        let row = screen.visible_row(r);
        let y0 = r * line_h;
        for c in 0..cols {
            let cell = row.get(c).copied().unwrap_or(vte::Cell::EMPTY);
            let x0 = c * cell_w;
            let is_caret = show_caret && r == caret.row && c == caret.col;
            draw_cell(font, fb, stride, p, x0, y0, cell_w, line_h, ascent, cell, is_caret);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_cell(
    font: &mut TermFont,
    fb: &mut [u32],
    stride: usize,
    p: &RenderParams,
    x0: usize,
    y0: usize,
    cell_w: usize,
    line_h: usize,
    ascent: i32,
    cell: vte::Cell,
    is_caret: bool,
) {
    // Resolve colors. Keep the default-bg substitution for the theme well.
    let mut fg = resolve_fg(cell.fg);
    let mut bg = match cell.bg {
        ColorSpec::Default => p.default_bg,
        other => resolve_bg(other),
    };
    if cell.attrs & attrs::REVERSE != 0 {
        core::mem::swap(&mut fg, &mut bg);
    }
    // A block caret inverts the cell so the glyph stays legible.
    if is_caret {
        core::mem::swap(&mut fg, &mut bg);
    }
    if cell.attrs & attrs::BOLD != 0 {
        fg = brighten(fg);
    }

    // Background fill.
    fill_rect(fb, stride, x0, y0, cell_w, line_h, bg);

    // Glyph.
    if cell.ch != ' ' && cell.ch != '\0' {
        let glyph = font.glyph(cell.ch);
        let gx0 = x0 as i32 + glyph.x_offset as i32;
        let gy0 = y0 as i32 + ascent + glyph.y_offset as i32;
        let gw = glyph.width as i32;
        let gh = glyph.height as i32;
        for gy in 0..gh {
            let py = gy0 + gy;
            if py < 0 || py as usize >= (fb.len() / stride) {
                continue;
            }
            for gx in 0..gw {
                let px = gx0 + gx;
                if px < 0 || px as usize >= stride {
                    continue;
                }
                let alpha = glyph.coverage[(gy * gw + gx) as usize];
                if alpha == 0 {
                    continue;
                }
                let idx = py as usize * stride + px as usize;
                fb[idx] = blend(fg, fb[idx], alpha);
            }
        }
    }

    // Underline.
    if cell.attrs & attrs::UNDERLINE != 0 {
        let uy = y0 as i32 + ascent + 1;
        if uy >= 0 && (uy as usize) < fb.len() / stride {
            fill_rect(fb, stride, x0, uy as usize, cell_w, 1, fg);
        }
    }
}

fn resolve_fg(spec: ColorSpec) -> u32 {
    vte::colors::resolve(spec, true).to_xrgb()
}

fn resolve_bg(spec: ColorSpec) -> u32 {
    vte::colors::resolve(spec, false).to_xrgb()
}

fn fill_rect(fb: &mut [u32], stride: usize, x: usize, y: usize, w: usize, h: usize, color: u32) {
    let height = fb.len() / stride;
    for row in y..(y + h).min(height) {
        let base = row * stride;
        let end = (x + w).min(stride);
        for px in fb[base + x..base + end].iter_mut() {
            *px = color;
        }
    }
}

/// Alpha-blend `fg` over `bg` by an 8-bit coverage value.
fn blend(fg: u32, bg: u32, alpha: u8) -> u32 {
    if alpha == 255 {
        return fg;
    }
    let a = alpha as u32;
    let inv = 255 - a;
    let ch = |shift: u32| {
        let f = (fg >> shift) & 0xFF;
        let b = (bg >> shift) & 0xFF;
        ((f * a + b * inv) / 255) & 0xFF
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Lighten a color toward white for a synthetic-bold effect.
fn brighten(color: u32) -> u32 {
    let ch = |shift: u32| {
        let v = (color >> shift) & 0xFF;
        (v + (255 - v) / 3) & 0xFF
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}
