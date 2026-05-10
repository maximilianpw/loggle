use std::{
    collections::BTreeMap,
    env, fmt, fs, io,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

use crate::runtime::NamedCommand;

const PROJECT_CONFIG_FILE: &str = ".loggle.toml";
const CONFIG_DIR_NAME: &str = "loggle";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartConfig {
    pub root: PathBuf,
    pub source_fields: Vec<String>,
    pub commands: Vec<NamedCommand>,
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
            if command.is_empty() {
                return Err(validation_error(
                    path.clone(),
                    format!("command '{name}' must not be empty"),
                ));
            }
            if command[0].trim().is_empty() {
                return Err(validation_error(
                    path.clone(),
                    format!("command '{name}' executable must not be empty"),
                ));
            }

            Ok(NamedCommand {
                name,
                command,
                cwd: Some(root.clone()),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

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
    commands: Option<BTreeMap<String, Vec<String>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
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
                NamedCommand {
                    name: "api".to_string(),
                    command: command(&["pnpm", "--filter", "api", "dev"]),
                    cwd: Some(PathBuf::from("/Users/max-vev/Local/librestock")),
                },
                NamedCommand {
                    name: "web".to_string(),
                    command: command(&["pnpm", "--filter", "web", "dev"]),
                    cwd: Some(PathBuf::from("/Users/max-vev/Local/librestock")),
                },
            ]
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
