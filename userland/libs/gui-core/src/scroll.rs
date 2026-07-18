use crate::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarPolicy {
    Never,
    Auto,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    content: u32,
    viewport: u32,
    offset: u32,
    line_step: u32,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new(0, 0, 16)
    }
}

impl ScrollState {
    pub const fn new(content: u32, viewport: u32, line_step: u32) -> Self {
        Self {
            content,
            viewport,
            offset: 0,
            line_step: if line_step == 0 { 1 } else { line_step },
        }
    }

    pub fn content(self) -> u32 {
        self.content
    }

    pub fn viewport(self) -> u32 {
        self.viewport
    }

    pub fn offset(self) -> u32 {
        self.offset
    }

    pub fn max_offset(self) -> u32 {
        self.content.saturating_sub(self.viewport)
    }

    pub fn has_range(self) -> bool {
        self.max_offset() > 0
    }

    pub fn line_step(self) -> u32 {
        self.line_step
    }

    pub fn page_step(self) -> u32 {
        self.viewport
            .saturating_sub(self.line_step)
            .max(self.line_step)
    }

    pub fn set_line_step(&mut self, step: u32) {
        self.line_step = step.max(1);
    }

    pub fn set_extents(&mut self, content: u32, viewport: u32) -> bool {
        let previous = *self;
        self.content = content;
        self.viewport = viewport;
        self.offset = self.offset.min(self.max_offset());
        *self != previous
    }

    pub fn set_offset(&mut self, offset: u32) -> bool {
        let next = offset.min(self.max_offset());
        if next == self.offset {
            false
        } else {
            self.offset = next;
            true
        }
    }

    pub fn scroll_by(&mut self, delta: i32) -> bool {
        let next = if delta < 0 {
            self.offset.saturating_sub(delta.unsigned_abs())
        } else {
            self.offset.saturating_add(delta as u32)
        };
        self.set_offset(next)
    }

    pub fn scroll_lines(&mut self, lines: i32) -> bool {
        self.scroll_by(lines.saturating_mul(self.line_step as i32))
    }

    pub fn scroll_pages(&mut self, pages: i32) -> bool {
        self.scroll_by(pages.saturating_mul(self.page_step() as i32))
    }

