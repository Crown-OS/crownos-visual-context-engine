/// An axis-aligned rectangle in frame pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    pub const fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    pub const fn area(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then(|| Self::new(x, y, right - x, bottom - y))
    }

    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Grows each side by the margins, clamped to `bounds`.
    pub fn expanded_within(self, horizontal: u32, vertical: u32, bounds: Self) -> Self {
        let x = self.x.saturating_sub(horizontal).max(bounds.x);
        let y = self.y.saturating_sub(vertical).max(bounds.y);
        let right = self.right().saturating_add(horizontal).min(bounds.right());
        let bottom = self.bottom().saturating_add(vertical).min(bounds.bottom());
        Self::new(x, y, right.saturating_sub(x), bottom.saturating_sub(y))
    }

    /// The compositor sends signed damage; anything outside the frame is cut away.
    pub fn from_signed_within(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        bounds: Self,
    ) -> Option<Self> {
        let left = i64::from(x).max(0);
        let top = i64::from(y).max(0);
        let right = (i64::from(x) + i64::from(width)).min(i64::from(bounds.right()));
        let bottom = (i64::from(y) + i64::from(height)).min(i64::from(bounds.bottom()));
        let to_u32 = |value: i64| u32::try_from(value).ok();
        let rect = Self::new(
            to_u32(left)?,
            to_u32(top)?,
            to_u32(right - left)?,
            to_u32(bottom - top)?,
        );
        (!rect.is_empty()).then_some(rect)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn intersection_union_and_margins() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersection(b), Some(Rect::new(5, 5, 5, 5)));
        assert_eq!(a.intersection(Rect::new(10, 0, 5, 5)), None);
        assert_eq!(a.union(b), Rect::new(0, 0, 15, 15));
        let bounds = Rect::new(0, 0, 12, 12);
        assert_eq!(b.expanded_within(4, 2, bounds), Rect::new(1, 3, 11, 9));
    }

    #[test]
    fn signed_damage_is_clamped_to_the_frame() {
        let bounds = Rect::new(0, 0, 100, 50);
        assert_eq!(
            Rect::from_signed_within(-10, 40, 200, 30, bounds),
            Some(Rect::new(0, 40, 100, 10))
        );
        assert_eq!(Rect::from_signed_within(120, 0, 5, 5, bounds), None);
    }
}
