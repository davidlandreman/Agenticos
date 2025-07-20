//! Base window implementation with common functionality

use alloc::vec::Vec;
use crate::window::{Window, WindowId, Rect, Event, EventResult};

/// Base window structure that provides common functionality
pub struct WindowBase {
    /// Unique window ID
    id: WindowId,
    /// Window bounds relative to parent
    bounds: Rect,
    /// Whether the window is visible
    visible: bool,
    /// Parent window ID
    parent: Option<WindowId>,
    /// Child window IDs
    children: Vec<WindowId>,
    /// Whether this window needs repainting
    needs_repaint: bool,
    /// Whether this window can receive focus
    can_focus: bool,
    /// Whether this window currently has focus
    has_focus: bool,
}

impl WindowBase {
    /// Create a new window base
    pub fn new(bounds: Rect) -> Self {
        WindowBase {
            id: WindowId::new(),
            bounds,
            visible: true,
            parent: None,
            children: Vec::new(),
            needs_repaint: true,
            can_focus: false,
            has_focus: false,
        }
    }
    
    /// Set the parent window
    pub fn set_parent(&mut self, parent: Option<WindowId>) {
        self.parent = parent;
    }
    
    /// Add a child window
    pub fn add_child(&mut self, child: WindowId) {
        if !self.children.contains(&child) {
            self.children.push(child);
        }
    }
    
    /// Remove a child window
    pub fn remove_child(&mut self, child: WindowId) {
        self.children.retain(|&id| id != child);
    }
    
    /// Set whether this window can receive focus
    pub fn set_can_focus(&mut self, can_focus: bool) {
        self.can_focus = can_focus;
    }
    
    /// Update the window bounds
    pub fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
        self.needs_repaint = true;
    }
    
    /// Set visibility
    pub fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.needs_repaint = true;
        }
    }
}

// Implement getters for the Window trait
impl WindowBase {
    pub fn id(&self) -> WindowId { self.id }
    pub fn bounds(&self) -> Rect { self.bounds }
    pub fn visible(&self) -> bool { self.visible }
    pub fn parent(&self) -> Option<WindowId> { self.parent }
    pub fn children(&self) -> &[WindowId] { &self.children }
    pub fn needs_repaint(&self) -> bool { self.needs_repaint }
    pub fn can_focus(&self) -> bool { self.can_focus }
    pub fn has_focus(&self) -> bool { self.has_focus }
    
    pub fn invalidate(&mut self) { self.needs_repaint = true; }
    pub fn clear_needs_repaint(&mut self) { self.needs_repaint = false; }
    
    pub fn set_focus(&mut self, focused: bool) { 
        self.has_focus = focused;
        self.needs_repaint = true;
    }
}