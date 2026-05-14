use std::{error::Error, time::Duration};

use clap::Parser;
use loggle::perf::{BenchConfig, BenchFilter, run_benchmark};

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
}

fn parse_filter(value: &str) -> Result<BenchFilter, String> {
    BenchFilter::parse(value)
        .ok_or_else(|| "filter must be one of: none, text, source, level, property".to_string())
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let report = run_benchmark(&BenchConfig {
        lines: cli.lines,
        filter: cli.filter,
        iterations: cli.iterations,
        viewport_width: cli.width,
        viewport_height: cli.height,
    });

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

    Ok(())
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
