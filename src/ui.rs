use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, Paragraph},
};

use crate::{
    app::{App, Mode, PromptKind},
    model::{Level, LogEvent},
};

#[derive(Debug, Clone, Copy)]
struct GraphiteTheme {
    background: Color,
    panel_alt: Color,
    accent: Color,
    text: Color,
    muted: Color,
    removed: Color,
    warning: Color,
    info: Color,
    debug: Color,
    trace: Color,
    unknown: Color,
    line_number_bg: Color,
    line_number_fg: Color,
}

const THEME: GraphiteTheme = GraphiteTheme {
    background: Color::Rgb(17, 19, 21),
    panel_alt: Color::Rgb(29, 33, 38),
    accent: Color::Rgb(213, 224, 234),
    text: Color::Rgb(242, 244, 246),
    muted: Color::Rgb(154, 164, 175),
    removed: Color::Rgb(240, 160, 160),
    warning: Color::Rgb(230, 207, 152),
    info: Color::Rgb(136, 211, 155),
    debug: Color::Rgb(127, 209, 255),
    trace: Color::Rgb(196, 155, 255),
    unknown: Color::Rgb(121, 133, 146),
    line_number_bg: Color::Rgb(20, 24, 27),
    line_number_fg: Color::Rgb(121, 133, 146),
};

pub fn draw(frame: &mut Frame<'_>, app: &App, color_enabled: bool) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.background)),
        frame.area(),
    );

    draw_header(frame, chunks[0], app);
    draw_logs(frame, chunks[1], app, color_enabled);
    draw_footer(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let follow = if app.is_following() { "follow" } else { "paused" };
    let visible = app.visible_events().len();
    let style = Style::default().fg(THEME.muted).bg(THEME.panel_alt);
    let accent_style = Style::default()
        .fg(THEME.accent)
        .bg(THEME.panel_alt)
        .add_modifier(Modifier::BOLD);

    let header = Line::from(vec![
        Span::styled(" loggle ", accent_style),
        Span::styled(follow, Style::default().fg(THEME.text).bg(THEME.panel_alt)),
        Span::styled("  retained ", style),
        Span::styled(
            app.retained_len().to_string(),
            Style::default().fg(THEME.text).bg(THEME.panel_alt),
        ),
        Span::styled("  visible ", style),
        Span::styled(
            visible.to_string(),
            Style::default().fg(THEME.text).bg(THEME.panel_alt),
        ),
    ]);

    frame.render_widget(Paragraph::new(header).style(style), area);
}

fn draw_logs(frame: &mut Frame<'_>, area: Rect, app: &App, color_enabled: bool) {
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.background)),
        area,
    );

    let visible_events = app.visible_events();
    let viewport_height = area.height as usize;
    let selected = app.selected();
    let start = selected.saturating_sub(viewport_height.saturating_sub(1));
    let end = (start + viewport_height).min(visible_events.len());

    let items = visible_events[start..end]
        .iter()
        .enumerate()
        .map(|(offset, event)| {
            let visible_index = start + offset;
            ListItem::new(render_event(event, color_enabled, visible_index == selected))
        })
        .collect::<Vec<_>>();

    let list = List::new(items);
    frame.render_widget(list, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    match app.mode() {
        Mode::Prompt(_) => draw_prompt(frame, area, app),
        Mode::Normal => draw_status(frame, area, app),
    }
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = app.filters().text.as_deref().unwrap_or("-");
    let source = app.filters().source.as_deref().unwrap_or("-");
    let level = app
        .filters()
        .level
        .map(|level| level.to_string())
        .unwrap_or_else(|| "-".to_string());
    let base = Style::default().fg(THEME.muted).bg(THEME.panel_alt);
    let value = Style::default().fg(THEME.text).bg(THEME.panel_alt);
    let key = Style::default().fg(THEME.accent).bg(THEME.panel_alt);

    let status = Line::from(vec![
        Span::styled(" filters ", base),
        Span::styled("source=", base),
        Span::styled(source.to_string(), value),
        Span::styled("  level=", base),
        Span::styled(level, value),
        Span::styled("  search=", base),
        Span::styled(text.to_string(), value),
        Span::styled("   ", base),
        Span::styled("q", key),
        Span::styled(" quit  ", base),
        Span::styled("/", key),
        Span::styled(" search  ", base),
        Span::styled("s", key),
        Span::styled(" source  ", base),
        Span::styled("l", key),
        Span::styled(" level  ", base),
        Span::styled("c", key),
        Span::styled(" clear", base),
    ]);

    frame.render_widget(Paragraph::new(status).style(base), area);
}

fn draw_prompt(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let label = match app.mode() {
        Mode::Prompt(PromptKind::Text) => "/",
        Mode::Prompt(PromptKind::Source) => "source: ",
        Mode::Prompt(PromptKind::Level) => "level: ",
        Mode::Normal => "",
    };
    let base = Style::default().fg(THEME.text).bg(THEME.panel_alt);
    let prompt = Line::from(vec![
        Span::styled(" ", base),
        Span::styled(
            label.to_string(),
            Style::default().fg(THEME.accent).bg(THEME.panel_alt),
        ),
        Span::styled(app.prompt().to_string(), base),
    ]);

    frame.render_widget(Paragraph::new(prompt).style(base), area);
}

fn render_event(event: &LogEvent, color_enabled: bool, selected: bool) -> Line<'static> {
    let row_bg = if selected {
        THEME.panel_alt
    } else {
        THEME.background
    };
    let row_style = Style::default().fg(THEME.text).bg(row_bg);
    let rail_style = Style::default().bg(if selected { THEME.accent } else { row_bg });
    let sequence = Span::styled(
        format!("{:>6} ", event.sequence),
        Style::default().fg(THEME.line_number_fg).bg(THEME.line_number_bg),
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

    Line::from(vec![
        Span::styled(" ", rail_style),
        sequence,
        Span::styled(format!("{:<14}", truncate(&event.source, 14)), source_style),
        Span::styled(" ", row_style),
        Span::styled(format!("{:<7}", event.level.to_string()), level_style),
        Span::styled(" ", row_style),
        Span::styled(display_message(&event.message), row_style),
    ])
}

fn display_message(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }

    value.chars().take(max.saturating_sub(1)).collect::<String>() + "~"
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

    #[test]
    fn display_message_compacts_nestjs_alignment_spacing() {
        let message =
            "[Nest]      32  - 05/08/2026,      4:18:15 PM          LOG      [InstanceLoader]";

        assert_eq!(
            display_message(message),
            "[Nest] 32 - 05/08/2026, 4:18:15 PM LOG [InstanceLoader]"
        );
    }
}
