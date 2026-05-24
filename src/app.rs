//! Glue: own the workspace, run the event loop, route input, drive the
//! renderer, and restore the host terminal on exit.

use std::io::{self, Write};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::{cursor, execute, terminal};

use crate::input::{self, Action};
use crate::pane::Pane;
use crate::render;
use crate::workspace::Workspace;

const TICK_MS: u64 = 16;
const SCROLL_STEP: i32 = 20;
const WIDTH_STEP: i32 = 10;

pub struct App {
    ws: Workspace,
    next_id: u32,
    quitting: bool,
    pending_close: Vec<Pane>,
}

impl App {
    pub fn new(screen_w: u16, screen_h: u16, default_width: u16) -> Self {
        Self {
            ws: Workspace::new(screen_w, screen_h, default_width),
            next_id: 1,
            quitting: false,
            pending_close: Vec::new(),
        }
    }

    pub fn spawn_default(&mut self) -> io::Result<()> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let name = format!("col{}", self.next_id);
        let pane = Pane::spawn(
            name,
            &[shell],
            self.ws.default_width,
            self.ws.pane_height(),
        )?;
        self.next_id += 1;
        self.ws.push_pane(pane);
        Ok(())
    }

    pub fn run<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        // Spawn an initial shell so the user sees something useful immediately.
        self.spawn_default()?;

        render::render(out, &self.ws)?;

        while !self.quitting && !self.ws.panes.is_empty() {
            // Block up to TICK_MS for an input event. If nothing arrives we
            // still come back around to repaint dirty PTY output.
            if event::poll(Duration::from_millis(TICK_MS))? {
                self.handle_event(event::read()?)?;
            }

            self.reap_pending();
            if self.ws.panes.is_empty() {
                break;
            }

            if self.any_dirty() || self.force_redraw() {
                render::render(out, &self.ws)?;
            }
        }

        // Best-effort: kill any leftover children when quitting.
        for p in self.ws.panes.drain(..) {
            p.shutdown();
        }
        for p in self.pending_close.drain(..) {
            p.shutdown();
        }
        Ok(())
    }

    fn any_dirty(&self) -> bool {
        self.ws
            .panes
            .iter()
            .any(|p| p.dirty.swap(false, Ordering::AcqRel))
    }

    fn force_redraw(&self) -> bool {
        // Currently we only force a redraw immediately after input events,
        // which are already handled inline. Hook for future use.
        false
    }

    fn handle_event(&mut self, ev: Event) -> io::Result<()> {
        match ev {
            Event::Key(k) => {
                if let Some(action) = input::map(k) {
                    self.handle_action(action)?;
                }
            }
            Event::Resize(cols, rows) => {
                self.ws.handle_host_resize(cols, rows);
                for p in &self.ws.panes {
                    p.dirty.store(true, Ordering::Release);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_action(&mut self, action: Action) -> io::Result<()> {
        match action {
            Action::Quit => self.quitting = true,
            Action::FocusPrev => {
                self.ws.focus_prev();
                self.mark_all_dirty();
            }
            Action::FocusNext => {
                self.ws.focus_next();
                self.mark_all_dirty();
            }
            Action::MoveLeft => {
                self.ws.move_focused(-1);
                self.mark_all_dirty();
            }
            Action::MoveRight => {
                self.ws.move_focused(1);
                self.mark_all_dirty();
            }
            Action::NewPane => {
                self.spawn_default()?;
                self.mark_all_dirty();
            }
            Action::CloseFocused => {
                if let Some(p) = self.ws.remove_focused() {
                    self.pending_close.push(p);
                }
                self.mark_all_dirty();
            }
            Action::Center => {
                self.ws.center_focused();
                self.mark_all_dirty();
            }
            Action::ScrollLeft => {
                self.ws.scroll_viewport(-SCROLL_STEP);
                self.mark_all_dirty();
            }
            Action::ScrollRight => {
                self.ws.scroll_viewport(SCROLL_STEP);
                self.mark_all_dirty();
            }
            Action::GrowWidth => {
                self.ws.resize_focused(WIDTH_STEP);
                self.mark_all_dirty();
            }
            Action::ShrinkWidth => {
                self.ws.resize_focused(-WIDTH_STEP);
                self.mark_all_dirty();
            }
            Action::Input(bytes) => {
                if let Some(p) = self.ws.panes.get_mut(self.ws.focused) {
                    p.write_input(&bytes);
                }
            }
        }
        Ok(())
    }

    fn mark_all_dirty(&self) {
        for p in &self.ws.panes {
            p.dirty.store(true, Ordering::Release);
        }
    }

    fn reap_pending(&mut self) {
        // Closed panes still need their reader threads to drain — we just
        // shut them down here.
        let panes = std::mem::take(&mut self.pending_close);
        for p in panes {
            p.shutdown();
        }
    }
}

/// RAII guard that puts the host terminal into raw mode + alternate screen
/// on construction and restores it on drop, even if we panic.
pub struct TerminalGuard {
    restored: bool,
}

impl TerminalGuard {
    pub fn install() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(
            io::stdout(),
            terminal::EnterAlternateScreen,
            cursor::Hide,
            event::EnableBracketedPaste
        )?;
        Ok(Self { restored: false })
    }

    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = execute!(
            io::stdout(),
            event::DisableBracketedPaste,
            terminal::LeaveAlternateScreen,
            cursor::Show
        );
        let _ = terminal::disable_raw_mode();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
