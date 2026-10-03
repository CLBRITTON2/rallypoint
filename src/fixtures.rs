//! Test data shared across modules. Every value is neutral: a fixture never comes from a real session.

use crate::model::{AppState, ExePath, Owner, SavedWindow, WindowState};

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
