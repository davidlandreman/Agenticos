use alloc::string::{String, ToString};

use crate::window::theme::{self, FrameChrome};
use crate::window::{
    CompositorProperties, Event, EventResult, GraphicsDevice, Insets, Rect, Window, WindowId,
};

use super::base::WindowBase;

/// A window with themed decorations (title bar, borders, and retained shadow).
pub struct FrameWindow {
    base: WindowBase,
    title: String,
    active: bool,
    content_window_id: Option<WindowId>,
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
        }
    }

    pub fn set_content_window(&mut self, window_id: WindowId) {
        self.content_window_id = Some(window_id);
        self.base.add_child(window_id);
        self.base.invalidate();
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
                close_button_rect: theme::close_button_rect(bounds, theme::metrics()),
            },
            device,
        );
        self.base.clear_needs_repaint();
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
}
