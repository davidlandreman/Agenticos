use alloc::string::{String, ToString};

use crate::window::theme::{self, FrameChrome, FrameMetrics, ThemeKind};
use crate::window::{
    CompositorProperties, Event, EventResult, GraphicsDevice, Insets, Rect, Window, WindowId,
};

use super::base::WindowBase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePlacement {
    Normal,
    Maximized { restore_bounds: Rect },
}

/// A window with themed decorations (title bar, borders, and retained shadow).
pub struct FrameWindow {
    base: WindowBase,
    title: String,
    active: bool,
    content_window_id: Option<WindowId>,
    resizable: bool,
    placement: FramePlacement,
    minimized: bool,
}

impl FrameWindow {
    pub fn new(id: WindowId, title: &str) -> Self {
        let mut base = WindowBase::new_with_id(id, Rect::new(0, 0, 800, 600));
        let effect = theme::frame_effect();
        if effect != crate::graphics::scene::LayerEffect::None {
            base.set_compositor_properties(CompositorProperties {
                effect,
                ..CompositorProperties::OPAQUE
            });
        }
        Self {
            base,
            title: title.to_string(),
            active: false,
            content_window_id: None,
            resizable: true,
            placement: FramePlacement::Normal,
            minimized: false,
        }
    }

    pub fn set_content_window(&mut self, window_id: WindowId) {
        self.content_window_id = Some(window_id);
        self.base.add_child(window_id);
        self.base.invalidate();
    }

    pub fn set_title(&mut self, title: &str) {
        self.title.clear();
        self.title.push_str(title);
        self.base.invalidate();
    }

    pub fn set_resizable(&mut self, resizable: bool) {
        if self.resizable != resizable {
            self.resizable = resizable;
            self.base.invalidate();
        }
    }

    pub const fn is_resizable(&self) -> bool {
        self.resizable
    }

    pub const fn is_maximized(&self) -> bool {
        matches!(self.placement, FramePlacement::Maximized { .. })
    }

    pub const fn is_minimized(&self) -> bool {
        self.minimized
    }

    pub fn set_minimized(&mut self, minimized: bool) -> bool {
        if !self.resizable || self.minimized == minimized {
            return false;
        }
        self.minimized = minimized;
        self.base.set_visible(!minimized);
        self.base.invalidate();
        true
    }

    pub fn toggle_maximized(&mut self, work_area: Rect) -> Option<(Rect, Rect)> {
        if !self.resizable || self.minimized {
            return None;
        }
        let old = self.base.bounds();
        let next = match self.placement {
            FramePlacement::Normal => {
                self.placement = FramePlacement::Maximized {
                    restore_bounds: old,
                };
                work_area
            }
            FramePlacement::Maximized { restore_bounds } => {
                self.placement = FramePlacement::Normal;
                restore_bounds
            }
        };
        self.base.set_bounds(next);
        self.base.invalidate();
        Some((old, next))
    }

    pub fn content_area(&self) -> Rect {
        let metrics = theme::metrics();
        let border = metrics.border_width;
        Rect::new(
            border as i32,
            (metrics.title_bar_height + border) as i32,
            self.base.bounds().width.saturating_sub(2 * border),
            self.base
                .bounds()
                .height
                .saturating_sub(metrics.title_bar_height + 2 * border),
        )
    }

    /// Update decoration geometry/effects while preserving client dimensions.
    pub fn apply_theme(&mut self, old: FrameMetrics, new: FrameMetrics, kind: ThemeKind) {
        let reframe = |bounds: Rect| {
            let client_width = bounds.width.saturating_sub(old.border_width * 2);
            let client_height = bounds
                .height
                .saturating_sub(old.title_bar_height + old.border_width * 2);
            Rect::new(
                bounds.x,
                bounds.y,
                client_width.saturating_add(new.border_width * 2),
                client_height
                    .saturating_add(new.title_bar_height)
                    .saturating_add(new.border_width * 2),
            )
        };
        match self.placement {
            FramePlacement::Normal => self.base.set_bounds(reframe(self.base.bounds())),
            FramePlacement::Maximized { restore_bounds } => {
                self.placement = FramePlacement::Maximized {
                    restore_bounds: reframe(restore_bounds),
                };
            }
        }
        self.base.set_compositor_properties(CompositorProperties {
            effect: theme::frame_effect_for(kind),
            ..CompositorProperties::OPAQUE
        });
        self.base.invalidate();
    }
}

impl Window for FrameWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }
        let bounds = self.base.bounds();
        theme::draw_frame(
            &FrameChrome {
                bounds,
                title: &self.title,
                active: self.active,
                buttons: theme::caption_button_layout(bounds, theme::metrics(), self.resizable),
                maximized: self.is_maximized(),
            },
            device,
        );
        self.base.clear_needs_repaint();
    }

    fn wants_paint_overlay(&self) -> bool {
        theme::has_frame_overlay()
    }

    fn paint_overlay(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }
        let bounds = self.base.bounds();
        theme::draw_frame_overlay(
            &FrameChrome {
                bounds,
                title: &self.title,
                active: self.active,
                buttons: theme::caption_button_layout(bounds, theme::metrics(), self.resizable),
                maximized: self.is_maximized(),
            },
            device,
        );
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Focus(focus_event) => {
                self.active = focus_event.gained;
                self.base.invalidate();
                EventResult::Handled
            }
            _ => EventResult::Propagate,
        }
    }

    fn can_focus(&self) -> bool {
        true
    }
    fn decoration_insets(&self) -> Insets {
        Insets::uniform(theme::metrics().shadow_margin)
    }
    fn has_focus(&self) -> bool {
        self.active
    }
    fn set_focus(&mut self, focused: bool) {
        self.active = focused;
        self.base.invalidate();
    }
    fn window_title(&self) -> Option<&str> {
        Some(&self.title)
    }

    fn as_frame_window(&self) -> Option<&FrameWindow> {
        Some(self)
    }

    fn as_frame_window_mut(&mut self) -> Option<&mut FrameWindow> {
        Some(self)
    }
}
