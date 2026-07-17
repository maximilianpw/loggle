use std::{
    error::Error,
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Parser, ValueEnum};
use loggle::{
    ConfigEnv, LogLevel, LogPageError, LogPageId, LogPageQueryOptions, NamedCommand, RuntimeConfig,
    RuntimeError, RuntimeInput, SourceConfig, active_log_pages, load_named_config,
    load_project_config, print_log_page_query, query_log_page_records, run,
};

#[derive(Debug, Parser)]
#[command(
    name = "loggle",
    about = "A terminal log viewer for piped Docker Compose-style logs.",
    dont_delimit_trailing_values = true,
    after_help = "Agent log access:\n  loggle -- docker compose up\n  loggle pages\n  loggle log -i 1 -n 5\n  loggle log -i 1 -n 5 --service api --text error --property tenantId=tenant-1\n  loggle log -i 1 -n 5 --level error --format jsonl"
)]
struct Cli {
    #[arg(long, default_value_t = 100_000, value_parser = parse_buffer_lines)]
    buffer_lines: usize,

    #[arg(long)]
    no_color: bool,

    #[arg(long)]
    record: Option<std::path::PathBuf>,

    #[arg(
        short = 'i',
        long = "id",
        visible_alias = "page-id",
        value_name = "ID",
        help = "Use this log page ID instead of an auto-generated ID"
    )]
    page_id: Option<LogPageId>,

    #[arg(
        long = "no-page-log",
        help = "Disables the per-session page log used by loggle log/pages"
    )]
    no_page_log: bool,

    #[arg(long = "source-field", value_delimiter = ',', value_parser = parse_source_field)]
    source_fields: Vec<String>,

    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Command to run under loggle; use dc as a shortcut for docker compose up"
    )]
    command: Vec<String>,
}

#[derive(Debug, Parser)]
#[command(name = "loggle log", about = "Print logs from a tagged Loggle page.")]
struct LogCli {
    #[arg(short = 'i', long = "id", value_name = "ID")]
    id: LogPageId,

    #[arg(short = 'n', long = "lines", default_value_t = 100, value_parser = parse_tail_lines)]
    lines: usize,

    #[arg(
        short = 's',
        long = "source",
        visible_alias = "service",
        value_name = "SOURCE",
        value_parser = parse_source_filter
    )]
    source: Option<String>,

    #[arg(
        short = 'p',
        long = "property",
        value_name = "FILTER",
        value_parser = parse_property_filter
    )]
    property_filters: Vec<String>,

    #[arg(
        short = 't',
        long = "text",
        visible_alias = "search",
        value_name = "QUERY",
        value_parser = parse_text_filter
    )]
    text: Option<String>,

    #[arg(long, value_name = "LEVEL", value_parser = parse_level)]
    level: Option<LogLevel>,

    #[arg(long, value_enum, default_value = "raw")]
    format: LogOutputFormat,

    #[arg(long = "source-field", value_delimiter = ',', value_parser = parse_source_field)]
    source_fields: Vec<String>,
}

