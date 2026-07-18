use gui_core::{
    Axis, ControlResponse, PointerInput, PointerKind, Rect, ScrollState, ScrollbarGeometry,
};

use crate::{theme, Canvas, FONT_CELL_WIDTH, FONT_LINE_HEIGHT};

pub const SCROLLBAR_THICKNESS: u32 = 16;
const MIN_THUMB: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Decrement,
    Increment,
    Thumb,
    TrackBefore,
    TrackAfter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarAction {
    Changed,
}

pub struct Scrollbar {
    axis: Axis,
    bounds: Rect,
    state: ScrollState,
    enabled: bool,
    hot: Option<Part>,
    pressed: Option<Part>,
    drag_grab: i32,
}

impl Scrollbar {
    pub fn new(axis: Axis, bounds: Rect) -> Self {
        Self {
            axis,
            bounds,
            state: ScrollState::default(),
            enabled: true,
            hot: None,
            pressed: None,
            drag_grab: 0,
        }
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    pub fn state(&self) -> ScrollState {
        self.state
    }

    pub fn state_mut(&mut self) -> &mut ScrollState {
        &mut self.state
    }

    pub fn set_extents(&mut self, content: u32, viewport: u32) -> bool {
        self.state.set_extents(content, viewport)
    }

    pub fn set_offset(&mut self, offset: u32) -> bool {
        self.state.set_offset(offset)
    }

    pub fn offset(&self) -> u32 {
        self.state.offset()
    }

    pub fn set_line_step(&mut self, step: u32) {
        self.state.set_line_step(step);
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.cancel();
        }
    }

    pub fn is_captured(&self) -> bool {
        self.pressed.is_some()
    }

    pub fn cancel(&mut self) {
        self.hot = None;
        self.pressed = None;
        self.drag_grab = 0;
    }

    fn geometry(&self) -> ScrollbarGeometry {
        ScrollbarGeometry::calculate(self.axis, self.bounds, self.state, MIN_THUMB)
    }

    fn part_at(&self, x: i32, y: i32) -> Option<Part> {
        let geometry = self.geometry();
        if geometry.decrement.contains(x, y) {
            Some(Part::Decrement)
        } else if geometry.increment.contains(x, y) {
            Some(Part::Increment)
        } else if geometry.thumb.contains(x, y) {
            Some(Part::Thumb)
        } else if geometry.track.contains(x, y) {
            let before = match self.axis {
                Axis::Horizontal => x < geometry.thumb.x,
                Axis::Vertical => y < geometry.thumb.y,
            };
            Some(if before {
                Part::TrackBefore
            } else {
                Part::TrackAfter
            })
        } else {
            None
        }
    }

    pub fn handle_pointer(&mut self, input: PointerInput) -> ControlResponse<ScrollbarAction> {
        let active = self.enabled && self.state.has_range();
        match input.kind {
            PointerKind::Cancel => {
                let repaint = self.hot.is_some() || self.pressed.is_some();
                self.cancel();
                ControlResponse::consumed(repaint, None)
            }
            PointerKind::Move => {
                if self.pressed == Some(Part::Thumb) {
                    let geometry = self.geometry();
                    let coordinate = match self.axis {
                        Axis::Horizontal => input.x,
                        Axis::Vertical => input.y,
                    };
                    let offset = geometry.offset_for_thumb_start(
                        self.axis,
                        coordinate.saturating_sub(self.drag_grab),
                        self.state,
                    );
                    let changed = self.state.set_offset(offset);
                    return ControlResponse::consumed(
                        changed,
                        changed.then_some(ScrollbarAction::Changed),
                    );
                }
                let next = self.part_at(input.x, input.y);
                let repaint = next != self.hot;
                self.hot = next;
                ControlResponse {
                    consumed: self.bounds.contains(input.x, input.y),
                    repaint,
                    action: None,
                }
            }
            PointerKind::Down if self.bounds.contains(input.x, input.y) => {
                let part = self.part_at(input.x, input.y);
                self.hot = part;
                if !active {
                    return ControlResponse::consumed(true, None);
                }
                self.pressed = part;
                let changed = match part {
                    Some(Part::Decrement) => self.state.scroll_lines(-1),
                    Some(Part::Increment) => self.state.scroll_lines(1),
                    Some(Part::TrackBefore) => self.state.scroll_pages(-1),
                    Some(Part::TrackAfter) => self.state.scroll_pages(1),
                    Some(Part::Thumb) => {
                        let geometry = self.geometry();
                        self.drag_grab = match self.axis {
                            Axis::Horizontal => input.x - geometry.thumb.x,
                            Axis::Vertical => input.y - geometry.thumb.y,
                        };
                        false
                    }
                    None => false,
                };
                ControlResponse::consumed(true, changed.then_some(ScrollbarAction::Changed))
            }
            PointerKind::Up if self.pressed.is_some() => {
                self.pressed = None;
                self.hot = self.part_at(input.x, input.y);
                ControlResponse::consumed(true, None)
            }
            PointerKind::Scroll { delta_x, delta_y } if self.bounds.contains(input.x, input.y) => {
                let delta = match self.axis {
                    Axis::Horizontal => delta_x,
                    Axis::Vertical => delta_y,
                };
                let changed = self.state.scroll_lines(delta);
                ControlResponse::consumed(changed, changed.then_some(ScrollbarAction::Changed))
            }
            _ => ControlResponse::ignored(),
        }
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        let geometry = self.geometry();
        let enabled = self.enabled && self.state.has_range();
        theme::draw_scrollbar_track(canvas, geometry.bounds);
        for (part, rect) in [
            (Part::Decrement, geometry.decrement),
            (Part::Increment, geometry.increment),
            (Part::Thumb, geometry.thumb),
        ] {
            theme::draw_scrollbar_part(
                canvas,
                rect,
                enabled,
                self.hot == Some(part),
                self.pressed == Some(part),
            );
        }
        let palette = theme::palette();
        let (dec, inc) = match self.axis {
            Axis::Horizontal => ('<', '>'),
            Axis::Vertical => ('^', 'v'),
        };
        for (character, rect) in [(dec, geometry.decrement), (inc, geometry.increment)] {
            let x = rect.x + (rect.w as i32 - FONT_CELL_WIDTH) / 2;
            let y = rect.y + (rect.h as i32 - FONT_LINE_HEIGHT) / 2;
            canvas.draw_char(
                x,
                y,
                character,
                if enabled {
                    palette.text
                } else {
                    palette.disabled_text
                },
            );
        }
    }
}
