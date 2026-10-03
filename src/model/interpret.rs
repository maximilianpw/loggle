use super::{
    LogEvent, LogProperty, ParsedLine, PropertyBlockHeader, SourceConfig,
    message_without_source_prefix, parse_compose_line, parse_property_block_header,
    parse_property_object,
};

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

    pub(crate) fn property_block_header(&self, line: &str) -> Option<PropertyBlockHeader> {
        parse_property_block_header(line)
    }

    pub(crate) fn property_object(&self, input: &str) -> Option<Vec<LogProperty>> {
        parse_property_object(input)
    }

    pub(crate) fn message_without_source_prefix(&self, line: &str) -> String {
        message_without_source_prefix(line)
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
}
