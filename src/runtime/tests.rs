//! Isolate terminal state, signal dispositions, and orphan adoption in a fresh
//! process. Linux subreaping lets the fixture prove descendants died without
//! depending on the host's PID 1 to reap their zombies.
use super::*;
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Command, Stdio},
    thread,
    time::Instant,
};

#[test]
fn no_tty_startup_reaps_process_group() {
    check_lifecycle("no-tty", false);
}

#[test]
fn no_tty_single_command_reaps_child() {
    check_lifecycle("single", false);
}

#[test]
fn partial_named_startup_reaps_child() {
    check_lifecycle("named", false);
}

#[test]
fn partial_startup_reaps_process_group() {
    check_lifecycle("partial", false);
}

#[test]
fn exited_before_readiness_terminates_descendants() {
    check_lifecycle("exited", false);
}

#[test]
fn pty_output_initialization_failure_restores_raw_mode() {
    check_lifecycle("output-error", true);
}

#[test]
fn pty_recording_failure_restores_terminal_and_reaps_group() {
    check_lifecycle("record-error", true);
}

#[test]
fn pty_quit_reaps_group_and_restores_terminal() {
    check_lifecycle("quit", true);
}

#[test]
fn pty_repeated_quit_escalates_and_reaps_group() {
    check_lifecycle("escalate", true);
}

fn termios(file: &File) -> libc::termios {
    let mut value = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::tcgetattr(file.as_raw_fd(), &mut value) }, 0);
    value
}

fn drain_pty(mut master: &File, output: &mut String) {
    let mut bytes = [0; 8192];
    while let Ok(count) = master.read(&mut bytes) {
        if count == 0 {
            break;
        }
        output.push_str(&String::from_utf8_lossy(&bytes[..count]));
    }
}

