use crate::buffer::LogBuffer;
use crate::filter::LogFilter;
use crate::model::{Level, LogEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Text,
    Source,
    Level,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prompt(PromptKind),
}

#[derive(Debug)]
pub struct App {
    buffer: LogBuffer,
    filters: LogFilter,
    selected: usize,
    follow: bool,
    mode: Mode,
    prompt: String,
    pending_g: bool,
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
            pending_g: false,
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

    pub fn filters(&self) -> &LogFilter {
        &self.filters
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
    }

    pub fn move_up(&mut self, amount: usize) {
        self.follow = false;
        self.pending_g = false;
        self.selected = self.selected.saturating_sub(amount);
    }

    pub fn jump_top(&mut self) {
        self.follow = false;
        self.pending_g = false;
        self.selected = 0;
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
        };
        self.mode = Mode::Prompt(kind);
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

            if crate::filter::contains_ignore_ascii_case(&visible[index].raw, query) {
                self.selected = index;
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
    }
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
}
