use alloc::vec::Vec;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WidgetId(pub u64);

pub struct FocusManager {
    order: Vec<WidgetId>,
    focused: Option<usize>,
    restore: Option<WidgetId>,
    default: Option<WidgetId>,
    cancel: Option<WidgetId>,
}

impl FocusManager {
    pub const fn new() -> Self {
        Self {
            order: Vec::new(),
            focused: None,
            restore: None,
            default: None,
            cancel: None,
        }
    }

    pub fn begin_layout(&mut self) {
        let previous = self.focused();
        self.order.clear();
        self.focused = None;
        self.restore = previous;
    }

    pub fn register(&mut self, id: WidgetId, enabled: bool) {
        if !enabled || self.order.contains(&id) {
            return;
        }
        self.order.push(id);
        if self.restore == Some(id) {
            self.focused = Some(self.order.len() - 1);
            self.restore = None;
        } else if self.focused.is_none() {
            self.focused = Some(0);
        }
    }

    pub fn focused(&self) -> Option<WidgetId> {
        self.focused.and_then(|index| self.order.get(index).copied())
    }

    pub fn focus(&mut self, id: WidgetId) -> bool {
        let Some(index) = self.order.iter().position(|candidate| *candidate == id) else {
            return false;
        };
        let changed = self.focused != Some(index);
        self.focused = Some(index);
        changed
    }

    pub fn advance(&mut self, reverse: bool) -> Option<WidgetId> {
        if self.order.is_empty() {
            self.focused = None;
            return None;
        }
        let current = self.focused.unwrap_or(0);
        self.focused = Some(if reverse {
            current.checked_sub(1).unwrap_or(self.order.len() - 1)
        } else {
            (current + 1) % self.order.len()
        });
        self.focused()
    }

    pub fn set_default(&mut self, id: Option<WidgetId>) {
        self.default = id;
    }

    pub fn set_cancel(&mut self, id: Option<WidgetId>) {
        self.cancel = id;
    }

    pub const fn default(&self) -> Option<WidgetId> {
        self.default
    }

    pub const fn cancel(&self) -> Option<WidgetId> {
        self.cancel
    }

    pub fn clear(&mut self) {
        self.order.clear();
        self.focused = None;
        self.restore = None;
        self.default = None;
        self.cancel = None;
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{FocusManager, WidgetId};

    #[test]
    fn traversal_wraps_and_skips_disabled_widgets() {
        let mut focus = FocusManager::new();
        focus.register(WidgetId(1), true);
        focus.register(WidgetId(2), false);
        focus.register(WidgetId(3), true);
        assert_eq!(focus.focused(), Some(WidgetId(1)));
        assert_eq!(focus.advance(false), Some(WidgetId(3)));
        assert_eq!(focus.advance(false), Some(WidgetId(1)));
        assert_eq!(focus.advance(true), Some(WidgetId(3)));
    }
}
