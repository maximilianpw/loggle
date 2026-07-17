mod interpret;

use std::{borrow::Cow, fmt};

use serde_json::{Map, Value};

pub(crate) use interpret::{LogInterpreter, StructuredLineKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
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
    configured_fields: Vec<String>,
    fields: Vec<String>,
}

impl SourceConfig {
    pub fn with_fields<I, S>(fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut configured_fields = Vec::new();
        for field in fields {
            Self::push_unique_field(&mut configured_fields, field.as_ref());
        }

        let mut fields = configured_fields.clone();
        for field in DEFAULT_SOURCE_FIELDS {
            Self::push_unique_field(&mut fields, field);
        }

        Self {
            configured_fields,
            fields,
        }
    }

    pub(crate) fn configured_fields(&self) -> &[String] {
        &self.configured_fields
    }

    pub(crate) fn fields(&self) -> &[String] {
        &self.fields
    }

    pub(crate) fn merged_with_configured_fields<I, S>(&self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut merged = self.configured_fields.clone();
        for field in fields {
            Self::push_unique_field(&mut merged, field.as_ref());
        }

        Self::with_fields(merged)
    }

    fn push_unique_field(fields: &mut Vec<String>, field: &str) {
        let field = field.trim();
        if field.is_empty() || fields.iter().any(|existing| existing == field) {
            return;
        }

        fields.push(field.to_string());
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
struct StructuredJsonLog {
    timestamp: Option<String>,
    level: Level,
    message: String,
    properties: Vec<LogProperty>,
}

const JSON_MESSAGE_KEYS: &[&str] = &["message", "msg", "log", "Log"];
const JSON_LEVEL_KEYS: &[&str] = &["level", "severity"];
const JSON_TIMESTAMP_KEYS: &[&str] = &["timestamp", "time", "ts", "@timestamp"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyBlockHeader {
    pub timestamp: String,
    pub level: Level,
    pub source: Option<String>,
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

pub fn parse_compose_line(line: &str) -> ParsedLine {
    let SourceMessage { source, message } = split_source_message(line);

    if let Some(source) = source {
        return ParsedLine {
            source,
            message,
            source_explicit: true,
        };
    }

    if let Some(parsed) = parse_compose_status_line(&message) {
        return parsed;
    }

    ParsedLine {
        source: "unknown".to_string(),
        message,
        source_explicit: false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceMessage {
    source: Option<String>,
    message: String,
}

fn split_source_message(line: &str) -> SourceMessage {
    let line = clean_display_text(line);

    if let Some(rest) = line.strip_prefix('[')
        && let Some((candidate, message)) = rest.split_once(']')
    {
        let candidate = candidate.trim();
        if looks_like_timestamp(candidate) {
            return SourceMessage {
                source: None,
                message: line,
            };
        }

        if !candidate.is_empty() {
            return SourceMessage {
                source: Some(candidate.to_string()),
                message: message.trim_start().to_string(),
            };
        }
    }

    if let Some((source, message)) = line.split_once('|') {
        let source = source.trim();
        if !source.is_empty() {
            return SourceMessage {
                source: Some(source.to_string()),
                message: message.trim_start().to_string(),
            };
        }
    }

    SourceMessage {
        source: None,
        message: line,
    }
}

pub(crate) fn parse_buildkit_step_line(line: &str) -> Option<BuildKitStepLine> {
    let line = clean_display_text(line);
    let trimmed = line.trim_start();
    let after_hash = trimmed.strip_prefix('#')?;
    let id_end = after_hash
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))?;
    let step_number = &after_hash[..id_end];
    if step_number.is_empty() || !step_number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let step_id = format!("#{step_number}");
    let after_step = after_hash[id_end..].trim_start();
    let Some(after_open) = after_step.strip_prefix('[') else {
        return Some(BuildKitStepLine {
            step_id,
            source: None,
            message: trimmed.to_string(),
        });
    };

    let (context, after_context) = after_open.split_once(']')?;
    let context = context.trim();
    let source = buildkit_context_source(context);
    let message = buildkit_message_without_service(&step_id, context, after_context);

    Some(BuildKitStepLine {
        step_id,
        source,
        message,
    })
}

fn parse_compose_status_line(line: &str) -> Option<ParsedLine> {
    let trimmed = line.trim_start();
    let (source, rest) = split_first_token(trimmed)?;
    if !looks_like_source_token(source) {
        return None;
    }

    let rest = rest.trim_start();
    let (status, _) = split_first_token(rest)?;
    let status = status.trim_end_matches(':');
    if !COMPOSE_STATUS_TOKENS
        .iter()
        .any(|candidate| status.eq_ignore_ascii_case(candidate))
    {
        return None;
    }

    Some(ParsedLine {
        source: source.to_string(),
        message: rest.to_string(),
        source_explicit: true,
    })
}

fn buildkit_context_source(context: &str) -> Option<String> {
    let parts = context.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }

    let source = parts[0];
    if !looks_like_source_token(source) || is_reserved_buildkit_source(source) {
        return None;
    }

    let second = parts[1];
    if second.eq_ignore_ascii_case("internal") {
        return Some(source.to_string());
    }

    (parts.len() >= 3
        && parts
            .last()
            .is_some_and(|part| looks_like_buildkit_step_count(part)))
    .then(|| source.to_string())
}

fn buildkit_message_without_service(step_id: &str, context: &str, after_context: &str) -> String {
    let mut context_parts = context.split_whitespace();
    let context_without_source = if buildkit_context_source(context).is_some() {
        context_parts.next();
        context_parts.collect::<Vec<_>>().join(" ")
    } else {
        context.to_string()
    };
    let after_context = after_context.trim_start();

    if context_without_source.is_empty() {
        format!("{step_id} {after_context}").trim_end().to_string()
    } else if after_context.is_empty() {
        format!("{step_id} [{context_without_source}]")
    } else {
        format!("{step_id} [{context_without_source}] {after_context}")
    }
}

fn is_reserved_buildkit_source(source: &str) -> bool {
    source.eq_ignore_ascii_case("internal") || source.eq_ignore_ascii_case("auth")
}

fn looks_like_buildkit_step_count(value: &str) -> bool {
    let Some((current, total)) = value.split_once('/') else {
        return false;
    };

    !current.is_empty()
        && !total.is_empty()
        && current.chars().all(|ch| ch.is_ascii_digit())
        && total.chars().all(|ch| ch.is_ascii_digit())
}

fn looks_like_source_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

const COMPOSE_STATUS_TOKENS: &[&str] = &[
    "Pulling",
    "Pulled",
    "Building",
    "Built",
    "Error",
    "Started",
    "Starting",
    "Healthy",
    "Waiting",
    "Exited",
    "Recreated",
    "Recreating",
    "Running",
    "Created",
    "Creating",
];

pub fn parse_structured_message(message: &str) -> Option<StructuredMessage> {
    let message = clean_display_text(message);
    let trimmed = message.trim();
    let (first, rest) = split_first_token(trimmed)?;

    if looks_like_date(first) {
        let (time, rest) = split_first_token(rest.trim_start())?;
        if looks_like_timestamp(time) {
            let (level_token, remainder) = split_first_token(rest.trim_start())?;
            let level = parse_level_token(level_token)?;
            return Some(StructuredMessage {
                timestamp: Some(format!("{first} {time}")),
                level,
                message: remainder.trim_start().to_string(),
            });
        }
    }

    if looks_like_timestamp(first) {
        let (level_token, remainder) = split_first_token(rest.trim_start())?;
        let level = parse_level_token(level_token)?;
        return Some(StructuredMessage {
            timestamp: Some(first.to_string()),
            level,
            message: remainder.trim_start().to_string(),
        });
    }

    parse_level_token(first).map(|level| StructuredMessage {
        timestamp: None,
        level,
        message: rest.trim_start().to_string(),
    })
}

pub fn parse_property_block_header(line: &str) -> Option<PropertyBlockHeader> {
    let source = split_source_message(line).source;
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
        source,
    })
}

pub(crate) fn message_without_source_prefix(line: &str) -> String {
    split_source_message(line).message
}

fn is_property_body_line(message: &str) -> bool {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return true;
    }

