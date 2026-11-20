# ogle specification: processes

Status: draft, phase 1
Covers: process enumeration and per-process fields, collection cadence and caching, the
processcount summary line, the processlist and programlist panels, sorting, the cursor,
filtering and focus, filter summary rows, kill, renice, and the pinned (extended) view.

Related files:

- `00-overview.md`: scope and the cross-cutting decisions (GEN-xxx). They bind this file.
- `01-runtime-ui.md`: the full hotkey table, the popup mechanics (geometry, text input,
  confirmation), the layout engine and the colour roles. This file names the keys and
  popups it needs; `01-runtime-ui.md` defines how they work.
- `02-system-modules.md`: the `auto_unit` number formatter, referred to here as
  "auto-unit" and "auto-unit (low precision)".
- `05-alerts.md`: threshold evaluation, decorations and the event list. The auto-sort
  rules here consume events that `05-alerts.md` defines.
- `06-platform-sources.md`: where each field comes from on each OS. This file says only
  *what* a field means. When a platform cannot provide a field, the field is "absent"
  (GEN-005).
- `07-performance.md`: numeric budgets. This file states the structural rules that make
  those budgets reachable.

Glances references point into `/usr/lib/python3.14/site-packages/glances/` (4.5.9). They
are informative.

---

## 1. Process identity

- **PROC-001 Process key.** ogle identifies a process by the pair (PID, start time).
  Start time is the platform's process creation timestamp at whatever resolution the OS
  provides (Linux: `starttime` in clock ticks since boot). If a PID comes back with a
  different start time, ogle treats it as a new process: it discards all cached data,
  previous counters and cached strings for the old one.
- **PROC-002 Self.** ogle lists its own process like any other. It does not hide itself.
- **PROC-003 Excluded pseudo-processes.** These entries are never listed and never
  counted:
  - Windows: PID 0 "System Idle Process".
  - FreeBSD/OpenBSD/NetBSD: the `idle` kernel process.
  - macOS: PID 0 `kernel_task`.
  - Any process that has exited between enumeration and reading its fields. ogle drops
    it silently for this cycle. This is not an error.

## 2. Per-process fields

- **PROC-010 Field list.** For each process ogle keeps the fields below. "Tier" decides
  how often a field is read (section 3).

| Field | Meaning | Tier |
|---|---|---|
| `pid` | Process ID | base |
| `ppid` | Parent PID (needed for kernel-thread detection on some OSes, and kept for a future tree view) | base |
| `start_time` | See PROC-001 | base |
| `name` | Short executable name as the kernel reports it (Linux `comm`, up to 15 characters) | base |
| `status` | One-letter state (PROC-012) | base |
| `cpu_time_user`, `cpu_time_system` | Cumulative CPU seconds | base |
| `cpu_percent` | Derived; see PROC-013 | derived |
| `rss` | Resident memory in bytes | base |
| `vms` | Virtual memory size in bytes | base |
| `memory_percent` | `rss / total physical memory * 100` | derived |
| `num_threads` | Thread count | base |
| `nice` | Nice value on Unix; priority class on Windows (PROC-043) | base |
| `cpu_num` | Last CPU the process ran on. Absent on platforms that do not expose it | base |
| `uid` | Effective user ID (Unix; PLT-143, PLT-148, PLT-151) or user SID (Windows) | base |
| `is_kernel_thread` | PROC-020 | base |
| `cmdline` | Argument vector | cached string |
| `username` | Name resolved from `uid` | cached string |
| `io_read_bytes`, `io_write_bytes` | Cumulative storage I/O bytes | on demand |
| extended fields | Section 13 | pinned only |

- **PROC-011 Absent vs. denied.** Two different "no value" states exist, and the UI shows
  them differently:
  - *absent*: the platform has no such field at all. The column is not shown (PROC-052).
  - *denied*: the field exists, but this process's value cannot be read (permissions, or
    the process is a zombie). The cell shows `?`.
- **PROC-012 Status letter.** ogle maps each OS's states onto one letter:
  `R` running, `S` sleeping (interruptible), `D` uninterruptible disk sleep, `I` idle
  kernel thread, `Z` zombie, `T` stopped, `t` tracing stop, `W` waiting/paging,
  `X` dead, `L` locked, `P` parked, `K` wakekill. The Linux letters are used unchanged. On other OSes
  each native state maps to the nearest letter; `06-platform-sources.md` gives the
  tables. Windows uses only `R` (a thread is running, ready or on standby), `T` (every
  thread is suspended) and `S` (anything else), as PLT-150 defines.
- **PROC-013 CPU percent.** For each process:
  `cpu_percent = (Δ(cpu_time_user + cpu_time_system) / Δwall_seconds) * 100`, where
  `Δwall_seconds` comes from ogle's monotonic clock between the two process samples. On
  a multi-core machine the value can exceed 100 ("Irix mode", the default). On a
  process's first sample, `cpu_percent` is unknown. It shows as `-` and sorts as 0
  (GEN-007).
- **PROC-014 Memory percent** uses the same "total physical memory" as the mem module
  (`02-system-modules.md`). It is computed from `rss` and needs no extra syscall.

