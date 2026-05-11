use crate::model::{Level, LogEvent};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {
    pub text: Option<String>,
    pub source: Option<String>,
    pub level: Option<Level>,
    pub property_includes: Vec<PropertyPredicate>,
    pub property_excludes: Vec<PropertyPredicate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyPredicate {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyFilterUpdate {
    pub exclude: bool,
    pub predicate: PropertyPredicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyFilterId {
    pub exclude: bool,
    pub index: usize,
}

impl LogFilter {
    pub fn has_active_filters(&self) -> bool {
        self.text.as_ref().is_some_and(|query| !query.is_empty())
            || self.source.as_ref().is_some_and(|source| !source.is_empty())
            || self.level.is_some()
            || !self.property_includes.is_empty()
            || !self.property_excludes.is_empty()
    }

    pub fn matches(&self, event: &LogEvent) -> bool {
        self.matches_text(event)
            && self.matches_source(event)
            && self.matches_level(event)
            && self.matches_property_filters(event)
    }

    pub fn clear(&mut self) {
        self.text = None;
        self.source = None;
        self.level = None;
        self.property_includes.clear();
        self.property_excludes.clear();
    }

    pub fn add_property_filter(&mut self, update: PropertyFilterUpdate) {
        let filters = if update.exclude {
            &mut self.property_excludes
        } else {
            &mut self.property_includes
        };

        if !filters.contains(&update.predicate) {
            filters.push(update.predicate);
        }
    }

    pub fn property_filter(&self, id: PropertyFilterId) -> Option<&PropertyPredicate> {
        if id.exclude {
            self.property_excludes.get(id.index)
        } else {
            self.property_includes.get(id.index)
        }
    }

    pub fn remove_property_filter(&mut self, id: PropertyFilterId) -> Option<PropertyPredicate> {
        let filters = if id.exclude {
            &mut self.property_excludes
        } else {
            &mut self.property_includes
        };

        (id.index < filters.len()).then(|| filters.remove(id.index))
    }

    pub fn replace_property_filter(
        &mut self,
        id: PropertyFilterId,
        update: PropertyFilterUpdate,
    ) -> bool {
        if self.remove_property_filter(id).is_none() {
            return false;
        }

        self.add_property_filter(update);
        true
    }

    pub fn property_highlight_values(&self) -> Vec<&str> {
        self.property_includes
            .iter()
            .chain(self.property_excludes.iter())
            .filter_map(|predicate| predicate.value.as_deref())
            .filter(|value| !value.is_empty())
            .collect()
    }

    fn matches_text(&self, event: &LogEvent) -> bool {
        let Some(query) = self.text.as_ref().filter(|query| !query.is_empty()) else {
            return true;
        };

        event_contains(event, query)
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

    fn matches_property_filters(&self, event: &LogEvent) -> bool {
        self.property_includes
            .iter()
            .all(|predicate| predicate.matches(event))
            && !self
                .property_excludes
                .iter()
                .any(|predicate| predicate.matches(event))
    }
}

impl PropertyPredicate {
    pub fn exact(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: Some(value.into()),
        }
    }

    pub fn exists(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: None,
        }
    }

    pub fn matches(&self, event: &LogEvent) -> bool {
        event.property(&self.key).is_some_and(|property| {
            self.value
                .as_ref()
                .is_none_or(|value| property.value.to_string() == *value)
        })
    }

    pub fn summary(&self) -> String {
        match self.value.as_ref() {
            Some(value) => format!("{}={}", self.key, value),
            None => self.key.clone(),
        }
    }

    pub fn exclude_summary(&self) -> String {
        match self.value.as_ref() {
            Some(value) => format!("{}!={}", self.key, value),
            None => format!("!{}", self.key),
        }
    }

    pub fn summary_for(&self, exclude: bool) -> String {
        if exclude {
            self.exclude_summary()
        } else {
            self.summary()
        }
    }
}

impl PropertyFilterUpdate {
    pub fn parse(input: &str, default_exclude: bool) -> Option<Self> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }

        if let Some((key, value)) = input.split_once("!=") {
            return property_exact(key, value, true);
        }

        if let Some((key, value)) = input.split_once('=') {
            return property_exact(key, value, default_exclude);
        }

        if let Some(key) = input.strip_prefix('!') {
            let key = key.trim();
            return (!key.is_empty()).then(|| Self {
                exclude: true,
                predicate: PropertyPredicate::exists(key),
            });
        }

        Some(Self {
            exclude: default_exclude,
            predicate: PropertyPredicate::exists(input),
        })
    }
}

fn property_exact(key: &str, value: &str, exclude: bool) -> Option<PropertyFilterUpdate> {
    let key = key.trim();
    let value = normalize_filter_value(value.trim());

    (!key.is_empty()).then(|| PropertyFilterUpdate {
        exclude,
        predicate: PropertyPredicate::exact(key, value),
    })
}

fn normalize_filter_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0] as char;
        let last = trimmed.as_bytes()[trimmed.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }

    trimmed.to_string()
}

pub fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

