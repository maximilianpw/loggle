use super::{
    Level, LogProperty, PropertyBlockHeader, PropertyValue,
    compose::message_without_source_prefix,
    structured::{looks_like_timestamp, split_first_token},
};

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
pub(super) fn parse_property_entry(entry: &str) -> Option<LogProperty> {
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

pub(super) fn parse_property_value(value: &str) -> PropertyValue {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_property_block_header() {
        let header = parse_property_block_header("[14:06:58.892] INFO (#147):").unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Info);
        assert!(parse_property_block_header("[frontend] VITE ready").is_none());
    }

    #[test]
    fn parses_prefixed_property_block_header() {
        let header = parse_property_block_header("[backend] [14:06:58.892] INFO (#147):").unwrap();

        assert_eq!(header.timestamp, "14:06:58.892");
        assert_eq!(header.level, Level::Info);
    }

    #[test]
    fn parses_colored_prefixed_property_block_header() {
        let header =
            parse_property_block_header("\u{1b}[36m[backend]\u{1b}[0m [14:06:58.892] INFO (#147):")
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
}
