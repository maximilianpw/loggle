use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, DialogKind, Mode, PromptKind, YankedLines};
use crate::commands::{CommandAction, normal_action_for_key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    Continue,
    Copy { text: String, line_count: usize },
    Quit,
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent, half_page: usize) -> KeyOutcome {
    app.clear_notice();

    match app.mode() {
        Mode::Prompt(_) => handle_prompt_key(app, key),
        Mode::Palette => handle_palette_key(app, key, half_page),
        Mode::Dialog(kind) => handle_dialog_key(app, *kind, key, half_page),
        Mode::Visual => handle_visual_key(app, key, half_page),
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
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => app.move_down(1),
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => app.move_up(1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.move_down(half_page),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.move_up(half_page),
        (KeyCode::Char('g'), _) => app.handle_g(),
        (KeyCode::Char('?'), _) => app.toggle_palette(),
        (KeyCode::Esc, _) => app.clear_transient(),
        _ => {
            if let Some(action) = normal_action_for_key(key) {
                return execute_command(app, action);
            }
            app.clear_transient();
        }
    }

    KeyOutcome::Continue
}

fn handle_visual_key(app: &mut App, key: KeyEvent, half_page: usize) -> KeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Char('y'), _) => return copy_outcome(app.yank_visual_selection()),
        (KeyCode::Esc, _) | (KeyCode::Char('v'), _) => app.cancel_visual_selection(),
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => app.move_down(1),
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => app.move_up(1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.move_down(half_page),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.move_up(half_page),
        (KeyCode::Char('g'), _) => app.handle_g(),
        (KeyCode::Char('G'), _) => app.move_to_last_visible(),
        _ => app.clear_transient(),
    }

    KeyOutcome::Continue
}

fn handle_dialog_key(
    app: &mut App,
    kind: DialogKind,
    key: KeyEvent,
    half_page: usize,
) -> KeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => app.close_dialog(),
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => app.move_dialog_down(kind, 1),
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => app.move_dialog_up(kind, 1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.move_dialog_down(kind, half_page),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.move_dialog_up(kind, half_page),
        (KeyCode::Backspace, _) if app.dialog_query(kind).is_empty() => {
            app.delete_selected_dialog_row(kind)
        }
        (KeyCode::Backspace, _) => app.pop_dialog_query_char(kind),
        (KeyCode::Delete, _) if delete_key_removes_dialog_row(app, kind) => {
            app.delete_selected_dialog_row(kind)
        }
        (KeyCode::Enter, _) => app.activate_selected_dialog_row(kind),
        (KeyCode::Char(value), modifiers)
            if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
        {
            app.push_dialog_query_char(kind, value);
        }
        _ => {}
    }

    KeyOutcome::Continue
}

fn delete_key_removes_dialog_row(app: &App, kind: DialogKind) -> bool {
    kind == DialogKind::PropertyFilters
        || kind == DialogKind::DisplayFields
        || app.dialog_query(kind).is_empty()
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
        CommandAction::CopySelectedLine => return copy_outcome(app.yank_selected_line()),
        CommandAction::StartVisualSelection => app.start_visual_selection(),
        CommandAction::PreviousProperty => app.previous_property(),
        CommandAction::NextProperty => app.next_property(),
        CommandAction::FollowProperty => app.follow_selected_property(),
        CommandAction::AddDisplayField => app.add_selected_display_field(),
        CommandAction::DisplayFields => app.open_dialog(DialogKind::DisplayFields),
        CommandAction::PropertyFilters => app.open_dialog(DialogKind::PropertyFilters),
        CommandAction::IncludeProperty => app.start_prompt(PromptKind::IncludeProperty),
        CommandAction::ExcludeProperty => app.start_prompt(PromptKind::ExcludeProperty),
        CommandAction::ClearFilters => app.clear_filters(),
        CommandAction::UndoFilterChange => app.undo_filter_change(),
        CommandAction::SaveFilterPreset => app.save_filter_preset(),
        CommandAction::FilterPresets => app.open_dialog(DialogKind::FilterPresets),
        CommandAction::ExportVisibleLogs => {
            let _ = app.export_visible_logs_default();
        }
        CommandAction::ToggleMarker => app.toggle_selected_marker(),
        CommandAction::Sources => app.open_dialog(DialogKind::Sources),
        CommandAction::NextMatch => app.next_search_match(),
        CommandAction::PreviousMatch => app.previous_search_match(),
        CommandAction::ToggleFollow => app.toggle_follow(),
        CommandAction::JumpTop => app.jump_top(),
        CommandAction::JumpBottom => app.jump_bottom(),
        CommandAction::Quit => return KeyOutcome::Quit,
    }

    KeyOutcome::Continue
}

