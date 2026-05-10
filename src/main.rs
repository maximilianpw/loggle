use std::{error::Error, path::Path};

use clap::Parser;
use loggle::{
    ConfigEnv, NamedCommand, RuntimeConfig, RuntimeError, RuntimeInput, SourceConfig,
    load_named_config, load_project_config, run,
};

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
    let command_tail = command_tail_from_args(&raw_args, &cli.command);
    let resolved_input = match runtime_input_for_command(command_tail) {
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
        input: resolved_input.input,
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
        let path = std::env::temp_dir().join(format!(
            "loggle-cli-test-{}-{name}",
            std::process::id()
        ));
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
            runtime_input(command_tail_from_args(&raw_args, &cli.command)),
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
            runtime_input(command_tail_from_args(&raw_args, &cli.command)),
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