## 3. Collection cadence and caching

The process table is by far the most expensive thing ogle collects. These rules keep the
cost proportional to what is on screen wherever possible.

- **PROC-020 Kernel threads.** `is_kernel_thread` is true for kernel threads. On Linux it
  comes from the `PF_KTHREAD` flag (0x00200000) in `/proc/<pid>/stat` field 9. The
  fallback, for kernels without that flag, is "PID 2 (`kthreadd`) or a process whose
  parent PID is 2". On other OSes see `06-platform-sources.md`. Where the platform has
  no kernel-thread flag (macOS and Windows, PLT-149 and PLT-157), the field is false for
  every process.
- **PROC-021 Hiding kernel threads.** With `--hide-kernel-threads`, or config
  `[processlist] hide_kernel_threads = true`, kernel threads are excluded from the list,
  the counts and the filter summary. Default: shown. On platforms without the
  kernel-thread flag (PROC-020) the option is accepted and has no effect. ogle MUST NOT read any per-process
  data beyond what it needs for the kernel-thread test for a hidden kernel thread.
- **PROC-022 Base tier.** Every process-module cycle, ogle enumerates all processes and
  reads the base-tier fields. It reads them in as few syscalls/files per process as the
  OS allows (on Linux: one read of `/proc/<pid>/stat`, which supplies vsize and rss
  through fields 23 and 24; `statm` is not read; see PLT-147 in
  `06-platform-sources.md`). The base tier is needed for every process, because the
  counts and the CPU/memory sort cover all processes.
- **PROC-023 Cached strings (cmdline).** The cmdline is read once per process key
  (PROC-001) and cached until the key disappears (PLT-125). There is no periodic
  re-read (Glances re-reads every 60 s). The only exception: ogle re-reads it when the
  process's `name` changes, which catches `exec` without a new PID.

  A process with an empty cmdline (typical for kernel threads and zombies) is cached as
  empty. It is not re-read every cycle.
- **PROC-024 Username cache.** ogle resolves `uid` to a user name through a UID-to-name
  map that lives for the whole session. A UID is resolved once, on first sight. A UID
  that cannot be resolved displays as its number and is cached as such. Rationale: the
  name lookup (NSS, `getpwuid_r`, LookupAccountSid) can be slow and must not run once per
  process per cycle.
- **PROC-025 Lazy strings.** Cmdline and username are fetched only for processes that
  need them:
  - processes in visible rows;
  - all processes, while a filter or focus list is active that uses the `cmdline` or
    `username` key, or no key at all (PROC-071);
  - all processes, while the sort key is `user`;
  - all members, in programs mode only for the `username` agreement check (PROC-131).

  Once fetched, the value stays cached (PROC-023/024), so the extra cost occurs once per
  process, not once per cycle.
- **PROC-026 On-demand I/O counters.** `io_read_bytes` and `io_write_bytes` are read for:
  - processes in visible rows while the R/s and W/s columns are shown (PROC-052), and
  - all processes while the sort key is `io`.

  On platforms where per-process I/O is absent (the BSDs, PLT-153), nothing is read.
  ogle keeps the last I/O sample per process key. A process that has no previous sample
  (first sample, or just scrolled into view) shows `-` in R/s and W/s for one cycle.
  Glances reads I/O for every process every cycle; ogle does not.
- **PROC-027 Process refresh interval.** The process module refreshes at the global
  refresh interval (default 2 s). Config can set `[processlist] refresh`. The
  processcount, processlist and programlist panels all read the same snapshot. ogle
  takes the snapshot once per process refresh.
- **PROC-028 Reaping caches.** At the end of each process refresh, ogle drops every
  cache entry whose process key was not seen in this enumeration. Memory use therefore
  stays bounded by the live process count.
- **PROC-029 Disabling processes.** The `z` key and the `--no-processes` option stop
  all process enumeration (GEN-004). The panels show the text in PROC-031. Re-enabling
  makes the next sample a "first sample", so every CPU% shows `-` for one cycle.
- **PROC-030 Forced refresh.** The refresh key (F5 / Ctrl-R, see `01-runtime-ui.md`)
  clears the cmdline cache and the username cache, and then takes a new snapshot at once.

## 4. processcount line

- **PROC-031** The processcount panel is one line, two when a filter or focus is active.
  - Disabled (PROC-029): `PROCESSES DISABLED (press 'z' to display)`.
  - Filter line (only when a filter is set), drawn first:
    `Processes filter:` (TITLE) + ` <pattern>` (FILTER colour) + ` on column <key>` when
    the pattern has a key (PROC-070) + ` ('ENTER' to edit, 'E' to reset)` (DEFAULT).
  - Focus line (only when a focus list is set): `Focus on:` (TITLE) + the
    comma-separated focus patterns (FILTER).
  - Summary line:
    `TASKS` (TITLE), then the total as `{:>4}`, then ` ({threads} thr),`, then
    ` {running} run,`, then ` {sleeping} slp,`, then ` {other} oth `.
    After that comes `Threads` or `Programs` (programs mode), followed by either
    ` sorted automatically by <human>` (auto sort) or ` sorted by <human>` (manual sort).
