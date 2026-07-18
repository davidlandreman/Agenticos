#![no_std]

extern crate alloc;

pub mod geometry;
pub mod input;
pub mod scroll;
pub mod text_edit;

pub use geometry::Rect;
pub use input::{
    ControlInput, ControlResponse, KeyInput, Modifiers, MouseButtons, PointerInput, PointerKind,
};
pub use scroll::{
    layout_scrollbars, Axis, ScrollState, ScrollbarGeometry, ScrollbarPolicy, ScrollbarsLayout,
};
pub use text_edit::TextEdit;
