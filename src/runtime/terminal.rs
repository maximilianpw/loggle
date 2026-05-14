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
    mut children: Vec<Child>,
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
        &mut children,
    );

    if result.is_err() {
        for child in &mut children {
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
    children: &mut Vec<Child>,
) -> io::Result<()> {
    let mut app = App::with_source_config(buffer_lines, source_config);
    let mut shutdown: Option<Vec<ChildShutdown>> = None;
    let mut dirty = true;

    loop {
        while let Ok(line) = rx.try_recv() {
            app.push_line(line);
            dirty = true;
        }

        if let Some(active_shutdowns) = shutdown.as_mut() {
            let mut all_exited = true;
            for (active_child, active_shutdown) in children.iter_mut().zip(active_shutdowns) {
                let previous_status = active_shutdown.status();
                let status = active_shutdown.tick(active_child, Instant::now())?;
                if status != previous_status {
                    dirty = true;
                }

                if let ShutdownStatus::Exited = status {
                    input::reap_child(active_child);
                } else {
                    all_exited = false;
                }
            }

            if all_exited {
                children.clear();
                return Ok(());
            }
        }

        if dirty {
            terminal.draw(|frame| {
                ui::draw(
                    frame,
                    &app,
                    color_enabled,
                    shutdown.as_deref().map(closing_message),
                )
            })?;
            dirty = false;
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    let requested_quit = if shutdown.is_some() {
                        matches!(key.code, KeyCode::Char('q'))
                    } else {
                        let half_page = (terminal.size()?.height as usize / 2).max(1);
                        keys::handle_key(&mut app, key, half_page) == KeyOutcome::Quit
                    };

                    if requested_quit {
                        match shutdown.as_mut() {
                            _ if children.is_empty() => return Ok(()),
                            None => {
                                let now = Instant::now();
                                shutdown = Some(
                                    children
                                        .iter()
                                        .map(|child| ChildShutdown::start(child, now))
                                        .collect(),
                                );
                            }
                            Some(active_shutdowns) => {
                                let now = Instant::now();
                                for active_shutdown in active_shutdowns {
                                    active_shutdown.escalate_now(now);
                                }
                            }
                        }
                    }

                    dirty = true;
                }
                Event::Resize(_, _) => {
                    dirty = true;
                }
                _ => {}
            }
        }
    }
}

fn closing_message(shutdowns: &[ChildShutdown]) -> &'static str {
    let status = shutdowns
        .iter()
        .map(ChildShutdown::status)
        .find(|status| !matches!(status, ShutdownStatus::Exited))
        .unwrap_or(ShutdownStatus::Exited);

    match status {
        ShutdownStatus::Waiting(ShutdownSignal::Interrupt) => "closing... sent interrupt",
        ShutdownStatus::Waiting(ShutdownSignal::Terminate) => "closing... terminating children",
        ShutdownStatus::Waiting(ShutdownSignal::Kill) => "closing... force killing children",
        ShutdownStatus::Exited => "closing... children exited",
    }
}
