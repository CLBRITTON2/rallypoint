//! Test data shared across modules. Every value is neutral: a fixture never comes from a real session.

use std::path::PathBuf;

use crate::model::{AppState, ExePath, Owner, PaneArea, SavedWindow, WeztermPane, WindowState};

/// A tiling program window on workspace `1`. Tests override the fields they are about.
pub fn window(process_name: &str, executable_path: Option<&str>) -> SavedWindow {
    SavedWindow {
        workspace: "1".to_string(),
        process_name: process_name.to_string(),
        executable_path: executable_path.map(|path| ExePath::new(path.to_string())),
        command_line: None,
        owner: Owner::new("owner".to_string()),
        title: String::new(),
        class_name: String::new(),
        state: WindowState::Tiling,
        app: AppState::Program,
    }
}

/// An inactive wezterm pane in `cwd` at the given cells, running no program of its own.
pub fn pane(cwd: &str, left: u32, top: u32, cols: u32, rows: u32) -> WeztermPane {
    WeztermPane {
        cwd: PathBuf::from(cwd),
        resume: None,
        area: PaneArea {
            left,
            top,
            cols,
            rows,
        },
        active: false,
    }
}
