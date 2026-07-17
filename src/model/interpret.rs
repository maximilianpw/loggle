use super::{
    LogEvent, LogProperty, ParsedLine, PropertyBlockHeader, SourceConfig, is_property_body_line,
    looks_like_timestamp, parse_compose_line, parse_level_token, parse_property_block_header,
    parse_property_object, parse_structured_message, split_first_token,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuredLineKind {
    None,
    LevelOnly,
    Timestamped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingPropertyLine {
    pub(crate) source: Option<String>,
    pub(crate) message: String,
    pub(crate) is_property_body: bool,
    pub(crate) structured: StructuredLineKind,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LogInterpreter;

impl LogInterpreter {
    pub(crate) fn parse_source_line(self, line: &str) -> ParsedLine {
        parse_compose_line(line)
    }

    pub(crate) fn property_block_header(self, line: &str) -> Option<PropertyBlockHeader> {
        parse_property_block_header(line)
    }

    pub(crate) fn property_object(self, input: &str) -> Option<Vec<LogProperty>> {
        parse_property_object(input)
    }

    pub(crate) fn pending_property_line(self, line: &str) -> PendingPropertyLine {
        let parsed = parse_compose_line(line);
        let structured = match parse_structured_message(&parsed.message) {
            Some(message) if message.timestamp.is_some() => StructuredLineKind::Timestamped,
            Some(_) => StructuredLineKind::LevelOnly,
            None if is_bracketed_timestamp_structured(&parsed.message) => {
                StructuredLineKind::Timestamped
            }
            None => StructuredLineKind::None,
        };
        PendingPropertyLine {
            is_property_body: is_property_body_line(&parsed.message),
            source: parsed.source_explicit.then_some(parsed.source),
            message: parsed.message,
            structured,
        }
    }

    pub(crate) fn event_from_source_line(
        self,
        sequence: u64,
        raw: String,
        parsed: ParsedLine,
        source_config: &SourceConfig,
    ) -> LogEvent {
        let mut event = LogEvent::from_parsed_line(sequence, raw, parsed);
        self.promote_source(&mut event, source_config);
        event
    }

    pub(crate) fn apply_properties(
        self,
        event: &mut LogEvent,
        properties: Vec<LogProperty>,
        source_config: &SourceConfig,
    ) {
        event.set_properties(properties);
        self.promote_source(event, source_config);
    }

    fn promote_source(self, event: &mut LogEvent, source_config: &SourceConfig) {
        if event.source != "unknown" {
            return;
        }

        for field in source_config.fields() {
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

fn is_bracketed_timestamp_structured(message: &str) -> bool {
    let Some(after_open) = message.trim().strip_prefix('[') else {
        return false;
    };
    let Some((timestamp, after_timestamp)) = after_open.split_once(']') else {
        return false;
    };
    if !looks_like_timestamp(timestamp) {
        return false;
    }

    split_first_token(after_timestamp.trim_start())
        .is_some_and(|(level, _)| parse_level_token(level).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PropertyValue, SourceConfig};

    #[test]
    fn event_construction_promotes_source_from_properties() {
        let interpreter = LogInterpreter;
        let parsed = interpreter.parse_source_line("INFO ready service=api");

        let event = interpreter.event_from_source_line(
            0,
            "INFO ready service=api".to_string(),
            parsed,
            &SourceConfig::default(),
        );

        assert_eq!(event.source, "api");
        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::Text("api".to_string()))
        );
    }

    #[test]
    fn applying_properties_can_promote_source() {
        let interpreter = LogInterpreter;
        let parsed = interpreter.parse_source_line("INFO ready");
        let mut event = interpreter.event_from_source_line(
            0,
            "INFO ready".to_string(),
            parsed,
            &SourceConfig::default(),
        );

        interpreter.apply_properties(
            &mut event,
            vec![LogProperty {
                key: "service".to_string(),
                value: PropertyValue::String("worker".to_string()),
            }],
            &SourceConfig::default(),
        );

        assert_eq!(event.source, "worker");
    }
}
