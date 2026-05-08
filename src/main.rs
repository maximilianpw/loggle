mod app;
mod buffer;
mod filter;
mod model;
mod ui;

use std::{
    error::Error,
    fs::File,
    io::{self, BufRead, IsTerminal, Read},
    os::fd::FromRawFd,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use app::{App, Mode, PromptKind};
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

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
        help = "Command to run under loggle, for example: -- docker compose up"
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let (tx, rx) = mpsc::channel(1024);

    let mut child = if cli.command.is_empty() {
        if io::stdin().is_terminal() {
            eprintln!(
                "loggle reads newline-delimited logs from stdin or runs a command.\n\nUsage:\n  docker compose up 2>&1 | loggle\n  loggle -- docker compose up"
            );
            std::process::exit(1);
        }

        let log_input = prepare_terminal_input()?;
        spawn_line_reader(log_input, tx);
        None
    } else {
        Some(spawn_command(&cli.command, tx)?)
    };

    let result = run_terminal(rx, cli.buffer_lines, !cli.no_color);
    if let Some(child) = child.as_mut() {
        terminate_child(child);
    }

    result?;
    Ok(())
}

fn prepare_terminal_input() -> io::Result<File> {
    let stdin_fd = unsafe { libc::dup(libc::STDIN_FILENO) };
    if stdin_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(unsafe { File::from_raw_fd(stdin_fd) })
}

fn spawn_command(command: &[String], tx: mpsc::Sender<String>) -> io::Result<Child> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(stdout) = child.stdout.take() {
        spawn_line_reader(stdout, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_line_reader(stderr, tx);
    }

    Ok(child)
}

fn spawn_line_reader<R>(input: R, tx: mpsc::Sender<String>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_lines(input, tx));
}

fn read_lines<R>(input: R, tx: mpsc::Sender<String>)
where
    R: Read,
{
    let reader = io::BufReader::new(input);
    for line in reader.lines() {
        match line {
            Ok(line) => {
                if tx.blocking_send(line).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn terminate_child(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(_) => {
            let _ = child.kill();
        }
    }
}

fn run_terminal(
    mut rx: mpsc::Receiver<String>,
    buffer_lines: usize,
    color_enabled: bool,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_app(&mut terminal, &mut rx, buffer_lines, color_enabled);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
    terminal.show_cursor()?;

    result
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    rx: &mut mpsc::Receiver<String>,
    buffer_lines: usize,
    color_enabled: bool,
) -> io::Result<()> {
    let mut app = App::new(buffer_lines);

    loop {
        while let Ok(line) = rx.try_recv() {
            app.push_line(line);
        }

        terminal.draw(|frame| ui::draw(frame, &app, color_enabled))?;

        if event::poll(Duration::from_millis(50))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };

            let half_page = (terminal.size()?.height as usize / 2).max(1);
            if handle_key(&mut app, key, half_page) {
                return Ok(());
            }
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent, half_page: usize) -> bool {
    match app.mode() {
        Mode::Prompt(_) => handle_prompt_key(app, key),
        Mode::Normal => handle_normal_key(app, key, half_page),
    }
}

fn handle_prompt_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => app.cancel_prompt(),
        KeyCode::Enter => app.apply_prompt(),
        KeyCode::Backspace => app.pop_prompt_char(),
        KeyCode::Char(value) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            app.push_prompt_char(value);
        }
        _ => {}
    }

    false
}

fn handle_normal_key(app: &mut App, key: KeyEvent, half_page: usize) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) => return true,
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => app.move_down(1),
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => app.move_up(1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.move_down(half_page),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.move_up(half_page),
        (KeyCode::Char('g'), _) => app.handle_g(),
        (KeyCode::Char('G'), _) => app.jump_bottom(),
        (KeyCode::Char('/'), _) => app.start_prompt(PromptKind::Text),
        (KeyCode::Char('s'), _) => app.start_prompt(PromptKind::Source),
        (KeyCode::Char('l'), _) => app.start_prompt(PromptKind::Level),
        (KeyCode::Char('c'), _) => app.clear_filters(),
        (KeyCode::Char(' '), _) | (KeyCode::Char('p'), _) => app.toggle_follow(),
        (KeyCode::Char('n'), _) => app.next_search_match(),
        (KeyCode::Char('N'), _) => app.previous_search_match(),
        (KeyCode::Esc, _) => app.clear_transient(),
        _ => app.clear_transient(),
    }

    false
}
