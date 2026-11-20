# ogle specification: performance and resource budgets

Status: draft, phase 1 (interactive TUI only)
Requirement ID prefix: `PERF-`

This file defines the resource budgets that ogle must meet, the implementation rules
that make them reachable, and the method used to check them. Resource efficiency is the
reason ogle exists (see `00-overview.md`, section 1). A feature that cannot meet these
budgets must be redesigned or dropped. The budgets are not to be relaxed to fit it.

Appendix A gives the measured baseline for Glances 4.5.9, htop 3.5.3 and procps `top`
on the reference machine. Every budget below is set against that baseline.

## 1. Definitions

- **Reference machine (R1)**: the machine on which Appendix A was measured. Linux 7.2.14
  (Arch), AMD Ryzen AI MAX+ 395 with 32 logical CPUs, 125 GiB RAM, about 710 processes.
  Budgets that state CPU time are calibrated to R1. On slower hardware, scale CPU time
  budgets by the ratio of single-thread performance. Do not scale memory, byte or
  syscall budgets.
- **Reference workload (W-N)**: ogle runs with the default configuration (no config
  file), the default 2 s refresh, all phase 1 modules that are on by default enabled, in
  a 200 columns x 50 rows pseudo-terminal with `TERM=xterm-256color`. The host has N
  processes in total. Standard sizes are W-700 (the natural process count of R1),
  W-1000, W-5000 and W-20000. Section 6 describes how the harness pads the process
  count.
- **Steady state**: from 10 s after start to the end of the measurement window. A
  measurement window is at least 60 s (30 cycles) unless stated otherwise.
- **CPU %**: user plus system CPU time of the ogle process (all threads), divided by
  wall-clock time, as a percentage of one core. Use `utime + stime` from
  `/proc/<pid>/stat`.
- **RSS**: `VmRSS` from `/proc/<pid>/status`. **Peak RSS**: `VmHWM`.
- **Read syscalls** and **bytes read**: `syscr` and `rchar` from `/proc/<pid>/io`.
  **Write syscalls** and **bytes written**: `syscw` and `wchar`.
- **Frame**: one redraw of the screen. **Bytes per frame**: bytes ogle writes to the
  terminal for that frame.

## 2. Budgets

The MUST value is a release gate. The SHOULD value is the design target. All values
apply on Linux at W-700 on R1 unless a row states something else.

### 2.1 Memory

- **PERF-001 Steady-state RSS.** RSS at W-700 MUST be at most 8 MiB and SHOULD be at
  most 5 MiB. (Glances: 74 MiB. htop: 18 MiB. top: 8.3 MiB.)
- **PERF-002 Peak RSS.** Peak RSS during the whole run, start-up included, MUST be at
  most 1.25 times the steady-state RSS of the same run.
- **PERF-003 Scaling with process count.** RSS MUST grow by no more than 1 KiB per extra
  process. Concretely, RSS at W-5000 minus RSS at W-1000 MUST be at most 4 MiB. Memory
  for per-process state (name, cmdline, previous counters) is the main cost here. Keep
  it compact: intern user names and keep one cmdline buffer per process.
- **PERF-004 No growth over time.** RSS after 1 hour at W-700 MUST be no more than
  512 KiB above the RSS at 60 s. Memory for processes that have exited MUST be
  reclaimed or reused within 2 cycles. Event list, history and caches are bounded by
  construction (see PERF-033).
- **PERF-005 Process churn.** The harness churn test (section 6.3) starts and stops
  100 short-lived processes per second for 10 minutes. RSS at the end MUST be no more
  than 1 MiB above the RSS at the start of the churn phase.

### 2.2 CPU

- **PERF-006 Steady-state CPU.** Average CPU at W-700 with 2 s refresh MUST be at most
  1.0% and SHOULD be at most 0.5% of one core. That is 20 ms (MUST) and 10 ms (SHOULD)
  of CPU time per cycle. (Glances: 4.67%. htop: 5.08%. top: 0.60%.)
- **PERF-007 Scaling with process count.** CPU time per cycle MUST grow by no more than
  15 microseconds per extra process on R1. At W-5000 the per-cycle CPU time MUST be at
  most 90 ms, and at W-20000 at most 320 ms.