    if trimmed
        .chars()
        .all(|ch| ch.is_whitespace() || matches!(ch, '{' | '}' | '[' | ']' | ','))
    {
        return true;
    }

    if parse_property_object(trimmed).is_some() {
        return true;
    }

    let scalar = trim_trailing_comma(trimmed);
    parse_property_entry(scalar).is_some()
        || is_complete_quoted_scalar(scalar)
        || is_number_literal(scalar)
        || matches!(scalar, "true" | "false" | "null")
}

fn is_complete_quoted_scalar(value: &str) -> bool {
    let Some(quote) = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
    else {
        return false;
    };
    if value.len() < 2 || !value.ends_with(quote) {
        return false;
    }

    let mut escaped = false;
    for ch in value[quote.len_utf8()..value.len() - quote.len_utf8()].chars() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return false;
        }
    }
    !escaped
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

fn looks_like_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(year) = parts.next() else {
        return false;
    };
    let Some(month) = parts.next() else {
        return false;
    };
    let Some(day) = parts.next() else {
        return false;
    };

    parts.next().is_none()
        && year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && year.chars().all(|ch| ch.is_ascii_digit())
        && month.chars().all(|ch| ch.is_ascii_digit())
        && day.chars().all(|ch| ch.is_ascii_digit())
}

