# Known gaps (phase 1)

This file records known gaps, shortcuts and unverified assumptions in the phase 1
implementation. Remove an entry once it is fixed or verified. Spec references point into
`spec/`.

## 1. Build and verification status

- **Linux builds and runs.** `cargo build --release` succeeds with 7 dead-code warnings
  (unused interface fields such as `CpuTopology::physical`, `LoadAvg::emulated`,
  `Memory::shared`). `cargo test` passes 56 tests on Linux. Tests inside the macOS, BSD and
  Windows backends have not run.
- **Non-Linux targets have never been compiled.** Check them with
  `cargo check --target <triple>` on a machine that has the target's std installed.
- **Performance measured on Linux only.** First measurement (32 CPUs, ~750 processes,
  200x50 pty, 2 s refresh): 0.47% of one core, 4.25 MiB RSS, 816 read syscalls and 1 write
  per cycle, 913 terminal bytes per frame, first output after 2 ms. The larger PERF harness
  (1000/5000/20000-process workloads, churn test, 1-hour run) has not been run.
- **Benchmark harness is not in the repo.** The Glances/htop/top baseline scripts and raw
  output live in `/tmp/ogle-bench/`, which is lost on reboot. Copy them into the repo
  (for example `bench/`) before relying on them.

## 2. Platform interface (`src/platform/mod.rs`)

- **`Proc` cannot mark core fields unknown.** `cpu_user`, `cpu_system`, `rss` and
  `num_threads` are plain values, not `Option`.
  - macOS: for other users' processes (EPERM) and zombies the backend reports 0 for these
    and `vms = None`. The UI shows 0, not `?`.
  - OpenBSD: `num_threads` is not available and is reported as 0.
  - Fix: make these fields `Option` and show `?` in the process list.
- **`Battery` has no time-remaining field.** The spec lists seconds left; no backend
  reports it.
- **`Swap` has no swap-in/swap-out counters.** Not collected on any platform.
- **`CpuTimes` has no `interrupt` field.** Windows reports interrupt time in `irq`.
- **`ActionError` has no "refused" variant.** Signalling or renicing ogle's own PID returns
  `PermissionDenied` from the backends. The processes module refuses its own PID earlier
  and shows the correct message, so this only matters for direct callers.
- **Process status `L`.** FreeBSD reports `L` (SLOCK), which is not in the PLT-150 list.
  The UI renders unknown letters as-is and does not count them as running or sleeping.

## 3. Linux backend

- **Process owner of non-dumpable processes.** The owner comes from the owner of
  `/proc/<pid>` (PLT-143). The kernel shows non-dumpable processes (setuid programs, some
  agents) as root, so they appear as owned by root.
- **Socket counts across namespaces.** Pinned-process TCP/UDP counts read
  `/proc/net/tcp*`, which is ogle's own network namespace. A pinned process in another
  container gets wrong counts. Fix: read `/proc/<pid>/net/tcp*`.
- **ARM CPU model.** The implementer/part lookup table is not implemented, so the CPU
  model is absent on ARM systems without a `model name` line in `/proc/cpuinfo`.
- **CPU model is read once.** If the first read finds no model, quicklook shows none until
  F5.
- **Offline CPUs** produce zeroed entries in the per-CPU list, because it is indexed by
  CPU number.
- **Battery files** (`capacity`, `status`) are opened by path every sensors cycle instead
  of being kept open. No allocation; minor syscall cost.
- **PERF-024** (optional per-process `stat` handle cache) is not used.
- **`start_time`** is the raw tick count from `/proc/<pid>/stat` field 22, used as an
  opaque identity token. The spec describes converting it to wall-clock seconds.

## 4. macOS backend

- **No temperature or fan sensors.** `sensors()` returns an empty list. SMC access is
  Intel-only and Apple Silicon needs private APIs. Batteries work.
- **Denied process fields show 0.** See section 2.
- **Context switches** are reported as a single total in `ctx_voluntary`.
- **CoreFoundation allocates per cycle** for disk statistics (one dictionary per disk) and
  battery info. The IOKit API offers no way around this.
- **Hand-written struct layouts.** `KinfoProc` (648 bytes) and other `repr(C)` structs are
  declared locally with compile-time size asserts. They still need a real macOS build to
  confirm.

## 5. BSD backends

- **Unverified layouts.** The private `repr(C)` structs `Devstat`, `DiskStats`,
  `SensorRec`, `ApmPowerInfo` and `IoSysctl` were not checked against real headers.
- **OpenBSD `uvmexp`** is read by fixed field positions from the 7.x header. Older
  releases with a different layout would give wrong memory, swap and counter values.