#[derive(Debug, Parser)]
#[command(name = "loggle pages", about = "List active tagged Loggle pages.")]
struct PagesCli {
    #[arg(long, value_enum, default_value = "table")]
    format: PagesOutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LogOutputFormat {
    Raw,
    Jsonl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PagesOutputFormat {
    Table,
    Jsonl,
}

fn parse_buffer_lines(input: &str) -> Result<usize, String> {
    let value = input
        .parse::<usize>()
        .map_err(|error| format!("invalid buffer size: {error}"))?;

    if value == 0 {
        Err("buffer size must be greater than zero".to_string())
    } else {
        Ok(value)
    }
}

fn parse_tail_lines(input: &str) -> Result<usize, String> {
    let value = input
        .parse::<usize>()
        .map_err(|error| format!("invalid line count: {error}"))?;

    Ok(value)
}

fn parse_non_empty(input: &str, label: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(input.to_string())
    }
}

fn parse_source_field(input: &str) -> Result<String, String> {
    parse_non_empty(input, "source field")
}

fn parse_source_filter(input: &str) -> Result<String, String> {
    parse_non_empty(input, "source filter")
}

fn parse_property_filter(input: &str) -> Result<String, String> {
    parse_non_empty(input, "property filter")
}

fn parse_text_filter(input: &str) -> Result<String, String> {
    parse_non_empty(input, "text filter")
}

fn parse_level(input: &str) -> Result<LogLevel, String> {
    LogLevel::parse(input).ok_or_else(|| {
        format!(
            "invalid level '{input}'; expected one of: fatal, error, warn, info, debug, trace, unknown"
        )
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let raw_args = std::env::args().skip(1).collect::<Vec<_>>();
    if raw_args.first().is_some_and(|arg| arg == "log") {
        let cli = LogCli::parse_from(
            std::iter::once("loggle log".to_string()).chain(raw_args.iter().skip(1).cloned()),
        );
        return report_command(run_log_command(cli));
    }
    if raw_args.first().is_some_and(|arg| arg == "pages") {
        let cli = PagesCli::parse_from(
            std::iter::once("loggle pages".to_string()).chain(raw_args.iter().skip(1).cloned()),
        );
        return report_command(run_pages_command(cli));
    }

    let cli = Cli::parse();
    // clap captures the trailing command verbatim (trailing_var_arg), so it is
    // the single source of truth for what to run — no hand-rolled arg skipping.
    let resolved_input = match runtime_input_for_command(cli.command) {
        Ok(input) => input,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    let source_fields = merged_source_fields(cli.source_fields, resolved_input.source_fields);

    match run(RuntimeConfig {
        buffer_lines: cli.buffer_lines,
        color_enabled: !cli.no_color,
        source_config: SourceConfig::with_fields(source_fields),
        page_command: runtime_input_summary(&resolved_input.input),
        input: resolved_input.input,
        record_path: cli.record,
        page_id: cli.page_id,
        page_logging: !cli.no_page_log,
    }) {
        Ok(()) => Ok(()),
        Err(RuntimeError::MissingInput) => {
            eprintln!("{}", RuntimeError::MissingInput);
            std::process::exit(1);
        }
        Err(error) => Err(Box::new(error)),
    }
}

fn report_command(result: Result<(), LogPageError>) -> Result<(), Box<dyn Error>> {
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }

    Ok(())
}

fn run_log_command(cli: LogCli) -> Result<(), LogPageError> {
    let LogCli {
        id,
        lines,
        source,
        property_filters,
        text,
        level,
        format,
        source_fields,
    } = cli;
    let mut options = LogPageQueryOptions::new(lines);
    options.source = source;
    options.text = text;
    options.level = level;
    options.property_filters = property_filters;
    options.source_config = SourceConfig::with_fields(source_fields);

    let mut stdout = io::stdout().lock();
    match format {
        LogOutputFormat::Raw => print_log_page_query(&id, &options, &mut stdout),
        LogOutputFormat::Jsonl => {
            for record in query_log_page_records(&id, &options)? {
                write_json_line(&mut stdout, &record)?;
            }
            Ok(())
        }
    }
}

fn run_pages_command(cli: PagesCli) -> Result<(), LogPageError> {
    let pages = active_log_pages()?;
    let mut stdout = io::stdout().lock();
    if cli.format == PagesOutputFormat::Jsonl {
        for page in pages {
            write_json_line(&mut stdout, &page)?;
        }
        return Ok(());
    }

    if pages.is_empty() {
        writeln!(stdout, "no active loggle pages").map_err(LogPageError::Output)?;
        return Ok(());
    }

    writeln!(stdout, "ID\tPID\tAGE\tCOMMAND").map_err(LogPageError::Output)?;
    let now = current_unix_seconds();
    for page in pages {
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}",
            page.id,
            page.pid,
            format_age(page.started_unix_seconds, now),
            page.command
        )
        .map_err(LogPageError::Output)?;
    }

    Ok(())
}

fn write_json_line<W: Write, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), LogPageError> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| LogPageError::Output(io::Error::other(error)))?;
    writeln!(writer).map_err(LogPageError::Output)
}

