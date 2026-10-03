mod clipboard;
mod input;
mod keys;
mod start_plan;
mod terminal;
#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::{collections::BTreeMap, fmt, io, path::PathBuf, sync::mpsc, time::Duration};

use crate::{model::SourceConfig, page_log::LogPageId};

use input::{Child, StartScheduler};
pub(crate) use start_plan::StartPlan;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub buffer_lines: usize,
    pub color_enabled: bool,
    pub source_config: SourceConfig,
    pub input: RuntimeInput,
    pub record_path: Option<PathBuf>,
    pub page_id: Option<LogPageId>,
    pub page_command: String,
    pub page_logging: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeInput {
    Stdin,
    Command(Vec<String>),
    Commands(Vec<NamedCommand>),
    StartCommands(Vec<StartCommand>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedCommand {
    pub name: String,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartCommand {
    pub name: String,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub wait_for: Vec<String>,
    pub ready: Option<ReadySpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadySpec {
    Line {
        text: String,
        timeout: Duration,
    },
    Command {
        command: Vec<String>,
        interval: Duration,
        timeout: Duration,
    },
}

#[derive(Debug)]
pub enum RuntimeError {
    MissingInput,
    Io(io::Error),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingInput => {
                f.write_str("no input: stdin is a terminal and no command was given")
            }
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingInput => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for RuntimeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn run(config: RuntimeConfig) -> Result<(), RuntimeError> {
    let (tx, rx) = mpsc::sync_channel(input::LINE_CHANNEL_CAPACITY);
    let started = start_input(&config.input, tx)?;

    terminal::run(rx, started.children, started.scheduler, config).map_err(RuntimeError::from)
}

struct StartedInput {
    children: Vec<Child>,
    /// `loggle start` spawns nothing up front: the event loop ticks the
    /// scheduler so the terminal is interactive while commands become ready.
    scheduler: Option<StartScheduler>,
}

fn start_input(
    input_mode: &RuntimeInput,
    tx: mpsc::SyncSender<String>,
) -> Result<StartedInput, RuntimeError> {
    match input_mode {
        RuntimeInput::Stdin => {
            if input::stdin_is_terminal() {
                return Err(RuntimeError::MissingInput);
            }

            input::spawn_stdin_reader(tx)?;
            Ok(StartedInput {
                children: Vec::new(),
                scheduler: None,
            })
        }
        RuntimeInput::Command(command) => Ok(StartedInput {
            children: vec![input::spawn_command(command, tx)?],
            scheduler: None,
        }),
        RuntimeInput::Commands(commands) => Ok(StartedInput {
            children: input::spawn_named_commands(commands, tx)?,
            scheduler: None,
        }),
        RuntimeInput::StartCommands(commands) => Ok(StartedInput {
            children: Vec::new(),
            scheduler: Some(StartScheduler::new(commands.clone(), tx)?),
        }),
    }
}