fn parse_level_token(token: &str) -> Option<Level> {
    Level::parse(token.trim_end_matches(':'))
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
    if let Some(properties) = parse_inline_json_properties(message) {
        return properties;
    }

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

fn split_trailing_json_properties(message: &str) -> (String, Vec<LogProperty>) {
    for (index, ch) in message.char_indices() {
        if ch != '{' {
            continue;
        }

        let prefix = message[..index].trim_end();
        if prefix.is_empty() {
            continue;
        }

        let Some(object) = parse_json_object(message[index..].trim()) else {
            continue;
        };

        return (prefix.to_string(), json_object_properties(&object, &[]));
    }

    (message.to_string(), Vec::new())
}

fn parse_json_log_message(message: &str) -> Option<StructuredJsonLog> {
    let trimmed = message.trim();
    let object = parse_json_object(trimmed)?;
    parse_json_log_object(&object)
}

fn parse_json_log_object(object: &Map<String, Value>) -> Option<StructuredJsonLog> {
    if let Some(log) = parse_embedded_json_log(object) {
        return Some(log);
    }

    let message = first_json_string_field(object, JSON_MESSAGE_KEYS)?;
    let timestamp = first_json_string_field(object, JSON_TIMESTAMP_KEYS);
    let level = first_json_string_field(object, JSON_LEVEL_KEYS)
        .and_then(|level| Level::parse(&level))
        .unwrap_or_else(|| infer_level(&message));
    let properties = json_object_properties(object, JSON_ROW_KEYS);

    Some(StructuredJsonLog {
        timestamp,
        level,
        message: clean_json_message(&message),
        properties,
    })
}

fn parse_inline_json_properties(message: &str) -> Option<Vec<LogProperty>> {
    let trimmed = message.trim();
    if let Some(object) = parse_json_object(trimmed) {
        return Some(json_object_properties(&object, &[]));
    }

    let inner = trimmed.strip_prefix('{')?.strip_suffix('}')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }

    let mut properties = Vec::new();
    for entry in split_top_level_commas(inner) {
        if let Some(property) = parse_property_entry(entry.trim()) {
            properties.push(property);
        }
    }

    Some(properties)
}

fn parse_json_object(input: &str) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(input).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

const JSON_ROW_KEYS: &[&str] = &[
    "level",
    "severity",
    "message",
    "msg",
    "log",
    "Log",
    "timestamp",
    "time",
    "ts",
    "@timestamp",
];

