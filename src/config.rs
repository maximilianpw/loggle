use std::{
    collections::BTreeMap,
    env, fmt, fs, io,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use crate::runtime::{ReadySpec, StartCommand};

const PROJECT_CONFIG_FILE: &str = ".loggle.toml";
const CONFIG_DIR_NAME: &str = "loggle";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartConfig {
    pub root: PathBuf,
    pub source_fields: Vec<String>,
    pub commands: Vec<StartCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEnv {
    pub xdg_config_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl ConfigEnv {
    pub fn from_env() -> Self {
        Self {
            xdg_config_home: env_path("XDG_CONFIG_HOME"),
            home: env_path("HOME"),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    InvalidName { name: String },
    MissingConfigDirectory,
    MissingFile(PathBuf),
    Read { path: PathBuf, source: io::Error },
    Parse {
        path: Option<PathBuf>,
        source: toml::de::Error,
    },
    Validation {
        path: Option<PathBuf>,
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => write!(
                f,
                "invalid config name '{name}': named configs must be simple names without path separators"
            ),
            Self::MissingConfigDirectory => {
                f.write_str("could not resolve config directory: set XDG_CONFIG_HOME or HOME")
            }
            Self::MissingFile(path) => write!(f, "config file not found: {}", path.display()),
            Self::Read { path, source } => {
                write!(f, "failed to read config {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                if let Some(path) = path {
                    write!(f, "failed to parse config {}: {source}", path.display())
                } else {
                    write!(f, "failed to parse config: {source}")
                }
            }
            Self::Validation { path, message } => {
                if let Some(path) = path {
                    write!(f, "invalid config {}: {message}", path.display())
                } else {
                    write!(f, "invalid config: {message}")
                }
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn project_config_path(current_dir: &Path) -> PathBuf {
    current_dir.join(PROJECT_CONFIG_FILE)
}

pub fn named_config_path(name: &str, config_env: &ConfigEnv) -> Result<PathBuf, ConfigError> {
    let name = validate_config_name(name)?;
    let file_name = format!("{name}.toml");

    if let Some(config_home) = config_env
        .xdg_config_home
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        return Ok(config_home.join(CONFIG_DIR_NAME).join(file_name));
    }

    let Some(home) = config_env
        .home
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    else {
        return Err(ConfigError::MissingConfigDirectory);
    };

    Ok(home.join(".config").join(CONFIG_DIR_NAME).join(file_name))
}

pub fn load_project_config(current_dir: &Path) -> Result<StartConfig, ConfigError> {
    load_config_file(&project_config_path(current_dir))
}

pub fn load_named_config(name: &str, config_env: &ConfigEnv) -> Result<StartConfig, ConfigError> {
    let path = named_config_path(name, config_env)?;
    load_config_file(&path)
}

pub fn load_config_file(path: &Path) -> Result<StartConfig, ConfigError> {
    let input = fs::read_to_string(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            ConfigError::MissingFile(path.to_path_buf())
        } else {
            ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;

    parse_config_with_path(&input, Some(path.to_path_buf()))
}

pub fn parse_config(input: &str) -> Result<StartConfig, ConfigError> {
    parse_config_with_path(input, None)
}

fn parse_config_with_path(
    input: &str,
    path: Option<PathBuf>,
) -> Result<StartConfig, ConfigError> {
    let raw = toml::from_str::<RawConfig>(input).map_err(|source| ConfigError::Parse {
        path: path.clone(),
        source,
    })?;

    validate_config(raw, path)
}

fn validate_config(
    raw: RawConfig,
    path: Option<PathBuf>,
) -> Result<StartConfig, ConfigError> {
    let root = raw
        .root
        .ok_or_else(|| validation_error(path.clone(), "missing required field `root`"))?;
    if root.as_os_str().is_empty() {
        return Err(validation_error(path, "`root` must not be empty"));
    }

    let source_fields = raw
        .source_fields
        .into_iter()
        .map(|field| field.trim().to_string())
        .map(|field| {
            if field.is_empty() {
                Err(validation_error(
                    path.clone(),
                    "`source_fields` must not contain empty field names",
                ))
            } else {
                Ok(field)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    let env = raw.env;
    validate_env("top-level env", &env, path.clone())?;

    let commands = raw
        .commands
        .ok_or_else(|| validation_error(path.clone(), "missing required table `[commands]`"))?;
    if commands.is_empty() {
        return Err(validation_error(
            path,
            "`[commands]` must include at least one command",
        ));
    }

    let commands = commands
        .into_iter()
        .map(|(name, command)| {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(validation_error(path.clone(), "command names must not be empty"));
            }

            let command = validate_command(name, command, &root, &env, path.clone())?;
            Ok(command)
        })
        .collect::<Result<Vec<_>, _>>()?;

    validate_dependency_graph(&commands, path.clone())?;

    Ok(StartConfig {
        root,
        source_fields,
        commands,
    })
}

fn validation_error(path: Option<PathBuf>, message: impl Into<String>) -> ConfigError {
    ConfigError::Validation {
        path,
        message: message.into(),
    }
}

fn validate_command(
    name: String,
    command: RawCommand,
    root: &Path,
    config_env: &BTreeMap<String, String>,
    path: Option<PathBuf>,
) -> Result<StartCommand, ConfigError> {
    let RawCommandParts {
        argv,
        argv_field,
        env,
        wait_for,
        ready,
    } = match command {
        RawCommand::Simple(argv) => RawCommandParts {
            argv,
            argv_field: None,
            env: BTreeMap::new(),
            wait_for: Vec::new(),
            ready: None,
        },
        RawCommand::Advanced(command) => RawCommandParts {
            argv: command.argv.ok_or_else(|| {
                validation_error(
                    path.clone(),
                    format!("command '{name}' advanced form must include `argv`"),
                )
            })?,
            argv_field: Some("argv"),
            env: command.env,
            wait_for: command.wait_for,
            ready: command.ready,
        },
    };

    validate_argv(&name, &argv, argv_field, path.clone())?;
    validate_env(&format!("command '{name}' env"), &env, path.clone())?;
    let env = merged_env(config_env, env);

    let wait_for = wait_for
        .into_iter()
        .map(|dependency| dependency.trim().to_string())
        .map(|dependency| {
            if dependency.is_empty() {
                Err(validation_error(
                    path.clone(),
                    format!("command '{name}' wait_for must not contain empty command names"),
                ))
            } else {
                Ok(dependency)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    let ready = ready
        .map(|ready| validate_ready(&name, ready, path.clone()))
        .transpose()?;

    Ok(StartCommand {
        name,
        argv,
        cwd: Some(root.to_path_buf()),
        env,
        wait_for,
        ready,
    })
}

struct RawCommandParts {
    argv: Vec<String>,
    argv_field: Option<&'static str>,
    env: BTreeMap<String, String>,
    wait_for: Vec<String>,
    ready: Option<RawReady>,
}

fn validate_argv(
    command_name: &str,
    argv: &[String],
    field_name: Option<&str>,
    path: Option<PathBuf>,
) -> Result<(), ConfigError> {
    if argv.is_empty() {
        let field_name = field_name
            .map(|field_name| format!(" {field_name}"))
            .unwrap_or_default();
        return Err(validation_error(
            path,
            format!("command '{command_name}'{field_name} must not be empty"),
        ));
    }
    if argv[0].trim().is_empty() {
        return Err(validation_error(
            path,
            format!("command '{command_name}' executable must not be empty"),
        ));
    }

    Ok(())
}

fn validate_ready(
    command_name: &str,
    ready: RawReady,
    path: Option<PathBuf>,
) -> Result<ReadySpec, ConfigError> {
    let strategy_count = usize::from(ready.line.is_some()) + usize::from(ready.command.is_some());
    if strategy_count != 1 {
        return Err(validation_error(
            path,
            format!(
                "command '{command_name}' ready must include exactly one of `line` or `command`"
            ),
        ));
    }

    let timeout = duration_ms(
        command_name,
        "ready.timeout_ms",
        ready.timeout_ms.unwrap_or(30_000),
        path.clone(),
    )?;

    if let Some(line) = ready.line {
        let line = line.trim().to_string();
        if line.is_empty() {
            return Err(validation_error(
                path,
                format!("command '{command_name}' ready.line must not be empty"),
            ));
        }

        return Ok(ReadySpec::Line {
            text: line,
            timeout,
        });
    }

    let command = ready.command.unwrap_or_default();
    validate_argv(command_name, &command, Some("ready.command"), path.clone())?;
    let interval = duration_ms(command_name, "ready.ms", ready.ms.unwrap_or(500), path)?;

    Ok(ReadySpec::Command {
        command,
        interval,
        timeout,
    })
}

fn duration_ms(
    command_name: &str,
    field_name: &str,
    value: u64,
    path: Option<PathBuf>,
) -> Result<Duration, ConfigError> {
    if value == 0 {
        Err(validation_error(
            path,
            format!("command '{command_name}' {field_name} must be greater than zero"),
        ))
    } else {
        Ok(Duration::from_millis(value))
    }
}

fn validate_env(
    label: &str,
    env: &BTreeMap<String, String>,
    path: Option<PathBuf>,
) -> Result<(), ConfigError> {
    for (name, value) in env {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err(validation_error(
                path,
                format!("{label} contains invalid variable name '{name}'"),
            ));
        }
        if value.contains('\0') {
            return Err(validation_error(
                path,
                format!("{label} variable '{name}' contains a null byte"),
            ));
        }
    }

    Ok(())
}

fn merged_env(
    config_env: &BTreeMap<String, String>,
    command_env: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = config_env.clone();
    env.extend(command_env);
    env
}

fn validate_dependency_graph(
    commands: &[StartCommand],
    path: Option<PathBuf>,
) -> Result<(), ConfigError> {
    let command_indexes = commands
        .iter()
        .enumerate()
        .map(|(index, command)| (command.name.as_str(), index))
        .collect::<BTreeMap<_, _>>();

    for command in commands {
        for dependency in &command.wait_for {
            if !command_indexes.contains_key(dependency.as_str()) {
                return Err(validation_error(
                    path,
                    format!(
                        "command '{}' waits for unknown command '{}'",
                        command.name, dependency
                    ),
                ));
            }
        }
    }

    let mut states = vec![VisitState::Unvisited; commands.len()];
    let mut stack = Vec::new();
    for index in 0..commands.len() {
        visit_dependencies(
            index,
            commands,
            &command_indexes,
            &mut states,
            &mut stack,
            path.clone(),
        )?;
    }

    Ok(())
}

fn visit_dependencies(
    index: usize,
    commands: &[StartCommand],
    command_indexes: &BTreeMap<&str, usize>,
    states: &mut [VisitState],
    stack: &mut Vec<usize>,
    path: Option<PathBuf>,
) -> Result<(), ConfigError> {
    match states[index] {
        VisitState::Visited => return Ok(()),
        VisitState::Visiting => {
            return Err(validation_error(
                path,
                format_cycle(commands, stack, index),
            ));
        }
        VisitState::Unvisited => {}
    }

    states[index] = VisitState::Visiting;
    stack.push(index);

    for dependency in &commands[index].wait_for {
        let dependency_index = command_indexes[dependency.as_str()];
        visit_dependencies(
            dependency_index,
            commands,
            command_indexes,
            states,
            stack,
            path.clone(),
        )?;
    }

    stack.pop();
    states[index] = VisitState::Visited;
    Ok(())
}

fn format_cycle(commands: &[StartCommand], stack: &[usize], repeated_index: usize) -> String {
    let start = stack
        .iter()
        .position(|index| *index == repeated_index)
        .unwrap_or(0);
    let mut names = stack[start..]
        .iter()
        .map(|index| commands[*index].name.as_str())
        .collect::<Vec<_>>();
    names.push(commands[repeated_index].name.as_str());

    format!("command dependency cycle detected: {}", names.join(" -> "))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Unvisited,
    Visiting,
    Visited,
}

fn validate_config_name(name: &str) -> Result<&str, ConfigError> {
    let name = name.trim();
    let mut components = Path::new(name).components();
    let is_simple_name = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none();

    if is_simple_name {
        Ok(name)
    } else {
        Err(ConfigError::InvalidName {
            name: name.to_string(),
        })
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    root: Option<PathBuf>,
    #[serde(default)]
    source_fields: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    commands: Option<BTreeMap<String, RawCommand>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawCommand {
    Simple(Vec<String>),
    Advanced(RawAdvancedCommand),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAdvancedCommand {
    argv: Option<Vec<String>>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    wait_for: Vec<String>,
    ready: Option<RawReady>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReady {
    line: Option<String>,
    command: Option<Vec<String>>,
    ms: Option<u64>,
    timeout_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn env(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn parses_start_config() {
        let config = parse_config(
            r#"
root = "/Users/max-vev/Local/librestock"
source_fields = ["service", "app", "logger"]

[commands]
api = ["pnpm", "--filter", "api", "dev"]
web = ["pnpm", "--filter", "web", "dev"]
"#,
        )
        .unwrap();

        assert_eq!(
            config.root,
            PathBuf::from("/Users/max-vev/Local/librestock")
        );
        assert_eq!(config.source_fields, command(&["service", "app", "logger"]));
        assert_eq!(
            config.commands,
            vec![
                StartCommand {
                    name: "api".to_string(),
                    argv: command(&["pnpm", "--filter", "api", "dev"]),
                    cwd: Some(PathBuf::from("/Users/max-vev/Local/librestock")),
                    env: BTreeMap::new(),
                    wait_for: Vec::new(),
                    ready: None,
                },
                StartCommand {
                    name: "web".to_string(),
                    argv: command(&["pnpm", "--filter", "web", "dev"]),
                    cwd: Some(PathBuf::from("/Users/max-vev/Local/librestock")),
                    env: BTreeMap::new(),
                    wait_for: Vec::new(),
                    ready: None,
                },
            ]
        );
    }

    #[test]
    fn parses_top_level_and_command_env() {
        let config = parse_config(
            r#"
root = "/tmp/project"
env = { NODE_ENV = "development", SHARED = "top" }

[commands.api]
argv = ["pnpm", "start"]
env = { DATABASE_URL = "postgres://localhost/db", SHARED = "command" }

[commands.web]
argv = ["pnpm", "dev"]
"#,
        )
        .unwrap();

        assert_eq!(
            config.commands[0].env,
            env(&[
                ("DATABASE_URL", "postgres://localhost/db"),
                ("NODE_ENV", "development"),
                ("SHARED", "command"),
            ])
        );
        assert_eq!(
            config.commands[1].env,
            env(&[("NODE_ENV", "development"), ("SHARED", "top")])
        );
    }

    #[test]
    fn parses_advanced_start_config_with_readiness_dependencies() {
        let config = parse_config(
            r#"
root = "/tmp/project"

[commands.db]
argv = ["docker", "compose", "up", "postgres"]

[commands.db.ready]
command = ["docker", "compose", "exec", "-T", "postgres", "pg_isready"]
ms = 250
timeout_ms = 1000

[commands.api]
argv = ["pnpm", "start"]
wait_for = ["db"]
ready = { line = "ready", timeout_ms = 2000 }
"#,
        )
        .unwrap();

        assert_eq!(
            config.commands,
            vec![
                StartCommand {
                    name: "api".to_string(),
                    argv: command(&["pnpm", "start"]),
                    cwd: Some(PathBuf::from("/tmp/project")),
                    env: BTreeMap::new(),
                    wait_for: command(&["db"]),
                    ready: Some(ReadySpec::Line {
                        text: "ready".to_string(),
                        timeout: Duration::from_millis(2000),
                    }),
                },
                StartCommand {
                    name: "db".to_string(),
                    argv: command(&["docker", "compose", "up", "postgres"]),
                    cwd: Some(PathBuf::from("/tmp/project")),
                    env: BTreeMap::new(),
                    wait_for: Vec::new(),
                    ready: Some(ReadySpec::Command {
                        command: command(&[
                            "docker",
                            "compose",
                            "exec",
                            "-T",
                            "postgres",
                            "pg_isready",
                        ]),
                        interval: Duration::from_millis(250),
                        timeout: Duration::from_millis(1000),
                    }),
                },
            ]
        );
    }

    #[test]
    fn applies_readiness_defaults() {
        let config = parse_config(
            r#"
root = "/tmp/project"

[commands.api]
argv = ["pnpm", "start"]
ready = { command = ["true"] }
"#,
        )
        .unwrap();

        assert_eq!(
            config.commands[0].ready,
            Some(ReadySpec::Command {
                command: command(&["true"]),
                interval: Duration::from_millis(500),
                timeout: Duration::from_millis(30_000),
            })
        );
    }

    #[test]
    fn rejects_missing_root() {
        assert_eq!(
            parse_config("[commands]\napi = [\"pnpm\", \"dev\"]")
                .unwrap_err()
                .to_string(),
            "invalid config: missing required field `root`"
        );
    }

    #[test]
    fn rejects_missing_commands_table() {
        assert_eq!(
            parse_config("root = \"/tmp\"").unwrap_err().to_string(),
            "invalid config: missing required table `[commands]`"
        );
    }

    #[test]
    fn rejects_empty_commands_table() {
        assert_eq!(
            parse_config("root = \"/tmp\"\n[commands]")
                .unwrap_err()
                .to_string(),
            "invalid config: `[commands]` must include at least one command"
        );
    }

    #[test]
    fn rejects_empty_command_arrays() {
        assert_eq!(
            parse_config("root = \"/tmp\"\n[commands]\napi = []")
                .unwrap_err()
                .to_string(),
            "invalid config: command 'api' must not be empty"
        );
    }

    #[test]
    fn rejects_empty_advanced_argv() {
        assert_eq!(
            parse_config("root = \"/tmp\"\n[commands.api]\nargv = []")
                .unwrap_err()
                .to_string(),
            "invalid config: command 'api' argv must not be empty"
        );
    }

    #[test]
    fn rejects_invalid_top_level_env_name() {
        assert_eq!(
            parse_config(
                r#"
root = "/tmp"
env = { "BAD=NAME" = "value" }

[commands]
api = ["pnpm", "start"]
"#
            )
            .unwrap_err()
            .to_string(),
            "invalid config: top-level env contains invalid variable name 'BAD=NAME'"
        );
    }

    #[test]
    fn rejects_invalid_command_env_name() {
        assert_eq!(
            parse_config(
                r#"
root = "/tmp"

[commands.api]
argv = ["pnpm", "start"]
env = { "BAD=NAME" = "value" }
"#
            )
            .unwrap_err()
            .to_string(),
            "invalid config: command 'api' env contains invalid variable name 'BAD=NAME'"
        );
    }

    #[test]
    fn rejects_ready_without_strategy() {
        assert_eq!(
            parse_config(
                r#"
root = "/tmp"

[commands.api]
argv = ["pnpm", "start"]
ready = { timeout_ms = 1000 }
"#
            )
            .unwrap_err()
            .to_string(),
            "invalid config: command 'api' ready must include exactly one of `line` or `command`"
        );
    }

    #[test]
    fn rejects_ready_with_multiple_strategies() {
        assert_eq!(
            parse_config(
                r#"
root = "/tmp"

[commands.api]
argv = ["pnpm", "start"]
ready = { line = "ready", command = ["true"] }
"#
            )
            .unwrap_err()
            .to_string(),
            "invalid config: command 'api' ready must include exactly one of `line` or `command`"
        );
    }

    #[test]
    fn rejects_empty_ready_command() {
        assert_eq!(
            parse_config(
                r#"
root = "/tmp"

[commands.api]
argv = ["pnpm", "start"]
ready = { command = [] }
"#
            )
            .unwrap_err()
            .to_string(),
            "invalid config: command 'api' ready.command must not be empty"
        );
    }

    #[test]
    fn rejects_unknown_wait_target() {
        assert_eq!(
            parse_config(
                r#"
root = "/tmp"

[commands.api]
argv = ["pnpm", "start"]
wait_for = ["db"]
"#
            )
            .unwrap_err()
            .to_string(),
            "invalid config: command 'api' waits for unknown command 'db'"
        );
    }

    #[test]
    fn rejects_dependency_cycles() {
        assert_eq!(
            parse_config(
                r#"
root = "/tmp"

[commands.api]
argv = ["pnpm", "start"]
wait_for = ["web"]

[commands.web]
argv = ["pnpm", "dev"]
wait_for = ["api"]
"#
            )
            .unwrap_err()
            .to_string(),
            "invalid config: command dependency cycle detected: api -> web -> api"
        );
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(
            parse_config("root = ").unwrap_err().to_string().starts_with(
                "failed to parse config: TOML parse error"
            )
        );
    }

    #[test]
    fn named_config_path_uses_xdg_config_home_when_present() {
        let path = named_config_path(
            "libre",
            &ConfigEnv {
                xdg_config_home: Some(PathBuf::from("/xdg")),
                home: Some(PathBuf::from("/home/max")),
            },
        )
        .unwrap();

        assert_eq!(path, PathBuf::from("/xdg/loggle/libre.toml"));
    }

    #[test]
    fn named_config_path_falls_back_to_home_config() {
        let path = named_config_path(
            "libre",
            &ConfigEnv {
                xdg_config_home: None,
                home: Some(PathBuf::from("/home/max")),
            },
        )
        .unwrap();

        assert_eq!(path, PathBuf::from("/home/max/.config/loggle/libre.toml"));
    }
}
