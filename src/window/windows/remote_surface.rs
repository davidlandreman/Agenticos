//! Kernel-owned copy-blit surface presented by a ring-3 process.

use bootloader_api::info::PixelFormat;

use crate::graphics::color::Color;
use crate::userland::gui;
use crate::window::event::{Event, EventResult, FocusEvent, ResizeEvent};
use crate::window::{GraphicsDevice, Rect, Window, WindowBuffer, WindowId};

use super::base::WindowBase;

pub struct RemoteSurface {
    base: WindowBase,
    owner_pid: u32,
    handle: u32,
    buffer: WindowBuffer,
}

impl RemoteSurface {
    pub fn new(id: WindowId, bounds: Rect, owner_pid: u32, handle: u32) -> Self {
        let mut base = WindowBase::new_with_id(id, bounds);
        base.set_can_focus(true);
        Self {
            base,
            owner_pid,
            handle,
            // ABI v1 is little-endian XRGB8888: bytes B, G, R, unused.
            buffer: WindowBuffer::new(
                bounds.width as usize,
                bounds.height as usize,
                PixelFormat::Bgr,
                4,
            ),
        }
    }

    pub fn present(&mut self, pixels: &[u8], width: u32, height: u32, stride: usize) -> bool {
        let bounds = self.base.bounds();
        if width != bounds.width || height != bounds.height || stride < width as usize * 4 {
            return false;
        }
        let Some(required) = stride.checked_mul(height as usize) else {
            return false;
        };
        if pixels.len() < required {
            return false;
        }
        let mut next = WindowBuffer::new(width as usize, height as usize, PixelFormat::Bgr, 4);
        let row_bytes = width as usize * 4;
        for row in 0..height as usize {
            let source = &pixels[row * stride..row * stride + row_bytes];
            let destination = &mut next.pixels[row * row_bytes..(row + 1) * row_bytes];
            destination.copy_from_slice(source);
        }
        self.buffer = next;
        self.base.invalidate();
        true
    }

    fn emit(&self, event: Event) {
        if let Some(encoded) = gui::encode_window_event(self.handle, &event) {
            gui::enqueue_event(self.owner_pid, encoded);
        }
    }
}

impl Window for RemoteSurface {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        let bounds = self.base.bounds();
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            Color::BLACK,
        );
        device.blit_buffer(bounds.x, bounds.y, &self.buffer);
        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        self.emit(event);
        EventResult::Handled
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn set_bounds(&mut self, bounds: Rect) {
        let old = self.base.bounds();
        self.base.set_bounds(bounds);
        if old.width != bounds.width || old.height != bounds.height {
            self.emit(Event::Resize(ResizeEvent {
                width: bounds.width,
                height: bounds.height,
            }));
        }
    }

    fn set_focus(&mut self, focused: bool) {
        if self.base.has_focus() != focused {
            self.base.set_focus(focused);
            self.emit(Event::Focus(FocusEvent { gained: focused }));
        }
    }

    fn accepts_close_request(&self) -> bool {
        true
    }

    fn as_remote_surface_mut(&mut self) -> Option<&mut RemoteSurface> {
        Some(self)
    }
}
