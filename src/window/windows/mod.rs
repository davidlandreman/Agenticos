//! Window implementations

pub mod base;
pub mod container;
pub mod text;
pub mod terminal;
pub mod frame;
pub mod desktop;
pub mod label;
pub mod button;
pub mod text_input;
pub mod list;
pub mod multi_column_list;
pub mod menu;
pub mod taskbar;
pub mod text_editor;
pub mod menu_bar;
pub mod menu_bar_popup;
pub mod dialog;
pub mod layout;

pub use container::ContainerWindow;
pub use terminal::TerminalWindow;
pub use frame::FrameWindow;
pub use desktop::DesktopWindow;
pub use label::Label;
pub use button::Button;
pub use text_input::TextInput;
pub use multi_column_list::{MultiColumnList, Column};
pub use menu::MenuWindow;
pub use taskbar::TaskbarWindow;
pub use text_editor::TextEditor;
pub use menu_bar::{MenuBar, MenuItemDef, PendingPopup, MENU_BAR_HEIGHT};
pub use menu_bar_popup::MenuBarPopup;
pub use layout::{HBox, Padding, SizeHint, Spacer, VBox};