    pub fn ensure_visible(&mut self, start: u32, end: u32) -> bool {
        let start = start.min(self.content);
        let end = end.max(start).min(self.content);
        let target = if start < self.offset {
            start
        } else if end > self.offset.saturating_add(self.viewport) {
            if end.saturating_sub(start) > self.viewport {
                start
            } else {
                end.saturating_sub(self.viewport)
            }
        } else {
            self.offset
        };
        self.set_offset(target)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarsLayout {
    pub viewport: Rect,
    pub horizontal: Option<Rect>,
    pub vertical: Option<Rect>,
    pub corner: Option<Rect>,
}

fn policy_visible(policy: ScrollbarPolicy, overflow: bool) -> bool {
    match policy {
        ScrollbarPolicy::Never => false,
        ScrollbarPolicy::Auto => overflow,
        ScrollbarPolicy::Always => true,
    }
}

pub fn layout_scrollbars(
    outer: Rect,
    content_w: u32,
    content_h: u32,
    horizontal_policy: ScrollbarPolicy,
    vertical_policy: ScrollbarPolicy,
    thickness: u32,
) -> ScrollbarsLayout {
    let thickness = thickness.min(outer.w).min(outer.h);
    let mut horizontal = horizontal_policy == ScrollbarPolicy::Always;
    let mut vertical = vertical_policy == ScrollbarPolicy::Always;
    for _ in 0..4 {
        let viewport_w = outer.w.saturating_sub(if vertical { thickness } else { 0 });
        let viewport_h = outer
            .h
            .saturating_sub(if horizontal { thickness } else { 0 });
        let next_h = policy_visible(horizontal_policy, content_w > viewport_w);
        let next_v = policy_visible(vertical_policy, content_h > viewport_h);
        if next_h == horizontal && next_v == vertical {
            break;
        }
        horizontal = next_h;
        vertical = next_v;
    }

    let viewport = Rect::new(
        outer.x,
        outer.y,
        outer.w.saturating_sub(if vertical { thickness } else { 0 }),
        outer
            .h
            .saturating_sub(if horizontal { thickness } else { 0 }),
    );
    let hbar = horizontal.then_some(Rect::new(outer.x, viewport.bottom(), viewport.w, thickness));
    let vbar = vertical.then_some(Rect::new(viewport.right(), outer.y, thickness, viewport.h));
    let corner = (horizontal && vertical).then_some(Rect::new(
        viewport.right(),
        viewport.bottom(),
        thickness,
        thickness,
    ));
    ScrollbarsLayout {
        viewport,
        horizontal: hbar,
        vertical: vbar,
        corner,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    pub bounds: Rect,
    pub decrement: Rect,
    pub increment: Rect,
    pub track: Rect,
    pub thumb: Rect,
}

impl ScrollbarGeometry {
    pub fn calculate(axis: Axis, bounds: Rect, state: ScrollState, min_thumb: u32) -> Self {
        let length = match axis {
            Axis::Horizontal => bounds.w,
            Axis::Vertical => bounds.h,
        };
        let cross = match axis {
            Axis::Horizontal => bounds.h,
            Axis::Vertical => bounds.w,
        };
        let button = cross.min(length / 2);
        let track_length = length.saturating_sub(button.saturating_mul(2));
        let thumb_length = if state.content() == 0 || track_length == 0 {
            track_length
        } else {
            (((track_length as u64 * state.viewport() as u64) / state.content() as u64) as u32)
                .max(min_thumb.min(track_length))
                .min(track_length)
        };
        let thumb_range = track_length.saturating_sub(thumb_length);
        let thumb_offset = if state.max_offset() == 0 {
            0
        } else {
            ((state.offset() as u64 * thumb_range as u64) / state.max_offset() as u64) as u32
        };

        match axis {
            Axis::Horizontal => {
                let decrement = Rect::new(bounds.x, bounds.y, button, bounds.h);
                let track = Rect::new(
                    bounds.x.saturating_add(button as i32),
                    bounds.y,
                    track_length,
                    bounds.h,
                );
                let thumb = Rect::new(
                    track.x.saturating_add(thumb_offset as i32),
                    track.y,
                    thumb_length,
                    track.h,
                );
                let increment = Rect::new(
                    bounds.right().saturating_sub(button as i32),
                    bounds.y,
                    button,
                    bounds.h,
                );
                Self {
                    bounds,
                    decrement,
                    increment,
                    track,
                    thumb,
                }
            }
            Axis::Vertical => {
                let decrement = Rect::new(bounds.x, bounds.y, bounds.w, button);
                let track = Rect::new(
                    bounds.x,
                    bounds.y.saturating_add(button as i32),
                    bounds.w,
                    track_length,
                );
                let thumb = Rect::new(
                    track.x,
                    track.y.saturating_add(thumb_offset as i32),
                    track.w,
                    thumb_length,
                );
                let increment = Rect::new(
                    bounds.x,
                    bounds.bottom().saturating_sub(button as i32),
                    bounds.w,
                    button,
                );
                Self {
                    bounds,
                    decrement,
                    increment,
                    track,
                    thumb,
                }
            }
        }
    }

    pub fn offset_for_thumb_start(self, axis: Axis, thumb_start: i32, state: ScrollState) -> u32 {
        let (track_start, track_length, thumb_length) = match axis {
            Axis::Horizontal => (self.track.x, self.track.w, self.thumb.w),
            Axis::Vertical => (self.track.y, self.track.h, self.thumb.h),
        };
        let thumb_range = track_length.saturating_sub(thumb_length);
        if thumb_range == 0 || state.max_offset() == 0 {
            return 0;
        }
        let pixel = thumb_start
            .saturating_sub(track_start)
            .clamp(0, thumb_range as i32) as u32;
        ((pixel as u64 * state.max_offset() as u64) / thumb_range as u64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_clamps_and_ensures_visible() {
        let mut state = ScrollState::new(100, 20, 4);
        assert!(state.set_offset(200));
        assert_eq!(state.offset(), 80);
        assert!(state.ensure_visible(10, 11));
        assert_eq!(state.offset(), 10);
        assert!(state.ensure_visible(50, 55));
        assert_eq!(state.offset(), 35);
        state.set_extents(10, 20);
        assert_eq!(state.offset(), 0);
    }

    #[test]
    fn cross_axis_overflow_stabilizes() {
        let layout = layout_scrollbars(
            Rect::new(0, 0, 100, 100),
            100,
            101,
            ScrollbarPolicy::Auto,
            ScrollbarPolicy::Auto,
            16,
        );
        assert!(layout.horizontal.is_some());
        assert!(layout.vertical.is_some());
        assert_eq!(layout.viewport, Rect::new(0, 0, 84, 84));
    }

    #[test]
    fn thumb_round_trip_hits_track_ends() {
        let state = ScrollState::new(1000, 100, 10);
        let bounds = Rect::new(0, 0, 16, 200);
        let start = ScrollbarGeometry::calculate(Axis::Vertical, bounds, state, 10);
        assert_eq!(start.thumb.y, start.track.y);
        let mut end_state = state;
        end_state.set_offset(end_state.max_offset());
        let end = ScrollbarGeometry::calculate(Axis::Vertical, bounds, end_state, 10);
        assert_eq!(end.thumb.bottom(), end.track.bottom());
        assert_eq!(
            end.offset_for_thumb_start(Axis::Vertical, end.thumb.y, end_state),
            end_state.max_offset()
        );
    }
}
