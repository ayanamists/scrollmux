//! Classify parsed terminal input into mux commands, while preserving raw bytes
//! for the focused PTY whenever input is not a single mux shortcut.

use termwiz::input::{InputEvent, InputParser, KeyCode, Modifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
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

pub fn classify(ev: &InputEvent) -> Option<Action> {
    let InputEvent::Key(key) = ev else {
        return None;
    };
    let modifiers = key.modifiers.remove_positional_mods();
    if !modifiers.contains(Modifiers::ALT) {
        return None;
    }
    let shift = modifiers.contains(Modifiers::SHIFT);

    match key.key {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('h') if shift => Some(Action::MoveLeft),
        KeyCode::Char('H') => Some(Action::MoveLeft),
        KeyCode::Char('l') if shift => Some(Action::MoveRight),
        KeyCode::Char('L') => Some(Action::MoveRight),
        KeyCode::Char('h') => Some(Action::FocusPrev),
        KeyCode::Char('l') => Some(Action::FocusNext),
        KeyCode::Char('n') => Some(Action::NewPane),
        KeyCode::Char('w') => Some(Action::CloseFocused),
        KeyCode::Char('f') => Some(Action::Center),
        KeyCode::Char('[') => Some(Action::ScrollLeft),
        KeyCode::Char(']') => Some(Action::ScrollRight),
        KeyCode::Char('=') | KeyCode::Char('+') => Some(Action::GrowWidth),
        KeyCode::Char('-') | KeyCode::Char('_') => Some(Action::ShrinkWidth),
        _ => None,
    }
}

pub struct InputAccumulator {
    parser: InputParser,
    pending: Vec<u8>,
}

impl InputAccumulator {
    pub fn new() -> Self {
        Self {
            parser: InputParser::new(),
            pending: Vec::new(),
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn push(&mut self, bytes: &[u8], maybe_more: bool) -> Option<Action> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        self.parser
            .parse(bytes, |event| events.push(event), maybe_more);
        self.dispatch(events, maybe_more)
    }

    pub fn finish(&mut self) -> Option<Action> {
        if self.pending.is_empty() {
            return None;
        }
        let mut events = Vec::new();
        self.parser.parse(&[], |event| events.push(event), false);
        self.dispatch(events, false)
    }

    fn dispatch(&mut self, events: Vec<InputEvent>, maybe_more: bool) -> Option<Action> {
        if events.is_empty() {
            if maybe_more || self.pending.is_empty() {
                return None;
            }
            return Some(Action::Input(std::mem::take(&mut self.pending)));
        }

        if events.len() == 1 {
            if let Some(action) = classify(&events[0]) {
                self.pending.clear();
                return Some(action);
            }
        }

        Some(Action::Input(std::mem::take(&mut self.pending)))
    }
}

impl Default for InputAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termwiz::input::KeyEvent;

    fn key(ch: char, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char(ch),
            modifiers,
        })
    }

    #[test]
    fn classifies_mux_shortcuts() {
        let cases = [
            ('q', Modifiers::ALT, Action::Quit),
            ('h', Modifiers::ALT, Action::FocusPrev),
            ('l', Modifiers::ALT, Action::FocusNext),
            ('h', Modifiers::ALT | Modifiers::SHIFT, Action::MoveLeft),
            ('H', Modifiers::ALT, Action::MoveLeft),
            ('l', Modifiers::ALT | Modifiers::SHIFT, Action::MoveRight),
            ('L', Modifiers::ALT, Action::MoveRight),
            ('n', Modifiers::ALT, Action::NewPane),
            ('w', Modifiers::ALT, Action::CloseFocused),
            ('f', Modifiers::ALT, Action::Center),
            ('[', Modifiers::ALT, Action::ScrollLeft),
            (']', Modifiers::ALT, Action::ScrollRight),
            ('=', Modifiers::ALT, Action::GrowWidth),
            ('+', Modifiers::ALT, Action::GrowWidth),
            ('-', Modifiers::ALT, Action::ShrinkWidth),
            ('_', Modifiers::ALT, Action::ShrinkWidth),
        ];

        for (ch, modifiers, action) in cases {
            assert_eq!(classify(&key(ch, modifiers)), Some(action));
        }
    }

    #[test]
    fn non_mux_events_are_not_classified() {
        assert_eq!(classify(&key('h', Modifiers::NONE)), None);
        assert_eq!(classify(&InputEvent::Paste("hello".into())), None);
    }

    #[test]
    fn alt_h_is_consumed_when_it_is_the_only_event() {
        let mut input = InputAccumulator::new();
        assert_eq!(input.push(b"\x1bh", true), Some(Action::FocusPrev));
    }

    #[test]
    fn bare_escape_is_forwarded_after_timeout() {
        let mut input = InputAccumulator::new();
        assert_eq!(input.push(b"\x1b", true), None);
        assert_eq!(input.finish(), Some(Action::Input(b"\x1b".to_vec())));
    }

    #[test]
    fn bracketed_paste_is_forwarded_with_markers() {
        let bytes = b"\x1b[200~hello\x1b[201~";
        let mut input = InputAccumulator::new();
        assert_eq!(input.push(bytes, true), Some(Action::Input(bytes.to_vec())));
    }

    #[test]
    fn mouse_reports_are_forwarded_as_original_bytes() {
        let bytes = b"\x1b[<66;42;12M\x1b[<67;42;12M";
        let mut input = InputAccumulator::new();
        assert_eq!(input.push(bytes, true), Some(Action::Input(bytes.to_vec())));
    }

    #[test]
    fn unclassified_escape_sequences_are_forwarded_after_timeout() {
        let bytes = b"\x1b]52;c;abcd\x07";
        let mut input = InputAccumulator::new();
        let action = input.push(bytes, true).or_else(|| input.finish());
        assert_eq!(action, Some(Action::Input(bytes.to_vec())));
    }

    #[test]
    fn partial_alt_left_bracket_waits_for_timeout() {
        let mut input = InputAccumulator::new();
        assert_eq!(input.push(b"\x1b[", true), None);
        assert_eq!(input.finish(), Some(Action::ScrollLeft));
    }

    #[test]
    fn split_utf8_is_forwarded_as_original_bytes() {
        let mut input = InputAccumulator::new();
        assert_eq!(input.push(&[0xc3], true), None);
        assert_eq!(
            input.push(&[0xa9], true),
            Some(Action::Input("é".as_bytes().to_vec()))
        );
    }

    #[test]
    fn multi_event_read_is_forwarded_even_when_it_contains_a_mux_shortcut() {
        let bytes = b"\x1bhls";
        let mut input = InputAccumulator::new();
        assert_eq!(input.push(bytes, true), Some(Action::Input(bytes.to_vec())));
    }
}
