//! Workspace state and layout math.
//!
//! Invariants (see CLAUDE.md):
//!   * Adding a pane does not resize existing panes.
//!   * Moving the viewport does not resize any pane.
//!   * Focus movement and viewport movement are separate operations.
//!   * Rendering is clipped to the viewport.

use crate::pane::Pane;

pub const STATUS_BAR_ROWS: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisiblePane {
    /// Index into `Workspace::panes`.
    pub idx: usize,
    /// First column inside the pane that is visible (inclusive).
    pub src_left: u16,
    /// Last column inside the pane that is visible (exclusive).
    pub src_right: u16,
    /// Column on the host screen where the clipped pane begins.
    pub dst_left: u16,
}

pub struct Workspace {
    pub panes: Vec<Pane>,
    pub focused: usize,
    /// Virtual horizontal offset, in cells. `0` is the leftmost pane.
    pub viewport_x: u32,
    /// Host terminal width (cells).
    pub screen_width: u16,
    /// Host terminal height (cells). Pane area is `screen_height - STATUS_BAR_ROWS`.
    pub screen_height: u16,
    /// Default width applied to newly spawned panes.
    pub default_width: u16,
}

impl Workspace {
    pub fn new(screen_width: u16, screen_height: u16, default_width: u16) -> Self {
        Self {
            panes: Vec::new(),
            focused: 0,
            viewport_x: 0,
            screen_width,
            screen_height,
            default_width,
        }
    }

    pub fn pane_height(&self) -> u16 {
        self.screen_height.saturating_sub(STATUS_BAR_ROWS).max(1)
    }

    /// Total virtual width occupied by all panes.
    pub fn total_width(&self) -> u32 {
        self.panes.iter().map(|p| p.width as u32).sum()
    }

    /// Virtual `[start, end)` range for the pane at `idx`.
    pub fn pane_x_range(&self, idx: usize) -> (u32, u32) {
        let start: u32 = self.panes[..idx].iter().map(|p| p.width as u32).sum();
        let end = start + self.panes[idx].width as u32;
        (start, end)
    }

    /// All panes that intersect the viewport, with src/dst clipping ranges.
    pub fn visible_panes(&self) -> Vec<VisiblePane> {
        let view_start = self.viewport_x;
        let view_end = view_start.saturating_add(self.screen_width as u32);
        let mut out = Vec::new();
        let mut x: u32 = 0;
        for (i, p) in self.panes.iter().enumerate() {
            let pane_start = x;
            let pane_end = pane_start + p.width as u32;
            x = pane_end;
            if pane_end <= view_start || pane_start >= view_end {
                continue;
            }
            let src_left = view_start.saturating_sub(pane_start) as u16;
            let src_right = (pane_end.min(view_end) - pane_start) as u16;
            let dst_left = pane_start.saturating_sub(view_start) as u16;
            out.push(VisiblePane {
                idx: i,
                src_left,
                src_right,
                dst_left,
            });
        }
        out
    }

    /// Center the focused pane in the viewport.
    pub fn center_focused(&mut self) {
        if self.panes.is_empty() {
            self.viewport_x = 0;
            return;
        }
        let (s, e) = self.pane_x_range(self.focused);
        let mid = (s + e) / 2;
        let half = (self.screen_width as u32) / 2;
        self.viewport_x = mid.saturating_sub(half);
    }

    /// Scroll viewport so that the focused pane is fully visible (if it fits).
    pub fn ensure_focused_visible(&mut self) {
        if self.panes.is_empty() {
            self.viewport_x = 0;
            return;
        }
        let (s, e) = self.pane_x_range(self.focused);
        let sw = self.screen_width as u32;
        let view_end = self.viewport_x + sw;
        if s < self.viewport_x {
            self.viewport_x = s;
        } else if e > view_end {
            // align right edge of pane to right edge of viewport, but never
            // go negative.
            self.viewport_x = e.saturating_sub(sw);
        }
    }

    pub fn focus_next(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        self.focused = (self.focused + 1).min(self.panes.len() - 1);
        self.ensure_focused_visible();
    }

    pub fn focus_prev(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        self.focused = self.focused.saturating_sub(1);
        self.ensure_focused_visible();
    }

    pub fn scroll_viewport(&mut self, delta: i32) {
        let new = self.viewport_x as i64 + delta as i64;
        let max = self
            .total_width()
            .saturating_sub(self.screen_width as u32) as i64;
        self.viewport_x = new.clamp(0, max.max(0)) as u32;
    }

    pub fn move_focused(&mut self, dir: i32) {
        if self.panes.is_empty() {
            return;
        }
        let i = self.focused;
        let j = i as i32 + dir;
        if j < 0 || j as usize >= self.panes.len() {
            return;
        }
        self.panes.swap(i, j as usize);
        self.focused = j as usize;
        self.ensure_focused_visible();
    }

