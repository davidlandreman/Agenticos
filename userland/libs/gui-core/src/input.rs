#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseButtons {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub key: u32,
    pub character: char,
    pub modifiers: Modifiers,
    pub pressed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Move,
    Down,
    Up,
    Scroll { delta_x: i32, delta_y: i32 },
    Cancel,
}

/// Mouse-pointer image requested by a control at the current hover point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorIcon {
    #[default]
    Arrow,
    Wait,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerInput {
    pub x: i32,
    pub y: i32,
    pub buttons: MouseButtons,
    pub modifiers: Modifiers,
    pub kind: PointerKind,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlInput {
    Key(KeyInput),
    Pointer(PointerInput),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlResponse<A> {
    pub consumed: bool,
    pub repaint: bool,
    pub action: Option<A>,
}

impl<A> ControlResponse<A> {
    pub const fn ignored() -> Self {
        Self {
            consumed: false,
            repaint: false,
            action: None,
        }
    }

    pub const fn consumed(repaint: bool, action: Option<A>) -> Self {
        Self {
            consumed: true,
            repaint,
            action,
        }
    }
}
