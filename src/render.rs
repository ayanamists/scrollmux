//! Frame rendering: walk the visible-pane list, copy each pane's vt100 screen
//! into the host terminal frame with viewport clipping, then draw a one-line
//! status bar.

use std::io::Write;

use crossterm::style::{
    Attribute, Color as CColor, Print, ResetColor, SetAttribute, SetBackgroundColor,
    SetForegroundColor,
};
use crossterm::{cursor, queue, terminal};
use unicode_width::UnicodeWidthChar;

use crate::workspace::{VisiblePane, Workspace, STATUS_BAR_ROWS};

#[derive(Default)]
pub struct Renderer {
    last: Option<Frame>,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render<W: Write>(&mut self, w: &mut W, ws: &Workspace) -> std::io::Result<()> {
        queue!(w, cursor::Hide, ResetColor, SetAttribute(Attribute::Reset))?;

        let visible = ws.visible_panes();
        let frame = Frame::from_workspace(ws, &visible);
        let needs_full_redraw = self
            .last
            .as_ref()
            .is_none_or(|last| last.width != frame.width || last.height != frame.height);

        if needs_full_redraw {
            queue!(w, terminal::Clear(terminal::ClearType::All))?;
        }

        match &self.last {
            Some(last) if !needs_full_redraw => {
                for row in 0..frame.height {
                    if frame.row(row) != last.row(row) {
                        draw_row(w, row as u16, frame.row(row))?;
                    }
                }
            }
            _ => {
                for row in 0..frame.height {
                    draw_row(w, row as u16, frame.row(row))?;
                }
            }
        }

        draw_cursor(w, ws, &visible)?;

        self.last = Some(frame);
        w.flush()?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Frame {
    width: usize,
    height: usize,
    cells: Vec<FrameCell>,
}

impl Frame {
    fn from_workspace(ws: &Workspace, visible: &[VisiblePane]) -> Self {
        let width = ws.screen_width as usize;
        let height = ws.screen_height.max(1) as usize;
        let mut frame = Self {
            width,
            height,
            cells: vec![FrameCell::blank(); width * height],
        };

        frame.copy_visible_panes(ws, visible);
        frame.draw_status_bar(ws);
        frame
    }

    fn copy_visible_panes(&mut self, ws: &Workspace, visible: &[VisiblePane]) {
        let pane_rows = ws.pane_height();
        for vp in visible {
            let pane = &ws.panes[vp.idx];
            let parser = match pane.parser.lock() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let screen = parser.screen();
            let (rows, _cols) = screen.size();
            let draw_rows = rows.min(pane_rows);
            for row in 0..draw_rows {
                let row_idx = row as usize;
                if row_idx >= self.height {
                    break;
                }
                for col in vp.src_left..vp.src_right {
                    let dst_col = vp.dst_left + (col - vp.src_left);
                    let dst_idx = dst_col as usize;
                    if dst_idx >= self.width {
                        continue;
                    }

                    self[(row_idx, dst_idx)] = match screen.cell(row, col) {
                        Some(cell) => FrameCell::from_vt_cell(cell, col > vp.src_left),
                        None => FrameCell::blank(),
                    };
                }
            }
        }
    }

    fn draw_status_bar(&mut self, ws: &Workspace) {
        let row = ws.screen_height.saturating_sub(STATUS_BAR_ROWS) as usize;
        if row >= self.height {
            return;
        }

        let attrs = Attrs {
            fg: Some(CColor::White),
            bg: Some(CColor::DarkGrey),
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        };
        for cell in self.row_mut(row) {
            *cell = FrameCell::space(attrs);
        }

        let mut line = String::new();
        build_status_left(&mut line, ws);

        let max = self.width;
        let used = self.put_str(row, 0, &line, attrs);

        let total = ws.total_width();
        let right = format!(" view {} / {} ", ws.viewport_x, total);
        let right_len = display_width(&right);
        if used + right_len < max {
            self.put_str(row, max - right_len, &right, attrs);
        }
    }

    fn put_str(&mut self, row: usize, col: usize, text: &str, attrs: Attrs) -> usize {
        if row >= self.height {
            return col;
        }

        let mut idx = col;
        for ch in text.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width == 0 {
                continue;
            }
            if idx + width > self.width {
                break;
            }

            self[(row, idx)] = FrameCell::char(ch, attrs);
            for continuation in 1..width {
                self[(row, idx + continuation)] = FrameCell::wide_continuation();
            }
            idx += width;
        }
        idx
    }

    fn row(&self, row: usize) -> &[FrameCell] {
        let start = self.row_start(row);
        &self.cells[start..start + self.width]
    }

    fn row_mut(&mut self, row: usize) -> &mut [FrameCell] {
        let start = self.row_start(row);
        &mut self.cells[start..start + self.width]
    }

    fn row_start(&self, row: usize) -> usize {
        row * self.width
    }
}

impl std::ops::Index<(usize, usize)> for Frame {
    type Output = FrameCell;

