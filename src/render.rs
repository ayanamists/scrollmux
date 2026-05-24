//! Frame rendering: walk the visible-pane list, copy each pane's vt100 screen
//! into the host terminal frame with viewport clipping, then draw a one-line
//! status bar.

use std::io::Write;

use crossterm::style::{
    Attribute, Color as CColor, Print, ResetColor, SetAttribute, SetBackgroundColor,
    SetForegroundColor,
};
use crossterm::{cursor, queue, terminal};

use crate::workspace::{Workspace, STATUS_BAR_ROWS};

pub fn render<W: Write>(w: &mut W, ws: &Workspace) -> std::io::Result<()> {
    queue!(w, cursor::Hide, ResetColor)?;
    // Clear so trailing whitespace from the previous frame doesn't bleed into
    // the next when a column shrinks or disappears.
    queue!(w, terminal::Clear(terminal::ClearType::All))?;

    let visible = ws.visible_panes();
    let pane_rows = ws.pane_height();

    for vp in &visible {
        let pane = &ws.panes[vp.idx];
        let parser = match pane.parser.lock() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let screen = parser.screen();
        let (rows, _cols) = screen.size();
        let draw_rows = rows.min(pane_rows);
        for row in 0..draw_rows {
            queue!(w, cursor::MoveTo(vp.dst_left, row))?;
            let mut last_attrs = Attrs::default();
            queue!(w, ResetColor)?;
            for col in vp.src_left..vp.src_right {
                if let Some(cell) = screen.cell(row, col) {
                    let attrs = Attrs::from_cell(cell);
                    if attrs != last_attrs {
                        apply_attrs(w, &attrs, &last_attrs)?;
                        last_attrs = attrs;
                    }
                    let s = cell.contents();
                    if s.is_empty() {
                        // Either a blank cell or the trailing half of a wide
                        // glyph — for the trailing-half case the leading cell
                        // already emitted both columns of output, so skip.
                        if !cell.is_wide_continuation() {
                            queue!(w, Print(" "))?;
                        }
                    } else {
                        queue!(w, Print(s))?;
                    }
                } else {
                    queue!(w, Print(" "))?;
                }
            }
            queue!(w, ResetColor)?;
        }
    }

    draw_status_bar(w, ws)?;

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

    w.flush()?;
    Ok(())
}

fn draw_status_bar<W: Write>(w: &mut W, ws: &Workspace) -> std::io::Result<()> {
    let row = ws.screen_height.saturating_sub(STATUS_BAR_ROWS);
    queue!(
        w,
        cursor::MoveTo(0, row),
        SetBackgroundColor(CColor::DarkGrey),
        SetForegroundColor(CColor::White),
        terminal::Clear(terminal::ClearType::CurrentLine)
    )?;
    let mut line = String::new();
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
    // Truncate to screen_width to avoid wrapping into the next line.
    let max = ws.screen_width as usize;
    if line.chars().count() > max {
        line = line.chars().take(max).collect();
    }
    queue!(w, Print(&line))?;
    // Right-aligned viewport indicator.
    let total = ws.total_width();
    let right = format!(" view {} / {} ", ws.viewport_x, total);
    let used = line.chars().count();
    let right_len = right.chars().count();
    if used + right_len < max {
        let pad = max - used - right_len;
        let padding = " ".repeat(pad);
        queue!(w, Print(padding), Print(&right))?;
    }
    queue!(w, ResetColor)?;
    Ok(())
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
