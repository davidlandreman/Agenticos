//! Signed-coordinate clipping for graphics adapters.
//!
//! Drawing primitives accept signed `i32` positions that may be negative or
//! beyond the device's pixel grid. The helpers here intersect a candidate rect
//! against the device bounds and the active clip rect, returning `usize`
//! coordinates that are guaranteed in-range — or `None` when nothing is
//! visible. Intermediate math widens to `i64` so callers may pass extreme
//! inputs (e.g. `i32::MIN`, `u32::MAX`) without overflow.

use crate::window::Rect;

/// Result of clipping a rect: in-bounds, non-negative `(x, y, width, height)`
/// ready to hand to the framebuffer writer, or `None` when fully clipped.
pub type ClippedRect = (usize, usize, usize, usize);

/// Intersect `(x, y, w, h)` with the device bounds and optional clip rect.
pub fn clip_rect(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    device_width: usize,
    device_height: usize,
    clip: Option<&Rect>,
) -> Option<ClippedRect> {
    if w == 0 || h == 0 {
        return None;
    }

    let left = x as i64;
    let top = y as i64;
    let right = left + w as i64;
    let bottom = top + h as i64;

    let mut cl = left.max(0);
    let mut ct = top.max(0);
    let mut cr = right.min(device_width as i64);
    let mut cb = bottom.min(device_height as i64);

    if let Some(c) = clip {
        let xl = c.x as i64;
        let yt = c.y as i64;
        let xr = xl + c.width as i64;
        let yb = yt + c.height as i64;
        cl = cl.max(xl);
        ct = ct.max(yt);
        cr = cr.min(xr);
        cb = cb.min(yb);
    }

    if cr <= cl || cb <= ct {
        return None;
    }

    Some((
        cl as usize,
        ct as usize,
        (cr - cl) as usize,
        (cb - ct) as usize,
    ))
}

/// Test whether the single pixel `(x, y)` is visible on the device under the
/// active clip rect. Returns the in-bounds `(usize, usize)` when visible.
pub fn pixel_visible(
    x: i32,
    y: i32,
    device_width: usize,
    device_height: usize,
    clip: Option<&Rect>,
) -> Option<(usize, usize)> {
    if x < 0 || y < 0 {
        return None;
    }
    let xu = x as i64;
    let yu = y as i64;
    if xu >= device_width as i64 || yu >= device_height as i64 {
        return None;
    }
    if let Some(c) = clip {
        let xl = c.x as i64;
        let yt = c.y as i64;
        let xr = xl + c.width as i64;
        let yb = yt + c.height as i64;
        if xu < xl || yu < yt || xu >= xr || yu >= yb {
            return None;
        }
    }
    Some((x as usize, y as usize))
}

/// Cohen–Sutherland region codes.
const INSIDE: u8 = 0b0000;
const LEFT: u8 = 0b0001;
const RIGHT: u8 = 0b0010;
const BOTTOM: u8 = 0b0100;
const TOP: u8 = 0b1000;

fn region_code(x: i64, y: i64, xmin: i64, ymin: i64, xmax: i64, ymax: i64) -> u8 {
    let mut code = INSIDE;
    if x < xmin {
        code |= LEFT;
    } else if x >= xmax {
        code |= RIGHT;
    }
    if y < ymin {
        code |= TOP;
    } else if y >= ymax {
        code |= BOTTOM;
    }
    code
}

/// Cohen–Sutherland line clip against the visible region (device bounds
/// intersected with the active clip rect). Returns the clipped endpoints in
/// `i32` space, or `None` when the line is wholly outside.
///
/// The visible region is treated as `[xmin, xmax) × [ymin, ymax)` to match the
/// half-open convention of `clip_rect` and `pixel_visible`.
pub fn clip_line(
    mut x1: i32,
    mut y1: i32,
    mut x2: i32,
    mut y2: i32,
    device_width: usize,
    device_height: usize,
    clip: Option<&Rect>,
) -> Option<((i32, i32), (i32, i32))> {
    let xmin: i64 = clip.map_or(0, |c| (c.x as i64).max(0));
    let ymin: i64 = clip.map_or(0, |c| (c.y as i64).max(0));
    let xmax: i64 = clip.map_or(device_width as i64, |c| {
        (c.x as i64 + c.width as i64).min(device_width as i64)
    });
    let ymax: i64 = clip.map_or(device_height as i64, |c| {
        (c.y as i64 + c.height as i64).min(device_height as i64)
    });

    if xmax <= xmin || ymax <= ymin {
        return None;
    }

    let mut p1x = x1 as i64;
    let mut p1y = y1 as i64;
    let mut p2x = x2 as i64;
    let mut p2y = y2 as i64;

    let mut c1 = region_code(p1x, p1y, xmin, ymin, xmax, ymax);
    let mut c2 = region_code(p2x, p2y, xmin, ymin, xmax, ymax);

    loop {
        if c1 | c2 == INSIDE {
            x1 = p1x as i32;
            y1 = p1y as i32;
            x2 = p2x as i32;
            y2 = p2y as i32;
            return Some(((x1, y1), (x2, y2)));
        }
        if c1 & c2 != INSIDE {
            return None;
        }

        let outcode = if c1 != INSIDE { c1 } else { c2 };
        let (nx, ny);

        if outcode & TOP != 0 {
            // y < ymin — move to ymin
            nx = p1x + (p2x - p1x) * (ymin - p1y) / (p2y - p1y);
            ny = ymin;
        } else if outcode & BOTTOM != 0 {
            // y >= ymax — move to ymax - 1 (half-open)
            let target = ymax - 1;
            nx = p1x + (p2x - p1x) * (target - p1y) / (p2y - p1y);
            ny = target;
        } else if outcode & LEFT != 0 {
            ny = p1y + (p2y - p1y) * (xmin - p1x) / (p2x - p1x);
            nx = xmin;
        } else {
            // RIGHT
            let target = xmax - 1;
            ny = p1y + (p2y - p1y) * (target - p1x) / (p2x - p1x);
            nx = target;
        }

        if outcode == c1 {
            p1x = nx;
            p1y = ny;
            c1 = region_code(p1x, p1y, xmin, ymin, xmax, ymax);
        } else {
            p2x = nx;
            p2y = ny;
            c2 = region_code(p2x, p2y, xmin, ymin, xmax, ymax);
        }
    }
}
