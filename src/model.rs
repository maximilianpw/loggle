mod block;
mod buildkit;
mod compose;
mod inline;
mod interpret;
mod json;
mod structured;
mod text;

use std::{borrow::Cow, fmt};

pub use block::{parse_property_block_header, parse_property_object};
pub(crate) use buildkit::parse_buildkit_step_line;
pub use compose::parse_compose_line;
pub use inline::parse_inline_properties;
pub(crate) use interpret::{LogInterpreter, PropertyFoldHeader};
use json::{parse_json_log_message, split_trailing_json_properties};
pub use structured::{infer_level, parse_structured_message};
pub use text::clean_display_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Unknown,
}

impl Level {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("fatal") {
            Some(Self::Fatal)
        } else if input.eq_ignore_ascii_case("error") || input.eq_ignore_ascii_case("err") {
            Some(Self::Error)
        } else if input.eq_ignore_ascii_case("warn") || input.eq_ignore_ascii_case("warning") {
            Some(Self::Warn)
        } else if input.eq_ignore_ascii_case("info") || input.eq_ignore_ascii_case("log") {
            Some(Self::Info)
        } else if input.eq_ignore_ascii_case("debug") {
            Some(Self::Debug)
        } else if input.eq_ignore_ascii_case("trace") || input.eq_ignore_ascii_case("verbose") {
            Some(Self::Trace)
        } else if input.eq_ignore_ascii_case("unknown") {
            Some(Self::Unknown)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fatal => "fatal",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyValue {
    String(String),
    Number(String),
    Bool(bool),
    Null,
    Text(String),
}

impl PropertyValue {
    pub(crate) fn as_display_str(&self) -> Cow<'_, str> {
        match self {
            Self::String(value) | Self::Number(value) | Self::Text(value) => Cow::Borrowed(value),
            Self::Bool(true) => Cow::Borrowed("true"),
            Self::Bool(false) => Cow::Borrowed("false"),
            Self::Null => Cow::Borrowed("null"),
        }
    }
}

impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_display_str().as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogProperty {
    pub key: String,
    pub value: PropertyValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceConfig {
    fields: Vec<String>,
}

impl SourceConfig {
    pub fn with_fields<I, S>(fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut config = Self { fields: Vec::new() };
        for field in fields {
            config.push_field(field.as_ref());
        }
        for field in DEFAULT_SOURCE_FIELDS {
            config.push_field(field);
        }
        config
    }

    pub(crate) fn fields(&self) -> &[String] {
        &self.fields
    }

    fn push_field(&mut self, field: &str) {
        let field = field.trim();
        if field.is_empty() || self.fields.iter().any(|existing| existing == field) {
            return;
        }

        self.fields.push(field.to_string());
    }
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self::with_fields(Vec::<String>::new())
    }
}

const DEFAULT_SOURCE_FIELDS: &[&str] =
    &["source", "service", "app", "logger", "target", "component"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub source: String,
    pub message: String,
    pub source_explicit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildKitStepLine {
    pub(crate) step_id: String,
    pub(crate) source: Option<String>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredMessage {
    pub timestamp: Option<String>,
    pub level: Level,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyBlockHeader {
    pub timestamp: String,
    pub level: Level,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEvent {
    pub sequence: u64,
    pub source: String,
    pub timestamp: Option<String>,
    pub level: Level,
    pub raw: String,
    pub message: String,
    pub properties: Vec<LogProperty>,
}

impl LogEvent {
    #[cfg(test)]
    pub fn from_line(sequence: u64, raw: String) -> Self {
        let parsed = parse_compose_line(&raw);
        Self::from_parsed_line(sequence, raw, parsed)
    }

    pub(crate) fn from_parsed_line(sequence: u64, raw: String, parsed: ParsedLine) -> Self {
        let ParsedLine {
            source, message, ..
        } = parsed;
        if let Some(json_log) = parse_json_log_message(&message) {
            return Self {
                sequence,
                source,
                timestamp: json_log.timestamp,
                level: json_log.level,
                raw,
                message: json_log.message,
                properties: json_log.properties,
            };
        }

        let structured = parse_structured_message(&message);
        let timestamp = structured
            .as_ref()
            .and_then(|message| message.timestamp.clone());
        let level = structured
            .as_ref()
            .map(|message| message.level)
            .unwrap_or_else(|| infer_level(&message));
        let message = structured.map(|message| message.message).unwrap_or(message);
        let (message, trailing_json_properties) = split_trailing_json_properties(&message);
        let mut properties = parse_inline_properties(&message);
        properties.extend(trailing_json_properties);

        Self {
            sequence,
            source,
            timestamp,
            level,
            raw,
            message,
            properties,
        }
    }

    pub fn set_properties(&mut self, properties: Vec<LogProperty>) {
        self.properties = properties;
    }

    pub fn property(&self, key: &str) -> Option<&LogProperty> {
        self.properties.iter().find(|property| property.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_concurrently_backend_prefix_with_level() {
        let raw = "[backend] INFO http.request GET /api/v1/auth/me 200".to_string();
        let event = LogEvent::from_line(0, raw.clone());

        assert_eq!(event.source, "backend");
        assert_eq!(event.message, "http.request GET /api/v1/auth/me 200");
        assert_eq!(event.level, Level::Info);
        assert_eq!(event.raw, raw);
    }

    #[test]
    fn parses_winston_console_line_with_trailing_json_properties() {
        let event = LogEvent::from_line(
            0,
            r#"vev-mcp | 2026-06-04 13:30:19 debug: Retrieved conversation history {"module":"Function","unknown":{"userId":"user-1","tenantId":"tenant-1","messageCount":13}}"#.to_string(),
        );

        assert_eq!(event.source, "vev-mcp");
        assert_eq!(event.timestamp.as_deref(), Some("2026-06-04 13:30:19"));
        assert_eq!(event.level, Level::Debug);
        assert_eq!(event.message, "Retrieved conversation history");
        assert_eq!(
            event.property("module").map(|property| &property.value),
            Some(&PropertyValue::String("Function".to_string()))
        );
        assert_eq!(
            event
                .property("unknown.userId")
                .map(|property| &property.value),
            Some(&PropertyValue::String("user-1".to_string()))
        );
        assert_eq!(
            event
                .property("unknown.messageCount")
                .map(|property| &property.value),
            Some(&PropertyValue::Number("13".to_string()))
        );
    }
    #[test]
    fn parses_compose_prefixed_structured_summary() {
        let event = LogEvent::from_line(
            0,
            "api | 14:06:58.892 WARNING http.request failed".to_string(),
        );

        assert_eq!(event.source, "api");
        assert_eq!(event.timestamp.as_deref(), Some("14:06:58.892"));
        assert_eq!(event.level, Level::Warn);
        assert_eq!(event.message, "http.request failed");
    }
}
