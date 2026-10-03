use super::{Level, StructuredMessage, clean_display_text};

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

/// How strongly a source-stripped message looks like the start of a new
/// structured record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StructuredLineKind {
    None,
    /// A level-first summary such as `ERROR failed`. Property entries named
    /// after a level (`error: "x"`) also classify here.
    LevelOnly,
    /// A timestamp + level summary, bare (`10:00:00.000 INFO`) or bracketed
    /// (`[10:00:00.000] INFO`).
    Timestamped,
}

pub(super) fn structured_line_kind(message: &str) -> StructuredLineKind {
    match parse_structured_message(message) {
        Some(message) if message.timestamp.is_some() => StructuredLineKind::Timestamped,
        Some(_) => StructuredLineKind::LevelOnly,
        None if is_bracketed_timestamp_structured(message) => StructuredLineKind::Timestamped,
        None => StructuredLineKind::None,
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

pub(super) fn split_first_token(value: &str) -> Option<(&str, &str)> {
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

pub(super) fn looks_like_timestamp(value: &str) -> bool {
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
    use crate::model::LogEvent;

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
    fn parses_level_first_structured_summary() {
        let event = LogEvent::from_line(0, "ERROR sync.failed retry exhausted".to_string());

        assert_eq!(event.timestamp, None);
        assert_eq!(event.level, Level::Error);
        assert_eq!(event.message, "sync.failed retry exhausted");
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

    #[test]
    fn classifies_structured_record_starts() {
        assert_eq!(
            structured_line_kind("10:00:00.000 INFO ready"),
            StructuredLineKind::Timestamped
        );
        assert_eq!(
            structured_line_kind("[10:00:01.000] ERROR recovered"),
            StructuredLineKind::Timestamped
        );
        assert_eq!(
            structured_line_kind("ERROR recovered"),
            StructuredLineKind::LevelOnly
        );
        assert_eq!(
            structured_line_kind("[10:00:01.000] recovered"),
            StructuredLineKind::None
        );
        assert_eq!(
            structured_line_kind("requestId: \"abc\","),
            StructuredLineKind::None
        );
    }
}
