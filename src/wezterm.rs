//! Panes of a wezterm-gui window. Every wezterm-gui process runs its own mux behind
//! `<owner home>\.local\share\wezterm\gui-sock-<pid>`, so the CLI is pointed at that socket.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use percent_encoding::percent_decode_str;
use serde::Deserialize;

use crate::error::Error;

pub struct Pane {
    pub cwd: PathBuf,
    pub title: String,
}

#[derive(Deserialize)]
struct ListedPane {
    window_id: u64,
    cwd: String,
    title: String,
}

pub fn socket(owner_home: &Path, pid: u32) -> PathBuf {
    owner_home.join(format!(r".local\share\wezterm\gui-sock-{pid}"))
}

/// The panes of wezterm-gui process `pid`, which must hold a single window.
pub fn panes(socket: &Path, pid: u32) -> Result<Vec<Pane>, Error> {
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

fn single_window(listed: Vec<ListedPane>, pid: u32) -> Result<Vec<Pane>, Error> {
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
            Ok(Pane {
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
        .map_err(|_| not_local())?
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
