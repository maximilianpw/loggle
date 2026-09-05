use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
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

use crate::{
    app::App,
    model::SourceConfig,
    page_log::{ActiveLogPageRegistration, LogPageId, PageLogRecorder, claim_active_log_page},
    ui,
};

use super::{
    clipboard,
    input::{self, Child, ChildShutdown, ShutdownSignal, ShutdownStatus},
    keys::{self, KeyOutcome},
};

pub(super) fn run(
    mut rx: mpsc::Receiver<String>,
    startup_lines: Vec<String>,
    buffer_lines: usize,
    color_enabled: bool,
    source_config: SourceConfig,
    record_path: Option<PathBuf>,
    page_id: Option<LogPageId>,
    page_command: String,
    page_logging: bool,
    mut children: Vec<Child>,
) -> io::Result<()> {
    let mut terminal = TerminalSession::enter()?;
    let result = run_app(
        terminal.terminal_mut(),
        &mut rx,
        startup_lines,
        buffer_lines,
        color_enabled,
        source_config,
        record_path,
        page_id,
        page_command,
        page_logging,
        &mut children,
    );

    drop(children);

    let cleanup_result = terminal.restore();
    match (result, cleanup_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    mode: TerminalModeGuard,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        let mut mode = TerminalModeGuard::enter_raw_mode()?;
        let mut stdout = io::stdout();
        // A write can fail after entering the alternate screen. Arm restoration
        // before attempting any output, not only after the entire write succeeds.
        mode.mark_alternate_screen_entered();
        execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Self { terminal, mode })
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    fn restore(&mut self) -> io::Result<()> {
        self.mode.restore(&mut self.terminal)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct TerminalModeGuard {
    raw_mode: bool,
    alternate_screen: bool,
}

impl TerminalModeGuard {
    fn enter_raw_mode() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self {
            raw_mode: true,
            alternate_screen: false,
        })
    }

    fn mark_alternate_screen_entered(&mut self) {
        self.alternate_screen = true;
    }

    fn restore(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        let mut first_error = None;

        if self.raw_mode {
            if let Err(error) = disable_raw_mode() {
                first_error.get_or_insert(error);
            }
            self.raw_mode = false;
        }

        if self.alternate_screen {
            if let Err(error) = execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)
            {
                first_error.get_or_insert(error);
            }
            self.alternate_screen = false;
        }

        if let Err(error) = terminal.show_cursor() {
            first_error.get_or_insert(error);
        }

        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        if self.raw_mode {
            let _ = disable_raw_mode();
            self.raw_mode = false;
        }

        if self.alternate_screen {
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
            self.alternate_screen = false;
        }
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    rx: &mut mpsc::Receiver<String>,
    startup_lines: Vec<String>,
    buffer_lines: usize,
    color_enabled: bool,
    source_config: SourceConfig,
    record_path: Option<PathBuf>,
    page_id: Option<LogPageId>,
    page_command: String,
    page_logging: bool,
    children: &mut Vec<Child>,
) -> io::Result<()> {
    let mut app = App::with_source_config(buffer_lines, source_config);
    let mut shutdown: Option<Vec<ChildShutdown>> = None;
    let mut dirty = true;
    let mut recorder = record_path.map(SessionRecorder::create).transpose()?;
    // The page log is an auxiliary, always-on feature; failures disable it with
    // a notice rather than tearing down the viewer the user actually asked for.
    let mut page_recorder = None;
    let mut page_id_for_header = None;
    let mut active_page = None;
    if page_logging {
        match start_page_log(page_id, &page_command, buffer_lines) {
            Ok((id, recorder, registration)) => {
                page_id_for_header = Some(id);
                page_recorder = Some(recorder);
                active_page = Some(registration);
            }
            Err(error) => app.set_notice(format!("page log disabled: {error}")),
        }
    }
    let _active_page = active_page;

    let had_startup_lines = !startup_lines.is_empty();
    for line in startup_lines {
        ingest_line(&mut app, &mut recorder, &mut page_recorder, line)?;
    }
    if had_startup_lines {
        flush_page_recorder(&mut app, &mut page_recorder);
    }

    loop {
        let mut received = false;
        while let Ok(line) = rx.try_recv() {
            ingest_line(&mut app, &mut recorder, &mut page_recorder, line)?;
            received = true;
            dirty = true;
        }

        // Flush once per drain instead of per line, so the read command sees
        // fresh data without a syscall on every ingested line.
        if received {
            flush_page_recorder(&mut app, &mut page_recorder);
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
                flush_recorders(&mut recorder, &mut page_recorder)?;
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
                    page_id_for_header.as_ref(),
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
                                flush_recorders(&mut recorder, &mut page_recorder)?;
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

fn ingest_line(
    app: &mut App,
    recorder: &mut Option<SessionRecorder>,
    page_recorder: &mut Option<PageLogRecorder>,
    line: String,
) -> io::Result<()> {
    if let Some(recorder) = recorder.as_mut() {
        recorder.record_line(&line)?;
    }
    if let Some(active_recorder) = page_recorder.as_mut() {
        if let Err(error) = active_recorder.record_line(&line) {
            app.set_notice(format!("page log disabled: {error}"));
            *page_recorder = None;
        }
    }
    app.push_line(line);
    Ok(())
}

fn flush_page_recorder(app: &mut App, page_recorder: &mut Option<PageLogRecorder>) {
    if let Some(active_recorder) = page_recorder.as_mut() {
        if let Err(error) = active_recorder.flush() {
            app.set_notice(format!("page log disabled: {error}"));
            *page_recorder = None;
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

fn start_page_log(
    page_id: Option<LogPageId>,
    page_command: &str,
    buffer_lines: usize,
) -> io::Result<(LogPageId, PageLogRecorder, ActiveLogPageRegistration)> {
    let (id, registration) =
        claim_active_log_page(page_id, page_command).map_err(io::Error::other)?;
    let recorder = PageLogRecorder::create(&id, buffer_lines).map_err(io::Error::other)?;
    Ok((id, recorder, registration))
}

fn flush_recorders(
    recorder: &mut Option<SessionRecorder>,
    page_recorder: &mut Option<PageLogRecorder>,
) -> io::Result<()> {
    if let Some(recorder) = recorder.as_mut() {
        recorder.flush()?;
    }
    // Best-effort: a failure flushing the auxiliary page log must not fail the
    // session's clean shutdown.
    if let Some(page_recorder) = page_recorder.as_mut() {
        let _ = page_recorder.flush();
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
        let path =
            std::env::temp_dir().join(format!("loggle-record-test-{}.log", std::process::id()));
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
