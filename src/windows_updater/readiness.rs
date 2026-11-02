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
    use std::path::Path;
    use std::process::Stdio;

    struct Helper {
        child: Child,
    }

    impl Drop for Helper {
        fn drop(&mut self) {
            stop(&mut self.child);
        }
    }

    fn helper(before_start: &str, script: &str) -> Helper {
        // Keep shell startup and module imports outside the protocol deadline. Any initial
        // partial output must also be flushed before announcing that the fixture has started.
        let script = format!(
            "{before_start}\n[Console]::Error.WriteLine('STARTED'); [Console]::Error.Flush();\n{script}"
        );
        let mut helper = Helper {
            child: super::super::powershell(
                &script,
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("target"),
            )
            .unwrap_or_else(|error| panic!("{error}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("{error}")),
        };
        let stderr = helper
            .child
            .stderr
            .take()
            .unwrap_or_else(|| panic!("missing fixture stderr"));
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stderr).read_line(&mut line).map(|_| line);
            if sender.send(result).is_err() {
                eprintln!("The readiness fixture started after its startup deadline");
            }
        });
        let started = receiver.recv_timeout(Duration::from_secs(30));
        assert!(
            matches!(&started, Ok(Ok(line)) if line.trim() == "STARTED"),
            "the readiness fixture did not start: {started:?}"
        );
        reader
            .join()
            .unwrap_or_else(|_| panic!("fixture startup reader panicked"));
        helper
    }

    #[test]
    fn missing_or_incomplete_ready_lines_time_out_and_stop_the_helper() {
        for before_start in ["", "[Console]::Out.Write('REA'); [Console]::Out.Flush();"] {
            let mut helper = helper(before_start, "Start-Sleep -Seconds 30");
            let result = wait(&mut helper.child, Duration::from_millis(120));
            assert!(result.is_err_and(|error| error.contains("timed out")));
            assert!(
                helper.child.try_wait().is_ok_and(|status| status.is_some()),
                "the timed-out helper must be reaped"
            );
        }
    }

    #[test]
    fn a_ready_line_can_arrive_in_pieces() {
        let mut helper = helper(
            "[Console]::Out.Write('REA'); [Console]::Out.Flush();",
            "Start-Sleep -Milliseconds 40; [Console]::Out.WriteLine('DY'); [Console]::Out.Flush(); Start-Sleep -Seconds 30",
        );
        let result = wait(&mut helper.child, Duration::from_secs(5));
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn an_invalid_reply_is_rejected_without_waiting_for_the_deadline() {
        let mut helper = helper(
            "",
            "[Console]::Out.WriteLine('NOT READY'); [Console]::Out.Flush(); Start-Sleep -Seconds 30",
        );
        let result = wait(&mut helper.child, Duration::from_secs(5));
        assert!(result.is_err_and(|error| error.contains("did not acknowledge")));
        assert!(helper.child.try_wait().is_ok_and(|status| status.is_some()));
    }
}
