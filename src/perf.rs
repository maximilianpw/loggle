use std::time::{Duration, Instant};

use ratatui::{Terminal, backend::TestBackend};

use crate::{
    app::{App, PromptKind},
    model::SourceConfig,
    ui,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchFilter {
    None,
    Text,
    Source,
    Level,
    Property,
}

impl BenchFilter {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "text" => Some(Self::Text),
            "source" => Some(Self::Source),
            "level" => Some(Self::Level),
            "property" => Some(Self::Property),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Text => "text",
            Self::Source => "source",
            Self::Level => "level",
            Self::Property => "property",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchConfig {
    pub lines: usize,
    pub filter: BenchFilter,
    pub iterations: usize,
    pub viewport_width: u16,
    pub viewport_height: u16,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            lines: 100_000,
            filter: BenchFilter::None,
            iterations: 20,
            viewport_width: 120,
            viewport_height: 40,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchReport {
    pub lines: usize,
    pub filter: BenchFilter,
    pub iterations: usize,
    pub retained: usize,
    pub visible: usize,
    pub ingest: Duration,
    pub filter_apply: Duration,
    pub visible_count: Duration,
    pub viewport_iteration: Duration,
    pub draw: Duration,
}

pub fn run_benchmark(config: &BenchConfig) -> BenchReport {
    let started = Instant::now();
    let mut app = App::with_source_config(config.lines.max(1), SourceConfig::default());
    for index in 0..config.lines {
        app.push_line(synthetic_line(index));
    }
    let ingest = started.elapsed();

    let started = Instant::now();
    apply_filter(&mut app, config.filter);
    let filter_apply = started.elapsed();

    let started = Instant::now();
    let mut visible = 0;
    for _ in 0..config.iterations {
        visible = app.visible_count();
    }
    let visible_count = started.elapsed();

    let started = Instant::now();
    for _ in 0..config.iterations {
        let selected = app.selected();
        let start = selected.saturating_sub(config.viewport_height as usize);
        app.for_each_visible_event(start, config.viewport_height as usize, |_, event| {
            std::hint::black_box(event.sequence);
        });
    }
    let viewport_iteration = started.elapsed();

    let mut terminal = Terminal::new(TestBackend::new(
        config.viewport_width,
        config.viewport_height,
    ))
    .expect("test backend should initialize");
    let started = Instant::now();
    for _ in 0..config.iterations {
        terminal
            .draw(|frame| ui::draw(frame, &mut app, true, None))
            .expect("test backend draw should succeed");
    }
    let draw = started.elapsed();

    BenchReport {
        lines: config.lines,
        filter: config.filter,
        iterations: config.iterations,
        retained: app.retained_len(),
        visible,
        ingest,
        filter_apply,
        visible_count,
        viewport_iteration,
        draw,
    }
}

fn apply_filter(app: &mut App, filter: BenchFilter) {
    match filter {
        BenchFilter::None => {}
        BenchFilter::Text => apply_prompt(app, PromptKind::Text, "request completed"),
        BenchFilter::Source => apply_prompt(app, PromptKind::Source, "api"),
        BenchFilter::Level => apply_prompt(app, PromptKind::Level, "error"),
        BenchFilter::Property => apply_prompt(app, PromptKind::IncludeProperty, "tenantId=tenant-4"),
    }
}

fn apply_prompt(app: &mut App, kind: PromptKind, value: &str) {
    app.start_prompt(kind);
    while !app.prompt().is_empty() {
        app.pop_prompt_char();
    }
    for ch in value.chars() {
        app.push_prompt_char(ch);
    }
    app.apply_prompt();
}

fn synthetic_line(index: usize) -> String {
    let source = match index % 4 {
        0 => "api",
        1 => "worker",
        2 => "web",
        _ => "db",
    };
    let level = match index % 8 {
        0 => "ERROR",
        1 | 2 => "WARN",
        _ => "INFO",
    };
    format!(
        "{source} | {level} request completed tenantId=tenant-{} requestId=req-{index:08} durationMs={}",
        index % 10,
        15 + (index % 200)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_harness_exercises_filtered_draw_path() {
        let report = run_benchmark(&BenchConfig {
            lines: 256,
            filter: BenchFilter::Property,
            iterations: 2,
            viewport_width: 80,
            viewport_height: 20,
        });

        assert_eq!(report.retained, 256);
        assert!(report.visible > 0);
        assert!(report.visible < report.retained);
    }
}
