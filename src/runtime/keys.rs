use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Mode, PromptKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    Continue,
    Quit,
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent, half_page: usize) -> KeyOutcome {
    match app.mode() {
        Mode::Prompt(_) => handle_prompt_key(app, key),
        Mode::Normal => handle_normal_key(app, key, half_page),
    }
}

fn handle_prompt_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    match key.code {
        KeyCode::Esc => app.cancel_prompt(),
        KeyCode::Enter => app.apply_prompt(),
        KeyCode::Backspace => app.pop_prompt_char(),
        KeyCode::Char(value)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            app.push_prompt_char(value);
        }
        _ => {}
    }

    KeyOutcome::Continue
}

fn handle_normal_key(app: &mut App, key: KeyEvent, half_page: usize) -> KeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) => return KeyOutcome::Quit,
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => app.move_down(1),
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => app.move_up(1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.move_down(half_page),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.move_up(half_page),
        (KeyCode::Char('g'), _) => app.handle_g(),
        (KeyCode::Char('G'), _) => app.jump_bottom(),
        (KeyCode::Char('/'), _) => app.start_prompt(PromptKind::Text),
        (KeyCode::Char('s'), _) => app.start_prompt(PromptKind::Source),
        (KeyCode::Char('l'), _) => app.start_prompt(PromptKind::Level),
        (KeyCode::Char('c'), _) => app.clear_filters(),
        (KeyCode::Char(' '), _) | (KeyCode::Char('p'), _) => app.toggle_follow(),
        (KeyCode::Char('n'), _) => app.next_search_match(),
        (KeyCode::Char('N'), _) => app.previous_search_match(),
        (KeyCode::Esc, _) => app.clear_transient(),
        _ => app.clear_transient(),
    }

    KeyOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Level;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn normal_mode_quit_key_requests_exit() {
        let mut app = App::new(10);

        let outcome = handle_key(&mut app, key(KeyCode::Char('q')), 5);

        assert_eq!(outcome, KeyOutcome::Quit);
    }

    #[test]
    fn normal_mode_search_key_starts_text_prompt() {
        let mut app = App::new(10);

        let outcome = handle_key(&mut app, key(KeyCode::Char('/')), 5);

        assert_eq!(outcome, KeyOutcome::Continue);
        assert_eq!(app.mode(), &Mode::Prompt(PromptKind::Text));
    }

    #[test]
    fn prompt_mode_edits_and_applies_prompt() {
        let mut app = App::new(10);
        app.start_prompt(PromptKind::Level);

        for value in "error".chars() {
            handle_key(&mut app, key(KeyCode::Char(value)), 5);
        }
        handle_key(&mut app, key(KeyCode::Enter), 5);

        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.filters().level, Some(Level::Error));
    }

    #[test]
    fn prompt_escape_cancels_without_applying() {
        let mut app = App::new(10);
        app.start_prompt(PromptKind::Text);
        handle_key(&mut app, key(KeyCode::Char('e')), 5);

        handle_key(&mut app, key(KeyCode::Esc), 5);

        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.filters().text, None);
    }
}
