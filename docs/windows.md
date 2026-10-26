# Native Windows

Download the x86_64 MSVC ZIP and its SHA256 checksum from the
[latest release](https://github.com/drewdunne/kerdr/releases/latest). Extract `kerdr.exe` into
a folder on PATH. No WSL, Git Bash, or Rust installation is needed to run it.

Use Windows 10/11 with Windows Terminal and install
[native Herdr](https://herdr.dev/docs/windows-beta/). Kerdr uses the same board, colors,
keys, feed format, and action format as Linux and macOS. `kerdr demo` opens an offline board;
`kerdr doctor` checks your real machines. Windows builds are unsigned.

## Local sessions

Start a native Herdr session normally. Kerdr discovers its running sessions and snapshots
through Herdr's native CLI. Press `i` to see unlinked conversations, Enter to open the
selected workspace, and Herdr's prefix then `q` to detach back to Kerdr. Sessions and agents
keep running when you return to the board or quit Kerdr.

To enable prefix+Space (next card needing you) and prefix+Backspace (board), run:

```powershell
kerdr herdr-keys --install
herdr --session NAME server reload-config
```

Install the keys on each server machine. Kerdr emits commands for the platform on which you
run `herdr-keys`, because Herdr executes them on its server. The default prefix is Ctrl+B;
Kerdr reads your configured prefix. The standard prefix+q detach works without custom keys.

## Remote Linux and macOS machines

Install Windows OpenSSH Client and configure your hosts in `%USERPROFILE%\.ssh\config`.
Check that `ssh HOST` connects normally, then add a machine to `%APPDATA%\kerdr\config.toml`:

```toml
[[machine]]
name = "workbox"
ssh = "workbox"
```

The target needs Herdr 0.9.1 or newer and a POSIX shell with its usual utilities. Remote
polling and adapters use that host's POSIX shell. Herdr's own `--remote` handles interactive
attach when compatible; older or unknown servers use a plain SSH terminal so Kerdr never
requests a restart that would stop their agents. Windows OpenSSH has no Unix control socket
sharing; use keys loaded in Windows `ssh-agent` for repeated connections. Kerdr polls without
prompts and offers its existing connection flow when authentication or host trust needs you.
Remote Windows targets are not supported by Kerdr's POSIX polling path in this release.

## Configuration and adapters

Windows defaults:

| Data | Location |
| --- | --- |
| Kerdr config | `%APPDATA%\kerdr\config.toml` |
| Herdr config / installed keys | `%APPDATA%\herdr\config.toml` |
| Board state | `%LOCALAPPDATA%\kerdr` |
| Cache / temporary client config | `%LOCALAPPDATA%\cache\kerdr` |
| Home / feed working directory | `%USERPROFILE%` |

Absolute `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, and `XDG_CACHE_HOME` overrides still work.
`KERDR_CONFIG` selects an explicit Kerdr config; `HERDR_CONFIG_PATH` selects Herdr's config.
Local feeds and actions use PowerShell 5.1 with UTF-8 input/output; remote adapters use the
remote shell. See [the adapter requirements](feed.md#native-windows-adapters) before reusing
a Unix-only adapter. No adapter is required for Herdr workspace discovery and navigation.

## Verification

The CI matrix runs the Rust tests and strict lint checks on Windows, Linux, and macOS.
Windows-specific fixtures exercise native polling, Unicode feeds, literal JSON action input,
confirmation responses, failures, and deadlines. The Windows integration test starts an
isolated real Herdr server and uses Microsoft's ConPTY to exercise Enter, detach, both custom
step-out keys, board redraw, and persistence after Kerdr exits. It never installs test builds
or modifies an existing user's Herdr config. Release jobs test and package native MSVC builds.

The hosted Windows runner has no route to a private Linux host. CI checks the POSIX collector
against real Linux/macOS Herdr servers and checks the native Windows OpenSSH options, while
the interactive Windows-to-Linux SSH handoff remains outside this suite's verified coverage.


The accounts screens and every account action run in the three-system CI matrix. Windows
uses its native URL handler for sign-in links (no command interpreter), OSC 52 for copying
a link in Windows Terminal, and a red ✕ for usage waits instead of an emoji pause glyph.
