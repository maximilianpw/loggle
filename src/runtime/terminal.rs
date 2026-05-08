use std::{io, time::Duration};

use crossterm::{
    cursor,
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::{app::App, ui};

use super::keys::{self, KeyOutcome};

pub(super) fn run(
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
            if keys::handle_key(&mut app, key, half_page) == KeyOutcome::Quit {
                return Ok(());
            }
        }
    }
}
