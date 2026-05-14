use std::borrow::Cow;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::model::{Level, LogEvent};

use super::{
    text::{compact_whitespace, truncate_tail},
    theme::THEME,
};

pub(super) fn render_event<'a>(
    event: &'a LogEvent,
    color_enabled: bool,
    selected: bool,
    message_field_keys: &'a [String],
    highlight_values: &[&str],
) -> Line<'a> {
    let row_bg = if selected {
        THEME.panel_alt
    } else {
        THEME.background
    };
    let row_style = Style::default().fg(THEME.text).bg(row_bg);
    let rail_style = Style::default().bg(if selected { THEME.accent } else { row_bg });
    let sequence = Span::styled(
        format!("{:>6} ", event.sequence),
        Style::default()
            .fg(THEME.line_number_fg)
            .bg(THEME.line_number_bg),
    );
    let source_style = if color_enabled {
        Style::default()
            .fg(source_color(&event.source))
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        row_style
    };
    let level_style = if color_enabled {
        Style::default().fg(level_color(event.level)).bg(row_bg)
    } else {
        row_style
    };

    let mut spans = vec![
        Span::styled(" ", rail_style),
        sequence,
        Span::styled(format!("{:<14}", truncate_tail(&event.source, 14)), source_style),
        Span::styled(" ", row_style),
        Span::styled(format!("{:<7}", event.level.as_str()), level_style),
        Span::styled(" ", row_style),
    ];
    spans.extend(message_spans(
        compact_whitespace(&event.message),
        row_style,
        highlight_values,
    ));

    for key in message_field_keys {
        if let Some(property) = event.property(key) {
            spans.push(Span::styled(" ", row_style));
            spans.push(Span::styled(
                format!("{}={}", property.key, property.value),
                row_style,
            ));
        }
    }

    Line::from(spans)
}

fn message_spans<'a>(
    message: Cow<'a, str>,
    row_style: Style,
    highlight_values: &[&str],
) -> Vec<Span<'a>> {
    if highlight_values.iter().all(|value| value.is_empty()) {
        return vec![Span::styled(message, row_style)];
    }

    let highlight_style = row_style
        .fg(THEME.background)
        .bg(THEME.highlight)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let message = message.as_ref();
    let mut remaining = message;

    while !remaining.is_empty() {
        let Some((start, value)) = earliest_match(remaining, highlight_values) else {
            spans.push(Span::styled(remaining.to_string(), row_style));
            break;
        };

        if start > 0 {
            spans.push(Span::styled(remaining[..start].to_string(), row_style));
        }

        let end = start + value.len();
        spans.push(Span::styled(remaining[start..end].to_string(), highlight_style));
        remaining = &remaining[end..];
    }

    spans
}

fn earliest_match<'a>(message: &str, highlight_values: &'a [&str]) -> Option<(usize, &'a str)> {
    highlight_values
        .iter()
        .copied()
        .filter(|value| !value.is_empty())
        .filter_map(|value| highlight_match(message, value).map(|index| (index, value)))
        .min_by_key(|(index, value)| (*index, std::cmp::Reverse(value.len())))
}

fn highlight_match(message: &str, value: &str) -> Option<usize> {
    message
        .match_indices(value)
        .find_map(|(index, _)| highlight_match_allowed(message, index, value).then_some(index))
}

fn highlight_match_allowed(message: &str, start: usize, value: &str) -> bool {
    if value.chars().any(char::is_alphanumeric) {
        return true;
    }

    let end = start + value.len();
    !adjacent_char(message, start, Direction::Before).is_some_and(char::is_alphanumeric)
        && !adjacent_char(message, end, Direction::After).is_some_and(char::is_alphanumeric)
}

#[derive(Clone, Copy)]
enum Direction {
    Before,
    After,
}

fn adjacent_char(message: &str, index: usize, direction: Direction) -> Option<char> {
    match direction {
        Direction::Before => message[..index].chars().next_back(),
        Direction::After => message[index..].chars().next(),
    }
}

fn level_color(level: Level) -> Color {
    match level {
        Level::Fatal | Level::Error => THEME.removed,
        Level::Warn => THEME.warning,
        Level::Info => THEME.info,
        Level::Debug => THEME.debug,
        Level::Trace => THEME.trace,
        Level::Unknown => THEME.unknown,
    }
}

