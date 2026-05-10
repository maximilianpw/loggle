use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Mode, PromptKind};
use crate::commands::CommandAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    Continue,
    Quit,
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent, half_page: usize) -> KeyOutcome {
    match app.mode() {
        Mode::Prompt(_) => handle_prompt_key(app, key),
        Mode::Palette => handle_palette_key(app, key, half_page),
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
        (KeyCode::Char('q'), _) => return execute_command(app, CommandAction::Quit),
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => app.move_down(1),
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => app.move_up(1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.move_down(half_page),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.move_up(half_page),
        (KeyCode::Char('g'), _) => app.handle_g(),
        (KeyCode::Char('G'), _) => return execute_command(app, CommandAction::JumpBottom),
        (KeyCode::Enter, _) => return execute_command(app, CommandAction::ToggleDetails),
        (KeyCode::Char('/'), _) => return execute_command(app, CommandAction::Search),
        (KeyCode::Char('s'), _) => return execute_command(app, CommandAction::SourceFilter),
        (KeyCode::Char('l'), _) => return execute_command(app, CommandAction::LevelFilter),
        (KeyCode::Char('+'), _) => return execute_command(app, CommandAction::IncludeProperty),
        (KeyCode::Char('-'), _) => return execute_command(app, CommandAction::ExcludeProperty),
        (KeyCode::Char('f'), _) => return execute_command(app, CommandAction::FollowProperty),
        (KeyCode::Char(']'), _) => return execute_command(app, CommandAction::NextProperty),
        (KeyCode::Char('['), _) => return execute_command(app, CommandAction::PreviousProperty),
        (KeyCode::Char('c'), _) => return execute_command(app, CommandAction::ClearFilters),
        (KeyCode::Char(' '), _) | (KeyCode::Char('p'), _) => {
            return execute_command(app, CommandAction::ToggleFollow);
        }
        (KeyCode::Char('n'), _) => return execute_command(app, CommandAction::NextMatch),
        (KeyCode::Char('N'), _) => return execute_command(app, CommandAction::PreviousMatch),
        (KeyCode::Char('?'), _) => app.toggle_palette(),
        (KeyCode::Esc, _) => app.clear_transient(),
        _ => app.clear_transient(),
    }

    KeyOutcome::Continue
}

fn handle_palette_key(app: &mut App, key: KeyEvent, half_page: usize) -> KeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('?'), _) => app.close_palette(),
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => app.move_palette_down(1),
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => app.move_palette_up(1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.move_palette_down(half_page),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.move_palette_up(half_page),
        (KeyCode::Enter, _) => {
            let Some(command) = app.selected_palette_command() else {
                app.close_palette();
                return KeyOutcome::Continue;
            };
            app.close_palette();
            return execute_command(app, command.action);
        }
        _ => {}
    }

    KeyOutcome::Continue
}

fn execute_command(app: &mut App, action: CommandAction) -> KeyOutcome {
    match action {
        CommandAction::Search => app.start_prompt(PromptKind::Text),
        CommandAction::SourceFilter => app.start_prompt(PromptKind::Source),
        CommandAction::LevelFilter => app.start_prompt(PromptKind::Level),
        CommandAction::ToggleDetails => app.toggle_details(),
        CommandAction::PreviousProperty => app.previous_property(),
        CommandAction::NextProperty => app.next_property(),
        CommandAction::FollowProperty => app.follow_selected_property(),
        CommandAction::IncludeProperty => app.start_prompt(PromptKind::IncludeProperty),
        CommandAction::ExcludeProperty => app.start_prompt(PromptKind::ExcludeProperty),
        CommandAction::ClearFilters => app.clear_filters(),
        CommandAction::NextMatch => app.next_search_match(),
        CommandAction::PreviousMatch => app.previous_search_match(),
        CommandAction::ToggleFollow => app.toggle_follow(),
        CommandAction::JumpTop => app.jump_top(),
        CommandAction::JumpBottom => app.jump_bottom(),
        CommandAction::Quit => return KeyOutcome::Quit,
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
    fn property_prompt_adds_include_filter() {
        let mut app = App::new(10);

        handle_key(&mut app, key(KeyCode::Char('+')), 5);
        for value in "tenantId=tenant-1".chars() {
            handle_key(&mut app, key(KeyCode::Char(value)), 5);
        }
        handle_key(&mut app, key(KeyCode::Enter), 5);

        assert_eq!(app.filters().property_includes.len(), 1);
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

    #[test]
    fn question_mark_opens_palette() {
        let mut app = App::new(10);

        handle_key(&mut app, key(KeyCode::Char('?')), 5);

        assert_eq!(app.mode(), &Mode::Palette);
    }

    #[test]
    fn palette_escape_and_question_mark_close_palette() {
        let mut app = App::new(10);
        app.open_palette();

        handle_key(&mut app, key(KeyCode::Esc), 5);
        assert_eq!(app.mode(), &Mode::Normal);

        app.open_palette();
        handle_key(&mut app, key(KeyCode::Char('?')), 5);
        assert_eq!(app.mode(), &Mode::Normal);
    }

    #[test]
    fn palette_enter_starts_prompt_commands() {
        let mut app = App::new(10);
        assert_palette_prompt(&mut app, CommandAction::Search, PromptKind::Text);
        assert_palette_prompt(&mut app, CommandAction::SourceFilter, PromptKind::Source);
        assert_palette_prompt(&mut app, CommandAction::LevelFilter, PromptKind::Level);
        assert_palette_prompt(
            &mut app,
            CommandAction::IncludeProperty,
            PromptKind::IncludeProperty,
        );
        assert_palette_prompt(
            &mut app,
            CommandAction::ExcludeProperty,
            PromptKind::ExcludeProperty,
        );
    }

    #[test]
    fn palette_enter_can_request_quit() {
        let mut app = App::new(10);
        app.open_palette();
        app.move_palette_down(app.palette_commands().len());

        let outcome = handle_key(&mut app, key(KeyCode::Enter), 5);

        assert_eq!(outcome, KeyOutcome::Quit);
    }

    fn assert_palette_prompt(app: &mut App, action: CommandAction, expected: PromptKind) {
        app.open_palette();
        move_palette_to_action(app, action);

        handle_key(app, key(KeyCode::Enter), 5);

        assert_eq!(app.mode(), &Mode::Prompt(expected));
        app.cancel_prompt();
    }

    fn move_palette_to_action(app: &mut App, action: CommandAction) {
        let index = app
            .palette_commands()
            .iter()
            .position(|command| command.action == action)
            .unwrap();
        app.move_palette_up(usize::MAX);
        app.move_palette_down(index);
    }
}
