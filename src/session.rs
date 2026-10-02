//! A saved session: every GlazeWM window, the workspace it sits on, and what relaunching it needs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::claude;
use crate::error::Error;
use crate::glazewm::{Client, State, Window};
use crate::process::{Process, Processes};
use crate::wezterm;

#[derive(Serialize, Deserialize)]
pub struct Session {
    /// Unix milliseconds.
    pub saved_at: u128,
    pub windows: Vec<SavedWindow>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SavedWindow {
    pub workspace: String,
    pub process_name: String,
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
    pub owner: String,
    pub title: String,
    pub class_name: String,
    pub state: State,
    /// Only wezterm-gui windows have panes.
    pub panes: Vec<SavedPane>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SavedPane {
    pub cwd: PathBuf,
    pub title: String,
    pub claude_session_id: Option<String>,
}

/// An open window: what a save records of it, and the GlazeWM container ID that commands it.
pub struct LiveWindow {
    pub id: String,
    pub window: SavedWindow,
}

/// The connections reading a session needs, opened once and reused across reads.
pub struct Sources {
    glazewm: Client,
    processes: Processes,
    profiles: PathBuf,
}

impl Sources {
    pub fn connect() -> Result<Sources, Error> {
        Ok(Sources {
            glazewm: Client::connect()?,
            processes: Processes::connect()?,
            profiles: profiles_folder()?,
        })
    }

    pub fn glazewm(&mut self) -> &mut Client {
        &mut self.glazewm
    }

    pub fn processes(&self) -> &Processes {
        &self.processes
    }

    /// Every open window, read from GlazeWM, WMI, wezterm and Claude Code's session files.
    pub fn live_windows(&mut self) -> Result<Vec<LiveWindow>, Error> {
        let mut windows = Vec::new();
        for workspace in self.glazewm.workspaces()? {
            for window in workspace.windows() {
                let process = self.processes.of_window(window.handle)?;
                let panes = match window.process_name.as_str() {
                    "wezterm-gui" => saved_panes(&self.profiles.join(&process.owner), &process)?,
                    _ => Vec::new(),
                };
                windows.push(LiveWindow {
                    id: window.id.clone(),
                    window: saved_window(&workspace.name, window, process, panes),
                });
            }
        }
        Ok(windows)
    }
}

pub fn capture() -> Result<Session, Error> {
    let windows = Sources::connect()?.live_windows()?;
    Ok(Session {
        saved_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(Error::Clock)?
            .as_millis(),
        windows: windows.into_iter().map(|live| live.window).collect(),
    })
}

/// Every session file in `folder`, oldest first.
fn saved_paths(folder: &Path) -> Result<Vec<PathBuf>, Error> {
    let read_error = |source| Error::Read {
        path: folder.to_path_buf(),
        source,
    };
    let mut saved: Vec<(u128, PathBuf)> = Vec::new();
    for entry in fs::read_dir(folder).map_err(read_error)? {
        let path = entry.map_err(read_error)?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let saved_at = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u128>().ok())
            .ok_or_else(|| Error::SessionName { path: path.clone() })?;
        saved.push((saved_at, path));
    }
    saved.sort_by_key(|(saved_at, _)| *saved_at);
    Ok(saved.into_iter().map(|(_, path)| path).collect())
}

/// The newest session in `folder`, the one with the largest `saved_at`.
pub fn latest(folder: &Path) -> Result<Session, Error> {
    let path = saved_paths(folder)?.pop().ok_or_else(|| Error::NoSession {
        folder: folder.to_path_buf(),
    })?;
    let text = fs::read_to_string(&path).map_err(|source| Error::Read {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| Error::Decode { path, source })
}

fn saved_window(
    workspace: &str,
    window: Window,
    process: Process,
    panes: Vec<SavedPane>,
) -> SavedWindow {
    SavedWindow {
        workspace: workspace.to_string(),
        process_name: window.process_name,
        executable_path: process.executable_path,
        command_line: process.command_line,
        owner: process.owner,
        title: window.title,
        class_name: window.class_name,
        state: window.state.kind,
        panes,
    }
}

fn saved_panes(owner_home: &Path, process: &Process) -> Result<Vec<SavedPane>, Error> {
    let socket = wezterm::socket(owner_home, process.pid);
    wezterm::panes(&socket, process.pid)?
        .into_iter()
        .map(|pane| {
            Ok(SavedPane {
                claude_session_id: claude::session_id(owner_home, &pane.cwd, &pane.title)?,
                cwd: pane.cwd,
                title: pane.title,
            })
        })
        .collect()
}

/// The folder holding every account's profile, so an owner's home is found without naming a user folder.
fn profiles_folder() -> Result<PathBuf, Error> {
    let profile = std::env::var_os("USERPROFILE").ok_or(Error::Env {
        name: "USERPROFILE",
    })?;
    let profile = PathBuf::from(profile);
    profile.parent().map(Path::to_path_buf).ok_or(Error::Env {
        name: "USERPROFILE",
    })
}

/// `%LOCALAPPDATA%\rallypoint\sessions`, where every save lands.
pub fn sessions_folder() -> Result<PathBuf, Error> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or(Error::Env {
        name: "LOCALAPPDATA",
    })?;
    Ok(PathBuf::from(local).join(r"rallypoint\sessions"))
}

/// Writes `session` to `<folder>\<saved_at>.json` through a temp file, so a crash mid-write never leaves a torn
/// session behind. Returns the path written.
pub fn write(folder: &Path, session: &Session) -> Result<PathBuf, Error> {
    let write_error = |path: &Path| {
        let path = path.to_path_buf();
        move |source| Error::Write { path, source }
    };
    fs::create_dir_all(folder).map_err(write_error(folder))?;
    let path = folder.join(format!("{}.json", session.saved_at));
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(session).map_err(Error::Encode)?;
    fs::write(&temp, json).map_err(write_error(&temp))?;
    fs::rename(&temp, &path).map_err(write_error(&path))?;
    Ok(path)
}

/// Deletes all but the newest `keep` sessions in `folder`.
pub fn prune(folder: &Path, keep: usize) -> Result<(), Error> {
    let saved = saved_paths(folder)?;
    let stale = saved.len().saturating_sub(keep);
    for path in saved.into_iter().take(stale) {
        fs::remove_file(&path).map_err(|source| Error::Remove { path, source })?;
    }
    Ok(())
}

/// Whether two saves hold the same windows, ignoring titles: a title changes with every page or command and needs
/// no new save, while a Claude session change shows up in `claude_session_id`.
pub fn same_windows(a: &[SavedWindow], b: &[SavedWindow]) -> bool {
    untitled(a) == untitled(b)
}

fn untitled(windows: &[SavedWindow]) -> Vec<SavedWindow> {
    windows
        .iter()
        .map(|window| SavedWindow {
            title: String::new(),
            panes: window
                .panes
                .iter()
                .map(|pane| SavedPane {
                    title: String::new(),
                    ..pane.clone()
                })
                .collect(),
            ..window.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(title: &str, workspace: &str) -> SavedWindow {
        SavedWindow {
            workspace: workspace.to_string(),
            process_name: "wezterm-gui".to_string(),
            executable_path: None,
            command_line: None,
            owner: "owner".to_string(),
            title: title.to_string(),
            class_name: "c".to_string(),
            state: State::Tiling,
            panes: vec![SavedPane {
                cwd: PathBuf::from(r"C:\x"),
                title: title.to_string(),
                claude_session_id: None,
            }],
        }
    }

    #[test]
    fn same_windows_ignores_titles_only() {
        assert!(same_windows(&[window("a", "1")], &[window("b", "1")]));
        assert!(!same_windows(&[window("a", "1")], &[window("a", "2")]));
    }

    #[test]
    fn prune_keeps_the_newest() -> Result<(), Box<dyn std::error::Error>> {
        let folder = std::env::temp_dir().join(format!("rallypoint-prune-{}", std::process::id()));
        for saved_at in [3, 10, 2] {
            write(
                &folder,
                &Session {
                    saved_at,
                    windows: Vec::new(),
                },
            )?;
        }
        prune(&folder, 2)?;
        let left: Vec<PathBuf> = saved_paths(&folder)?;
        fs::remove_dir_all(&folder)?;
        assert_eq!(left, vec![folder.join("3.json"), folder.join("10.json")]);
        Ok(())
    }
}
