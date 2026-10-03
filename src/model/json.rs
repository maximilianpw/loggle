use serde_json::{Map, Value};

use super::{Level, LogProperty, PropertyValue, structured::infer_level};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StructuredJsonLog {
    pub(super) timestamp: Option<String>,
    pub(super) level: Level,
    pub(super) message: String,
    pub(super) properties: Vec<LogProperty>,
}

const JSON_MESSAGE_KEYS: &[&str] = &["message", "msg", "log", "Log"];
const JSON_LEVEL_KEYS: &[&str] = &["level", "severity"];
const JSON_TIMESTAMP_KEYS: &[&str] = &["timestamp", "time", "ts", "@timestamp"];
pub(super) fn split_trailing_json_properties(message: &str) -> (String, Vec<LogProperty>) {
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

pub(super) fn parse_json_log_message(message: &str) -> Option<StructuredJsonLog> {
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
pub(super) fn parse_json_object(input: &str) -> Option<Map<String, Value>> {
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
        if inner_log.level == Level::Unknown
            && let Some(level) = first_json_string_field(object, JSON_LEVEL_KEYS)
                .and_then(|level| Level::parse(&level))
        {
            inner_log.level = level;
        }

        let mut properties = json_object_properties(object, JSON_ROW_KEYS);
        properties.extend(inner_log.properties);
        inner_log.properties = properties;
        return Some(inner_log);
    }

    None
}

fn clean_json_message(message: &str) -> String {
    message.trim_end_matches(['\r', '\n']).to_string()
}

fn first_json_string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| json_string_field(object, key))
}

fn json_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(ToString::to_string)
}

pub(super) fn json_object_properties(
    object: &Map<String, Value>,
    excluded_keys: &[&str],
) -> Vec<LogProperty> {
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

#[cfg(test)]
mod tests {
    use crate::model::{Level, LogEvent, PropertyValue};

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
}
