//! A saved session: every GlazeWM window, the workspace it sits on, and what relaunching it needs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::glazewm::State;

/// The session format `capture` writes and `store::read` accepts. Bump it on any change an older rallypoint could
/// misread, since a session of another version is refused rather than migrated.
pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    /// Unix milliseconds.
    pub saved_at: u64,
    pub focus: Focus,
    pub windows: Vec<SavedWindow>,
}

/// Which workspaces were on screen.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Focus {
    /// One per monitor.
    pub displayed: Vec<String>,
    pub focused: Option<String>,
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
    pub state: WindowState,
    pub app: AppState,
}

/// rallypoint's copy of a GlazeWM window state, so the session format does not change with GlazeWM's IPC types.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    Tiling,
    Floating,
    Minimized,
    Fullscreen,
}

impl From<State> for WindowState {
    fn from(state: State) -> WindowState {
        match state {
            State::Tiling => WindowState::Tiling,
            State::Floating => WindowState::Floating,
            State::Minimized => WindowState::Minimized,
            State::Fullscreen => WindowState::Fullscreen,
        }
    }
}

impl From<WindowState> for State {
    fn from(state: WindowState) -> State {
        match state {
            WindowState::Tiling => State::Tiling,
            WindowState::Floating => State::Floating,
            WindowState::Minimized => State::Minimized,
            WindowState::Fullscreen => State::Fullscreen,
        }
    }
}

/// What reopening a window needs beyond its program.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppState {
    Program,
    /// A console shell. None is a folder save could not read.
    Shell {
        cwd: Option<PathBuf>,
    },
    /// A terminal window: a wezterm window's panes or a Windows Terminal window's tabs, oldest first.
    Terminal {
        panes: Vec<Pane>,
    },
}

/// One wezterm pane or Windows Terminal tab.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Pane {
    /// The program the pane runs. None for a wezterm pane, and for a tab running a protected process.
    pub program: Option<String>,
    pub command_line: Option<String>,
    /// None for a tab that is not a shell, and for a shell whose folder could not be read.
    pub cwd: Option<PathBuf>,
    pub resume: Option<Resume>,
}

/// A program session a pane reopens into.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resume {
    ClaudeCode { session_id: String },
}

/// Whether two saves hold the same windows, ignoring titles: a title changes with every page or command and needs
/// no new save, while a Claude session change shows up in a pane's `resume`.
pub fn same_windows(a: &[SavedWindow], b: &[SavedWindow]) -> bool {
    untitled(a) == untitled(b)
}

fn untitled(windows: &[SavedWindow]) -> Vec<SavedWindow> {
    windows
        .iter()
        .map(|window| SavedWindow {
            title: String::new(),
            ..window.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn window(title: &str, workspace: &str) -> SavedWindow {
        SavedWindow {
            workspace: workspace.to_string(),
            title: title.to_string(),
            ..fixtures::window("app", None)
        }
    }

    #[test]
    fn same_windows_ignores_titles_only() {
        assert!(same_windows(&[window("a", "1")], &[window("b", "1")]));
        assert!(!same_windows(&[window("a", "1")], &[window("a", "2")]));
    }
}
