use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    app::DisplayFieldColumn,
    model::{Level, LogEvent},
};

use super::{
    text::{compact_whitespace, truncate_tail},
    theme::THEME,
};

const MAX_TRUNCATION_MARKER: char = '~';
const BASE_PREFIX_WIDTH: usize = 31;

pub(super) fn render_header(display_columns: &[DisplayFieldColumn], width: usize) -> Line<'static> {
    let header_style = Style::default().fg(THEME.muted).bg(THEME.panel_alt);
    let message_width = message_width(width, display_columns);
    let mut spans = vec![
        Span::styled(" ".repeat(8), header_style),
        Span::styled(format!("{:<14}", "service"), header_style),
        Span::styled(" ", header_style),
        Span::styled(format!("{:<7}", "level"), header_style),
        Span::styled(" ", header_style),
    ];
    spans.extend(padded_message_spans(
        "message",
        header_style,
        &[],
        message_width,
        !display_columns.is_empty(),
    ));
    spans.extend(field_header_spans(display_columns, header_style));
    Line::from(spans)
}

pub(super) fn render_event(
    event: &LogEvent,
    color_enabled: bool,
    selected: bool,
    marked: bool,
    display_columns: &[DisplayFieldColumn],
    highlight_values: &[&str],
    width: usize,
    max_lines: usize,
) -> Vec<Line<'static>> {
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

    let prefix_spans = vec![
        Span::styled(if marked { "*" } else { " " }, rail_style),
        sequence,
        Span::styled(
            format!("{:<14}", truncate_tail(&event.source, 14)),
            source_style,
        ),
        Span::styled(" ", row_style),
        Span::styled(format!("{:<7}", event.level.as_str()), level_style),
        Span::styled(" ", row_style),
    ];

    let message_width = message_width(width, display_columns);
    let message = compact_whitespace(&event.message).into_owned();
    let message_lines = wrap_message(&message, message_width, max_lines);
    let mut lines = Vec::with_capacity(message_lines.len());

    for (index, message_line) in message_lines.iter().enumerate() {
        let mut spans = if index == 0 {
            prefix_spans.clone()
        } else {
            continuation_prefix(row_style)
        };
        spans.extend(padded_message_spans(
            message_line,
            row_style,
            highlight_values,
            message_width,
            !display_columns.is_empty(),
        ));
        if index == 0 {
            spans.extend(field_value_spans(event, display_columns, row_style));
        } else {
            spans.extend(field_blank_spans(display_columns, row_style));
        }
        lines.push(Line::from(spans));
    }

    lines
}

pub(super) fn event_height(
    event: &LogEvent,
    width: usize,
    display_columns: &[DisplayFieldColumn],
    max_lines: usize,
) -> usize {
    let message_width = message_width(width, display_columns);
    wrap_message(
        &compact_whitespace(&event.message),
        message_width,
        max_lines,
    )
    .len()
}

fn message_width(width: usize, display_columns: &[DisplayFieldColumn]) -> usize {
    width
        .saturating_sub(BASE_PREFIX_WIDTH + field_suffix_width(display_columns))
        .max(1)
}

fn field_suffix_width(display_columns: &[DisplayFieldColumn]) -> usize {
    display_columns.iter().map(|column| 1 + column.width).sum()
}

fn continuation_prefix(row_style: Style) -> Vec<Span<'static>> {
    vec![Span::styled(" ".repeat(BASE_PREFIX_WIDTH), row_style)]
}

fn field_header_spans(display_columns: &[DisplayFieldColumn], style: Style) -> Vec<Span<'static>> {
    display_columns
        .iter()
        .map(|column| {
            Span::styled(
                format!(" {}", padded_cell(&column.key, column.width)),
                style,
            )
        })
        .collect()
}

fn field_value_spans(
    event: &LogEvent,
    display_columns: &[DisplayFieldColumn],
    style: Style,
) -> Vec<Span<'static>> {
    display_columns
        .iter()
        .map(|column| {
            let value = event
                .property(&column.key)
                .map(|property| property.value.as_display_str())
                .unwrap_or_else(|| "-".into());
            Span::styled(
                format!(" {}", padded_cell(value.as_ref(), column.width)),
                style,
            )
        })
        .collect()
}

fn field_blank_spans(display_columns: &[DisplayFieldColumn], style: Style) -> Vec<Span<'static>> {
    display_columns
        .iter()
        .map(|column| Span::styled(format!(" {}", " ".repeat(column.width)), style))
        .collect()
}

fn padded_cell(value: &str, width: usize) -> String {
    let truncated = truncate_tail(value, width);
    let padding = width.saturating_sub(truncated.chars().count());
    format!("{}{}", truncated, " ".repeat(padding))
}