    /// Insert a new pane immediately after the focused index. The new pane
    /// becomes focused. Existing panes keep their widths.
    pub fn push_pane(&mut self, pane: Pane) {
        if self.panes.is_empty() {
            self.panes.push(pane);
            self.focused = 0;
        } else {
            let at = self.focused + 1;
            self.panes.insert(at, pane);
            self.focused = at;
        }
        self.center_focused();
    }

    /// Remove the focused pane. Returns the removed pane so the caller can
    /// shut down its PTY. Adjusts focus to the previous index.
    pub fn remove_focused(&mut self) -> Option<Pane> {
        if self.panes.is_empty() {
            return None;
        }
        let p = self.panes.remove(self.focused);
        if self.focused >= self.panes.len() && self.focused > 0 {
            self.focused -= 1;
        }
        if !self.panes.is_empty() {
            self.ensure_focused_visible();
        } else {
            self.viewport_x = 0;
        }
        Some(p)
    }

    pub fn resize_focused(&mut self, delta: i32) {
        if self.panes.is_empty() {
            return;
        }
        let h = self.pane_height();
        let p = &mut self.panes[self.focused];
        let new = (p.width as i32 + delta).clamp(20, 500) as u16;
        if new != p.width {
            p.set_width(new, h);
        }
        self.ensure_focused_visible();
    }

    /// Called when the host terminal is resized. Per the invariants, pane
    /// widths are preserved; only heights change.
    pub fn handle_host_resize(&mut self, cols: u16, rows: u16) {
        self.screen_width = cols;
        self.screen_height = rows;
        let h = self.pane_height();
        for p in &mut self.panes {
            p.set_height(h);
        }
        self.ensure_focused_visible();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::Pane;

    fn ws_with_widths(widths: &[u16], screen_w: u16) -> Workspace {
        let mut w = Workspace::new(screen_w, 24, 120);
        for &width in widths {
            w.panes.push(Pane::fake(width, 23));
        }
        w
    }

    #[test]
    fn visible_panes_single_fully_visible() {
        let mut w = ws_with_widths(&[80], 100);
        w.viewport_x = 0;
        let v = w.visible_panes();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].src_left, 0);
        assert_eq!(v[0].src_right, 80);
        assert_eq!(v[0].dst_left, 0);
    }

    #[test]
    fn visible_panes_clipped_on_left() {
        // screen_w=100, panes 120+120+120, viewport=60 → see col 60..120 of pane0 (60 cols),
        // and cols 0..40 of pane1.
        let mut w = ws_with_widths(&[120, 120, 120], 100);
        w.viewport_x = 60;
        let v = w.visible_panes();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].idx, 0);
        assert_eq!(v[0].src_left, 60);
        assert_eq!(v[0].src_right, 120);
        assert_eq!(v[0].dst_left, 0);
        assert_eq!(v[1].idx, 1);
        assert_eq!(v[1].src_left, 0);
        assert_eq!(v[1].src_right, 40);
        assert_eq!(v[1].dst_left, 60);
    }

    #[test]
    fn visible_panes_skip_offscreen() {
        let mut w = ws_with_widths(&[120, 120, 120], 100);
        w.viewport_x = 250;
        let v = w.visible_panes();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].idx, 2);
        assert_eq!(v[0].src_left, 10);
        assert_eq!(v[0].dst_left, 0);
    }

    #[test]
    fn focus_navigation_clamped() {
        let mut w = ws_with_widths(&[100, 100, 100], 80);
        w.focused = 0;
        w.focus_prev();
        assert_eq!(w.focused, 0);
        w.focus_next();
        w.focus_next();
        w.focus_next();
        assert_eq!(w.focused, 2);
    }

    #[test]
    fn move_focused_swaps_and_follows() {
        let mut w = ws_with_widths(&[100, 100, 100], 80);
        w.panes[0].name = "a".into();
        w.panes[1].name = "b".into();
        w.panes[2].name = "c".into();
        w.focused = 1;
        w.move_focused(1);
        assert_eq!(w.focused, 2);
        let names: Vec<_> = w.panes.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, vec!["a", "c", "b"]);
    }

    #[test]
    fn host_resize_preserves_widths() {
        let mut w = ws_with_widths(&[100, 140, 80], 200);
        w.handle_host_resize(50, 30);
        assert_eq!(w.screen_width, 50);
        let widths: Vec<u16> = w.panes.iter().map(|p| p.width).collect();
        assert_eq!(widths, vec![100, 140, 80]);
    }

    #[test]
    fn center_focused_centers() {
        let mut w = ws_with_widths(&[100, 100, 100], 60);
        w.focused = 1;
        w.center_focused();
        // pane 1 spans [100, 200), midpoint 150, half-screen 30 → viewport_x = 120.
        assert_eq!(w.viewport_x, 120);
    }
}
