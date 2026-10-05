//! The session files in `%LOCALAPPDATA%\rallypoint\sessions`, one `<saved_at>.json` per save.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::error::Error;
use crate::model::{Session, UnixMillis, VERSION};

/// The part of a session file read before the rest, so a session of another version fails on its version instead
/// of on whichever field changed. Sessions saved before versioning have none.
#[derive(Deserialize)]
struct Header {
    version: Option<u32>,
}

pub fn now() -> Result<UnixMillis, Error> {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(Error::Clock)?;
    let millis = u64::try_from(since_epoch.as_millis()).map_err(Error::ClockRange)?;
    Ok(UnixMillis::new(millis))
}

/// Every session file in `folder`, oldest first. None when `folder` does not exist, as before the first save.
fn saved_paths(folder: &Path) -> Result<Vec<PathBuf>, Error> {
    let read_error = |source| Error::Read {
        path: folder.to_path_buf(),
        source,
    };
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(read_error(error)),
    };
    let mut saved: Vec<(UnixMillis, PathBuf)> = Vec::new();
    for entry in entries {
        let path = entry.map_err(read_error)?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let saved_at = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok())
            .map(UnixMillis::new)
            .ok_or_else(|| Error::SessionName { path: path.clone() })?;
        saved.push((saved_at, path));
    }
    saved.sort_by_key(|(saved_at, _)| *saved_at);
    Ok(saved.into_iter().map(|(_, path)| path).collect())
}

/// Every session file in `folder`, newest first.
pub fn newest_first(folder: &Path) -> Result<Vec<PathBuf>, Error> {
    Ok(saved_paths(folder)?.into_iter().rev().collect())
}

/// The newest session in `folder`, the one with the largest `saved_at`.
pub fn latest(folder: &Path) -> Result<Session, Error> {
    let path = saved_paths(folder)?.pop().ok_or_else(|| Error::NoSession {
        folder: folder.to_path_buf(),
    })?;
    read(&path)
}

pub fn read(path: &Path) -> Result<Session, Error> {
    let text = fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let decode_error = |source| Error::Decode {
        path: path.to_path_buf(),
        source,
    };
    let header: Header = serde_json::from_str(&text).map_err(decode_error)?;
    if header.version != Some(VERSION) {
        return Err(Error::SessionVersion {
            path: path.to_path_buf(),
            found: header.version,
            expected: VERSION,
        });
    }
    serde_json::from_str(&text).map_err(decode_error)
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
    let json = serde_json::to_vec_pretty(session).map_err(|source| Error::Encode {
        path: path.clone(),
        source,
    })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::{
        AppState, ExePath, Focus, Resume, SavedWindow, Tab, WeztermPane, WeztermTab, WindowState,
    };

    fn sessions_saved_at(folder: &Path, saved_at: &[u64]) -> Result<(), Error> {
        for &saved_at in saved_at {
            write(
                folder,
                &Session {
                    version: VERSION,
                    saved_at: UnixMillis::new(saved_at),
                    focus: Focus::default(),
                    windows: Vec::new(),
                },
            )?;
        }
        Ok(())
    }

    #[test]
    fn read_refuses_a_session_of_another_version() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let unversioned = folder.path().join("1.json");
        fs::write(&unversioned, r#"{"saved_at":1,"windows":[]}"#)?;
        let newer = folder.path().join("2.json");
        fs::write(
            &newer,
            r#"{"version":99,"saved_at":2,"focus":{},"windows":[{}]}"#,
        )?;
        assert!(matches!(
            read(&unversioned),
            Err(Error::SessionVersion {
                found: None,
                expected: VERSION,
                ..
            })
        ));
        assert!(matches!(
            read(&newer),
            Err(Error::SessionVersion {
                found: Some(99),
                expected: VERSION,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn a_written_session_reads_back() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let pane = WeztermPane {
            resume: Some(Resume::ClaudeCode {
                session_id: "id".to_string(),
            }),
            active: true,
            ..fixtures::pane(r"C:\work\project", 0, 0, 80, 24)
        };
        let tab = Tab {
            program: Some(ExePath::new(r"C:\tools\shell.exe".to_string())),
            command_line: None,
            cwd: Some(PathBuf::from(r"C:\work\project")),
        };
        let windows = vec![
            fixtures::window("app", Some(r"C:\tools\app.exe")),
            SavedWindow {
                app: AppState::Shell { cwd: None },
                state: WindowState::Floating,
                ..fixtures::window("shell", None)
            },
            SavedWindow {
                app: AppState::Wezterm {
                    tabs: vec![WeztermTab { panes: vec![pane] }],
                },
                ..fixtures::window("wezterm", None)
            },
            SavedWindow {
                app: AppState::WindowsTerminal { tabs: vec![tab] },
                ..fixtures::window("windows terminal", None)
            },
        ];
        let session = Session {
            version: VERSION,
            saved_at: UnixMillis::new(7),
            focus: Focus::default(),
            windows: windows.clone(),
        };
        assert_eq!(read(&write(folder.path(), &session)?)?.windows, windows);
        Ok(())
    }

    #[test]
    fn newest_first_reverses_save_order() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        sessions_saved_at(folder.path(), &[3, 10, 2])?;
        let names = ["10.json", "3.json", "2.json"].map(|name| folder.path().join(name));
        assert_eq!(newest_first(folder.path())?, names);
        Ok(())
    }

    #[test]
    fn a_missing_folder_holds_no_session() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("missing");
        assert_eq!(newest_first(&folder)?, Vec::<PathBuf>::new());
        assert!(matches!(latest(&folder), Err(Error::NoSession { .. })));
        Ok(())
    }

    #[test]
    fn a_stray_json_file_is_not_a_session() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        fs::write(folder.path().join("x.json"), "{}")?;
        assert!(matches!(
            newest_first(folder.path()),
            Err(Error::SessionName { .. })
        ));
        Ok(())
    }

    #[test]
    fn read_refuses_a_file_that_is_not_json() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("1.json");
        fs::write(&path, "not json")?;
        assert!(matches!(read(&path), Err(Error::Decode { .. })));
        Ok(())
    }

    #[test]
    fn prune_keeps_the_newest() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        sessions_saved_at(folder.path(), &[3, 10, 2])?;
        prune(folder.path(), 2)?;
        let left = vec![folder.path().join("3.json"), folder.path().join("10.json")];
        assert_eq!(saved_paths(folder.path())?, left);
        Ok(())
    }
}