pub fn event_contains(event: &LogEvent, query: &str) -> bool {
    contains_ignore_ascii_case(&event.raw, query)
        || contains_ignore_ascii_case(&event.message, query)
        || contains_ignore_ascii_case(&event.source, query)
        || event.properties.iter().any(|property| {
            contains_ignore_ascii_case(&property.key, query)
                || contains_ignore_ascii_case(&property.value.to_string(), query)
        })
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
            ..LogFilter::default()
        };

        assert!(filter.matches(&event));
    }

    #[test]
    fn empty_text_and_source_are_not_active_filters() {
        let filter = LogFilter {
            text: Some(String::new()),
            source: Some(String::new()),
            ..LogFilter::default()
        };

        assert!(!filter.has_active_filters());
    }

    #[test]
    fn rejects_events_that_do_not_match_all_filters() {
        let event = LogEvent::from_line(0, "web | info ready".to_string());
        let filter = LogFilter {
            text: Some("ready".to_string()),
            source: Some("api".to_string()),
            level: Some(Level::Info),
            ..LogFilter::default()
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
            ..LogFilter::default()
        };

        assert!(filter.matches(&event));
    }

    #[test]
    fn event_contains_checks_raw_source_message_and_properties() {
        let mut event = LogEvent::from_line(0, "api | INFO request completed".to_string());
        event.set_properties(vec![
            crate::model::LogProperty {
                key: "tenantId".to_string(),
                value: crate::model::PropertyValue::String("tenant-1".to_string()),
            },
            crate::model::LogProperty {
                key: "statusCode".to_string(),
                value: crate::model::PropertyValue::Number("200".to_string()),
            },
        ]);

        assert!(event_contains(&event, "|"));
        assert!(event_contains(&event, "api"));
        assert!(event_contains(&event, "request completed"));
        assert!(event_contains(&event, "tenantid"));
        assert!(event_contains(&event, "tenant-1"));
        assert!(event_contains(&event, "200"));
        assert!(!event_contains(&event, "missing"));
    }

    #[test]
    fn source_filter_matches_concurrently_prefix() {
        let event = LogEvent::from_line(0, "[backend] INFO listening".to_string());
        let filter = LogFilter {
            text: None,
            source: Some("backend".to_string()),
            level: None,
            ..LogFilter::default()
        };

        assert!(filter.matches(&event));
    }

    #[test]
    fn matches_included_property_exact_value() {
        let mut event = LogEvent::from_line(0, "INFO request completed".to_string());
        event.set_properties(vec![crate::model::LogProperty {
            key: "tenantId".to_string(),
            value: crate::model::PropertyValue::String("tenant-1".to_string()),
        }]);
        let filter = LogFilter {
            property_includes: vec![PropertyPredicate::exact("tenantId", "tenant-1")],
            ..LogFilter::default()
        };

        assert!(filter.matches(&event));
    }

    #[test]
    fn rejects_excluded_property_exact_value() {
        let mut event = LogEvent::from_line(0, "INFO request completed".to_string());
        event.set_properties(vec![crate::model::LogProperty {
            key: "statusCode".to_string(),
            value: crate::model::PropertyValue::Number("500".to_string()),
        }]);
        let filter = LogFilter {
            property_excludes: vec![PropertyPredicate::exact("statusCode", "500")],
            ..LogFilter::default()
        };

        assert!(!filter.matches(&event));
    }

    #[test]
    fn matches_property_existence_filters() {
        let mut event = LogEvent::from_line(0, "INFO request completed".to_string());
        event.set_properties(vec![crate::model::LogProperty {
            key: "requestId".to_string(),
            value: crate::model::PropertyValue::String("abc".to_string()),
        }]);
        let include = LogFilter {
            property_includes: vec![PropertyPredicate::exists("requestId")],
            ..LogFilter::default()
        };
        let exclude = LogFilter {
            property_excludes: vec![PropertyPredicate::exists("requestId")],
            ..LogFilter::default()
        };

        assert!(include.matches(&event));
        assert!(!exclude.matches(&event));
    }

    #[test]
    fn parses_property_filter_inputs() {
        assert_eq!(
            PropertyFilterUpdate::parse("tenantId=tenant-1", false).unwrap(),
            PropertyFilterUpdate {
                exclude: false,
                predicate: PropertyPredicate::exact("tenantId", "tenant-1")
            }
        );
        assert_eq!(
            PropertyFilterUpdate::parse("statusCode!=500", false).unwrap(),
            PropertyFilterUpdate {
                exclude: true,
                predicate: PropertyPredicate::exact("statusCode", "500")
            }
        );
        assert_eq!(
            PropertyFilterUpdate::parse("!debug", false).unwrap(),
            PropertyFilterUpdate {
                exclude: true,
                predicate: PropertyPredicate::exists("debug")
            }
        );
    }

    #[test]
    fn property_filter_summaries_format_include_and_exclude_filters() {
        let exact = PropertyPredicate::exact("statusCode", "500");
        let exists = PropertyPredicate::exists("debug");

        assert_eq!(exact.summary(), "statusCode=500");
        assert_eq!(exists.summary(), "debug");
        assert_eq!(exact.exclude_summary(), "statusCode!=500");
        assert_eq!(exists.exclude_summary(), "!debug");
        assert_eq!(exact.summary_for(true), "statusCode!=500");
    }

    #[test]
    fn removes_and_replaces_property_filters_by_id() {
        let mut filters = LogFilter {
            property_includes: vec![
                PropertyPredicate::exact("tenantId", "tenant-1"),
                PropertyPredicate::exists("requestId"),
            ],
            property_excludes: vec![PropertyPredicate::exact("statusCode", "500")],
            ..LogFilter::default()
        };

        assert_eq!(
            filters.remove_property_filter(PropertyFilterId {
                exclude: false,
                index: 1
            }),
            Some(PropertyPredicate::exists("requestId"))
        );
        assert!(filters.replace_property_filter(
            PropertyFilterId {
                exclude: true,
                index: 0
            },
            PropertyFilterUpdate {
                exclude: false,
                predicate: PropertyPredicate::exact("statusCode", "200")
            }
        ));

        assert_eq!(
            filters.property_includes,
            vec![
                PropertyPredicate::exact("tenantId", "tenant-1"),
                PropertyPredicate::exact("statusCode", "200"),
            ]
        );
        assert!(filters.property_excludes.is_empty());
    }
}
