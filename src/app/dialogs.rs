use super::list_state::SearchableListState;
use super::{App, DialogKind, FacetDialogRow};
use crate::facet::{FacetGroup, FacetKind, escape_facet_text};

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
    facets: SearchableListState,
    /// Present while the facet dialog is drilled into one property key's values.
    facet_values: Option<SearchableListState>,
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

    pub(super) fn facet_values_open(&self) -> bool {
        self.facet_values.is_some()
    }

    /// Start a fresh facet dialog at the root view with an empty query.
    pub(super) fn reset_facets(&mut self) {
        self.facets = SearchableListState::default();
        self.facet_values = None;
    }

    /// Switch to a fresh property-value view; the root query and selection are kept.
    pub(super) fn open_facet_values(&mut self) {
        self.facet_values = Some(SearchableListState::default());
    }

    /// Return to the root view. Returns `false` when already at the root.
    pub(super) fn close_facet_values(&mut self) -> bool {
        self.facet_values.take().is_some()
    }

    /// Select `index` in the root facet view, clamped to `len` rows.
    pub(super) fn select_facet_root(&mut self, index: usize, len: usize) {
        self.facets.move_up(usize::MAX);
        self.facets.move_down(index, len);
    }

    fn state(&self, kind: DialogKind) -> &SearchableListState {
        match kind {
            DialogKind::PropertyFilters => &self.property_filters,
            DialogKind::MessageFields => &self.message_fields,
            DialogKind::FilterPresets => &self.filter_presets,
            DialogKind::Sources => &self.sources,
            DialogKind::Facets => self.facet_values.as_ref().unwrap_or(&self.facets),
        }
    }

    fn state_mut(&mut self, kind: DialogKind) -> &mut SearchableListState {
        match kind {
            DialogKind::PropertyFilters => &mut self.property_filters,
            DialogKind::MessageFields => &mut self.message_fields,
            DialogKind::FilterPresets => &mut self.filter_presets,
            DialogKind::Sources => &mut self.sources,
            DialogKind::Facets => match &mut self.facet_values {
                Some(values) => values,
                None => &mut self.facets,
            },
        }
    }
}

/// Facet buckets captured when the facet dialog opens (and refreshed on drilldown), so
/// rows stay stable while new lines arrive and searching never rescans the buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct FacetSnapshot {
    pub(super) root_rows: Vec<FacetDialogRow>,
    pub(super) root_summary: String,
    pub(super) property_key: Option<String>,
    pub(super) value_rows: Vec<FacetDialogRow>,
    pub(super) value_summary: String,
}

impl FacetSnapshot {
    pub(super) fn from_groups(groups: &[FacetGroup]) -> Self {
        let value_group = facet_group(groups, FacetKind::PropertyValue);
        Self {
            root_rows: [FacetKind::Source, FacetKind::Level, FacetKind::PropertyKey]
                .into_iter()
                .filter_map(|kind| facet_group(groups, kind))
                .flat_map(facet_group_rows)
                .collect(),
            root_summary: facet_root_summary(groups),
            property_key: value_group.and_then(|group| group.property_key.clone()),
            value_rows: value_group.map(facet_group_rows).unwrap_or_default(),
            value_summary: value_group.map(facet_value_summary).unwrap_or_default(),
        }
    }

    pub(super) fn rows(&self, values: bool) -> &[FacetDialogRow] {
        if values {
            &self.value_rows
        } else {
            &self.root_rows
        }
    }

    pub(super) fn summary(&self, values: bool) -> &str {
        if values {
            &self.value_summary
        } else {
            &self.root_summary
        }
    }
}

pub(super) fn facet_row_matches(row: &FacetDialogRow, query: &str) -> bool {
    use crate::filter::contains_ignore_ascii_case;

    query.is_empty()
        || contains_ignore_ascii_case(&row.value, query)
        || contains_ignore_ascii_case(&escape_facet_text(&row.value), query)
        || contains_ignore_ascii_case(row.facet.as_str(), query)
        || row
            .value_types
            .iter()
            .any(|value_type| contains_ignore_ascii_case(value_type.as_str(), query))
}

fn facet_group(groups: &[FacetGroup], kind: FacetKind) -> Option<&FacetGroup> {
    groups.iter().find(|group| group.facet == kind)
}

fn facet_group_rows(group: &FacetGroup) -> Vec<FacetDialogRow> {
    group
        .buckets
        .iter()
        .map(|bucket| FacetDialogRow {
            facet: group.facet,
            value: bucket.value.clone(),
            count: bucket.count,
            value_types: bucket.value_types.clone(),
        })
        .collect()
}