fn runtime_input_summary(input: &RuntimeInput) -> String {
    match input {
        RuntimeInput::Stdin => "stdin".to_string(),
        RuntimeInput::Command(command) => command.join(" "),
        RuntimeInput::Commands(commands) => {
            command_names_summary("run", commands.iter().map(|c| &c.name))
        }
        RuntimeInput::StartCommands(commands) => {
            command_names_summary("start", commands.iter().map(|c| &c.name))
        }
    }
}

fn command_names_summary<'a>(prefix: &str, names: impl Iterator<Item = &'a String>) -> String {
    let names = names.map(String::as_str).collect::<Vec<_>>().join(", ");
    if names.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {names}")
    }
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_age(started_unix_seconds: u64, now_unix_seconds: u64) -> String {
    let seconds = now_unix_seconds.saturating_sub(started_unix_seconds);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 60 * 60 * 24 {
        format!("{}h", seconds / 60 / 60)
    } else {
        format!("{}d", seconds / 60 / 60 / 24)
    }
}

fn merged_source_fields(
    cli_source_fields: Vec<String>,
    config_source_fields: Vec<String>,
) -> Vec<String> {
    cli_source_fields
        .into_iter()
        .chain(config_source_fields)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedRuntimeInput {
    input: RuntimeInput,
    source_fields: Vec<String>,
}

impl ResolvedRuntimeInput {
    fn new(input: RuntimeInput) -> Self {
        Self {
            input,
            source_fields: Vec::new(),
        }
    }
}

fn runtime_input_for_command(command: Vec<String>) -> Result<ResolvedRuntimeInput, String> {
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("could not read current directory: {error}"))?;
    let config_env = ConfigEnv::from_env();

    runtime_input_for_command_with_context(command, &current_dir, &config_env)
}

fn runtime_input_for_command_with_context(
    command: Vec<String>,
    current_dir: &Path,
    config_env: &ConfigEnv,
) -> Result<ResolvedRuntimeInput, String> {
    if command.is_empty() {
        return Ok(ResolvedRuntimeInput::new(RuntimeInput::Stdin));
    }

    if command[0] == "run" {
        return parse_runner_commands(&command[1..])
            .map(RuntimeInput::Commands)
            .map(ResolvedRuntimeInput::new);
    }

    if command[0] == "start" {
        return parse_start_command(&command[1..], current_dir, config_env);
    }

    Ok(ResolvedRuntimeInput::new(RuntimeInput::Command(
        command_for_runtime(command),
    )))
}

fn command_for_runtime(command: Vec<String>) -> Vec<String> {
    if command == ["dc"] {
        vec!["docker".into(), "compose".into(), "up".into()]
    } else {
        command
    }
}

fn parse_runner_commands(args: &[String]) -> Result<Vec<NamedCommand>, String> {
    if args.is_empty() {
        return Err("runner mode requires at least one --name <name> -- <command>".to_string());
    }

    let mut commands = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args.get(index).map(String::as_str) != Some("--name") {
            return Err("runner commands must start with --name <name> -- <command>".to_string());
        }
        index += 1;

        let Some(name) = args.get(index).map(|name| name.trim()) else {
            return Err("--name requires a process name".to_string());
        };
        if name.is_empty() || name == "--" {
            return Err("--name requires a process name".to_string());
        }
        let name = name.to_string();
        index += 1;

        if args.get(index).map(String::as_str) != Some("--") {
            return Err(format!(
                "runner command '{name}' must include -- before the command"
            ));
        }
        index += 1;

        let command_start = index;
        while index < args.len() && args[index] != "--name" {
            index += 1;
        }

        if command_start == index {
            return Err(format!("runner command '{name}' is empty"));
        }

        commands.push(NamedCommand {
            name,
            command: args[command_start..index].to_vec(),
            cwd: None,
        });
    }

    Ok(commands)
}

