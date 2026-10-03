use std::{
    error::Error,
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Args, Parser, Subcommand};
use loggle::{
    ConfigEnv, LogPageError, LogPageId, LogPageTailOptions, NamedCommand, RuntimeConfig,
    RuntimeError, RuntimeInput, SourceConfig, active_log_pages, load_named_config,
    load_project_config, print_log_page_sources, print_log_page_tail_with_options, run,
};

/// Printed when loggle is started from a terminal with nothing to read.
const USAGE: &str = "loggle reads newline-delimited logs from stdin or runs commands.\n\nUsage:\n  docker compose up 2>&1 | loggle\n  loggle -- docker compose up\n  loggle pages\n  loggle log -i 1 -n 5\n  loggle log -i 1 -n 5 --service api --property tenantId=tenant-1\n  loggle run --name api -- pnpm start --name web -- pnpm dev\n  loggle start [name]";

#[derive(Debug, Parser)]
#[command(
    name = "loggle",
    about = "A terminal log viewer for piped Docker Compose-style logs.",
    dont_delimit_trailing_values = true,
    // `loggle help` must stay a bare command, as it was before subcommands.
    disable_help_subcommand = true,
    subcommand_value_name = "SUBCOMMAND",
    subcommand_help_heading = "Subcommands",
    override_usage = "loggle [OPTIONS] [--] [COMMAND]...\n       loggle [OPTIONS] <SUBCOMMAND>",
    after_help = "Agent log access:\n  loggle -- docker compose up\n  loggle pages\n  loggle sources -i 1\n  loggle log -i 1 -n 5 --clean\n  loggle log -i 1 -n 5 --service api --text error --property tenantId=tenant-1"
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

    #[command(subcommand)]
    subcommand: Option<CliCommand>,

    // A first word matching a subcommand name is routed to the subcommand;
    // anything after `--` is always the command.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Command to run under loggle; use dc as a shortcut for docker compose up"
    )]
    command: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    #[command(flatten)]
    Runtime(RuntimeCommand),

    #[command(about = "Print logs from a tagged Loggle page.")]
    Log(LogArgs),

    #[command(about = "List active tagged Loggle pages.")]
    Pages,

    #[command(
        about = "List observed source names and record counts in a retained page (not Compose service aliases)."
    )]
    Sources(SourcesArgs),
}

/// Subcommands that open the viewer, as opposed to querying page logs.
#[derive(Debug, Subcommand)]
enum RuntimeCommand {
    #[command(
        about = "Run one or more named commands in one Loggle session.",
        after_help = "Each command is a --name NAME -- COMMAND... group; output lines are prefixed with [NAME].\n\nExample:\n  loggle run --name api -- pnpm start --name web -- pnpm dev"
    )]
    Run(RunArgs),

    #[command(
        about = "Launch commands from .loggle.toml, or from a named config in the Loggle user config directory."
    )]
    Start(StartArgs),
}

#[derive(Debug, Args)]
struct RunArgs {
    // clap cannot express repeated `--name NAME -- CMD...` groups, so the raw
    // words are captured here and split by `parse_runner_commands`.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "--name NAME -- COMMAND",
        help = "Named command groups to run"
    )]
    groups: Vec<String>,
}

#[derive(Debug, Args)]
struct StartArgs {
    #[arg(
        value_name = "NAME",
        help = "Named config to load instead of ./.loggle.toml"
    )]
    name: Option<String>,
}

#[derive(Debug, Args)]
struct LogArgs {
    #[arg(short = 'i', long = "id", value_name = "ID")]
    id: LogPageId,

    #[arg(short = 'n', long = "lines", default_value_t = 100, value_parser = parse_tail_lines)]
    lines: usize,

    #[arg(
        long,
        help = "Strip ANSI/control codes from output; leave stored logs and matching unchanged"
    )]
    clean: bool,

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

    #[arg(long = "source-field", value_delimiter = ',', value_parser = parse_source_field)]
    source_fields: Vec<String>,
}

#[derive(Debug, Args)]
struct SourcesArgs {
    #[arg(short = 'i', long = "id", value_name = "ID")]
    id: LogPageId,

