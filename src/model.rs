use std::{borrow::Cow, fmt};

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
        let structured = parse_structured_message(&message);
        let timestamp = structured
            .as_ref()
            .and_then(|message| message.timestamp.clone());
        let level = structured
            .as_ref()
            .map(|message| message.level)
            .unwrap_or_else(|| infer_level(&message));
        let message = structured
            .map(|message| message.message)
            .unwrap_or(message);
        let properties = parse_inline_properties(&message);

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

pub fn parse_compose_line(line: &str) -> ParsedLine {
    let line = clean_display_text(line);

    if let Some(parsed) = parse_bracket_prefixed_line(&line) {
        return parsed;
    }

    if let Some((source, message)) = line.split_once('|') {
        let source = source.trim().to_string();
        if !source.is_empty() {
            return ParsedLine {
                source,
                message: message.trim_start().to_string(),
                source_explicit: true,
            };
        }
    }

    ParsedLine {
        source: "unknown".to_string(),
        message: line,
        source_explicit: false,
    }
}

fn parse_bracket_prefixed_line(line: &str) -> Option<ParsedLine> {
    let rest = line.strip_prefix('[')?;
    let (source, message) = rest.split_once(']')?;
    let source = source.trim().to_string();

    (!source.is_empty()).then(|| ParsedLine {
        source,
        message: message.trim_start().to_string(),
        source_explicit: true,
    })
}

pub fn parse_structured_message(message: &str) -> Option<StructuredMessage> {
    let message = clean_display_text(message);
    let trimmed = message.trim();
    let (first, rest) = split_first_token(trimmed)?;

    if looks_like_timestamp(first) {
        let (level_token, remainder) = split_first_token(rest.trim_start())?;
        let level = Level::parse(level_token)?;
        return Some(StructuredMessage {
            timestamp: Some(first.to_string()),
            level,
            message: remainder.trim_start().to_string(),
        });
    }

    Level::parse(first).map(|level| StructuredMessage {
        timestamp: None,
        level,
        message: rest.trim_start().to_string(),
    })
}

pub fn parse_property_block_header(line: &str) -> Option<PropertyBlockHeader> {
    let message = message_without_source_prefix(line);
    let rest = message.trim().strip_prefix('[')?;
    let (timestamp, after_timestamp) = rest.split_once(']')?;
    if !looks_like_timestamp(timestamp) {
        return None;
    }

    let (level_token, after_level) = split_first_token(after_timestamp.trim_start())?;
    let level = Level::parse(level_token)?;
    let after_level = after_level.trim_start();
    let after_marker = if let Some(marker) = after_level.strip_prefix("(#") {
        let (_, after_marker) = marker.split_once(')')?;
        after_marker.trim_start()
    } else {
        after_level
    };

    after_marker.strip_prefix(':')?;
    Some(PropertyBlockHeader {
        timestamp: timestamp.to_string(),
        level,
    })
}

pub(crate) fn message_without_source_prefix(line: &str) -> String {
    let line = clean_display_text(line);

    if let Some((source, message)) = line.split_once('|') {
        if !source.trim().is_empty() {
            return message.trim_start().to_string();
        }
    }

    if let Some(rest) = line.strip_prefix('[') {
        if let Some((source, message)) = rest.split_once(']') {
            let source = source.trim();
            if !source.is_empty() && !looks_like_timestamp(source) {
                return message.trim_start().to_string();
            }
        }
    }

    line
}

fn split_first_token(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_start();
    if value.is_empty() {
        return None;
    }

    let end = value
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(value.len());
    Some((&value[..end], &value[end..]))
}

fn looks_like_timestamp(value: &str) -> bool {
    let mut has_colon = false;
    let mut has_digit = false;

    for ch in value.chars() {
        match ch {
            ':' => has_colon = true,
            '0'..='9' => has_digit = true,
            '.' => {}
            _ => return false,
        }
    }

    has_colon && has_digit && value.len() >= 5
}

pub fn parse_property_object(input: &str) -> Option<Vec<LogProperty>> {
    let mut saw_open = false;
    let mut properties = Vec::new();

    for line in input.lines() {
        let mut entry = line.trim();
        if entry.is_empty() {
            continue;
        }

        if !saw_open {
            let Some(after_open) = entry.strip_prefix('{') else {
                continue;
            };
            saw_open = true;
            entry = after_open.trim();
            if entry.is_empty() {
                continue;
            }
        }

        if entry.starts_with('}') {
            break;
        }

        let entry = trim_trailing_comma(entry);
        if entry.is_empty() || entry == "}" {
            continue;
        }

        if let Some(property) = parse_property_entry(entry) {
            properties.push(property);
        }
    }

    saw_open.then_some(properties)
}

