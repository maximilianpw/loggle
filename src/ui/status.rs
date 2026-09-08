use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    app::{App, Mode},
    commands::{CommandHelpItem, CommandHelpLevel, status_help_items},
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
    Essential,
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
    let level = filters.level.map(|level| level.as_str()).unwrap_or("-");
    let properties = property_filters_summary(filters);
    let help = help_variant(width);
    let (source_limit, level_limit, text_limit, property_limit) = value_limits(width, help);

    let (source_label, level_label, text_label, properties_label) = match help {
        HelpVariant::Essential => ("s=", "  l=", "  /=", "  p="),
        _ => ("source=", "  level=", "  search=", "  props="),
    };
    let mut segments = vec![
        base(" filters "),
        base(source_label),
        value(truncate_tail(source, source_limit)),
        base(level_label),
        value(truncate_tail(level, level_limit)),
        base(text_label),
        value(truncate_tail(text, text_limit)),
        base(properties_label),
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
    if help == HelpVariant::Essential {
        // 23 columns of compact filter labels, 3 before help, 18 of help.
        let available = (width as usize).saturating_sub(44);
        let slot = available / 4;
        let level = slot.min(7);
        return (slot, level, slot, available - slot * 2 - level);
    }
    let help_len = match help {
        HelpVariant::Full => 80,
        HelpVariant::Compact => 38,
        HelpVariant::Essential => 18,
        HelpVariant::None => 0,
    };
    let separator_len = usize::from(help != HelpVariant::None) * 3;
    let fixed_len = 41 + separator_len + help_len;
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
    } else if width >= 48 {
        HelpVariant::Essential
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
        HelpVariant::Essential => append_help_items(
            segments,
            [
                CommandHelpItem {
                    shortcut: "q",
                    label: "quit",
                },
                CommandHelpItem {
                    shortcut: "?",
                    label: "commands",
                },
            ],
        ),
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
    use ratatui::{Terminal, backend::TestBackend};

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
    fn status_keeps_essential_help_at_narrow_widths() {
        for width in [72, 60] {
            let status = plain_status(&LogFilter::default(), width);

            assert!(status.contains("q quit"), "width {width}: {status}");
            assert!(status.contains("? commands"), "width {width}: {status}");
            assert!(status.len() <= width as usize, "width {width}: {status}");
            assert!(status.contains("filters s=-"));
        }
    }

    #[test]
    fn narrow_status_shortens_long_filters_before_essential_help() {
        let filters = LogFilter {
            text: Some("database connection failure in shard six".to_string()),
            source: Some("very-long-service-name".to_string()),
            level: Some(Level::Error),
            property_includes: vec![PropertyPredicate::exact("tenantId", "tenant-1")],
            property_excludes: Vec::new(),
        };

        for width in 48..80 {
            let status = plain_status(&filters, width);
            assert!(status.contains("q quit"), "width {width}: {status}");
            assert!(status.contains("? commands"), "width {width}: {status}");
            assert!(status.len() <= width as usize, "width {width}: {status}");
            assert!(!status.contains("very-long-service-name"));
            assert!(!status.contains("database connection failure"));
        }
    }

    #[test]
    fn very_narrow_render_clips_safely_after_essential_help_is_dropped() {
        let width = 24;
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|frame| {
                let spans = status_segments(&LogFilter::default(), width)
                    .into_iter()
                    .map(|segment| Span::raw(segment.text))
                    .collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(Line::from(spans)), frame.area());
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(rendered.chars().count(), width as usize);
        assert!(!rendered.contains("q quit"));
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
