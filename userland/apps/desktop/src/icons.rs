//! Procedural Start-menu icons, drawn with `Canvas` primitives in the same
//! spirit as the kernel's SVG icons and the ring-3 `file_ui` icons. Each icon
//! renders inside an `s`×`s` box at `(x, y)` using the theme's foreground and
//! accent colors so it tracks the active theme.

use gui::Canvas;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Programs,
    Documents,
    Settings,
    Run,
    ShutDown,
    FileManager,
    WebBrowser,
    Terminal,
    Notepad,
    Painting,
    Calc,
    GlGame,
    TaskManager,
}

/// Draw `icon` inside `(x, y, s, s)`. `fg` is the primary line color, `accent`
/// the theme highlight.
pub fn draw(canvas: &mut Canvas, icon: Icon, x: i32, y: i32, s: i32, fg: u32, accent: u32) {
    match icon {
        Icon::Programs => {
            // Four small app tiles in a 2×2 grid.
            let t = (s * 5 / 12).max(3);
            let gap = (s - 2 * t).max(1) / 3;
            for (col, row) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let ix = x + gap + col * (t + gap);
                let iy = y + gap + row * (t + gap);
                let color = if (col + row) % 2 == 0 { accent } else { fg };
                canvas.fill_rect(ix, iy, t as u32, t as u32, color);
            }
        }
        Icon::Documents => {
            page(canvas, x, y, s, fg);
        }
        Icon::Settings => {
            // Gear: outer ring with four teeth and a hub.
            let cx = x + s / 2;
            let cy = y + s / 2;
            let r = s * 5 / 12;
            canvas.rect(cx - r, cy - r, (2 * r) as u32, (2 * r) as u32, fg);
            canvas.horizontal_line(cx - 1, cy - r - 2, 3, fg);
            canvas.horizontal_line(cx - 1, cy + r, 3, fg);
            canvas.vertical_line(cx - r - 2, cy - 1, 3, fg);
            canvas.vertical_line(cx + r, cy - 1, 3, fg);
            canvas.fill_rect(cx - 2, cy - 2, 4, 4, accent);
        }
        Icon::Run => {
            // Command box with a prompt caret.
            command_box(canvas, x, y, s, fg, accent);
        }
        Icon::ShutDown => {
            // Power symbol: a ring open at the top with a vertical stem.
            let cx = x + s / 2;
            let cy = y + s / 2 + 1;
            let r = s * 4 / 12;
            let side = (2 * r) as u32;
            canvas.horizontal_line(cx - r, cy + r, side, accent); // bottom
            canvas.vertical_line(cx - r, cy - r, side, accent); // left
            canvas.vertical_line(cx + r, cy - r, side, accent); // right
            // Top edge split into two halves, leaving the power gap.
            canvas.horizontal_line(cx - r, cy - r, (r - 1).max(0) as u32, accent);
            canvas.horizontal_line(cx + 2, cy - r, (r - 1).max(0) as u32, accent);
            canvas.vertical_line(cx, y + 1, (s / 2) as u32, fg); // stem
        }
        Icon::FileManager => {
            folder(canvas, x, y, s, accent);
        }
        Icon::WebBrowser => {
            // Globe: square ring with a meridian and an equator.
            let cx = x + s / 2;
            let cy = y + s / 2;
            let r = s * 5 / 12;
            canvas.rect(cx - r, cy - r, (2 * r) as u32, (2 * r) as u32, accent);
            canvas.vertical_line(cx, cy - r, (2 * r) as u32, accent);
            canvas.horizontal_line(cx - r, cy, (2 * r) as u32, accent);
        }
        Icon::Terminal => {
            // Dark console with a ">" prompt.
            let w = s * 11 / 12;
            let h = s * 5 / 6;
            let ix = x + (s - w) / 2;
            let iy = y + (s - h) / 2;
            canvas.fill_rect(ix, iy, w as u32, h as u32, 0x101820);
            canvas.rect(ix, iy, w as u32, h as u32, fg);
            canvas.draw_text(ix + 3, iy + 3, ">", accent);
        }
        Icon::Notepad => {
            page(canvas, x, y, s, fg);
            // A small accent pencil across the page corner.
            for i in 0..(s / 3) {
                canvas.pixel(x + s - 4 - i, y + 3 + i, accent);
            }
        }
        Icon::Painting => {
            // Palette: a rounded blob with three paint dots.
            let cx = x + s / 2;
            let cy = y + s / 2;
            let r = s * 5 / 12;
            canvas.rect(cx - r, cy - r, (2 * r) as u32, (2 * r) as u32, fg);
            canvas.fill_rect(cx - r + 2, cy - 2, 2, 2, accent);
            canvas.fill_rect(cx - 1, cy - r + 2, 2, 2, 0xC83232);
            canvas.fill_rect(cx + r - 4, cy, 2, 2, 0x32C832);
        }
        Icon::Calc => {
            // Calculator: display strip over a button grid.
            let w = s * 5 / 6;
            let h = s;
            let ix = x + (s - w) / 2;
            canvas.rect(ix, y, w as u32, h as u32, fg);
            canvas.fill_rect(ix + 2, y + 2, (w - 4) as u32, (s / 5) as u32, accent);
            let by = y + s / 3;
            for r in 0..3 {
                for c in 0..3 {
                    canvas.fill_rect(ix + 2 + c * (s / 5), by + r * (s / 6), 2, 2, fg);
                }
            }
        }
        Icon::GlGame => {
            // Wireframe cube.
            let d = s / 4;
            let a = s / 6;
            let bx = x + a;
            let by = y + s - a - d * 2;
            canvas.rect(bx, by, (d * 2) as u32, (d * 2) as u32, accent);
            canvas.rect(bx + d, by - d, (d * 2) as u32, (d * 2) as u32, fg);
            canvas.pixel(bx, by, fg);
            canvas.pixel(bx + d * 2, by, fg);
        }
        Icon::TaskManager => {
            // Bar chart of three columns.
            let base = y + s - 2;
            let bw = (s / 5).max(2);
            let heights = [s / 2, s * 5 / 6, s * 2 / 3];
            for (i, h) in heights.iter().enumerate() {
                let bx = x + 2 + i as i32 * (bw + 2);
                let color = if i == 1 { accent } else { fg };
                canvas.fill_rect(bx, base - h, bw as u32, *h as u32, color);
            }
        }
    }
}