- **NetBSD `uvmexp_sysctl`** indices 72/73 (`filepages`/`execpages`) are unverified.
- **NetBSD CPU frequency** uses `machdep.cpu.frequency.*`, which may not exist; drivers
  may use `machdep.est.*` or `machdep.powernow.*`. Frequency is absent if the node is
  missing.
- **NetBSD sensors and battery** are not implemented (empty lists, allowed by the spec).
- **FreeBSD fallbacks not implemented:** `kern.sched.topology_spec` for the physical core
  count and `vm.swap_info` for swap.
- **OpenBSD/NetBSD read `uvmexp` three times per cycle** (memory, swap, counters). Small
  cost; the contract has no per-cycle cache point.
- **`process_cmdline`** does not re-check the start time before reading.
- **Swap on OpenBSD/NetBSD** comes from `uvmexp` page counts, not `swapctl`.

## 6. Windows backend

- **Remote drives report `fs_type = "remote"`.** This keeps `GetVolumeInformationW` off the
  UI thread (PLT-010). Users must add `"remote"` to `[fs] allow` to see network drives.
  The default config comment and PLT-122 do not mention this yet.
- **Drives without volume info** (empty card readers, optical drives) are left out.
- **Drive roots have no device basename,** so the fs panel adds no ` (device)` suffix.
- **Affinity** is absent for processes that span more than one processor group.
- **Involuntary context switches** are absent; the total goes in `ctx_voluntary`.
- **`start_time`** has whole-second resolution. A PID reused within the same second would
  pass the identity check.
- **A process with zero threads** gets status `S`; the spec does not define this case.
- **Boot time** is queried on every call instead of using clock-jump detection.
- **Clock `%Z`** prints the numeric UTC offset, not a zone name.
- **Alert timestamps** use the current UTC offset, so an event from before a DST change
  shows today's offset.
- **Redirected stdio.** The terminal layer uses `GetStdHandle`, not `CONIN$`/`CONOUT$`, so
  ogle fails if stdin or stdout is redirected.

## 7. Modules and UI

### Processes

- **Rows scrolled into view by cursor keys** show the bare name and numeric uid until the
  next update fetches their command line and user name.
- **After `j` (programs) or `/` (long names)** the affected text is stale until the next
  update.
- **Programs mode** groups by the raw short name, not the PLT-144 long name, and checks
  user agreement by uid.
- **MAX highlight** is computed over the whole filtered list, not only the visible rows.
- **Initial filter is parsed twice:** `Processes::new` drops an invalid filter silently
  and the app re-parses it to exit with status 2. Invalid `--focus` entries are dropped
  without an error.
- **No hotkey for hiding kernel threads.** The spec defines none; only
  `[processlist] hide_kernel_threads` sets it.
- **`Processes` is `!Sync`** because of an interior `Cell<bool>` used to pass the
  process-list width from render to update.

### I/O modules

- **Module re-enable detection (IO-024)** is a heuristic: a gap longer than 3x the refresh
  interval plus 1 s discards old counters. The update API has no explicit signal.
- **fs blocking.** The first time a mount hangs, the UI thread can wait up to 2 s. The
  single worker thread handles mounts one at a time, so a hung mount delays the mounts
  queued behind it.
- **Alias threshold lookup (ALR-013)** uses the alias when `items."<alias>"` exists,
  otherwise the real name. It does not fall back level by level from alias to real name.
- **Hardware temperature limits** (high/critical) are read only when a sensor first
  appears.
- **fs and sensors keep a copy of their threshold config** because `update` receives no
  `Config`.

### System modules

- **Per-CPU times** are collected only in per-CPU mode (`1`), following SYS-061.
  After pressing `1`, the per-CPU cells show `?%` until the next cycle, because a
  percentage needs two samples.
- **`{linux_distro}` in `[system] info_format`** silently falls back to the default
  template when `/etc/os-release` is missing.
- **System info** builds new Strings each time it refreshes (every 60 s by default).

### Terminal and app

- **No Alt key support.** Alt+key sequences are consumed and dropped.
- **External SIGSTOP/SIGTSTP** cannot restore the terminal first; SIGCONT re-enters raw
  mode afterwards.
- **`Alerts::end_cycle`'s `wall` argument** is unused (event times come from
  `SystemTime`).

## 8. Spec updates pending

- Add the config keys the implementation invented: `processlist.filter` (initial filter,
  set by `--filter`) and `processlist.collect` (set to false by `--no-processes`).
- PLT-122: Windows remote drives report `fs_type = "remote"` without calling
  `GetVolumeInformationW`.
- PLT-150: add FreeBSD `L`.
- PLT-143: note the non-dumpable-process ownership caveat.
- Linux `start_time` is an opaque tick token, not wall-clock seconds.
- `[fs] allow` documentation: mention `"remote"` for Windows network drives.
