use crate::model::{Level, LogEvent};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {
    pub text: Option<String>,
    pub source: Option<String>,
    pub level: Option<Level>,
}

impl LogFilter {
    pub fn matches(&self, event: &LogEvent) -> bool {
        self.matches_text(event) && self.matches_source(event) && self.matches_level(event)
    }

    pub fn clear(&mut self) {
        self.text = None;
        self.source = None;
        self.level = None;
    }

    fn matches_text(&self, event: &LogEvent) -> bool {
        let Some(query) = self.text.as_ref().filter(|query| !query.is_empty()) else {
            return true;
        };

        contains_ignore_ascii_case(&event.raw, query)
            || contains_ignore_ascii_case(&event.message, query)
            || contains_ignore_ascii_case(&event.source, query)
    }

    fn matches_source(&self, event: &LogEvent) -> bool {
        let Some(source) = self.source.as_ref().filter(|source| !source.is_empty()) else {
            return true;
        };

        event.source.eq_ignore_ascii_case(source)
    }

    fn matches_level(&self, event: &LogEvent) -> bool {
        self.level.is_none_or(|level| event.level == level)
    }
}

pub fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LogEvent;

    #[test]
    fn matches_text_source_and_level_together() {
        let event = LogEvent::from_line(0, "api | ERROR database failed".to_string());
        let filter = LogFilter {
            text: Some("database".to_string()),
            source: Some("api".to_string()),
            level: Some(Level::Error),
        };

        assert!(filter.matches(&event));
    }

    #[test]
    fn rejects_events_that_do_not_match_all_filters() {
        let event = LogEvent::from_line(0, "web | info ready".to_string());
        let filter = LogFilter {
            text: Some("ready".to_string()),
            source: Some("api".to_string()),
            level: Some(Level::Info),
        };

        assert!(!filter.matches(&event));
    }

    #[test]
    fn text_matching_is_case_insensitive() {
        let event = LogEvent::from_line(0, "api | ERROR failed".to_string());
        let filter = LogFilter {
            text: Some("error".to_string()),
            source: None,
            level: None,
        };

        assert!(filter.matches(&event));
    }

    #[test]
    fn source_filter_matches_concurrently_prefix() {
        let event = LogEvent::from_line(0, "[backend] INFO listening".to_string());
        let filter = LogFilter {
            text: None,
            source: Some("backend".to_string()),
            level: None,
        };

        assert!(filter.matches(&event));
    }
}
