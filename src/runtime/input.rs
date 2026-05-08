use std::{
    fs::File,
    io::{self, BufRead, IsTerminal, Read},
    os::fd::FromRawFd,
    process::{Child, Command, Stdio},
    thread,
};

use tokio::sync::mpsc;

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

pub(super) fn terminate_child(child: &mut Child) {
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
}