- **PERF-008 Refresh-rate scaling.** CPU % MUST scale roughly linearly with the refresh
  rate. At `-t 1` it MUST be at most 2.2 times the `-t 2` value. At `-t 10` it MUST be
  at most 0.3 times the `-t 2` value. Fixed per-wakeup overhead must not dominate.
- **PERF-009 Key handling.** A key press that changes only the view (sort order, a
  toggle, cursor movement) MUST redraw from the data already sampled, without new
  collection. It MUST cost at most 2 ms of CPU at W-700. Glances behaves the same way
  (see `01-runtime-ui.md`).
- **PERF-010 Hidden data costs nothing.** A module that is disabled or not visible, a
  column that is hidden, and a toggle-only view that is off MUST cost no collection work
  (GEN-004). Examples: no `/proc/<pid>/io` reads while the R/s and W/s columns are
  hidden (narrow terminal, PROC-051), except while sorting by `io` (PROC-026). On the
  BSDs these columns are never shown and nothing is read (PROC-052, PLT-153). No
  per-core stats beyond what the total needs when neither percpu nor quicklook per-core
  bars are shown. Extended process stats only for the pinned process.

### 2.3 Start-up and latency

- **PERF-011 Time to first frame.** Time from `exec` to the first complete frame written
  to the terminal MUST be at most 100 ms and SHOULD be at most 50 ms. The frame shows
  every value that needs only one sample (memory, load, uptime, file systems, process
  names, counts). Rate-derived values show `-`, as GEN-007 requires. (Glances: about
  550 ms. htop and top: about 215 ms, because both wait for a CPU sample before the
  first draw.)
- **PERF-012 Time to populated frame.** ogle SHOULD take a priming second sample about
  500 ms after the first. It then shows a frame with all rates populated no later than
  1.0 s after `exec`. After that it follows the normal refresh cadence. `01-runtime-ui.md`
  owns the exact cadence. This requirement only bounds the time.
- **PERF-013 Input latency.** A key press MUST be reflected on screen within 50 ms,
  measured from the byte arriving on stdin to the frame write completing, at W-700.
- **PERF-014 Exit.** `q` MUST restore the terminal and exit within 50 ms.

### 2.4 Terminal output

- **PERF-015 Differential output.** In steady state, ogle MUST write only the cells that
  changed (GEN-013). Average bytes written per frame at W-700 MUST be at most 2 KiB and
  SHOULD be at most 1 KiB. A full repaint is allowed only on start-up, on terminal
  resize, on resume from suspend and on F5/Ctrl-R (UI-045, UI-080). Closing a popup
  uses the normal diff. (Glances via curses: about 760 B per frame. htop: about 4.9 KiB. top: about
  10.5 KiB.)
- **PERF-016 One write per frame.** Each frame MUST be assembled in a reused buffer and
  written with a single `write()` call (or as few calls as needed for partial writes).
  Average write syscalls per cycle MUST be at most 4 in steady state with no input.
  (Glances: about 49. htop: about 139. top: about 6.)
- **PERF-017 Minimal escape sequences.** Cursor moves SHOULD use the shortest sequence
  available (relative moves, or plain spaces for short gaps). Attribute changes are
  emitted only when the attribute differs from the current one.

### 2.5 System calls and file access

- **PERF-018 Read syscall budget.** Read-type syscalls per cycle (`syscr` delta / cycles)
  at steady state MUST be at most `2 * N + 100` and SHOULD be at most `1.5 * N + 50`,
  where N is the number of processes. The I/O columns are visible at 200 columns, so the
  budget allows `stat` plus `io` per process. Each per-process file MUST be read with one
  read call into a buffer large enough for its content. Do not issue a second read just
  to detect end of file. (At W-700: Glances 6768, htop 18398, top 1441.)
- **PERF-019 Bytes read budget.** Bytes read per cycle (`rchar` delta / cycles) MUST be
  at most `1 KiB * N + 128 KiB`. (At W-700: Glances 1.35 MB, htop 11.1 MB, top 184 KB.)