    #[arg(long = "source-field", value_delimiter = ',', value_parser = parse_source_field)]
    source_fields: Vec<String>,
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

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let runtime_command = match cli.subcommand {
        Some(CliCommand::Log(args)) => return report_command(run_log_command(args)),
        Some(CliCommand::Pages) => return report_command(run_pages_command()),
        Some(CliCommand::Sources(args)) => {
            return report_command(print_log_page_sources(
                &args.id,
                SourceConfig::with_fields(args.source_fields),
                &mut io::stdout().lock(),
            ));
        }
        Some(CliCommand::Runtime(command)) => Some(command),
        None => None,
    };

    let resolved_input = match runtime_input_for_command(runtime_command, cli.command) {
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
            eprintln!("{USAGE}");
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

fn run_log_command(args: LogArgs) -> Result<(), LogPageError> {
    let options = LogPageTailOptions {
        line_count: args.lines,
        clean: args.clean,
        source: args.source,
        text: args.text,
        property_filters: args.property_filters,
        source_config: SourceConfig::with_fields(args.source_fields),
    };

    let mut stdout = io::stdout().lock();
    print_log_page_tail_with_options(&args.id, &options, &mut stdout)
}

fn run_pages_command() -> Result<(), LogPageError> {
    let pages = active_log_pages()?;
    let mut stdout = io::stdout().lock();
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

fn runtime_input_for_command(
    runtime_command: Option<RuntimeCommand>,
    command: Vec<String>,
) -> Result<ResolvedRuntimeInput, String> {
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("could not read current directory: {error}"))?;
    let config_env = ConfigEnv::from_env();

    runtime_input_for_command_with_context(runtime_command, command, &current_dir, &config_env)
}

fn runtime_input_for_command_with_context(
    runtime_command: Option<RuntimeCommand>,
    command: Vec<String>,
    current_dir: &Path,
    config_env: &ConfigEnv,
) -> Result<ResolvedRuntimeInput, String> {
    match runtime_command {
        Some(RuntimeCommand::Run(args)) => parse_runner_commands(&args.groups)
            .map(RuntimeInput::Commands)
            .map(ResolvedRuntimeInput::new),
        Some(RuntimeCommand::Start(args)) => {
            parse_start_command(args.name.as_deref(), current_dir, config_env)
        }
        None if command.is_empty() => Ok(ResolvedRuntimeInput::new(RuntimeInput::Stdin)),
        None => Ok(ResolvedRuntimeInput::new(RuntimeInput::Command(
            command_for_runtime(command),
        ))),
    }
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
    name: Option<&str>,
    current_dir: &Path,
    config_env: &ConfigEnv,
) -> Result<ResolvedRuntimeInput, String> {
    let config = if let Some(name) = name {
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
    use clap::error::ErrorKind;
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

    fn try_parse_cli(raw_args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("loggle").chain(raw_args.iter().copied()))
    }

    fn parse_cli(raw_args: &[&str]) -> Cli {
        try_parse_cli(raw_args).unwrap()
    }

    fn runtime_command(subcommand: Option<CliCommand>) -> Option<RuntimeCommand> {
        match subcommand {
            None => None,
            Some(CliCommand::Runtime(command)) => Some(command),
            Some(other) => panic!("expected a runtime subcommand, got {other:?}"),
        }
    }

    fn resolve_with_context(
        raw_args: &[&str],
        current_dir: &Path,
        config_env: &ConfigEnv,
    ) -> Result<ResolvedRuntimeInput, String> {
        let cli = parse_cli(raw_args);
        runtime_input_for_command_with_context(
            runtime_command(cli.subcommand),
            cli.command,
            current_dir,
            config_env,
        )
    }

    fn resolve(raw_args: &[&str]) -> Result<ResolvedRuntimeInput, String> {
        let cli = parse_cli(raw_args);
        runtime_input_for_command(runtime_command(cli.subcommand), cli.command)
    }

    fn runtime_input(raw_args: &[&str]) -> RuntimeInput {
        resolve(raw_args).unwrap().input
    }

    fn log_args(raw_args: &[&str]) -> LogArgs {
        match parse_cli(raw_args).subcommand {
            Some(CliCommand::Log(args)) => args,
            other => panic!("expected log subcommand, got {other:?}"),
        }
    }

    fn assert_help(raw_args: &[&str]) {
        let error = try_parse_cli(raw_args).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DisplayHelp, "{raw_args:?}");
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
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn dc_expands_to_docker_compose_up() {
        assert_eq!(
            runtime_input(&["dc"]),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn dc_with_arguments_is_not_a_compose_shortcut() {
        assert_eq!(
            runtime_input(&["dc", "logs", "-f"]),
            RuntimeInput::Command(command(&["dc", "logs", "-f"]))
        );
    }

    #[test]
    fn ordinary_commands_are_unchanged() {
        assert_eq!(
            runtime_input(&["docker", "compose", "logs", "-f"]),
            RuntimeInput::Command(command(&["docker", "compose", "logs", "-f"]))
        );
    }

    #[test]
    fn empty_command_reads_from_stdin() {
        assert_eq!(runtime_input(&[]), RuntimeInput::Stdin);
    }

    #[test]
    fn cli_source_fields_are_checked_before_config_source_fields() {
        assert_eq!(
            merged_source_fields(command(&["logger"]), command(&["service", "logger"])),
            command(&["logger", "service", "logger"])
        );
    }

    #[test]
    fn runner_cli_parses_two_named_commands() {
        assert_eq!(
            runtime_input(&[
                "run", "--name", "api", "--", "pnpm", "start", "--name", "web", "--", "pnpm",
                "dev",
            ]),
            RuntimeInput::Commands(vec![
                named_command("api", &["pnpm", "start"]),
                named_command("web", &["pnpm", "dev"]),
            ])
        );
    }

    #[test]
    fn runner_cli_keeps_hyphenated_command_arguments() {
        assert_eq!(
            runtime_input(&["run", "--name", "api", "--", "pnpm", "--help", "-x"]),
            RuntimeInput::Commands(vec![named_command("api", &["pnpm", "--help", "-x"])])
        );
    }

    #[test]
    fn runner_cli_preserves_command_arguments_after_top_level_separator() {
        assert_eq!(
            runtime_input(&["--", "docker", "compose", "up", "--watch"]),
            RuntimeInput::Command(command(&["docker", "compose", "up", "--watch"]))
        );
    }

    #[test]
    fn record_option_does_not_consume_runtime_command() {
        assert_eq!(
            runtime_input(&["--record", "session.log", "docker", "compose", "up"]),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn page_id_option_does_not_consume_runtime_command() {
        let cli = parse_cli(&["--id", "1", "docker", "compose", "up"]);

        assert_eq!(cli.page_id.as_ref().unwrap().as_str(), "1");
        assert_eq!(
            runtime_input(&["--id", "1", "docker", "compose", "up"]),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn short_page_id_option_does_not_consume_runtime_command() {
        let cli = parse_cli(&["-i", "1", "docker", "compose", "up"]);

        assert_eq!(cli.page_id.as_ref().unwrap().as_str(), "1");
        assert_eq!(
            runtime_input(&["-i", "1", "docker", "compose", "up"]),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn no_page_log_flag_does_not_consume_runtime_command() {
        let cli = parse_cli(&["--no-page-log", "docker", "compose", "up"]);

        assert!(cli.no_page_log);
        assert_eq!(
            runtime_input(&["--no-page-log", "docker", "compose", "up"]),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn global_flags_parse_before_a_subcommand() {
        let cli = parse_cli(&[
            "--id",
            "api",
            "--no-color",
            "--record",
            "session.log",
            "--buffer-lines",
            "500",
            "--source-field",
            "service",
            "--no-page-log",
            "start",
            "libre",
        ]);

        assert_eq!(cli.page_id.as_ref().unwrap().as_str(), "api");
        assert!(cli.no_color);
        assert!(cli.no_page_log);
        assert_eq!(cli.buffer_lines, 500);
        assert_eq!(cli.record, Some(std::path::PathBuf::from("session.log")));
        assert_eq!(cli.source_fields, command(&["service"]));
        assert!(cli.command.is_empty());
        let Some(CliCommand::Runtime(RuntimeCommand::Start(args))) = cli.subcommand else {
            panic!("expected start subcommand");
        };
        assert_eq!(args.name.as_deref(), Some("libre"));

        assert_eq!(
            runtime_input(&["-i", "api", "run", "--name", "api", "--", "pnpm", "start"]),
            RuntimeInput::Commands(vec![named_command("api", &["pnpm", "start"])])
        );
    }

    #[test]
    fn page_id_with_separator_runs_command() {
        let cli = parse_cli(&["--id", "api", "--", "docker", "compose", "up"]);

        assert_eq!(cli.page_id.as_ref().unwrap().as_str(), "api");
        assert!(cli.subcommand.is_none());
        assert_eq!(cli.command, command(&["docker", "compose", "up"]));
    }

    #[test]
    fn subcommands_have_help() {
        assert_help(&["--help"]);
        assert_help(&["run", "--help"]);
        assert_help(&["start", "--help"]);
        assert_help(&["log", "--help"]);
        assert_help(&["pages", "--help"]);
        assert_help(&["sources", "--help"]);
    }

    #[test]
    fn help_word_is_a_bare_command() {
        assert_eq!(
            runtime_input(&["help", "me"]),
            RuntimeInput::Command(command(&["help", "me"]))
        );
    }

    #[test]
    fn log_command_cli_parses_tail_request() {
        let args = log_args(&[
            "log",
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
            "--clean",
        ]);

        assert!(args.clean);
        assert_eq!(args.id.as_str(), "1");
        assert_eq!(args.lines, 5);
        assert_eq!(args.source.as_deref(), Some("api"));
        assert_eq!(args.text.as_deref(), Some("database"));
        assert_eq!(args.property_filters, command(&["tenantId=tenant-1"]));
        assert_eq!(args.source_fields, command(&["service"]));
    }

    #[test]
    fn pages_command_cli_parses() {
        assert!(matches!(
            parse_cli(&["pages"]).subcommand,
            Some(CliCommand::Pages)
        ));
    }

    #[test]
    fn source_discovery_requires_page_and_supports_custom_source_fields() {
        assert!(try_parse_cli(&["sources"]).is_err());
        let Some(CliCommand::Sources(args)) =
            parse_cli(&["sources", "-i", "vev", "--source-field", "unit,logger"]).subcommand
        else {
            panic!("expected sources subcommand");
        };
        assert_eq!(args.id.as_str(), "vev");
        assert_eq!(args.source_fields, command(&["unit", "logger"]));
        assert!(!log_args(&["log", "-i", "vev"]).clean);
        let cli = parse_cli(&["--", "sources", "--help"]);
        assert!(cli.subcommand.is_none());
        assert_eq!(cli.command, command(&["sources", "--help"]));
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
    fn runner_rejects_no_commands() {
        assert_eq!(
            resolve(&["run"]).unwrap_err(),
            "runner mode requires at least one --name <name> -- <command>"
        );
    }

    #[test]
    fn runner_rejects_missing_name() {
        assert_eq!(
            resolve(&["run", "api", "--", "pnpm", "start"]).unwrap_err(),
            "runner commands must start with --name <name> -- <command>"
        );
    }

    #[test]
    fn runner_rejects_empty_command() {
        assert_eq!(
            resolve(&["run", "--name", "api", "--"]).unwrap_err(),
            "runner command 'api' is empty"
        );
    }

    #[test]
    fn runner_rejects_missing_command_separator() {
        assert_eq!(
            resolve(&["run", "--name", "api", "pnpm", "start"]).unwrap_err(),
            "runner command 'api' must include -- before the command"
        );
    }

    #[test]
    fn start_without_name_loads_project_config() {
        let project_dir = temp_dir("project");
        let root = project_dir.join("workspace");
        fs::create_dir_all(&root).unwrap();
        write_config(&project_dir.join(".loggle.toml"), &root);

        let resolved = resolve_with_context(
            &["start"],
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
        let error = resolve_with_context(
            &["start"],
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

        let resolved = resolve_with_context(
            &["start", "libre"],
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
        let error = try_parse_cli(&["start", "libre", "extra"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }
}
