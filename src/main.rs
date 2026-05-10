use std::error::Error;

use clap::Parser;
use loggle::{RuntimeConfig, RuntimeError, run};

#[derive(Debug, Parser)]
#[command(
    name = "loggle",
    about = "A terminal log viewer for piped Docker Compose-style logs."
)]
struct Cli {
    #[arg(long, default_value_t = 100_000, value_parser = parse_buffer_lines)]
    buffer_lines: usize,

    #[arg(long)]
    no_color: bool,

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

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match run(RuntimeConfig {
        buffer_lines: cli.buffer_lines,
        color_enabled: !cli.no_color,
        command: command_for_runtime(cli.command),
    }) {
        Ok(()) => Ok(()),
        Err(RuntimeError::MissingInput) => {
            eprintln!("{}", RuntimeError::MissingInput);
            std::process::exit(1);
        }
        Err(error) => Err(Box::new(error)),
    }
}

fn command_for_runtime(command: Vec<String>) -> Vec<String> {
    if command == ["dc"] {
        vec!["docker".into(), "compose".into(), "up".into()]
    } else {
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn dc_expands_to_docker_compose_up() {
        assert_eq!(
            command_for_runtime(command(&["dc"])),
            command(&["docker", "compose", "up"])
        );
    }

    #[test]
    fn dc_with_arguments_is_not_a_compose_shortcut() {
        assert_eq!(
            command_for_runtime(command(&["dc", "logs", "-f"])),
            command(&["dc", "logs", "-f"])
        );
    }

    #[test]
    fn ordinary_commands_are_unchanged() {
        assert_eq!(
            command_for_runtime(command(&["docker", "compose", "logs", "-f"])),
            command(&["docker", "compose", "logs", "-f"])
        );
    }
}
