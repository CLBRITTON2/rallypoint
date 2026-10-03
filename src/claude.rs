//! Which Claude Code session a terminal pane shows, found from the title Claude Code gives the terminal.

use std::cmp::Reverse;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::error::Error;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TitleRecord {
    ai_title: Option<String>,
    custom_title: Option<String>,
}

/// The session in `cwd` whose title is `pane_title`. With several, the newest wins, since `/clear` and forks start a
/// new session file under the same title. None when the pane runs no Claude Code session from that folder.
pub fn session_id(
    owner_home: &Path,
    cwd: &Path,
    pane_title: &str,
) -> Result<Option<String>, Error> {
    let title = strip_status(pane_title);
    let folder = owner_home.join(r".claude\projects").join(project_slug(cwd));
    let read_error = |source| Error::Read {
        path: folder.clone(),
        source,
    };
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(read_error(error)),
    };
    let mut sessions: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries {
        let path = entry.map_err(read_error)?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            let modified = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .map_err(|source| Error::Read {
                    path: path.clone(),
                    source,
                })?;
            sessions.push((modified, path));
        }
    }
    sessions.sort_by_key(|(modified, _)| Reverse(*modified));
    for (_, path) in sessions {
        let text = fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        if last_title(&text, &path)?.as_deref() == Some(title) {
            return Ok(path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned()));
        }
    }
    Ok(None)
}

/// The title without the status glyph Claude Code puts in front of it (an idle star or a busy spinner).
fn strip_status(title: &str) -> &str {
    let mut chars = title.chars();
    match (chars.next(), chars.next()) {
        (Some(glyph), Some(' ')) if !glyph.is_alphanumeric() => chars.as_str(),
        _ => title,
    }
}

/// The folder name Claude Code files a working directory's sessions under.
fn project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The title of the last `ai-title` or `custom-title` record in a session file.
fn last_title(text: &str, path: &Path) -> Result<Option<String>, Error> {
    // Inside a message, record JSON is escaped, so these markers match only real records.
    let Some(line) = text.lines().rev().find(|line| {
        line.contains(r#""type":"ai-title""#) || line.contains(r#""type":"custom-title""#)
    }) else {
        return Ok(None);
    };
    let record: TitleRecord =
        serde_json::from_str(line).map_err(|source| Error::SessionRecord {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(record.custom_title.or(record.ai_title))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_status_drops_a_leading_glyph() {
        assert_eq!(strip_status("✳ Example title"), "Example title");
        assert_eq!(strip_status("◐ Example title"), "Example title");
    }

    #[test]
    fn strip_status_keeps_a_title_without_a_glyph() {
        assert_eq!(strip_status("A plain title"), "A plain title");
        assert_eq!(strip_status("C:\\Program Files"), "C:\\Program Files");
    }

    #[test]
    fn project_slug_replaces_every_non_alphanumeric() {
        assert_eq!(
            project_slug(Path::new(r"C:\Users\owner\dev\my-app_win.x")),
            "C--Users-owner-dev-my-app-win-x"
        );
    }

    #[test]
    fn last_title_takes_the_last_record_of_either_kind() -> Result<(), Error> {
        let text = [
            r#"{"type":"ai-title","aiTitle":"first"}"#,
            r#"{"type":"user","message":"{\"type\":\"ai-title\"}"}"#,
            r#"{"type":"custom-title","customTitle":"renamed"}"#,
            r#"{"type":"assistant"}"#,
        ]
        .join("\n");
        assert_eq!(
            last_title(&text, Path::new("x"))?.as_deref(),
            Some("renamed")
        );
        Ok(())
    }

    #[test]
    fn last_title_is_none_without_a_record() -> Result<(), Error> {
        assert_eq!(last_title(r#"{"type":"user"}"#, Path::new("x"))?, None);
        Ok(())
    }
}
