mod dialog;
mod row;
mod status;
mod text;
mod theme;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, Paragraph},
};

use crate::app::{App, Mode, PromptKind};

use theme::THEME;

pub fn draw(frame: &mut Frame<'_>, app: &App, color_enabled: bool, closing: Option<&str>) {
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
    draw_body(frame, chunks[1], app, color_enabled);
    draw_footer(frame, chunks[2], app);

    if app.mode() == &Mode::Palette {
        let items = dialog::command_items(app.palette_commands());
        dialog::draw_dialog(
            frame,
            frame.area(),
            "Commands",
            &items,
            app.palette_selected(),
        );
    } else if app.mode() == &Mode::PropertyFilters {
        draw_property_filters_dialog(frame, app);
    } else if app.mode() == &Mode::MessageFields {
        draw_message_fields_dialog(frame, app);
    }

    if let Some(message) = closing {
        draw_closing_overlay(frame, frame.area(), message);
    }
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

fn draw_body(frame: &mut Frame<'_>, area: Rect, app: &App, color_enabled: bool) {
    if app.details_open() && area.height >= 4 {
        let details_height = area.height.saturating_sub(1).min(10).max(3);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(details_height)])
            .split(area);

        draw_logs(frame, chunks[0], app, color_enabled);
        draw_details(frame, chunks[1], app);
    } else {
        draw_logs(frame, area, app, color_enabled);
    }
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
                app.message_field_keys(),
            ))
        })
        .collect::<Vec<_>>();

    let list = List::new(items);
    frame.render_widget(list, area);
}

fn draw_details(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let base = Style::default().fg(THEME.text).bg(THEME.panel_alt);
    let muted = Style::default().fg(THEME.muted).bg(THEME.panel_alt);
    let accent = Style::default()
        .fg(THEME.accent)
        .bg(THEME.panel_alt)
        .add_modifier(Modifier::BOLD);

    frame.render_widget(Block::default().style(base), area);

    let Some(event) = app.selected_event() else {
        return;
    };

    let width = area.width.saturating_sub(2) as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" details ", accent),
            Span::styled("source=", muted),
            Span::styled(text::truncate_tail(&event.source, 18), base),
            Span::styled(" level=", muted),
            Span::styled(event.level.to_string(), base),
            Span::styled(" time=", muted),
            Span::styled(event.timestamp.as_deref().unwrap_or("-").to_string(), base),
        ]),
        Line::from(vec![
            Span::styled(" message ", muted),
            Span::styled(text::truncate_tail(&event.message, width.saturating_sub(9)), base),
        ]),
    ];

    if event.properties.is_empty() {
        lines.push(Line::from(Span::styled(" no properties", muted)));
    } else {
        let available = area.height.saturating_sub(2) as usize;
        let selected = app.selected_property_index();
        let start = selected.saturating_sub(available.saturating_sub(1));
        let end = (start + available).min(event.properties.len());

        for (index, property) in event.properties[start..end].iter().enumerate() {
            let property_index = start + index;
            let row_style = if property_index == selected {
                accent
            } else {
                base
            };
            let marker = if property_index == selected { ">" } else { " " };
            let value = property.value.to_string();
            let text = format!(
                "{} {} = {}",
                marker,
                text::truncate_tail(&property.key, 24),
                text::truncate_tail(&value, width.saturating_sub(31))
            );
            lines.push(Line::from(Span::styled(text, row_style)));
        }
    }

    frame.render_widget(Paragraph::new(lines).style(base), area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    match app.mode() {
        Mode::Prompt(_) => draw_prompt(frame, area, app),
        Mode::Normal | Mode::Palette | Mode::PropertyFilters | Mode::MessageFields => {
            status::draw_status(frame, area, app)
        }
    }
}

fn draw_property_filters_dialog(frame: &mut Frame<'_>, app: &App) {
    let rows = app.property_filter_rows();
    let items;
    let empty_items;
    let rendered = if rows.is_empty() {
        empty_items = [dialog::SelectableListItem {
            shortcut: None,
            label: "No property filters",
            description: "Add filters with f, +, or -",
        }];
        &empty_items[..]
    } else {
        items = rows
            .iter()
            .map(|row| dialog::SelectableListItem {
                shortcut: Some(row.kind),
                label: &row.summary,
                description: "Enter edit  Backspace/Delete remove",
            })
            .collect::<Vec<_>>();
        &items[..]
    };

    dialog::draw_searchable_dialog(
        frame,
        frame.area(),
        "Property filters",
        app.property_filter_query(),
        rendered,
        app.selected_property_filter_index(),
    );
}

fn draw_message_fields_dialog(frame: &mut Frame<'_>, app: &App) {
    let rows = app.message_field_rows();
    let items;
    let empty_items;
    let rendered = if rows.is_empty() {
        empty_items = [dialog::SelectableListItem {
            shortcut: None,
            label: "No message fields",
            description: "Add fields with m from details",
        }];
        &empty_items[..]
    } else {
        items = rows
            .iter()
            .map(|key| dialog::SelectableListItem {
                shortcut: None,
                label: key,
                description: "Backspace/Delete remove",
            })
            .collect::<Vec<_>>();
        &items[..]
    };

    dialog::draw_searchable_dialog(
        frame,
        frame.area(),
        "Message fields",
        app.message_field_query(),
        rendered,
        app.selected_message_field_index(),
    );
}

fn draw_closing_overlay(frame: &mut Frame<'_>, area: Rect, message: &str) {
    let width = area.width.clamp(32, 54).min(area.width);
    let height = 5.min(area.height);
    let overlay = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.panel_alt)),
        overlay,
    );

    let content = Rect {
        x: overlay.x.saturating_add(2),
        y: overlay.y.saturating_add(2),
        width: overlay.width.saturating_sub(4),
        height: 1,
    };
    let line = Line::from(vec![
        Span::styled(
            "* ",
            Style::default()
                .fg(THEME.accent)
                .bg(THEME.panel_alt)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(message.to_string(), Style::default().fg(THEME.text).bg(THEME.panel_alt)),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(THEME.panel_alt)), content);
}

fn draw_prompt(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let label = match app.mode() {
        Mode::Prompt(PromptKind::Text) => "/",
        Mode::Prompt(PromptKind::Source) => "source: ",
        Mode::Prompt(PromptKind::Level) => "level: ",
        Mode::Prompt(PromptKind::IncludeProperty) => "show prop: ",
        Mode::Prompt(PromptKind::ExcludeProperty) => "hide prop: ",
        Mode::Prompt(PromptKind::EditPropertyFilter) => "edit prop: ",
        Mode::Normal | Mode::Palette | Mode::PropertyFilters | Mode::MessageFields => "",
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
