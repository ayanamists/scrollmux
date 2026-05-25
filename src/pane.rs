//! A single column in the workspace: name, fixed logical width, and the PTY
//! plumbing that backs it.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

pub type Parser = Arc<Mutex<vt100::Parser>>;

pub struct Pane {
    pub name: String,
    /// Fixed logical width in cells. Invariant: independent from the host
    /// terminal width.
    pub width: u16,
    pub height: u16,
    pub parser: Parser,
    /// Set by the PTY reader thread whenever new output is parsed.
    pub dirty: Arc<AtomicBool>,
    /// `None` for fake panes used in tests.
    pty: Option<PtyHandles>,
}

struct PtyHandles {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Pane {
    /// Spawn a real PTY-backed pane running `command`.
    pub fn spawn(
        name: String,
        command: &[String],
        width: u16,
        height: u16,
    ) -> std::io::Result<Self> {
        if command.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty command",
            ));
        }
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: height,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_other)?;

        let mut cmd = CommandBuilder::new(&command[0]);
        for arg in &command[1..] {
            cmd.arg(arg);
        }
        // Inherit the user's environment so $PATH/$HOME/$TERM behave normally.
        for (k, v) in std::env::vars_os() {
            cmd.env(k, v);
        }
        // Tell the child it's running in an xterm-256-compatible terminal —
        // vt100 implements that subset.
        cmd.env("TERM", "xterm-256color");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }

        let child = pair.slave.spawn_command(cmd).map_err(io_other)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(io_other)?;
        let writer = pair.master.take_writer().map_err(io_other)?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(height, width, 0)));
        let dirty = Arc::new(AtomicBool::new(true));

        let parser_t = parser.clone();
        let dirty_t = dirty.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut p) = parser_t.lock() {
                            p.process(&buf[..n]);
                        }
                        dirty_t.store(true, Ordering::Release);
                    }
                    Err(_) => break,
                }
            }
            // Mark dirty one last time so the UI repaints any final output.
            dirty_t.store(true, Ordering::Release);
        });

        Ok(Pane {
            name,
            width,
            height,
            parser,
            dirty,
            pty: Some(PtyHandles {
                master: pair.master,
                writer,
                child,
            }),
        })
    }

    /// Construct a pane with no PTY backing — used by layout tests.
    #[cfg(test)]
    pub fn fake(width: u16, height: u16) -> Self {
        Pane {
            name: String::new(),
            width,
            height,
            parser: Arc::new(Mutex::new(vt100::Parser::new(height, width, 0))),
            dirty: Arc::new(AtomicBool::new(false)),
            pty: None,
        }
    }

    pub fn write_input(&mut self, bytes: &[u8]) {
        if let Some(p) = &mut self.pty {
            let _ = p.writer.write_all(bytes);
            let _ = p.writer.flush();
        }
    }

    pub fn set_width(&mut self, new_width: u16, height: u16) {
        self.width = new_width;
        self.height = height;
        if let Some(p) = &self.pty {
            let _ = p.master.resize(PtySize {
                rows: height,
                cols: new_width,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        if let Ok(mut parser) = self.parser.lock() {
            parser.set_size(height, new_width);
        }
        self.dirty.store(true, Ordering::Release);
    }

    pub fn set_height(&mut self, new_height: u16) {
        if new_height == self.height {
            return;
        }
        self.height = new_height;
        if let Some(p) = &self.pty {
            let _ = p.master.resize(PtySize {
                rows: new_height,
                cols: self.width,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        if let Ok(mut parser) = self.parser.lock() {
            parser.set_size(new_height, self.width);
        }
        self.dirty.store(true, Ordering::Release);
    }

    /// Best-effort cleanup. Dropping the master closes the PTY, which sends
    /// SIGHUP to the child; if that isn't enough we kill explicitly.
    pub fn shutdown(mut self) {
        if let Some(mut p) = self.pty.take() {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let Some(p) = &mut self.pty {
            let _ = p.child.kill();
        }
    }
}

fn io_other<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::other(e.to_string())
}
