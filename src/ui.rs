mod row;
mod status;
mod text;
mod theme;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, Paragraph},
};

use crate::app::{App, Mode, PromptKind};

use theme::THEME;

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
    let follow = if app.is_following() {
        "follow"
    } else {
        "paused"
    };
    let visible = app.visible_events().len();
    let style = Style::default().fg(THEME.muted).bg(THEME.panel_alt);
    let accent_style = Style::default()
        .fg(THEME.accent)
        .bg(THEME.panel_alt)
        .add_modifier(Modifier::BOLD);

    let header = Line::from(vec![
        Span::styled(" loggle ", accent_style),
        Span::styled(
            follow,
            Style::default().fg(THEME.text).bg(THEME.panel_alt),
        ),
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
            ListItem::new(row::render_event(
                event,
                color_enabled,
                visible_index == selected,
            ))
        })
        .collect::<Vec<_>>();

    let list = List::new(items);
    frame.render_widget(list, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    match app.mode() {
        Mode::Prompt(_) => draw_prompt(frame, area, app),
        Mode::Normal => status::draw_status(frame, area, app),
    }
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
