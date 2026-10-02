# rallypoint

Saves your [GlazeWM](https://github.com/glzr-io/glazewm) session and brings it back after a reboot, a crash or a
GlazeWM restart. Every window returns to its workspace, in its tiling or floating state, and programs that are not
running are started again.

## Install

Download the zip from the [latest release](https://github.com/CLBRITTON2/rallypoint/releases/latest) and put
`rallypoint.exe` anywhere, or build it:

```powershell
cargo install --path .
```

Requires Windows 11 and GlazeWM 3 with its IPC server on the default `ws://127.0.0.1:6123`.

## Usage

```powershell
# write the current session and print its path
rallypoint save
# print every saved session, newest first, with its age and window count
rallypoint list
# bring the newest session back and print one line per saved window
rallypoint restore
# bring back an older session, by a path from list
rallypoint restore $env:LOCALAPPDATA\rallypoint\sessions\1790965947095.json
# keep the saved session current until GlazeWM exits
rallypoint watch
```

The usual setup is to run `restore` then `watch` from GlazeWM's `startup_commands`, in one hidden shell, so `watch`
starts only after `restore` is done. A `watch` that started first would save the half-open login session over the
one you want back. Adjust the path to wherever `rallypoint.exe` lives:

```yaml
general:
  startup_commands:
    - 'shell-exec --hide-window C:/Program Files/PowerShell/7/pwsh.exe -NoProfile -Command & $HOME/.cargo/bin/rallypoint.exe restore *> $env:TEMP/rallypoint-restore.log; & $HOME/.cargo/bin/rallypoint.exe watch *> $env:TEMP/rallypoint-watch.log'
```

A keybinding that runs `restore` in a visible shell is handy for bringing the session back by hand and reading the
report:

```yaml
keybindings:
  - commands: ['shell-exec C:/Program Files/PowerShell/7/pwsh.exe -NoExit -NoProfile -Command & $HOME/.cargo/bin/rallypoint.exe restore']
    bindings: ['lwin+alt+r']
```

Exit codes: 0 done, 1 when `restore` could not bring back every window, 2 error. Errors go to stderr.

## What it saves

For each window GlazeWM manages: its workspace and state, and the program behind it (executable path, command line
and owning account). Each session also records the workspace shown on each monitor and the one with focus. For a
[wezterm](https://wezterm.org) window it also saves the working directory of each pane, and the
[Claude Code](https://claude.com/claude-code) session a pane shows, if any. For a console shell (pwsh, Windows
PowerShell or cmd) it saves the shell's folder. For a Windows Terminal window it saves each tab's program and, for a
shell tab, its folder. PowerShell's `cd` changes only PowerShell's own location, not its process's folder, so a
PowerShell console or tab reopens in the folder it started in. To have it reopen where you last `cd`'d, add this
line to the `prompt` function in your PowerShell profile (it is optional, and rallypoint works without it):

```powershell
if ($PWD.Provider.Name -eq 'FileSystem') { [Environment]::CurrentDirectory = $PWD.ProviderPath }
```

Sessions are JSON files in `%LOCALAPPDATA%\rallypoint\sessions\`, named by the time they were written. `watch`
saves 2 s after a burst of window events and once a minute, skips a save whose windows match the last one, and keeps
the newest 10. A focus change alone writes no session, so the focus restored is the one at the last window change. It
stops saving while Windows shuts down, so the apps closing one by one never overwrite the session with an empty one.

## How restore works

1. It waits until no new window has appeared for 5 s, so apps still starting at login count as open.
2. Windows of saved programs that a GlazeWM `wm-exit` left hidden and unmanaged are shown again and handed back to
   GlazeWM, instead of being started a second time.
3. Open windows are matched to saved ones, and the rest are launched. A wezterm window is matched by its first pane's
   folder and reopened there, resuming its Claude Code session if it had one. Any other program is matched by its
   executable and launched once with its saved command line, since a second launch of most apps opens a stray window
   instead of restoring the saved ones. A shell is matched by its folder and reopened there in a console of its own,
   one per window, without its saved arguments, so a window opened to run one command does not run it again.
   A Windows Terminal window is matched by its tabs and reopened with `wt.exe -w new`, one tab per saved tab in the
   order they were opened, each shell tab in its folder and without its saved arguments.
4. Each window is moved to its saved workspace and state. Windows that are already right are left alone.
5. The workspace each monitor showed is shown again, and the one that had focus gets it back.

Running `restore` twice is safe: open windows are moved, never launched again.

A window owned by another Windows account is launched as that account with `runas /user:<account> /savecred`. Run
that once by hand to save the password. Note that a saved credential lets any program running as you start programs
as that account.

## Limitations

- Split layouts are not rebuilt. Windows come back on the right workspace in saved order, but GlazeWM's IPC cannot
  build a split tree.
- Store apps other than Windows Terminal are matched when open but never launched, since their executables cannot be
  started directly.
- Windows Terminal split panes come back as tabs, tabs come back in the order they were opened rather than a dragged
  order, and the first tab is the active one.
- Elevated windows relaunch unelevated. The folder of an elevated shell, or of another account's, cannot be read, so
  it reopens in the default folder.
- A multi-pane wezterm window comes back with its first pane only.
- What happens inside a window is up to the app: a browser restores its own tabs.

## Test

```powershell
./scripts/lint.ps1
./scripts/unit-tests.ps1
```

## License

Apache-2.0, see [LICENSE](LICENSE).
