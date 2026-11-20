# ogle specification: overview and cross-cutting decisions

Status: draft, phase 1 (interactive TUI only)
Reference implementation: Glances 4.5.9 (Python, psutil 7.2.2)

## 1. Purpose

ogle is a terminal system monitor inspired by Glances. It shows a dense, single-screen
overview of CPU, memory, swap, load, network, disk I/O, file systems, sensors and
processes, and it raises alerts when values cross thresholds.

The main reason ogle exists is resource efficiency. Glances is written in Python on top
of psutil. It uses tens of megabytes of RAM and a noticeable share of one CPU core. ogle
must give the same day-to-day value at a small fraction of that cost. See
`07-performance.md` for the budgets.

## 2. Scope

### 2.1 In scope (phase 1)

- The interactive full-screen terminal UI ("standalone mode" in Glances terms).
- These modules: system, uptime, now (clock), cpu, percpu, load, mem, memswap,
  quicklook, network, diskio, fs, sensors (temperature, fan, battery), processcount,
  processlist, programlist, alert.
- Process interaction: sort, filter, select, kill, renice, pin (extended view).
- A configuration file and command-line options for the above.
- Target platforms: Linux, macOS, FreeBSD/OpenBSD/NetBSD, Windows.

### 2.2 Out of scope (phase 1)

- Server/daemon mode, client mode, the client browser, the web UI, the REST API, MCP and SNMP.
- Exporters (CSV, InfluxDB, Prometheus, graph generation and so on), plus stdout/JSON/fetch modes.
- Optional Glances modules: containers, vms, gpu, npu, mpp, raid, smart, wifi, irq, ip,
  public ip, cloud, ports, folders, connections, AMPs, hddtemp.
- Action scripts that run on thresholds (`*_action`).
- Update checks and any other network access.

Later phases may add these. The architecture must not prevent them (see section 6), but
phase 1 must not carry their cost.

## 3. Relationship to Glances

ogle is "inspired by" Glances, not a drop-in replacement:

- Screen layout, information density, colour semantics and most hotkeys follow Glances.
  A Glances user should feel at home.
- ogle uses its own configuration file format and its own command-line options. They do
  not have to be compatible with `glances.conf` or Glances flags.
- ogle fixes Glances bugs and quirks that the research found. Each section lists the
  quirks that apply to it and what ogle does instead.
- Where Glances behaviour is expensive and gives little value, ogle may simplify it.
  For example, ogle keeps no 1200-sample history buffers when only the last 30 samples
  are ever shown.

## 4. Document map

| File | Content | Requirement ID prefix |
|---|---|---|
| `00-overview.md` | This file: scope, decisions, conventions | `GEN-` |
| `01-runtime-ui.md` | CLI, config file, main loop, input, layout engine, rendering, colours, popups, help, hotkeys | `UI-` |
| `02-system-modules.md` | system, uptime, now, cpu, percpu, load, mem, memswap, quicklook | `SYS-` |
| `03-io-modules.md` | network, diskio, fs, sensors | `IO-` |
| `04-processes.md` | process collection, processcount, processlist, programlist, sort, filter, kill, nice, pin | `PROC-` |
| `05-alerts.md` | threshold evaluation, decorations, event list, alert panel | `ALR-` |
| `06-platform-sources.md` | Data sources for every metric on each OS | `PLT-` |
| `07-performance.md` | Resource budgets, efficiency rules, Glances baseline measurements | `PERF-` |

## 5. Conventions

- MUST, MUST NOT, SHOULD and MAY follow RFC 2119.
- Each normative statement has a stable ID such as `UI-012`. Number IDs in steps of 1
  within each file. Never reuse an ID.
- "Module" means one stats collector plus its on-screen panel. Glances calls these
  "plugins". ogle does not use the word plugin, which is kept for a possible future
  extension mechanism.
- "Cycle" means one refresh: sample all due modules, then redraw.
- "Rate" means a per-second value derived from two samples of a monotonic counter.
- Units: B, KiB-style binary scaling shown with the short Glances symbols (K, M, G, T, P).
  Network shows bits by default (see `03-io-modules.md`).
- References like `glances/plugins/cpu/__init__.py:296` point into the Glances 4.5.9
  source and are given for implementers. They are informative, not normative.

## 6. Cross-cutting decisions

These decisions bind every section file.

- **GEN-001 Language-agnostic.** The spec names no implementation language. It does
  state budgets that rule out heavyweight runtimes in practice (see `07-performance.md`).
- **GEN-002 No runtime dependencies** beyond the OS and its standard system libraries.
  No Python, no interpreter, no psutil. ogle reads OS sources directly
  (`/proc` and `/sys` on Linux, sysctl/libproc/Mach on macOS, sysctl/kvm on BSD,
  Win32/NT APIs on Windows).
- **GEN-003 Single static-ish binary.** ogle ships as one executable. No data files are
  required at run time. Help text and defaults are compiled in.
