//! Bounded capture for commands whose output comes from untrusted data.

use std::io::{self, Read};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;

/// Captures both pipes concurrently, stopping and reaping the child on a read or size error.
/// The command must enforce its own deadline and must not leave descendants holding its pipes.
///
/// # Errors
/// Returns an error if starting, reading, or waiting fails, or either byte limit is exceeded.
pub fn bounded(
    command: &mut Command,
    stdout_limit: usize,
    stderr_limit: usize,
) -> io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    capture(&mut child, stdout_limit, stderr_limit)
}

fn capture(child: &mut Child, stdout_limit: usize, stderr_limit: usize) -> io::Result<Output> {
    std::thread::scope(|scope| {
        let result = (|| {
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("Missing stdout pipe"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("Missing stderr pipe"))?;
            let (sender, receiver) = mpsc::sync_channel(2);
            let stdout_sender = sender.clone();
            std::thread::Builder::new()
                .name("alttabio-stdout".to_owned())
                .spawn_scoped(scope, move || {
                    let result = read_bounded(stdout, stdout_limit, "stdout");
                    // An earlier failure on the other pipe may have ended capture already.
                    let _delivered = stdout_sender.send((true, result));
                })?;
            std::thread::Builder::new()
                .name("alttabio-stderr".to_owned())
                .spawn_scoped(scope, move || {
                    let result = read_bounded(stderr, stderr_limit, "stderr");
                    let _delivered = sender.send((false, result));
                })?;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            for _ in 0..2 {
                let (is_stdout, bytes) = receiver.recv().map_err(io::Error::other)?;
                if is_stdout {
                    stdout = bytes?;
                } else {
                    stderr = bytes?;
                }
            }
            Ok(Output {
                status: child.wait()?,
                stdout,
                stderr,
            })
        })();
        if let Err(error) = &result {
            // Closing the child's pipes also releases the other reader before the scope joins.
            if let Err(cleanup) = stop(child) {
                return Err(io::Error::other(format!("{error}; {cleanup}")));
            }
        }
        result
    })
}

fn stop(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_none()
        && let Err(error) = child.kill()
        && child.try_wait()?.is_none()
    {
        return Err(error);
    }
    child.wait().map(drop)
}

fn read_bounded(mut reader: impl Read, limit: usize, stream: &str) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        // Read at most one byte beyond the limit to distinguish exact size from overflow.
        let remaining = limit - bytes.len();
        let length = remaining.saturating_add(1).min(buffer.len());
        let count = match reader.read(&mut buffer[..length]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(bytes);
        }
        if count > remaining {
            return Err(io::Error::other(format!(
                "{stream} exceeded the {limit} byte limit"
            )));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn accepts_empty_and_exact_limit_but_rejects_one_byte_more() -> io::Result<()> {
        assert_eq!(read_bounded(&b""[..], 0, "test")?, b"");
        assert_eq!(read_bounded(&b"1234"[..], 4, "test")?, b"1234");
        assert!(read_bounded(&b"1"[..], 0, "test").is_err());
        assert!(read_bounded(&b"12345"[..], 4, "test").is_err());
        Ok(())
    }

    #[test]
    fn overflow_stops_reading_after_one_extra_byte() {
        let mut reader = io::repeat(1).take(1_000_000);
        assert!(read_bounded(&mut reader, 1024, "test").is_err());
        assert_eq!(reader.limit(), 1_000_000 - 1025);
    }

    #[test]
    fn read_errors_are_not_accepted_as_partial_output() {
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("read failed"))
            }
        }
        let reader = (&b"partial"[..]).chain(FailedRead);
        assert!(read_bounded(reader, 1024, "test").is_err());
    }

    fn fixture(mode: &str) -> io::Result<Command> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "command_output::tests::child_output_fixture",
                "--nocapture",
            ])
            .env("ALTTABIO_OUTPUT_FIXTURE", mode);
        Ok(command)
    }

    #[test]
    fn drains_both_pipes_and_preserves_failure_diagnostics() -> io::Result<()> {
        for (mode, success) in [("both", true), ("failure", false)] {
            let output = bounded(&mut fixture(mode)?, 512 * 1024, 512 * 1024)?;
            assert_eq!(output.status.success(), success);
            assert!(output.stdout.windows(8).any(|bytes| bytes == b"out-data"));
            assert!(output.stderr.windows(8).any(|bytes| bytes == b"err-data"));
        }
        Ok(())
    }

    #[test]
    fn overflow_on_either_pipe_stops_and_reaps_the_child() -> io::Result<()> {
        for stream in ["stdout", "stderr"] {
            let mut child = fixture(stream)?
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let result = capture(&mut child, 16 * 1024, 16 * 1024);
            assert!(result.is_err_and(|error| error.to_string().contains(stream)));
            assert!(child.try_wait()?.is_some_and(|status| !status.success()));
        }
        Ok(())
    }

    #[test]
    fn child_output_fixture() -> io::Result<()> {
        let Ok(mode) = std::env::var("ALTTABIO_OUTPUT_FIXTURE") else {
            return Ok(());
        };
        let mut stdout = io::stdout().lock();
        let mut stderr = io::stderr().lock();
        match mode.as_str() {
            "stdout" => loop {
                stdout.write_all(&[b'x'; 8192])?;
            },
            "stderr" => loop {
                stderr.write_all(&[b'x'; 8192])?;
            },
            _ => {
                // More than a pipe buffer on both streams detects serial-reader deadlocks.
                for _ in 0..16_384 {
                    stdout.write_all(b"out-data")?;
                    stderr.write_all(b"err-data")?;
                }
                if mode == "failure" {
                    std::process::exit(7);
                }
                Ok(())
            }
        }
    }
}
