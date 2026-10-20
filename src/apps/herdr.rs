//! herdr, a terminal workspace manager for AI coding agents. Its server saves its own workspaces and resumes their
//! agent sessions when it starts, so a pane that ran herdr needs only herdr started again.

use crate::error::Error;
use crate::process::Processes;

/// herdr's client, the process a terminal pane runs.
const EXECUTABLE: &str = "herdr.exe";

/// Whether process `pid` started herdr's client, as a wezterm-gui process does for a pane running it.
pub fn runs_in(processes: &Processes, pid: u32) -> Result<bool, Error> {
    Ok(!processes.children_named(pid, EXECUTABLE)?.is_empty())
}

/// The command that starts herdr, by name through the owner's PATH, since the running client's path names the
/// release folder of one herdr version.
pub fn command() -> Vec<String> {
    vec!["herdr".to_string()]
}