- **GEN-004 Module model.** Each module has: an enabled flag, a refresh interval, a
  `collect()` step that returns raw samples, a derived-stats step (rates, percentages,
  thresholds), and a `render(width, max_height)` step that returns styled lines.
  A disabled module MUST NOT collect anything (no file reads, no syscalls).
- **GEN-005 Platform abstraction.** All OS access goes through one platform interface
  per data domain (cpu, memory, load, network, disk, fs, sensors, processes, system info).
  Each platform implements the interface. A field the platform cannot provide is
  "absent", not zero. Panels hide absent fields, as Glances does, unless a module file
  says otherwise (for example, the process CPU core column shows `-`, PROC-052).
  `06-platform-sources.md` lists which fields each platform has.
- **GEN-006 Per-module refresh intervals.** The global refresh interval defaults to 2 s.
  Modules may have longer defaults: system 60 s, fs 60 s, sensors 10 s. Config can
  override every interval. A module that is not due in a cycle shows its last values.
- **GEN-007 Rates.** Compute rates as `(counter_now - counter_prev) / elapsed_seconds`,
  using a monotonic clock and floating-point division (Glances uses floor division).
  On the first sample, or when a counter goes backwards (wrap or reset), the rate is
  "unknown" and shows as `-`. It is not shown as 0.
- **GEN-008 History.** Keep only the history the UI needs: 30 samples for trend arrows,
  plus the sparkline width when sparklines are enabled. Use fixed-size ring buffers that
  are allocated once.
- **GEN-009 Thresholds and alerts** follow Glances semantics (careful/warning/critical,
  percent-of-maximum, `_LOG` decorations, the event list). `05-alerts.md` is the single
  source of truth, including the config layout: every limit lives in the
  `[<module>.thresholds]` sub-table (ALR-011, ALR-012). Module sections only state which
  fields are evaluated, against which maximum, and the default limits. Every limit,
  including load, is in percent of the stat's maximum.
- **GEN-010 Hotkeys.** Keep Glances hotkeys for in-scope features. Keys bound only to
  out-of-scope Glances modules (`6 7 8 A C D G I K Q R V W g`) are unbound in phase 1
  and reserved for later. Two keys that Glances binds to out-of-scope modules are
  reused in phase 1: `r` reverses the process sort (PROC-064) and `P` sorts by PID
  (PROC-060). `01-runtime-ui.md` owns the full key table.
- **GEN-011 Glances quirks.** Do not reproduce Glances bugs. Examples: the raw codes 65/66
  that make `A`/`B` move the cursor, the help screen that does not match the bindings,
  network speed scaled by 1048576 instead of 10^6, "cumulative" network mode showing
  deltas, the `processs` typo, and the timer that measures elapsed instead of remaining
  time. Each section names the quirks it fixes.
- **GEN-012 Configuration format.** TOML. Search order: the `--config PATH` option, then
  `$XDG_CONFIG_HOME/ogle/config.toml` (default `~/.config/ogle/config.toml`) on
  Unix-like systems including macOS, and `%APPDATA%\ogle\config.toml` on Windows.
  A missing file is not an error. One TOML table per module, named after the module
  (`[cpu]`, `[network]`, ...), plus `[global]` and `[ui]`. Threshold limits live in a
  `[<module>.thresholds]` sub-table (ALR-011). Unknown keys produce one
  warning line on stderr before the UI starts. They are not fatal.
- **GEN-013 Terminal handling.** Use raw terminal I/O (termios plus ANSI/VT sequences on
  Unix, the console VT mode on Windows 10 and later). Do not depend on a heavyweight
  curses runtime. A small terminfo-aware layer or a lightweight library is acceptable.
  Use differential redraw: write only the cells that changed since the last frame.
- **GEN-014 Privileges.** ogle runs unprivileged. Data that needs privileges (other
  users' process I/O, some process details) shows `?` and never stops the program.
  ogle never asks for elevation.
- **GEN-015 No network access, no subprocesses.** In phase 1 ogle MUST NOT open sockets
  or spawn processes. (Glances talks to hddtemp over TCP. That is out of scope.)
- **GEN-016 Accessibility of colour.** Every colour-coded state MUST also be visible on
  monochrome terminals through attributes (bold, underline, reverse), as Glances does.
  `--no-color` and the `NO_COLOR` environment variable force monochrome.
- **GEN-017 Unicode.** UTF-8 box and arrow glyphs by default. `--ascii` (or a non-UTF-8
  locale) switches to the ASCII fallbacks (`^ v - _` and so on).

## 7. Open questions (to resolve before or during implementation)

- OQ-1: Minimum supported OS versions per platform (proposed: Linux 4.x kernels,
  macOS 12+, FreeBSD 13+, Windows 10 1809+).
- OQ-2: Whether NetBSD and OpenBSD are tier 1 or best effort (proposed: best effort).
- OQ-3: Whether the config file should support per-host overrides (proposed: no).
