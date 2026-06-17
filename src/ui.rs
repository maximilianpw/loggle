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

use crate::{LogPageId, app::{App, DialogKind, Mode, PromptKind}};

use theme::THEME;

pub fn draw(
    frame: &mut Frame<'_>,
    app: &mut App,
    color_enabled: bool,
    closing: Option<&str>,
    page_id: Option<&LogPageId>,
) {
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

    let visible_count = app.visible_count();

    draw_header(frame, chunks[0], app, visible_count, page_id);
    draw_body(frame, chunks[1], app, color_enabled, visible_count);
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
    } else if let Mode::Dialog(kind) = *app.mode() {
        draw_searchable_dialog(frame, app, kind);
    }

    if let Some(message) = closing {
        draw_closing_overlay(frame, frame.area(), message);
    }
}

fn draw_header(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    visible_count: usize,
    page_id: Option<&LogPageId>,
) {
    let follow = if app.is_following() {
        "follow"
    } else {
        "paused"
    };
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
            visible_count.to_string(),
            Style::default().fg(THEME.text).bg(THEME.panel_alt),
        ),
        Span::styled("  markers ", style),
        Span::styled(
            app.marker_count().to_string(),
            Style::default().fg(THEME.text).bg(THEME.panel_alt),
        ),
    ]);
    let header = if app.paused_backlog() == 0 {
        header
    } else {
        let mut spans = header.spans;
        spans.extend([
            Span::styled("  new ", style),
            Span::styled(
                app.paused_backlog().to_string(),
                Style::default().fg(THEME.text).bg(THEME.panel_alt),
            ),
        ]);
        Line::from(spans)
    };

    frame.render_widget(Paragraph::new(header).style(style), area);

    if let Some(page_id) = page_id {
        draw_page_id(frame, area, page_id, style);
    }
}

fn draw_page_id(frame: &mut Frame<'_>, area: Rect, page_id: &LogPageId, style: Style) {
    if area.width < 8 {
        return;
    }

    let available = area.width.saturating_sub(5) as usize;
    let value = text::truncate_tail(page_id.as_str(), available);
    let label = format!(" id={value} ");
    let width = (label.len() as u16).min(area.width);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width),
        y: area.y,
        width,
        height: area.height,
    };
    let line = Line::from(Span::styled(
        label,
        Style::default()
            .fg(THEME.accent)
            .bg(THEME.panel_alt)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(line).style(style), rect);
}

fn draw_body(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    color_enabled: bool,
    visible_count: usize,
) {
    if app.details_open() && area.height >= 4 {
        let details_height = area.height.saturating_sub(1).min(10).max(3);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(details_height)])
            .split(area);

        draw_logs(frame, chunks[0], app, color_enabled, visible_count);
        draw_details(frame, chunks[1], app);
    } else {
        draw_logs(frame, area, app, color_enabled, visible_count);
    }
}

fn draw_logs(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    color_enabled: bool,
    visible_count: usize,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.background)),
        area,
    );

    let viewport_height = area.height as usize;
    app.sync_log_viewport(viewport_height);

    let mut highlight_values = app.filters().property_highlight_values();
    if let Some(query) = app
        .filters()
        .text
        .as_deref()
        .filter(|query| !query.is_empty())
    {
        highlight_values.push(query);
    }
    let selected = app.selected();
    let visual_range = app.visual_selection_range();
    let start = app.log_viewport_start();
    let end = start.saturating_add(viewport_height).min(visible_count);

    let mut items = Vec::with_capacity(end.saturating_sub(start));
    app.for_each_visible_event(start, end.saturating_sub(start), |visible_index, event| {
        let in_visual_range =
            visual_range.is_some_and(|(start, end)| (start..=end).contains(&visible_index));
        items.push(ListItem::new(row::render_event(
            event,
            color_enabled,
            visible_index == selected || in_visual_range,
            app.is_marked(event.sequence),
            app.message_field_keys(),
            &highlight_values,
        )));
    });

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
            Span::styled(event.level.as_str(), base),
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
            let value = property.value.as_display_str();
            let text = format!(
                "{} {} = {}",
                marker,
                text::truncate_tail(&property.key, 24),
                text::truncate_tail(value.as_ref(), width.saturating_sub(31))
            );
            lines.push(Line::from(Span::styled(text, row_style)));
        }
    }

    frame.render_widget(Paragraph::new(lines).style(base), area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    match app.mode() {
        Mode::Prompt(_) => draw_prompt(frame, area, app),
        Mode::Normal | Mode::Visual | Mode::Palette | Mode::Dialog(_) => {
            status::draw_status(frame, area, app)
        }
    }
}

