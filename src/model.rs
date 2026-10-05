//! A saved session: every GlazeWM window, the workspace it sits on, and what relaunching it needs.

use std::fmt;
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
    pub executable_path: Option<ExePath>,
    pub command_line: Option<String>,
    pub owner: Owner,
    pub title: String,
    pub class_name: String,
    pub state: WindowState,
    pub app: AppState,
}

/// An executable path, equal to another regardless of case, as Windows resolves paths.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(transparent)]
pub struct ExePath(String);

impl ExePath {
    pub fn new(path: String) -> ExePath {
        ExePath(path)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The rest of `text` after this path, when `text` starts with it regardless of case.
    pub fn strip_from<'a>(&self, text: &'a str) -> Option<&'a str> {
        let head = text.get(..self.0.len())?;
        caseless_eq(head, &self.0)
            .then(|| text.get(self.0.len()..))
            .flatten()
    }

    /// Whether the executable sits in a packaged app's install folder, which cannot be started directly.
    pub fn is_packaged(&self) -> bool {
        self.0.to_lowercase().contains(r"\windowsapps\")
    }
}

impl PartialEq for ExePath {
    fn eq(&self, other: &ExePath) -> bool {
        caseless_eq(&self.0, &other.0)
    }
}

impl fmt::Display for ExePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Windows account name, equal to another regardless of case, as Windows matches account names.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(transparent)]
pub struct Owner(String);

impl Owner {
    pub fn new(name: String) -> Owner {
        Owner(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PartialEq for Owner {
    fn eq(&self, other: &Owner) -> bool {
        caseless_eq(&self.0, &other.0)
    }
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn caseless_eq(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
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
    pub program: Option<ExePath>,
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

    #[test]
    fn exe_paths_and_owners_compare_regardless_of_case() {
        let path = |path: &str| ExePath::new(path.to_string());
        assert_eq!(path(r"C:\Tools\App.exe"), path(r"c:\tools\app.EXE"));
        assert_ne!(path(r"C:\tools\app.exe"), path(r"C:\tools\other.exe"));
        assert_eq!(
            Owner::new("Owner".to_string()),
            Owner::new("owner".to_string())
        );
    }

    #[test]
    fn is_packaged_ignores_case() {
        let packaged = ExePath::new(r"C:\Program Files\windowsapps\Example_1\app.exe".to_string());
        assert!(packaged.is_packaged());
        assert!(!ExePath::new(r"C:\tools\app.exe".to_string()).is_packaged());
    }
}
