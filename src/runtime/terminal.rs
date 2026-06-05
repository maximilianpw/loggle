use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
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
    clipboard,
    input::{self, ChildShutdown, ShutdownSignal, ShutdownStatus},
    keys::{self, KeyOutcome},
};

pub(super) fn run(
    mut rx: mpsc::Receiver<String>,
    buffer_lines: usize,
    color_enabled: bool,
    source_config: SourceConfig,
    record_path: Option<PathBuf>,
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
        record_path,
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
    record_path: Option<PathBuf>,
    children: &mut Vec<Child>,
) -> io::Result<()> {
    let mut app = App::with_source_config(buffer_lines, source_config);
    let mut shutdown: Option<Vec<ChildShutdown>> = None;
    let mut dirty = true;
    let mut recorder = record_path.map(SessionRecorder::create).transpose()?;

    loop {
        while let Ok(line) = rx.try_recv() {
            if let Some(recorder) = recorder.as_mut() {
                recorder.record_line(&line)?;
            }
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
                flush_recorder(&mut recorder)?;
                children.clear();
                return Ok(());
            }
        }

        if dirty {
            terminal.draw(|frame| {
                ui::draw(
                    frame,
                    &mut app,
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
                        match keys::handle_key(&mut app, key, half_page) {
                            KeyOutcome::Continue => false,
                            KeyOutcome::Quit => true,
                            KeyOutcome::Copy { text, line_count } => {
                                copy_to_clipboard(&mut app, &text, line_count);
                                false
                            }
                        }
                    };

                    if requested_quit {
                        match shutdown.as_mut() {
                            _ if children.is_empty() => {
                                flush_recorder(&mut recorder)?;
                                return Ok(());
                            }
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

struct SessionRecorder {
    writer: BufWriter<File>,
}

impl SessionRecorder {
    fn create(path: PathBuf) -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(File::create(path)?),
        })
    }

    fn record_line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.writer, "{line}")
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

fn flush_recorder(recorder: &mut Option<SessionRecorder>) -> io::Result<()> {
    if let Some(recorder) = recorder.as_mut() {
        recorder.flush()?;
    }
    Ok(())
}

fn copy_to_clipboard(app: &mut App, text: &str, line_count: usize) {
    match clipboard::write(text) {
        Ok(()) => app.set_notice(format!("copied {}", line_count_label(line_count))),
        Err(error) => app.set_notice(format!("copy failed: {error}")),
    }
}

fn line_count_label(count: usize) -> String {
    if count == 1 {
        "1 line".to_string()
    } else {
        format!("{count} lines")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_recorder_writes_raw_lines() {
        let path = std::env::temp_dir().join(format!(
            "loggle-record-test-{}.log",
            std::process::id()
        ));
        {
            let mut recorder = SessionRecorder::create(path.clone()).unwrap();
            recorder.record_line("api | one").unwrap();
            recorder.record_line("web | two").unwrap();
            recorder.flush().unwrap();
        }

        let output = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(output, "api | one\nweb | two\n");
    }
}
