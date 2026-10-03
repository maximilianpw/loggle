use super::list_state::SearchableListState;
use super::{App, DialogKind};

/// Query and selection state for the command palette and every searchable dialog.
///
/// Row data lives on [`App`]; callers pass the current row count so selections can be
/// clamped. Use [`dialog_len`] and friends to compute those counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Dialogs {
    palette: SearchableListState,
    property_filters: SearchableListState,
    message_fields: SearchableListState,
    filter_presets: SearchableListState,
    sources: SearchableListState,
}

impl Dialogs {
    pub(super) fn query(&self, kind: DialogKind) -> &str {
        self.state(kind).query()
    }

    pub(super) fn selected(&self, kind: DialogKind) -> usize {
        self.state(kind).selected()
    }

    pub(super) fn move_down(&mut self, kind: DialogKind, amount: usize, len: usize) {
        self.state_mut(kind).move_down(amount, len);
    }

    pub(super) fn move_up(&mut self, kind: DialogKind, amount: usize) {
        self.state_mut(kind).move_up(amount);
    }

    pub(super) fn push_query_char(&mut self, kind: DialogKind, value: char, len: usize) {
        self.state_mut(kind).push_query_char(value, len);
    }

    pub(super) fn pop_query_char(&mut self, kind: DialogKind, len: usize) {
        self.state_mut(kind).pop_query_char(len);
    }

    pub(super) fn sync(&mut self, kind: DialogKind, len: usize) {
        self.state_mut(kind).sync(len);
    }

    pub(super) fn palette_selected(&self) -> usize {
        self.palette.selected()
    }

    pub(super) fn move_palette_down(&mut self, amount: usize, len: usize) {
        self.palette.move_down(amount, len);
    }

    pub(super) fn move_palette_up(&mut self, amount: usize) {
        self.palette.move_up(amount);
    }

    pub(super) fn sync_palette(&mut self, len: usize) {
        self.palette.sync(len);
    }

    fn state(&self, kind: DialogKind) -> &SearchableListState {
        match kind {
            DialogKind::PropertyFilters => &self.property_filters,
            DialogKind::MessageFields => &self.message_fields,
            DialogKind::FilterPresets => &self.filter_presets,
            DialogKind::Sources => &self.sources,
        }
    }

    fn state_mut(&mut self, kind: DialogKind) -> &mut SearchableListState {
        match kind {
            DialogKind::PropertyFilters => &mut self.property_filters,
            DialogKind::MessageFields => &mut self.message_fields,
            DialogKind::FilterPresets => &mut self.filter_presets,
            DialogKind::Sources => &mut self.sources,
        }
    }
}

/// Number of rows the dialog currently shows for its query.
pub(super) fn dialog_len(app: &App, kind: DialogKind) -> usize {
    dialog_len_for_query(app, kind, app.dialogs.query(kind).trim())
}

/// Number of rows the dialog would show after `value` is appended to its query.
pub(super) fn dialog_len_after_query_push(app: &App, kind: DialogKind, value: char) -> usize {
    let mut query = app.dialogs.query(kind).to_string();
    query.push(value);
    dialog_len_for_query(app, kind, query.trim())
}

/// Number of rows the dialog would show after the last query character is removed.
pub(super) fn dialog_len_after_query_pop(app: &App, kind: DialogKind) -> usize {
    let mut query = app.dialogs.query(kind).to_string();
    query.pop();
    dialog_len_for_query(app, kind, query.trim())
}

