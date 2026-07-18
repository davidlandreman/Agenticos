#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(self) -> i32 {
        self.x.saturating_add(self.w as i32)
    }

    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.h as i32)
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inset(self, amount: u32) -> Self {
        let twice = amount.saturating_mul(2);
        Self::new(
            self.x.saturating_add(amount as i32),
            self.y.saturating_add(amount as i32),
            self.w.saturating_sub(twice),
            self.h.saturating_sub(twice),
        )
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then_some(Self::new(
            left,
            top,
            (right - left) as u32,
            (bottom - top) as u32,
        ))
    }
}