pub fn parse_inline_properties(message: &str) -> Vec<LogProperty> {
    let mut properties = Vec::new();
    let mut index = 0;

    while index < message.len() {
        index = skip_inline_separators(message, index);
        if index >= message.len() {
            break;
        }

        let key_start = index;
        while index < message.len() {
            let Some(ch) = message[index..].chars().next() else {
                break;
            };

            if !is_inline_property_key_char(ch) {
                break;
            }

            index += ch.len_utf8();
        }

        if key_start == index || !message[index..].starts_with('=') {
            index = skip_inline_token(message, key_start);
            continue;
        }

        let key = &message[key_start..index];
        index += '='.len_utf8();
        let value_start = index;
        index = inline_property_value_end(message, value_start);
        let value = &message[value_start..index];

        if !value.is_empty() {
            properties.push(LogProperty {
                key: key.to_string(),
                value: parse_property_value(value),
            });
        }
    }

    properties
}

fn skip_inline_separators(message: &str, mut index: usize) -> usize {
    while index < message.len() {
        let Some(ch) = message[index..].chars().next() else {
            break;
        };

        if !ch.is_whitespace() {
            break;
        }

        index += ch.len_utf8();
    }

    index
}

fn skip_inline_token(message: &str, mut index: usize) -> usize {
    while index < message.len() {
        let Some(ch) = message[index..].chars().next() else {
            break;
        };

        if ch.is_whitespace() {
            break;
        }

        index += ch.len_utf8();
    }

    index
}

fn inline_property_value_end(message: &str, value_start: usize) -> usize {
    let Some(quote) = message[value_start..].chars().next() else {
        return value_start;
    };

    if quote != '"' && quote != '\'' {
        return skip_inline_token(message, value_start);
    }

    let mut index = value_start + quote.len_utf8();
    let mut escaped = false;
    while index < message.len() {
        let Some(ch) = message[index..].chars().next() else {
            break;
        };

        index += ch.len_utf8();
        if escaped {
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if ch == quote {
            return index;
        }
    }

    skip_inline_token(message, value_start)
}

fn is_inline_property_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}

fn parse_property_entry(entry: &str) -> Option<LogProperty> {
    let (key, value) = entry.split_once(':')?;
    let key = parse_property_key(key.trim())?;
    let value = parse_property_value(value.trim());

    Some(LogProperty { key, value })
}

fn parse_property_key(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }

    if let Some(value) = parse_quoted_string(key) {
        return Some(value);
    }

    Some(key.to_string())
}

fn parse_property_value(value: &str) -> PropertyValue {
    let value = trim_trailing_comma(value.trim());

    if let Some(value) = parse_quoted_string(value) {
        return PropertyValue::String(value);
    }

    match value {
        "true" => PropertyValue::Bool(true),
        "false" => PropertyValue::Bool(false),
        "null" => PropertyValue::Null,
        value if is_number_literal(value) => PropertyValue::Number(value.to_string()),
        value => PropertyValue::Text(value.to_string()),
    }
}

fn trim_trailing_comma(value: &str) -> &str {
    value.trim_end()
        .strip_suffix(',')
        .map(str::trim_end)
        .unwrap_or(value.trim_end())
}

fn parse_quoted_string(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }

    let mut output = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            match ch {
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                't' => output.push('\t'),
                '\\' => output.push('\\'),
                '"' => output.push('"'),
                '\'' => output.push('\''),
                value => output.push(value),
            }
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if ch == quote {
            return Some(output);
        }

        output.push(ch);
    }

    None
}

fn is_number_literal(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }

    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let (whole, fraction) = if let Some((whole, fraction)) = unsigned.split_once('.') {
        (whole, fraction)
    } else {
        (unsigned, "")
    };

    !whole.is_empty()
        && whole.chars().all(|ch| ch.is_ascii_digit())
        && fraction.chars().all(|ch| ch.is_ascii_digit())
}

pub fn clean_display_text(input: &str) -> String {
    strip_control_chars(&strip_ansi_escapes(input))
}

