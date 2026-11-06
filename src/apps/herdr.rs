//! herdr, a terminal workspace manager for AI coding agents. Its server saves its own workspaces and resumes their
//! agent sessions when it starts, so a pane that ran herdr needs only herdr started again with the same arguments.

use std::iter;

use crate::apps::arguments_of;
use crate::error::Error;
use crate::process::Processes;

/// herdr's client, the process a terminal pane runs.
const EXECUTABLE: &str = "herdr.exe";

/// The arguments of the herdr client that process `pid` started, as a wezterm-gui process does for a pane running it,
/// such as `--remote <host>` for a client attached to a server over ssh. None when it started none.
pub fn client_arguments(processes: &Processes, pid: u32) -> Result<Option<Vec<String>>, Error> {
    let Some(client) = processes
        .children_named(pid, EXECUTABLE)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    match (&client.executable_path, &client.command_line) {
        // herdr's options take no value holding a space, so splitting on whitespace keeps each one whole.
        (Some(executable_path), Some(command_line)) => Ok(Some(
            arguments_of(command_line, executable_path)
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        )),
        _ => Err(Error::HerdrCommandLineHidden { pid: client.pid }),
    }
}

/// The command that starts herdr with `arguments`, by name through the owner's PATH, since the running client's path
/// names the release folder of one herdr version.
pub fn command(arguments: &[String]) -> Vec<String> {
    iter::once("herdr".to_string())
        .chain(arguments.iter().cloned())
        .collect()
}
