//! Test data shared across modules. Every value is neutral: a fixture never comes from a real session.

use crate::glazewm::State;
use crate::session::SavedWindow;

/// A tiling window on workspace `1` with no folder, panes or tabs. Tests override the fields they are about.
pub fn window(process_name: &str, executable_path: Option<&str>) -> SavedWindow {
    SavedWindow {
        workspace: "1".to_string(),
        process_name: process_name.to_string(),
        executable_path: executable_path.map(str::to_string),
        command_line: None,
        owner: "owner".to_string(),
        title: String::new(),
        class_name: String::new(),
        state: State::Tiling,
        cwd: None,
        panes: Vec::new(),
        tabs: Vec::new(),
    }
}
