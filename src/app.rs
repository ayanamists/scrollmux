//! Glue: own the workspace, run the event loop, route input, drive the
//! renderer, and restore the host terminal on exit.

use std::io::{self, Write};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::style::Print;
use crossterm::{cursor, execute, terminal};

use crate::input::{Action, InputAccumulator};
use crate::pane::Pane;
use crate::render;
use crate::signal_pump;
use crate::stdin_pump;
use crate::workspace::Workspace;

const TICK_MS: u64 = 16;
const ESCAPE_TIMEOUT_MS: u64 = 50;
const SCROLL_STEP: i32 = 20;
const WIDTH_STEP: i32 = 10;

#[derive(Debug)]
pub(crate) enum HostEvent {
    Bytes(Vec<u8>),
    Resize(u16, u16),
    Tick,
}

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
        let pane = Pane::spawn(name, &[shell], self.ws.default_width, self.ws.pane_height())?;
        self.next_id += 1;
        self.ws.push_pane(pane);
        Ok(())
    }

    pub fn run<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        // Spawn an initial shell so the user sees something useful immediately.
        self.spawn_default()?;

        let mut renderer = render::Renderer::new();
        renderer.render(out, &self.ws)?;
        self.take_visible_dirty();

        let (tx, rx) = mpsc::channel();
        let _stdin_handle = stdin_pump::spawn(tx.clone());
        let _signal_handle = signal_pump::spawn(tx.clone())?;
        let _tick_handle = spawn_tick_pump(tx);

        let mut input = InputAccumulator::new();
        let mut last_pending_input: Option<Instant> = None;

        while !self.quitting && !self.ws.panes.is_empty() {
            match rx.recv_timeout(Duration::from_millis(ESCAPE_TIMEOUT_MS)) {
                Ok(event) => {
                    self.handle_host_event(event, &mut input, &mut last_pending_input)?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            self.finish_pending_input_if_due(&mut input, &mut last_pending_input)?;

            self.reap_pending();
            if self.ws.panes.is_empty() {
                break;
            }

            if self.take_visible_dirty() || self.force_redraw() {
                renderer.render(out, &self.ws)?;
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

    fn take_visible_dirty(&self) -> bool {
        let mut dirty = false;
        for vp in self.ws.visible_panes() {
            dirty |= self.ws.panes[vp.idx].dirty.swap(false, Ordering::AcqRel);
        }
        dirty
    }

    fn force_redraw(&self) -> bool {
        // Currently we only force a redraw immediately after input events,
        // which are already handled inline. Hook for future use.
        false
    }

    fn handle_host_event(
        &mut self,
        ev: HostEvent,
        input: &mut InputAccumulator,
        last_pending_input: &mut Option<Instant>,
    ) -> io::Result<()> {
        match ev {
            HostEvent::Bytes(bytes) => {
                if let Some(action) = input.push(&bytes, true) {
                    *last_pending_input = None;
                    self.handle_action(action)?;
                } else if input.has_pending() {
                    *last_pending_input = Some(Instant::now());
                }
            }
            HostEvent::Resize(cols, rows) => {
                self.ws.handle_host_resize(cols, rows);
                for p in &self.ws.panes {
                    p.dirty.store(true, Ordering::Release);
                }
            }
            HostEvent::Tick => {}
        }
        Ok(())
    }

    fn finish_pending_input_if_due(
        &mut self,
        input: &mut InputAccumulator,
        last_pending_input: &mut Option<Instant>,
    ) -> io::Result<()> {
        let Some(last) = *last_pending_input else {
            return Ok(());
        };
        if last.elapsed() < Duration::from_millis(ESCAPE_TIMEOUT_MS) {
            return Ok(());
        }

        *last_pending_input = None;
        if let Some(action) = input.finish() {
            self.handle_action(action)?;
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
            Print("\x1b[?2004h")
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
            Print("\x1b[?2004l"),
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

fn spawn_tick_pump(tx: mpsc::Sender<HostEvent>) -> thread::JoinHandle<()> {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(TICK_MS));
        if tx.send(HostEvent::Tick).is_err() {
            break;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::Pane;

    #[test]
    fn visible_dirty_ignores_offscreen_panes() {
        let mut app = App::new(10, 5, 10);
        app.ws.panes.push(Pane::fake(10, 4));
        app.ws.panes.push(Pane::fake(10, 4));
        app.ws.viewport_x = 0;

        app.ws.panes[1].dirty.store(true, Ordering::Release);
        assert!(!app.take_visible_dirty());
        assert!(app.ws.panes[1].dirty.load(Ordering::Acquire));

        app.ws.panes[0].dirty.store(true, Ordering::Release);
        assert!(app.take_visible_dirty());
        assert!(!app.ws.panes[0].dirty.load(Ordering::Acquire));
        assert!(app.ws.panes[1].dirty.load(Ordering::Acquire));
    }
}
