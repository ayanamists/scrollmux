//! SIGWINCH listener: report host terminal resize events to the app loop.

use std::io;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use crossterm::terminal;
use signal_hook::consts::signal::SIGWINCH;
use signal_hook::iterator::Signals;

use crate::app::HostEvent;

pub fn spawn(tx: mpsc::Sender<HostEvent>) -> io::Result<JoinHandle<()>> {
    let mut signals = Signals::new([SIGWINCH])?;
    Ok(thread::spawn(move || {
        for _ in signals.forever() {
            let Ok((cols, rows)) = terminal::size() else {
                continue;
            };
            if tx.send(HostEvent::Resize(cols, rows)).is_err() {
                break;
            }
        }
    }))
}