fn draw_searchable_dialog(frame: &mut Frame<'_>, app: &App, kind: DialogKind) {
    let title = match kind {
        DialogKind::PropertyFilters => "Property filters",
        DialogKind::MessageFields => "Pinned fields",
        DialogKind::FilterPresets => "Filter presets",
        DialogKind::Sources => "Sources",
    };
    let empty_item = empty_dialog_item(kind);
    let property_rows;
    let message_rows;
    let preset_rows;
    let source_rows;
    let source_summaries;
    let items;
    let rendered = match kind {
        DialogKind::PropertyFilters => {
            property_rows = app.property_filter_rows();
            if property_rows.is_empty() {
                std::slice::from_ref(&empty_item)
            } else {
                items = property_rows
                    .iter()
                    .map(|row| dialog::SelectableListItem {
                        shortcut: Some(row.kind),
                        label: &row.summary,
                        description: "Enter edit  Backspace/Delete remove",
                    })
                    .collect::<Vec<_>>();
                &items[..]
            }
        }
        DialogKind::MessageFields => {
            message_rows = app.message_field_rows();
            if message_rows.is_empty() {
                std::slice::from_ref(&empty_item)
            } else {
                items = message_rows
                    .iter()
                    .map(|key| dialog::SelectableListItem {
                        shortcut: None,
                        label: key,
                        description: "Backspace/Delete remove",
                    })
                    .collect::<Vec<_>>();
                &items[..]
            }
        }
        DialogKind::FilterPresets => {
            preset_rows = app.filter_preset_rows();
            if preset_rows.is_empty() {
                std::slice::from_ref(&empty_item)
            } else {
                items = preset_rows
                    .iter()
                    .map(|row| dialog::SelectableListItem {
                        shortcut: None,
                        label: &row.name,
                        description: &row.summary,
                    })
                    .collect::<Vec<_>>();
                &items[..]
            }
        }
        DialogKind::Sources => {
            source_rows = app.source_status_rows();
            if source_rows.is_empty() {
                std::slice::from_ref(&empty_item)
            } else {
                source_summaries = source_rows
                    .iter()
                    .map(|row| {
                        format!(
                            "{} rows  {} errors  {} warnings  last {} #{}",
                            row.count, row.errors, row.warnings, row.last_level, row.last_sequence
                        )
                    })
                    .collect::<Vec<_>>();
                items = source_rows
                    .iter()
                    .zip(source_summaries.iter())
                    .map(|(row, summary)| {
                        dialog::SelectableListItem {
                            shortcut: None,
                            label: &row.source,
                            description: summary,
                        }
                    })
                    .collect::<Vec<_>>();
                &items[..]
            }
        }
    };

    dialog::draw_searchable_dialog(
        frame,
        frame.area(),
        title,
        app.dialog_query(kind),
        rendered,
        app.selected_dialog_index(kind),
    );
}

fn empty_dialog_item(kind: DialogKind) -> dialog::SelectableListItem<'static> {
    match kind {
        DialogKind::PropertyFilters => dialog::SelectableListItem {
            shortcut: None,
            label: "No property filters",
            description: "Add filters with f, +, or -",
        },
        DialogKind::MessageFields => dialog::SelectableListItem {
            shortcut: None,
            label: "No pinned fields",
            description: "Add fields with m from details",
        },
        DialogKind::FilterPresets => dialog::SelectableListItem {
            shortcut: None,
            label: "No filter presets",
            description: "Save the current filters with S",
        },
        DialogKind::Sources => dialog::SelectableListItem {
            shortcut: None,
            label: "No sources",
            description: "Observed sources appear after logs arrive",
        },
    }
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
        Mode::Normal | Mode::Visual | Mode::Palette | Mode::Dialog(_) => "",
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
