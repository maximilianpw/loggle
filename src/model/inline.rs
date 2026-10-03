use super::{
    LogProperty,
    block::{parse_property_entry, parse_property_value},
    json::{json_object_properties, parse_json_object},
};

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

#[cfg(test)]
mod tests {
    use crate::model::{LogEvent, PropertyValue};

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
}