fn padded_message_spans(
    message: &str,
    row_style: Style,
    highlight_values: &[&str],
    width: usize,
    pad: bool,
) -> Vec<Span<'static>> {
    let mut spans = message_spans(message, row_style, highlight_values);
    if pad {
        let padding = width.saturating_sub(message.chars().count());
        if padding > 0 {
            spans.push(Span::styled(" ".repeat(padding), row_style));
        }
    }
    spans
}

fn message_spans(message: &str, row_style: Style, highlight_values: &[&str]) -> Vec<Span<'static>> {
    if highlight_values.iter().all(|value| value.is_empty()) {
        return vec![Span::styled(message.to_string(), row_style)];
    }

    let highlight_style = row_style
        .fg(THEME.background)
        .bg(THEME.highlight)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
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
        spans.push(Span::styled(
            remaining[start..end].to_string(),
            highlight_style,
        ));
        remaining = &remaining[end..];
    }

    spans
}

fn wrap_message(message: &str, width: usize, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }

    if width == 0 {
        return vec![String::new()];
    }

    let chars = message.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut start = 0;

    while start < chars.len() && lines.len() < max_lines {
        while start < chars.len() && chars[start].is_whitespace() {
            start += 1;
        }

        if start >= chars.len() {
            break;
        }

        let max_end = start.saturating_add(width).min(chars.len());
        let remaining_lines = max_lines - lines.len();

        if max_end == chars.len() {
            lines.push(chars[start..max_end].iter().collect());
            break;
        }

        let break_at = (start..max_end)
            .filter(|index| chars[*index].is_whitespace())
            .last()
            .filter(|index| *index > start);

        let (end, next_start) = if let Some(index) = break_at {
            (index, index + 1)
        } else {
            (max_end, max_end)
        };

        let mut line = chars[start..end].iter().collect::<String>();
        let has_more = chars[next_start..].iter().any(|ch| !ch.is_whitespace());
        if remaining_lines == 1 && has_more {
            mark_truncated(&mut line, width);
        }

        lines.push(line);
        start = next_start;
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn mark_truncated(line: &mut String, width: usize) {
    if width == 0 {
        line.clear();
        return;
    }

    if width == 1 {
        line.clear();
        line.push(MAX_TRUNCATION_MARKER);
        return;
    }

    let mut truncated = line.chars().take(width - 1).collect::<String>();
    truncated.push(MAX_TRUNCATION_MARKER);
    *line = truncated;
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
    use crate::{
        app::DisplayFieldColumn,
        model::{LogProperty, PropertyValue},
    };

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
    fn default_row_rendering_is_unchanged_without_display_fields() {
        let mut event = LogEvent::from_line(1, "api | INFO request completed".to_string());
        event.set_properties(vec![LogProperty {
            key: "tenantId".to_string(),
            value: PropertyValue::String("tenant-1".to_string()),
        }]);

        let row = render_event(&event, false, false, false, &[], &[], 120, 3);
        let text = row_text(row);

        assert!(text.ends_with("request completed"));
        assert!(!text.contains("tenant-1"));
    }

    #[test]
    fn selected_display_fields_render_value_columns_after_message() {
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

        let columns = [display_column("tenantId", 10), display_column("missing", 7)];
        let row = render_event(&event, false, false, false, &columns, &[], 120, 3);
        let text = row_text(row);

        assert!(text.contains("request completed"));
        assert!(text.contains(" tenant-1   -      "));
        assert!(!text.contains("tenantId=tenant-1"));
    }

    #[test]
    fn display_field_header_labels_columns() {
        let columns = [
            display_column("tenantId", 8),
            display_column("requestId", 9),
        ];

        let header = render_header(&columns, 80);
        let text = header
            .spans
            .into_iter()
            .map(|span| span.content)
            .collect::<String>();

        assert!(text.contains("service"));
        assert!(text.contains("level"));
        assert!(text.contains("message"));
        assert!(text.contains("tenantId"));
        assert!(text.contains("requestId"));
    }

    #[test]
    fn long_display_field_values_are_truncated_in_fixed_columns() {
        let mut event = LogEvent::from_line(1, "api | INFO request completed".to_string());
        event.set_properties(vec![LogProperty {
            key: "requestId".to_string(),
            value: PropertyValue::String("abcdefghijklmnopqrstuvwxyz".to_string()),
        }]);
        let columns = [display_column("requestId", 8)];

        let row = render_event(&event, false, false, false, &columns, &[], 80, 3);
        let text = row_text(row);

        assert!(text.contains("abcdefg~"));
    }

    #[test]
    fn property_values_are_highlighted_inside_message_text() {
        let event = LogEvent::from_line(
            1,
            "api | INFO request tenant-1 completed for abc".to_string(),
        );

        let row = render_event(&event, false, false, false, &[], &["tenant-1"], 120, 3);
        let highlighted = row
            .into_iter()
            .flat_map(|line| line.spans)
            .filter(|span| span.style.bg == Some(THEME.highlight))
            .map(|span| span.content)
            .collect::<String>();

        assert_eq!(highlighted, "tenant-1");
    }

    #[test]
    fn search_values_are_highlighted_inside_message_text() {
        let event = LogEvent::from_line(1, "api | INFO request completed".to_string());

        let row = render_event(&event, false, false, false, &[], &["request"], 120, 3);
        let highlighted = row
            .into_iter()
            .flat_map(|line| line.spans)
            .filter(|span| span.style.bg == Some(THEME.highlight))
            .map(|span| span.content)
            .collect::<String>();

        assert_eq!(highlighted, "request");
    }

    #[test]
    fn multiple_property_values_are_highlighted_inside_message_text() {
        let event = LogEvent::from_line(
            1,
            "api | INFO request tenant-1 completed for abc".to_string(),
        );

        let row = render_event(
            &event,
            false,
            false,
            false,
            &[],
            &["tenant-1", "abc"],
            120,
            3,
        );
        let highlighted = row
            .into_iter()
            .flat_map(|line| line.spans)
            .filter(|span| span.style.bg == Some(THEME.highlight))
            .map(|span| span.content)
            .collect::<Vec<_>>();

        assert_eq!(highlighted, vec!["tenant-1", "abc"]);
    }

    #[test]
    fn punctuation_highlight_values_do_not_match_inside_alphanumeric_tokens() {
        let event = LogEvent::from_line(1, "api | INFO range 0-0 done".to_string());

        let row = render_event(&event, false, false, false, &[], &["-"], 120, 3);

        assert!(
            row.into_iter()
                .flat_map(|line| line.spans)
                .all(|span| span.style.bg != Some(THEME.highlight))
        );
    }

    #[test]
    fn punctuation_highlight_values_still_match_standalone_tokens() {
        let event = LogEvent::from_line(1, "api | INFO empty - done".to_string());

        let row = render_event(&event, false, false, false, &[], &["-"], 120, 3);
        let highlighted = row
            .into_iter()
            .flat_map(|line| line.spans)
            .filter(|span| span.style.bg == Some(THEME.highlight))
            .map(|span| span.content)
            .collect::<String>();

        assert_eq!(highlighted, "-");
    }

    #[test]
    fn empty_highlight_values_do_not_change_message_styling() {
        let event = LogEvent::from_line(1, "api | INFO request completed".to_string());

        let row = render_event(&event, false, false, false, &[], &[], 120, 3);

        assert!(
            row.into_iter()
                .flat_map(|line| line.spans)
                .all(|span| span.style.bg != Some(THEME.highlight))
        );
    }

    #[test]
    fn marked_rows_render_marker_in_rail() {
        let event = LogEvent::from_line(1, "api | INFO request completed".to_string());

        let row = render_event(&event, false, false, true, &[], &[], 120, 3);

        assert_eq!(row[0].spans[0].content, "*");
    }

    #[test]
    fn long_messages_wrap_to_at_most_three_lines() {
        let event = LogEvent::from_line(
            1,
            "api | INFO alpha beta gamma delta epsilon zeta eta theta".to_string(),
        );

        let row = render_event(&event, false, false, false, &[], &[], 43, 3);
        let text = row
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(row.len(), 3);
        assert!(text[2].ends_with('~'));
    }

    #[test]
    fn wrapped_continuation_lines_align_with_message_column() {
        let event = LogEvent::from_line(
            1,
            "api | INFO alpha beta gamma delta epsilon zeta".to_string(),
        );

        let row = render_event(&event, false, false, false, &[], &[], 43, 3);

        assert_eq!(row[1].spans[0].content.chars().count(), BASE_PREFIX_WIDTH);
        assert!(row[1].spans[0].content.chars().all(|ch| ch == ' '));
    }

    #[test]
    fn wrapped_messages_do_not_repeat_display_field_values() {
        let mut event = LogEvent::from_line(
            1,
            "api | INFO alpha beta gamma delta epsilon zeta eta theta".to_string(),
        );
        event.set_properties(vec![LogProperty {
            key: "tenantId".to_string(),
            value: PropertyValue::String("tenant-1".to_string()),
        }]);
        let columns = [display_column("tenantId", 8)];

        let row = render_event(&event, false, false, false, &columns, &[], 52, 3);
        let text = row
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            text.iter().filter(|line| line.contains("tenant-1")).count(),
            1
        );
        assert!(text[1].ends_with("         "));
    }

    #[test]
    fn wrapped_rows_report_their_visual_height() {
        let event = LogEvent::from_line(
            1,
            "api | INFO alpha beta gamma delta epsilon zeta eta theta".to_string(),
        );

        assert_eq!(event_height(&event, 43, &[], 3), 3);
    }

    fn display_column(key: &str, width: usize) -> DisplayFieldColumn {
        DisplayFieldColumn {
            key: key.to_string(),
            width,
        }
    }

    fn row_text(row: Vec<Line<'static>>) -> String {
        row.into_iter()
            .flat_map(|line| line.spans)
            .map(|span| span.content)
            .collect::<String>()
    }
}
