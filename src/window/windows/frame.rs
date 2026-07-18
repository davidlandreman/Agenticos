use alloc::string::{String, ToString};

use crate::graphics::scene::LayerEffect;
use crate::window::theme::{self, FrameChrome, ThemeKind};
use crate::window::types::HitTestResult;
use crate::window::{
    CompositorProperties, Event, EventResult, GraphicsDevice, Insets, Point, Rect, Window, WindowId,
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
        if theme::active() == ThemeKind::Aero {
            base.set_compositor_properties(CompositorProperties {
                effect: LayerEffect::BackdropSample { radius: 4 },
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

    pub fn title(&self) -> &str {
        &self.title
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

    /// Perform a hit test at the given window-local coordinates.
    pub fn hit_test(&self, local_point: Point) -> HitTestResult {
        let bounds = self.base.bounds();
        let (x, y) = (local_point.x, local_point.y);
        if x < 0 || y < 0 || x >= bounds.width as i32 || y >= bounds.height as i32 {
            return HitTestResult::None;
        }
        let metrics = theme::metrics();
        let border = metrics.border_width as i32;
        let title_height = metrics.title_bar_height as i32;
        if y >= border
            && y < border + title_height
            && x >= border
            && x < bounds.width as i32 - border
        {
            let local_bounds = Rect::new(0, 0, bounds.width, bounds.height);
            if theme::close_button_rect(local_bounds, metrics).contains_point(local_point) {
                return HitTestResult::CloseButton;
            }
            return HitTestResult::TitleBar;
        }

        let at_left = x < border;
        let at_right = x >= bounds.width as i32 - border;
        let at_top = y < border;
        let at_bottom = y >= bounds.height as i32 - border;
        use crate::window::types::ResizeEdge;
        match (at_top, at_bottom, at_left, at_right) {
            (true, _, true, _) => HitTestResult::Border(ResizeEdge::TopLeft),
            (true, _, _, true) => HitTestResult::Border(ResizeEdge::TopRight),
            (_, true, true, _) => HitTestResult::Border(ResizeEdge::BottomLeft),
            (_, true, _, true) => HitTestResult::Border(ResizeEdge::BottomRight),
            (true, _, _, _) => HitTestResult::Border(ResizeEdge::Top),
            (_, true, _, _) => HitTestResult::Border(ResizeEdge::Bottom),
            (_, _, true, _) => HitTestResult::Border(ResizeEdge::Left),
            (_, _, _, true) => HitTestResult::Border(ResizeEdge::Right),
            _ => HitTestResult::Client,
        }
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
