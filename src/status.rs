//! Whether a `rallypoint watch` is running, under any account, for `rallypoint status`.

use crate::apps;
use crate::error::Error;
use crate::process::{Processes, Started};

const EXECUTABLE: &str = "rallypoint.exe";

/// The pids of every running `rallypoint watch`, oldest first.
pub fn watchers() -> Result<Vec<u32>, Error> {
    let mut pids: Vec<u32> = Vec::new();
    for process in Processes::connect()?.named(EXECUTABLE)? {
        if is_watch(&process)? {
            pids.push(process.pid);
        }
    }
    Ok(pids)
}

/// Whether `process` runs the `watch` command. A rallypoint whose command line WMI withholds cannot be told apart, so
/// it is an error rather than a guess.
fn is_watch(process: &Started) -> Result<bool, Error> {
    match (&process.executable_path, &process.command_line) {
        (Some(executable_path), Some(command_line)) => {
            Ok(apps::arguments_of(command_line, executable_path) == "watch")
        }
        _ => Err(Error::CommandLineHidden { pid: process.pid }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExePath;

    fn rallypoint(command_line: Option<&str>) -> Started {
        Started {
            pid: 7,
            executable_path: Some(ExePath::new(r"C:\tools\rallypoint.exe".to_string())),
            command_line: command_line.map(str::to_string),
        }
    }

    #[test]
    fn is_watch_tells_watch_from_other_commands() -> Result<(), Error> {
        assert!(is_watch(&rallypoint(Some(
            r#""C:\tools\rallypoint.exe" watch"#
        )))?);
        assert!(is_watch(&rallypoint(Some("rallypoint.exe watch")))?);
        assert!(!is_watch(&rallypoint(Some("rallypoint.exe status")))?);
        assert!(!is_watch(&rallypoint(Some(
            "rallypoint.exe restore watch.json"
        )))?);
        Ok(())
    }

    #[test]
    fn is_watch_refuses_a_hidden_command_line() {
        assert!(matches!(
            is_watch(&rallypoint(None)),
            Err(Error::CommandLineHidden { pid: 7 })
        ));
    }
}
