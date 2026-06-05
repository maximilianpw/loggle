use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    app::{App, Mode},
    commands::{status_help_items, CommandHelpItem, CommandHelpLevel},
    filter::LogFilter,
};

use super::{text::truncate_tail, theme::THEME};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusRole {
    Base,
    Value,
    Key,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusSegment {
    text: String,
    role: StatusRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpVariant {
    Full,
    Compact,
    None,
}

pub(super) fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let base = Style::default().fg(THEME.muted).bg(THEME.panel_alt);
    let value = Style::default().fg(THEME.text).bg(THEME.panel_alt);
    let key = Style::default().fg(THEME.accent).bg(THEME.panel_alt);
    let spans = app_status_segments(app, area.width)
        .into_iter()
        .map(|segment| {
            let style = match segment.role {
                StatusRole::Base => base,
                StatusRole::Value => value,
                StatusRole::Key => key,
            };
            Span::styled(segment.text, style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}

fn app_status_segments(app: &App, width: u16) -> Vec<StatusSegment> {
    if let Some(notice) = app.notice() {
        return notice_segments(notice, width);
    }

    if app.mode() == &Mode::Visual {
        return visual_segments(app);
    }

    status_segments(app.filters(), width)
}

fn notice_segments(notice: &str, width: u16) -> Vec<StatusSegment> {
    vec![base(" "), value(truncate_tail(notice, width as usize))]
}

fn visual_segments(app: &App) -> Vec<StatusSegment> {
    let count = app.visual_selected_count();
    vec![
        base(" visual "),
        value(line_count_label(count)),
        base("  "),
        key("y"),
        base(" copy  "),
        key("Esc"),
        base(" cancel"),
    ]
}

fn status_segments(filters: &LogFilter, width: u16) -> Vec<StatusSegment> {
    let text = filters.text.as_deref().unwrap_or("-");
    let source = filters.source.as_deref().unwrap_or("-");
    let level = filters
        .level
        .map(|level| level.as_str())
        .unwrap_or("-");
    let properties = property_filters_summary(filters);
    let help = help_variant(width);
    let (source_limit, level_limit, text_limit, property_limit) = value_limits(width, help);

    let mut segments = vec![
        base(" filters "),
        base("source="),
        value(truncate_tail(source, source_limit)),
        base("  level="),
        value(truncate_tail(level, level_limit)),
        base("  search="),
        value(truncate_tail(text, text_limit)),
        base("  props="),
        value(truncate_tail(&properties, property_limit)),
    ];

    append_help(&mut segments, help);
    segments
}

fn line_count_label(count: usize) -> String {
    if count == 1 {
        "1 line".to_string()
    } else {
        format!("{count} lines")
    }
}

fn value_limits(width: u16, help: HelpVariant) -> (usize, usize, usize, usize) {
    let help_len = match help {
        HelpVariant::Full => 80,
        HelpVariant::Compact => 38,
        HelpVariant::None => 0,
    };
    let separator_len = usize::from(help != HelpVariant::None) * 3;
    let fixed_len = 9 + 7 + 8 + 9 + 8 + separator_len + help_len;
    let available = (width as usize).saturating_sub(fixed_len);

    if available >= 55 {
        (16, 7, 24, 32)
    } else if available >= 35 {
        (10, 7, 12, 10)
    } else if available >= 21 {
        (7, 5, 7, 6)
    } else if available >= 12 {
        (4, 3, 4, 3)
    } else {
        (1, 1, 1, 1)
    }
}

fn help_variant(width: u16) -> HelpVariant {
    if width >= 120 {
        HelpVariant::Full
    } else if width >= 80 {
        HelpVariant::Compact
    } else {
        HelpVariant::None
    }
}

fn append_help(segments: &mut Vec<StatusSegment>, help: HelpVariant) {
    match help {
        HelpVariant::Full => append_help_items(segments, status_help_items(CommandHelpLevel::Full)),
        HelpVariant::Compact => {
            append_help_items(segments, status_help_items(CommandHelpLevel::Compact));
        }
        HelpVariant::None => {}
    }
}

fn append_help_items(
    segments: &mut Vec<StatusSegment>,
    items: impl IntoIterator<Item = CommandHelpItem>,
) {
    let mut items = items.into_iter().peekable();
    if items.peek().is_none() {
        return;
    }

    segments.push(base("   "));
    for (index, item) in items.enumerate() {
        if index > 0 {
            segments.push(base("  "));
        }
        segments.push(key(item.shortcut));
        segments.push(base(format!(" {}", item.label)));
    }
}

fn base(text: impl Into<String>) -> StatusSegment {
    StatusSegment {
        text: text.into(),
        role: StatusRole::Base,
    }
}

fn value(text: impl Into<String>) -> StatusSegment {
    StatusSegment {
        text: text.into(),
        role: StatusRole::Value,
    }
}

fn key(text: impl Into<String>) -> StatusSegment {
    StatusSegment {
        text: text.into(),
        role: StatusRole::Key,
    }
}

fn property_filters_summary(filters: &LogFilter) -> String {
    if filters.property_includes.is_empty() && filters.property_excludes.is_empty() {
        return "-".to_string();
    }

    filters
        .property_includes
        .iter()
        .map(|predicate| predicate.summary_for(false))
        .chain(
            filters
                .property_excludes
                .iter()
                .map(|predicate| predicate.summary_for(true)),
        )
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{filter::PropertyPredicate, model::Level};

    fn plain_status(filters: &LogFilter, width: u16) -> String {
        status_segments(filters, width)
            .into_iter()
            .map(|segment| segment.text)
            .collect::<String>()
    }

    #[test]
    fn status_clips_long_filter_values_for_compact_widths() {
        let filters = LogFilter {
            text: Some("database connection failure in shard six".to_string()),
            source: Some("very-long-service-name".to_string()),
            level: Some(Level::Error),
            property_includes: vec![PropertyPredicate::exact("tenantId", "tenant-1")],
            property_excludes: Vec::new(),
        };

        let status = plain_status(&filters, 110);

        assert!(status.contains("very-l~"));
        assert!(status.contains("databa~"));
        assert!(!status.contains("very-long-service-name"));
        assert!(!status.contains("database connection failure"));
        assert!(status.contains("q quit"));
        assert!(status.contains("c clear"));
        assert!(!status.contains("l level"));
    }

    #[test]
    fn status_uses_full_help_for_wide_widths() {
        let status = plain_status(&LogFilter::default(), 140);

        assert!(status.contains("s source"));
        assert!(status.contains("l level"));
    }

    #[test]
    fn status_omits_help_for_narrow_widths() {
        let status = plain_status(&LogFilter::default(), 60);

        assert!(!status.contains("q quit"));
        assert!(status.contains("filters source=-"));
    }

    #[test]
    fn status_summarizes_property_filters() {
        let filters = LogFilter {
            property_includes: vec![PropertyPredicate::exact("tenantId", "tenant-1")],
            property_excludes: vec![
                PropertyPredicate::exists("debug"),
                PropertyPredicate::exact("statusCode", "500"),
            ],
            ..LogFilter::default()
        };
        let status = plain_status(&filters, 220);

        assert_eq!(
            property_filters_summary(&filters),
            "tenantId=tenant-1,!debug,statusCode!=500"
        );
        assert!(status.contains("props=tenantId=tenant-1,!debug"));
    }
}
