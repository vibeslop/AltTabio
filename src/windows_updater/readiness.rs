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

    fn helper(script: &str) -> Child {
        super::super::powershell(
            script,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("target"),
        )
        .unwrap_or_else(|error| panic!("{error}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn missing_or_incomplete_ready_lines_time_out_and_stop_the_helper() {
        for script in [
            "Start-Sleep -Seconds 30",
            "[Console]::Out.Write('REA'); [Console]::Out.Flush(); Start-Sleep -Seconds 30",
        ] {
            let mut child = helper(script);
            let result = wait(&mut child, Duration::from_millis(120));
            assert!(result.is_err_and(|error| error.contains("timed out")));
            assert!(
                child.try_wait().is_ok_and(|status| status.is_some()),
                "the timed-out helper must be reaped"
            );
        }
    }

    #[test]
    fn a_ready_line_can_arrive_in_pieces() {
        let mut child = helper(
            "[Console]::Out.Write('REA'); [Console]::Out.Flush(); Start-Sleep -Milliseconds 40; [Console]::Out.WriteLine('DY'); [Console]::Out.Flush(); Start-Sleep -Seconds 30",
        );
        let result = wait(&mut child, Duration::from_secs(5));
        stop(&mut child);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn an_invalid_reply_is_rejected_without_waiting_for_the_deadline() {
        let mut child = helper(
            "[Console]::Out.WriteLine('NOT READY'); [Console]::Out.Flush(); Start-Sleep -Seconds 30",
        );
        let result = wait(&mut child, Duration::from_secs(5));
        assert!(result.is_err_and(|error| error.contains("did not acknowledge")));
        assert!(child.try_wait().is_ok_and(|status| status.is_some()));
    }
}
