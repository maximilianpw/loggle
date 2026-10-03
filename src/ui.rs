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

use crate::{
    LogPageId,
    app::{App, DialogKind, Mode, PromptKind},
};

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
    let value_style = Style::default().fg(THEME.text).bg(THEME.panel_alt);

    let mut spans = vec![
        Span::styled(" loggle ", panel_accent_style()),
        Span::styled(follow, value_style),
        Span::styled("  retained ", style),
        Span::styled(app.retained_len().to_string(), value_style),
        Span::styled("  visible ", style),
        Span::styled(visible_count.to_string(), value_style),
        Span::styled("  markers ", style),
        Span::styled(app.marker_count().to_string(), value_style),
    ];
    if app.paused_backlog() != 0 {
        spans.extend([
            Span::styled("  new ", style),
            Span::styled(app.paused_backlog().to_string(), value_style),
        ]);
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);

    if let Some(page_id) = page_id {
        draw_page_id(frame, area, page_id, style);
    }
}

fn panel_accent_style() -> Style {
    Style::default()
        .fg(THEME.accent)
        .bg(THEME.panel_alt)
        .add_modifier(Modifier::BOLD)
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
    let line = Line::from(Span::styled(label, panel_accent_style()));
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
        let details_height = area.height.saturating_sub(1).clamp(3, 10);
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
    let accent = panel_accent_style();

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
            Span::styled(
                text::truncate_tail(&event.message, width.saturating_sub(9)),
                base,
            ),
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
                    .map(|(row, summary)| dialog::SelectableListItem {
                        shortcut: None,
                        label: &row.source,
                        description: summary,
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
        height: overlay.height.saturating_sub(2).min(1),
    };
    let line = Line::from(vec![
        Span::styled("* ", panel_accent_style()),
        Span::styled(
            message.to_string(),
            Style::default().fg(THEME.text).bg(THEME.panel_alt),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(THEME.panel_alt)),
        content,
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn render(
        app: &mut App,
        width: u16,
        height: u16,
        closing: Option<&str>,
        page_id: Option<&LogPageId>,
    ) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, app, true, closing, page_id))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn render_lines(app: &mut App, width: u16, height: u16) -> Vec<String> {
        lines(&render(app, width, height, None, None))
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| row_text(buffer, y))
            .collect()
    }

    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn column_of(buffer: &Buffer, y: u16, needle: &str) -> u16 {
        let row = row_text(buffer, y);
        let byte_index = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not found in {row:?}"));
        u16::try_from(row[..byte_index].chars().count()).unwrap()
    }

    fn app_with_lines(lines: &[&str]) -> App {
        let mut app = App::new(100);
        for line in lines {
            app.push_line((*line).to_string());
        }
        app
    }

    #[test]
    fn header_shows_follow_counts_and_page_id() {
        let mut app = app_with_lines(&["api | INFO one", "web | INFO two"]);
        let page_id = LogPageId::parse("page-42").unwrap();

        let buffer = render(&mut app, 100, 8, None, Some(&page_id));
        let header = row_text(&buffer, 0);

        assert!(header.starts_with(" loggle follow"), "{header:?}");
        assert!(header.contains("retained 2"), "{header:?}");
        assert!(header.contains("visible 2"), "{header:?}");
        assert!(header.contains("markers 0"), "{header:?}");
        assert!(header.ends_with(" id=page-42 "), "{header:?}");
    }

    #[test]
    fn header_omits_page_id_when_absent() {
        let mut app = app_with_lines(&["api | INFO one"]);

        let header = render_lines(&mut app, 100, 8).remove(0);

        assert!(header.contains("retained 1"), "{header:?}");
        assert!(!header.contains("id="), "{header:?}");
    }

    #[test]
    fn header_shows_paused_state_and_marker_count() {
        let mut app = app_with_lines(&["api | INFO one", "api | INFO two"]);
        app.move_up(1);
        app.toggle_selected_marker();

        let header = render_lines(&mut app, 100, 8).remove(0);

        assert!(header.starts_with(" loggle paused"), "{header:?}");
        assert!(header.contains("markers 1"), "{header:?}");
    }

    #[test]
    fn log_rows_render_sequence_source_level_and_message() {
        let mut app = app_with_lines(&["api | ERROR boom", "web | INFO hello"]);

        let lines = render_lines(&mut app, 80, 8);

        assert!(
            lines[1].starts_with("      0 api            error   boom"),
            "{:?}",
            lines[1]
        );
        assert!(
            lines[2].starts_with("      1 web            info    hello"),
            "{:?}",
            lines[2]
        );
    }

    #[test]
    fn selected_row_is_visually_distinguished() {
        // Following selects the newest row, so the second row is selected.
        let mut app = app_with_lines(&["api | INFO boom", "web | INFO hello"]);

        let buffer = render(&mut app, 80, 8, None, None);
        let unselected_message = column_of(&buffer, 1, "boom");
        let selected_message = column_of(&buffer, 2, "hello");

        assert_eq!(buffer[(0, 1)].bg, THEME.background);
        assert_eq!(buffer[(0, 2)].bg, THEME.accent);
        assert_eq!(buffer[(unselected_message, 1)].bg, THEME.background);
        assert_eq!(buffer[(selected_message, 2)].bg, THEME.panel_alt);
    }

    #[test]
    fn marked_row_renders_marker_in_rail() {
        let mut app = app_with_lines(&["api | INFO one", "api | INFO two"]);
        app.toggle_selected_marker();

        let lines = render_lines(&mut app, 80, 8);

        assert!(lines[1].starts_with(' '), "{:?}", lines[1]);
        assert!(lines[2].starts_with('*'), "{:?}", lines[2]);
    }

    #[test]
    fn text_filter_highlights_matching_message_text() {
        let mut app = app_with_lines(&["api | INFO a boom here", "api | INFO quiet"]);
        app.start_prompt(PromptKind::Text);
        for ch in "boom".chars() {
            app.push_prompt_char(ch);
        }
        app.apply_prompt();

        let buffer = render(&mut app, 140, 8, None, None);
        let lines = lines(&buffer);
        let start = column_of(&buffer, 1, "boom");
        let plain = buffer[(start - 2, 1)].style();

        assert!(lines[1].contains("a boom here"), "{:?}", lines[1]);
        assert!(!lines[2].contains("quiet"), "{:?}", lines[2]);
        for x in start..start + 4 {
            assert_eq!(buffer[(x, 1)].bg, THEME.highlight);
            assert_ne!(buffer[(x, 1)].style(), plain);
        }
        assert_eq!(buffer[(start + 4, 1)].style(), plain);
        assert!(lines[7].contains("search=boom"), "{:?}", lines[7]);
    }

    #[test]
    fn pinned_message_field_renders_value_or_dash() {
        let mut app = app_with_lines(&["api | INFO second", "api | INFO first tenantId=t1"]);
        app.add_selected_message_field();

        let lines = render_lines(&mut app, 100, 8);

        assert_eq!(app.message_field_keys(), ["tenantId".to_string()]);
        assert!(
            lines[1].contains(&format!("{:<20} second", "-")),
            "{:?}",
            lines[1]
        );
        assert!(
            lines[2].contains(&format!("{:<20} first tenantId=t1", "tenantId=t1")),
            "{:?}",
            lines[2]
        );
    }

    #[test]
    fn details_pane_renders_when_open_and_tall_enough() {
        let mut app = app_with_lines(&["api | WARN slow request tenantId=t1 durationMs=96"]);
        app.toggle_details();

        let text = render_lines(&mut app, 80, 12).join("\n");

        assert!(
            text.contains(" details source=api level=warn time=-"),
            "{text}"
        );
        assert!(text.contains(" message slow request"), "{text}");
        assert!(text.contains("> tenantId = t1"), "{text}");
        assert!(text.contains("  durationMs = 96"), "{text}");
    }

    #[test]
    fn details_pane_reports_missing_properties() {
        let mut app = app_with_lines(&["api | INFO plain"]);
        app.toggle_details();

        let text = render_lines(&mut app, 80, 12).join("\n");

        assert!(text.contains(" no properties"), "{text}");
    }

    #[test]
    fn details_pane_is_skipped_when_body_is_too_short() {
        let mut app = app_with_lines(&["api | INFO slow request tenantId=t1"]);
        app.toggle_details();

        // Header and footer take two rows, leaving a body of height 3.
        let text = render_lines(&mut app, 80, 5).join("\n");

        assert!(app.details_open());
        assert!(!text.contains(" details "), "{text}");
        assert!(text.contains("slow request"), "{text}");
    }

    #[test]
    fn prompt_footer_shows_label_and_prompt_text() {
        let mut app = app_with_lines(&["api | INFO one"]);
        app.start_prompt(PromptKind::Source);
        for ch in "web".chars() {
            app.push_prompt_char(ch);
        }

        let footer = render_lines(&mut app, 80, 6).pop().unwrap();

        assert!(footer.starts_with(" source: web"), "{footer:?}");
    }

    #[test]
    fn status_footer_shows_notice() {
        let mut app = app_with_lines(&["api | INFO one"]);
        app.set_notice("copied 1 line");

        let footer = render_lines(&mut app, 80, 6).pop().unwrap();

        assert!(footer.starts_with(" copied 1 line"), "{footer:?}");
    }

    #[test]
    fn status_footer_shows_visual_selection() {
        let mut app = app_with_lines(&["api | INFO one", "api | INFO two"]);
        app.start_visual_selection();
        app.move_up(1);

        let footer = render_lines(&mut app, 80, 6).pop().unwrap();

        assert!(footer.starts_with(" visual 2 lines  y copy"), "{footer:?}");
    }

    #[test]
    fn message_fields_dialog_draws_title_and_selected_row() {
        let mut app = app_with_lines(&["api | INFO first tenantId=t1"]);
        app.add_selected_message_field();
        app.open_dialog(DialogKind::MessageFields);

        let buffer = render(&mut app, 100, 24, None, None);
        let lines = lines(&buffer);
        let y = lines
            .iter()
            .position(|line| line.contains("Backspace/Delete remove"))
            .expect("pinned field row rendered");

        assert!(lines.iter().any(|line| line.contains(" Pinned fields ")));
        assert!(lines.iter().any(|line| line.contains(" search ")));
        assert!(lines[y].contains("> "), "{:?}", lines[y]);
        assert!(lines[y].contains("tenantId"), "{:?}", lines[y]);
    }

    #[test]
    fn empty_dialog_shows_placeholder_row() {
        let mut app = app_with_lines(&["api | INFO one"]);
        app.open_dialog(DialogKind::FilterPresets);

        let text = render_lines(&mut app, 100, 24).join("\n");

        assert!(text.contains(" Filter presets "), "{text}");
        assert!(text.contains("No filter presets"), "{text}");
    }

    #[test]
    fn palette_draws_commands_with_selected_row_highlighted() {
        let mut app = app_with_lines(&["api | INFO one"]);
        app.open_palette();

        let buffer = render(&mut app, 100, 24, None, None);
        let lines = lines(&buffer);
        let label = app.palette_commands()[app.palette_selected()].label;
        let y = lines
            .iter()
            .position(|line| line.contains(label))
            .expect("selected command rendered");
        let row = u16::try_from(y).unwrap();
        let x = column_of(&buffer, row, label);

        assert!(lines.iter().any(|line| line.contains(" Commands ")));
        assert!(lines[y].contains("> "), "{:?}", lines[y]);
        assert_eq!(buffer[(x, row)].bg, THEME.accent);
    }

    #[test]
    fn closing_overlay_renders_message() {
        let mut app = app_with_lines(&["api | INFO one"]);

        let buffer = render(&mut app, 80, 12, Some("Stopping sources..."), None);
        let text = lines(&buffer).join("\n");

        assert!(text.contains("* Stopping sources..."), "{text}");
    }

    #[test]
    fn page_id_is_skipped_for_narrow_areas() {
        let page_id = LogPageId::parse("page-42").unwrap();
        let mut terminal = Terminal::new(TestBackend::new(7, 1)).unwrap();

        terminal
            .draw(|frame| {
                draw_page_id(frame, frame.area(), &page_id, Style::default());
            })
            .unwrap();

        assert_eq!(row_text(terminal.backend().buffer(), 0), "       ");
    }

    #[test]
    fn page_id_is_truncated_to_fit() {
        let page_id = LogPageId::parse("a-very-long-page-identifier").unwrap();
        let mut terminal = Terminal::new(TestBackend::new(12, 1)).unwrap();

        terminal
            .draw(|frame| {
                draw_page_id(frame, frame.area(), &page_id, Style::default());
            })
            .unwrap();

        let row = row_text(terminal.backend().buffer(), 0);
        assert!(row.contains(" id="), "{row:?}");
        assert!(!row.contains("identifier"), "{row:?}");
    }

    #[test]
    fn tiny_and_empty_areas_do_not_panic() {
        let page_id = LogPageId::parse("page-42").unwrap();
        let sizes = [
            (1, 1),
            (1, 0),
            (0, 1),
            (80, 0),
            (80, 1),
            (80, 2),
            (5, 3),
            (7, 4),
        ];
        for (width, height) in sizes {
            let mut app = app_with_lines(&["api | INFO one tenantId=t1"]);
            app.toggle_details();
            render(&mut app, width, height, Some("closing"), Some(&page_id));

            app.open_palette();
            render(&mut app, width, height, None, Some(&page_id));

            app.open_dialog(DialogKind::Sources);
            render(&mut app, width, height, None, None);

            app.start_prompt(PromptKind::Text);
            render(&mut app, width, height, None, None);
        }
    }
}
