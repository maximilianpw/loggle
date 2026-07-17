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
use crate::facet::{
    FacetGroup, FacetKind, FacetOptions, FacetValueType, MAX_FACET_BUCKET_LIMIT,
    MAX_FACET_RECORD_LIMIT, aggregate_facets, escape_facet_text,
};
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
    MessageFields,
    FilterPresets,
    Sources,
    Facets,
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
pub struct FacetDialogRow {
    pub facet: FacetKind,
    pub value: String,
    pub count: usize,
    pub value_types: Vec<FacetValueType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FacetDialogView {
    Root,
    PropertyValues { property_key: String },
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
    message_field_keys: Vec<String>,
    message_fields: SearchableListState,
    filter_preset_list: SearchableListState,
    sources: SearchableListState,
    facet_root_list: SearchableListState,
    facet_value_list: SearchableListState,
    facet_view: FacetDialogView,
    facet_root_rows: Vec<FacetDialogRow>,
    facet_value_rows: Vec<FacetDialogRow>,
    facet_root_summary: String,
    facet_value_summary: String,
}

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
            message_field_keys: Vec::new(),
            message_fields: SearchableListState::default(),
            filter_preset_list: SearchableListState::default(),
            sources: SearchableListState::default(),
            facet_root_list: SearchableListState::default(),
            facet_value_list: SearchableListState::default(),
            facet_view: FacetDialogView::Root,
            facet_root_rows: Vec::new(),
            facet_value_rows: Vec::new(),
            facet_root_summary: String::new(),
            facet_value_summary: String::new(),
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

    pub fn message_field_keys(&self) -> &[String] {
        &self.message_field_keys
    }

    pub fn message_field_rows(&self) -> Vec<&str> {
        let query = self.message_fields.query().trim();
        self.message_field_keys
            .iter()
            .map(String::as_str)
            .filter(|key| query.is_empty() || crate::filter::contains_ignore_ascii_case(key, query))
            .collect()
    }