fn facet_root_summary(groups: &[FacetGroup]) -> String {
    let Some(first) = groups.first() else {
        return String::new();
    };
    let shown = |kind| facet_group(groups, kind).map_or(0, |group| group.buckets.len());
    let total = |kind| facet_group(groups, kind).map_or(0, |group| group.total_buckets);
    format!(
        "win={}/{}{} src={}/{} lvl={}/{} key={}/{}",
        first.window_records,
        first.available_records,
        facet_window_suffix(first.window_truncated),
        shown(FacetKind::Source),
        total(FacetKind::Source),
        shown(FacetKind::Level),
        total(FacetKind::Level),
        shown(FacetKind::PropertyKey),
        total(FacetKind::PropertyKey),
    )
}

fn facet_value_summary(group: &FacetGroup) -> String {
    const MAX_SUMMARY_CHARS: usize = 80;
    let prefix = format!(
        "win={}/{}{} val={}/{} key=",
        group.window_records,
        group.available_records,
        facet_window_suffix(group.window_truncated),
        group.buckets.len(),
        group.total_buckets,
    );
    let escaped_key = escape_facet_text(group.property_key.as_deref().unwrap_or_default());
    let key_budget = MAX_SUMMARY_CHARS.saturating_sub(prefix.chars().count());
    format!(
        "{prefix}{}",
        truncate_facet_summary_suffix(&escaped_key, key_budget)
    )
}

fn facet_window_suffix(truncated: bool) -> &'static str {
    if truncated { " clipped" } else { "" }
}

