# memory-watchdog

A small Rust daemon, run as a macOS launchd agent, that watches memory pressure. When pressure turns critical, it kills the largest runaway process before the system runs out of swap and starts killing system helpers, which is what freezes apps.

Candidates for killing:

- Dev tools (tsgo, tsc, tsserver, node, next-server, esbuild, vite, vitest, jest, webpack, eslint, and anything under `node_modules/`) using more than 1500 MB.
- Chrome tab renderer processes using more than 500 MB. The browser itself is never killed.

Only processes owned by the current user are considered. Memory is the physical footprint (the number top and Activity Monitor show, including compressed memory), read directly with `proc_pid_rusage`; pressure comes from the `kern.memorystatus_vm_pressure_level` sysctl. Pressure counts as critical at level 4, or at level 2 (warn) with 10% or less memory available. Each kill sends `SIGTERM`, then `SIGKILL` after 3 seconds, and shows a notification.

## Install

Requires a Rust toolchain.

```sh
./install.sh
```

This builds the release binary, copies it to `~/.local/bin/memory-watchdog`, writes `~/Library/LaunchAgents/local.memory-watchdog.plist` and starts the agent. It starts at login and restarts if it stops.

Remove it with `./uninstall.sh`. Stop it temporarily with `launchctl bootout gui/$(id -u)/local.memory-watchdog`.

Log: `~/Library/Logs/memory-watchdog.log`

## Configuration

Environment variables, mainly for testing:

| Variable | Default | Meaning |
| --- | --- | --- |
| `WATCHDOG_PATTERN` | dev-tool regex | Commands that may be killed |
| `WATCHDOG_MIN_MB` | `1500` | Minimum footprint for a dev-tool candidate |
| `WATCHDOG_CHROME_PATTERN` | Chrome renderer regex | Chrome processes that may be killed |
| `WATCHDOG_CHROME_MIN_MB` | `500` | Minimum footprint for a Chrome candidate |
| `WATCHDOG_FORCE=1` | | Act as if pressure were critical |
| `WATCHDOG_DRY_RUN=1` | | Log the choice without killing |
| `WATCHDOG_ONCE=1` | | Run one check and exit |

Test without killing anything:

```sh
WATCHDOG_FORCE=1 WATCHDOG_DRY_RUN=1 WATCHDOG_ONCE=1 cargo run --release && tail -1 ~/Library/Logs/memory-watchdog.log
```

## Development

```sh
cargo test
```