fn parse_embedded_json_log(object: &Map<String, Value>) -> Option<StructuredJsonLog> {
    for key in JSON_MESSAGE_KEYS {
        let Some(raw_message) = json_string_field(object, key) else {
            continue;
        };
        let inner_object = parse_json_object(clean_json_message(&raw_message).trim());
        let Some(inner_object) = inner_object else {
            continue;
        };
        let Some(mut inner_log) = parse_json_log_object(&inner_object) else {
            continue;
        };

        inner_log.timestamp = inner_log
            .timestamp
            .or_else(|| first_json_string_field(object, JSON_TIMESTAMP_KEYS));
        if inner_log.level == Level::Unknown {
            if let Some(level) = first_json_string_field(object, JSON_LEVEL_KEYS)
                .and_then(|level| Level::parse(&level))
            {
                inner_log.level = level;
            }
        }

        let mut properties = json_object_properties(object, JSON_ROW_KEYS);
        properties.extend(inner_log.properties);
        inner_log.properties = properties;
        return Some(inner_log);
    }

    None
}

fn clean_json_message(message: &str) -> String {
    message
        .trim_end_matches(|ch| ch == '\r' || ch == '\n')
        .to_string()
}

fn first_json_string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| json_string_field(object, key))
}

fn json_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(ToString::to_string)
}

fn json_object_properties(object: &Map<String, Value>, excluded_keys: &[&str]) -> Vec<LogProperty> {
    let mut properties = Vec::new();
    for (key, value) in object {
        if excluded_keys.iter().any(|excluded| key == excluded) {
            continue;
        }

        flatten_json_property(key, value, &mut properties);
    }

    properties
}

fn flatten_json_property(key: &str, value: &Value, properties: &mut Vec<LogProperty>) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (nested_key, nested_value) in object {
                flatten_json_property(&format!("{key}.{nested_key}"), nested_value, properties);
            }
        }
        value => properties.push(LogProperty {
            key: key.to_string(),
            value: json_property_value(value),
        }),
    }
}

fn json_property_value(value: &Value) -> PropertyValue {
    match value {
        Value::String(value) => PropertyValue::String(value.clone()),
        Value::Number(value) => PropertyValue::Number(value.to_string()),
        Value::Bool(value) => PropertyValue::Bool(*value),
        Value::Null => PropertyValue::Null,
        Value::Array(_) | Value::Object(_) => PropertyValue::Text(value.to_string()),
    }
}

