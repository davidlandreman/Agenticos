//! Passive layout containers.
//!
//! These windows compute child bounds from their own bounds plus per-
//! child sizing hints and write the result back through the
//! `WindowManager`. Resizing a layout container automatically reflows
//! its children — see `WindowManager::with_window_mut` and the
//! "Layout container resize flow" section of the U2 plan.
//!
//! Layout containers are passive: they do not paint anything of their
//! own. Children are positioned relative to the container's coordinate
//! system, the same way every other parent works in the window tree.

pub mod hbox;
pub mod padding;
pub mod spacer;
pub mod vbox;

pub use hbox::HBox;
pub use padding::Padding;
pub use spacer::{SizeHint, Spacer};
pub use vbox::VBox;
