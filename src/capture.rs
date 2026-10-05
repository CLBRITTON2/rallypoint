//! Reads the open windows into a session: GlazeWM for the windows, WMI for their processes, and each app module for
//! what reopening its windows needs.

use crate::account;
use crate::apps::{self, Kind, packaged, wezterm, windows_terminal};
use crate::cwd;
use crate::error::Error;
use crate::glazewm::{Client, Window, Workspace};
use crate::model::{AppState, ExePath, Focus, SavedWindow, Session, VERSION};
use crate::process::{Process, Processes};
use crate::store;

/// An open window: what a save records of it, the GlazeWM container ID that commands it, and its process.
pub struct LiveWindow {
    pub id: String,
    pub pid: u32,
    pub window: SavedWindow,
}

/// The connections reading a session needs, opened once and reused across reads.
pub struct Sources {
    glazewm: Client,
    processes: Processes,
}

impl Sources {
    pub fn connect() -> Result<Sources, Error> {
        Ok(Sources {
            glazewm: Client::connect()?,
            processes: Processes::connect()?,
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
        let tab_shells = windows_terminal::tab_shells()?;
        for workspace in self.glazewm.workspaces()? {
            for window in workspace.windows() {
                let process = self.processes.of_window(window.handle)?;
                let app = match apps::kind_of(&window.process_name) {
                    Kind::Wezterm => {
                        let owner_home = account::home_of(&process.owner)?;
                        AppState::Wezterm {
                            tabs: wezterm::saved_tabs(&owner_home, &process)?,
                        }
                    }
                    Kind::WindowsTerminal => AppState::WindowsTerminal {
                        tabs: windows_terminal::saved_tabs(
                            &self.processes,
                            &tab_shells,
                            window.handle,
                        )?,
                    },
                    Kind::Shell => AppState::Shell {
                        cwd: cwd::of_process(process.pid)?,
                    },
                    Kind::FrameHost => AppState::Packaged {
                        aumid: packaged::aumid_of_window(window.handle)?,
                    },
                    Kind::Program
                        if process
                            .executable_path
                            .as_ref()
                            .is_some_and(ExePath::is_packaged) =>
                    {
                        AppState::Packaged {
                            aumid: packaged::aumid_of(process.pid)?,
                        }
                    }
                    Kind::Program => AppState::Program,
                };
                windows.push(LiveWindow {
                    id: window.id.clone(),
                    pid: process.pid,
                    window: saved_window(&workspace.name, window, process, app),
                });
            }
        }
        Ok(windows)
    }

    pub fn focus(&mut self) -> Result<Focus, Error> {
        Ok(focus_of(&self.glazewm.workspaces()?))
    }
}

pub fn capture() -> Result<Session, Error> {
    let mut sources = Sources::connect()?;
    let windows = sources.live_windows()?;
    Ok(Session {
        version: VERSION,
        saved_at: store::now()?,
        focus: sources.focus()?,
        windows: windows.into_iter().map(|live| live.window).collect(),
    })
}

fn focus_of(workspaces: &[Workspace]) -> Focus {
    Focus {
        displayed: workspaces
            .iter()
            .filter(|workspace| workspace.is_displayed)
            .map(|workspace| workspace.name.clone())
            .collect(),
        focused: workspaces
            .iter()
            .find(|workspace| workspace.has_focus)
            .map(|workspace| workspace.name.clone()),
    }
}

fn saved_window(workspace: &str, window: Window, process: Process, app: AppState) -> SavedWindow {
    SavedWindow {
        workspace: workspace.to_string(),
        process_name: window.process_name,
        executable_path: process.executable_path,
        command_line: process.command_line,
        owner: process.owner,
        title: window.title,
        class_name: window.class_name,
        state: window.state.into(),
        app,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_of_takes_displayed_and_focused_workspaces() -> Result<(), serde_json::Error> {
        let workspaces: Vec<Workspace> = serde_json::from_str(
            r#"[{"name":"1","hasFocus":false,"isDisplayed":false,"children":[]},
                {"name":"2","hasFocus":false,"isDisplayed":true,"children":[]},
                {"name":"11","hasFocus":true,"isDisplayed":true,"children":[]}]"#,
        )?;
        let expected = Focus {
            displayed: vec!["2".to_string(), "11".to_string()],
            focused: Some("11".to_string()),
        };
        assert_eq!(focus_of(&workspaces), expected);
        Ok(())
    }
}
