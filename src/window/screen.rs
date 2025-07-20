//! Screen abstraction for virtual displays

use alloc::boxed::Box;
use super::{Window, WindowId, ScreenId};

/// Mode of operation for a screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenMode {
    /// Text-based console mode
    Text,
    /// Graphical user interface mode
    Gui,
}

/// A screen represents a virtual display that can contain windows
pub struct Screen {
    /// Unique identifier for this screen
    pub id: ScreenId,
    /// Root window for this screen
    pub root_window: Option<WindowId>,
    /// Operating mode of this screen
    pub mode: ScreenMode,
}

impl Screen {
    /// Create a new screen with the specified mode
    pub fn new(mode: ScreenMode) -> Self {
        Screen {
            id: ScreenId::new(),
            root_window: None,
            mode,
        }
    }
    
    /// Set the root window for this screen
    pub fn set_root_window(&mut self, window_id: WindowId) {
        self.root_window = Some(window_id);
    }
}