fn check_lifecycle(case: &str, with_pty: bool) {
    let cwd = std::env::temp_dir().join(format!("loggle-lifecycle-{}-{case}", std::process::id()));
    fs::create_dir_all(&cwd).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "runtime::tests::lifecycle_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("LOGGLE_LIFECYCLE_CASE", case)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let pty = with_pty.then(|| {
        let (mut master, mut slave) = (-1, -1);
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &size,
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let before = termios(&slave);
        command.stdin(slave.try_clone().unwrap());
        command.stdout(slave.try_clone().unwrap());
        unsafe {
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        }
        (master, slave, before)
    });
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                libc::signal(signal, libc::SIG_DFL);
            }
            if with_pty && libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut output = String::new();
    let mut quits = 0;
    let status = loop {
        if let Some((master, _, _)) = &pty {
            drain_pty(master, &mut output);
            let should_quit = quits == 0 && output.contains("fixture-ready");
            // Ratatui emits a cell diff, not the entire replacement message.
            let should_escalate = case == "escalate"
                && ((quits == 1 && output.contains("sent interrupt"))
                    || (quits == 2 && output.contains("children")));
            if (case == "quit" || case == "escalate") && (should_quit || should_escalate) {
                (&*master).write_all(b"q").unwrap();
                quits += 1;
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{case} timed out: {output}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    if let Some((master, _, _)) = &pty {
        drain_pty(master, &mut output);
    }
    let mut errors = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(status.success(), "{case}: {status}\n{errors}\n{output}");
    if let Some((_, slave, before)) = pty {
        let after = termios(&slave);
        assert_eq!(after.c_iflag, before.c_iflag, "{case}: input flags");
        assert_eq!(after.c_oflag, before.c_oflag, "{case}: output flags");
        assert_eq!(after.c_cflag, before.c_cflag, "{case}: control flags");
        assert_eq!(after.c_lflag, before.c_lflag, "{case}: local flags");
        assert_eq!(after.c_cc, before.c_cc, "{case}: control chars");
        if case != "output-error" {
            assert!(output.contains("\x1b[?1049h"), "alternate screen entered");
            assert!(output.contains("\x1b[?1049l"), "alternate screen restored");
            assert!(output.contains("\x1b[?25h"), "cursor restored");
        }
    }
    if case == "escalate" {
        assert_eq!(quits, 3);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
#[ignore = "subprocess fixture invoked by lifecycle tests"]
fn lifecycle_fixture() {
    let case = std::env::var("LOGGLE_LIFECYCLE_CASE").unwrap();
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let trap = if case == "escalate" {
        "trap '' INT TERM;"
    } else {
        "trap 'exit 0' INT;"
    };
    let ending = if case == "exited" {
        "exit 7"
    } else {
        "echo fixture-ready; wait"
    };
    // Single/named startup may kill the shell at any instruction. Publish the
    // PID atomically so a correctly interrupted write is not a parse failure.
    let script = format!(
        "{trap} echo $$ > leader.tmp; mv leader.tmp leader; (sleep 2; touch leaked; sleep 30) & echo $! > descendant; {ending}"
    );
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script];
    let service = StartCommand {
        name: "service".to_string(),
        argv: argv.clone(),
        cwd: None,
        env: BTreeMap::new(),
        wait_for: Vec::new(),
        ready: Some(ReadySpec::Line {
            text: "fixture-ready".to_string(),
            timeout: Duration::from_secs(3),
        }),
    };
    let input = match case.as_str() {
        "single" => RuntimeInput::Command(argv),
        "named" => RuntimeInput::Commands(vec![
            NamedCommand {
                name: "service".into(),
                command: argv,
                cwd: None,
            },
            NamedCommand {
                name: "missing".into(),
                command: vec!["/nonexistent/loggle-test".into()],
                cwd: None,
            },
        ]),
        "partial" => RuntimeInput::StartCommands(vec![
            service,
            StartCommand {
                name: "missing".into(),
                argv: vec!["/nonexistent/loggle-test".into()],
                cwd: None,
                env: BTreeMap::new(),
                wait_for: vec!["service".into()],
                ready: None,
            },
        ]),
        _ => RuntimeInput::StartCommands(vec![service]),
    };
    // Redirect only the runtime's output: libtest itself must still be able to
    // print its test list and final result to the PTY.
    let saved_stdout = (case == "output-error").then(|| {
        io::stdout().flush().unwrap();
        let saved = unsafe { libc::dup(1) };
        assert!(saved >= 0);
        let full = File::options().write(true).open("/dev/full").unwrap();
        assert_eq!(unsafe { libc::dup2(full.as_raw_fd(), 1) }, 1);
        unsafe { File::from_raw_fd(saved) }
    });
    let result = run(RuntimeConfig {
        buffer_lines: 100,
        color_enabled: false,
        source_config: SourceConfig::default(),
        input,
        record_path: (case == "record-error").then(|| PathBuf::from("missing/record.log")),
        page_id: None,
        page_command: "lifecycle fixture".into(),
        page_logging: false,
    });
    if let Some(saved) = saved_stdout {
        assert_eq!(unsafe { libc::dup2(saved.as_raw_fd(), 1) }, 1);
    }
    if case == "quit" || case == "escalate" {
        result.unwrap();
    } else {
        let RuntimeError::Io(error) = result.unwrap_err() else {
            panic!("expected an I/O startup failure");
        };
        match case.as_str() {
            "no-tty" | "single" => assert!(
                matches!(error.raw_os_error(), Some(libc::ENXIO | libc::ENOTTY)),
                "expected terminal initialization error, got {error}"
            ),
            "output-error" => assert_eq!(error.raw_os_error(), Some(libc::ENOSPC)),
            "partial" | "named" | "record-error" => {
                assert_eq!(error.kind(), io::ErrorKind::NotFound)
            }
            "exited" => assert!(error.to_string().contains("exited before readiness")),
            _ => unreachable!(),
        }
    }

    // The runtime, not this fixture, must already have reaped the direct child.
    if let Ok(pid) = fs::read_to_string("leader") {
        let pid: i32 = pid.trim().parse().unwrap();
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
    // Adopted descendants may briefly be zombies. Reap them here, but never
    // signal them: a still-running orphan is a failure of the lifecycle guard.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if pid == -1 {
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "launched process survived runtime return"
        );
        if pid == 0 {
            thread::sleep(Duration::from_millis(10));
        }
    }
    if case != "single" && case != "named" {
        assert!(
            fs::metadata("descendant").is_ok(),
            "fixture must launch descendants"
        );
    }
    if case != "quit" && case != "escalate" {
        assert!(
            !PathBuf::from("leaked").exists(),
            "descendant wrote after startup failure"
        );
    }
}
