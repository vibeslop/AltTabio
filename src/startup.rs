use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutostartStatus {
    pub enabled: bool,
    pub task_exists: bool,
}

pub fn status() -> Result<AutostartStatus, String> {
    let task_query = run(&["/Query", "/TN", "AltTabio", "/XML"])?;
    if task_query.status.success() {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
        let xml = String::from_utf8(task_query.stdout)
            .map_err(|error| format!("Autostart task XML is not valid UTF-8: {error}"))?;
        return Ok(AutostartStatus {
            enabled: task_targets_executable(&xml, &executable)?,
            task_exists: true,
        });
    }

    // schtasks uses the same nonzero status for a missing task and operational failures. A general
    // query distinguishes an absent named task from an unavailable or inaccessible scheduler
    // without parsing localized error text.
    let scheduler_query = run(&["/Query", "/FO", "CSV", "/NH"])?;
    if scheduler_query.status.success() {
        Ok(AutostartStatus {
            enabled: false,
            task_exists: false,
        })
    } else {
        Err(format!(
            "Autostart status query failed: {}",
            output_details(&task_query)
        ))
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let output = if enabled {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the AltTabio executable: {error}"))?;
        run_owned(create_arguments(&executable))?
    } else {
        if !status()?.task_exists {
            return Ok(());
        }
        run(&["/Delete", "/TN", "AltTabio", "/F"])?
    };
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Autostart task update failed: {}",
            output_details(&output)
        ))
    }
}

fn task_targets_executable(xml: &str, executable: &Path) -> Result<bool, String> {
    let settings = element_contents(xml, "Settings")
        .ok_or_else(|| "Autostart task XML has no Settings element".to_owned())?;
    let enabled = element_contents(settings, "Enabled").map_or(Ok(true), |value| {
        match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            value => Err(format!(
                "Autostart task XML has an invalid Enabled value: {value}"
            )),
        }
    })?;
    if !enabled {
        return Ok(false);
    }

    let action = element_contents(xml, "Exec")
        .and_then(|action| element_contents(action, "Command"))
        .ok_or_else(|| "Autostart task XML has no executable command".to_owned())?;
    let action = decode_xml_text(action.trim().trim_matches('"'));
    Ok(paths_match(&PathBuf::from(action), executable))
}

fn element_contents<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let start = xml.find(&opening)? + opening.len();
    let end = xml.get(start..)?.find(&closing)? + start;
    xml.get(start..end)
}

fn decode_xml_text(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn paths_match(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        path.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('/', r"\")
    }

    normalized(left).eq_ignore_ascii_case(&normalized(right))
}

fn create_arguments(executable: &Path) -> Vec<OsString> {
    vec![
        "/Create".into(),
        "/TN".into(),
        "AltTabio".into(),
        "/SC".into(),
        "ONLOGON".into(),
        "/RL".into(),
        "HIGHEST".into(),
        "/TR".into(),
        format!("\"{}\"", executable.display()).into(),
        "/F".into(),
    ]
}

fn run(arguments: &[&str]) -> Result<Output, String> {
    run_owned(arguments.iter().map(OsString::from))
}

fn run_owned(arguments: impl IntoIterator<Item = OsString>) -> Result<Output, String> {
    use std::os::windows::process::CommandExt;

    Command::new("schtasks.exe")
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("Could not run schtasks.exe: {error}"))
}

fn output_details(output: &Output) -> String {
    let details = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    let details = String::from_utf8_lossy(details);
    let details = details.trim();
    if details.is_empty() {
        format!("schtasks.exe exited with {}", output.status)
    } else {
        details.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_arguments_quote_the_executable_and_request_highest_on_logon() {
        let arguments = create_arguments(Path::new(r"C:\Program Files\AltTabio\AltTabio.exe"));
        let arguments = arguments
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(arguments.windows(2).any(|pair| pair == ["/SC", "ONLOGON"]));
        assert!(arguments.windows(2).any(|pair| pair == ["/RL", "HIGHEST"]));
        assert!(
            arguments
                .iter()
                .any(|value| { value.as_ref() == r#""C:\Program Files\AltTabio\AltTabio.exe""# })
        );
    }

    #[test]
    fn task_status_requires_an_enabled_task_targeting_the_current_executable() {
        let executable = Path::new(r"C:\Apps & Tools\AltTabio.exe");
        let matching = r"<Task><Settings></Settings><Actions><Exec><Command>C:\Apps &amp; Tools\AltTabio.exe</Command></Exec></Actions></Task>";
        let disabled = r"<Task><Settings><Enabled>false</Enabled></Settings><Actions><Exec><Command>C:\Apps &amp; Tools\AltTabio.exe</Command></Exec></Actions></Task>";
        let stale = r"<Task><Settings><Enabled>true</Enabled></Settings><Actions><Exec><Command>C:\Old\AltTabio.exe</Command></Exec></Actions></Task>";

        assert_eq!(task_targets_executable(matching, executable), Ok(true));
        assert_eq!(task_targets_executable(disabled, executable), Ok(false));
        assert_eq!(task_targets_executable(stale, executable), Ok(false));
    }

    #[test]
    fn malformed_task_status_is_reported_instead_of_assumed_enabled() {
        assert!(
            task_targets_executable("<Task><Settings></Settings></Task>", Path::new("app.exe"))
                .is_err()
        );
        assert!(
            task_targets_executable(
                "<Task><Settings><Enabled>maybe</Enabled></Settings></Task>",
                Path::new("app.exe")
            )
            .is_err()
        );
    }
}