/// A document page with a folded corner and a few text lines.
fn page(canvas: &mut Canvas, x: i32, y: i32, s: i32, fg: u32) {
    let w = s * 2 / 3;
    let h = s * 5 / 6;
    let ix = x + (s - w) / 2;
    let iy = y + (s - h) / 2;
    canvas.rect(ix, iy, w as u32, h as u32, fg);
    canvas.horizontal_line(ix + 2, iy + 3, (w - 6) as u32, fg);
    canvas.horizontal_line(ix + 2, iy + 6, (w - 6) as u32, fg);
    canvas.horizontal_line(ix + 2, iy + 9, (w - 8) as u32, fg);
}

/// A classic folder tab silhouette.
fn folder(canvas: &mut Canvas, x: i32, y: i32, s: i32, color: u32) {
    let w = s * 5 / 6;
    let h = s * 2 / 3;
    let ix = x + (s - w) / 2;
    let iy = y + (s - h) / 2;
    canvas.fill_rect(ix, iy + 2, w as u32, (h - 2) as u32, color);
    canvas.fill_rect(ix, iy, (w / 2) as u32, 3, color);
}

/// A `>` prompt inside a bordered box (Run).
fn command_box(canvas: &mut Canvas, x: i32, y: i32, s: i32, fg: u32, accent: u32) {
    let w = s * 5 / 6;
    let h = s * 2 / 3;
    let ix = x + (s - w) / 2;
    let iy = y + (s - h) / 2;
    canvas.rect(ix, iy, w as u32, h as u32, fg);
    canvas.draw_text(ix + 2, iy + 2, ">", accent);
}