- **PERF-020 Persistent handles for system-wide files (Linux).** Files that are re-read
  every cycle and whose content procfs regenerates on a read from offset 0 MUST be opened
  once and re-read with `pread(fd, buf, len, 0)` (or `lseek` plus `read`). This covers
  `/proc/stat`, `/proc/meminfo`, `/proc/loadavg`, `/proc/vmstat`, `/proc/net/dev`,
  `/proc/diskstats`, `/proc/uptime` and the hwmon `sysfs` attribute files. sysfs
  attributes need `pread` at offset 0 as well. The one exception: if a re-read returns an
  error such as `ENODEV` (a hot-unplugged device), close the handle and re-open it on the
  next due cycle.
- **PERF-021 Per-process reads (Linux).** For each process, each cycle reads exactly one
  file, `/proc/<pid>/stat`. It has state, ppid, utime, stime, priority, nice,
  num_threads, starttime, vsize, rss and processor. `/proc/<pid>/io` is read only as
  PROC-026 allows: for visible rows while the R/s and W/s columns are shown, and for all
  processes while sorting by `io`. If opening `io` fails with `EACCES` or `EPERM`,
  remember that for the process identity and do not retry. The column shows `?`
  (GEN-014). Open these with `openat` relative to a directory handle on `/proc` that is
  kept open, so the kernel does no repeated path walk from `/`.
  `/proc/<pid>/status` and `/proc/<pid>/statm` MUST NOT be read every cycle. The owner
  UID comes from `fstatat` on `/proc/<pid>` once, when the process is first seen.
- **PERF-022 Identity and once-per-process data.** A process is identified by
  `(pid, starttime)`. cmdline, executable name and UID MUST be read once per process
  identity. They are not re-read on a timer. (Glances re-reads cmdline and username every
  60 s.) Exceptions: when the process name (the `stat` comm field, read every cycle
  anyway) changes, as after an `exec` without a new PID, cmdline is read again
  (PROC-023). The identity does not change. F5 and Ctrl-R clear the cmdline and user
  name caches (PROC-030, UI-045).
- **PERF-023 User name cache.** Map UIDs to names through a cache that lives as long as
  the process. Resolve each UID once with the platform API (`getpwuid_r` on POSIX,
  `LookupAccountSid` on Windows). Never resolve per process per cycle.
- **PERF-024 Optional persistent per-process handles.** Implementations MAY keep the
  `/proc/<pid>/stat` handle open across cycles and use `pread`. This is safe because an
  open procfs handle stays bound to the original task. Reads fail with `ESRCH` after the
  task exits, so PID reuse cannot mix data. If used, the number of cached handles MUST be
  capped at the lower of 4096 and half of the soft `RLIMIT_NOFILE`. Processes over the
  cap fall back to open, read and close. Handles of exited processes MUST be closed in the
  same cycle that detects the exit.
- **PERF-025 Directory scans.** Enumerate `/proc` with `getdents64` into one reused
  buffer of at least 32 KiB, through the same directory handle with a rewind each cycle.
  Do not list `/proc/<pid>/task` (ogle shows processes, not threads). The thread count
  comes from `stat`.
- **PERF-026 Expensive per-process data only for the pinned process.** Extended fields
  (swap, file descriptor counts, socket counts, CPU affinity and I/O priority) are
  collected only for the single pinned process, once per process refresh (PROC-122,
  PROC-123). On Linux, swap comes from `VmSwap` in `/proc/<pid>/status`. ogle MUST NOT
  read `smaps` or `smaps_rollup` for it. Socket counts are the most expensive part and
  SHOULD be read at most every 10 s, even when the refresh is faster (PROC-123).
- **PERF-027 Other platforms.** Follow the same discipline with native APIs: one bulk
  call where the OS provides one, and per-process calls only for data that bulk calls
  do not give. Examples: `sysctl(KERN_PROC_ALL)` on BSD and macOS,
  `NtQuerySystemInformation(SystemProcessInformation)` on Windows, and
  `proc_pidinfo(PROC_PIDTASKINFO)` on macOS only for fields that `KERN_PROC` lacks.
  `06-platform-sources.md` names the calls.

### 2.6 Wakeups, threads and blocking

- **PERF-028 Event-driven waiting.** Between cycles ogle MUST block in one wait call
  (`poll`/`ppoll`/`kevent`/`WaitForMultipleObjects`) on stdin, a signal source (resize,
  terminate) and the earlier of the next cycle deadline and the expiry of the active
  status message (UI-025, UI-114). A status-message expiry only redraws; it does not
  start a new cycle. It MUST NOT poll for input on a short timer.
  Glances wakes every 100 ms through `napms(100)` (`glances/outputs/glances_curses.py`).
  With no input, voluntary context switches MUST average at most 3 per cycle.
