//! [`Scroller`]: which rows of a terminal a view shows.

use std::ops::Range;

/// Where to scroll a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTo {
    /// The oldest row.
    Top,
    /// The last row, and keep following new output.
    Bottom,
    /// This row at the top of the view.
    Row(usize),
}

/// A window of `visible` rows over `total`, and whether it follows the
/// end as rows are added.
///
/// The window never starts past the last full page, so it is always
/// full once there are enough rows. It starts following the end, and
/// follows it again whenever it is scrolled back there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroller {
    total: usize,
    visible: usize,
    top: usize,
    follow: bool,
}

impl Scroller {
    pub fn new(visible: usize) -> Self {
        Self {
            total: 0,
            visible,
            top: 0,
            follow: true,
        }
    }

    /// Rows in all.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Rows the window shows at most.
    pub fn visible(&self) -> usize {
        self.visible
    }

    /// The first row shown.
    pub fn top(&self) -> usize {
        self.top
    }

    /// Whether the window follows the end.
    pub fn follows(&self) -> bool {
        self.follow
    }

    /// The highest `top` can be: the last full page.
    pub fn max_top(&self) -> usize {
        self.total.saturating_sub(self.visible)
    }

    /// The rows shown.
    pub fn range(&self) -> Range<usize> {
        self.top..self.top.saturating_add(self.visible).min(self.total)
    }

    /// Whether there are more rows than the window shows.
    pub fn scrolls(&self) -> bool {
        self.total > self.visible
    }

    /// Sets how many rows there are. A window that follows the end moves
    /// with it; any other keeps its top, within bounds.
    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        self.settle();
    }

    /// Sets how many rows the window shows.
    pub fn set_visible(&mut self, visible: usize) {
        self.visible = visible;
        self.settle();
    }

    /// Scrolls by `rows` (negative is up). Returns whether the window
    /// moved.
    pub fn scroll_by(&mut self, rows: isize) -> bool {
        let top = self.top.saturating_add_signed(rows).min(self.max_top());
        self.move_to(top)
    }

    /// Scrolls to `to`. Returns whether the window moved.
    pub fn scroll_to(&mut self, to: ScrollTo) -> bool {
        let top = match to {
            ScrollTo::Top => 0,
            ScrollTo::Bottom => self.max_top(),
            ScrollTo::Row(row) => row.min(self.max_top()),
        };
        self.move_to(top)
    }

    /// The scrollbar's thumb on a track `track` long: its offset and
    /// length, at least `min` long. `None` when nothing scrolls.
    pub fn thumb(&self, track: f32, min: f32) -> Option<(f32, f32)> {
        if !self.scrolls() || track <= 0.0 {
            return None;
        }
        let len = (track * self.visible as f32 / self.total as f32)
            .max(min)
            .min(track);
        let offset = if self.max_top() == 0 {
            0.0
        } else {
            (track - len) * self.top as f32 / self.max_top() as f32
        };
        Some((offset, len))
    }

    /// The top row that puts the thumb at `offset` on a track `track`
    /// long, with a thumb `len` long: for dragging it.
    pub fn top_at(&self, offset: f32, track: f32, len: f32) -> usize {
        let room = track - len;
        if room <= 0.0 {
            return 0;
        }
        let share = (offset / room).clamp(0.0, 1.0);
        (share * self.max_top() as f32).round() as usize
    }

    fn move_to(&mut self, top: usize) -> bool {
        let moved = top != self.top;
        self.top = top;
        self.follow = top == self.max_top();
        moved
    }

    fn settle(&mut self) {
        self.top = if self.follow {
            self.max_top()
        } else {
            self.top.min(self.max_top())
        };
    }
}