fn split_top_level_commas(input: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }

        if ch == ',' {
            entries.push(&input[start..index]);
            start = index + ch.len_utf8();
        }
    }

    entries.push(&input[start..]);
    entries
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
    value
        .trim_end()
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
    fn source_config_equality_preserves_configured_field_precedence() {
        let defaults = SourceConfig::default();
        let explicit_default = SourceConfig::with_fields(["source"]);

        assert_ne!(defaults, explicit_default);

        let default_reader = defaults.merged_with_configured_fields(["session"]);
        let explicit_reader = explicit_default.merged_with_configured_fields(["session"]);
        assert_eq!(&default_reader.fields()[..2], ["session", "source"]);
        assert_eq!(&explicit_reader.fields()[..2], ["source", "session"]);
    }

    #[test]
    fn source_config_distinguishes_configured_fields_from_defaults() {
        let config = SourceConfig::with_fields(["tenant", "service", "tenant", " "]);

        assert_eq!(config.configured_fields(), ["tenant", "service"]);
        assert_eq!(
            config.fields(),
            [
                "tenant",
                "service",
                "source",
                "app",
                "logger",
                "target",
                "component",
            ]
        );
        assert!(SourceConfig::default().configured_fields().is_empty());
    }

    #[test]
    fn source_config_merges_reader_fields_before_persisted_fields() {
        let reader = SourceConfig::with_fields(["reader", "shared"]);
        let merged =
            reader.merged_with_configured_fields(["session", "shared", "source", "session"]);

        assert_eq!(
            merged.configured_fields(),
            ["reader", "shared", "session", "source"]
        );
        assert_eq!(
            merged.fields(),
            [
                "reader",
                "shared",
                "session",
                "source",
                "service",
                "app",
                "logger",
                "target",
                "component",
            ]
        );
    }

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
        let event = LogEvent::from_line(
            0,
            "INFO request completed service=\"api server\"".to_string(),
        );

        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::String("api server".to_string()))
        );
    }

    #[test]
    fn parses_single_line_json_properties() {
        let event = LogEvent::from_line(
            0,
            r#"api | {"requestId":"abc-123","statusCode":200,"ok":true}"#.to_string(),
        );

        assert_eq!(
            event.property("requestId").map(|property| &property.value),
            Some(&PropertyValue::String("abc-123".to_string()))
        );
        assert_eq!(
            event.property("statusCode").map(|property| &property.value),
            Some(&PropertyValue::Number("200".to_string()))
        );
        assert_eq!(
            event.property("ok").map(|property| &property.value),
            Some(&PropertyValue::Bool(true))
        );
    }

    #[test]
    fn parses_structured_json_log_message() {
        let event = LogEvent::from_line(
            0,
            r#"vev-mcp | {"level":"debug","message":"Retrieved conversation history","module":"Function","service":"vev-mcp","vev-mcp":{"messageCount":13,"tenantId":"tenant-1","userId":"user-1"}}"#.to_string(),
        );

        assert_eq!(event.source, "vev-mcp");
        assert_eq!(event.level, Level::Debug);
        assert_eq!(event.message, "Retrieved conversation history");
        assert!(event.property("message").is_none());
        assert!(event.property("level").is_none());
        assert_eq!(
            event.property("module").map(|property| &property.value),
            Some(&PropertyValue::String("Function".to_string()))
        );
        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::String("vev-mcp".to_string()))
        );
        assert_eq!(
            event
                .property("vev-mcp.messageCount")
                .map(|property| &property.value),
            Some(&PropertyValue::Number("13".to_string()))
        );
        assert_eq!(
            event
                .property("vev-mcp.tenantId")
                .map(|property| &property.value),
            Some(&PropertyValue::String("tenant-1".to_string()))
        );
        assert_eq!(
            event
                .property("vev-mcp.userId")
                .map(|property| &property.value),
            Some(&PropertyValue::String("user-1".to_string()))
        );
    }

    #[test]
    fn parses_json_log_embedded_in_message_field() {
        let event = LogEvent::from_line(
            0,
            r#"api | {"message":"{\"level\":\"debug\",\"message\":\"Retrieved conversation history\",\"module\":\"Function\",\"service\":\"vev-mcp\",\"vev-mcp\":{\"messageCount\":13}}","stream":"stdout","time":"2026-06-04T13:00:20Z"}"#.to_string(),
        );

        assert_eq!(event.source, "api");
        assert_eq!(event.timestamp.as_deref(), Some("2026-06-04T13:00:20Z"));
        assert_eq!(event.level, Level::Debug);
        assert_eq!(event.message, "Retrieved conversation history");
        assert_eq!(
            event.property("stream").map(|property| &property.value),
            Some(&PropertyValue::String("stdout".to_string()))
        );
        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::String("vev-mcp".to_string()))
        );
        assert_eq!(
            event
                .property("vev-mcp.messageCount")
                .map(|property| &property.value),
            Some(&PropertyValue::Number("13".to_string()))
        );
        assert!(event.property("message").is_none());
        assert!(event.property("log").is_none());
    }

    #[test]
    fn parses_json_log_embedded_in_docker_log_field() {
        let event = LogEvent::from_line(
            0,
            r#"{"log":"{\"level\":\"info\",\"message\":\"ready\",\"service\":\"vev-mcp\"}\n","stream":"stdout","time":"2026-06-04T13:00:20Z"}"#.to_string(),
        );

        assert_eq!(event.source, "unknown");
        assert_eq!(event.timestamp.as_deref(), Some("2026-06-04T13:00:20Z"));
        assert_eq!(event.level, Level::Info);
        assert_eq!(event.message, "ready");
        assert_eq!(
            event.property("stream").map(|property| &property.value),
            Some(&PropertyValue::String("stdout".to_string()))
        );
        assert_eq!(
            event.property("service").map(|property| &property.value),
            Some(&PropertyValue::String("vev-mcp".to_string()))
        );
    }

    #[test]
    fn parses_logfmt_style_inline_properties() {
        let event = LogEvent::from_line(
            0,
            r#"api | INFO request method=GET path="/api/items" duration_ms=42"#.to_string(),
        );

        assert_eq!(
            event.property("method").map(|property| &property.value),
            Some(&PropertyValue::Text("GET".to_string()))
        );
        assert_eq!(
            event.property("path").map(|property| &property.value),
            Some(&PropertyValue::String("/api/items".to_string()))
        );
        assert_eq!(
            event
                .property("duration_ms")
                .map(|property| &property.value),
            Some(&PropertyValue::Number("42".to_string()))
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
        assert_eq!(event.message, "http.request GET /api/v1/inventory 200 96ms");
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
    fn parses_compose_status_lines_as_sources() {
        let parsed = parse_compose_line("vev-server-rest Pulling");

        assert_eq!(parsed.source, "vev-server-rest");
        assert_eq!(parsed.message, "Pulling");
        assert!(parsed.source_explicit);
    }

    #[test]
    fn parses_compose_error_status_lines_as_sources() {
        let parsed = parse_compose_line(
            "vev-server-rest Error response from daemon: pull access denied for image",
        );

        assert_eq!(parsed.source, "vev-server-rest");
        assert_eq!(
            parsed.message,
            "Error response from daemon: pull access denied for image"
        );
        assert!(parsed.source_explicit);
    }

    #[test]
    fn keeps_non_status_plain_lines_unknown() {
        let parsed = parse_compose_line("vev-server-rest connected to postgres");

        assert_eq!(parsed.source, "unknown");
        assert_eq!(parsed.message, "vev-server-rest connected to postgres");
        assert!(!parsed.source_explicit);
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
        assert_eq!(header.source, None);
        assert!(parse_property_block_header("[frontend] VITE ready").is_none());
    }

    #[test]
    fn parses_prefixed_property_block_header() {
        let header = parse_property_block_header("[backend] [14:06:58.892] INFO (#147):").unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Info);
        assert_eq!(header.source.as_deref(), Some("backend"));
    }

    #[test]
    fn parses_pipe_prefixed_property_block_header_with_trimmed_source_spelling() {
        let header =
            parse_property_block_header("  api worker  | [14:06:58.892] ERROR (#147):").unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Error);
        assert_eq!(header.source.as_deref(), Some("api worker"));
    }

    #[test]
    fn parses_colored_prefixed_property_block_header() {
        let header =
            parse_property_block_header("\u{1b}[36m[backend]\u{1b}[0m [14:06:58.892] INFO (#147):")
                .unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Info);
        assert_eq!(header.source.as_deref(), Some("backend"));
    }

    #[test]
    fn timestamp_bracket_is_not_misclassified_as_a_source() {
        let parsed = parse_compose_line("[14:06:58.892] INFO (#147):");

        assert_eq!(parsed.source, "unknown");
        assert_eq!(parsed.message, "[14:06:58.892] INFO (#147):");
        assert!(!parsed.source_explicit);
    }

    #[test]
    fn bracket_source_precedes_pipe_data_in_property_values() {
        let parsed = parse_compose_line("[api] value: \"left | right\"");

        assert_eq!(parsed.source, "api");
        assert_eq!(parsed.message, "value: \"left | right\"");
        assert_eq!(
            message_without_source_prefix("[api] value: \"left | right\""),
            "value: \"left | right\""
        );
    }

    #[test]
    fn colored_bracket_source_and_message_use_the_shared_splitter() {
        let parsed =
            parse_compose_line("\u{1b}[36m[api worker ]\u{1b}[0m [14:06:58.892] WARN (#9):");
        let header = parse_property_block_header(
            "\u{1b}[36m[api worker ]\u{1b}[0m [14:06:58.892] WARN (#9):",
        )
        .unwrap();

        assert_eq!(parsed.source, "api worker");
        assert_eq!(parsed.message, "[14:06:58.892] WARN (#9):");
        assert_eq!(header.source.as_deref(), Some("api worker"));
        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Warn);
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
        assert_eq!(
            properties[1].value,
            PropertyValue::Number("200".to_string())
        );
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
            infer_level(
                "[Nest] 32 - 05/08/2026, 4:18:15 PM LOG [NestFactory] Starting Nest application..."
            ),
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