fn dialog_len_for_query(app: &App, kind: DialogKind, query: &str) -> usize {
    match kind {
        DialogKind::PropertyFilters => app.filter_workflow.property_filter_row_count(query),
        DialogKind::MessageFields => app
            .message_field_keys
            .iter()
            .filter(|key| {
                query.is_empty() || crate::filter::contains_ignore_ascii_case(key.as_str(), query)
            })
            .count(),
        DialogKind::FilterPresets => app.filter_workflow.filter_preset_row_count(query),
        DialogKind::Sources => app.source_status_rows_for_query(query).len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{PropertyFilterUpdate, PropertyPredicate};
    use crate::model::Level;

    use crate::app::{Mode, PromptKind};

    #[test]
    fn dialog_states_are_independent_per_kind() {
        let mut dialogs = Dialogs::default();

        dialogs.push_query_char(DialogKind::Sources, 'a', 3);
        dialogs.move_down(DialogKind::Sources, 2, 3);
        dialogs.move_down(DialogKind::MessageFields, 1, 3);

        assert_eq!(dialogs.query(DialogKind::Sources), "a");
        assert_eq!(dialogs.selected(DialogKind::Sources), 2);
        assert_eq!(dialogs.query(DialogKind::MessageFields), "");
        assert_eq!(dialogs.selected(DialogKind::MessageFields), 1);
        assert_eq!(dialogs.selected(DialogKind::PropertyFilters), 0);
        assert_eq!(dialogs.palette_selected(), 0);
    }

    #[test]
    fn filter_presets_save_search_and_restore_filters() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("web | INFO two".to_string());
        app.start_prompt(PromptKind::Level);
        for ch in "error".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        app.save_filter_preset();
        app.clear_filters();
        assert_eq!(app.visible_count(), 2);

        app.open_dialog(DialogKind::FilterPresets);
        app.push_dialog_query_char(DialogKind::FilterPresets, 'e');
        app.push_dialog_query_char(DialogKind::FilterPresets, 'r');
        let rows = app.filter_preset_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].summary, "level=error");

        app.activate_selected_dialog_row(DialogKind::FilterPresets);

        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.filters().level, Some(Level::Error));
        assert_eq!(app.visible_count(), 1);
    }

    #[test]
    fn filter_presets_are_not_duplicated() {
        let mut app = App::new(10);
        app.start_prompt(PromptKind::Source);
        for ch in "api".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        app.save_filter_preset();
        app.save_filter_preset();

        assert_eq!(app.filter_preset_rows().len(), 1);
    }

    #[test]
    fn message_field_dialog_searches_and_deletes_selected_fields() {
        let mut app = App::new(10);
        app.message_field_keys = vec![
            "tenantId".to_string(),
            "requestId".to_string(),
            "durationMs".to_string(),
        ];

        app.open_dialog(DialogKind::MessageFields);
        app.push_dialog_query_char(DialogKind::MessageFields, 'r');
        app.push_dialog_query_char(DialogKind::MessageFields, 'e');

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::MessageFields));
        assert_eq!(app.message_field_rows(), vec!["requestId"]);

        app.delete_selected_dialog_row(DialogKind::MessageFields);

        assert_eq!(
            app.message_field_keys(),
            &["tenantId".to_string(), "durationMs".to_string()]
        );
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::MessageFields));
        assert_eq!(app.selected_dialog_index(DialogKind::MessageFields), 0);
    }

    #[test]
    fn message_field_dialog_backspace_deletes_when_search_is_empty() {
        let mut app = App::new(10);
        app.message_field_keys = vec!["tenantId".to_string()];

        app.open_dialog(DialogKind::MessageFields);
        app.delete_selected_dialog_row(DialogKind::MessageFields);

        assert!(app.message_field_keys().is_empty());
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::MessageFields));
    }

    #[test]
    fn palette_opens_and_closes_from_normal_mode() {
        let mut app = App::new(10);

        app.toggle_palette();
        assert_eq!(app.mode(), &Mode::Palette);

        app.toggle_palette();
        assert_eq!(app.mode(), &Mode::Normal);
    }

    #[test]
    fn palette_selection_moves_and_clamps() {
        let mut app = App::new(10);
        app.open_palette();

        app.move_palette_down(2);
        assert_eq!(app.palette_selected(), 2);

        app.move_palette_down(usize::MAX);
        assert_eq!(app.palette_selected(), app.palette_commands().len() - 1);

        app.move_palette_up(usize::MAX);
        assert_eq!(app.palette_selected(), 0);
    }

    #[test]
    fn property_filter_dialog_searches_active_filters() {
        let mut app = App::new(10);
        app.filters_mut().add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("tenantId", "tenant-1"),
        });
        app.filters_mut().add_property_filter(PropertyFilterUpdate {
            exclude: true,
            predicate: PropertyPredicate::exists("debug"),
        });

        app.open_dialog(DialogKind::PropertyFilters);
        app.push_dialog_query_char(DialogKind::PropertyFilters, 'i');
        app.push_dialog_query_char(DialogKind::PropertyFilters, 'g');

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::PropertyFilters));
        let rows = app.property_filter_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "ignore");
        assert_eq!(rows[0].summary, "!debug");
    }

    #[test]
    fn deleting_selected_property_filter_removes_it() {
        let mut app = App::new(10);
        app.filters_mut().add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("tenantId", "tenant-1"),
        });
        app.filters_mut().add_property_filter(PropertyFilterUpdate {
            exclude: true,
            predicate: PropertyPredicate::exists("debug"),
        });

        app.open_dialog(DialogKind::PropertyFilters);
        app.move_dialog_down(DialogKind::PropertyFilters, 1);
        app.delete_selected_dialog_row(DialogKind::PropertyFilters);

        assert_eq!(
            app.filters().property_includes,
            vec![PropertyPredicate::exact("tenantId", "tenant-1")]
        );
        assert!(app.filters().property_excludes.is_empty());
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::PropertyFilters));
        assert_eq!(app.selected_dialog_index(DialogKind::PropertyFilters), 0);
    }

    #[test]
    fn editing_property_filter_replaces_existing_filter() {
        let mut app = App::new(10);
        app.filters_mut().add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("tenantId", "tenant-1"),
        });

        app.open_dialog(DialogKind::PropertyFilters);
        app.start_property_filter_edit();
        assert_eq!(app.mode(), &Mode::Prompt(PromptKind::EditPropertyFilter));
        assert_eq!(app.prompt(), "tenantId=tenant-1");
        for _ in 0.."tenantId=tenant-1".len() {
            app.pop_prompt_char();
        }
        for value in "tenantId!=tenant-2".chars() {
            app.push_prompt_char(value);
        }
        app.apply_prompt();

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::PropertyFilters));
        assert!(app.filters().property_includes.is_empty());
        assert_eq!(
            app.filters().property_excludes,
            vec![PropertyPredicate::exact("tenantId", "tenant-2")]
        );
    }
}
