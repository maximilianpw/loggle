mod list_state;

use crate::buffer::LogBuffer;
use crate::commands::{Command, COMMANDS};
use crate::filter::{
    event_contains, LogFilter, PropertyFilterId, PropertyFilterUpdate, PropertyPredicate,
};
use crate::model::{Level, LogEvent, LogProperty, SourceConfig};

use list_state::SearchableListState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Text,
    Source,
    Level,
    IncludeProperty,
    ExcludeProperty,
    EditPropertyFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prompt(PromptKind),
    Palette,
    PropertyFilters,
    MessageFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyFilterRow {
    pub id: PropertyFilterId,
    pub kind: &'static str,
    pub summary: String,
}

#[derive(Debug)]
pub struct App {
    buffer: LogBuffer,
    filters: LogFilter,
    selected: usize,
    follow: bool,
    mode: Mode,
    prompt: String,
    palette: SearchableListState,
    pending_g: bool,
    details_open: bool,
    selected_property: usize,
    property_filters: SearchableListState,
    editing_property_filter: Option<PropertyFilterId>,
    message_field_keys: Vec<String>,
    message_fields: SearchableListState,
}

impl App {
    #[cfg(test)]
    pub fn new(buffer_lines: usize) -> Self {
        Self::with_source_config(buffer_lines, SourceConfig::default())
    }

    pub fn with_source_config(buffer_lines: usize, source_config: SourceConfig) -> Self {
        Self {
            buffer: LogBuffer::with_source_config(buffer_lines, source_config),
            filters: LogFilter::default(),
            selected: 0,
            follow: true,
            mode: Mode::Normal,
            prompt: String::new(),
            palette: SearchableListState::default(),
            pending_g: false,
            details_open: false,
            selected_property: 0,
            property_filters: SearchableListState::default(),
            editing_property_filter: None,
            message_field_keys: Vec::new(),
            message_fields: SearchableListState::default(),
        }
    }

    pub fn push_line(&mut self, line: String) {
        self.buffer.push_line(line);
        self.sync_selection();
    }

    pub fn retained_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn palette_selected(&self) -> usize {
        self.palette.selected()
    }

