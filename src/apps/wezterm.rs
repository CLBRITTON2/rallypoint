//! wezterm windows: their panes, and the launch that reopens one. Every wezterm-gui process runs its own mux behind
//! `<owner home>\.local\share\wezterm\gui-sock-<pid>`, so the CLI is pointed at that socket.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use percent_encoding::percent_decode_str;
use serde::Deserialize;

use crate::apps::{claude_code, quoted};
use crate::error::Error;
use crate::model::{ExePath, Owner, Resume, WeztermPane};
use crate::plan::{Key, Launch, SkipReason, Start};
use crate::process::Process;

pub const PROCESS_NAME: &str = "wezterm-gui";

/// A pane as wezterm reports it. The title is Claude Code's session title when the pane runs it.
struct LivePane {
    cwd: PathBuf,
    title: String,
}

#[derive(Deserialize)]
struct ListedPane {
    window_id: u64,
    cwd: String,
    title: String,
}

/// The panes of the wezterm window `process` draws, each with the Claude Code session it shows.
pub fn saved_panes(owner_home: &Path, process: &Process) -> Result<Vec<WeztermPane>, Error> {
    panes(&socket(owner_home, process.pid), process.pid)?
        .into_iter()
        .map(|pane| {
            let resume = claude_code::session_id(owner_home, &pane.cwd, &pane.title)?
                .map(|session_id| Resume::ClaudeCode { session_id });
            Ok(WeztermPane {
                cwd: pane.cwd,
                resume,
            })
        })
        .collect()
}

/// The key of a wezterm window, which its first pane alone tells apart.
pub fn key(
    executable_path: &ExePath,
    owner: &Owner,
    panes: &[WeztermPane],
) -> Result<Key, SkipReason> {
    let pane = panes.first().ok_or(SkipReason::NoPanes)?;
    Ok(Key::Wezterm {
        executable_path: executable_path.clone(),
        owner: owner.clone(),
        cwd: pane.cwd.clone(),
    })
}

/// Reopens a wezterm window in `cwd`, resuming the Claude Code session of its first pane.
pub fn launch(
    executable_path: &ExePath,
    owner: &Owner,
    cwd: &Path,
    panes: &[WeztermPane],
) -> Launch {
    Launch {
        owner: owner.clone(),
        program: executable_path.to_string(),
        arguments: launch_arguments(cwd, panes.first().and_then(|pane| pane.resume.as_ref())),
        start: Start::Detached,
    }
}

/// `wezterm-gui.exe` arguments opening a window in `cwd` that resumes `resume`.
fn launch_arguments(cwd: &Path, resume: Option<&Resume>) -> String {
    let resume = resume
        .map(|Resume::ClaudeCode { session_id }| {
            format!(" -- {}", claude_code::resume_command(session_id))
        })
        .unwrap_or_default();
    format!("start --cwd {}{resume}", quoted(cwd))
}

fn socket(owner_home: &Path, pid: u32) -> PathBuf {
    owner_home.join(format!(r".local\share\wezterm\gui-sock-{pid}"))
}

/// The panes of wezterm-gui process `pid`, which must hold a single window.
fn panes(socket: &Path, pid: u32) -> Result<Vec<LivePane>, Error> {
    let output = Command::new("wezterm")
        .args(["cli", "list", "--format", "json"])
        .env("WEZTERM_UNIX_SOCKET", socket)
        .output()
        .map_err(|source| Error::Spawn {
            program: "wezterm",
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Wezterm {
            socket: socket.to_path_buf(),
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let listed: Vec<ListedPane> =
        serde_json::from_slice(&output.stdout).map_err(|source| Error::WeztermParse {
            socket: socket.to_path_buf(),
            source,
        })?;
    single_window(listed, pid)
}

fn single_window(listed: Vec<ListedPane>, pid: u32) -> Result<Vec<LivePane>, Error> {
    let windows: BTreeSet<u64> = listed.iter().map(|pane| pane.window_id).collect();
    if windows.len() > 1 {
        return Err(Error::WeztermWindows {
            pid,
            windows: windows.len(),
        });
    }
    listed
        .into_iter()
        .map(|pane| {
            Ok(LivePane {
                cwd: local_path(&pane.cwd)?,
                title: pane.title,
            })
        })
        .collect()
}

/// The path of a `file:///C:/...` URI, without the trailing separator wezterm reports for a folder.
fn local_path(cwd: &str) -> Result<PathBuf, Error> {
    let not_local = || Error::PaneCwd {
        cwd: cwd.to_string(),
    };
    let encoded = cwd.strip_prefix("file:///").ok_or_else(not_local)?;
    let path = percent_decode_str(encoded)
        .decode_utf8()
        .map_err(|source| Error::PaneCwdEncoding {
            cwd: cwd.to_string(),
            source,
        })?
        .replace('/', "\\");
    Ok(PathBuf::from(path.trim_end_matches('\\')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(window_id: u64, cwd: &str) -> ListedPane {
        ListedPane {
            window_id,
            cwd: cwd.to_string(),
            title: String::new(),
        }
    }

    #[test]
    fn local_path_decodes_and_drops_the_trailing_separator() -> Result<(), Error> {
        assert_eq!(
            local_path("file:///C:/Program%20Files/x/")?,
            PathBuf::from(r"C:\Program Files\x")
        );
        Ok(())
    }

    #[test]
    fn local_path_rejects_a_remote_uri() {
        assert!(matches!(
            local_path("file://server/share/x"),
            Err(Error::PaneCwd { .. })
        ));
    }

    #[test]
    fn local_path_rejects_a_path_that_is_not_utf8() {
        assert!(matches!(
            local_path("file:///C:/%FF"),
            Err(Error::PaneCwdEncoding { .. })
        ));
    }

    #[test]
    fn single_window_rejects_two_windows() {
        let panes = vec![listed(0, "file:///C:/a/"), listed(1, "file:///C:/b/")];
        assert!(matches!(
            single_window(panes, 7),
            Err(Error::WeztermWindows { pid: 7, windows: 2 })
        ));
    }

    #[test]
    fn single_window_keeps_every_pane_of_one_window() -> Result<(), Error> {
        let panes = vec![listed(3, "file:///C:/a/"), listed(3, "file:///C:/b/")];
        let cwds: Vec<PathBuf> = single_window(panes, 7)?
            .into_iter()
            .map(|p| p.cwd)
            .collect();
        assert_eq!(cwds, vec![PathBuf::from(r"C:\a"), PathBuf::from(r"C:\b")]);
        Ok(())
    }
}
