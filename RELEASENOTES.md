# Release notes

Each release has a `## [<version>] - <date>` section, newest first. `scripts/release.ps1` publishes the section for
the tagged version as its GitHub release notes, and refuses to release a version without one. Changes since the last
release go under `## [Unreleased]`, renamed to the version when it is tagged.

## [Unreleased]

- `rallypoint list` prints every saved session, newest first, with its age, window count and workspace count.
- `rallypoint restore <path>` brings back the session at that path, so a bad save no longer hides the one before it.
- Sessions record the workspace shown on each monitor and the focused one, and `restore` shows and focuses them
  again.
- Sessions carry a format version, and `restore` and `list` refuse a session of any other version with an error
  naming both. Sessions saved by 0.1.0 have none, so the first `save` after upgrading is the oldest one that
  restores.
- pwsh, Windows PowerShell and cmd windows reopen in their saved folder, one console per window. A PowerShell
  console's folder is the one it started in, since `cd` does not change it. The README has an optional prompt line
  that makes it the folder of the last `cd`.
- Windows Terminal windows reopen through `wt.exe` with their saved tabs, each shell tab in its folder.
- `rallypoint status` prints whether `rallypoint watch` is running and the newest session, and exits 1 when it is not.
- `rallypoint watch` shows a notification area icon whose right-click menu saves at once or stops `watch`.

## [0.1.0] - 2026-10-02

First release.

- `save` writes every GlazeWM window's workspace, state and program to `%LOCALAPPDATA%\rallypoint\sessions\`, with
  each wezterm pane's folder and Claude Code session.
- `restore` moves open windows back to their saved workspaces and states and launches the programs that are not
  running. wezterm windows reopen in their folder and resume their Claude Code session. Windows of another account
  launch through `runas /savecred`.
- `restore` shows the windows a GlazeWM `wm-exit` left hidden and unmanaged again, instead of launching their
  programs a second time.
- `watch` saves after window changes and once a minute until GlazeWM exits, keeps the newest 10 sessions, and stops
  saving during a Windows shutdown.
