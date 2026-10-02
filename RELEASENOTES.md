# Release notes

Each release has a `## [<version>] - <date>` section, newest first. `scripts/release.ps1` publishes the section for
the tagged version as its GitHub release notes, and refuses to release a version without one. Changes since the last
release go under `## [Unreleased]`, renamed to the version when it is tagged.

## [Unreleased]

- `rallypoint list` prints every saved session, newest first, with its age, window count and workspace count.
- `rallypoint restore <path>` brings back the session at that path, so a bad save no longer hides the one before it.

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
