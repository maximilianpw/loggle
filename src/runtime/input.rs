use std::{
    fs::File,
    io::{self, BufRead, IsTerminal, Read},
    os::fd::FromRawFd,
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use super::NamedCommand;

pub(super) const LINE_CHANNEL_CAPACITY: usize = 1024;

pub(super) fn stdin_is_terminal() -> bool {
    io::stdin().is_terminal()
}

pub(super) fn spawn_stdin_reader(tx: mpsc::Sender<String>) -> io::Result<()> {
    let input = prepare_terminal_input()?;
    spawn_line_reader(input, tx);
    Ok(())
}

pub(super) fn spawn_command(command: &[String], tx: mpsc::Sender<String>) -> io::Result<Child> {
    let mut child = spawn_child(command)?;

    if let Some(stdout) = child.stdout.take() {
        spawn_line_reader(stdout, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_line_reader(stderr, tx);
    }

    Ok(child)
}

pub(super) fn spawn_named_commands(
    commands: &[NamedCommand],
    tx: mpsc::Sender<String>,
) -> io::Result<Vec<Child>> {
    let mut children = Vec::with_capacity(commands.len());

    for command in commands {
        match spawn_named_command(command, tx.clone()) {
            Ok(child) => children.push(child),
            Err(error) => {
                for child in &mut children {
                    force_kill_child_group(child);
                }
                return Err(error);
            }
        }
    }

    Ok(children)
}

fn spawn_named_command(command: &NamedCommand, tx: mpsc::Sender<String>) -> io::Result<Child> {
    let mut child = spawn_child(&command.command)?;

    if let Some(stdout) = child.stdout.take() {
        spawn_prefixed_line_reader(stdout, command.name.clone(), tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_prefixed_line_reader(stderr, command.name.clone(), tx);
    }

    Ok(child)
}

fn spawn_child(command: &[String]) -> io::Result<Child> {
    let mut command_builder = Command::new(&command[0]);
    command_builder
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    unsafe {
        command_builder.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    command_builder.spawn()
}

fn prepare_terminal_input() -> io::Result<File> {
    let stdin_fd = unsafe { libc::dup(libc::STDIN_FILENO) };
    if stdin_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(unsafe { File::from_raw_fd(stdin_fd) })
}

fn spawn_line_reader<R>(input: R, tx: mpsc::Sender<String>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_lines(input, tx));
}

fn spawn_prefixed_line_reader<R>(input: R, source: String, tx: mpsc::Sender<String>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_lines_with_prefix(input, Some(source), tx));
}

fn read_lines<R>(input: R, tx: mpsc::Sender<String>)
where
    R: Read,
{
    read_lines_with_prefix(input, None, tx);
}

fn read_lines_with_prefix<R>(input: R, source: Option<String>, tx: mpsc::Sender<String>)
where
    R: Read,
{
    let reader = io::BufReader::new(input);
    for line in reader.lines() {
        match line {
            Ok(line) => {
                let line = source
                    .as_ref()
                    .map(|source| format!("[{source}] {line}"))
                    .unwrap_or(line);
                if tx.blocking_send(line).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShutdownSignal {
    Interrupt,
    Terminate,
    Kill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShutdownStatus {
    Waiting(ShutdownSignal),
    Exited,
}

#[derive(Debug)]
pub(super) struct ChildShutdown {
    process_group: libc::pid_t,
    signal: ShutdownSignal,
    next_escalation: Instant,
    finished: bool,
    send_signals: bool,
}

impl ChildShutdown {
    pub(super) fn start(child: &Child, now: Instant) -> Self {
        let shutdown = Self {
            process_group: child.id() as libc::pid_t,
            signal: ShutdownSignal::Interrupt,
            next_escalation: now + interrupt_timeout(),
            finished: false,
            send_signals: true,
        };
        shutdown.send_current_signal();
        shutdown
    }

    pub(super) fn status(&self) -> ShutdownStatus {
        if self.finished {
            ShutdownStatus::Exited
        } else {
            ShutdownStatus::Waiting(self.signal)
        }
    }

    pub(super) fn tick(&mut self, child: &mut Child, now: Instant) -> io::Result<ShutdownStatus> {
        if child.try_wait()?.is_some() {
            self.finished = true;
            return Ok(ShutdownStatus::Exited);
        }

        if now >= self.next_escalation {
            self.escalate(now);
        }

        Ok(self.status())
    }

    pub(super) fn escalate_now(&mut self, now: Instant) {
        self.escalate(now);
    }

    fn escalate(&mut self, now: Instant) {
        match self.signal {
            ShutdownSignal::Interrupt => {
                self.signal = ShutdownSignal::Terminate;
                self.next_escalation = now + terminate_timeout();
                self.send_current_signal();
            }
            ShutdownSignal::Terminate => {
                self.signal = ShutdownSignal::Kill;
                self.next_escalation = now + kill_retry_timeout();
                self.send_current_signal();
            }
            ShutdownSignal::Kill => {
                self.next_escalation = now + kill_retry_timeout();
                self.send_current_signal();
            }
        }
    }

    fn send_current_signal(&self) {
        if !self.send_signals {
            return;
        }

        let signal = match self.signal {
            ShutdownSignal::Interrupt => libc::SIGINT,
            ShutdownSignal::Terminate => libc::SIGTERM,
            ShutdownSignal::Kill => libc::SIGKILL,
        };
        unsafe {
            libc::kill(-self.process_group, signal);
        }
    }
}

pub(super) fn reap_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.wait();
    }
}

pub(super) fn force_kill_child_group(child: &mut Child) {
    let process_group = child.id() as libc::pid_t;
    unsafe {
        libc::kill(-process_group, libc::SIGKILL);
    }
    let _ = child.wait();
}

fn interrupt_timeout() -> Duration {
    Duration::from_secs(5)
}

fn terminate_timeout() -> Duration {
    Duration::from_secs(2)
}

fn kill_retry_timeout() -> Duration {
    Duration::from_millis(500)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_lines_sends_each_line_until_eof() {
        let (tx, mut rx) = mpsc::channel(4);

        read_lines("one\ntwo\n".as_bytes(), tx);

        assert_eq!(rx.blocking_recv(), Some("one".to_string()));
        assert_eq!(rx.blocking_recv(), Some("two".to_string()));
        assert_eq!(rx.blocking_recv(), None);
    }

    #[test]
    fn prefixed_line_reader_marks_each_line_with_source_name() {
        let (tx, mut rx) = mpsc::channel(4);

        read_lines_with_prefix("one\ntwo\n".as_bytes(), Some("api".to_string()), tx);

        assert_eq!(rx.blocking_recv(), Some("[api] one".to_string()));
        assert_eq!(rx.blocking_recv(), Some("[api] two".to_string()));
        assert_eq!(rx.blocking_recv(), None);
    }

    #[test]
    fn shutdown_escalates_by_timeout() {
        let now = Instant::now();
        let mut shutdown = ChildShutdown {
            process_group: 1,
            signal: ShutdownSignal::Interrupt,
            next_escalation: now + interrupt_timeout(),
            finished: false,
            send_signals: false,
        };

        shutdown.escalate(now + interrupt_timeout());
        assert_eq!(shutdown.status(), ShutdownStatus::Waiting(ShutdownSignal::Terminate));

        shutdown.escalate(now + interrupt_timeout() + terminate_timeout());
        assert_eq!(shutdown.status(), ShutdownStatus::Waiting(ShutdownSignal::Kill));
    }

    #[test]
    fn shutdown_second_quit_escalates_immediately() {
        let now = Instant::now();
        let mut shutdown = ChildShutdown {
            process_group: 1,
            signal: ShutdownSignal::Interrupt,
            next_escalation: now + interrupt_timeout(),
            finished: false,
            send_signals: false,
        };

        shutdown.escalate_now(now);

        assert_eq!(shutdown.status(), ShutdownStatus::Waiting(ShutdownSignal::Terminate));
    }

    #[test]
    fn shutdown_interrupt_stops_spawned_process_group() {
        let (tx, _rx) = mpsc::channel(4);
        let mut child = spawn_command(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "trap 'exit 42' INT; while true; do sleep 1; done".to_string(),
            ],
            tx,
        )
        .unwrap();
        let mut shutdown = ChildShutdown::start(&child, Instant::now());
        let deadline = Instant::now() + Duration::from_secs(3);

        while Instant::now() < deadline {
            if shutdown.tick(&mut child, Instant::now()).unwrap() == ShutdownStatus::Exited {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }

        force_kill_child_group(&mut child);
        panic!("spawned process group did not exit after SIGINT");
    }
}
