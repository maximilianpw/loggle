use std::error::Error;

use clap::Parser;
use loggle::{NamedCommand, RuntimeConfig, RuntimeError, RuntimeInput, SourceConfig, run};

#[derive(Debug, Parser)]
#[command(
    name = "loggle",
    about = "A terminal log viewer for piped Docker Compose-style logs.",
    dont_delimit_trailing_values = true
)]
struct Cli {
    #[arg(long, default_value_t = 100_000, value_parser = parse_buffer_lines)]
    buffer_lines: usize,

    #[arg(long)]
    no_color: bool,

    #[arg(long = "source-field", value_delimiter = ',', value_parser = parse_source_field)]
    source_fields: Vec<String>,

    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Command to run under loggle; use dc as a shortcut for docker compose up"
    )]
    command: Vec<String>,
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

fn parse_source_field(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        Err("source field must not be empty".to_string())
    } else {
        Ok(input.to_string())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let raw_args = std::env::args().skip(1).collect::<Vec<_>>();
    let cli = Cli::parse();
    let input = match runtime_input_for_command(command_tail_from_args(&raw_args, &cli.command)) {
        Ok(input) => input,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };

    match run(RuntimeConfig {
        buffer_lines: cli.buffer_lines,
        color_enabled: !cli.no_color,
        source_config: SourceConfig::with_fields(cli.source_fields),
        input,
    }) {
        Ok(()) => Ok(()),
        Err(RuntimeError::MissingInput) => {
            eprintln!("{}", RuntimeError::MissingInput);
            std::process::exit(1);
        }
        Err(error) => Err(Box::new(error)),
    }
}

fn command_tail_from_args(raw_args: &[String], clap_command: &[String]) -> Vec<String> {
    let mut index = 0;
    while index < raw_args.len() {
        let value = raw_args[index].as_str();
        match value {
            "--" => return raw_args[index + 1..].to_vec(),
            "--buffer-lines" | "--source-field" => {
                index += 2;
            }
            "--no-color" => {
                index += 1;
            }
            value if value.starts_with("--buffer-lines=") => {
                index += 1;
            }
            value if value.starts_with("--source-field=") => {
                index += 1;
            }
            value if value.starts_with('-') => {
                return clap_command.to_vec();
            }
            _ => return raw_args[index..].to_vec(),
        }
    }

    clap_command.to_vec()
}

fn runtime_input_for_command(command: Vec<String>) -> Result<RuntimeInput, String> {
    if command.is_empty() {
        return Ok(RuntimeInput::Stdin);
    }

    if command[0] == "run" {
        return parse_runner_commands(&command[1..]).map(RuntimeInput::Commands);
    }

    Ok(RuntimeInput::Command(command_for_runtime(command)))
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
        });
    }

    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn named_command(name: &str, values: &[&str]) -> NamedCommand {
        NamedCommand {
            name: name.to_string(),
            command: command(values),
        }
    }

    #[test]
    fn dc_expands_to_docker_compose_up() {
        assert_eq!(
            runtime_input_for_command(command(&["dc"])).unwrap(),
            RuntimeInput::Command(command(&["docker", "compose", "up"]))
        );
    }

    #[test]
    fn dc_with_arguments_is_not_a_compose_shortcut() {
        assert_eq!(
            runtime_input_for_command(command(&["dc", "logs", "-f"])).unwrap(),
            RuntimeInput::Command(command(&["dc", "logs", "-f"]))
        );
    }

    #[test]
    fn ordinary_commands_are_unchanged() {
        assert_eq!(
            runtime_input_for_command(command(&["docker", "compose", "logs", "-f"])).unwrap(),
            RuntimeInput::Command(command(&["docker", "compose", "logs", "-f"]))
        );
    }

    #[test]
    fn empty_command_reads_from_stdin() {
        assert_eq!(
            runtime_input_for_command(Vec::new()).unwrap(),
            RuntimeInput::Stdin
        );
    }

    #[test]
    fn runner_cli_parses_two_named_commands() {
        let raw_args = command(&[
            "run", "--name", "api", "--", "pnpm", "start", "--name", "web", "--", "pnpm", "dev",
        ]);
        let cli = Cli::try_parse_from(
            std::iter::once("loggle".to_string()).chain(raw_args.iter().cloned()),
        )
        .unwrap();

        assert_eq!(
            runtime_input_for_command(command_tail_from_args(&raw_args, &cli.command)).unwrap(),
            RuntimeInput::Commands(vec![
                named_command("api", &["pnpm", "start"]),
                named_command("web", &["pnpm", "dev"]),
            ])
        );
    }

    #[test]
    fn runner_cli_preserves_command_arguments_after_top_level_separator() {
        let raw_args = command(&["--", "docker", "compose", "up", "--watch"]);
        let cli = Cli::try_parse_from(
            std::iter::once("loggle".to_string()).chain(raw_args.iter().cloned()),
        )
        .unwrap();

        assert_eq!(
            runtime_input_for_command(command_tail_from_args(&raw_args, &cli.command)).unwrap(),
            RuntimeInput::Command(command(&["docker", "compose", "up", "--watch"]))
        );
    }

    #[test]
    fn runner_rejects_missing_name() {
        assert_eq!(
            runtime_input_for_command(command(&["run", "api", "--", "pnpm", "start"]))
                .unwrap_err(),
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
}
