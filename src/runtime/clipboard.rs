use std::{
    io::{self, Write},
    process::{Command, Stdio},
};

#[derive(Clone, Copy)]
struct ClipboardCommand<'a> {
    program: &'a str,
    args: &'a [&'a str],
}

pub(super) fn write(text: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        return write_to_command(
            ClipboardCommand {
                program: "pbcopy",
                args: &[],
            },
            text,
        );
    }

    #[cfg(target_os = "linux")]
    {
        return write_first_available(
            &[
                ClipboardCommand {
                    program: "wl-copy",
                    args: &[],
                },
                ClipboardCommand {
                    program: "xclip",
                    args: &["-selection", "clipboard"],
                },
                ClipboardCommand {
                    program: "xsel",
                    args: &["--clipboard", "--input"],
                },
            ],
            text,
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = text;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "clipboard copy is not supported on this platform",
        ))
    }
}

#[cfg(target_os = "linux")]
fn write_first_available(commands: &[ClipboardCommand<'_>], text: &str) -> io::Result<()> {
    let mut last_not_found = None;

    for command in commands {
        match write_to_command(*command, text) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                last_not_found = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_not_found.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no clipboard command found; install wl-copy, xclip, or xsel",
        )
    }))
}

fn write_to_command(command: ClipboardCommand<'_>, text: &str) -> io::Result<()> {
    let mut child = Command::new(command.program)
        .args(command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("{} stdin was unavailable", command.program),
        )
    })?;
    stdin.write_all(text.as_bytes())?;
    drop(stdin);

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{} exited with {status}", command.program),
        ))
    }
}
