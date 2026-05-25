mod app;
mod input;
mod pane;
mod render;
mod signal_pump;
mod stdin_pump;
mod workspace;

use std::io::{self, Write};

use crossterm::terminal;

use crate::app::{App, TerminalGuard};

const DEFAULT_PANE_WIDTH: u16 = 120;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("scrollmux: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> io::Result<()> {
    let (cols, rows) = terminal::size()?;

    // Install raw-mode/alt-screen guard FIRST so any later panic still
    // restores the terminal via Drop.
    let mut guard = TerminalGuard::install()?;

    // Hook the panic handler so the user sees a useful message instead of a
    // broken terminal if anything goes wrong inside the event loop.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = terminal::disable_raw_mode();
        default_hook(info);
    }));

    let mut app = App::new(cols, rows, DEFAULT_PANE_WIDTH);

    let result = {
        let mut out = io::stdout().lock();
        let r = app.run(&mut out);
        let _ = out.flush();
        r
    };

    guard.restore();
    result
}
