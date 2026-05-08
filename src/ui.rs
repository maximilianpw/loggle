use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use crate::{
    app::{App, Mode, PromptKind},
    model::{Level, LogEvent},
};

pub fn draw(frame: &mut Frame<'_>, app: &App, color_enabled: bool) {
    let prompt_height = matches!(app.mode(), Mode::Prompt(_)) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(prompt_height),
        ])
        .split(frame.area());

    draw_logs(frame, chunks[0], app, color_enabled);
    draw_status(frame, chunks[1], app);

    if matches!(app.mode(), Mode::Prompt(_)) {
        draw_prompt(frame, chunks[2], app);
    }
}

fn draw_logs(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App, color_enabled: bool) {
    frame.render_widget(Clear, area);

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
            let selected_style = if visible_index == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(render_event(event, color_enabled)).style(selected_style)
        })
        .collect::<Vec<_>>();

    let list = List::new(items);
    frame.render_widget(list, area);
}

fn draw_status(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let follow = if app.is_following() { "follow" } else { "paused" };
    let text = app.filters().text.as_deref().unwrap_or("-");
    let source = app.filters().source.as_deref().unwrap_or("-");
    let level = app
        .filters()
        .level
        .map(|level| level.to_string())
        .unwrap_or_else(|| "-".to_string());
    let visible = app.visible_events().len();

    let status = format!(
        " {follow} | retained {} | visible {visible} | source {source} | level {level} | search {text} | q quit  / search  s source  l level  c clear",
        app.retained_len()
    );

    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Black).bg(Color::Gray)),
        area,
    );
}

fn draw_prompt(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let label = match app.mode() {
        Mode::Prompt(PromptKind::Text) => "/",
        Mode::Prompt(PromptKind::Source) => "source: ",
        Mode::Prompt(PromptKind::Level) => "level: ",
        Mode::Normal => "",
    };
    let prompt = format!("{label}{}", app.prompt());

    frame.render_widget(
        Paragraph::new(prompt).block(Block::default().borders(Borders::empty())),
        area,
    );
}

fn render_event(event: &LogEvent, color_enabled: bool) -> Line<'static> {
    let sequence = Span::styled(
        format!("{:>6} ", event.sequence),
        Style::default().fg(Color::DarkGray),
    );
    let source_style = if color_enabled {
        Style::default()
            .fg(source_color(&event.source))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let level_style = if color_enabled {
        Style::default().fg(level_color(event.level))
    } else {
        Style::default()
    };

    Line::from(vec![
        sequence,
        Span::styled(format!("{:<14}", truncate(&event.source, 14)), source_style),
        Span::raw(" "),
        Span::styled(format!("{:<7}", event.level.to_string()), level_style),
        Span::raw(" "),
        Span::raw(display_message(&event.message)),
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
        Level::Fatal | Level::Error => Color::Red,
        Level::Warn => Color::Yellow,
        Level::Info => Color::Green,
        Level::Debug => Color::Blue,
        Level::Trace => Color::Magenta,
        Level::Unknown => Color::DarkGray,
    }
}

fn source_color(source: &str) -> Color {
    const COLORS: [Color; 8] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::LightCyan,
        Color::LightGreen,
        Color::LightYellow,
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