fn source_color(source: &str) -> Color {
    const COLORS: [Color; 8] = [
        Color::Rgb(127, 209, 255),
        Color::Rgb(136, 211, 155),
        Color::Rgb(230, 207, 152),
        Color::Rgb(196, 155, 255),
        Color::Rgb(186, 200, 212),
        Color::Rgb(216, 198, 239),
        Color::Rgb(223, 230, 237),
        Color::Rgb(169, 180, 191),
    ];

    let hash = source.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as usize)
    });
    COLORS[hash % COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LogProperty, PropertyValue};

    #[test]
    fn display_message_compacts_nestjs_alignment_spacing() {
        let message =
            "[Nest]      32  - 05/08/2026,      4:18:15 PM          LOG      [InstanceLoader]";

        assert_eq!(
            compact_whitespace(message),
            "[Nest] 32 - 05/08/2026, 4:18:15 PM LOG [InstanceLoader]"
        );
    }

    #[test]
    fn default_row_rendering_is_unchanged_without_message_fields() {
        let mut event = LogEvent::from_line(1, "api | INFO request completed".to_string());
        event.set_properties(vec![LogProperty {
            key: "tenantId".to_string(),
            value: PropertyValue::String("tenant-1".to_string()),
        }]);

        let row = render_event(&event, false, false, &[], &[]);
        let text = row.spans.into_iter().map(|span| span.content).collect::<String>();

        assert!(text.ends_with("request completed"));
        assert!(!text.contains("tenantId=tenant-1"));
    }

    #[test]
    fn selected_message_fields_append_key_value_segments() {
        let mut event = LogEvent::from_line(1, "api | INFO request completed".to_string());
        event.set_properties(vec![
            LogProperty {
                key: "tenantId".to_string(),
                value: PropertyValue::String("tenant-1".to_string()),
            },
            LogProperty {
                key: "durationMs".to_string(),
                value: PropertyValue::Number("96".to_string()),
            },
        ]);

        let message_fields = ["tenantId".to_string(), "missing".to_string()];
        let row = render_event(&event, false, false, &message_fields, &[]);
        let text = row.spans.into_iter().map(|span| span.content).collect::<String>();

        assert!(text.ends_with("request completed tenantId=tenant-1"));
        assert!(!text.contains("missing="));
    }

    #[test]
    fn property_values_are_highlighted_inside_message_text() {
        let event = LogEvent::from_line(
            1,
            "api | INFO request tenant-1 completed for abc".to_string(),
        );

        let row = render_event(&event, false, false, &[], &["tenant-1"]);
        let highlighted = row
            .spans
            .into_iter()
            .filter(|span| span.style.bg == Some(THEME.highlight))
            .map(|span| span.content)
            .collect::<String>();

        assert_eq!(highlighted, "tenant-1");
    }

    #[test]
    fn multiple_property_values_are_highlighted_inside_message_text() {
        let event = LogEvent::from_line(
            1,
            "api | INFO request tenant-1 completed for abc".to_string(),
        );

        let row = render_event(&event, false, false, &[], &["tenant-1", "abc"]);
        let highlighted = row
            .spans
            .into_iter()
            .filter(|span| span.style.bg == Some(THEME.highlight))
            .map(|span| span.content)
            .collect::<Vec<_>>();

        assert_eq!(highlighted, vec!["tenant-1", "abc"]);
    }

    #[test]
    fn punctuation_highlight_values_do_not_match_inside_alphanumeric_tokens() {
        let event = LogEvent::from_line(1, "api | INFO range 0-0 done".to_string());

        let row = render_event(&event, false, false, &[], &["-"]);

        assert!(row
            .spans
            .into_iter()
            .all(|span| span.style.bg != Some(THEME.highlight)));
    }

    #[test]
    fn punctuation_highlight_values_still_match_standalone_tokens() {
        let event = LogEvent::from_line(1, "api | INFO empty - done".to_string());

        let row = render_event(&event, false, false, &[], &["-"]);
        let highlighted = row
            .spans
            .into_iter()
            .filter(|span| span.style.bg == Some(THEME.highlight))
            .map(|span| span.content)
            .collect::<String>();

        assert_eq!(highlighted, "-");
    }

    #[test]
    fn empty_highlight_values_do_not_change_message_styling() {
        let event = LogEvent::from_line(1, "api | INFO request completed".to_string());

        let row = render_event(&event, false, false, &[], &[]);

        assert!(row
            .spans
            .into_iter()
            .all(|span| span.style.bg != Some(THEME.highlight)));
    }
}