    fn index(&self, (row, col): (usize, usize)) -> &Self::Output {
        &self.cells[self.row_start(row) + col]
    }
}

impl std::ops::IndexMut<(usize, usize)> for Frame {
    fn index_mut(&mut self, (row, col): (usize, usize)) -> &mut Self::Output {
        let idx = self.row_start(row) + col;
        &mut self.cells[idx]
    }
}

fn build_status_left(line: &mut String, ws: &Workspace) {
    line.push_str(" scrollmux  ");
    for (i, p) in ws.panes.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        if i == ws.focused {
            line.push('[');
        } else {
            line.push(' ');
        }
        if p.name.is_empty() {
            line.push_str(&format!("{}", i));
        } else {
            line.push_str(&p.name);
        }
        if i == ws.focused {
            line.push(']');
        } else {
            line.push(' ');
        }
    }
}

fn draw_row<W: Write>(w: &mut W, row: u16, cells: &[FrameCell]) -> std::io::Result<()> {
    queue!(
        w,
        cursor::MoveTo(0, row),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
    let mut last_attrs = Attrs::default();
    for cell in cells {
        if cell.skip {
            continue;
        }
        if cell.attrs != last_attrs {
            apply_attrs(w, &cell.attrs, &last_attrs)?;
            last_attrs = cell.attrs;
        }
        cell.text.queue_print(w)?;
    }
    queue!(w, ResetColor, SetAttribute(Attribute::Reset))?;
    Ok(())
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn draw_cursor<W: Write>(
    w: &mut W,
    ws: &Workspace,
    visible: &[VisiblePane],
) -> std::io::Result<()> {
    // Show the cursor at the focused pane's reported position, if it lies
    // within the visible viewport region of that pane.
    if let Some(vp) = visible.iter().find(|v| v.idx == ws.focused) {
        let pane = &ws.panes[vp.idx];
        if let Ok(parser) = pane.parser.lock() {
            let screen = parser.screen();
            let (crow, ccol) = screen.cursor_position();
            let row_in_view = crow < ws.pane_height();
            let col_in_view = ccol >= vp.src_left && ccol < vp.src_right;
            if row_in_view && col_in_view && !screen.hide_cursor() {
                let dst_col = vp.dst_left + (ccol - vp.src_left);
                queue!(w, cursor::MoveTo(dst_col, crow), cursor::Show)?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
struct FrameCell {
    text: CellText,
    attrs: Attrs,
    skip: bool,
}

impl FrameCell {
    fn blank() -> Self {
        Self::space(Attrs::default())
    }

    fn space(attrs: Attrs) -> Self {
        Self {
            text: CellText::Space,
            attrs,
            skip: false,
        }
    }

    fn char(ch: char, attrs: Attrs) -> Self {
        Self {
            text: if ch == ' ' {
                CellText::Space
            } else {
                CellText::Char(ch)
            },
            attrs,
            skip: false,
        }
    }

    fn text(text: &str, attrs: Attrs) -> Self {
        Self {
            text: CellText::from_str(text),
            attrs,
            skip: false,
        }
    }

    fn wide_continuation() -> Self {
        Self {
            text: CellText::Space,
            attrs: Attrs::default(),
            skip: true,
        }
    }

    fn from_vt_cell(cell: &vt100::Cell, can_skip_wide_continuation: bool) -> Self {
        let attrs = Attrs::from_cell(cell);
        let text = cell.contents();
        if !text.is_empty() {
            return Self::text(&text, attrs);
        }
        if cell.is_wide_continuation() && can_skip_wide_continuation {
            Self::wide_continuation()
        } else {
            Self::space(attrs)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CellText {
    Space,
    Char(char),
    Text(String),
}

impl CellText {
    fn from_str(text: &str) -> Self {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(' '), None) => CellText::Space,
            (Some(ch), None) => CellText::Char(ch),
            _ => CellText::Text(text.to_string()),
        }
    }

    fn queue_print<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        match self {
            CellText::Space => queue!(w, Print(" "))?,
            CellText::Char(ch) => queue!(w, Print(*ch))?,
            CellText::Text(s) => queue!(w, Print(s))?,
        }
        Ok(())
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct Attrs {
    fg: Option<CColor>,
    bg: Option<CColor>,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Attrs {
    fn from_cell(cell: &vt100::Cell) -> Self {
        Self {
            fg: cvt_color(cell.fgcolor()),
            bg: cvt_color(cell.bgcolor()),
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }
}

fn apply_attrs<W: Write>(w: &mut W, attrs: &Attrs, prev: &Attrs) -> std::io::Result<()> {
    // Whenever any attribute flag toggles off we need a full reset, because
    // crossterm doesn't expose per-attribute "off" toggles uniformly.
    let needs_reset = (prev.bold && !attrs.bold)
        || (prev.italic && !attrs.italic)
        || (prev.underline && !attrs.underline)
        || (prev.inverse && !attrs.inverse);
    if needs_reset {
        queue!(w, ResetColor, SetAttribute(Attribute::Reset))?;
    }
    if let Some(c) = attrs.fg {
        queue!(w, SetForegroundColor(c))?;
    } else if needs_reset || prev.fg.is_some() {
        queue!(w, SetForegroundColor(CColor::Reset))?;
    }
    if let Some(c) = attrs.bg {
        queue!(w, SetBackgroundColor(c))?;
    } else if needs_reset || prev.bg.is_some() {
        queue!(w, SetBackgroundColor(CColor::Reset))?;
    }
    if attrs.bold {
        queue!(w, SetAttribute(Attribute::Bold))?;
    }
    if attrs.italic {
        queue!(w, SetAttribute(Attribute::Italic))?;
    }
    if attrs.underline {
        queue!(w, SetAttribute(Attribute::Underlined))?;
    }
    if attrs.inverse {
        queue!(w, SetAttribute(Attribute::Reverse))?;
    }
    Ok(())
}

fn cvt_color(c: vt100::Color) -> Option<CColor> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(n) => Some(CColor::AnsiValue(n)),
        vt100::Color::Rgb(r, g, b) => Some(CColor::Rgb { r, g, b }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::Pane;

    #[test]
    fn stable_frame_does_not_clear_all_twice() {
        let mut ws = Workspace::new(10, 4, 10);
        ws.panes.push(Pane::fake(10, 3));

        let mut renderer = Renderer::new();
        let mut out = Vec::new();
        renderer.render(&mut out, &ws).unwrap();
        let split = out.len();
        renderer.render(&mut out, &ws).unwrap();

        let first = &out[..split];
        let second = &out[split..];
        assert!(first.windows(b"\x1b[2J".len()).any(|w| w == b"\x1b[2J"));
        assert!(!second.windows(b"\x1b[2J".len()).any(|w| w == b"\x1b[2J"));
    }

    #[test]
    fn status_bar_accounts_for_wide_pane_names() {
        let mut ws = Workspace::new(32, 4, 10);
        ws.panes.push(Pane::fake(10, 3));
        ws.panes[0].name = "中".into();

        let frame = Frame::from_workspace(&ws, &ws.visible_panes());
        let row = frame.row(ws.screen_height.saturating_sub(STATUS_BAR_ROWS) as usize);
        let wide_col = row
            .iter()
            .position(|cell| cell.text == CellText::Char('中'))
            .unwrap();
        assert!(row[wide_col + 1].skip);

        let right = format!(" view {} / {} ", ws.viewport_x, ws.total_width());
        let right_start = frame.width - display_width(&right);
        assert_eq!(row[right_start + 1].text, CellText::Char('v'));
    }
}