- **PROC-032 What is counted.** The counts cover every listed process after exclusions
  (PROC-003) and after kernel-thread hiding (PROC-021), but *before* the user filter and
  focus. This matches Glances. `running` counts status `R`. `sleeping` counts status `S`
  and `I`. `other` is `total - running - sleeping`. `threads` is the sum of
  `num_threads`.
- **PROC-033 Human sort names.**

| Sort key | Text |
|---|---|
| `cpu` | `CPU consumption` |
| `mem` | `memory consumption` |
| `io` | `disk IO` |
| `time` | `process time` |
| `user` | `user name` |
| `name` | `process name` |
| `pid` | `process ID` |
| `core` | `CPU core number` |

## 5. processlist panel: columns

- **PROC-040 Column order.** Left to right: selector, CPU%, MEM%, VIRT, RES, PID, USER,
  TIME+, THR, NI, S, R/s, W/s, CPU (core), Command. Every cell except Command ends with
  one space of separation, as below.

| Column | Header | Cell format | Width rule | Optional (PROC-051) |
|---|---|---|---|---|
| selector | (none) | `>` if this row is the cursor row, else space | 1 | no |
| CPU% | `CPU%` | `{:<6.1f}`; values ≥ 100 use `{:<6.0f}`; unknown `-` | 6 | no |
| MEM% | `MEM%` | `{:<5.1f} ` | 6 | no |
| VIRT | `VIRT` | auto-unit of `vms`, `{:<5} ` | 6 | yes |
| RES | `RES` | auto-unit of `rss`, `{:<5} ` | 6 | yes |
| PID | `PID` | right-aligned in W, then space. W = number of digits in the platform's maximum PID (`pid_max()`, PLT-128), at least 5 and at most 7. Where `pid_max` is absent (Windows), use the digits of the largest live PID, with the same bounds | W+1 | no |
| USER | `USER` | username cut to 9 characters, `{:<10} ` | 10 | no |
| TIME+ | `TIME+` | PROC-045, right-aligned in 8, then space | 9 | yes |
| THR | `THR` | `{:<3} ` | 4 | no |
| NI | `NI` | `{:>3} ` (PROC-043 for Windows) | 4 | no |
| S | `S` | `{:>1} ` | 2 | no |
| R/s | `R/s` | auto-unit (low precision) of the read rate, `{:>4} ` | 5 | yes |
| W/s | `W/s` | auto-unit (low precision) of the write rate, `{:<4} ` | 5 | yes |
| CPU (core) | `CPU` | `cpu_num` as `{:>3} `; `-` when unknown or absent on the platform (PROC-052) | 4 | yes |
| Command | `Command` | PROC-046; takes all remaining width | rest | no |

  The header row uses the same widths. The header of the column that is the active sort
  key uses the SORT decoration. The Command header is followed by ` ('e' to pin | 'k' to
  kill)` whenever the list has a cursor row.

- **PROC-041 Cell colours.**
  - CPU%: stat `processlist.cpu` evaluated on the *displayed* value (after per-core
    normalisation, PROC-044), maximum 100, default limits 50/70/90 (`05-alerts.md`).
    Decoration only (ALR-007), no event logging.
  - MEM%: stat `processlist.mem` on `memory_percent`, maximum 100, default limits
    50/70/90. Decoration only, no event logging.
  - Limits use the `[processlist.thresholds]` layout of ALR-011:
    `cpu = { careful = 50, warning = 70, critical = 90 }` and the same for `mem`. The
    program list uses the same stats and limits.
  - MAX: exactly the ALR-040 rule. Among the rows the list shows after filtering
    (filter, focus and kernel-thread hiding), the top CPU% cell and the top MEM% cell
    get the MAX decoration, but only when that cell would otherwise be OK and its value
    is greater than 0. Ties: the first row in display order wins. Glances computes this
    but never applies it; ogle applies it.
  - TIME+: CPU_TIME decoration when total CPU time is 1 hour or more.
  - NI: config lists `[processlist] nice_ok`, `nice_careful`, `nice_warning` and
    `nice_critical` (lists of integers). The first matching list in order
    critical, warning, careful, ok decides the decoration. Default: `nice_warning` =
    every value except 0 (written as the special value `"nonzero"`); the others are empty.
    No match means DEFAULT.
  - S: config lists `status_ok`, `status_careful`, `status_warning`, `status_critical`
    (lists of letters), checked in the same order. Defaults: ok = `R, W, P, I`,
    critical = `Z, D`. No match means DEFAULT.
  - The NI and S match lists are not percent-of-maximum limits and are not evaluated
    through `05-alerts.md`, so they stay as plain keys in `[processlist]`, not in
    `[processlist.thresholds]`.
  - Command: the executable part uses PROCESS (green); on the cursor row it uses
    PROCESS_SELECTED (green, underlined). The path prefix and the arguments use DEFAULT.
- **PROC-042 Denied and absent values.** A denied field shows `?`, aligned like the
  value would be. When a process's VIRT/RES/TIME+ cannot be read, the cell shows `?`
  rather than hiding the row.