    pub fn selected_message_field_key(&self) -> Option<&str> {
        self.message_field_rows()
            .get(self.message_fields.selected())
            .copied()
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

    pub fn facet_dialog_rows(&self) -> Vec<&FacetDialogRow> {
        let query = self.dialog_query(DialogKind::Facets).trim();
        self.active_facet_rows()
            .iter()
            .filter(|row| facet_dialog_row_matches(row, query))
            .collect()
    }

    pub fn facet_dialog_summary(&self) -> &str {
        match self.facet_view {
            FacetDialogView::Root => &self.facet_root_summary,
            FacetDialogView::PropertyValues { .. } => &self.facet_value_summary,
        }
    }

    pub fn facet_dialog_is_drilldown(&self) -> bool {
        matches!(self.facet_view, FacetDialogView::PropertyValues { .. })
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
        if kind == DialogKind::Facets {
            self.open_facet_dialog();
            return;
        }
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

    pub fn add_selected_message_field(&mut self) {
        self.pending_g = false;
        let Some(key) = self
            .selected_property()
            .map(|property| property.key.clone())
        else {
            return;
        };

        if !self
            .message_field_keys
            .iter()
            .any(|existing| existing == &key)
        {
            self.message_field_keys.push(key);
        }
        self.sync_message_field_selection();
    }

    pub fn activate_selected_dialog_row(&mut self, kind: DialogKind) {
        match kind {
            DialogKind::PropertyFilters => self.start_property_filter_edit(),
            DialogKind::MessageFields => {}
            DialogKind::FilterPresets => self.apply_selected_filter_preset(),
            DialogKind::Sources => {}
            DialogKind::Facets => self.activate_selected_facet_row(),
        }
    }

    pub fn delete_selected_dialog_row(&mut self, kind: DialogKind) {
        match kind {
            DialogKind::PropertyFilters => self.delete_selected_property_filter(),
            DialogKind::MessageFields => self.delete_selected_message_field(),
            DialogKind::FilterPresets => self.delete_selected_filter_preset(),
            DialogKind::Sources => {}
            DialogKind::Facets => self.return_to_facet_root(),
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
            DialogKind::MessageFields => &self.message_fields,
            DialogKind::FilterPresets => &self.filter_preset_list,
            DialogKind::Sources => &self.sources,
            DialogKind::Facets => match self.facet_view {
                FacetDialogView::Root => &self.facet_root_list,
                FacetDialogView::PropertyValues { .. } => &self.facet_value_list,
            },
        }
    }

    fn dialog_state_mut(&mut self, kind: DialogKind) -> &mut SearchableListState {
        match kind {
            DialogKind::PropertyFilters => &mut self.property_filters,
            DialogKind::MessageFields => &mut self.message_fields,
            DialogKind::FilterPresets => &mut self.filter_preset_list,
            DialogKind::Sources => &mut self.sources,
            DialogKind::Facets => match self.facet_view {
                FacetDialogView::Root => &mut self.facet_root_list,
                FacetDialogView::PropertyValues { .. } => &mut self.facet_value_list,
            },
        }
    }

    fn dialog_len(&self, kind: DialogKind) -> usize {
        match kind {
            DialogKind::PropertyFilters => self.property_filter_rows().len(),
            DialogKind::MessageFields => self.message_field_rows().len(),
            DialogKind::FilterPresets => self.filter_preset_rows().len(),
            DialogKind::Sources => self.source_status_rows().len(),
            DialogKind::Facets => self.facet_dialog_rows().len(),
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
            DialogKind::MessageFields => self
                .message_field_keys
                .iter()
                .filter(|key| {
                    query.is_empty()
                        || crate::filter::contains_ignore_ascii_case(key.as_str(), query)
                })
                .count(),
            DialogKind::FilterPresets => self.filter_workflow.filter_preset_row_count(query),
            DialogKind::Sources => self.source_status_rows_for_query(query).len(),
            DialogKind::Facets => self
                .active_facet_rows()
                .iter()
                .filter(|row| facet_dialog_row_matches(row, query))
                .count(),
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

    fn sync_message_field_selection(&mut self) {
        self.sync_dialog_selection(DialogKind::MessageFields);
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

    fn delete_selected_message_field(&mut self) {
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

    fn open_facet_dialog(&mut self) {
        let groups = self.aggregate_facet_snapshot(None);
        self.facet_root_rows = facet_root_rows(&groups);
        self.facet_root_summary = facet_root_summary(&groups);
        self.facet_value_rows.clear();
        self.facet_value_summary.clear();
        self.facet_root_list = SearchableListState::default();
        self.facet_value_list = SearchableListState::default();
        self.facet_view = FacetDialogView::Root;
        self.mode = Mode::Dialog(DialogKind::Facets);
        self.prompt.clear();
        self.pending_g = false;
        self.visual_anchor = None;
        self.sync_dialog_selection(DialogKind::Facets);
    }

    fn aggregate_facet_snapshot(&self, property_key: Option<&str>) -> Vec<FacetGroup> {
        let options = FacetOptions::new(MAX_FACET_BUCKET_LIMIT, property_key.map(str::to_string))
            .expect("the built-in facet bucket limit and selected non-empty key are valid");
        aggregate_facets(
            self.buffer.events().iter(),
            MAX_FACET_RECORD_LIMIT,
            self.filter_workflow.filters(),
            &options,
        )
    }

    fn active_facet_rows(&self) -> &[FacetDialogRow] {
        match self.facet_view {
            FacetDialogView::Root => &self.facet_root_rows,
            FacetDialogView::PropertyValues { .. } => &self.facet_value_rows,
        }
    }

    fn selected_facet_dialog_row(&self) -> Option<FacetDialogRow> {
        self.facet_dialog_rows()
            .get(self.selected_dialog_index(DialogKind::Facets))
            .map(|row| (*row).clone())
    }

    fn activate_selected_facet_row(&mut self) {
        let Some(row) = self.selected_facet_dialog_row() else {
            return;
        };

        match &self.facet_view {
            FacetDialogView::Root => match row.facet {
                FacetKind::Source => {
                    let changed = self.filter_workflow.replace_source_from_facet(&row.value);
                    self.finish_facet_filter_application(changed);
                }
                FacetKind::Level => {
                    let changed = Level::parse(&row.value)
                        .is_some_and(|level| self.filter_workflow.replace_level_from_facet(level));
                    self.finish_facet_filter_application(changed);
                }
                FacetKind::PropertyKey => self.refresh_facet_drilldown(row.value),
                FacetKind::PropertyValue => {}
            },
            FacetDialogView::PropertyValues { property_key } => {
                let property_key = property_key.clone();
                let changed = self
                    .filter_workflow
                    .replace_property_value_from_facet(&property_key, &row.value);
                self.finish_facet_filter_application(changed);
            }
        }
    }

    fn refresh_facet_drilldown(&mut self, property_key: String) {
        if property_key.trim().is_empty() {
            return;
        }
        let previous_selected = self.facet_root_list.selected();
        let selected_identity = self
            .selected_facet_dialog_row()
            .map(|row| (row.facet, row.value));
        let groups = self.aggregate_facet_snapshot(Some(&property_key));
        let root_rows = facet_root_rows(&groups);
        let root_summary = facet_root_summary(&groups);
        let value_group = facet_group(&groups, FacetKind::PropertyValue);
        let value_rows = value_group.map(facet_group_rows).unwrap_or_default();
        let value_summary = value_group
            .map(|group| facet_value_summary(group, &property_key))
            .unwrap_or_default();
        let query = self.facet_root_list.query().trim();
        let filtered_root_rows = root_rows
            .iter()
            .filter(|row| facet_dialog_row_matches(row, query))
            .collect::<Vec<_>>();
        let root_len = filtered_root_rows.len();
        let restored_selected = selected_identity
            .as_ref()
            .and_then(|(facet, value)| {
                filtered_root_rows
                    .iter()
                    .position(|row| row.facet == *facet && row.value == *value)
            })
            .unwrap_or_else(|| previous_selected.min(root_len.saturating_sub(1)));

        self.facet_root_rows = root_rows;
        self.facet_root_summary = root_summary;
        self.facet_value_rows = value_rows;
        self.facet_value_summary = value_summary;
        self.facet_root_list.move_up(usize::MAX);
        self.facet_root_list.move_down(restored_selected, root_len);
        self.facet_value_list = SearchableListState::default();
        self.facet_view = FacetDialogView::PropertyValues { property_key };
        self.sync_dialog_selection(DialogKind::Facets);
    }

    fn return_to_facet_root(&mut self) {
        if !self.facet_dialog_is_drilldown() {
            return;
        }
        self.facet_view = FacetDialogView::Root;
        self.sync_dialog_selection(DialogKind::Facets);
    }

    fn finish_facet_filter_application(&mut self, changed: bool) {
        if changed {
            self.sync_visible_cache();
            self.sync_selection();
        }
        self.close_dialog();
    }
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

fn facet_root_rows(groups: &[FacetGroup]) -> Vec<FacetDialogRow> {
    [FacetKind::Source, FacetKind::Level, FacetKind::PropertyKey]
        .into_iter()
        .filter_map(|kind| facet_group(groups, kind))
        .flat_map(facet_group_rows)
        .collect()
}

fn facet_root_summary(groups: &[FacetGroup]) -> String {
    let Some(first) = groups.first() else {
        return String::new();
    };
    let source = facet_group(groups, FacetKind::Source);
    let level = facet_group(groups, FacetKind::Level);
    let property_key = facet_group(groups, FacetKind::PropertyKey);
    format!(
        "win={}/{}{} src={}/{} lvl={}/{} key={}/{}",
        first.window_records,
        first.available_records,
        facet_window_suffix(first.window_truncated),
        source.map_or(0, |group| group.buckets.len()),
        source.map_or(0, |group| group.total_buckets),
        level.map_or(0, |group| group.buckets.len()),
        level.map_or(0, |group| group.total_buckets),
        property_key.map_or(0, |group| group.buckets.len()),
        property_key.map_or(0, |group| group.total_buckets),
    )
}

fn facet_value_summary(group: &FacetGroup, property_key: &str) -> String {
    const MAX_SUMMARY_CHARS: usize = 80;
    let prefix = format!(
        "win={}/{}{} val={}/{} key=",
        group.window_records,
        group.available_records,
        facet_window_suffix(group.window_truncated),
        group.buckets.len(),
        group.total_buckets,
    );
    let escaped_key = escape_facet_text(property_key);
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
    if maximum == 1 {
        return "~".to_string();
    }

    let mut truncated = value.chars().take(maximum - 1).collect::<String>();
    truncated.push('~');
    truncated
}

fn facet_dialog_row_matches(row: &FacetDialogRow, query: &str) -> bool {
    query.is_empty()
        || crate::filter::contains_ignore_ascii_case(&row.value, query)
        || crate::filter::contains_ignore_ascii_case(&escape_facet_text(&row.value), query)
        || crate::filter::contains_ignore_ascii_case(row.facet.as_str(), query)
        || row
            .value_types
            .iter()
            .any(|value_type| crate::filter::contains_ignore_ascii_case(value_type.as_str(), query))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facet::FacetBucket;
    use crate::filter::{PropertyFilterUpdate, PropertyPredicate};

    fn select_facet_row(app: &mut App, facet: FacetKind, value: &str) {
        let index = app
            .facet_dialog_rows()
            .iter()
            .position(|row| row.facet == facet && row.value == value)
            .unwrap();
        app.move_dialog_up(DialogKind::Facets, usize::MAX);
        app.move_dialog_down(DialogKind::Facets, index);
    }

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
        let rows = app
            .facet_dialog_rows()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert!(summary.contains("win=2/2"));

        app.push_line("worker | WARN three new=value".to_string());
        assert_eq!(app.facet_dialog_summary(), summary);
        assert_eq!(
            app.facet_dialog_rows()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            rows
        );

        for character in "api".chars() {
            app.push_dialog_query_char(DialogKind::Facets, character);
        }
        let filtered = app.facet_dialog_rows();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].facet, FacetKind::Source);
        assert_eq!(filtered[0].value, "api");
        assert_eq!(app.facet_dialog_summary(), summary);
    }

    #[test]
    fn facet_property_drilldown_refreshes_all_groups_and_preserves_root_state() {
        let mut app = App::new(20);
        app.push_line("api | INFO row tenant=one region=eu".to_string());
        app.open_dialog(DialogKind::Facets);
        for character in "property_key".chars() {
            app.push_dialog_query_char(DialogKind::Facets, character);
        }
        assert_eq!(app.facet_dialog_rows().len(), 2);
        select_facet_row(&mut app, FacetKind::PropertyKey, "tenant");
        assert_eq!(app.selected_dialog_index(DialogKind::Facets), 1);

        app.push_line("web | ERROR row tenant=two".to_string());
        app.activate_selected_dialog_row(DialogKind::Facets);

        assert!(app.facet_dialog_is_drilldown());
        assert_eq!(app.dialog_query(DialogKind::Facets), "");
        assert!(app.facet_dialog_summary().contains("win=2/2"));
        assert!(app.facet_dialog_summary().contains("val=2/2"));
        assert_eq!(
            app.facet_dialog_rows()
                .iter()
                .map(|row| row.value.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );

        app.delete_selected_dialog_row(DialogKind::Facets);
        assert!(!app.facet_dialog_is_drilldown());
        assert_eq!(app.dialog_query(DialogKind::Facets), "property_key");
        assert_eq!(app.selected_dialog_index(DialogKind::Facets), 0);
        let selected = app.facet_dialog_rows()[app.selected_dialog_index(DialogKind::Facets)];
        assert_eq!(selected.facet, FacetKind::PropertyKey);
        assert_eq!(selected.value, "tenant");
        assert!(app.facet_dialog_summary().contains("src=2/2"));
        assert_eq!(app.facet_dialog_rows().len(), 2);

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
        assert_eq!(app.facet_dialog_rows().len(), 1);
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

        let value_group = group(FacetKind::PropertyValue, 100, 100_000);
        let property_key = format!("{}{}", r"tenant\\segment".repeat(12), "\n".repeat(12));
        let escaped_key = escape_facet_text(&property_key);
        let summary = facet_value_summary(&value_group, &property_key);

        assert!(summary.starts_with("win=100000/100001 clipped val=100/100000 key="));
        assert_eq!(summary.chars().count(), 80);
        assert!(summary.ends_with('~'));
        assert!(!summary.contains(&escaped_key));
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
            .facet_dialog_rows()
            .iter()
            .map(|row| escape_facet_text(&row.value))
            .collect::<Vec<_>>();
        assert!(rendered.contains(&r"\n".to_string()));
        assert!(rendered.contains(&r"\\n".to_string()));
        assert_ne!(rendered[0], rendered[1]);

        for character in r"\n".chars() {
            app.push_dialog_query_char(DialogKind::Facets, character);
        }
        assert_eq!(app.facet_dialog_rows().len(), 2);
        select_facet_row(&mut app, FacetKind::PropertyValue, "\n");
        app.activate_selected_dialog_row(DialogKind::Facets);
        assert_eq!(
            app.filters().property_includes,
            vec![PropertyPredicate::exact("value", "\n")]
        );
    }
}
