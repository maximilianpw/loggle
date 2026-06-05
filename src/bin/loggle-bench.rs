use std::{error::Error, time::Duration};

use clap::Parser;
use loggle::perf::{BenchConfig, BenchFilter, BenchReport, run_benchmark};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "loggle-bench",
    about = "Run synthetic Loggle ingestion, filter, viewport, and draw timings."
)]
struct Cli {
    #[arg(long, default_value_t = 100_000)]
    lines: usize,

    #[arg(long, default_value = "none", value_parser = parse_filter)]
    filter: BenchFilter,

    #[arg(long, default_value_t = 20)]
    iterations: usize,

    #[arg(long, default_value_t = 120)]
    width: u16,

    #[arg(long, default_value_t = 40)]
    height: u16,

    #[arg(long, help = "Emit machine-readable JSON with timings in microseconds")]
    json: bool,
}

fn parse_filter(value: &str) -> Result<BenchFilter, String> {
    BenchFilter::parse(value)
        .ok_or_else(|| "filter must be one of: none, text, source, level, property".to_string())
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let config = BenchConfig {
        lines: cli.lines,
        filter: cli.filter,
        iterations: cli.iterations,
        viewport_width: cli.width,
        viewport_height: cli.height,
    };
    let report = run_benchmark(&config);

    if cli.json {
        print_json_report(&report, &config)?;
    } else {
        print_human_report(&report);
    }

    Ok(())
}

fn print_human_report(report: &BenchReport) {
    println!("lines: {}", report.lines);
    println!("filter: {}", report.filter.as_str());
    println!("iterations: {}", report.iterations);
    println!("retained: {}", report.retained);
    println!("visible: {}", report.visible);
    println!("ingest: {}", format_duration(report.ingest));
    println!("filter_apply: {}", format_duration(report.filter_apply));
    println!("visible_count: {}", format_duration(report.visible_count));
    println!(
        "viewport_iteration: {}",
        format_duration(report.viewport_iteration)
    );
    println!("draw: {}", format_duration(report.draw));
}

fn print_json_report(report: &BenchReport, config: &BenchConfig) -> Result<(), serde_json::Error> {
    let report = JsonBenchReport::from_report(report, config);
    serde_json::to_writer_pretty(std::io::stdout(), &report)?;
    println!();
    Ok(())
}

#[derive(Debug, Serialize)]
struct JsonBenchReport<'a> {
    lines: usize,
    filter: &'a str,
    iterations: usize,
    viewport_width: u16,
    viewport_height: u16,
    retained: usize,
    visible: usize,
    timings_us: JsonBenchTimings,
}

#[derive(Debug, Serialize)]
struct JsonBenchTimings {
    ingest: u64,
    filter_apply: u64,
    visible_count: u64,
    viewport_iteration: u64,
    draw: u64,
}

impl<'a> JsonBenchReport<'a> {
    fn from_report(report: &'a BenchReport, config: &BenchConfig) -> Self {
        Self {
            lines: report.lines,
            filter: report.filter.as_str(),
            iterations: report.iterations,
            viewport_width: config.viewport_width,
            viewport_height: config.viewport_height,
            retained: report.retained,
            visible: report.visible,
            timings_us: JsonBenchTimings {
                ingest: duration_micros(report.ingest),
                filter_apply: duration_micros(report.filter_apply),
                visible_count: duration_micros(report.visible_count),
                viewport_iteration: duration_micros(report.viewport_iteration),
                draw: duration_micros(report.draw),
            },
        }
    }
}

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}

fn format_duration(duration: Duration) -> String {
    let micros = duration.as_micros();
    if micros < 1_000 {
        return format!("{micros}us");
    }

    let millis = duration.as_secs_f64() * 1_000.0;
    if millis < 1_000.0 {
        return format!("{millis:.2}ms");
    }

    format!("{:.2}s", duration.as_secs_f64())
}