fn copy_outcome(yanked: Option<YankedLines>) -> KeyOutcome {
    if let Some(yanked) = yanked {
        KeyOutcome::Copy {
            text: yanked.text,
            line_count: yanked.line_count,
        }
    } else {
        KeyOutcome::Continue
    }
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
    fn normal_mode_u_restores_previous_filter_state() {
        let mut app = App::new(10);
        handle_key(&mut app, key(KeyCode::Char('l')), 5);
        for value in "error".chars() {
            handle_key(&mut app, key(KeyCode::Char(value)), 5);
        }
        handle_key(&mut app, key(KeyCode::Enter), 5);

        assert_eq!(app.filters().level, Some(Level::Error));

        handle_key(&mut app, key(KeyCode::Char('u')), 5);

        assert_eq!(app.filters().level, None);
    }

    #[test]
    fn question_mark_opens_palette() {
        let mut app = App::new(10);

        handle_key(&mut app, key(KeyCode::Char('?')), 5);

        assert_eq!(app.mode(), &Mode::Palette);
    }

    #[test]
    fn capital_p_opens_property_filter_dialog() {
        let mut app = App::new(10);

        handle_key(&mut app, key(KeyCode::Char('P')), 5);

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::PropertyFilters));
    }

    #[test]
    fn capital_m_opens_display_fields_dialog() {
        let mut app = App::new(10);

        handle_key(&mut app, key(KeyCode::Char('M')), 5);

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::DisplayFields));
    }

    #[test]
    fn capital_v_opens_filter_presets_dialog() {
        let mut app = App::new(10);

        handle_key(&mut app, key(KeyCode::Char('V')), 5);

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::FilterPresets));
    }

    #[test]
    fn capital_t_toggles_selected_marker() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());

        handle_key(&mut app, key(KeyCode::Char('T')), 5);

        assert_eq!(app.marker_count(), 1);
    }

    #[test]
    fn capital_o_opens_sources_dialog() {
        let mut app = App::new(10);

        handle_key(&mut app, key(KeyCode::Char('O')), 5);

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::Sources));
    }

    #[test]
    fn normal_mode_y_copies_selected_line() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.push_line("web | WARN two".to_string());

        let outcome = handle_key(&mut app, key(KeyCode::Char('y')), 5);

        assert_eq!(
            outcome,
            KeyOutcome::Copy {
                text: "web | WARN two".to_string(),
                line_count: 1
            }
        );
        assert_eq!(app.mode(), &Mode::Normal);
    }

    #[test]
    fn normal_mode_v_starts_visual_selection() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());

        handle_key(&mut app, key(KeyCode::Char('v')), 5);

        assert_eq!(app.mode(), &Mode::Visual);
        assert_eq!(app.visual_selection_range(), Some((0, 0)));
    }

    #[test]
    fn visual_mode_y_copies_range_and_returns_to_normal() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.push_line("web | INFO two".to_string());
        app.push_line("worker | INFO three".to_string());
        app.jump_top();
        app.start_visual_selection();

        handle_key(&mut app, key(KeyCode::Char('j')), 5);
        let outcome = handle_key(&mut app, key(KeyCode::Char('y')), 5);

        assert_eq!(
            outcome,
            KeyOutcome::Copy {
                text: "api | INFO one\nweb | INFO two".to_string(),
                line_count: 2
            }
        );
        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.visual_selection_range(), None);
    }

    #[test]
    fn visual_mode_escape_cancels_selection() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.start_visual_selection();

        handle_key(&mut app, key(KeyCode::Esc), 5);

        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.visual_selection_range(), None);
    }

    #[test]
    fn property_filter_dialog_searches_and_deletes() {
        let mut app = App::new(10);
        add_include_filter(&mut app, "tenantId=tenant-1");

        app.open_dialog(DialogKind::PropertyFilters);
        handle_key(&mut app, key(KeyCode::Char('t')), 5);
        handle_key(&mut app, key(KeyCode::Char('e')), 5);
        assert_eq!(app.dialog_query(DialogKind::PropertyFilters), "te");

        handle_key(&mut app, key(KeyCode::Backspace), 5);
        assert_eq!(app.dialog_query(DialogKind::PropertyFilters), "t");

        handle_key(&mut app, key(KeyCode::Delete), 5);
        assert!(app.filters().property_includes.is_empty());
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::PropertyFilters));
    }

    #[test]
    fn property_filter_dialog_backspace_deletes_when_search_is_empty() {
        let mut app = App::new(10);
        add_include_filter(&mut app, "tenantId=tenant-1");

        app.open_dialog(DialogKind::PropertyFilters);
        handle_key(&mut app, key(KeyCode::Backspace), 5);

        assert!(app.filters().property_includes.is_empty());
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::PropertyFilters));
    }

    #[test]
    fn display_fields_dialog_searches_toggles_and_deletes() {
        let mut app = App::new(10);
        add_display_field_from_selected_property(&mut app);

        app.open_dialog(DialogKind::DisplayFields);
        handle_key(&mut app, key(KeyCode::Char('t')), 5);
        assert_eq!(app.dialog_query(DialogKind::DisplayFields), "t");

        handle_key(&mut app, key(KeyCode::Backspace), 5);
        assert_eq!(app.dialog_query(DialogKind::DisplayFields), "");

        handle_key(&mut app, key(KeyCode::Enter), 5);
        assert!(app.display_field_keys().is_empty());
        handle_key(&mut app, key(KeyCode::Enter), 5);
        assert_eq!(app.display_field_keys(), &["tenantId".to_string()]);
        handle_key(&mut app, key(KeyCode::Delete), 5);
        assert!(app.display_field_keys().is_empty());
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::DisplayFields));
    }

    #[test]
    fn display_fields_dialog_backspace_deletes_when_search_is_empty() {
        let mut app = App::new(10);
        add_display_field_from_selected_property(&mut app);

        app.open_dialog(DialogKind::DisplayFields);
        handle_key(&mut app, key(KeyCode::Backspace), 5);

        assert!(app.display_field_keys().is_empty());
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::DisplayFields));
    }

    #[test]
    fn property_filter_dialog_enter_starts_edit_prompt() {
        let mut app = App::new(10);
        add_exclude_filter(&mut app, "debug");

        app.open_dialog(DialogKind::PropertyFilters);
        handle_key(&mut app, key(KeyCode::Enter), 5);

        assert_eq!(app.mode(), &Mode::Prompt(PromptKind::EditPropertyFilter));
        assert_eq!(app.prompt(), "!debug");
    }

    #[test]
    fn property_filter_edit_escape_returns_to_dialog() {
        let mut app = App::new(10);
        add_include_filter(&mut app, "tenantId=tenant-1");

        app.open_dialog(DialogKind::PropertyFilters);
        handle_key(&mut app, key(KeyCode::Enter), 5);
        handle_key(&mut app, key(KeyCode::Esc), 5);

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::PropertyFilters));
        assert_eq!(app.filters().property_includes.len(), 1);
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

    fn add_include_filter(app: &mut App, value: &str) {
        app.start_prompt(PromptKind::IncludeProperty);
        for ch in value.chars() {
            handle_key(app, key(KeyCode::Char(ch)), 5);
        }
        handle_key(app, key(KeyCode::Enter), 5);
    }

    fn add_exclude_filter(app: &mut App, value: &str) {
        app.start_prompt(PromptKind::ExcludeProperty);
        for ch in value.chars() {
            handle_key(app, key(KeyCode::Char(ch)), 5);
        }
        handle_key(app, key(KeyCode::Enter), 5);
    }

    fn add_display_field_from_selected_property(app: &mut App) {
        app.push_line("api | INFO request tenantId=tenant-1".to_string());

        handle_key(app, key(KeyCode::Char('m')), 5);
        assert_eq!(app.display_field_keys(), &["tenantId".to_string()]);
    }
}
