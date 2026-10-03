use super::{
    Level, LogEvent, LogProperty, ParsedLine, SourceConfig,
    block::is_property_body_line,
    clean_display_text, parse_compose_line, parse_property_block_header, parse_property_object,
    structured::{StructuredLineKind, looks_like_timestamp, structured_line_kind},
};

/// A property-block header (`[api] [10:00:00.000] INFO (#1):`) together with
/// the explicit source prefix it was logged under, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PropertyFoldHeader {
    pub(crate) timestamp: String,
    pub(crate) level: Level,
    pub(crate) source: Option<String>,
}

impl PropertyFoldHeader {
    /// Whether `event` is the summary this header describes: same time-of-day
    /// timestamp string and level and, for a sourced header, the same
    /// (ASCII-case-insensitive) canonical event source.
    pub(crate) fn matches(&self, event: &LogEvent) -> bool {
        event.timestamp.as_deref() == Some(self.timestamp.as_str())
            && event.level == self.level
            && self
                .source
                .as_deref()
                .is_none_or(|source| event.source.eq_ignore_ascii_case(source))
    }
}

/// One raw line classified for an open property fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingPropertyLine {
    /// The explicit source prefix, if the line had one.
    pub(crate) source: Option<String>,
    /// The line with any source prefix removed.
    pub(crate) message: String,
    pub(crate) is_property_body: bool,
    /// The line starts a new structured record (a timestamped summary, or a
    /// level-first summary that is not itself a property entry), so it cannot
    /// continue an open block.
    pub(crate) starts_record: bool,
}

/// Turns raw source lines into [`LogEvent`]s, promoting a source from the
/// configured property fields when the line itself did not name one.
#[derive(Debug, Clone)]
pub(crate) struct LogInterpreter {
    source_config: SourceConfig,
}

impl LogInterpreter {
    pub(crate) fn new(source_config: SourceConfig) -> Self {
        Self { source_config }
    }

    pub(crate) fn parse_source_line(&self, line: &str) -> ParsedLine {
        parse_compose_line(line)
    }

    pub(crate) fn property_block_header(&self, line: &str) -> Option<PropertyFoldHeader> {
        let header = parse_property_block_header(line)?;
        let parsed = parse_fold_line(line);
        Some(PropertyFoldHeader {
            timestamp: header.timestamp,
            level: header.level,
            source: parsed.source_explicit.then_some(parsed.source),
        })
    }

    pub(crate) fn property_object(&self, input: &str) -> Option<Vec<LogProperty>> {
        parse_property_object(input)
    }

    pub(crate) fn pending_property_line(&self, line: &str) -> PendingPropertyLine {
        let ParsedLine {
            source,
            message,
            source_explicit,
        } = parse_fold_line(line);
        let is_property_body = is_property_body_line(&message);
        let starts_record = match structured_line_kind(&message) {
            StructuredLineKind::Timestamped => true,
            StructuredLineKind::LevelOnly => !is_property_body,
            StructuredLineKind::None => false,
        };
        PendingPropertyLine {
            source: source_explicit.then_some(source),
            message,
            is_property_body,
            starts_record,
        }
    }

    pub(crate) fn event_from_source_line(
        &self,
        sequence: u64,
        raw: String,
        parsed: ParsedLine,
    ) -> LogEvent {
        let mut event = LogEvent::from_parsed_line(sequence, raw, parsed);
        self.promote_source(&mut event);
        event
    }

    pub(crate) fn apply_properties(&self, event: &mut LogEvent, properties: Vec<LogProperty>) {
        event.set_properties(properties);
        self.promote_source(event);
    }

    fn promote_source(&self, event: &mut LogEvent) {
        if event.source != "unknown" {
            return;
        }

        for field in self.source_config.fields() {
            let Some(source) = event
                .property(field)
                .map(|property| property.value.as_display_str())
            else {
                continue;
            };

            let source = source.trim();
            if !source.is_empty() {
                event.source = source.to_string();
                return;
            }
        }
    }
}

/// Splits a source prefix like [`parse_compose_line`], except that a leading
/// bracketed timestamp (`[10:00:00.000] INFO ...`) is part of the message, not
/// a source.
fn parse_fold_line(line: &str) -> ParsedLine {
    let cleaned = clean_display_text(line);
    if let Some(rest) = cleaned.strip_prefix('[')
        && let Some((candidate, _)) = rest.split_once(']')
        && looks_like_timestamp(candidate.trim())
    {
        return ParsedLine {
            source: "unknown".to_string(),
            message: cleaned,
            source_explicit: false,
        };
    }

    parse_compose_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PropertyValue;

    #[test]
    fn event_construction_promotes_source_from_properties() {
        let interpreter = LogInterpreter::new(SourceConfig::default());
        let parsed = interpreter.parse_source_line("INFO ready service=api");

        let event =
            interpreter.event_from_source_line(0, "INFO ready service=api".to_string(), parsed);

        assert_eq!(event.source, "api");
        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::Text("api".to_string()))
        );
    }

    #[test]
    fn applying_properties_can_promote_source() {
        let interpreter = LogInterpreter::new(SourceConfig::default());
        let parsed = interpreter.parse_source_line("INFO ready");
        let mut event = interpreter.event_from_source_line(0, "INFO ready".to_string(), parsed);

        interpreter.apply_properties(
            &mut event,
            vec![LogProperty {
                key: "service".to_string(),
                value: PropertyValue::String("worker".to_string()),
            }],
        );

        assert_eq!(event.source, "worker");
    }

    #[test]
    fn property_fold_header_keeps_trimmed_explicit_source() {
        let interpreter = LogInterpreter::new(SourceConfig::default());

        let header = interpreter
            .property_block_header("  api worker  | [14:06:58.892] ERROR (#147):")
            .unwrap();
        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Error);
        assert_eq!(header.source.as_deref(), Some("api worker"));

        let colored = interpreter
            .property_block_header("\u{1b}[36m[api worker ]\u{1b}[0m [14:06:58.892] WARN (#9):")
            .unwrap();
        assert_eq!(colored.level, Level::Warn);
        assert_eq!(colored.source.as_deref(), Some("api worker"));
    }

    #[test]
    fn bracketed_timestamp_is_not_a_fold_source() {
        let interpreter = LogInterpreter::new(SourceConfig::default());

        let header = interpreter
            .property_block_header("[14:06:58.892] INFO (#147):")
            .unwrap();
        assert_eq!(header.source, None);

        let line = interpreter.pending_property_line("[10:00:01.000] ERROR recovered");
        assert_eq!(line.source, None);
        assert_eq!(line.message, "[10:00:01.000] ERROR recovered");
        assert!(line.starts_record);
    }

    #[test]
    fn pending_property_line_separates_bodies_from_record_starts() {
        let interpreter = LogInterpreter::new(SourceConfig::default());

        let body = interpreter.pending_property_line("[api] error: \"failed\",");
        assert_eq!(body.source.as_deref(), Some("api"));
        assert_eq!(body.message, "error: \"failed\",");
        assert!(body.is_property_body);
        assert!(!body.starts_record);

        let summary = interpreter.pending_property_line("api | ERROR recovered");
        assert!(!summary.is_property_body);
        assert!(summary.starts_record);

        let piped_value = interpreter.pending_property_line("[api] value: \"left | right\"");
        assert_eq!(piped_value.source.as_deref(), Some("api"));
        assert_eq!(piped_value.message, "value: \"left | right\"");
    }
}
