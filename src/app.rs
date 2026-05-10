use crate::buffer::LogBuffer;
use crate::commands::{Command, COMMANDS};
use crate::filter::{LogFilter, PropertyFilterUpdate, PropertyPredicate};
use crate::model::{Level, LogEvent, LogProperty};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Text,
    Source,
    Level,
    IncludeProperty,
    ExcludeProperty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prompt(PromptKind),
    Palette,
}

#[derive(Debug)]
pub struct App {
    buffer: LogBuffer,
    filters: LogFilter,
    selected: usize,
    follow: bool,
    mode: Mode,
    prompt: String,
    palette_selected: usize,
    pending_g: bool,
    details_open: bool,
    selected_property: usize,
}

impl App {
    pub fn new(buffer_lines: usize) -> Self {
        Self {
            buffer: LogBuffer::new(buffer_lines),
            filters: LogFilter::default(),
            selected: 0,
            follow: true,
            mode: Mode::Normal,
            prompt: String::new(),
            palette_selected: 0,
            pending_g: false,
            details_open: false,
            selected_property: 0,
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
        self.palette_selected
    }

    pub fn palette_commands(&self) -> &'static [Command] {
        COMMANDS
    }

    pub fn selected_palette_command(&self) -> Option<&'static Command> {
        self.palette_commands().get(self.palette_selected)
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
        };
        self.mode = Mode::Prompt(kind);
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
        self.palette_selected = self
            .palette_selected
            .saturating_add(amount)
            .min(self.palette_max_index());
    }

    pub fn move_palette_up(&mut self, amount: usize) {
        self.palette_selected = self.palette_selected.saturating_sub(amount);
    }

    pub fn push_prompt_char(&mut self, value: char) {
        self.prompt.push(value);
    }

    pub fn pop_prompt_char(&mut self) {
        self.prompt.pop();
    }

    pub fn cancel_prompt(&mut self) {
        self.mode = Mode::Normal;
        self.prompt.clear();
        self.pending_g = false;
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
        }

        self.mode = Mode::Normal;
        self.prompt.clear();
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

            if event_matches_search(visible[index], query) {
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
        self.palette_selected = self.palette_selected.min(self.palette_max_index());
    }

    fn palette_max_index(&self) -> usize {
        self.palette_commands().len().saturating_sub(1)
    }
}

fn property_prompt_value(property: &LogProperty) -> String {
    format!("{}={}", property.key, property.value)
}

fn event_matches_search(event: &LogEvent, query: &str) -> bool {
    crate::filter::contains_ignore_ascii_case(&event.raw, query)
        || crate::filter::contains_ignore_ascii_case(&event.message, query)
        || crate::filter::contains_ignore_ascii_case(&event.source, query)
        || event.properties.iter().any(|property| {
            crate::filter::contains_ignore_ascii_case(&property.key, query)
                || crate::filter::contains_ignore_ascii_case(&property.value.to_string(), query)
        })
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
}
