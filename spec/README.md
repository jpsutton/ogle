# ogle specification

ogle is a terminal system monitor inspired by Glances. It shows a dense, single-screen
view of CPU, memory, swap, load, network, disk I/O, file systems, sensors and processes,
and raises alerts when values cross thresholds. Its main goal is to give the same
day-to-day value as Glances at a small fraction of the CPU and memory cost. Phase 1 covers
the interactive TUI only, on Linux, macOS, FreeBSD, OpenBSD, NetBSD and Windows.

## Files

| File | Content | ID prefix |
|---|---|---|
| `00-overview.md` | Purpose, scope, document map, conventions and cross-cutting decisions | `GEN-` |
| `01-runtime-ui.md` | CLI options, config file, main loop, input, layout, rendering, colours, popups, help and hotkeys | `UI-` |
| `02-system-modules.md` | system, uptime, now, cpu, percpu, load, mem, memswap and quicklook modules | `SYS-` |
| `03-io-modules.md` | network, diskio, fs and sensors modules | `IO-` |
| `04-processes.md` | Process collection, processcount, processlist, programlist, sort, filter, kill, renice and pin | `PROC-` |
| `05-alerts.md` | Threshold evaluation, decorations, event list and the alert panel | `ALR-` |
| `06-platform-sources.md` | OS data source, unit, cost and availability for every field on each platform | `PLT-` |
| `07-performance.md` | Resource budgets, efficiency rules and Glances baseline measurements | `PERF-` |

## Reading order

1. `00-overview.md` first. Its decisions bind every other file.
2. `01-runtime-ui.md` for the program shell that hosts every module.
3. `05-alerts.md` next, because the module files refer to its threshold model.
4. The module files: `02-system-modules.md`, `03-io-modules.md`, `04-processes.md`.
5. `06-platform-sources.md` when implementing a platform or checking field availability.
6. `07-performance.md` last, as the budgets every implementation choice must meet.