    pub fn palette_commands(&self) -> &'static [Command] {
        COMMANDS
    }

    pub fn selected_palette_command(&self) -> Option<&'static Command> {
        self.palette_commands().get(self.palette.selected())
    }

    pub fn property_filter_query(&self) -> &str {
        self.property_filters.query()
    }

    pub fn selected_property_filter_index(&self) -> usize {
        self.property_filters.selected()
    }

    pub fn property_filter_rows(&self) -> Vec<PropertyFilterRow> {
        let query = self.property_filters.query().trim();
        self.all_property_filter_rows()
            .into_iter()
            .filter(|row| property_filter_row_matches(row, query))
            .collect()
    }

    pub fn selected_property_filter_row(&self) -> Option<PropertyFilterRow> {
        self.property_filter_rows()
            .get(self.property_filters.selected())
            .cloned()
    }

    pub fn message_field_keys(&self) -> &[String] {
        &self.message_field_keys
    }

    pub fn message_field_query(&self) -> &str {
        self.message_fields.query()
    }

    pub fn selected_message_field_index(&self) -> usize {
        self.message_fields.selected()
    }

    pub fn message_field_rows(&self) -> Vec<&str> {
        let query = self.message_fields.query().trim();
        self.message_field_keys
            .iter()
            .map(String::as_str)
            .filter(|key| {
                query.is_empty() || crate::filter::contains_ignore_ascii_case(key, query)
            })
            .collect()
    }

    pub fn selected_message_field_key(&self) -> Option<&str> {
        self.message_field_rows()
            .get(self.message_fields.selected())
            .copied()
    }

    pub fn filters(&self) -> &LogFilter {
        &self.filters
    }

    pub fn details_open(&self) -> bool {
        self.details_open
    }

    pub fn selected_property_index(&self) -> usize {
        self.selected_property
    }

    pub fn visible_indices(&self) -> Vec<usize> {
        self.buffer
            .events()
            .iter()
            .enumerate()
            .filter_map(|(index, event)| self.filters.matches(event).then_some(index))
            .collect()
    }

    pub fn visible_events(&self) -> Vec<&LogEvent> {
        self.visible_indices()
            .into_iter()
            .filter_map(|index| self.buffer.events().get(index))
            .collect()
    }

    pub fn selected_event(&self) -> Option<&LogEvent> {
        self.visible_events().get(self.selected).copied()
    }

    pub fn selected_property(&self) -> Option<&LogProperty> {
        self.selected_event()
            .and_then(|event| event.properties.get(self.selected_property))
    }

    #[cfg(test)]
    pub fn event_at_visible(&self, visible_index: usize) -> Option<&LogEvent> {
        let buffer_index = self.visible_indices().get(visible_index).copied()?;
        self.buffer.events().get(buffer_index)
    }

    pub fn move_down(&mut self, amount: usize) {
        self.follow = false;
        self.pending_g = false;
        let visible_len = self.visible_indices().len();
        if visible_len == 0 {
            self.selected = 0;
        } else {
            self.selected = (self.selected + amount).min(visible_len - 1);
        }
        self.sync_selected_property();
    }

    pub fn move_up(&mut self, amount: usize) {
        self.follow = false;
        self.pending_g = false;
        self.selected = self.selected.saturating_sub(amount);
        self.sync_selected_property();
    }

    pub fn jump_top(&mut self) {
        self.follow = false;
        self.pending_g = false;
        self.selected = 0;
        self.sync_selected_property();
    }

    pub fn jump_bottom(&mut self) {
        self.follow = true;
        self.pending_g = false;
        self.sync_selection();
    }

    pub fn toggle_follow(&mut self) {
        self.pending_g = false;
        if self.follow {
            self.follow = false;
        } else {
            self.jump_bottom();
        }
    }

    pub fn start_prompt(&mut self, kind: PromptKind) {
        self.pending_g = false;
        self.prompt = match kind {
            PromptKind::Text => self.filters.text.clone().unwrap_or_default(),
            PromptKind::Source => self.filters.source.clone().unwrap_or_default(),
            PromptKind::Level => self
                .filters
                .level
                .map(|level| level.to_string())
                .unwrap_or_default(),
            PromptKind::IncludeProperty | PromptKind::ExcludeProperty => self
                .selected_property()
                .map(property_prompt_value)
                .unwrap_or_default(),
            PromptKind::EditPropertyFilter => String::new(),
        };
        self.editing_property_filter = None;
        self.mode = Mode::Prompt(kind);
    }

    pub fn start_property_filter_edit(&mut self) {
        let Some(row) = self.selected_property_filter_row() else {
            return;
        };
        let Some(predicate) = self.filters.property_filter(row.id) else {
            self.sync_property_filter_selection();
            return;
        };

        self.prompt = predicate.summary_for(row.id.exclude);
        self.editing_property_filter = Some(row.id);
        self.mode = Mode::Prompt(PromptKind::EditPropertyFilter);
        self.pending_g = false;
    }

    pub fn open_palette(&mut self) {
        self.mode = Mode::Palette;
        self.prompt.clear();
        self.pending_g = false;
        self.sync_palette_selection();
    }

    pub fn close_palette(&mut self) {
        self.mode = Mode::Normal;
        self.pending_g = false;
    }

    pub fn toggle_palette(&mut self) {
        if self.mode == Mode::Palette {
            self.close_palette();
        } else {
            self.open_palette();
        }
    }

    pub fn move_palette_down(&mut self, amount: usize) {
        self.palette.move_down(amount, self.palette_commands().len());
    }

    pub fn move_palette_up(&mut self, amount: usize) {
        self.palette.move_up(amount);
    }

    pub fn open_property_filters(&mut self) {
        self.mode = Mode::PropertyFilters;
        self.prompt.clear();
        self.pending_g = false;
        self.editing_property_filter = None;
        self.sync_property_filter_selection();
    }

    pub fn close_property_filters(&mut self) {
        self.mode = Mode::Normal;
        self.pending_g = false;
    }

    pub fn open_message_fields(&mut self) {
        self.mode = Mode::MessageFields;
        self.prompt.clear();
        self.pending_g = false;
        self.sync_message_field_selection();
    }

    pub fn close_message_fields(&mut self) {
        self.mode = Mode::Normal;
        self.pending_g = false;
    }

    pub fn move_property_filter_down(&mut self, amount: usize) {
        self.property_filters
            .move_down(amount, self.property_filter_rows().len());
    }

    pub fn move_property_filter_up(&mut self, amount: usize) {
        self.property_filters.move_up(amount);
    }

    pub fn push_property_filter_query_char(&mut self, value: char) {
        self.property_filters.push_query_char(value);
        self.sync_property_filter_selection();
    }

    pub fn pop_property_filter_query_char(&mut self) {
        self.property_filters.pop_query_char();
        self.sync_property_filter_selection();
    }

    pub fn delete_selected_property_filter(&mut self) {
        let Some(row) = self.selected_property_filter_row() else {
            return;
        };

        self.filters.remove_property_filter(row.id);
        self.sync_property_filter_selection();
        self.sync_selection();
    }

    pub fn move_message_field_down(&mut self, amount: usize) {
        self.message_fields
            .move_down(amount, self.message_field_rows().len());
    }

    pub fn move_message_field_up(&mut self, amount: usize) {
        self.message_fields.move_up(amount);
    }

    pub fn push_message_field_query_char(&mut self, value: char) {
        self.message_fields.push_query_char(value);
        self.sync_message_field_selection();
    }

    pub fn pop_message_field_query_char(&mut self) {
        self.message_fields.pop_query_char();
        self.sync_message_field_selection();
    }

    pub fn add_selected_message_field(&mut self) {
        self.pending_g = false;
        let Some(key) = self.selected_property().map(|property| property.key.clone()) else {
            return;
        };

        if !self.message_field_keys.iter().any(|existing| existing == &key) {
            self.message_field_keys.push(key);
        }
        self.sync_message_field_selection();
    }

    pub fn delete_selected_message_field(&mut self) {
        let Some(key) = self.selected_message_field_key().map(str::to_string) else {
            return;
        };
        if let Some(index) = self
            .message_field_keys
            .iter()
            .position(|candidate| candidate == &key)
        {
            self.message_field_keys.remove(index);
        }
        self.sync_message_field_selection();
    }

    pub fn push_prompt_char(&mut self, value: char) {
        self.prompt.push(value);
    }

    pub fn pop_prompt_char(&mut self) {
        self.prompt.pop();
    }

    pub fn cancel_prompt(&mut self) {
        self.mode = if self.editing_property_filter.is_some() {
            Mode::PropertyFilters
        } else {
            Mode::Normal
        };
        self.prompt.clear();
        self.pending_g = false;
        self.editing_property_filter = None;
    }

    pub fn clear_transient(&mut self) {
        self.pending_g = false;
    }

    pub fn apply_prompt(&mut self) {
        let Mode::Prompt(kind) = self.mode else {
            return;
        };

        let value = self.prompt.trim().to_string();
        match kind {
            PromptKind::Text => self.filters.text = (!value.is_empty()).then_some(value),
            PromptKind::Source => self.filters.source = (!value.is_empty()).then_some(value),
            PromptKind::Level => {
                self.filters.level = if value.is_empty() {
                    None
                } else {
                    Level::parse(&value)
                };
            }
            PromptKind::IncludeProperty => {
                if let Some(update) = PropertyFilterUpdate::parse(&value, false) {
                    self.filters.add_property_filter(update);
                }
            }
            PromptKind::ExcludeProperty => {
                if let Some(update) = PropertyFilterUpdate::parse(&value, true) {
                    self.filters.add_property_filter(update);
                }
            }
            PromptKind::EditPropertyFilter => {
                if let Some(update) = PropertyFilterUpdate::parse(&value, false) {
                    if let Some(id) = self.editing_property_filter {
                        self.filters.replace_property_filter(id, update);
                    }
                }
            }
        }

        self.mode = if matches!(kind, PromptKind::EditPropertyFilter) {
            Mode::PropertyFilters
        } else {
            Mode::Normal
        };
        self.prompt.clear();
        self.editing_property_filter = None;
        self.sync_property_filter_selection();
        self.sync_selection();
    }

    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.pending_g = false;
        self.sync_selection();
    }

    pub fn toggle_details(&mut self) {
        self.pending_g = false;
        self.details_open = !self.details_open && self.selected_event().is_some();
        self.sync_selected_property();
    }

    pub fn next_property(&mut self) {
        self.pending_g = false;
        let Some(event) = self.selected_event() else {
            self.selected_property = 0;
            return;
        };

        if !event.properties.is_empty() {
            self.selected_property = (self.selected_property + 1).min(event.properties.len() - 1);
        }
    }

    pub fn previous_property(&mut self) {
        self.pending_g = false;
        self.selected_property = self.selected_property.saturating_sub(1);
    }

    pub fn follow_selected_property(&mut self) {
        self.pending_g = false;
        let Some(property) = self.selected_property() else {
            return;
        };
        let predicate = PropertyPredicate::exact(&property.key, property.value.to_string());
        self.filters.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate,
        });
        self.sync_selection();
    }

    pub fn handle_g(&mut self) {
        if self.pending_g {
            self.jump_top();
        } else {
            self.pending_g = true;
        }
    }

    pub fn next_search_match(&mut self) {
        self.move_to_search_match(true);
    }

    pub fn previous_search_match(&mut self) {
        self.move_to_search_match(false);
    }

    fn move_to_search_match(&mut self, forward: bool) {
        self.pending_g = false;
        let Some(query) = self.filters.text.as_ref().filter(|query| !query.is_empty()) else {
            return;
        };
        self.follow = false;

        let visible = self.visible_events();
        if visible.is_empty() {
            return;
        }

        for step in 1..=visible.len() {
            let index = if forward {
                (self.selected + step) % visible.len()
            } else {
                (self.selected + visible.len() - (step % visible.len())) % visible.len()
            };

            if event_contains(visible[index], query) {
                self.selected = index;
                self.sync_selected_property();
                return;
            }
        }
    }

    fn sync_selection(&mut self) {
        let visible_len = self.visible_indices().len();
        if visible_len == 0 {
            self.selected = 0;
        } else if self.follow {
            self.selected = visible_len - 1;
        } else {
            self.selected = self.selected.min(visible_len - 1);
        }
        self.sync_selected_property();
    }

    fn sync_selected_property(&mut self) {
        let property_len = self
            .selected_event()
            .map(|event| event.properties.len())
            .unwrap_or_default();

        if property_len == 0 {
            self.selected_property = 0;
        } else {
            self.selected_property = self.selected_property.min(property_len - 1);
        }
    }

    fn sync_palette_selection(&mut self) {
        self.palette.sync(self.palette_commands().len());
    }

    fn sync_property_filter_selection(&mut self) {
        let len = self.property_filter_rows().len();
        self.property_filters.sync(len);
    }

    fn sync_message_field_selection(&mut self) {
        let len = self.message_field_rows().len();
        self.message_fields.sync(len);
    }

    fn all_property_filter_rows(&self) -> Vec<PropertyFilterRow> {
        self.filters
            .property_includes
            .iter()
            .enumerate()
            .map(|(index, predicate)| PropertyFilterRow {
                id: PropertyFilterId {
                    exclude: false,
                    index,
                },
                kind: "show",
                summary: predicate.summary_for(false),
            })
            .chain(
                self.filters
                    .property_excludes
                    .iter()
                    .enumerate()
                    .map(|(index, predicate)| PropertyFilterRow {
                        id: PropertyFilterId {
                            exclude: true,
                            index,
                        },
                        kind: "ignore",
                        summary: predicate.summary_for(true),
                    }),
            )
            .collect()
    }
}