- **PERF-029 Monotonic deadlines.** Schedule cycles against a monotonic clock as absolute
  deadlines, so collection time does not cause drift. A key press MUST NOT push the next
  sample later (this fixes GEN-011's elapsed-time timer quirk).
- **PERF-030 Threads.** ogle MUST use at most 2 threads: the main thread and at most one
  helper thread. The helper runs calls that can block without bound, such as `statvfs`
  on an unresponsive network file system (IO-058). A file system whose usage query has
  not returned within 2 s, or has failed, keeps its row with `?` in the value cells and
  is retried on the next fs cycle. The helper keeps no work queue beyond one in-flight
  request per mount.
- **PERF-031 Suspend and hidden terminal.** ogle cannot reliably detect that its terminal
  is not visible, so this spec makes no claim about that case. When stopped by
  `SIGTSTP` (Ctrl-Z), ogle restores the terminal and does no work until `SIGCONT`. Then
  it does a full repaint.

### 2.7 Allocation discipline

- **PERF-032 Steady-state allocation.** After warm-up, heap allocations per cycle MUST be
  proportional to the number of new processes and new devices seen in that cycle, not to
  the total process count. Sort buffers, line buffers, the frame buffer, the screen cell
  grid and parse buffers are allocated once and reused. They grow only on resize or when
  the process count grows.
- **PERF-033 Bounded structures.** Every structure with a lifetime longer than one cycle
  has a fixed upper bound: history rings (GEN-008: 30 samples plus sparkline width), the
  event list (`max_events`, default 10), the per-process table (live processes plus at
  most one cycle of exited ones) and the UID name cache (live UIDs).
- **PERF-034 Partial sort.** The process list needs order only for the visible rows plus
  the cursor range. Implementations SHOULD use a partial selection such as
  `nth_element` / `select_nth_unstable` followed by a sort of the visible part. A full
  sort of all N processes is allowed when N is at most 2000.

### 2.8 Binary and distribution

- **PERF-035 Binary size.** A stripped release binary for Linux x86_64 MUST be at most
  3 MiB and SHOULD be at most 1.5 MiB. A fully static build counts its whole size. The
  dynamically linked htop 3.5.3 is 395 KiB, and procps `top` is 140 KiB.
- **PERF-036 Dependencies.** No interpreter or runtime environment at run time (GEN-002).
  Glances needs a Python 3 interpreter (about 30 MB+ of installed files) plus psutil.

### 2.9 Other platforms

- **PERF-037 Cross-platform budgets.** On macOS, FreeBSD and Windows, the memory,
  terminal-output and thread budgets (PERF-001 to PERF-005, PERF-015 to PERF-017,
  PERF-030) apply unchanged. CPU budgets SHOULD be within 2 times the Linux values on
  comparable hardware, because per-process queries cost more on some kernels (macOS
  `proc_pidinfo`, for example). Measure with the platform equivalents in section 6.4.

## 3. Rules that change Glances behaviour for efficiency

These rules restate decisions from other files that exist mainly for efficiency. They are
listed here so the budget owner can see them in one place.

- **PERF-038** History is limited to what is displayed (GEN-008). Glances keeps up to
  1200 samples per field by default (`history_size`), plus min/max/mean accumulators over
  28800 samples that the TUI never shows. ogle MUST NOT keep those accumulators.
- **PERF-039** cmdline and user names are read once per process identity (PERF-022). They
  are not re-read on a 60 s timer.
- **PERF-040** Module refresh intervals follow GEN-006 (system 60 s, fs 60 s,
  sensors 10 s). Sensors are read sequentially on the main thread. Glances uses a thread
  pool, which is not needed for sysfs reads that take microseconds.
- **PERF-041** The per-cycle CPU sample is shared by the cpu, percpu and quicklook
  panels (SYS-011) and by the process CPU % computation. quicklook reuses the mem, swap
  and load samples of those modules (SYS-012). Each source file is read at most once per
  cycle, no matter how many modules use it.

## 4. Self-measurement support

- **PERF-042** ogle SHOULD offer a hidden diagnostic option, the environment variable
  `OGLE_TRACE_TIMING=<path>`. It appends one line per cycle with the cycle number,
  collection time, render time, output bytes and process count, written to the path
  given. It MUST cost nothing when unset (no clock reads beyond those the loop already
  makes). This trace is for development. Budget checks use the external harness, not
  this trace.

## 5. Acceptance

- **PERF-043** A release meets the budgets when the harness in section 6 passes for
  W-700 (or the natural count on the test machine), W-1000 and W-5000. Before release,
  run W-20000 and the churn test once by hand.
- **PERF-044** Each budget check is the median of 3 runs. A budget fails if the median
  exceeds the MUST value. A SHOULD miss is reported as a warning.
- **PERF-045** The harness always measures Glances and htop in the same session, on the
  same machine, with the same workload, when they are installed. It reports ogle's
  ratio to each. This keeps results comparable across machines that differ from R1.

## 6. Benchmark harness

The harness lives in the ogle repository (proposed path `bench/`). It is a shell script
plus small helpers. It needs only standard tools: `bash`, `script` (util-linux), `stty`,
`awk`, `pgrep`. It MUST NOT need root.

### 6.1 Running the tool under test

1. Start the tool in a pseudo-terminal of exactly 200 x 50:
   `script -qfc "stty rows 50 cols 200; export TERM=xterm-256color; exec <cmd>" /dev/null > tty.bin < <(sleep <duration+10>)`.
   Keep stdin open with a sleeping writer so the tool does not see EOF.
2. Find the tool's PID as the descendant of `script` whose `comm` is not
   `script`, `bash`, `sleep` or `timeout`.
3. Record the start time with a sub-millisecond clock (`$EPOCHREALTIME` in bash 5).
   Do not use `bc`; it is not always installed.
4. Use the tool's own default settings plus a 2 s refresh: `ogle -t 2`, `glances -t 2`,
   `htop -d 20`, `top -d 2`.

### 6.2 Metrics collection

- **Time to first frame**: poll the size of `tty.bin` every 10 ms. Record when it first
  exceeds 4000 bytes. Subtract the harness overhead, measured the same way with a command
  that prints 5000 bytes at once (about 13 ms on R1).
- **RSS, peak RSS, threads, voluntary context switches**: sample `/proc/<pid>/status`
  every 250 ms.
- **CPU**: `utime + stime` from `/proc/<pid>/stat` at the start and end of the steady
  window, divided by `CLK_TCK`.
- **Syscalls and bytes**: `/proc/<pid>/io` at the start and end of the steady window.
  The tool runs as the same user, so the harness can read this file.
- **Terminal bytes**: size of `tty.bin` at the start and end of the steady window,
  divided by the number of cycles.
- Write every raw sample to a file and keep it next to the summary, so a result can be
  re-examined without a rerun.

### 6.3 Workload generation

- **Process padding (W-N)**: the harness spawns idle processes until the total process
  count (entries in `/proc`) reaches N. Each padding process is a minimal sleeper with a
  distinct short argv, for example `sleep 3600` started as `exec -a ogle-pad-<i>`. They
  run as the invoking user and are killed when the harness exits, including on error
  (use a `trap`). This changes system state only temporarily, and only for the invoking
  user.
- **Load**: a second variant runs 4 busy-loop processes, each pinned to one core where
  possible, plus one process that writes and deletes a 256 MiB temporary file in a loop.
  Rate columns, I/O columns and the auto-sort logic then see real changes. Report it
  separately as `W-N-load`.
- **Churn (PERF-005)**: a loop that starts 100 `true` processes per second for 10
  minutes.
- Use the default configuration (`--config /dev/null` or the tool's equivalent) so user
  configs do not affect results.

### 6.4 Other platforms

- macOS and BSD: RSS and CPU from `ps -o rss=,time= -p <pid>` sampled every second.
  Syscall counts from `dtrace` or `truss -c` when available, marked informative.
- Windows: `Get-Process -Id <pid>` WorkingSet64, PeakWorkingSet64 and TotalProcessorTime
  in PowerShell, with the tool in Windows Terminal at 200 x 50.
- Time to first frame on these platforms is informative only.

## Appendix A (informative): Glances baseline on R1

Measured 2026-11-19 on the reference machine R1: Linux 7.2.14-arch1-1, AMD Ryzen AI
MAX+ 395 (32 logical CPUs), 125 GiB RAM, 711 processes, `CLK_TCK` = 100. The tools ran
one at a time in a 200 x 50 pseudo-terminal, `TERM=xterm-256color`, 2 s refresh. The
harness was the method in section 6, without process padding. Raw outputs are under
`/tmp/ogle-bench/` on the measuring machine; they are not kept.

Versions: Glances 4.5.9 (Python 3.14.10, psutil 7.2.2), htop 3.5.3, procps `top`.
btop was not installed.

### A.1 Memory, CPU and output (70 s run, steady window 10 s to 70 s)

| Metric | Glances 4.5.9 | htop 3.5.3 | top (procps) |
|---|---|---|---|
| RSS at about 2.5 s | 72.0 MiB | 18.0 MiB | 8.3 MiB |
| RSS at 60 s (steady) | 73.6 MiB | 18.1 MiB | 8.3 MiB |
| Peak RSS (VmHWM) | 73.6 MiB | 18.1 MiB | 8.3 MiB |
| Average CPU, % of one core | 4.67% | 5.08% | 0.60% |
| CPU time per 2 s cycle | about 93 ms | about 102 ms | about 12 ms |
| Threads | 1 (2 briefly at start) | 1 | 1 |
| Open file descriptors | 5 | 4 | 7 |
| Terminal bytes per second | 353 | 2445 | 5141 |
| Terminal bytes per frame | about 700 | about 4900 | about 10300 |

### A.2 Syscalls and reads (separate 45 s run, window 10 s to 40 s, 15 cycles)

| Metric (per cycle) | Glances 4.5.9 | htop 3.5.3 | top (procps) |
|---|---|---|---|
| Read syscalls (`syscr`) | 6768 | 18398 | 1441 |
| Read syscalls per process | about 9.5 | about 25.9 | about 2.0 |
| Bytes read (`rchar`) | 1.35 MB | 11.1 MB | 184 KB |
| Write syscalls (`syscw`) | 49 | 139 | 5.7 |
| CPU in this window | 4.6% | 4.9% | 0.6% |

htop reads more because it walks `/proc/<pid>/task` for user threads and reads several
files per task. Glances does about 9.5 reads per process because psutil opens and reads
`stat`, `status`, `statm`, `io` and others through separate calls per attribute, and
re-reads cmdline and username every 60 s.

### A.3 Time to first frame (3 runs each, 10 ms polling, harness overhead 13 to 14 ms)

| Tool | Raw | Net of overhead |
|---|---|---|
| Glances 4.5.9 | 543 / 578 / 593 ms | about 530 to 580 ms |
| htop 3.5.3 | 223 / 223 / 223 ms | about 210 ms |
| top (procps) | 230 / 232 / 244 ms | about 220 ms |

htop and top both wait about 200 ms to take a first CPU sample before drawing. Glances
spends most of its start-up importing Python modules and psutil, then does a full first
collection.

### A.4 Binary and install size

| Item | Size |
|---|---|
| htop binary (dynamic, stripped) | 395 KiB |
| top binary (dynamic) | 140 KiB |
| psutil native extensions | 35 KiB, plus Python sources and the Python 3 runtime |

### A.5 Observations behind the budgets

- Glances memory is dominated by the Python interpreter and imported modules. About
  72 MiB is resident before the first full frame, and it grows slowly (about 1.6 MiB in
  the first minute, the history buffers filling).
- Glances CPU cost comes from per-attribute psutil calls across about 710 processes, and
  from Python object churn. ogle's PERF-006 target of 0.5% to 1.0% is about 5 to 10 times
  lower than Glances. It is in line with top, which collects similar per-process data.
- Glances has the leanest terminal output of the three, thanks to curses' differential
  update. ogle must match it (PERF-015) and cannot assume a full-repaint approach is
  cheap.
- Glances wakes every 100 ms to poll the keyboard (`napms(100)` in
  `glances/outputs/glances_curses.py`), which is 20 wakeups per 2 s cycle even when idle.
  PERF-028 removes these wakeups.