fn truncate_facet_summary_suffix(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_string();
    }
    if maximum == 0 {
        return String::new();
    }

    let mut truncated = value.chars().take(maximum - 1).collect::<String>();
    truncated.push('~');
    truncated
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
        DialogKind::Facets => app
            .facet_snapshot
            .rows(app.dialogs.facet_values_open())
            .iter()
            .filter(|row| facet_row_matches(row, query))
            .count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facet::FacetBucket;
    use crate::filter::{PropertyFilterUpdate, PropertyPredicate};
    use crate::model::Level;

    use crate::app::{Mode, PromptKind};

    fn select_facet_row(app: &mut App, facet: FacetKind, value: &str) {
        let index = app
            .facet_rows()
            .iter()
            .position(|row| row.facet == facet && row.value == value)
            .unwrap();
        app.move_dialog_up(DialogKind::Facets, usize::MAX);
        app.move_dialog_down(DialogKind::Facets, index);
    }

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

    #[test]
    fn facet_dialog_snapshots_once_and_searches_stored_rows() {
        let mut app = App::new(20);
        app.push_line("api | INFO one tenant=one".to_string());
        app.push_line("web | ERROR two region=eu".to_string());
        app.open_dialog(DialogKind::Facets);

        let summary = app.facet_dialog_summary().to_string();
        let rows = app.facet_rows().into_iter().cloned().collect::<Vec<_>>();
        assert!(summary.contains("win=2/2"));

        app.push_line("worker | WARN three new=value".to_string());
        assert_eq!(app.facet_dialog_summary(), summary);
        assert_eq!(
            app.facet_rows().into_iter().cloned().collect::<Vec<_>>(),
            rows
        );

        for character in "api".chars() {
            app.push_dialog_query_char(DialogKind::Facets, character);
        }
        let filtered = app.facet_rows();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].facet, FacetKind::Source);
        assert_eq!(filtered[0].value, "api");
        assert_eq!(app.facet_dialog_summary(), summary);
    }

    #[test]
    fn facet_dialog_counts_respect_other_active_filters() {
        let mut app = App::new(20);
        app.push_line("api | ERROR one".to_string());
        app.push_line("api | INFO two".to_string());
        app.push_line("web | ERROR three".to_string());
        app.filters_mut().level = Some(Level::Error);
        app.filters_mut().source = Some("api".to_string());

        app.open_dialog(DialogKind::Facets);

        let counts = app
            .facet_rows()
            .iter()
            .map(|row| (row.facet, row.value.clone(), row.count))
            .collect::<Vec<_>>();
        assert!(counts.contains(&(FacetKind::Source, "api".to_string(), 1)));
        assert!(counts.contains(&(FacetKind::Source, "web".to_string(), 1)));
        assert!(counts.contains(&(FacetKind::Level, "error".to_string(), 1)));
        assert!(counts.contains(&(FacetKind::Level, "info".to_string(), 1)));
    }

    #[test]
    fn facet_property_drilldown_refreshes_all_groups_and_preserves_root_state() {
        let mut app = App::new(20);
        app.push_line("api | INFO row tenant=one region=eu".to_string());
        app.open_dialog(DialogKind::Facets);
        for character in "property_key".chars() {
            app.push_dialog_query_char(DialogKind::Facets, character);
        }
        assert_eq!(app.facet_rows().len(), 2);
        select_facet_row(&mut app, FacetKind::PropertyKey, "tenant");
        assert_eq!(app.selected_dialog_index(DialogKind::Facets), 1);

        app.push_line("web | ERROR row tenant=two".to_string());
        app.activate_selected_dialog_row(DialogKind::Facets);

        assert!(app.facet_dialog_is_drilldown());
        assert_eq!(app.dialog_query(DialogKind::Facets), "");
        assert!(app.facet_dialog_summary().contains("win=2/2"));
        assert!(app.facet_dialog_summary().contains("val=2/2"));
        assert_eq!(
            app.facet_rows()
                .iter()
                .map(|row| row.value.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );

        app.delete_selected_dialog_row(DialogKind::Facets);
        assert!(!app.facet_dialog_is_drilldown());
        assert_eq!(app.dialog_query(DialogKind::Facets), "property_key");
        assert_eq!(app.selected_dialog_index(DialogKind::Facets), 0);
        let selected = app.selected_facet_row().unwrap();
        assert_eq!(selected.facet, FacetKind::PropertyKey);
        assert_eq!(selected.value, "tenant");
        assert!(app.facet_dialog_summary().contains("src=2/2"));
        assert_eq!(app.facet_rows().len(), 2);

        app.activate_selected_dialog_row(DialogKind::Facets);
        assert!(app.facet_dialog_is_drilldown());
        assert!(app.facet_dialog_summary().contains("key=tenant"));
    }

    #[test]
    fn facet_source_and_level_choices_apply_with_undo_and_visible_resync() {
        let mut app = App::new(20);
        app.push_line("api | INFO one".to_string());
        app.push_line("web | ERROR two".to_string());

        app.open_dialog(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::Source, "web");
        app.activate_selected_dialog_row(DialogKind::Facets);
        assert_eq!(app.filters().source.as_deref(), Some("web"));
        assert_eq!(app.filter_history_len(), 1);
        assert_eq!(app.visible_count(), 1);

        app.undo_filter_change();
        assert_eq!(app.filters().source, None);
        assert_eq!(app.visible_count(), 2);

        app.open_dialog(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::Level, "error");
        app.activate_selected_dialog_row(DialogKind::Facets);
        assert_eq!(app.filters().level, Some(Level::Error));
        assert_eq!(app.filter_history_len(), 1);
        assert_eq!(app.visible_count(), 1);

        app.open_dialog(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::Level, "error");
        app.activate_selected_dialog_row(DialogKind::Facets);
        assert_eq!(app.filter_history_len(), 1);
        app.undo_filter_change();
        assert_eq!(app.filters().level, None);
        assert_eq!(app.visible_count(), 2);
    }

    #[test]
    fn facet_property_value_choice_replaces_same_key_filters_and_is_undoable() {
        let mut app = App::new(20);
        app.push_line("api | INFO row tenant=one region=eu".to_string());
        app.push_line("api | INFO row tenant=two region=eu".to_string());
        app.filters_mut()
            .property_excludes
            .push(PropertyPredicate::exact("tenant", "two"));
        app.filters_mut()
            .property_includes
            .push(PropertyPredicate::exact("region", "eu"));

        app.open_dialog(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::PropertyKey, "tenant");
        app.activate_selected_dialog_row(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::PropertyValue, "two");
        app.activate_selected_dialog_row(DialogKind::Facets);

        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.filter_history_len(), 1);
        assert_eq!(
            app.filters().property_includes,
            vec![
                PropertyPredicate::exact("region", "eu"),
                PropertyPredicate::exact("tenant", "two")
            ]
        );
        assert!(app.filters().property_excludes.is_empty());
        assert_eq!(app.visible_count(), 1);
        assert_eq!(
            app.visible_event_at(0)
                .unwrap()
                .property("tenant")
                .unwrap()
                .value
                .to_string(),
            "two"
        );

        app.undo_filter_change();
        assert_eq!(
            app.filters().property_excludes,
            vec![PropertyPredicate::exact("tenant", "two")]
        );
    }

    #[test]
    fn facet_reselection_semantic_no_ops_do_not_add_history() {
        let mut app = App::new(20);
        app.push_line("API | INFO row tenant=one region=eu".to_string());
        app.filters_mut().source = Some("api".to_string());
        app.open_dialog(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::Source, "api");
        app.activate_selected_dialog_row(DialogKind::Facets);
        assert_eq!(app.filter_history_len(), 0);

        app.filters_mut().property_includes = vec![
            PropertyPredicate::exact("region", "eu"),
            PropertyPredicate::exact("tenant", "one"),
        ];
        app.open_dialog(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::PropertyKey, "tenant");
        app.activate_selected_dialog_row(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::PropertyValue, "one");
        app.activate_selected_dialog_row(DialogKind::Facets);
        assert_eq!(app.filter_history_len(), 0);
    }

    #[test]
    fn facet_summaries_disclose_pre_search_bucket_clipping() {
        let mut app = App::new(200);
        for index in 0..101 {
            app.push_line(format!("source-{index:03} | INFO row"));
        }
        app.open_dialog(DialogKind::Facets);

        let summary = app.facet_dialog_summary().to_string();
        assert!(summary.contains("src=100/101"));
        for character in "source-000".chars() {
            app.push_dialog_query_char(DialogKind::Facets, character);
        }
        assert_eq!(app.facet_rows().len(), 1);
        assert_eq!(app.facet_dialog_summary(), summary);
    }

    #[test]
    fn facet_summaries_keep_all_counts_within_wide_dialog_content() {
        fn group(facet: FacetKind, shown: usize, total: usize) -> FacetGroup {
            FacetGroup {
                schema_version: 1,
                facet,
                property_key: None,
                available_records: 100_001,
                window_records: 100_000,
                window_truncated: true,
                matched_records: 100_000,
                eligible_records: 100_000,
                total_buckets: total,
                truncated: shown < total,
                buckets: (0..shown)
                    .map(|index| FacetBucket {
                        value: format!("value-{index}"),
                        count: 1,
                        value_types: Vec::new(),
                    })
                    .collect(),
            }
        }

        let groups = [
            group(FacetKind::Source, 100, 100_000),
            group(FacetKind::Level, 7, 7),
            group(FacetKind::PropertyKey, 100, 100_000),
        ];
        let summary = facet_root_summary(&groups);

        assert_eq!(
            summary,
            "win=100000/100001 clipped src=100/100000 lvl=7/7 key=100/100000"
        );
        assert!(summary.chars().count() <= 80);

        let property_key = format!("{}{}", r"tenant\\segment".repeat(12), "\n".repeat(12));
        let mut value_group = group(FacetKind::PropertyValue, 100, 100_000);
        value_group.property_key = Some(property_key.clone());
        let summary = facet_value_summary(&value_group);

        assert!(summary.starts_with("win=100000/100001 clipped val=100/100000 key="));
        assert_eq!(summary.chars().count(), 80);
        assert!(summary.ends_with('~'));
        assert!(!summary.contains(&escape_facet_text(&property_key)));
    }

    #[test]
    fn facet_rows_keep_raw_values_but_escape_literal_and_control_text_distinctly() {
        let mut app = App::new(20);
        app.push_line(r#"api | {"message":"literal","value":"\\n"}"#.to_string());
        app.push_line(r#"api | {"message":"control","value":"\n"}"#.to_string());
        app.open_dialog(DialogKind::Facets);
        select_facet_row(&mut app, FacetKind::PropertyKey, "value");
        app.activate_selected_dialog_row(DialogKind::Facets);

        let rendered = app
            .facet_rows()
            .iter()
            .map(|row| escape_facet_text(&row.value))
            .collect::<Vec<_>>();
        assert!(rendered.contains(&r"\n".to_string()));
        assert!(rendered.contains(&r"\\n".to_string()));
        assert_ne!(rendered[0], rendered[1]);

        for character in r"\n".chars() {
            app.push_dialog_query_char(DialogKind::Facets, character);
        }
        assert_eq!(app.facet_rows().len(), 2);
        select_facet_row(&mut app, FacetKind::PropertyValue, "\n");
        app.activate_selected_dialog_row(DialogKind::Facets);
        assert_eq!(
            app.filters().property_includes,
            vec![PropertyPredicate::exact("value", "\n")]
        );
    }
}
