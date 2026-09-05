use std::collections::VecDeque;

use crate::{
    buffer::{BufferChange, LogBuffer},
    filter::LogFilter,
    model::LogEvent,
};

#[derive(Debug, Default)]
pub(super) struct VisibleLogView {
    cache: VecDeque<u64>,
    selected: usize,
    viewport_start: usize,
    follow: bool,
    paused_backlog: usize,
}

impl VisibleLogView {
    pub(super) fn new() -> Self {
        Self {
            follow: true,
            ..Self::default()
        }
    }

    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn viewport_start(&self) -> usize {
        self.viewport_start
    }

    pub(super) fn is_following(&self) -> bool {
        self.follow
    }

    pub(super) fn paused_backlog(&self) -> usize {
        self.paused_backlog
    }

    pub(super) fn visible_count(&self, buffer: &LogBuffer, filters: &LogFilter) -> usize {
        if filters.has_active_filters() {
            self.cache.len()
        } else {
            buffer.len()
        }
    }

    pub(super) fn event_at<'a>(
        &'a self,
        buffer: &'a LogBuffer,
        filters: &LogFilter,
        visible_index: usize,
    ) -> Option<&'a LogEvent> {
        if !filters.has_active_filters() {
            return buffer.events().get(visible_index);
        }

        self.cache
            .get(visible_index)
            .and_then(|sequence| buffer.event_by_sequence(*sequence))
    }

    pub(super) fn selected_event<'a>(
        &'a self,
        buffer: &'a LogBuffer,
        filters: &LogFilter,
    ) -> Option<&'a LogEvent> {
        self.event_at(buffer, filters, self.selected)
    }

    pub(super) fn for_each_visible_event<'a>(
        &'a self,
        buffer: &'a LogBuffer,
        filters: &LogFilter,
        start: usize,
        limit: usize,
        mut visit: impl FnMut(usize, &'a LogEvent),
    ) {
        if limit == 0 {
            return;
        }

        if !filters.has_active_filters() {
            let end = start.saturating_add(limit).min(buffer.len());
            for visible_index in start..end {
                if let Some(event) = buffer.events().get(visible_index) {
                    visit(visible_index, event);
                }
            }
            return;
        }

        let end = start.saturating_add(limit).min(self.cache.len());
        for visible_index in start..end {
            if let Some(event) = self
                .cache
                .get(visible_index)
                .and_then(|sequence| buffer.event_by_sequence(*sequence))
            {
                visit(visible_index, event);
            }
        }
    }

    pub(super) fn sync_viewport(
        &mut self,
        buffer: &LogBuffer,
        filters: &LogFilter,
        viewport_height: usize,
    ) {
        let visible_len = self.visible_count(buffer, filters);
        if viewport_height == 0 || visible_len == 0 {
            self.viewport_start = 0;
            return;
        }

        let max_start = visible_len.saturating_sub(viewport_height);
        if self.follow {
            self.viewport_start = max_start;
            return;
        }

        let selected = self.selected.min(visible_len - 1);
        self.viewport_start = self.viewport_start.min(max_start);

        if selected < self.viewport_start {
            self.viewport_start = selected;
        } else if selected >= self.viewport_start.saturating_add(viewport_height) {
            self.viewport_start = selected.saturating_sub(viewport_height - 1);
        }

        self.viewport_start = self.viewport_start.min(max_start);
    }

    pub(super) fn move_down(&mut self, buffer: &LogBuffer, filters: &LogFilter, amount: usize) {
        self.follow = false;
        let visible_len = self.visible_count(buffer, filters);
        if visible_len == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.saturating_add(amount).min(visible_len - 1);
        }
    }

    pub(super) fn move_up(&mut self, amount: usize) {
        self.follow = false;
        self.selected = self.selected.saturating_sub(amount);
    }

    pub(super) fn jump_top(&mut self) {
        self.follow = false;
        self.selected = 0;
    }

    pub(super) fn move_to_last_visible(&mut self, buffer: &LogBuffer, filters: &LogFilter) {
        self.follow = false;
        self.selected = self.visible_count(buffer, filters).saturating_sub(1);
    }

    pub(super) fn start_visual_selection(
        &mut self,
        buffer: &LogBuffer,
        filters: &LogFilter,
    ) -> Option<usize> {
        let visible_len = self.visible_count(buffer, filters);
        if visible_len == 0 {
            return None;
        }

        self.follow = false;
        self.selected = self.selected.min(visible_len - 1);
        Some(self.selected)
    }

    pub(super) fn jump_bottom(&mut self, buffer: &LogBuffer, filters: &LogFilter) {
        self.follow = true;
        self.paused_backlog = 0;
        self.sync_selection(buffer, filters);
    }

    pub(super) fn toggle_follow(&mut self, buffer: &LogBuffer, filters: &LogFilter) {
        if self.follow {
            self.follow = false;
        } else {
            self.jump_bottom(buffer, filters);
        }
    }

    pub(super) fn move_to_search_match(
        &mut self,
        buffer: &LogBuffer,
        filters: &LogFilter,
        forward: bool,
    ) {
        let Some(_) = filters.text.as_ref().filter(|query| !query.is_empty()) else {
            return;
        };
        self.follow = false;

        let visible_len = self.visible_count(buffer, filters);
        if visible_len == 0 {
            return;
        }

        let selected = self.selected.min(visible_len - 1);
        self.selected = if forward {
            (selected + 1) % visible_len
        } else {
            (selected + visible_len - 1) % visible_len
        };
    }

    pub(super) fn on_line_received(
        &mut self,
        change: &BufferChange,
        buffer: &LogBuffer,
        filters: &LogFilter,
    ) {
        self.apply_buffer_change(change, buffer, filters);
        if !self.follow {
            self.paused_backlog = self.paused_backlog.saturating_add(1);
        }
        self.sync_selection(buffer, filters);
    }

    pub(super) fn on_filters_changed(&mut self, buffer: &LogBuffer, filters: &LogFilter) {
        self.rebuild_cache(buffer, filters);
        self.sync_selection(buffer, filters);
    }

    pub(super) fn sync_selection(&mut self, buffer: &LogBuffer, filters: &LogFilter) {
        let visible_len = self.visible_count(buffer, filters);
        if visible_len == 0 {
            self.selected = 0;
        } else if self.follow {
            self.selected = visible_len - 1;
        } else {
            self.selected = self.selected.min(visible_len - 1);
        }
    }

    fn apply_buffer_change(
        &mut self,
        change: &BufferChange,
        buffer: &LogBuffer,
        filters: &LogFilter,
    ) {
        for sequence in &change.removed {
            if filters.has_active_filters() {
                self.remove_sequence(*sequence);
            }
        }

        if !filters.has_active_filters() {
            self.cache.clear();
            return;
        }

        if let Some(sequence) = change.appended {
            if buffer
                .event_by_sequence(sequence)
                .is_some_and(|event| filters.matches(event))
            {
                self.cache.push_back(sequence);
            }
        }

        for sequence in &change.updated {
            self.refresh_sequence(*sequence, buffer, filters);
        }
    }

    fn rebuild_cache(&mut self, buffer: &LogBuffer, filters: &LogFilter) {
        self.cache.clear();
        if !filters.has_active_filters() {
            return;
        }

        self.cache.extend(
            buffer
                .events()
                .iter()
                .filter(|event| filters.matches(event))
                .map(|event| event.sequence),
        );
    }

    fn refresh_sequence(&mut self, sequence: u64, buffer: &LogBuffer, filters: &LogFilter) {
        self.remove_sequence(sequence);
        let Some(event) = buffer.event_by_sequence(sequence) else {
            return;
        };
        if !filters.matches(event) {
            return;
        }

        let index = self
            .cache
            .partition_point(|cached_sequence| *cached_sequence < sequence);
        self.cache.insert(index, sequence);
    }

    fn remove_sequence(&mut self, sequence: u64) {
        let Some(&first) = self.cache.front() else {
            return;
        };
        if sequence <= first {
            if sequence == first {
                self.cache.pop_front();
            }
            return;
        }
        if let Ok(index) = self.cache.binary_search(&sequence) {
            self.cache.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_filter_cache_matches_scan_during_sustained_eviction_and_updates() {
        let mut buffer = LogBuffer::new(127);
        let mut view = VisibleLogView::new();
        let mut filter = LogFilter::default();
        filter.text = Some("tenant".into());
        for index in 0..10_000 {
            let line = match index % 6 {
                0 => "api | [14:06:58.892] INFO (#1):".into(),
                1 => "api | {".into(),
                2 => "api | tenant: \"a\"".into(),
                3 => "api | }".into(),
                4 => "api | 14:06:58.892 INFO request".into(),
                _ => format!("web | INFO tenant {index}"),
            };
            let change = buffer.push_line(line);
            view.on_line_received(&change, &buffer, &filter);
            let expected: Vec<_> = buffer
                .events()
                .iter()
                .filter(|event| filter.matches(event))
                .map(|event| event.sequence)
                .collect();
            assert_eq!(view.cache.iter().copied().collect::<Vec<_>>(), expected);
            for (index, sequence) in expected.iter().enumerate() {
                assert_eq!(
                    view.event_at(&buffer, &filter, index).unwrap().sequence,
                    *sequence
                );
            }
        }
    }
}
