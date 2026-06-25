mod list_state;
mod visible;

use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Write},
    path::Path,
};

use crate::buffer::{BufferChange, LogBuffer};
use crate::commands::{COMMANDS, Command};
use crate::filter::{
    FilterEdit, FilterPresetRow, FilterWorkflow, LogFilter, PropertyFilterId, PropertyFilterRow,
};
use crate::model::{Level, LogEvent, LogProperty, SourceConfig};

use list_state::SearchableListState;
use visible::VisibleLogView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Text,
    Source,
    Level,
    IncludeProperty,
    ExcludeProperty,
    EditPropertyFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    PropertyFilters,
    DisplayFields,
    FilterPresets,
    Sources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual,
    Prompt(PromptKind),
    Palette,
    Dialog(DialogKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStatusRow {
    pub source: String,
    pub count: usize,
    pub warnings: usize,
    pub errors: usize,
    pub last_level: Level,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayFieldRow {
    pub key: String,
    pub count: usize,
    pub shown: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayFieldColumn {
    pub key: String,
    pub width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YankedLines {
    pub text: String,
    pub line_count: usize,
}

#[derive(Debug)]
pub struct App {
    buffer: LogBuffer,
    filter_workflow: FilterWorkflow,
    visible: VisibleLogView,
    visual_anchor: Option<usize>,
    marked_sequences: Vec<u64>,
    mode: Mode,
    notice: Option<String>,
    prompt: String,
    palette: SearchableListState,
    pending_g: bool,
    details_open: bool,
    selected_property: usize,
    property_filters: SearchableListState,
    editing_property_filter: Option<PropertyFilterId>,
    display_field_keys: Vec<String>,
    display_fields: SearchableListState,
    filter_preset_list: SearchableListState,
    sources: SearchableListState,
}

const MAX_DISPLAY_FIELD_WIDTH: usize = 32;

impl App {
    #[cfg(test)]
    pub fn new(buffer_lines: usize) -> Self {
        Self::with_source_config(buffer_lines, SourceConfig::default())
    }

    pub fn with_source_config(buffer_lines: usize, source_config: SourceConfig) -> Self {
        Self {
            buffer: LogBuffer::with_source_config(buffer_lines, source_config),
            filter_workflow: FilterWorkflow::default(),
            visible: VisibleLogView::new(),
            visual_anchor: None,
            marked_sequences: Vec::new(),
            mode: Mode::Normal,
            notice: None,
            prompt: String::new(),
            palette: SearchableListState::default(),
            pending_g: false,
            details_open: false,
            selected_property: 0,
            property_filters: SearchableListState::default(),
            editing_property_filter: None,
            display_field_keys: Vec::new(),
            display_fields: SearchableListState::default(),
            filter_preset_list: SearchableListState::default(),
            sources: SearchableListState::default(),
        }
    }

    pub fn push_line(&mut self, line: String) {
        let change = self.buffer.push_line(line);
        self.apply_buffer_change(change);
        self.sync_selection();
    }

    pub fn retained_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_following(&self) -> bool {
        self.visible.is_following()
    }

    pub fn paused_backlog(&self) -> usize {
        self.visible.paused_backlog()
    }

    pub fn marker_count(&self) -> usize {
        self.marked_sequences.len()
    }

    pub fn is_marked(&self, sequence: u64) -> bool {
        self.marked_sequences.contains(&sequence)
    }

    pub fn selected(&self) -> usize {
        self.visible.selected()
    }

    pub fn log_viewport_start(&self) -> usize {
        self.visible.viewport_start()
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
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

    pub fn dialog_query(&self, kind: DialogKind) -> &str {
        self.dialog_state(kind).query()
    }

    pub fn selected_dialog_index(&self, kind: DialogKind) -> usize {
        self.dialog_state(kind).selected()
    }

    pub fn property_filter_rows(&self) -> Vec<PropertyFilterRow> {
        let query = self.property_filters.query().trim();
        self.filter_workflow.property_filter_rows(query)
    }

    pub fn selected_property_filter_row(&self) -> Option<PropertyFilterRow> {
        self.property_filter_rows()
            .get(self.property_filters.selected())
            .cloned()
    }

    #[cfg(test)]
    pub fn display_field_keys(&self) -> &[String] {
        &self.display_field_keys
    }

    pub fn display_field_rows(&self) -> Vec<DisplayFieldRow> {
        let query = self.display_fields.query().trim();
        self.all_display_field_rows()
            .into_iter()
            .filter(|row| display_field_row_matches(row, query))
            .collect()
    }

    pub fn selected_display_field_row(&self) -> Option<DisplayFieldRow> {
        self.display_field_rows()
            .get(self.display_fields.selected())
            .cloned()
    }

    pub fn display_field_columns(&self) -> Vec<DisplayFieldColumn> {
        let mut columns = self
            .display_field_keys
            .iter()
            .map(|key| (key.clone(), key.chars().count().max("-".chars().count())))
            .collect::<Vec<_>>();

        if columns.is_empty() {
            return Vec::new();
        }

        self.for_each_visible_event(0, self.visible_count(), |_, event| {
            for (key, width) in &mut columns {
                if let Some(property) = event.property(key) {
                    *width = (*width).max(property.value.as_display_str().chars().count());
                }
            }
        });

        columns
            .into_iter()
            .map(|(key, width)| DisplayFieldColumn {
                key,
                width: width.min(MAX_DISPLAY_FIELD_WIDTH),
            })
            .collect()
    }

    pub fn filter_preset_rows(&self) -> Vec<FilterPresetRow> {
        let query = self.filter_preset_list.query().trim();
        self.filter_workflow.filter_preset_rows(query)
    }

    pub fn selected_filter_preset_row(&self) -> Option<FilterPresetRow> {
        self.filter_preset_rows()
            .get(self.filter_preset_list.selected())
            .cloned()
    }

    pub fn source_status_rows(&self) -> Vec<SourceStatusRow> {
        let query = self.sources.query().trim();
        self.source_status_rows_for_query(query)
    }

    fn source_status_rows_for_query(&self, query: &str) -> Vec<SourceStatusRow> {
        self.all_source_status_rows()
            .into_iter()
            .filter(|row| {
                query.is_empty()
                    || crate::filter::contains_ignore_ascii_case(&row.source, query)
                    || crate::filter::contains_ignore_ascii_case(row.last_level.as_str(), query)
            })
            .collect()
    }

    fn all_source_status_rows(&self) -> Vec<SourceStatusRow> {
        let mut rows = BTreeMap::<String, SourceStatusRow>::new();
        for event in self.buffer.events() {
            let row = rows
                .entry(event.source.clone())
                .or_insert_with(|| SourceStatusRow {
                    source: event.source.clone(),
                    count: 0,
                    warnings: 0,
                    errors: 0,
                    last_level: event.level,
                    last_sequence: event.sequence,
                });

            row.count += 1;
            if event.level == Level::Warn {
                row.warnings += 1;
            }
            if matches!(event.level, Level::Fatal | Level::Error) {
                row.errors += 1;
            }
            row.last_level = event.level;
            row.last_sequence = event.sequence;
        }
        rows.into_values().collect()
    }

    pub fn filters(&self) -> &LogFilter {
        self.filter_workflow.filters()
    }

    #[cfg(test)]
    pub fn filter_history_len(&self) -> usize {
        self.filter_workflow.history_len()
    }

    #[cfg(test)]
    fn filters_mut(&mut self) -> &mut LogFilter {
        self.filter_workflow.filters_mut()
    }

    pub fn details_open(&self) -> bool {
        self.details_open
    }

    pub fn selected_property_index(&self) -> usize {
        self.selected_property
    }

    pub fn selected_event(&self) -> Option<&LogEvent> {
        self.visible
            .selected_event(&self.buffer, self.filter_workflow.filters())
    }

    pub fn visual_selection_range(&self) -> Option<(usize, usize)> {
        if self.mode != Mode::Visual || self.visible_count() == 0 {
            return None;
        }

        let anchor = self.visual_anchor?;
        let selected = self.selected().min(self.visible_count() - 1);
        Some(if anchor <= selected {
            (anchor, selected)
        } else {
            (selected, anchor)
        })
    }

    pub fn visual_selected_count(&self) -> usize {
        self.visual_selection_range()
            .map(|(start, end)| end - start + 1)
            .unwrap_or_default()
    }

    pub fn selected_property(&self) -> Option<&LogProperty> {
        self.selected_event()
            .and_then(|event| event.properties.get(self.selected_property))
    }

    pub fn visible_count(&self) -> usize {
        self.visible
            .visible_count(&self.buffer, self.filter_workflow.filters())
    }

    pub fn visible_event_at(&self, visible_index: usize) -> Option<&LogEvent> {
        self.visible
            .event_at(&self.buffer, self.filter_workflow.filters(), visible_index)
    }

    pub fn for_each_visible_event<'a>(
        &'a self,
        start: usize,
        limit: usize,
        visit: impl FnMut(usize, &'a LogEvent),
    ) {
        self.visible.for_each_visible_event(
            &self.buffer,
            self.filter_workflow.filters(),
            start,
            limit,
            visit,
        );
    }

    #[cfg(test)]
    pub fn event_at_visible(&self, visible_index: usize) -> Option<&LogEvent> {
        self.visible_event_at(visible_index)
    }

    pub fn sync_log_viewport(&mut self, viewport_height: usize) {
        self.visible.sync_viewport(
            &self.buffer,
            self.filter_workflow.filters(),
            viewport_height,
        );
    }

    pub fn move_down(&mut self, amount: usize) {
        self.visible
            .move_down(&self.buffer, self.filter_workflow.filters(), amount);
        self.pending_g = false;
        self.sync_selected_property();
    }

    pub fn move_up(&mut self, amount: usize) {
        self.visible.move_up(amount);
        self.pending_g = false;
        self.sync_selected_property();
    }

    pub fn jump_top(&mut self) {
        self.visible.jump_top();
        self.pending_g = false;
        self.sync_selected_property();
    }

    pub fn move_to_last_visible(&mut self) {
        self.visible
            .move_to_last_visible(&self.buffer, self.filter_workflow.filters());
        self.pending_g = false;
        self.sync_selected_property();
    }

    pub fn jump_bottom(&mut self) {
        self.visible
            .jump_bottom(&self.buffer, self.filter_workflow.filters());
        self.pending_g = false;
        self.sync_selection();
    }

    pub fn toggle_follow(&mut self) {
        self.pending_g = false;
        self.visible
            .toggle_follow(&self.buffer, self.filter_workflow.filters());
    }

    pub fn start_prompt(&mut self, kind: PromptKind) {
        self.pending_g = false;
        let property = self.selected_property().cloned();
        self.prompt = self
            .filter_workflow
            .prompt_value(filter_edit(kind), property.as_ref());
        self.editing_property_filter = None;
        self.visual_anchor = None;
        self.mode = Mode::Prompt(kind);
    }

    pub fn start_property_filter_edit(&mut self) {
        let Some(row) = self.selected_property_filter_row() else {
            return;
        };
        let Some(predicate) = self.filter_workflow.property_filter(row.id) else {
            self.sync_property_filter_selection();
            return;
        };

        self.prompt = predicate.summary_for(row.id.exclude);
        self.editing_property_filter = Some(row.id);
        self.visual_anchor = None;
        self.mode = Mode::Prompt(PromptKind::EditPropertyFilter);
        self.pending_g = false;
    }

    pub fn open_palette(&mut self) {
        self.mode = Mode::Palette;
        self.prompt.clear();
        self.pending_g = false;
        self.visual_anchor = None;
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
        self.palette
            .move_down(amount, self.palette_commands().len());
    }

    pub fn move_palette_up(&mut self, amount: usize) {
        self.palette.move_up(amount);
    }

    pub fn open_dialog(&mut self, kind: DialogKind) {
        self.mode = Mode::Dialog(kind);
        self.prompt.clear();
        self.pending_g = false;
        self.visual_anchor = None;
        if kind == DialogKind::PropertyFilters {
            self.editing_property_filter = None;
        }
        self.sync_dialog_selection(kind);
    }

    pub fn close_dialog(&mut self) {
        self.mode = Mode::Normal;
        self.pending_g = false;
    }

    pub fn move_dialog_down(&mut self, kind: DialogKind, amount: usize) {
        let len = self.dialog_len(kind);
        self.dialog_state_mut(kind).move_down(amount, len);
    }

    pub fn move_dialog_up(&mut self, kind: DialogKind, amount: usize) {
        self.dialog_state_mut(kind).move_up(amount);
    }

    pub fn push_dialog_query_char(&mut self, kind: DialogKind, value: char) {
        let len = self.dialog_len_after_query_push(kind, value);
        self.dialog_state_mut(kind).push_query_char(value, len);
    }

    pub fn pop_dialog_query_char(&mut self, kind: DialogKind) {
        let len = self.dialog_len_after_query_pop(kind);
        self.dialog_state_mut(kind).pop_query_char(len);
    }

    pub fn add_selected_display_field(&mut self) {
        self.pending_g = false;
        let Some(key) = self
            .selected_property()
            .map(|property| property.key.clone())
        else {
            return;
        };

        self.show_display_field(key);
        self.sync_display_field_selection();
    }

    pub fn activate_selected_dialog_row(&mut self, kind: DialogKind) {
        match kind {
            DialogKind::PropertyFilters => self.start_property_filter_edit(),
            DialogKind::DisplayFields => self.toggle_selected_display_field(),
            DialogKind::FilterPresets => self.apply_selected_filter_preset(),
            DialogKind::Sources => {}
        }
    }

    pub fn delete_selected_dialog_row(&mut self, kind: DialogKind) {
        match kind {
            DialogKind::PropertyFilters => self.delete_selected_property_filter(),
            DialogKind::DisplayFields => self.delete_selected_display_field(),
            DialogKind::FilterPresets => self.delete_selected_filter_preset(),
            DialogKind::Sources => {}
        }
    }

    pub fn push_prompt_char(&mut self, value: char) {
        self.prompt.push(value);
    }

    pub fn pop_prompt_char(&mut self) {
        self.prompt.pop();
    }

    pub fn cancel_prompt(&mut self) {
        self.mode = if self.editing_property_filter.is_some() {
            Mode::Dialog(DialogKind::PropertyFilters)
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

    pub fn start_visual_selection(&mut self) {
        let Some(anchor) = self
            .visible
            .start_visual_selection(&self.buffer, self.filter_workflow.filters())
        else {
            self.visual_anchor = None;
            self.pending_g = false;
            return;
        };

        self.pending_g = false;
        self.visual_anchor = Some(anchor);
        self.mode = Mode::Visual;
    }

    pub fn cancel_visual_selection(&mut self) {
        self.mode = Mode::Normal;
        self.visual_anchor = None;
        self.pending_g = false;
    }

    pub fn yank_selected_line(&self) -> Option<YankedLines> {
        self.selected_event().map(|event| YankedLines {
            text: event.raw.clone(),
            line_count: 1,
        })
    }

    pub fn yank_visual_selection(&mut self) -> Option<YankedLines> {
        let yanked = self
            .visual_selection_range()
            .and_then(|(start, end)| self.yank_visible_range(start, end));
        self.cancel_visual_selection();
        yanked
    }

    pub fn apply_prompt(&mut self) {
        let Mode::Prompt(kind) = self.mode else {
            return;
        };

        self.filter_workflow.apply_prompt(
            filter_edit(kind),
            &self.prompt,
            self.editing_property_filter,
        );
        self.sync_visible_cache();
        self.mode = if matches!(kind, PromptKind::EditPropertyFilter) {
            Mode::Dialog(DialogKind::PropertyFilters)
        } else {
            Mode::Normal
        };
        self.prompt.clear();
        self.editing_property_filter = None;
        self.sync_property_filter_selection();
        self.sync_selection();
    }

    pub fn clear_filters(&mut self) {
        self.filter_workflow.clear();
        self.sync_visible_cache();
        self.pending_g = false;
        self.sync_selection();
    }

    pub fn save_filter_preset(&mut self) {
        self.pending_g = false;
        self.filter_workflow.save_preset();
        self.sync_filter_preset_selection();
    }

    pub fn toggle_selected_marker(&mut self) {
        self.pending_g = false;
        let Some(sequence) = self.selected_event().map(|event| event.sequence) else {
            return;
        };
        if let Some(index) = self
            .marked_sequences
            .iter()
            .position(|marked| *marked == sequence)
        {
            self.marked_sequences.remove(index);
        } else {
            self.marked_sequences.push(sequence);
            self.marked_sequences.sort_unstable();
        }
    }

    pub fn export_visible_logs(&self, path: &Path) -> io::Result<usize> {
        let mut file = File::create(path)?;
        let mut count = 0;
        let mut write_error = None;
        self.for_each_visible_event(0, self.visible_count(), |_, event| {
            if write_error.is_some() {
                return;
            }
            if let Err(error) = writeln!(file, "{}", event.raw) {
                write_error = Some(error);
            } else {
                count += 1;
            }
        });
        if let Some(error) = write_error {
            return Err(error);
        }
        file.flush()?;
        Ok(count)
    }

    pub fn export_visible_logs_default(&self) -> io::Result<usize> {
        self.export_visible_logs(Path::new("loggle-export.log"))
    }

    pub fn undo_filter_change(&mut self) {
        if !self.filter_workflow.undo() {
            self.pending_g = false;
            return;
        }

        self.sync_visible_cache();
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
        let Some(property) = self.selected_property().cloned() else {
            return;
        };
        self.filter_workflow.follow_property(&property);
        self.sync_visible_cache();
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
        self.visible
            .move_to_search_match(&self.buffer, self.filter_workflow.filters(), forward);
        self.sync_selected_property();
    }

    fn sync_selection(&mut self) {
        self.visible
            .sync_selection(&self.buffer, self.filter_workflow.filters());
        self.sync_selected_property();
        self.sync_visual_anchor();
    }

    fn apply_buffer_change(&mut self, change: BufferChange) {
        for sequence in &change.removed {
            self.remove_marker(sequence);
        }
        self.visible
            .on_line_received(&change, &self.buffer, self.filter_workflow.filters());
    }

    fn sync_visible_cache(&mut self) {
        self.visible
            .on_filters_changed(&self.buffer, self.filter_workflow.filters());
    }

    fn remove_marker(&mut self, sequence: &u64) {
        if let Some(index) = self
            .marked_sequences
            .iter()
            .position(|marked| marked == sequence)
        {
            self.marked_sequences.remove(index);
        }
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

    fn sync_visual_anchor(&mut self) {
        if self.mode != Mode::Visual {
            self.visual_anchor = None;
            return;
        }

        let visible_len = self.visible_count();
        if visible_len == 0 {
            self.visual_anchor = None;
        } else if let Some(anchor) = self.visual_anchor.as_mut() {
            *anchor = (*anchor).min(visible_len - 1);
        } else {
            self.visual_anchor = Some(self.selected().min(visible_len - 1));
        }
    }

    fn yank_visible_range(&self, start: usize, end: usize) -> Option<YankedLines> {
        let lines = (start..=end)
            .filter_map(|visible_index| self.visible_event_at(visible_index))
            .map(|event| event.raw.as_str())
            .collect::<Vec<_>>();

        (!lines.is_empty()).then(|| YankedLines {
            text: lines.join("\n"),
            line_count: lines.len(),
        })
    }

    fn dialog_state(&self, kind: DialogKind) -> &SearchableListState {
        match kind {
            DialogKind::PropertyFilters => &self.property_filters,
            DialogKind::DisplayFields => &self.display_fields,
            DialogKind::FilterPresets => &self.filter_preset_list,
            DialogKind::Sources => &self.sources,
        }
    }

    fn dialog_state_mut(&mut self, kind: DialogKind) -> &mut SearchableListState {
        match kind {
            DialogKind::PropertyFilters => &mut self.property_filters,
            DialogKind::DisplayFields => &mut self.display_fields,
            DialogKind::FilterPresets => &mut self.filter_preset_list,
            DialogKind::Sources => &mut self.sources,
        }
    }

    fn dialog_len(&self, kind: DialogKind) -> usize {
        match kind {
            DialogKind::PropertyFilters => self.property_filter_rows().len(),
            DialogKind::DisplayFields => self.display_field_rows().len(),
            DialogKind::FilterPresets => self.filter_preset_rows().len(),
            DialogKind::Sources => self.source_status_rows().len(),
        }
    }

    fn dialog_len_after_query_push(&self, kind: DialogKind, value: char) -> usize {
        let mut query = self.dialog_query(kind).to_string();
        query.push(value);
        self.dialog_len_for_query(kind, query.trim())
    }

    fn dialog_len_after_query_pop(&self, kind: DialogKind) -> usize {
        let mut query = self.dialog_query(kind).to_string();
        query.pop();
        self.dialog_len_for_query(kind, query.trim())
    }

    fn dialog_len_for_query(&self, kind: DialogKind, query: &str) -> usize {
        match kind {
            DialogKind::PropertyFilters => self.filter_workflow.property_filter_row_count(query),
            DialogKind::DisplayFields => self.display_field_row_count(query),
            DialogKind::FilterPresets => self.filter_workflow.filter_preset_row_count(query),
            DialogKind::Sources => self.source_status_rows_for_query(query).len(),
        }
    }

    fn sync_palette_selection(&mut self) {
        self.palette.sync(self.palette_commands().len());
    }

    fn sync_dialog_selection(&mut self, kind: DialogKind) {
        let len = self.dialog_len(kind);
        self.dialog_state_mut(kind).sync(len);
    }

    fn sync_property_filter_selection(&mut self) {
        self.sync_dialog_selection(DialogKind::PropertyFilters);
    }

    fn sync_display_field_selection(&mut self) {
        self.sync_dialog_selection(DialogKind::DisplayFields);
    }

    fn sync_filter_preset_selection(&mut self) {
        self.sync_dialog_selection(DialogKind::FilterPresets);
    }

    fn delete_selected_property_filter(&mut self) {
        let Some(row) = self.selected_property_filter_row() else {
            return;
        };

        self.filter_workflow.remove_property_filter(row.id);
        self.sync_visible_cache();
        self.sync_property_filter_selection();
        self.sync_selection();
    }

    fn delete_selected_display_field(&mut self) {
        let Some(row) = self.selected_display_field_row() else {
            return;
        };
        self.hide_display_field(&row.key);
        self.sync_display_field_selection();
    }

    fn toggle_selected_display_field(&mut self) {
        let Some(row) = self.selected_display_field_row() else {
            return;
        };

        if row.shown {
            self.hide_display_field(&row.key);
        } else {
            self.show_display_field(row.key);
        }
        self.sync_display_field_selection();
    }

    fn show_display_field(&mut self, key: String) {
        if !self
            .display_field_keys
            .iter()
            .any(|existing| existing == &key)
        {
            self.display_field_keys.push(key);
        }
    }

    fn hide_display_field(&mut self, key: &str) {
        if let Some(index) = self
            .display_field_keys
            .iter()
            .position(|candidate| candidate == key)
        {
            self.display_field_keys.remove(index);
        }
    }

    fn all_display_field_rows(&self) -> Vec<DisplayFieldRow> {
        let mut counts = BTreeMap::<String, usize>::new();
        for event in self.buffer.events() {
            for property in &event.properties {
                *counts.entry(property.key.clone()).or_default() += 1;
            }
        }
        for key in &self.display_field_keys {
            counts.entry(key.clone()).or_default();
        }

        counts
            .into_iter()
            .map(|(key, count)| {
                let shown = self
                    .display_field_keys
                    .iter()
                    .any(|display_key| display_key == &key);
                DisplayFieldRow { key, count, shown }
            })
            .collect()
    }

    fn display_field_row_count(&self, query: &str) -> usize {
        self.all_display_field_rows()
            .into_iter()
            .filter(|row| display_field_row_matches(row, query))
            .count()
    }

    fn apply_selected_filter_preset(&mut self) {
        let Some(row) = self.selected_filter_preset_row() else {
            return;
        };
        if !self.filter_workflow.apply_preset(row.index) {
            self.sync_filter_preset_selection();
            return;
        }

        self.sync_visible_cache();
        self.sync_selection();
        self.close_dialog();
    }

    fn delete_selected_filter_preset(&mut self) {
        let Some(row) = self.selected_filter_preset_row() else {
            return;
        };
        self.filter_workflow.delete_preset(row.index);
        self.sync_filter_preset_selection();
    }
}

fn filter_edit(kind: PromptKind) -> FilterEdit {
    match kind {
        PromptKind::Text => FilterEdit::Text,
        PromptKind::Source => FilterEdit::Source,
        PromptKind::Level => FilterEdit::Level,
        PromptKind::IncludeProperty => FilterEdit::IncludeProperty,
        PromptKind::ExcludeProperty => FilterEdit::ExcludeProperty,
        PromptKind::EditPropertyFilter => FilterEdit::EditPropertyFilter,
    }
}

fn display_field_row_matches(row: &DisplayFieldRow, query: &str) -> bool {
    query.is_empty() || crate::filter::contains_ignore_ascii_case(&row.key, query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{PropertyFilterUpdate, PropertyPredicate};

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
    fn paused_backlog_counts_lines_until_follow_resumes() {
        let mut app = App::new(10);
        app.push_line("api | one".to_string());
        app.push_line("api | two".to_string());

        app.move_up(1);
        app.push_line("api | three".to_string());
        app.push_line("api | four".to_string());

        assert!(!app.is_following());
        assert_eq!(app.paused_backlog(), 2);

        app.jump_bottom();

        assert!(app.is_following());
        assert_eq!(app.paused_backlog(), 0);
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
    fn moving_up_walks_selection_through_current_log_viewport_first() {
        let mut app = App::new(10);
        for index in 0..8 {
            app.push_line(format!("api | {index}"));
        }

        app.sync_log_viewport(4);
        assert_eq!(app.selected(), 7);
        assert_eq!(app.log_viewport_start(), 4);

        app.move_up(1);
        app.sync_log_viewport(4);
        assert_eq!(app.selected(), 6);
        assert_eq!(app.log_viewport_start(), 4);

        app.move_up(2);
        app.sync_log_viewport(4);
        assert_eq!(app.selected(), 4);
        assert_eq!(app.log_viewport_start(), 4);

        app.move_up(1);
        app.sync_log_viewport(4);
        assert_eq!(app.selected(), 3);
        assert_eq!(app.log_viewport_start(), 3);
    }

    #[test]
    fn moving_down_walks_selection_through_current_log_viewport_first() {
        let mut app = App::new(10);
        for index in 0..8 {
            app.push_line(format!("api | {index}"));
        }

        app.sync_log_viewport(4);
        app.move_up(3);
        app.sync_log_viewport(4);
        assert_eq!(app.selected(), 4);
        assert_eq!(app.log_viewport_start(), 4);

        app.move_down(2);
        app.sync_log_viewport(4);
        assert_eq!(app.selected(), 6);
        assert_eq!(app.log_viewport_start(), 4);

        app.move_down(1);
        app.sync_log_viewport(4);
        assert_eq!(app.selected(), 7);
        assert_eq!(app.log_viewport_start(), 4);
    }

    #[test]
    fn search_navigation_finds_next_and_previous_matches() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("api | ok".to_string());
        app.push_line("web | ERROR two".to_string());
        app.filters_mut().text = Some("error".to_string());
        app.sync_visible_cache();
        app.jump_top();

        app.next_search_match();
        assert_eq!(
            app.event_at_visible(app.selected()).unwrap().raw,
            "web | ERROR two"
        );

        app.previous_search_match();
        assert_eq!(
            app.event_at_visible(app.selected()).unwrap().raw,
            "api | ERROR one"
        );
    }

    #[test]
    fn visible_count_matches_retained_len_without_filters() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.push_line("web | INFO two".to_string());
        app.push_line("worker | WARN three".to_string());

        assert_eq!(app.visible_count(), app.retained_len());
        assert_eq!(app.visible_event_at(1).unwrap().raw, "web | INFO two");
    }

    #[test]
    fn visible_window_iterates_only_requested_filtered_rows() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("web | INFO two".to_string());
        app.push_line("worker | ERROR three".to_string());
        app.push_line("api | ERROR four".to_string());
        app.filters_mut().level = Some(Level::Error);
        app.sync_visible_cache();

        let mut rows = Vec::new();
        app.for_each_visible_event(1, 2, |visible_index, event| {
            rows.push((visible_index, event.raw.clone()));
        });

        assert_eq!(
            rows,
            vec![
                (1, "worker | ERROR three".to_string()),
                (2, "api | ERROR four".to_string())
            ]
        );
    }

    #[test]
    fn filtered_visible_cache_updates_as_matching_lines_arrive() {
        let mut app = App::new(10);
        app.start_prompt(PromptKind::Text);
        for ch in "error".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        app.push_line("api | INFO one".to_string());
        app.push_line("api | ERROR two".to_string());
        app.push_line("api | ERROR three".to_string());

        assert_eq!(app.visible_count(), 2);
        assert_eq!(app.visible_event_at(0).unwrap().raw, "api | ERROR two");
        assert_eq!(app.visible_event_at(1).unwrap().raw, "api | ERROR three");
    }

    #[test]
    fn filtered_visible_cache_drops_evicted_sequences() {
        let mut app = App::new(2);
        app.start_prompt(PromptKind::Text);
        for ch in "error".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        app.push_line("api | ERROR one".to_string());
        app.push_line("api | INFO two".to_string());
        app.push_line("api | ERROR three".to_string());

        assert_eq!(app.visible_count(), 1);
        assert_eq!(app.visible_event_at(0).unwrap().raw, "api | ERROR three");
    }

    #[test]
    fn filtered_visible_cache_refreshes_when_property_block_updates_event() {
        let mut app = App::new(10);
        app.start_prompt(PromptKind::IncludeProperty);
        while !app.prompt().is_empty() {
            app.pop_prompt_char();
        }
        for ch in "tenantId=tenant-1".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        app.push_line("14:06:58.892 INFO request completed".to_string());
        assert_eq!(app.visible_count(), 0);

        app.push_line("[14:06:58.892] INFO (#1):".to_string());
        app.push_line("{".to_string());
        app.push_line("tenantId: \"tenant-1\",".to_string());
        app.push_line("}".to_string());

        assert_eq!(app.visible_count(), 1);
        assert_eq!(
            app.visible_event_at(0).unwrap().message,
            "request completed"
        );
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
            app.filters().property_includes,
            vec![PropertyPredicate::exact("tenantId", "tenant-1")]
        );
    }

    #[test]
    fn undo_filter_change_restores_previous_filters() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("web | INFO two".to_string());

        app.start_prompt(PromptKind::Level);
        for ch in "error".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        assert_eq!(app.visible_count(), 1);
        assert_eq!(app.filter_history_len(), 1);

        app.undo_filter_change();

        assert_eq!(app.visible_count(), 2);
        assert_eq!(app.filters().level, None);
        assert_eq!(app.filter_history_len(), 0);
    }

    #[test]
    fn undo_filter_change_restores_property_isolation() {
        let mut app = App::new(10);
        app.push_line("14:06:58.892 INFO request completed".to_string());
        app.push_line("[14:06:58.892] INFO (#1):".to_string());
        app.push_line("{".to_string());
        app.push_line("tenantId: \"tenant-1\",".to_string());
        app.push_line("}".to_string());

        app.follow_selected_property();
        assert_eq!(app.visible_count(), 1);
        assert_eq!(app.filters().property_includes.len(), 1);

        app.undo_filter_change();

        assert_eq!(app.visible_count(), 1);
        assert!(app.filters().property_includes.is_empty());
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
    fn export_visible_logs_writes_filtered_rows() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("web | INFO two".to_string());
        app.push_line("api | ERROR three".to_string());
        app.start_prompt(PromptKind::Level);
        for ch in "error".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        let path =
            std::env::temp_dir().join(format!("loggle-export-test-{}.log", std::process::id()));
        let count = app.export_visible_logs(&path).unwrap();
        let output = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(count, 2);
        assert_eq!(output, "api | ERROR one\napi | ERROR three\n");
    }

    #[test]
    fn yanking_selected_line_returns_raw_log_text() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.push_line("web | WARN two".to_string());

        let yanked = app.yank_selected_line().unwrap();

        assert_eq!(yanked.text, "web | WARN two");
        assert_eq!(yanked.line_count, 1);
    }

    #[test]
    fn visual_selection_yanks_selected_visible_range() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.push_line("web | INFO two".to_string());
        app.push_line("worker | INFO three".to_string());
        app.jump_top();

        app.start_visual_selection();
        app.move_down(2);
        let yanked = app.yank_visual_selection().unwrap();

        assert_eq!(
            yanked.text,
            "api | INFO one\nweb | INFO two\nworker | INFO three"
        );
        assert_eq!(yanked.line_count, 3);
        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.visual_selection_range(), None);
    }

    #[test]
    fn visual_selection_range_handles_upward_selection() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.push_line("web | INFO two".to_string());
        app.push_line("worker | INFO three".to_string());

        app.start_visual_selection();
        app.move_up(2);

        assert_eq!(app.visual_selection_range(), Some((0, 2)));
        assert_eq!(app.visual_selected_count(), 3);
    }

    #[test]
    fn visual_selection_yanks_filtered_visible_rows_only() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("web | INFO two".to_string());
        app.push_line("api | ERROR three".to_string());
        app.start_prompt(PromptKind::Level);
        for ch in "error".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();
        app.jump_top();

        app.start_visual_selection();
        app.move_down(1);
        let yanked = app.yank_visual_selection().unwrap();

        assert_eq!(yanked.text, "api | ERROR one\napi | ERROR three");
        assert_eq!(yanked.line_count, 2);
    }

    #[test]
    fn visual_selection_does_not_start_without_visible_rows() {
        let mut app = App::new(10);

        app.start_visual_selection();

        assert_eq!(app.mode(), &Mode::Normal);
        assert_eq!(app.yank_selected_line(), None);
    }

    #[test]
    fn toggling_selected_marker_tracks_selected_event() {
        let mut app = App::new(10);
        app.push_line("api | INFO one".to_string());
        app.push_line("api | INFO two".to_string());

        let sequence = app.selected_event().unwrap().sequence;
        app.toggle_selected_marker();

        assert_eq!(app.marker_count(), 1);
        assert!(app.is_marked(sequence));

        app.toggle_selected_marker();

        assert_eq!(app.marker_count(), 0);
        assert!(!app.is_marked(sequence));
    }

    #[test]
    fn source_status_rows_summarize_observed_sources() {
        let mut app = App::new(10);
        app.push_line("api | ERROR one".to_string());
        app.push_line("api | WARN two".to_string());
        app.push_line("web | INFO three".to_string());

        let rows = app.source_status_rows();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, "api");
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].errors, 1);
        assert_eq!(rows[0].warnings, 1);
        assert_eq!(rows[0].last_level, Level::Warn);
        assert_eq!(rows[1].source, "web");
        assert_eq!(rows[1].count, 1);
    }

    #[test]
    fn markers_are_removed_when_events_are_evicted() {
        let mut app = App::new(1);
        app.push_line("api | INFO one".to_string());
        app.toggle_selected_marker();
        assert_eq!(app.marker_count(), 1);

        app.push_line("api | INFO two".to_string());

        assert_eq!(app.marker_count(), 0);
    }

    #[test]
    fn adding_selected_display_field_uses_property_key_once() {
        let mut app = App::new(10);
        app.push_line("api | INFO request tenantId=tenant-1 requestId=abc".to_string());

        app.add_selected_display_field();
        app.add_selected_display_field();
        app.next_property();
        app.add_selected_display_field();

        assert_eq!(
            app.display_field_keys(),
            &["tenantId".to_string(), "requestId".to_string()]
        );
    }

    #[test]
    fn display_field_rows_list_observed_property_counts_and_search() {
        let mut app = App::new(10);
        app.push_line("api | INFO request tenantId=tenant-1 requestId=abc".to_string());
        app.push_line("api | INFO request tenantId=tenant-2 durationMs=96".to_string());
        app.display_field_keys = vec!["tenantId".to_string()];

        let rows = app.display_field_rows();

        assert_eq!(
            rows,
            vec![
                DisplayFieldRow {
                    key: "durationMs".to_string(),
                    count: 1,
                    shown: false,
                },
                DisplayFieldRow {
                    key: "requestId".to_string(),
                    count: 1,
                    shown: false,
                },
                DisplayFieldRow {
                    key: "tenantId".to_string(),
                    count: 2,
                    shown: true,
                },
            ]
        );

        app.open_dialog(DialogKind::DisplayFields);
        app.push_dialog_query_char(DialogKind::DisplayFields, 'r');
        app.push_dialog_query_char(DialogKind::DisplayFields, 'e');

        assert_eq!(
            app.display_field_rows(),
            vec![DisplayFieldRow {
                key: "requestId".to_string(),
                count: 1,
                shown: false,
            }]
        );
    }

    #[test]
    fn display_field_dialog_toggles_selected_observed_field() {
        let mut app = App::new(10);
        app.push_line("api | INFO request tenantId=tenant-1 requestId=abc".to_string());

        app.open_dialog(DialogKind::DisplayFields);
        app.push_dialog_query_char(DialogKind::DisplayFields, 'r');
        app.push_dialog_query_char(DialogKind::DisplayFields, 'e');

        app.activate_selected_dialog_row(DialogKind::DisplayFields);
        assert_eq!(app.display_field_keys(), &["requestId".to_string()]);
        assert!(app.display_field_rows()[0].shown);

        app.activate_selected_dialog_row(DialogKind::DisplayFields);
        assert!(app.display_field_keys().is_empty());
        assert!(!app.display_field_rows()[0].shown);
    }

    #[test]
    fn display_field_rows_keep_shown_fields_after_eviction() {
        let mut app = App::new(1);
        app.push_line("api | INFO request tenantId=tenant-1".to_string());
        app.add_selected_display_field();

        app.push_line("api | INFO no structured properties".to_string());

        assert_eq!(
            app.display_field_rows(),
            vec![DisplayFieldRow {
                key: "tenantId".to_string(),
                count: 0,
                shown: true,
            }]
        );
    }

    #[test]
    fn display_field_dialog_deletes_shown_fields() {
        let mut app = App::new(10);
        app.push_line(
            "api | INFO request tenantId=tenant-1 requestId=abc durationMs=96".to_string(),
        );
        app.display_field_keys = vec![
            "tenantId".to_string(),
            "requestId".to_string(),
            "durationMs".to_string(),
        ];

        app.open_dialog(DialogKind::DisplayFields);
        app.push_dialog_query_char(DialogKind::DisplayFields, 'r');
        app.push_dialog_query_char(DialogKind::DisplayFields, 'e');

        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::DisplayFields));
        assert_eq!(app.display_field_rows()[0].key, "requestId");

        app.delete_selected_dialog_row(DialogKind::DisplayFields);

        assert_eq!(
            app.display_field_keys(),
            &["tenantId".to_string(), "durationMs".to_string()]
        );
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::DisplayFields));
        assert_eq!(app.selected_dialog_index(DialogKind::DisplayFields), 0);
    }

    #[test]
    fn display_field_dialog_backspace_deletes_when_search_is_empty() {
        let mut app = App::new(10);
        app.push_line("api | INFO request tenantId=tenant-1".to_string());
        app.display_field_keys = vec!["tenantId".to_string()];

        app.open_dialog(DialogKind::DisplayFields);
        app.delete_selected_dialog_row(DialogKind::DisplayFields);

        assert!(app.display_field_keys().is_empty());
        assert_eq!(app.mode(), &Mode::Dialog(DialogKind::DisplayFields));
    }

    #[test]
    fn display_field_columns_use_visible_rows_and_cap_long_values() {
        let mut app = App::new(10);
        app.push_line("api | INFO request tenantId=short requestId=req-api".to_string());
        app.push_line(
            "web | INFO request tenantId=very-very-long-hidden-value requestId=req-web".to_string(),
        );
        app.push_line(
            "api | INFO request longField=abcdefghijklmnopqrstuvwxyz0123456789".to_string(),
        );
        app.display_field_keys = vec![
            "tenantId".to_string(),
            "requestId".to_string(),
            "longField".to_string(),
            "missing".to_string(),
        ];
        app.start_prompt(PromptKind::Source);
        for ch in "api".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        assert_eq!(
            app.display_field_columns(),
            vec![
                DisplayFieldColumn {
                    key: "tenantId".to_string(),
                    width: 8,
                },
                DisplayFieldColumn {
                    key: "requestId".to_string(),
                    width: 9,
                },
                DisplayFieldColumn {
                    key: "longField".to_string(),
                    width: 32,
                },
                DisplayFieldColumn {
                    key: "missing".to_string(),
                    width: 7,
                },
            ]
        );
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
