//! Window implementations

pub mod base;
pub mod container;
pub mod text;
pub mod terminal;
pub mod frame;
pub mod desktop;

pub use base::WindowBase;
pub use container::ContainerWindow;
pub use text::TextWindow;
pub use terminal::TerminalWindow;
pub use frame::FrameWindow;
pub use desktop::DesktopWindow;