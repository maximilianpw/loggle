use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{self, BufRead, IsTerminal, Read},
    os::fd::FromRawFd,
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc as std_mpsc,
    thread,
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use super::{NamedCommand, ReadySpec, StartCommand};

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
    let mut child = spawn_child(command, None)?;

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

pub(super) fn spawn_start_commands(
    commands: &[StartCommand],
    tx: mpsc::Sender<String>,
) -> io::Result<Vec<Child>> {
    StartScheduler::new(commands, tx).run()
}

fn spawn_named_command(command: &NamedCommand, tx: mpsc::Sender<String>) -> io::Result<Child> {
    let mut child = spawn_child(&command.command, command.cwd.as_deref())?;

    if let Some(stdout) = child.stdout.take() {
        spawn_prefixed_line_reader(stdout, command.name.clone(), tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_prefixed_line_reader(stderr, command.name.clone(), tx);
    }

    Ok(child)
}

fn spawn_start_command(
    command: &StartCommand,
    tx: mpsc::Sender<String>,
) -> io::Result<SpawnedStartCommand> {
    let mut child = spawn_child(&command.argv, command.cwd.as_deref())?;
    let ready_line = match &command.ready {
        Some(ReadySpec::Line { text, .. }) => Some(text.clone()),
        _ => None,
    };
    let (ready_tx, ready_rx) = ready_line
        .as_ref()
        .map(|_| std_mpsc::channel())
        .map(|(tx, rx)| (Some(tx), Some(rx)))
        .unwrap_or((None, None));

    if let Some(stdout) = child.stdout.take() {
        spawn_start_line_reader(
            stdout,
            command.name.clone(),
            tx.clone(),
            ready_line.clone(),
            ready_tx.clone(),
        );
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_start_line_reader(
            stderr,
            command.name.clone(),
            tx,
            ready_line,
            ready_tx,
        );
    }

    Ok(SpawnedStartCommand {
        child,
        line_ready_rx: ready_rx,
    })
}

fn spawn_child(command: &[String], cwd: Option<&Path>) -> io::Result<Child> {
    let mut command_builder = Command::new(&command[0]);
    command_builder
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(cwd) = cwd {
        command_builder.current_dir(cwd);
    }

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

fn spawn_probe(command: &[String], cwd: Option<&Path>) -> io::Result<Child> {
    let mut command_builder = Command::new(&command[0]);
    command_builder
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(cwd) = cwd {
        command_builder.current_dir(cwd);
    }

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

fn spawn_start_line_reader<R>(
    input: R,
    source: String,
    tx: mpsc::Sender<String>,
    ready_line: Option<String>,
    ready_tx: Option<std_mpsc::Sender<()>>,
) where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_start_lines(input, source, tx, ready_line, ready_tx));
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

fn read_start_lines<R>(
    input: R,
    source: String,
    tx: mpsc::Sender<String>,
    ready_line: Option<String>,
    ready_tx: Option<std_mpsc::Sender<()>>,
) where
    R: Read,
{
    let reader = io::BufReader::new(input);
    let mut signaled_ready = false;

    for line in reader.lines() {
        match line {
            Ok(line) => {
                let matches_ready = ready_line
                    .as_ref()
                    .is_some_and(|ready_line| line.contains(ready_line));
                if tx.blocking_send(format!("[{source}] {line}")).is_err() {
                    break;
                }
                if matches_ready && !signaled_ready {
                    if let Some(ready_tx) = &ready_tx {
                        let _ = ready_tx.send(());
                    }
                    signaled_ready = true;
                }
            }
            Err(_) => break,
        }
    }
}

#[derive(Debug)]
struct SpawnedStartCommand {
    child: Child,
    line_ready_rx: Option<std_mpsc::Receiver<()>>,
}

struct StartScheduler<'a> {
    commands: &'a [StartCommand],
    tx: mpsc::Sender<String>,
    command_indexes: BTreeMap<&'a str, usize>,
    states: Vec<StartState>,
    children: Vec<Option<Child>>,
    line_ready: Vec<Option<std_mpsc::Receiver<()>>>,
    command_ready: Vec<Option<CommandReadyState>>,
}

impl<'a> StartScheduler<'a> {
    fn new(commands: &'a [StartCommand], tx: mpsc::Sender<String>) -> Self {
        let command_indexes = commands
            .iter()
            .enumerate()
            .map(|(index, command)| (command.name.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let len = commands.len();

        Self {
            commands,
            tx,
            command_indexes,
            states: vec![StartState::Pending; len],
            children: (0..len).map(|_| None).collect(),
            line_ready: (0..len).map(|_| None).collect(),
            command_ready: (0..len).map(|_| None).collect(),
        }
    }

    fn run(mut self) -> io::Result<Vec<Child>> {
        while !self.all_ready() {
            let now = Instant::now();
            let mut progressed = match self.spawn_unblocked(now) {
                Ok(progressed) => progressed,
                Err(error) => {
                    self.kill_started_children();
                    return Err(error);
                }
            };
            progressed |= match self.check_readiness(now) {
                Ok(progressed) => progressed,
                Err(error) => {
                    self.kill_started_children();
                    return Err(error);
                }
            };

            if !progressed {
                thread::sleep(Duration::from_millis(10));
            }
        }

        Ok(self
            .children
            .into_iter()
            .map(|child| child.expect("ready start commands have children"))
            .collect())
    }

    fn all_ready(&self) -> bool {
        self.states.iter().all(|state| *state == StartState::Ready)
    }

    fn spawn_unblocked(&mut self, now: Instant) -> io::Result<bool> {
        let mut progressed = false;
        for index in 0..self.commands.len() {
            if self.states[index] != StartState::Pending || !self.dependencies_ready(index) {
                continue;
            }

            self.spawn_command(index, now)?;
            progressed = true;
        }

        Ok(progressed)
    }

    fn dependencies_ready(&self, index: usize) -> bool {
        self.commands[index].wait_for.iter().all(|dependency| {
            let dependency_index = self.command_indexes[dependency.as_str()];
            self.states[dependency_index] == StartState::Ready
        })
    }

    fn spawn_command(&mut self, index: usize, now: Instant) -> io::Result<()> {
        let command = &self.commands[index];
        let spawned = spawn_start_command(command, self.tx.clone())?;

        self.children[index] = Some(spawned.child);
        match &command.ready {
            None => {
                self.states[index] = StartState::Ready;
            }
            Some(ReadySpec::Line { timeout, .. }) => {
                self.states[index] = StartState::Started;
                self.line_ready[index] = spawned.line_ready_rx;
                self.command_ready[index] = Some(CommandReadyState::line_timeout(now + *timeout));
            }
            Some(ReadySpec::Command {
                command,
                interval,
                timeout,
            }) => {
                self.states[index] = StartState::Started;
                self.command_ready[index] = Some(CommandReadyState::command(
                    command.clone(),
                    *interval,
                    now,
                    now + *timeout,
                ));
            }
        }

        Ok(())
    }

    fn check_readiness(&mut self, now: Instant) -> io::Result<bool> {
        let mut progressed = false;

        for index in 0..self.commands.len() {
            if self.states[index] != StartState::Started {
                continue;
            }

            if self.line_ready[index]
                .as_ref()
                .is_some_and(|rx| rx.try_recv().is_ok())
            {
                self.states[index] = StartState::Ready;
                progressed = true;
                continue;
            }

            if let Some(command_ready) = self.command_ready[index].as_mut() {
                if command_ready.is_command_probe_due(now) {
                    let probe_outcome =
                        command_ready.run_probe(self.commands[index].cwd.as_deref(), now);
                    let probe_outcome = match probe_outcome {
                        Ok(outcome) => outcome,
                        Err(error) => return Err(error),
                    };

                    match probe_outcome {
                        ProbeOutcome::Ready => {
                            self.states[index] = StartState::Ready;
                            progressed = true;
                            continue;
                        }
                        ProbeOutcome::NotReady => {}
                    }
                }

                if now >= command_ready.deadline {
                    return Err(command_ready.timeout_error(&self.commands[index].name));
                }
            }

            if let Some(child) = self.children[index].as_mut() {
                if let Some(status) = child.try_wait()? {
                    input_reap_child(child);
                    self.children[index] = None;
                    let message = format!(
                        "command '{}' exited before readiness{}",
                        self.commands[index].name,
                        status
                            .code()
                            .map(|code| format!(" with status {code}"))
                            .unwrap_or_default()
                    );
                    return Err(io::Error::other(message));
                }
            }
        }

        Ok(progressed)
    }

    fn kill_started_children(&mut self) {
        for child in self.children.iter_mut().flatten() {
            force_kill_child_group(child);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartState {
    Pending,
    Started,
    Ready,
}

#[derive(Debug)]
struct CommandReadyState {
    kind: ReadyKind,
    deadline: Instant,
    recent_output: RecentProbeOutput,
}

impl CommandReadyState {
    fn line_timeout(deadline: Instant) -> Self {
        Self {
            kind: ReadyKind::Line,
            deadline,
            recent_output: RecentProbeOutput::new(),
        }
    }

    fn command(
        command: Vec<String>,
        interval: Duration,
        next_probe: Instant,
        deadline: Instant,
    ) -> Self {
        Self {
            kind: ReadyKind::Command {
                command,
                interval,
                next_probe,
            },
            deadline,
            recent_output: RecentProbeOutput::new(),
        }
    }

    fn is_command_probe_due(&self, now: Instant) -> bool {
        matches!(
            &self.kind,
            ReadyKind::Command { next_probe, .. } if now >= *next_probe
        )
    }

    fn run_probe(&mut self, cwd: Option<&Path>, now: Instant) -> io::Result<ProbeOutcome> {
        let ReadyKind::Command {
            command,
            interval,
            next_probe,
        } = &mut self.kind
        else {
            return Ok(ProbeOutcome::NotReady);
        };

        let probe = run_probe_with_deadline(command, cwd, self.deadline)?;
        self.recent_output.push(probe.output_summary());

        if probe.success {
            return Ok(ProbeOutcome::Ready);
        }
        if probe.timed_out {
            return Err(self.timeout_error_for_output("readiness probe timed out"));
        }

        *next_probe = now + *interval;
        Ok(ProbeOutcome::NotReady)
    }

    fn timeout_error(&self, command_name: &str) -> io::Error {
        let mut message = format!("command '{command_name}' readiness timed out");
        let output = self.recent_output.summary();
        if !output.is_empty() {
            message.push_str("\nrecent readiness probe output:\n");
            message.push_str(&output);
        }

        io::Error::other(message)
    }

    fn timeout_error_for_output(&self, message: &str) -> io::Error {
        let output = self.recent_output.summary();
        if output.is_empty() {
            io::Error::other(message.to_string())
        } else {
            io::Error::other(format!("{message}\nrecent readiness probe output:\n{output}"))
        }
    }
}

#[derive(Debug)]
enum ReadyKind {
    Line,
    Command {
        command: Vec<String>,
        interval: Duration,
        next_probe: Instant,
    },
}

#[derive(Debug)]
enum ProbeOutcome {
    Ready,
    NotReady,
}

#[derive(Debug)]
struct ProbeRun {
    success: bool,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProbeRun {
    fn output_summary(&self) -> String {
        let stdout = String::from_utf8_lossy(&self.stdout);
        let stderr = String::from_utf8_lossy(&self.stderr);
        let mut output = String::new();

        if !stdout.trim().is_empty() {
            output.push_str("stdout:\n");
            output.push_str(stdout.trim_end());
        }
        if !stderr.trim().is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str("stderr:\n");
            output.push_str(stderr.trim_end());
        }

        output
    }
}

#[derive(Debug)]
struct RecentProbeOutput {
    entries: VecDeque<String>,
}

impl RecentProbeOutput {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn push(&mut self, output: String) {
        if output.is_empty() {
            return;
        }

        self.entries.push_back(output);
        while self.entries.len() > 5 {
            self.entries.pop_front();
        }
    }

    fn summary(&self) -> String {
        self.entries
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n---\n")
    }
}

fn run_probe_with_deadline(
    command: &[String],
    cwd: Option<&Path>,
    deadline: Instant,
) -> io::Result<ProbeRun> {
    let mut child = spawn_probe(command, cwd)?;
    let stdout = child
        .stdout
        .take()
        .map(read_pipe_in_thread)
        .expect("probe stdout is piped");
    let stderr = child
        .stderr
        .take()
        .map(read_pipe_in_thread)
        .expect("probe stderr is piped");
    let mut timed_out = false;
    let success;

    loop {
        if let Some(status) = child.try_wait()? {
            success = status.success();
            break;
        }

        if Instant::now() >= deadline {
            timed_out = true;
            force_kill_child_group(&mut child);
            success = false;
            break;
        }

        thread::sleep(Duration::from_millis(10));
    }

    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();

    Ok(ProbeRun {
        success,
        timed_out,
        stdout,
        stderr,
    })
}

fn read_pipe_in_thread<R>(mut input: R) -> thread::JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut output = Vec::new();
        let _ = input.read_to_end(&mut output);
        output
    })
}

fn input_reap_child(child: &mut Child) {
    let _ = child.wait();
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
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "loggle-runtime-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn command(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn start_command(name: &str, argv: &[&str]) -> StartCommand {
        StartCommand {
            name: name.to_string(),
            argv: command(argv),
            cwd: None,
            wait_for: Vec::new(),
            ready: None,
        }
    }

    fn recv_lines(rx: &mut mpsc::Receiver<String>, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut lines = Vec::new();

        while lines.len() < count && Instant::now() < deadline {
            match rx.try_recv() {
                Ok(line) => lines.push(line),
                Err(mpsc::error::TryRecvError::Empty) => thread::sleep(Duration::from_millis(10)),
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        lines
    }

    fn cleanup_children(children: &mut [Child]) {
        for child in children {
            if child.try_wait().ok().flatten().is_none() {
                force_kill_child_group(child);
            }
        }
    }

    fn process_is_running(pid: libc::pid_t) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

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
    fn named_commands_run_from_configured_cwd_and_keep_source_prefix() {
        let cwd = temp_dir("cwd");
        fs::write(cwd.join("marker"), "").unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let mut children = spawn_named_commands(
            &[NamedCommand {
                name: "api".to_string(),
                command: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "test -f marker && echo cwd-ok".to_string(),
                ],
                cwd: Some(cwd.clone()),
            }],
            tx,
        )
        .unwrap();

        assert_eq!(rx.blocking_recv(), Some("[api] cwd-ok".to_string()));
        assert!(children.pop().unwrap().wait().unwrap().success());
        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn start_commands_wait_for_ready_line_before_starting_dependents() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut db = start_command(
            "db",
            &[
                "/bin/sh",
                "-c",
                "echo booting; sleep 0.1; echo database-ready; sleep 1",
            ],
        );
        db.ready = Some(ReadySpec::Line {
            text: "database-ready".to_string(),
            timeout: Duration::from_secs(2),
        });
        let mut api = start_command("api", &["/bin/sh", "-c", "echo api-started"]);
        api.wait_for = command(&["db"]);

        let mut children = spawn_start_commands(&[db, api], tx).unwrap();
        let lines = recv_lines(&mut rx, 3);

        cleanup_children(&mut children);
        assert_eq!(
            lines,
            vec![
                "[db] booting".to_string(),
                "[db] database-ready".to_string(),
                "[api] api-started".to_string(),
            ]
        );
    }

    #[test]
    fn start_commands_wait_for_ready_command_before_starting_dependents() {
        let cwd = temp_dir("ready-command");
        let (tx, mut rx) = mpsc::channel(16);
        let mut db = start_command(
            "db",
            &["/bin/sh", "-c", "sleep 0.1; touch ready; sleep 1"],
        );
        db.cwd = Some(cwd.clone());
        db.ready = Some(ReadySpec::Command {
            command: command(&["/bin/sh", "-c", "echo probe-output; test -f ready"]),
            interval: Duration::from_millis(25),
            timeout: Duration::from_secs(2),
        });
        let mut api = start_command("api", &["/bin/sh", "-c", "echo api-started"]);
        api.cwd = Some(cwd.clone());
        api.wait_for = command(&["db"]);

        let mut children = spawn_start_commands(&[db, api], tx).unwrap();
        let lines = recv_lines(&mut rx, 1);

        cleanup_children(&mut children);
        assert_eq!(lines, vec!["[api] api-started".to_string()]);
        assert!(rx.try_recv().is_err());
        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn start_command_without_ready_unblocks_dependents_after_spawn() {
        let (tx, mut rx) = mpsc::channel(16);
        let db = start_command("db", &["/bin/sh", "-c", "sleep 1"]);
        let mut api = start_command("api", &["/bin/sh", "-c", "echo api-started"]);
        api.wait_for = command(&["db"]);

        let mut children = spawn_start_commands(&[db, api], tx).unwrap();
        let lines = recv_lines(&mut rx, 1);

        cleanup_children(&mut children);
        assert_eq!(lines, vec!["[api] api-started".to_string()]);
    }

    #[test]
    fn start_command_ready_timeout_kills_started_children() {
        let cwd = temp_dir("ready-timeout");
        let pid_file = cwd.join("pid");
        let (tx, _rx) = mpsc::channel(16);
        let mut db = start_command(
            "db",
            &[
                "/bin/sh",
                "-c",
                &format!("echo $$ > {}; sleep 5", pid_file.display()),
            ],
        );
        db.ready = Some(ReadySpec::Line {
            text: "never-ready".to_string(),
            timeout: Duration::from_millis(100),
        });

        let error = spawn_start_commands(&[db], tx).unwrap_err();
        let pid = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();

        assert!(error.to_string().contains("readiness timed out"));
        assert!(!process_is_running(pid));
        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn start_command_dependency_exit_before_ready_fails() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut db = start_command("db", &["/bin/sh", "-c", "exit 7"]);
        db.ready = Some(ReadySpec::Line {
            text: "ready".to_string(),
            timeout: Duration::from_secs(2),
        });
        let mut api = start_command("api", &["/bin/sh", "-c", "echo api-started"]);
        api.wait_for = command(&["db"]);

        let error = spawn_start_commands(&[db, api], tx).unwrap_err();

        assert!(error.to_string().contains("exited before readiness"));
        assert!(recv_lines(&mut rx, 1).is_empty());
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
