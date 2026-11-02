//! A helper must acknowledge readiness before a deadline. A partial line cannot block the worker.

use std::io::{BufRead, BufReader, Read};
use std::process::Child;
use std::sync::mpsc;
use std::time::Duration;

pub(super) fn wait(child: &mut Child, timeout: Duration) -> Result<(), String> {
    let result = receive(child, timeout);
    if result.is_err() {
        stop(child);
    }
    result
}

fn receive(child: &mut Child, timeout: Duration) -> Result<(), String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "The update helper has no output pipe".to_owned())?;
    receive_reader(stdout, timeout)
}

fn receive_reader(stdout: impl Read + Send + 'static, timeout: Duration) -> Result<(), String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("alttabio-update-ready".to_owned())
        .spawn(move || {
            let mut line = String::new();
            let reply = BufReader::new(stdout)
                .take(64)
                .read_line(&mut line)
                .map_err(|error| format!("Could not read the update helper's reply: {error}"))
                .and_then(|_| {
                    if line.ends_with('\n') && line.trim() == "READY" {
                        Ok(())
                    } else {
                        Err("The update helper did not acknowledge readiness".to_owned())
                    }
                });
            // A deadline may have expired and closed the receiver while this read was unwinding.
            if sender.send(reply).is_err() {
                eprintln!("The update helper reported after its readiness wait ended");
            }
        })
        .map_err(|error| format!("Could not start the update readiness reader: {error}"))?;
    receiver
        .recv_timeout(timeout)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => {
                "The update helper timed out before acknowledging readiness".to_owned()
            }
            mpsc::RecvTimeoutError::Disconnected => {
                "The update readiness reader stopped unexpectedly".to_owned()
            }
        })?
}

fn stop(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(error) => eprintln!("Could not inspect the failed update helper: {error}"),
    }
    // This process is the helper we just created, never the running AltTabio app. Terminating
    // it closes its output pipe so a timed-out reader can finish without blocking the worker.
    match child.kill() {
        Ok(()) => {
            if let Err(error) = child.wait() {
                eprintln!("Could not reap the failed update helper: {error}");
            }
        }
        Err(error) => eprintln!("Could not stop the failed update helper: {error}"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn readiness_requires_a_complete_line_and_has_a_deadline() {
        for prefix in [b"".as_slice(), b"REA"] {
            let (reader, mut writer) = std::io::pipe().unwrap_or_else(|error| panic!("{error}"));
            writer
                .write_all(prefix)
                .unwrap_or_else(|error| panic!("{error}"));
            assert!(
                receive_reader(reader, Duration::from_millis(120))
                    .is_err_and(|error| error.contains("timed out"))
            );
            drop(writer);
        }
        assert_eq!(
            receive_reader(std::io::Cursor::new(b"READY\n"), Duration::from_secs(1)),
            Ok(())
        );
        for text in [b"NOT READY\n".as_slice(), b"READY", b""] {
            assert!(receive_reader(std::io::Cursor::new(text), Duration::from_secs(1)).is_err());
        }
    }
}
