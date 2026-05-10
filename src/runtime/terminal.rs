use std::{
    io,
    process::Child,
    time::{Duration, Instant},
};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::{app::App, model::SourceConfig, ui};

use super::{
    input::{self, ChildShutdown, ShutdownSignal, ShutdownStatus},
    keys::{self, KeyOutcome},
};

pub(super) fn run(
    mut rx: mpsc::Receiver<String>,
    buffer_lines: usize,
    color_enabled: bool,
    source_config: SourceConfig,
    mut child: Option<Child>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_app(
        &mut terminal,
        &mut rx,
        buffer_lines,
        color_enabled,
        source_config,
        &mut child,
    );

    if result.is_err() {
        if let Some(child) = child.as_mut() {
            input::force_kill_child_group(child);
        }
    }

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
    source_config: SourceConfig,
    child: &mut Option<Child>,
) -> io::Result<()> {
    let mut app = App::with_source_config(buffer_lines, source_config);
    let mut shutdown: Option<ChildShutdown> = None;

    loop {
        while let Ok(line) = rx.try_recv() {
            app.push_line(line);
        }

        if let (Some(active_child), Some(active_shutdown)) = (child.as_mut(), shutdown.as_mut()) {
            if let ShutdownStatus::Exited = active_shutdown.tick(active_child, Instant::now())? {
                input::reap_child(active_child);
                child.take();
                return Ok(());
            }
        }

        terminal.draw(|frame| {
            ui::draw(
                frame,
                &app,
                color_enabled,
                shutdown.as_ref().map(closing_message),
            )
        })?;

        if event::poll(Duration::from_millis(50))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };

            let requested_quit = if shutdown.is_some() {
                matches!(key.code, KeyCode::Char('q'))
            } else {
                let half_page = (terminal.size()?.height as usize / 2).max(1);
                keys::handle_key(&mut app, key, half_page) == KeyOutcome::Quit
            };

            if requested_quit {
                match (child.as_ref(), shutdown.as_mut()) {
                    (None, _) => return Ok(()),
                    (Some(child), None) => {
                        shutdown = Some(ChildShutdown::start(child, Instant::now()));
                    }
                    (Some(_), Some(active_shutdown)) => {
                        active_shutdown.escalate_now(Instant::now());
                    }
                }
            }
        }
    }
}

fn closing_message(shutdown: &ChildShutdown) -> &'static str {
    match shutdown.status() {
        ShutdownStatus::Waiting(ShutdownSignal::Interrupt) => "closing... sent interrupt",
        ShutdownStatus::Waiting(ShutdownSignal::Terminate) => "closing... terminating child",
        ShutdownStatus::Waiting(ShutdownSignal::Kill) => "closing... force killing child",
        ShutdownStatus::Exited => "closing... child exited",
    }
}
