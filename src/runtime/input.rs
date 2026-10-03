use std::{
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{self, BufRead, IsTerminal, Read},
    os::fd::FromRawFd,
    os::unix::process::CommandExt,
    path::Path,
    process::{Child as ProcessChild, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use super::{NamedCommand, ReadySpec, StartCommand, StartPlan};

pub(super) const LINE_CHANNEL_CAPACITY: usize = 1024;

/// Own the process group as well as its leader, including across early returns.
/// Waiting for the leader alone does not guarantee that its descendants exited.
#[derive(Debug)]
pub(super) struct Child {
    process: ProcessChild,
}

impl std::ops::Deref for Child {
    type Target = ProcessChild;

    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

impl std::ops::DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        force_kill_child_group(self);
    }
}

pub(super) fn stdin_is_terminal() -> bool {
    io::stdin().is_terminal()
}

pub(super) fn spawn_stdin_reader(tx: mpsc::SyncSender<String>) -> io::Result<()> {
    let input = prepare_terminal_input()?;
    spawn_line_reader(input, tx, LineReaderConfig::default());
    Ok(())
}

pub(super) fn spawn_command(command: &[String], tx: mpsc::SyncSender<String>) -> io::Result<Child> {
    let mut child = spawn_child(command, None)?;
    spawn_output_readers(&mut child, tx, LineReaderConfig::default());

    Ok(child)
}

pub(super) fn spawn_named_commands(
    commands: &[NamedCommand],
    tx: mpsc::SyncSender<String>,
) -> io::Result<Vec<Child>> {
    let mut children = Vec::with_capacity(commands.len());

    for command in commands {
        children.push(spawn_named_command(command, tx.clone())?);
    }

    Ok(children)
}

/// Drives a scheduler to completion. The event loop ticks it instead, so the
/// terminal stays interactive while commands wait on readiness.
#[cfg(test)]
pub(super) fn spawn_start_commands(
    commands: &[StartCommand],
    tx: mpsc::SyncSender<String>,
) -> io::Result<Vec<Child>> {
    let mut scheduler = StartScheduler::new(commands.to_vec(), tx)?;
    let mut children = Vec::new();

    while scheduler.tick(&mut children, Instant::now())? == StartProgress::InProgress {
        thread::sleep(Duration::from_millis(10));
    }

    Ok(children)
}

fn spawn_named_command(command: &NamedCommand, tx: mpsc::SyncSender<String>) -> io::Result<Child> {
    let mut child = spawn_child(&command.command, command.cwd.as_deref())?;
    spawn_output_readers(
        &mut child,
        tx,
        LineReaderConfig::with_source(command.name.clone()),
    );

    Ok(child)
}

fn spawn_start_command(
    command: &StartCommand,
    tx: mpsc::SyncSender<String>,
) -> io::Result<SpawnedStartCommand> {
    let mut child = spawn_child_with_env(&command.argv, command.cwd.as_deref(), &command.env)?;
    let ready_line = match &command.ready {
        Some(ReadySpec::Line { text, .. }) => Some(text.clone()),
        _ => None,
    };
    let (ready_tx, ready_rx) = ready_line
        .as_ref()
        .map(|_| mpsc::channel())
        .map(|(tx, rx)| (Some(tx), Some(rx)))
        .unwrap_or((None, None));

    spawn_output_readers(
        &mut child,
        tx,
        LineReaderConfig::with_source_and_ready(command.name.clone(), ready_line, ready_tx),
    );

    Ok(SpawnedStartCommand {
        child,
        line_ready_rx: ready_rx,
    })
}

fn spawn_child(command: &[String], cwd: Option<&Path>) -> io::Result<Child> {
    spawn_child_with_env(command, cwd, &BTreeMap::new())
}

fn spawn_child_with_env(
    command: &[String],
    cwd: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> io::Result<Child> {
    let mut command_builder = Command::new(&command[0]);
    command_builder
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(cwd) = cwd {
        command_builder.current_dir(cwd);
    }
    command_builder.envs(env);

    unsafe {
        command_builder.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    command_builder.spawn().map(|process| Child { process })
}

fn spawn_probe(
    command: &[String],
    cwd: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> io::Result<Child> {
    spawn_child_with_env(command, cwd, env)
}

fn prepare_terminal_input() -> io::Result<File> {
    let stdin_fd = unsafe { libc::dup(libc::STDIN_FILENO) };
    if stdin_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(unsafe { File::from_raw_fd(stdin_fd) })
}

#[derive(Clone, Default)]
struct LineReaderConfig {
    source: Option<String>,
    ready_line: Option<String>,
    ready_tx: Option<mpsc::Sender<()>>,
}

impl LineReaderConfig {
    fn with_source(source: String) -> Self {
        Self {
            source: Some(source),
            ready_line: None,
            ready_tx: None,
        }
    }

    fn with_source_and_ready(
        source: String,
        ready_line: Option<String>,
        ready_tx: Option<mpsc::Sender<()>>,
    ) -> Self {
        Self {
            source: Some(source),
            ready_line,
            ready_tx,
        }
    }
}

fn spawn_output_readers(child: &mut Child, tx: mpsc::SyncSender<String>, config: LineReaderConfig) {
    if let Some(stdout) = child.stdout.take() {
        spawn_line_reader(stdout, tx.clone(), config.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_line_reader(stderr, tx, config);
    }
}

fn spawn_line_reader<R>(input: R, tx: mpsc::SyncSender<String>, config: LineReaderConfig)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_lines(input, tx, config));
}

fn read_lines<R>(input: R, tx: mpsc::SyncSender<String>, config: LineReaderConfig)
where
    R: Read,
{
    let reader = io::BufReader::new(input);
    let mut signaled_ready = false;

    for line in reader.lines() {
        match line {
            Ok(line) => {
                let matches_ready = config
                    .ready_line
                    .as_ref()
                    .is_some_and(|ready_line| line.contains(ready_line));
                let line = config
                    .source
                    .as_ref()
                    .map(|source| format!("[{source}] {line}"))
                    .unwrap_or(line);

                if tx.send(line).is_err() {
                    break;
                }

                if matches_ready && !signaled_ready {
                    if let Some(ready_tx) = &config.ready_tx {
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
    line_ready_rx: Option<mpsc::Receiver<()>>,
}

/// Starts `loggle start` commands in dependency order without blocking. The
/// event loop ticks it so the terminal stays interactive while commands wait on
/// readiness.
pub(super) struct StartScheduler {
    commands: Vec<StartCommand>,
    dependencies: Vec<Vec<usize>>,
    tx: mpsc::SyncSender<String>,
    states: Vec<StartState>,
    /// Index of each spawned command in the caller's `children`.
    child_slots: Vec<Option<usize>>,
    line_ready: Vec<Option<mpsc::Receiver<()>>>,
    command_ready: Vec<Option<CommandReadyState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartProgress {
    InProgress,
    Ready,
}

impl StartScheduler {
    pub(super) fn new(
        commands: Vec<StartCommand>,
        tx: mpsc::SyncSender<String>,
    ) -> io::Result<Self> {
        let plan =
            StartPlan::new(&commands).map_err(|error| io::Error::other(error.to_string()))?;
        let dependencies = (0..commands.len())
            .map(|index| plan.dependency_indexes(index).collect())
            .collect();
        let len = commands.len();

        Ok(Self {
            commands,
            dependencies,
            tx,
            states: vec![StartState::Pending; len],
            child_slots: vec![None; len],
            line_ready: (0..len).map(|_| None).collect(),
            command_ready: (0..len).map(|_| None).collect(),
        })
    }

    /// Spawns every command whose dependencies are ready and polls readiness.
    /// Spawned children are pushed onto `children`, so the caller's shutdown
    /// path owns them from the moment they start; the caller must not reorder
    /// or remove them while the scheduler is in progress.
    pub(super) fn tick(
        &mut self,
        children: &mut Vec<Child>,
        now: Instant,
    ) -> io::Result<StartProgress> {
        loop {
            let mut progressed = self.spawn_unblocked(children, now)?;
            progressed |= self.check_readiness(children, now)?;

            if !progressed {
                break;
            }
        }

        if self.all_ready() {
            Ok(StartProgress::Ready)
        } else {
            Ok(StartProgress::InProgress)
        }
    }

    /// Summarises startup for the status line, e.g.
    /// `starting: 1/3 ready; waiting for db (times out in 12s)`.
    pub(super) fn progress_notice(&self, now: Instant) -> String {
        let ready = self
            .states
            .iter()
            .filter(|state| **state == StartState::Ready)
            .count();
        let mut notice = format!("starting: {ready}/{} ready", self.commands.len());
        let waiting = (0..self.commands.len())
            .filter(|index| self.states[*index] == StartState::Started)
            .collect::<Vec<_>>();
        if waiting.is_empty() {
            return notice;
        }

        let names = waiting
            .iter()
            .map(|index| self.commands[*index].name.as_str())
            .collect::<Vec<_>>();
        notice.push_str("; waiting for ");
        notice.push_str(&names.join(", "));

        let deadline = waiting
            .iter()
            .filter_map(|index| self.command_ready[*index].as_ref())
            .map(|command_ready| command_ready.deadline)
            .min();
        if let Some(deadline) = deadline {
            let seconds = deadline
                .saturating_duration_since(now)
                .as_millis()
                .div_ceil(1000);
            notice.push_str(&format!(" (times out in {seconds}s)"));
        }

        notice
    }

    fn all_ready(&self) -> bool {
        self.states.iter().all(|state| *state == StartState::Ready)
    }

    fn spawn_unblocked(&mut self, children: &mut Vec<Child>, now: Instant) -> io::Result<bool> {
        let mut progressed = false;
        for index in 0..self.commands.len() {
            if self.states[index] != StartState::Pending || !self.dependencies_ready(index) {
                continue;
            }

            self.spawn_command(index, children, now)?;
            progressed = true;
        }

        Ok(progressed)
    }

    fn dependencies_ready(&self, index: usize) -> bool {
        self.dependencies[index]
            .iter()
            .all(|dependency_index| self.states[*dependency_index] == StartState::Ready)
    }

    fn spawn_command(
        &mut self,
        index: usize,
        children: &mut Vec<Child>,
        now: Instant,
    ) -> io::Result<()> {
        let command = &self.commands[index];
        let spawned = spawn_start_command(command, self.tx.clone())?;

        self.child_slots[index] = Some(children.len());
        children.push(spawned.child);
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

    fn check_readiness(&mut self, children: &mut [Child], now: Instant) -> io::Result<bool> {
        let mut progressed = false;

        for index in 0..self.commands.len() {
            if self.states[index] != StartState::Started {
                continue;
            }

            let command = &self.commands[index];

            if self.line_ready[index]
                .as_ref()
                .is_some_and(|rx| rx.try_recv().is_ok())
            {
                self.states[index] = StartState::Ready;
                progressed = true;
                continue;
            }

            if let Some(command_ready) = self.command_ready[index].as_mut() {
                let probe_outcome =
                    command_ready.poll_probe(command.cwd.as_deref(), &command.env, now)?;
                if let ProbeOutcome::Ready = probe_outcome {
                    self.states[index] = StartState::Ready;
                    progressed = true;
                    continue;
                }

                if now >= command_ready.deadline {
                    return Err(command_ready.timeout_error(&command.name));
                }
            }

            if let Some(child) = self.child_slots[index].map(|slot| &mut children[slot])
                && let Some(status) = child.try_wait()?
            {
                input_reap_child(child);
                let message = format!(
                    "command '{}' exited before readiness{}",
                    command.name,
                    status
                        .code()
                        .map(|code| format!(" with status {code}"))
                        .unwrap_or_default()
                );
                return Err(io::Error::other(message));
            }
        }

        Ok(progressed)
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
                running: None,
            },
            deadline,
            recent_output: RecentProbeOutput::new(),
        }
    }

    /// Advances the readiness probe one step without blocking: starts it when
    /// due, or collects its result once it has finished. A probe still running
    /// at the deadline is killed and reported as timed out.
    fn poll_probe(
        &mut self,
        cwd: Option<&Path>,
        env: &BTreeMap<String, String>,
        now: Instant,
    ) -> io::Result<ProbeOutcome> {
        let ReadyKind::Command {
            command,
            interval,
            next_probe,
            running,
        } = &mut self.kind
        else {
            return Ok(ProbeOutcome::NotReady);
        };

        let Some(mut probe) = running.take() else {
            if now >= *next_probe {
                *running = Some(RunningProbe::spawn(command, cwd, env)?);
            }
            return Ok(ProbeOutcome::NotReady);
        };

        if !probe.has_finished()? && now < self.deadline {
            *running = Some(probe);
            return Ok(ProbeOutcome::NotReady);
        }

        let probe = probe.finish();
        *next_probe = now + *interval;
        self.recent_output.push(probe.output_summary());

        if probe.success {
            return Ok(ProbeOutcome::Ready);
        }
        if probe.timed_out {
            return Err(self.timeout_error_for_output("readiness probe timed out"));
        }

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
            io::Error::other(format!(
                "{message}\nrecent readiness probe output:\n{output}"
            ))
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
        running: Option<RunningProbe>,
    },
}

#[derive(Debug)]
enum ProbeOutcome {
    Ready,
    NotReady,
}

/// A readiness probe in flight. Its output is read on background threads so
/// a chatty probe cannot block on a full pipe while the event loop polls it.
#[derive(Debug)]
struct RunningProbe {
    child: Child,
    status: Option<ExitStatus>,
    stdout: thread::JoinHandle<Vec<u8>>,
    stderr: thread::JoinHandle<Vec<u8>>,
}

impl RunningProbe {
    fn spawn(
        command: &[String],
        cwd: Option<&Path>,
        env: &BTreeMap<String, String>,
    ) -> io::Result<Self> {
        let mut child = spawn_probe(command, cwd, env)?;
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

        Ok(Self {
            child,
            status: None,
            stdout,
            stderr,
        })
    }

    /// The probe has finished once it has exited and its output is fully read.
    fn has_finished(&mut self) -> io::Result<bool> {
        if self.status.is_none() {
            self.status = self.child.try_wait()?;
        }

        Ok(self.status.is_some() && self.stdout.is_finished() && self.stderr.is_finished())
    }

    /// Kills the probe's process group, which also closes the output pipes of
    /// any descendant it left behind, and collects what it wrote.
    fn finish(mut self) -> ProbeRun {
        force_kill_child_group(&mut self.child);

        ProbeRun {
            success: self.status.is_some_and(|status| status.success()),
            timed_out: self.status.is_none(),
            stdout: probe_output(self.stdout),
            stderr: probe_output(self.stderr),
        }
    }
}

/// A descendant that left the probe's process group can hold its pipe open
/// indefinitely; give up on that output rather than stall the event loop.
fn probe_output(reader: thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_millis(100);
    while !reader.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }

    if reader.is_finished() {
        reader.join().unwrap_or_default()
    } else {
        Vec::new()
    }
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
        let path =
            std::env::temp_dir().join(format!("loggle-runtime-test-{}-{name}", std::process::id()));
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
            env: BTreeMap::new(),
            wait_for: Vec::new(),
            ready: None,
        }
    }

    fn recv_lines(rx: &mpsc::Receiver<String>, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut lines = Vec::new();

        while lines.len() < count && Instant::now() < deadline {
            match rx.try_recv() {
                Ok(line) => lines.push(line),
                Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(10)),
                Err(mpsc::TryRecvError::Disconnected) => break,
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
        let (tx, rx) = mpsc::sync_channel(4);

        read_lines("one\ntwo\n".as_bytes(), tx, LineReaderConfig::default());

        assert_eq!(rx.recv().ok(), Some("one".to_string()));
        assert_eq!(rx.recv().ok(), Some("two".to_string()));
        assert_eq!(rx.recv().ok(), None);
    }

    #[test]
    fn prefixed_line_reader_marks_each_line_with_source_name() {
        let (tx, rx) = mpsc::sync_channel(4);

        read_lines(
            "one\ntwo\n".as_bytes(),
            tx,
            LineReaderConfig::with_source("api".to_string()),
        );

        assert_eq!(rx.recv().ok(), Some("[api] one".to_string()));
        assert_eq!(rx.recv().ok(), Some("[api] two".to_string()));
        assert_eq!(rx.recv().ok(), None);
    }

    #[test]
    fn named_commands_run_from_configured_cwd_and_keep_source_prefix() {
        let cwd = temp_dir("cwd");
        fs::write(cwd.join("marker"), "").unwrap();
        let (tx, rx) = mpsc::sync_channel(4);
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

        assert_eq!(rx.recv().ok(), Some("[api] cwd-ok".to_string()));
        assert!(children.pop().unwrap().wait().unwrap().success());
        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn start_commands_wait_for_ready_line_before_starting_dependents() {
        let (tx, rx) = mpsc::sync_channel(16);
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
        let lines = recv_lines(&rx, 3);

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
        let (tx, rx) = mpsc::sync_channel(16);
        let mut db = start_command("db", &["/bin/sh", "-c", "sleep 0.1; touch ready; sleep 1"]);
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
        let lines = recv_lines(&rx, 1);

        cleanup_children(&mut children);
        assert_eq!(lines, vec!["[api] api-started".to_string()]);
        assert!(rx.try_recv().is_err());
        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn start_command_without_ready_unblocks_dependents_after_spawn() {
        let (tx, rx) = mpsc::sync_channel(16);
        let db = start_command("db", &["/bin/sh", "-c", "sleep 1"]);
        let mut api = start_command("api", &["/bin/sh", "-c", "echo api-started"]);
        api.wait_for = command(&["db"]);

        let mut children = spawn_start_commands(&[db, api], tx).unwrap();
        let lines = recv_lines(&rx, 1);

        cleanup_children(&mut children);
        assert_eq!(lines, vec!["[api] api-started".to_string()]);
    }

    #[test]
    fn start_commands_apply_configured_env() {
        let (tx, rx) = mpsc::sync_channel(16);
        let mut api = start_command("api", &["/bin/sh", "-c", "echo env=$LOGGLE_TEST_ENV"]);
        api.env
            .insert("LOGGLE_TEST_ENV".to_string(), "configured".to_string());

        let mut children = spawn_start_commands(&[api], tx).unwrap();
        let lines = recv_lines(&rx, 1);

        cleanup_children(&mut children);
        assert_eq!(lines, vec!["[api] env=configured".to_string()]);
    }

    #[test]
    fn ready_command_uses_configured_env() {
        let (tx, rx) = mpsc::sync_channel(16);
        let mut db = start_command("db", &["/bin/sh", "-c", "sleep 1"]);
        db.env.insert("LOGGLE_READY".to_string(), "yes".to_string());
        db.ready = Some(ReadySpec::Command {
            command: command(&["/bin/sh", "-c", "test \"$LOGGLE_READY\" = yes"]),
            interval: Duration::from_millis(25),
            timeout: Duration::from_secs(2),
        });
        let mut api = start_command("api", &["/bin/sh", "-c", "echo api-started"]);
        api.wait_for = command(&["db"]);

        let mut children = spawn_start_commands(&[db, api], tx).unwrap();
        let lines = recv_lines(&rx, 1);

        cleanup_children(&mut children);
        assert_eq!(lines, vec!["[api] api-started".to_string()]);
    }

    #[test]
    fn hanging_ready_probe_does_not_block_tick() {
        let (tx, _rx) = mpsc::sync_channel(16);
        let mut db = start_command("db", &["/bin/sh", "-c", "sleep 5"]);
        db.ready = Some(ReadySpec::Command {
            command: command(&["/bin/sh", "-c", "echo probing; sleep 5"]),
            interval: Duration::from_millis(25),
            timeout: Duration::from_millis(300),
        });
        let api = start_command("api", &["/bin/sh", "-c", "sleep 5"]);
        let mut scheduler = StartScheduler::new(vec![db, api], tx).unwrap();
        let mut children = Vec::new();
        let started = Instant::now();

        let progress = scheduler.tick(&mut children, started).unwrap();

        assert_eq!(progress, StartProgress::InProgress);
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(children.len(), 2);
        assert_eq!(
            scheduler.progress_notice(started),
            "starting: 1/2 ready; waiting for db (times out in 1s)"
        );

        let error = loop {
            match scheduler.tick(&mut children, Instant::now()) {
                Ok(progress) => assert_eq!(progress, StartProgress::InProgress),
                Err(error) => break error,
            }
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(10));
        };

        cleanup_children(&mut children);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            error.to_string(),
            "readiness probe timed out\nrecent readiness probe output:\nstdout:\nprobing"
        );
    }

    #[test]
    fn start_command_ready_timeout_kills_started_children() {
        let cwd = temp_dir("ready-timeout");
        let pid_file = cwd.join("pid");
        let (tx, _rx) = mpsc::sync_channel(16);
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
        let (tx, rx) = mpsc::sync_channel(16);
        let mut db = start_command("db", &["/bin/sh", "-c", "exit 7"]);
        db.ready = Some(ReadySpec::Line {
            text: "ready".to_string(),
            timeout: Duration::from_secs(2),
        });
        let mut api = start_command("api", &["/bin/sh", "-c", "echo api-started"]);
        api.wait_for = command(&["db"]);

        let error = spawn_start_commands(&[db, api], tx).unwrap_err();

        assert!(error.to_string().contains("exited before readiness"));
        assert!(recv_lines(&rx, 1).is_empty());
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
        assert_eq!(
            shutdown.status(),
            ShutdownStatus::Waiting(ShutdownSignal::Terminate)
        );

        shutdown.escalate(now + interrupt_timeout() + terminate_timeout());
        assert_eq!(
            shutdown.status(),
            ShutdownStatus::Waiting(ShutdownSignal::Kill)
        );
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

        assert_eq!(
            shutdown.status(),
            ShutdownStatus::Waiting(ShutdownSignal::Terminate)
        );
    }

    #[test]
    fn shutdown_interrupt_stops_spawned_process_group() {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "trap 'exit 42' INT; echo ready; while true; do sleep 1; done",
            ])
            .stdout(Stdio::piped())
            .process_group(0);
        // An ignored SIGINT survives exec; a shell cannot install an INT trap
        // in that case. Set this fixture's disposition without changing the
        // multithreaded test runner's process-wide signal handlers.
        unsafe {
            command.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                Ok(())
            });
        }
        let mut child = Child {
            process: command.spawn().unwrap(),
        };
        let mut ready = String::new();
        io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        let mut shutdown = ChildShutdown::start(&child, Instant::now());
        let deadline = Instant::now() + Duration::from_secs(3);

        while Instant::now() < deadline {
            if shutdown.tick(&mut child, Instant::now()).unwrap() == ShutdownStatus::Exited {
                assert_eq!(child.wait().unwrap().code(), Some(42));
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }

        force_kill_child_group(&mut child);
        panic!("spawned process group did not exit after SIGINT");
    }
}