fn property_prompt_value(property: &LogProperty) -> String {
    format!("{}={}", property.key, property.value)
}

fn property_filter_row_matches(row: &PropertyFilterRow, query: &str) -> bool {
    query.is_empty()
        || crate::filter::contains_ignore_ascii_case(row.kind, query)
        || crate::filter::contains_ignore_ascii_case(&row.summary, query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_mode_tracks_bottom_as_lines_arrive() {
        let mut app = App::new(10);

        app.push_line("api | one".to_string());
        app.push_line("api | two".to_string());

        assert!(app.is_following());
        assert_eq!(app.selected(), 1);
    }

    #[test]
    fn manual_navigation_disables_follow_mode() {
        let mut app = App::new(10);
        app.push_line("api | one".to_string());
        app.push_line("api | two".to_string());

        app.move_up(1);
        app.push_line("api | three".to_string());

        assert!(!app.is_following());
        assert_eq!(app.selected(), 0);
    }

    #[test]
    fn jumping_to_bottom_reenables_follow_mode() {
        let mut app = App::new(10);
        app.push_line("api | one".to_string());
        app.push_line("api | two".to_string());
        app.move_up(1);

        app.jump_bottom();
        app.push_line("api | three".to_string());

        assert!(app.is_following());
        assert_eq!(app.selected(), 2);
    }

    #[test]
    fn search_navigation_finds_next_and_previous_matches() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("api | ok".to_string());
        app.push_line("web | ERROR two".to_string());
        app.filters.text = Some("error".to_string());
        app.jump_top();

        app.next_search_match();
        assert_eq!(app.event_at_visible(app.selected()).unwrap().raw, "web | ERROR two");

        app.previous_search_match();
        assert_eq!(app.event_at_visible(app.selected()).unwrap().raw, "api | ERROR one");
    }

    #[test]
    fn details_mode_tracks_selected_property() {
        let mut app = App::new(10);
        app.push_line("14:06:58.892 INFO request completed".to_string());
        app.push_line("[14:06:58.892] INFO (#1):".to_string());
        app.push_line("{".to_string());
        app.push_line("requestId: \"abc\",".to_string());
        app.push_line("tenantId: \"tenant-1\",".to_string());
        app.push_line("}".to_string());

        app.toggle_details();
        app.next_property();

        assert!(app.details_open());
        assert_eq!(app.selected_property().unwrap().key, "tenantId");
    }

    #[test]
    fn follow_selected_property_adds_include_filter() {
        let mut app = App::new(10);
        app.push_line("14:06:58.892 INFO request completed".to_string());
        app.push_line("[14:06:58.892] INFO (#1):".to_string());
        app.push_line("{".to_string());
        app.push_line("tenantId: \"tenant-1\",".to_string());
        app.push_line("}".to_string());

        app.follow_selected_property();

        assert_eq!(
            app.filters.property_includes,
            vec![PropertyPredicate::exact("tenantId", "tenant-1")]
        );
    }

    #[test]
    fn adding_selected_message_field_uses_property_key_once() {
        let mut app = App::new(10);
        app.push_line("14:06:58.892 INFO request completed".to_string());
        app.push_line("[14:06:58.892] INFO (#1):".to_string());
        app.push_line("{".to_string());
        app.push_line("tenantId: \"tenant-1\",".to_string());
        app.push_line("requestId: \"abc\",".to_string());
        app.push_line("}".to_string());

        app.add_selected_message_field();
        app.add_selected_message_field();
        app.next_property();
        app.add_selected_message_field();

        assert_eq!(
            app.message_field_keys(),
            &["tenantId".to_string(), "requestId".to_string()]
        );
    }

    #[test]
    fn message_field_dialog_searches_and_deletes_selected_fields() {
        let mut app = App::new(10);
        app.message_field_keys = vec![
            "tenantId".to_string(),
            "requestId".to_string(),
            "durationMs".to_string(),
        ];

        app.open_message_fields();
        app.push_message_field_query_char('r');
        app.push_message_field_query_char('e');

        assert_eq!(app.mode(), &Mode::MessageFields);
        assert_eq!(app.message_field_rows(), vec!["requestId"]);

        app.delete_selected_message_field();

        assert_eq!(
            app.message_field_keys(),
            &["tenantId".to_string(), "durationMs".to_string()]
        );
        assert_eq!(app.mode(), &Mode::MessageFields);
        assert_eq!(app.selected_message_field_index(), 0);
    }

    #[test]
    fn message_field_dialog_backspace_deletes_when_search_is_empty() {
        let mut app = App::new(10);
        app.message_field_keys = vec!["tenantId".to_string()];

        app.open_message_fields();
        app.delete_selected_message_field();

        assert!(app.message_field_keys().is_empty());
        assert_eq!(app.mode(), &Mode::MessageFields);
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
        app.filters.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("tenantId", "tenant-1"),
        });
        app.filters.add_property_filter(PropertyFilterUpdate {
            exclude: true,
            predicate: PropertyPredicate::exists("debug"),
        });

        app.open_property_filters();
        app.push_property_filter_query_char('i');
        app.push_property_filter_query_char('g');

        assert_eq!(app.mode(), &Mode::PropertyFilters);
        let rows = app.property_filter_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "ignore");
        assert_eq!(rows[0].summary, "!debug");
    }

    #[test]
    fn deleting_selected_property_filter_removes_it() {
        let mut app = App::new(10);
        app.filters.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("tenantId", "tenant-1"),
        });
        app.filters.add_property_filter(PropertyFilterUpdate {
            exclude: true,
            predicate: PropertyPredicate::exists("debug"),
        });

        app.open_property_filters();
        app.move_property_filter_down(1);
        app.delete_selected_property_filter();

        assert_eq!(
            app.filters.property_includes,
            vec![PropertyPredicate::exact("tenantId", "tenant-1")]
        );
        assert!(app.filters.property_excludes.is_empty());
        assert_eq!(app.mode(), &Mode::PropertyFilters);
        assert_eq!(app.selected_property_filter_index(), 0);
    }

    #[test]
    fn editing_property_filter_replaces_existing_filter() {
        let mut app = App::new(10);
        app.filters.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("tenantId", "tenant-1"),
        });

        app.open_property_filters();
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

        assert_eq!(app.mode(), &Mode::PropertyFilters);
        assert!(app.filters.property_includes.is_empty());
        assert_eq!(
            app.filters.property_excludes,
            vec![PropertyPredicate::exact("tenantId", "tenant-2")]
        );
    }
}