fn parse_start_command(
    args: &[String],
    current_dir: &Path,
    config_env: &ConfigEnv,
) -> Result<ResolvedRuntimeInput, String> {
    if args.len() > 1 {
        return Err("start accepts at most one config name".to_string());
    }

    let config = if let Some(name) = args.first() {
        load_named_config(name, config_env)
    } else {
        load_project_config(current_dir)
    }
    .map_err(|error| error.to_string())?;

    Ok(ResolvedRuntimeInput {
        input: RuntimeInput::StartCommands(config.commands),
        source_fields: config.source_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use loggle::StartCommand;
    use std::collections::BTreeMap;
    use std::fs;

    fn command(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn named_command(name: &str, values: &[&str]) -> NamedCommand {
        NamedCommand {
            name: name.to_string(),
            command: command(values),
            cwd: None,
        }
    }

    fn runtime_input(command: Vec<String>) -> RuntimeInput {
        runtime_input_for_command(command).unwrap().input
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("loggle-cli-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_config(path: &std::path::Path, root: &std::path::Path) {
        fs::write(
            path,
            format!(
                r#"
root = "{}"
source_fields = ["service", "app"]

[commands]
api = ["pnpm", "start"]
"#,
                root.display()
            ),
        )
        .unwrap();
    }

    #[test]
    fn dc_expands_to_docker_compose_up() {
        assert_eq!(
            runtime_input(command(&["dc"])),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn dc_with_arguments_is_not_a_compose_shortcut() {
        assert_eq!(
            runtime_input(command(&["dc", "logs", "-f"])),
            RuntimeInput::Command(command(&["dc", "logs", "-f"]))
        );
    }

    #[test]
    fn ordinary_commands_are_unchanged() {
        assert_eq!(
            runtime_input(command(&["docker", "compose", "logs", "-f"])),
            RuntimeInput::Command(command(&["docker", "compose", "logs", "-f"]))
        );
    }

    #[test]
    fn empty_command_reads_from_stdin() {
        assert_eq!(runtime_input(Vec::new()), RuntimeInput::Stdin);
    }

    #[test]
    fn cli_source_fields_are_checked_before_config_source_fields() {
        assert_eq!(
            merged_source_fields(command(&["logger"]), command(&["service", "logger"])),
            command(&["logger", "service", "logger"])
        );
    }

    fn parse_cli(raw_args: &[String]) -> Cli {
        Cli::try_parse_from(std::iter::once("loggle".to_string()).chain(raw_args.iter().cloned()))
            .unwrap()
    }

    #[test]
    fn runner_cli_parses_two_named_commands() {
        let cli = parse_cli(&command(&[
            "run", "--name", "api", "--", "pnpm", "start", "--name", "web", "--", "pnpm", "dev",
        ]));

        assert_eq!(
            runtime_input(cli.command),
            RuntimeInput::Commands(vec![
                named_command("api", &["pnpm", "start"]),
                named_command("web", &["pnpm", "dev"]),
            ])
        );
    }

    #[test]
    fn runner_cli_preserves_command_arguments_after_top_level_separator() {
        let cli = parse_cli(&command(&["--", "docker", "compose", "up", "--watch"]));

        assert_eq!(
            runtime_input(cli.command),
            RuntimeInput::Command(command(&["docker", "compose", "up", "--watch"]))
        );
    }

    #[test]
    fn record_option_does_not_consume_runtime_command() {
        let cli = parse_cli(&command(&[
            "--record",
            "session.log",
            "docker",
            "compose",
            "up",
        ]));

        assert_eq!(
            runtime_input(cli.command),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn page_id_option_does_not_consume_runtime_command() {
        let cli = parse_cli(&command(&["--id", "1", "docker", "compose", "up"]));

        assert_eq!(cli.page_id.as_ref().unwrap().as_str(), "1");
        assert_eq!(
            runtime_input(cli.command),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn short_page_id_option_does_not_consume_runtime_command() {
        let cli = parse_cli(&command(&["-i", "1", "docker", "compose", "up"]));

        assert_eq!(cli.page_id.as_ref().unwrap().as_str(), "1");
        assert_eq!(
            runtime_input(cli.command),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn no_page_log_flag_does_not_consume_runtime_command() {
        let cli = parse_cli(&command(&["--no-page-log", "docker", "compose", "up"]));

        assert!(cli.no_page_log);
        assert_eq!(
            runtime_input(cli.command),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn log_command_cli_parses_tail_request() {
        let cli = LogCli::try_parse_from(command(&[
            "loggle log",
            "-i",
            "1",
            "-n",
            "5",
            "--service",
            "api",
            "--text",
            "database",
            "--property",
            "tenantId=tenant-1",
            "--source-field",
            "service",
        ]))
        .unwrap();

        assert_eq!(cli.id.as_str(), "1");
        assert_eq!(cli.lines, 5);
        assert_eq!(cli.source.as_deref(), Some("api"));
        assert_eq!(cli.text.as_deref(), Some("database"));
        assert_eq!(cli.level, None);
        assert_eq!(cli.format, LogOutputFormat::Raw);
        assert_eq!(cli.property_filters, command(&["tenantId=tenant-1"]));
        assert_eq!(cli.source_fields, command(&["service"]));
    }

    #[test]
    fn log_command_cli_parses_canonical_level_aliases_and_formats() {
        for (alias, expected) in [
            ("fatal", LogLevel::Fatal),
            ("ERR", LogLevel::Error),
            ("warning", LogLevel::Warn),
            ("log", LogLevel::Info),
            ("debug", LogLevel::Debug),
            ("verbose", LogLevel::Trace),
            ("unknown", LogLevel::Unknown),
        ] {
            let cli = LogCli::try_parse_from(command(&[
                "loggle log",
                "-i",
                "1",
                "--level",
                alias,
                "--format",
                "jsonl",
            ]))
            .unwrap();

            assert_eq!(cli.level, Some(expected));
            assert_eq!(cli.format, LogOutputFormat::Jsonl);
        }
    }

    #[test]
    fn log_command_cli_rejects_invalid_level_with_canonical_values() {
        let error =
            LogCli::try_parse_from(command(&["loggle log", "-i", "1", "--level", "notice"]))
                .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        let message = error.to_string();
        assert!(message.contains("invalid level 'notice'"));
        assert!(message.contains("fatal, error, warn, info, debug, trace, unknown"));
    }

    #[test]
    fn command_formats_are_scoped_and_defaulted() {
        let pages = PagesCli::try_parse_from(command(&["loggle pages"])).unwrap();
        assert_eq!(pages.format, PagesOutputFormat::Table);

        let pages =
            PagesCli::try_parse_from(command(&["loggle pages", "--format", "jsonl"])).unwrap();
        assert_eq!(pages.format, PagesOutputFormat::Jsonl);

        assert!(PagesCli::try_parse_from(command(&["loggle pages", "--format", "raw"])).is_err());
        assert!(
            LogCli::try_parse_from(command(&["loggle log", "-i", "1", "--format", "table"]))
                .is_err()
        );
    }

    #[test]
    fn runtime_input_summary_describes_page_command() {
        assert_eq!(
            runtime_input_summary(&RuntimeInput::Command(command(&[
                "docker", "compose", "up"
            ]))),
            "docker compose up"
        );
        assert_eq!(
            runtime_input_summary(&RuntimeInput::Commands(vec![
                named_command("api", &["pnpm", "start"]),
                named_command("web", &["pnpm", "dev"]),
            ])),
            "run api, web"
        );
    }

    #[test]
    fn format_age_uses_compact_units() {
        assert_eq!(format_age(100, 105), "5s");
        assert_eq!(format_age(100, 220), "2m");
        assert_eq!(format_age(100, 7300), "2h");
        assert_eq!(format_age(100, 172900), "2d");
    }

    #[test]
    fn runner_rejects_missing_name() {
        assert_eq!(
            runtime_input_for_command(command(&["run", "api", "--", "pnpm", "start"])).unwrap_err(),
            "runner commands must start with --name <name> -- <command>"
        );
    }

    #[test]
    fn runner_rejects_empty_command() {
        assert_eq!(
            runtime_input_for_command(command(&["run", "--name", "api", "--"])).unwrap_err(),
            "runner command 'api' is empty"
        );
    }

    #[test]
    fn runner_rejects_missing_command_separator() {
        assert_eq!(
            runtime_input_for_command(command(&["run", "--name", "api", "pnpm", "start"]))
                .unwrap_err(),
            "runner command 'api' must include -- before the command"
        );
    }

    #[test]
    fn start_without_name_loads_project_config() {
        let project_dir = temp_dir("project");
        let root = project_dir.join("workspace");
        fs::create_dir_all(&root).unwrap();
        write_config(&project_dir.join(".loggle.toml"), &root);

        let resolved = runtime_input_for_command_with_context(
            command(&["start"]),
            &project_dir,
            &ConfigEnv {
                xdg_config_home: None,
                home: None,
            },
        )
        .unwrap();

        assert_eq!(resolved.source_fields, command(&["service", "app"]));
        assert_eq!(
            resolved.input,
            RuntimeInput::StartCommands(vec![StartCommand {
                name: "api".to_string(),
                argv: command(&["pnpm", "start"]),
                cwd: Some(root),
                env: BTreeMap::new(),
                wait_for: Vec::new(),
                ready: None,
            }])
        );
        let _ = fs::remove_dir_all(project_dir);
    }

    #[test]
    fn start_without_name_reports_missing_project_config() {
        let project_dir = temp_dir("missing-project");
        let error = runtime_input_for_command_with_context(
            command(&["start"]),
            &project_dir,
            &ConfigEnv {
                xdg_config_home: None,
                home: None,
            },
        )
        .unwrap_err();

        assert!(error.contains(".loggle.toml"));
        assert!(error.starts_with("config file not found: "));
        let _ = fs::remove_dir_all(project_dir);
    }

    #[test]
    fn start_with_name_loads_named_home_config() {
        let home = temp_dir("home");
        let config_dir = home.join(".config").join("loggle");
        let root = home.join("workspace");
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&root).unwrap();
        write_config(&config_dir.join("libre.toml"), &root);

        let resolved = runtime_input_for_command_with_context(
            command(&["start", "libre"]),
            &home,
            &ConfigEnv {
                xdg_config_home: None,
                home: Some(home.clone()),
            },
        )
        .unwrap();

        assert_eq!(
            resolved.input,
            RuntimeInput::StartCommands(vec![StartCommand {
                name: "api".to_string(),
                argv: command(&["pnpm", "start"]),
                cwd: Some(root),
                env: BTreeMap::new(),
                wait_for: Vec::new(),
                ready: None,
            }])
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn start_rejects_extra_args() {
        assert_eq!(
            runtime_input_for_command_with_context(
                command(&["start", "libre", "extra"]),
                Path::new("/tmp"),
                &ConfigEnv {
                    xdg_config_home: None,
                    home: Some(std::path::PathBuf::from("/tmp")),
                },
            )
            .unwrap_err(),
            "start accepts at most one config name"
        );
    }
}