fn strip_ansi_escapes(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            output.push(ch);
            continue;
        }

        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for value in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&value) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(value) = chars.next() {
                    if value == '\u{7}' {
                        break;
                    }

                    if value == '\x1b' && chars.peek().copied() == Some('\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }

    output
}

fn strip_control_chars(input: &str) -> String {
    input
        .chars()
        .filter_map(|ch| match ch {
            '\t' => Some(' '),
            ch if ch.is_control() => None,
            ch => Some(ch),
        })
        .collect()
}

pub fn infer_level(message: &str) -> Level {
    let mut inferred = Level::Unknown;
    for level in message
        .split(|value: char| !value.is_ascii_alphanumeric())
        .filter_map(Level::parse)
    {
        if level == Level::Fatal {
            return Level::Fatal;
        }

        if level_priority(level) > level_priority(inferred) {
            inferred = level;
        }
    }

    inferred
}

fn level_priority(level: Level) -> u8 {
    match level {
        Level::Fatal => 6,
        Level::Error => 5,
        Level::Warn => 4,
        Level::Info => 3,
        Level::Debug => 2,
        Level::Trace => 1,
        Level::Unknown => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compose_line_with_standard_spacing() {
        let parsed = parse_compose_line("api | ERROR failed");

        assert_eq!(parsed.source, "api");
        assert_eq!(parsed.message, "ERROR failed");
    }

    #[test]
    fn parses_compose_line_with_spacing_variants() {
        let parsed = parse_compose_line("  worker  |    started");

        assert_eq!(parsed.source, "worker");
        assert_eq!(parsed.message, "started");
    }

    #[test]
    fn parses_concurrently_named_prefix() {
        let parsed = parse_compose_line("[frontend] VITE ready");

        assert_eq!(parsed.source, "frontend");
        assert_eq!(parsed.message, "VITE ready");
        assert!(parsed.source_explicit);
    }

    #[test]
    fn parses_colored_concurrently_named_prefix() {
        let parsed = parse_compose_line("\u{1b}[36m[backend]\u{1b}[0m INFO ready");

        assert_eq!(parsed.source, "backend");
        assert_eq!(parsed.message, "INFO ready");
        assert!(parsed.source_explicit);
    }

    #[test]
    fn parses_colored_concurrently_padded_prefix() {
        let parsed = parse_compose_line("\u{1b}[35m[backend ]\u{1b}[0m ERROR failed");

        assert_eq!(parsed.source, "backend");
        assert_eq!(parsed.message, "ERROR failed");
        assert!(parsed.source_explicit);
    }

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
    fn parses_inline_key_value_properties() {
        let event = LogEvent::from_line(
            0,
            "INFO request completed service=backend app=frontend logger=api".to_string(),
        );

        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::Text("backend".to_string()))
        );
        assert_eq!(
            event.property("app").map(|property| &property.value),
            Some(&PropertyValue::Text("frontend".to_string()))
        );
        assert_eq!(
            event.property("logger").map(|property| &property.value),
            Some(&PropertyValue::Text("api".to_string()))
        );
    }

    #[test]
    fn parses_quoted_inline_property_values() {
        let event =
            LogEvent::from_line(0, "INFO request completed service=\"api server\"".to_string());

        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::String("api server".to_string()))
        );
    }

    #[test]
    fn parses_structured_summary_with_timestamp_level_and_message() {
        let event = LogEvent::from_line(
            0,
            "14:06:58.892 INFO http.request GET /api/v1/inventory 200 96ms".to_string(),
        );

        assert_eq!(event.source, "unknown");
        assert_eq!(event.timestamp.as_deref(), Some("14:06:58.892"));
        assert_eq!(event.level, Level::Info);
        assert_eq!(
            event.message,
            "http.request GET /api/v1/inventory 200 96ms"
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

    #[test]
    fn parses_level_first_structured_summary() {
        let event = LogEvent::from_line(0, "ERROR sync.failed retry exhausted".to_string());

        assert_eq!(event.timestamp, None);
        assert_eq!(event.level, Level::Error);
        assert_eq!(event.message, "sync.failed retry exhausted");
    }

    #[test]
    fn parses_concurrently_padded_prefix() {
        let parsed = parse_compose_line("[backend ] ERROR failed");

        assert_eq!(parsed.source, "backend");
        assert_eq!(parsed.message, "ERROR failed");
    }

    #[test]
    fn parses_concurrently_numeric_prefix() {
        let parsed = parse_compose_line("[0] started");

        assert_eq!(parsed.source, "0");
        assert_eq!(parsed.message, "started");
        assert!(parsed.source_explicit);
    }

    #[test]
    fn falls_back_to_unknown_for_raw_lines() {
        let parsed = parse_compose_line("plain line with no prefix");

        assert_eq!(parsed.source, "unknown");
        assert_eq!(parsed.message, "plain line with no prefix");
        assert!(!parsed.source_explicit);
    }

    #[test]
    fn keeps_unprefixed_vite_and_api_lines_unknown() {
        let vite = parse_compose_line("VITE v5.4.0  ready in 200 ms");
        let api = parse_compose_line("GET /api/v1/auth/me 200");

        assert_eq!(vite.source, "unknown");
        assert_eq!(vite.message, "VITE v5.4.0  ready in 200 ms");
        assert_eq!(api.source, "unknown");
        assert_eq!(api.message, "GET /api/v1/auth/me 200");
    }

    #[test]
    fn strips_ansi_sequences_from_parsed_messages() {
        let parsed = parse_compose_line(
            "nestjs-backend | \u{1b}[32m[Nest] 32 - \u{1b}[39m05/08/2026 LOG ready",
        );

        assert_eq!(parsed.source, "nestjs-backend");
        assert_eq!(parsed.message, "[Nest] 32 - 05/08/2026 LOG ready");
    }

    #[test]
    fn strips_cursor_control_sequences_from_parsed_messages() {
        let parsed = parse_compose_line(
            "nestjs-backend | \u{1b}[J\u{1b}[3J\u{1b}[H[\u{1b}[90m4:25:35 PM\u{1b}[0m] Starting compilation",
        );

        assert_eq!(parsed.message, "[4:25:35 PM] Starting compilation");
    }

    #[test]
    fn strips_carriage_returns_and_other_control_chars_from_parsed_messages() {
        let parsed = parse_compose_line("api | progress 10%\rprogress 20%\u{8}\tready");

        assert_eq!(parsed.message, "progress 10%progress 20% ready");
    }

    #[test]
    fn parses_property_block_header() {
        let header = parse_property_block_header("[14:06:58.892] INFO (#147):").unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Info);
        assert!(parse_property_block_header("[frontend] VITE ready").is_none());
    }

    #[test]
    fn parses_prefixed_property_block_header() {
        let header =
            parse_property_block_header("[backend] [14:06:58.892] INFO (#147):").unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Info);
    }

    #[test]
    fn parses_colored_prefixed_property_block_header() {
        let header = parse_property_block_header(
            "\u{1b}[36m[backend]\u{1b}[0m [14:06:58.892] INFO (#147):",
        )
        .unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Info);
    }

    #[test]
    fn parses_js_like_property_object() {
        let properties = parse_property_object(
            r#"  {
    messageKey: "http.request",
    statusCode: 200,
    durationMs: 96,
    cached: false,
    metadata: null,
    userAgent: "Mozilla/5.0 (KHTML, like Gecko)",
  }"#,
        )
        .unwrap();

        assert_eq!(properties[0].key, "messageKey");
        assert_eq!(
            properties[0].value,
            PropertyValue::String("http.request".to_string())
        );
        assert_eq!(properties[1].value, PropertyValue::Number("200".to_string()));
        assert_eq!(properties[2].value, PropertyValue::Number("96".to_string()));
        assert_eq!(properties[3].value, PropertyValue::Bool(false));
        assert_eq!(properties[4].value, PropertyValue::Null);
        assert_eq!(
            properties[5].value,
            PropertyValue::String("Mozilla/5.0 (KHTML, like Gecko)".to_string())
        );
    }

    #[test]
    fn infers_common_levels_case_insensitively() {
        assert_eq!(infer_level("FATAL crash"), Level::Fatal);
        assert_eq!(infer_level("ERROR failed"), Level::Error);
        assert_eq!(infer_level("Warning: retrying"), Level::Warn);
        assert_eq!(infer_level("info: listening"), Level::Info);
        assert_eq!(
            infer_level("[Nest] 32 - 05/08/2026, 4:18:15 PM LOG [NestFactory] Starting Nest application..."),
            Level::Info
        );
        assert_eq!(infer_level("debug details"), Level::Debug);
        assert_eq!(infer_level("trace span"), Level::Trace);
        assert_eq!(infer_level("verbose route mapped"), Level::Trace);
    }

    #[test]
    fn infers_unknown_without_level_tokens() {
        assert_eq!(infer_level("request completed"), Level::Unknown);
    }
}
