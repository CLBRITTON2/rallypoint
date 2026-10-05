//! The line `rallypoint list` prints for each saved session.

use std::path::Path;

use crate::model::{SavedWindow, Session, UnixMillis};

/// `session`, saved at `path`, as one tab-separated line: its path, its age at `now`, and its window and workspace
/// counts.
pub fn summary(path: &Path, session: &Session, now: UnixMillis) -> String {
    format!(
        "{}\t{}\t{} windows on {} workspaces",
        path.display(),
        age(session.saved_at, now),
        session.windows.len(),
        workspace_count(&session.windows)
    )
}

/// How long before `now` a session saved at `saved_at` was written, in its largest whole unit, as `12 min ago`.
fn age(saved_at: UnixMillis, now: UnixMillis) -> String {
    let seconds = now.since(saved_at).as_secs();
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86400),
    }
}

/// How many distinct workspaces `windows` sit on.
fn workspace_count(windows: &[SavedWindow]) -> usize {
    let mut names: Vec<&str> = windows
        .iter()
        .map(|window| window.workspace.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn window(workspace: &str) -> SavedWindow {
        SavedWindow {
            workspace: workspace.to_string(),
            ..fixtures::window("app", None)
        }
    }

    #[test]
    fn age_uses_the_largest_whole_unit() {
        let age = |saved_at: u64, now: u64| age(UnixMillis::new(saved_at), UnixMillis::new(now));
        assert_eq!(age(0, 59_999), "59 s ago");
        assert_eq!(age(0, 60_000), "1 min ago");
        assert_eq!(age(0, 3_599_999), "59 min ago");
        assert_eq!(age(0, 7_200_000), "2 h ago");
        assert_eq!(age(0, 86_400_000), "1 d ago");
        assert_eq!(age(5_000, 1_000), "0 s ago");
    }

    #[test]
    fn workspace_count_counts_each_name_once() {
        let windows = [window("2"), window("1"), window("2")];
        assert_eq!(workspace_count(&windows), 2);
    }
}