- **PROC-043 NI on Windows.** Windows has no nice value. The NI column shows the
  priority class (`priority_class`, PLT-155) as a 2-letter label:
  `RT` realtime, `Hi` high, `AN` above normal, `No` normal, `BN` below normal,
  `Lo` idle. Colour lists match these labels.
- **PROC-044 Per-core normalisation (Irix mode).** Default: normalisation off ("Irix
  mode"), so CPU% is the share of *one* core and can exceed 100. The `0` key (UI-043),
  the `-0/--normalize-cpu` option and `[ui] normalize_cpu` turn normalisation on. With
  it on, each CPU% is divided by the logical core count, and the header becomes `CPU%/N`
  (N = core count) when N < 10, or `CPUi` otherwise. The threshold uses the displayed
  value in both modes. The same key also switches the load panel to load-as-percent
  (SYS-072).
- **PROC-045 TIME+ format.** Let `t = cpu_time_user + cpu_time_system` in seconds,
  `h = t // 3600`, `m = (t % 3600) // 60`, `s = t % 60` (integers).
  - `h ≥ 100`: `{h}h`.
  - `1 ≤ h ≤ 99`: `{h}h{m}:{s:02}` (for example `3h7:05`). Minutes are not zero-padded.
  - `h = 0`: `{m}:{s:02}` (for example `0:42`, `12:03`).

  This is the Glances format. ogle keeps it. Denied: `?`.
- **PROC-046 Command cell.** Built from `cmdline` (argv) and `name`:
  1. Empty cmdline: show `[name]`, all in PROCESS colour.
  2. Otherwise split `argv[0]`:
     - If `argv[0]` starts with `name`: path = empty, exe = `argv[0]`.
     - Else: path = the directory part of `argv[0]`, exe = its last component.
  3. Short-name mode (default): show exe only. Long-name mode: show `path/` before exe
     when path is non-empty. Unlike Glances, ogle does not check that the path exists on
     disk (that costs a syscall per row).
  4. Arguments = `argv[1..]` joined with single spaces, after one space.
  5. Before display, replace control characters (code < 0x20 and 0x7F) and newlines in
     every part with a space. Never write raw control bytes to the terminal.
  6. Argument scroll: an offset in characters into the arguments string, shared by all
     rows. Left/Right (see `01-runtime-ui.md`; Shift-Left/Shift-Right when
     `arrow_keys_sort` is on, UI-047) change it in steps of 1 (min 0). The `<` and `>`
     keys change it in steps of 8. When the offset is > 0, the shown arguments start
     with the scrolled-command marker `…` (`~` in ASCII mode, UI-081). The offset resets to 0 when
     the sort key, filter or mode changes.
  7. The cell is cut at the right edge of the screen. It never wraps.
- **PROC-047 Short/long name.** `/` toggles short-name mode. Short names are the
  default. The `--long-names` option, or config `[processlist] short_name = false`,
  starts in long-name mode.
- **PROC-048 Number formats.** VIRT, RES and the I/O rates use auto-unit as defined in
  `02-system-modules.md`. R/s and W/s show `0` for a zero rate.

## 6. processlist panel: layout

- **PROC-050 Placement.** The panel sits in the right column below processcount, above
  the alert panel. `01-runtime-ui.md` defines the column.
- **PROC-051 Optional columns.** VIRT, RES, TIME+, R/s, W/s and CPU (core) are optional.
  They are shown only when the terminal width is more than 102 columns. Below that
  width they disappear from both header and rows. ogle does not read I/O counters while
  R/s and W/s are hidden (PROC-026), except when sorting by `io`.
- **PROC-052 Absent columns.** A column whose field the platform does not provide at
  all is never shown at any width. Known case: R/s and W/s on the BSDs (PLT-153).
  Exception: the CPU (core) column follows PROC-051 like other optional columns and
  shows `-` in every row where `last_cpu` is absent (macOS, Windows). Where per-process
  I/O depends on privilege (Linux and macOS, other users' processes), the columns are
  shown and the denied cells show `?` (PROC-011).
- **PROC-053 Row count.** The visible row count is
  `panel_height - header_lines - extended_lines - summary_lines`, where `panel_height`
  is the space the layout engine gives the panel (screen height minus the panels above,
  minus the alert panel below). With 0 or fewer rows the panel shows only its header.
- **PROC-054 Extended config.** Config `[processlist] disable_columns = [...]` lists
  columns to hide always. Valid names: `virt`, `res`, `time`, `thr`, `ni`, `s`, `ior`,
  `iow`, `core`. A hidden column's data is not collected unless something else needs
  it (sort key or filter). CPU%, MEM%, PID, USER and Command cannot be hidden.

## 7. Sorting

- **PROC-060 Sort keys.**

| Key name | Hotkey | Sorts by | Default direction |
|---|---|---|---|
| `auto` | `a` | PROC-062 | n/a |
| `cpu` | `c` | `cpu_percent` (unknown = 0) | descending |
| `mem` | `m` | `rss` | descending |
| `io` | `i` | read rate + write rate (unknown = 0) | descending |
| `time` | `t` | `cpu_time_user + cpu_time_system` | descending |
| `name` | `p` | exe name (case-insensitive) | ascending |
| `user` | `u` | username, then uid | ascending |
| `core` | `o` | `cpu_num` | ascending |
| `pid` | `P` | pid | ascending |

  These names are used everywhere (CLI `--sort`, config `sort_key`, UI-047): `auto cpu
  mem io time name user core pid`. `P` (sort by PID) is new in ogle. Glances binds `P`
  to the out-of-scope ports module; ogle frees it (GEN-010). In programs mode `pid`
  sorts by process count (`nprocs`), descending.

  Absent and denied values (for example I/O on the BSDs, PLT-153, or `cpu_num` on
  macOS and Windows) sort after every known value, in either direction. Unknown values
  (first sample) count as 0 as stated in the table. Ties among them follow PROC-061.
- **PROC-061 Tie-breaks.** Equal primary keys break ties by `rss` descending, then by
  `pid` ascending. The order is therefore total and stable between refreshes, so rows
  do not shuffle while their values are equal.
- **PROC-062 Auto sort.** Default at start-up. Auto sort means `cpu`, except while an
  alert event is ongoing (see `05-alerts.md` for event types):
  - an ongoing `MEM` or `MEMSWAP` event makes it sort by `mem`;
  - an ongoing `CPU_IOWAIT` event makes it sort by `io`;
  - any other ongoing event (and no event) sorts by `cpu`.

  When several events are ongoing, the newest one decides. When the deciding event ends,
  the next newest ongoing event decides, or `cpu` if none. Glances jumps back to plain
  auto on every event end, even while other events are ongoing; ogle does not. Auto sort
  never changes direction.
- **PROC-063 Manual sort.** Pressing any sort hotkey other than `a` leaves auto mode.
  Alerts then no longer change the sort. `a` returns to auto mode.
- **PROC-064 Reverse.** `r` reverses the current direction. It is a toggle, and it
  resets when the sort key changes. The header shows the active sort column with the
  SORT decoration. In reverse it adds a `↑` marker (`^` in ASCII) after the header text,
  where the column width allows. Glances binds `r` to the out-of-scope SMART module;
  ogle binds it to reverse in phase 1 (GEN-010, UI table in `01-runtime-ui.md`).
- **PROC-065 Cycling.** Shift-Left and Shift-Right step through every key except
  `auto`, in the order `cpu, mem, io, time, name, user, core, pid` (wrapping around),
  and leave auto mode (UI-047). With
  `--arrow-keys-sort`, plain and Shift arrows swap roles (`01-runtime-ui.md`).
- **PROC-066 Start-up sort.** `--sort KEY` or config `[processlist] sort_key = "KEY"`
  with any key from PROC-060. The option wins over config. Default `auto`.
- **PROC-067 Sort cost.** Sorting MUST NOT need fields that are not already collected,
  except as PROC-025 and PROC-026 allow (`user` needs all usernames; `io` needs all I/O
  counters). Switching away from those keys stops the extra collection on the next cycle.

## 8. Filter and focus

- **PROC-070 Filter syntax.** A filter is `[key:]pattern`. The text splits at the first
  `:` only when the part before it is a valid key. Otherwise the whole text is the
  pattern, so `^/usr/lib/x86_64:` works without a key. Valid keys:
  - `name`: the exe name;
  - `cmdline`: the full command line, argv joined with single spaces;
  - `username`: the user name;
  - `pid`: the decimal PID;
  - `status`: the status letter.
- **PROC-071 No key.** Without a key, a process matches if the pattern matches `name`
  OR the full joined `cmdline`. Glances tests only `argv[0]`; ogle tests the full line.
- **PROC-072 Pattern semantics.**
  - The pattern is a regular expression in a linear-time dialect (RE2-style: no
    backreferences, no lookaround). Matching MUST NOT backtrack exponentially.
  - Matching is a *search* (unanchored). Use `^` and `$` to anchor. Glances anchors
    implicitly (`re.fullmatch`), so in Glances `python` does not match `python3`. In ogle
    it does.
  - Case: smart case. The match is case-insensitive when the pattern contains no
    uppercase letter, and case-sensitive otherwise.
  - An invalid regex does not clear the filter silently, as Glances does. The filter
    popup shows the error under the input line and keeps the popup open for correction.
    An invalid regex from the command line is a start-up error (exit code 2, with a
    message on stderr).
- **PROC-073 Setting the filter.** Enter opens the filter popup, pre-filled with the
  current filter (`01-runtime-ui.md` defines popups and text input). The popup shows
  these examples: `python`, `^/usr/lib`, `name:^nginx$`, `cmdline:--config`,
  `username:^root$`, `pid:^12`. An empty input clears the filter. `E` clears the filter
  without a popup. `-f/--filter PATTERN` sets the filter at start-up.
- **PROC-074 Focus list.** `--focus P1,P2,...` or config `[processlist] focus = [...]`.
  Each entry uses the filter syntax. A process is shown if it matches ANY focus entry.
  When both a focus list and a filter are set, a process must match the focus list AND
  the filter (Glances lets the focus list replace the filter; ogle combines them). Focus
  can only be set at start-up in phase 1. `E` does not clear it.
- **PROC-075 Filtered lists.** Filter and focus apply to the processlist and programlist
  rows and to the filter summary. They do not apply to processcount (PROC-032). In
  programs mode the filter applies to programs: `name` matches the program name, and the
  other keys match if ANY member process matches.

## 9. Filter summary rows

- **PROC-080** While a filter or focus is active, three rows follow the process rows,
  using the same columns:
  1. `current`: sums over all matching processes (not only visible ones) of CPU%,
     MEM%, VIRT, RES, R/s and W/s. Other cells are blank. The Command cell reads
     `< current`.
  2. `min`: the lowest value of each of those sums since the filter was set (or since
     the last reset). Command cell `< min ('M' to reset)`.
  3. `max`: the highest value of each sum since the filter was set or reset. Command cell
     `< max ('M' to reset)`.

  A separator line of `_` characters (ASCII) or `─` (Unicode), as wide as the columns
  before Command, comes before these rows. All three rows use the FILTER decoration.
- **PROC-081** Min/max reset when the filter or focus changes, and when `M` is pressed.
  `M` has no effect without a filter. Sum cells that include unknown values (`-`)
  treat them as 0. A sum with no known values shows `-`.
- **PROC-082** The summary rows count against the panel height (PROC-053). When rows are
  short, the summary keeps its place and the process rows give way.

## 10. Cursor and selection

- **PROC-090 Cursor model.** The cursor is a selected *process key* (programs mode: a
  selected program name), not a row index. After each refresh and re-sort, ogle finds
  the selected process's new position and keeps it selected. The list scrolls (keeping
  a scroll offset) so that the cursor row stays visible.
- **PROC-091 When the selected process goes away** (it exits, or no longer matches the
  filter): the cursor moves to the row that now occupies the old index, clamped to the
  last row. With an empty list there is no selection.
- **PROC-092 Navigation.** The cursor covers the whole filtered list, not only the first
  48 rows as in Glances.
  - Up / Down: one row.
  - PageUp / PageDown: one visible page (visible row count minus 1, at least 1).
  - Home / End: first / last row.
  - The view scrolls only when the cursor would leave the visible rows.
  - With the arrow-key sort option, the vertical keys do not change.
- **PROC-093 Initial selection.** At start-up, and whenever the sort key changes, the
  cursor goes to the first row and the scroll offset resets to 0.
- **PROC-094 Cursor and pinning.** Pinning a process (section 13) does not disable or
  lock the cursor (UI-046). The `>` marker stays visible, the cursor keys keep moving
  the selection, and `k`, `+` and `-` act on the cursor process. (Glances disables the
  cursor while a process is pinned.)
- **PROC-095 Cursor and timing.** A cursor key redraws at once from the current
  snapshot. It does not trigger a new sample and does not delay the next one
  (`01-runtime-ui.md` owns the loop timing).

## 11. Kill

- **PROC-100 Trigger.** `k` opens a confirmation popup for the cursor process (or
  program). Without a selected row, or while process collection is off, `k` does
  nothing (UI-105).
- **PROC-101 Popup.** The popup reads:

  ```
  Kill process: <name> (pid <pid>)?

  [t/y] terminate (SIGTERM)   [K] kill (SIGKILL)   [n/Esc] cancel
  ```

  In programs mode the first line is `Kill program: <name> (<n> processes)?`. Each key
  acts on a single keypress; no Enter is needed. `t` or `y` sends SIGTERM, the default
  action. `K` sends SIGKILL. `n` or Esc cancels. Any other key is ignored and the popup
  stays open. `01-runtime-ui.md` (UI-104) defines the same popup.
- **PROC-102 Signal choice per OS.**
  - Unix (Linux, macOS, BSD): `t`/`y` send `SIGTERM`, `K` sends `SIGKILL`.
  - Windows: `t`, `y` and `K` all call `TerminateProcess` with exit code 1. The popup
    shows only `[t/y] terminate   [n/Esc] cancel`.
  - Glances always sends SIGKILL. ogle defaults to the gentler SIGTERM and offers
    SIGKILL as an explicit second choice.
- **PROC-103 Safety.**
  - ogle verifies, just before sending, that the PID still has the same start time
    (PROC-001). If not, it sends nothing and shows `Process <pid> has already exited`
    in the status area (UI-114).
  - ogle refuses to signal its own PID and shows `Refusing to kill ogle itself` in the
    status area.
  - ogle does not wait for the process to exit (Glances blocks the UI for up to 3 s).
    The next refresh shows the result.
- **PROC-104 Programs mode.** The chosen signal goes to every member PID of the program,
  each checked as in PROC-103.
- **PROC-105 Errors.** Permission errors or a vanished process do not stop ogle. A
  one-line message (for example `Cannot kill 1234: Operation not permitted`) shows in
  the status area (UI-114) for 3 seconds. It is not written to stderr while the UI runs.

## 12. Renice

- **PROC-110** `+` raises the nice value of the cursor process by 1,
  which lowers its priority. `-` lowers it by 1, which raises its priority. On Unix this
  uses `setpriority(PRIO_PROCESS, pid, nice±1)`, clamped to the range −20..19. Lowering
  nice usually needs privileges; on failure ogle shows a message as in PROC-105.
- **PROC-111 Windows.** `+` moves the priority class one step down in the order
  Realtime > High > Above normal > Normal > Below normal > Idle; `-` moves one step up.
  ogle never moves a process into Realtime; that step is refused with a message.
- **PROC-112 Programs mode.** `+`/`-` apply to every member process, each with its own
  current value as the base.
- **PROC-113** The process's start time is checked as in PROC-103 before each change. The
  NI cell updates on the next refresh.

## 13. Pinned process (extended view)

- **PROC-120 Pin and unpin.** `e` pins the cursor process. If the cursor process is
  already pinned, `e` unpins it. If another process is pinned, `e` moves the pin to the
  cursor process. `e` does nothing in programs mode. Pinning does not disable the
  cursor (PROC-094). Pinning is interactive only; there is no start-up option (UI-007).
- **PROC-121 Unpin really stops collection.** When unpinned, or when the pinned process
  exits, ogle stops reading extended fields at once and frees the extended state.
  (Glances keeps collecting extended stats for the old PID after unpinning.) When the
  pinned process exits, the block shows `Pinned process <name> (pid <pid>) exited`
  until the user presses `e`.
- **PROC-122 Extended block.** Drawn above the column header row:

  ```
  Pinned process <name> (pid <pid>) ('e' to unpin)
   CPU  min/max/mean: <a>% <b>% <c>%     Affinity: <n> cores     IO nice: <class> <value>
   MEM  min/max/mean: <a>% <b>% <c>%     RSS <x>  VIRT <y>  Swap <z>  Shared <s>
   Open: <t> threads, <f> fds, <tcp> TCP, <udp> UDP          Ctx sw: <v> vol, <i> invol
  <blank line>
  ```

  - Min/max/mean of CPU% and MEM% cover samples since the pin started. They use
    `{:>5.1f}%`.
  - Affinity: the number of CPUs in the process's affinity mask (absent on macOS, OpenBSD and NetBSD).
  - IO nice: Linux I/O scheduling class name (`none`, `RT`, `BE`, `idle`) and level 0..7.
    Absent elsewhere.
  - Memory: RSS, VIRT, Swap (Linux: from `/proc/<pid>/status` `VmSwap`, not by summing
    `smaps` as Glances does), Shared (Linux only: `RssShmem` from the same file). Each
    uses auto-unit. Absent fields are left out.
  - Open: thread count, open file descriptors (Unix) or handles (Windows), and counts of
    TCP and UDP sockets that the process owns. On Linux, macOS and FreeBSD the fd and
    socket counts depend on privilege: for other users' processes they show `?` unless
    ogle runs as root (Linux: `CAP_SYS_PTRACE`). On OpenBSD and NetBSD they are absent
    (section 16 of `06-platform-sources.md`).
  - Context switches (voluntary/involuntary) as rates per second where available.
  - Any field that is denied shows `?`; any field that is absent is left out together
    with its label.
- **PROC-123 Extended cost.** Extended fields are read only for the pinned process, once
  per process refresh. Reading socket counts (on Linux, matching the process's socket
  inodes against `/proc/net/{tcp,tcp6,udp,udp6}`) is the most expensive part. ogle SHOULD
  read socket counts at most every 10 s, even when the refresh is faster (PLT-160).
- **PROC-124 Pinned row.** The pinned process keeps its normal place in the sorted list.
  Its Command cell uses PROCESS_SELECTED, so it can be found in the list.

## 14. programlist panel (programs mode)

- **PROC-130 Toggle.** `j` and the `--programs` option switch the panel between
  processlist (threads) and programlist (programs). The cursor resets (PROC-093), and any
  pin is released.
- **PROC-131 Grouping.** Processes group by `name` (exact, case-sensitive). One program
  row aggregates its members:
  - Summed: `cpu_percent`, `memory_percent`, `rss`, `vms`, `num_threads`, CPU times,
    I/O rates. Unknown member values count as 0. If every member is unknown, the sum is
    unknown.
  - `nprocs`: member count.
  - `username`, `nice`, `status`: the common value if all members agree, else `_`.
    Glances also uses `_`. In programs mode usernames are needed for all members
    (PROC-025).
  - Members: the list of process keys, used for kill and renice.
- **PROC-132 Columns.** Same as processlist (PROC-040) with these changes:
  - `NPROCS` (`{:>7} `, right-aligned) replaces PID.
  - There is no CPU (core) column.
  - The Command cell shows `[name]`, or `name` followed by the arguments of the member
    with the highest CPU% when long-name mode is on.
  - The header's Command text is `Programs` with ` ('k' to kill)` whenever the list has a
    cursor row.
  - The normalised CPU header (normalisation on) is the same as in PROC-044.
- **PROC-133 Sorting.** Programs sort with the same keys and rules as processes, applied
  to the aggregate values. `pid` sorts by `nprocs` (PROC-060). Ties break by `rss`,
  then by name.
- **PROC-134 Counts.** The processcount line keeps counting processes, not programs. Its
  last part reads `Programs sorted ...` (PROC-031).

## 15. Configuration summary

All keys are in the `[processlist]` table of the TOML config (GEN-012). Command-line
options win over config.

| Key | Type | Default | Option | Section |
|---|---|---|---|---|
| `refresh` | seconds | global refresh | — | PROC-027 |
| `hide_kernel_threads` | bool | false | `--hide-kernel-threads` | PROC-021 |
| `short_name` | bool | true | `--long-names` sets false | PROC-047 |
| `sort_key` | string | `auto` | `--sort KEY` | PROC-066 |
| `programs` | bool | false | `--programs` | PROC-130 |
| `focus` | list of strings | empty | `--focus` | PROC-074 |
| `disable_columns` | list of strings | empty | — | PROC-054 |
| `thresholds.cpu` (in `[processlist.thresholds]`) | limit set | 50/70/90 | — | PROC-041 |
| `thresholds.mem` (in `[processlist.thresholds]`) | limit set | 50/70/90 | — | PROC-041 |
| `nice_ok/careful/warning/critical` | list of int, or `"nonzero"` | warning = `"nonzero"` | — | PROC-041 |
| `status_ok/careful/warning/critical` | list of letters | ok = R,W,P,I; critical = Z,D | — | PROC-041 |
| (none) | — | — | `-f/--filter PATTERN` | PROC-073 |
| (none) | — | — | `--no-processes` | PROC-029 |

The per-core normalisation start state is `[ui] normalize_cpu` / `-0` in
`01-runtime-ui.md` (PROC-044), not a `[processlist]` key.

## 16. Hotkeys used by this file

This section is a summary for cross-checking. `01-runtime-ui.md` is authoritative.

| Key | Action | ID |
|---|---|---|
| `a c m i t p u o P` | Sort keys `auto cpu mem io time name user core pid` | PROC-060 |
| `r` | Reverse sort direction | PROC-064 |
| Shift-Left / Shift-Right | Cycle sort key | PROC-065 |
| Left / Right (1 step), `<` / `>` (8 steps) | Scroll arguments | PROC-046 |
| Up / Down / PageUp / PageDown / Home / End | Move cursor | PROC-092 |
| Enter | Filter popup | PROC-073 |
| `E` | Clear filter | PROC-073 |
| `M` | Reset filter min/max | PROC-081 |
| `k` | Kill popup | PROC-100 |
| `+` / `-` | Renice | PROC-110 |
| `e` | Pin / unpin | PROC-120 |
| `j` | Processes / programs | PROC-130 |
| `/` | Short / long name | PROC-047 |
| `0` | Per-core normalisation (process CPU% and load-as-percent) | PROC-044 |
| `z` | Disable / enable process collection | PROC-029 |
| F5 / Ctrl-R | Force refresh, clear string caches | PROC-030 |

## 17. Glances quirks fixed

| Glances 4.5.9 behaviour | ogle behaviour | ID |
|---|---|---|
| `--hide-kernel-threads` drops every process whose real GID is 0 (`processes.py:495`), which also hides many root-group daemons | Uses the kernel `PF_KTHREAD` flag | PROC-020 |
| Cursor stops at row 48 (`max_processes = 50`) | Cursor covers the whole list, with page/home/end keys | PROC-092 |
| Cursor is a row index; the selected process changes under the cursor after each re-sort | Cursor follows the process key | PROC-090 |
| Raw key codes 65/66 (`A`/`B`) also move the cursor | Not reproduced (input decoding is in `01-runtime-ui.md`) | GEN-011 |
| Filter uses `re.fullmatch`, so `python` does not match `python3` | Unanchored search with smart case | PROC-072 |
| Unkeyed filter tests only `argv[0]` | Tests name and the full command line | PROC-071 |
| Invalid regex clears the filter silently | Error shown in the popup; the popup stays open | PROC-072 |
| Focus list replaces the filter | Focus and filter combine (AND) | PROC-074 |
| Kill always sends SIGKILL and blocks the UI for up to 3 s | SIGTERM by default, SIGKILL on request, no blocking | PROC-101–103 |
| Pinning a process disables the cursor | Cursor stays active | PROC-094 |
| No PID-reuse check before kill/renice | Start-time check | PROC-103 |
| Unpinning keeps collecting extended stats | Collection stops | PROC-121 |
| Swap of a pinned process sums `memory_maps()` (reads all of `smaps`) | Reads `VmSwap` | PROC-122 |
| `is_max` is computed but never shown | MAX decoration applied | PROC-041 |
| Auto sort goes back to plain auto when any event ends, even while others are ongoing | The newest ongoing event decides | PROC-062 |
| `--process-short-name` defaults to true and the option cannot turn it off | `--long-names` or `short_name = false` turns it off | PROC-047 |
| `processs name` typo in the sort description | `process name` | PROC-033 |
| Long-name mode checks that the path is a directory (one syscall per row) | No check | PROC-046 |
| I/O counters and full attributes are read for every process every cycle | Tiered, on-demand collection | PROC-022–026 |
| CPU% threshold is evaluated on the raw value even when Irix is off | Evaluated on the displayed value | PROC-041 |
| Process rates use floor division | Float division | GEN-007 |
| The first sample shows CPU% 0.0 | Shows `-` | PROC-013 |
