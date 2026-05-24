//! Translate crossterm key events into either a workspace command or a byte
//! sequence to forward to the focused PTY.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    FocusPrev,
    FocusNext,
    MoveLeft,
    MoveRight,
    NewPane,
    CloseFocused,
    Center,
    ScrollLeft,
    ScrollRight,
    GrowWidth,
    ShrinkWidth,
    /// Pass these bytes to the focused PTY's stdin.
    Input(Vec<u8>),
}

pub fn map(ev: KeyEvent) -> Option<Action> {
    // Only react to presses; releases would double-fire.
    if !matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);

    if alt {
        match ev.code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Char('h') if shift => return Some(Action::MoveLeft),
            KeyCode::Char('H') => return Some(Action::MoveLeft),
            KeyCode::Char('l') if shift => return Some(Action::MoveRight),
            KeyCode::Char('L') => return Some(Action::MoveRight),
            KeyCode::Char('h') => return Some(Action::FocusPrev),
            KeyCode::Char('l') => return Some(Action::FocusNext),
            KeyCode::Char('n') => return Some(Action::NewPane),
            KeyCode::Char('w') => return Some(Action::CloseFocused),
            KeyCode::Char('f') => return Some(Action::Center),
            KeyCode::Char('[') => return Some(Action::ScrollLeft),
            KeyCode::Char(']') => return Some(Action::ScrollRight),
            KeyCode::Char('=') | KeyCode::Char('+') => return Some(Action::GrowWidth),
            KeyCode::Char('-') | KeyCode::Char('_') => return Some(Action::ShrinkWidth),
            _ => {}
        }
    }

    encode_for_pty(ev).map(Action::Input)
}

/// Best-effort key→bytes encoder. We aim for the common cases the shell needs;
/// anything exotic gets dropped, which matches what most users hit in v0.1.
fn encode_for_pty(ev: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);

    let body: Vec<u8> = match ev.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Map Ctrl+letter to control characters (0x01..0x1f).
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    vec![(lower as u8) - b'a' + 1]
                } else if c == ' ' {
                    vec![0]
                } else {
                    let mut buf = [0u8; 4];
                    c.encode_utf8(&mut buf).as_bytes().to_vec()
                }
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => return None,
        },
        _ => return None,
    };

    if alt {
        // ESC-prefix encodes Alt for terminals in xterm mode.
        let mut out = Vec::with_capacity(body.len() + 1);
        out.push(0x1b);
        out.extend_from_slice(&body);
        Some(out)
    } else {
        Some(body)
    }
}
