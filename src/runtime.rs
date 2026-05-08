mod input;
mod keys;
mod terminal;

use std::{fmt, io, process::Child};

use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub buffer_lines: usize,
    pub color_enabled: bool,
    pub command: Vec<String>,
}

#[derive(Debug)]
pub enum RuntimeError {
    MissingInput,
    Io(io::Error),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingInput => f.write_str(
                "loggle reads newline-delimited logs from stdin or runs a command.\n\nUsage:\n  docker compose up 2>&1 | loggle\n  loggle -- docker compose up",
            ),
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
    let (tx, rx) = mpsc::channel(input::LINE_CHANNEL_CAPACITY);
    let mut child = start_input(&config.command, tx)?;

    let result = terminal::run(rx, config.buffer_lines, config.color_enabled);
    if let Some(child) = child.as_mut() {
        input::terminate_child(child);
    }

    result.map_err(RuntimeError::from)
}

fn start_input(
    command: &[String],
    tx: mpsc::Sender<String>,
) -> Result<Option<Child>, RuntimeError> {
    if command.is_empty() {
        if input::stdin_is_terminal() {
            return Err(RuntimeError::MissingInput);
        }

        input::spawn_stdin_reader(tx)?;
        Ok(None)
    } else {
        Ok(Some(input::spawn_command(command, tx)?))
    }
}
