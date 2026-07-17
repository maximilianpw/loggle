use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use crate::commands::Command;

use super::{text::truncate_tail, theme::THEME};

#[derive(Debug, Clone, Copy)]
pub(super) struct SelectableListItem<'a> {
    pub shortcut: Option<&'a str>,
    pub label: &'a str,
    pub description: &'a str,
}

pub(super) fn draw_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    items: &[SelectableListItem<'_>],
    selected: usize,
) {
    draw_selectable_dialog(frame, area, title, None, None, items, selected);
}

pub(super) fn draw_searchable_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    query: &str,
    summary: Option<&str>,
    items: &[SelectableListItem<'_>],
    selected: usize,
) {
    draw_selectable_dialog(frame, area, title, Some(query), summary, items, selected);
}

pub(super) fn command_items(commands: &[Command]) -> Vec<SelectableListItem<'_>> {
    commands
        .iter()
        .map(|command| SelectableListItem {
            shortcut: Some(command.shortcut),
            label: command.label,
            description: command.description,
        })
        .collect()
}

fn draw_selectable_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    query: Option<&str>,
    summary: Option<&str>,
    items: &[SelectableListItem<'_>],
    selected: usize,
) {
    let dialog = centered_dialog(area);
    frame.render_widget(Clear, dialog);

    let base = Style::default().fg(THEME.text).bg(THEME.panel_alt);
    let muted = Style::default().fg(THEME.muted).bg(THEME.panel_alt);
    let title_style = Style::default()
        .fg(THEME.accent)
        .bg(THEME.panel_alt)
        .add_modifier(Modifier::BOLD);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(format!(" {title} "), title_style)))
        .style(base);
    frame.render_widget(block, dialog);

    let content = Rect {
        x: dialog.x.saturating_add(1),
        y: dialog.y.saturating_add(1),
        width: dialog.width.saturating_sub(2),
        height: dialog.height.saturating_sub(2),
    };

    if content.height == 0 || content.width == 0 {
        return;
    }

    let mut list_area = content;
    if let Some(summary) = summary {
        let width = list_area.width as usize;
        let summary_line = Line::from(Span::styled(
            truncate_tail(summary, width).into_owned(),
            muted,
        ));
        frame.render_widget(Paragraph::new(summary_line).style(base), list_area);
        list_area.y = list_area.y.saturating_add(1);
        list_area.height = list_area.height.saturating_sub(1);
    }

    if let Some(query) = query {
        let query_width = content.width.saturating_sub(9) as usize;
        let query_line = Line::from(vec![
            Span::styled(" search ", muted),
            Span::styled(truncate_tail(query, query_width).into_owned(), base),
        ]);
        frame.render_widget(Paragraph::new(query_line).style(base), list_area);
        list_area.y = list_area.y.saturating_add(1);
        list_area.height = list_area.height.saturating_sub(1);
    }

    if list_area.height == 0 {
        return;
    }

    let visible = visible_window(items.len(), selected, list_area.height as usize);
    let rows = render_selectable_rows(
        &items[visible.clone()],
        selected.saturating_sub(visible.start),
        list_area.width as usize,
    )
    .into_iter()
    .map(ListItem::new)
    .collect::<Vec<_>>();

    frame.render_widget(List::new(rows).style(base), list_area);
}

fn centered_dialog(area: Rect) -> Rect {
    let width = area.width.clamp(32, 82).min(area.width);
    let height = area.height.clamp(8, 20).min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn visible_window(len: usize, selected: usize, height: usize) -> std::ops::Range<usize> {
    if len == 0 || height == 0 {
        return 0..0;
    }

    let selected = selected.min(len - 1);
    let start = selected.saturating_sub(height.saturating_sub(1));
    let end = (start + height).min(len);
    start..end
}

fn render_selectable_rows(
    items: &[SelectableListItem<'_>],
    selected: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let shortcut_width = items
        .iter()
        .filter_map(|item| item.shortcut)
        .map(|shortcut| shortcut.chars().count())
        .max()
        .unwrap_or(0)
        .min(10);
    let label_width = 24usize.min(width.saturating_sub(shortcut_width + 7).max(1));

    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let is_selected = index == selected;
            let row_style = if is_selected {
                Style::default()
                    .fg(THEME.background)
                    .bg(THEME.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(THEME.text).bg(THEME.panel_alt)
            };
            let muted_style = if is_selected {
                row_style
            } else {
                Style::default().fg(THEME.muted).bg(THEME.panel_alt)
            };
            let marker = if is_selected { ">" } else { " " };
            let shortcut = item.shortcut.unwrap_or("");
            let prefix_len = 2 + shortcut_width + 2 + label_width + 2;
            let description_width = width.saturating_sub(prefix_len);
            let shortcut_text = format!("{shortcut:shortcut_width$}");
            let label_text = format!("{:<label_width$}", truncate_tail(item.label, label_width));

            Line::from(vec![
                Span::styled(format!("{marker} "), row_style),
                Span::styled(shortcut_text, muted_style),
                Span::styled("  ", row_style),
                Span::styled(label_text, row_style),
                Span::styled("  ", row_style),
                Span::styled(
                    truncate_tail(item.description, description_width).into_owned(),
                    muted_style,
                ),
            ])
            .alignment(Alignment::Left)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn visible_window_tracks_selected_row() {
        assert_eq!(visible_window(20, 0, 5), 0..5);
        assert_eq!(visible_window(20, 6, 5), 2..7);
        assert_eq!(visible_window(3, 10, 5), 0..3);
    }

    #[test]
    fn selectable_rows_clip_long_text() {
        let items = [SelectableListItem {
            shortcut: Some("very-long-shortcut"),
            label: "Extremely long command label",
            description: "A description that will not fit into a narrow dialog",
        }];

        let row = render_selectable_rows(&items, 0, 28)
            .into_iter()
            .next()
            .unwrap();
        let text = row
            .spans
            .into_iter()
            .map(|span| span.content)
            .collect::<String>();

        assert!(text.contains("~"));
        assert!(!text.contains("Extremely long command label"));
        assert!(!text.contains("A description that will not fit"));
    }

    #[test]
    fn searchable_dialog_renders_optional_summary_above_query_and_rows() {
        let mut terminal = Terminal::new(TestBackend::new(82, 20)).unwrap();
        let items = [SelectableListItem {
            shortcut: Some("source"),
            label: "api",
            description: "2 records",
        }];
        let summary = "win=100000/100001 clipped src=100/100000 lvl=7/7 key=100/100000";

        terminal
            .draw(|frame| {
                draw_searchable_dialog(
                    frame,
                    frame.area(),
                    "Filter facets",
                    "ap",
                    Some(summary),
                    &items,
                    0,
                );
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains(summary));
        assert!(rendered.contains("src=100/100000"));
        assert!(rendered.contains("lvl=7/7"));
        assert!(rendered.contains("key=100/100000"));
        assert!(rendered.contains("search ap"));
        assert!(rendered.contains("source"));
        assert!(rendered.contains("api"));
    }
}
