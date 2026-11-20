# ogle specification: platform data sources

Status: draft, phase 1
Binding decisions: `00-overview.md` (GEN-002, GEN-005, GEN-007, GEN-014, GEN-015).

This file defines the platform interface: one set of operations for each data domain. It
also states the exact OS source for every field on each supported platform. Module files
(`02`–`05`) say how ogle uses and displays each field. This file says only where the
value comes from, what unit it has, what it costs to read, and when it is absent.

Platforms:

- **Linux**: tier 1.
- **macOS**: tier 1.
- **FreeBSD**: tier 1.
- **Windows** (10 1809 and later): tier 1.
- **OpenBSD, NetBSD**: best effort (OQ-2). Notes are given where they differ from FreeBSD.

The psutil source is useful prior art. Most Linux paths below match
`psutil/_pslinux.py`. Where ogle departs from psutil, the text says so and gives the reason.

---

## 1. General rules

- **PLT-001 Absent is not zero.** Each field is optional. A field the platform cannot
  provide, or that a read failed to produce, is *absent*. Modules render absent fields
  per GEN-005: the row is hidden, or the value shows `-` or `?` as the module file says.
  The interface MUST NOT return 0 to mean "unknown".
- **PLT-002 Counters.** Cumulative counters are unsigned 64-bit integers in their native
  unit. The interface returns raw counters, not rates. Rates are computed above the
  platform layer (GEN-007). A platform with 32-bit counters (for example the legacy
  `if_data` on macOS) MUST widen them to 64 bits by detecting wrap: if
  `new < old`, add 2^32. A wrap is only assumed when the old value was above 2^31.
  Otherwise the counter is treated as reset.
- **PLT-003 Time units.** The interface converts every CPU time to seconds (f64), or to
  integer nanoseconds. Every duration in disk statistics uses milliseconds. Every
  timestamp is wall-clock Unix seconds. The native unit is listed for each source below
  so implementers convert correctly.
- **PLT-004 Sample timestamp.** Every collection call records a monotonic timestamp
  (`CLOCK_MONOTONIC`, `mach_continuous_time`, `QueryPerformanceCounter`) taken just
  before the read. Rate computation uses these timestamps.
- **PLT-005 Permission failures.** `EACCES`, `EPERM`, `ERROR_ACCESS_DENIED` and similar
  errors on a single item (one process, one sensor, one mount) make only that item's
  affected fields absent. They MUST NOT abort the collection of other items. ogle MUST NOT
  log them to the screen (GEN-014).
- **PLT-006 Vanished items.** A process, interface or mount that disappears between
  enumeration and read (`ENOENT`, `ESRCH`, `ERROR_INVALID_PARAMETER`) is dropped silently
  for that cycle.
- **PLT-007 Cost classes.** Every source below is labelled with one cost class:
  - **C0** static: read once at start-up. Re-read only on an explicit trigger
    (F5 / Ctrl-R, or a change in the item set).
  - **C1** cheap system-wide: one file or one syscall per cycle.
  - **C2** per-item, per-cycle: cost grows with the number of processes, disks or sensors.
  - **C3** expensive: reads that walk large tables, take many syscalls per item, or may
    block. Allowed only on demand (the pinned process) or at a slow cadence.
- **PLT-008 Buffer reuse.** Readers SHOULD read into reusable buffers and parse in place.
  Linux readers SHOULD use `openat()` relative to an open `/proc` directory fd
  instead of building absolute paths. See `07-performance.md`.
- **PLT-009 No subprocesses, no sockets.** No source may spawn a helper program, such as
  `sysctl`, `wmic`, `ioreg` or `powershell`, or open a network socket (GEN-015). OS library calls are
  allowed even when the OS may use the network internally. Examples: NSS user lookups on
  Linux, and `LookupAccountSidW` resolving a domain SID on Windows. Such calls MUST be
  cached (PLT-125, PLT-127).
- **PLT-010 Blocking calls.** Calls that can block for a long time run on a worker
  thread with a timeout, so the UI loop is never stalled. Examples: `statvfs` on a
  dead NFS mount, and `GetDiskFreeSpaceExW` on a disconnected network drive. A timed-out
  item returns no values for that cycle; the fs module keeps its row and shows `?` in
  the value cells (IO-058 in `03-io-modules.md`). Only one call per item may be in
  flight: a timed-out call that is still pending is not retried, and the next due cycle
  after it returns starts a new one. The
  default timeout is 2 s (the same as Glances `fs` timeout).

---

## 2. Interface overview

The interface is written as language-neutral records. `?` marks an optional field
(PLT-001). Each domain is one operation, so a disabled module never triggers reads for
another domain (GEN-004).

| Operation | Returns | Used by |
|---|---|---|
| `system_info()` | `SystemInfo` | system |
| `boot_time()` | Unix seconds | uptime |
| `cpu_topology()` | `CpuTopology` | cpu, percpu, load, quicklook, processlist |
| `cpu_freq()` | `CpuFreq` | quicklook |
| `cpu_times(per_cpu: bool)` | `CpuTimes` total, plus a list when `per_cpu` | cpu, percpu, quicklook |
| `cpu_counters()` | `CpuCounters` | cpu |
| `load_avg()` | `LoadAvg` | load, quicklook |
| `memory()` | `Memory` | mem, quicklook, processlist (MEM%) |
| `swap()` | `Swap` | memswap, quicklook |
| `net_interfaces(want_addrs: bool)` | list of `NetIf` | network |
| `disk_io()` | list of `DiskIo` | diskio |
| `filesystems()` | list of `Mount` | fs |
| `fs_usage(mount)` | `FsUsage` (may block, PLT-010) | fs |
| `sensors(kinds)` | list of `Sensor` | sensors |
| `battery()` | list of `Battery` | sensors |
| `processes(fields)` | list of `Proc` | processcount, processlist, programlist |
| `process_extended(pid, start_time)` | `ProcExtended` | processlist (pinned) |
| `process_signal(pid, start_time, sig)` | result | processlist (kill) |
| `process_set_priority(pid, start_time, delta)` | result | processlist (nice) |
| `pid_max()` | integer? | processlist (PID column width) |

- **PLT-011** `processes(fields)` takes the set of per-process fields the UI needs this
  cycle. The platform MUST skip reads for fields that are not requested. Examples:
  I/O counters when the I/O columns are hidden and the sort key is not `io`;
  command line on cycles where the cache is valid (PLT-125).

---

## 3. System information (`system_info`)

`SystemInfo { os_name, hostname, arch, os_version, distro? }`. Cost C0. Re-read every
60 s, because the hostname can change (GEN-006 system refresh).

- **PLT-020 Linux**
  - `os_name` = `"Linux"`.
  - `hostname` and `os_version` come from `uname()`: `nodename` and `release`.
  - `arch` comes from `uname().machine`, for example `x86_64` or `aarch64`. Glances
    shows Python's `architecture()[0]` (`64bit`); ogle shows the machine string instead.
    `02-system-modules.md` decides the display.
  - `distro` is `NAME` + `" "` + `VERSION_ID` from `/etc/os-release`, falling back to
    `/usr/lib/os-release`, with quotes stripped. It is absent if neither file exists.
- **PLT-021 macOS**
  - `os_name` = `"macOS"`.
  - `hostname` comes from `gethostname()`.
  - `os_version` comes from sysctl `kern.osproductversion` (string), for example `14.5`.
    A task with system-version compatibility enabled (`kern.system_version_compat`) gets
    the value from `SystemVersionCompat.plist` instead; ogle MUST NOT enable it.
  - `arch` comes from `uname().machine`.
  - `distro` is absent.
- **PLT-022 FreeBSD/OpenBSD/NetBSD**
  - `os_name` and `os_version` come from `uname().sysname` and `release`.
  - `hostname` comes from `gethostname()`, and `arch` from `uname().machine`.
  - `distro` is absent.
- **PLT-023 Windows**
  - `os_name` = `"Windows"`.
  - `hostname` comes from `GetComputerNameExW(ComputerNameDnsHostname)`.
  - The version comes from `RtlGetVersion`, giving major.minor.build.
    `GetVersionEx` lies under compatibility shims and MUST NOT be used.
  - The display name comes from registry
    `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`: `ProductName` plus
    `DisplayVersion`. `DisplayVersion` exists only from Windows 10 20H2; on 1809–2004 use
    `ReleaseId` instead (`ReleaseId` is frozen at `2009` from 20H2 on). If the build is
    ≥ 22000 and `ProductName` says "Windows 10", replace it with "Windows 11", because the
    registry value is stale on Windows 11.
  - `arch` comes from `IsWow64Process2(GetCurrentProcess(), …)`, using `pNativeMachine`
    (an `IMAGE_FILE_MACHINE_*` value). `GetNativeSystemInfo().wProcessorArchitecture` is
    only a fallback: an x64 build running under emulation on ARM64 sees x64 there, not
    ARM64.

## 4. Boot time (`boot_time`)

Cost C0. Re-read when the system clock jumps by more than 1 s compared with the monotonic
clock.

- **PLT-030 Linux**: the `btime` line of `/proc/stat` (Unix seconds).
- **PLT-031 macOS / FreeBSD / OpenBSD**: sysctl `kern.boottime` (`struct timeval`).
  NetBSD: `kern.boottime` returns a `struct timespec`, not a `timeval`.
- **PLT-032 Windows**: `NtQuerySystemInformation(SystemTimeOfDayInformation)`.
  `BootTime` is a FILETIME (100 ns since 1601). Convert it to Unix seconds. This value is
  stable, whereas `now - GetTickCount64()` (the psutil method) drifts with clock
  adjustments. Microsoft documents `SYSTEM_TIMEOFDAY_INFORMATION` only as an opaque
  48-byte block; the `BootTime` member comes from unofficial headers. Fallback:
  `GetSystemTimeAsFileTime() - GetTickCount64()`. (Stability claim unverified.)

## 5. CPU topology and frequency

`CpuTopology { logical: int, physical?: int, model_name? }`. Cost C0.

`CpuFreq { current_hz?, max_hz? }`. Cost C1, cached for 4 s (Glances caches for 4 s).

- **PLT-040 Linux**
  - `logical` = `sysconf(_SC_NPROCESSORS_ONLN)`.
  - `physical` = the number of distinct `(physical_package_id, core_id)` pairs under
    `/sys/devices/system/cpu/cpu*/topology/`. If those files are missing, count the
    distinct `core_cpus_list` (or `thread_siblings_list`) values.
  - `model_name`: the first `model name` line in `/proc/cpuinfo`. If there is none, use
    the first `Model` or `cpu model` line. On ARM without a model name, look up `CPU implementer` and
    `CPU part` in a compiled-in table, as Glances does. Otherwise the field is absent.
  - Re-read topology when the number of entries in `/proc/stat` changes (CPU hotplug).
- **PLT-041 Linux frequency**
  - `current_hz` = mean of `/sys/devices/system/cpu/cpufreq/policy*/scaling_cur_freq`.
  - `max_hz` = max of `cpuinfo_max_freq`.
  - Both files are in kHz; multiply by 1000.
  - Fallback: the `cpu MHz` lines of `/proc/cpuinfo`, for the current value only.
  - Absent if neither source exists, as in many VMs.
- **PLT-042 macOS**
  - `logical` = sysctl `hw.logicalcpu`; `physical` = `hw.physicalcpu`.
  - `model_name` = `machdep.cpu.brand_string`.
  - Frequency: `hw.cpufrequency` and `hw.cpufrequency_max` (Hz) on Intel. On release
    arm64 kernels these sysctls exist but return `ENOENT`.
  - Apple Silicon: `current_hz` is absent; no API gives the current frequency.
    `max_hz` MAY be read, best effort, from the IOKit `AppleARMIODevice` entry named
    `pmgr`, properties `voltage-states<N>-sram` (8-byte entries: 4-byte LE frequency +
    4-byte LE voltage), taking the max over CPU-cluster tables. This is what psutil does.
    The properties are undocumented and Apple renumbered them on M5; absent if not found.
- **PLT-043 FreeBSD**
  - `logical` = `hw.ncpu`.
  - `physical` = sysctl `kern.smp.cores` ("Number of physical cores online"; psutil uses
    it). smp(4) calls it a best-effort guess: where SMT detection is missing it equals
    `hw.ncpu`. On releases without it, fall back to parsing `kern.sched.topology_spec`
    (XML). The level that holds cores varies by CPU (package, cache group, core), so a
    fixed `level="2"` rule is wrong; the exact parse rule is (unverified). Absent if both
    fail.
  - `model_name` = `hw.model`.
  - Frequency: `dev.cpu.0.freq` (MHz) for current, and the first entry of
    `dev.cpu.0.freq_levels` for max. Absent without cpufreq(4).
- **PLT-044 OpenBSD/NetBSD**
  - `logical` = `hw.ncpuonline` (OpenBSD) or `hw.ncpu`.
  - `physical` is absent.
  - `model_name` = `hw.model`.
  - OpenBSD frequency: `hw.cpuspeed` (MHz) for current; max is absent.
  - NetBSD frequency: `machdep.cpu.frequency.current` or `.available`, where present.
    (unverified: cpufreq(9) does not document sysctl names; x86 drivers may use
    `machdep.est.*` / `machdep.powernow.*` instead.)
- **PLT-045 Windows**
  - Use `GetLogicalProcessorInformationEx(RelationProcessorCore)`:
    - `physical` = number of core records;
    - `logical` = sum of set bits in each core's group mask.
    - This handles processor groups (>64 CPUs).
  - `model_name` = registry
    `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0\ProcessorNameString`, trimmed.
  - Frequency: `CallNtPowerInformation(ProcessorInformation)` gives an array of
    `PROCESSOR_POWER_INFORMATION`, one per logical CPU (size the buffer for all). Use
    `CurrentMhz` (mean) and `MaxMhz` (max). Microsoft defines `CurrentMhz` as `MaxMhz`
    times the current throttle, so it never shows turbo clocks and is often equal to
    `MaxMhz`. The struct is missing from `WinNT.h`; declare it locally.

## 6. CPU times (`cpu_times`)

`CpuTimes { user, system, idle, nice?, iowait?, irq?, softirq?, steal?, guest?,
guest_nice?, dpc?, interrupt? }`. All values are cumulative seconds. Percentages are
derived above this layer, as Δfield / Δsum.

- **PLT-050 Sum rule.** The platform also returns `sum`, the denominator for percentages.
  On Linux `sum` excludes `guest` and `guest_nice`, because the kernel already counts
  them in `user` and `nice` (psutil does the same). On other platforms `sum` is the sum
  of all present fields, except as PLT-055 states for Windows.
- **PLT-051 Linux**: `/proc/stat`. Cost C1, one read serves both the total and the per-CPU
  values.
  - Line `cpu` holds the totals; lines `cpuN` hold per-CPU values.
  - Columns in order: user, nice, system, idle, iowait, irq, softirq, steal, guest,
    guest_nice.
  - The unit is USER_HZ ticks. Divide by `sysconf(_SC_CLK_TCK)` (normally 100).
  - Older kernels lack the trailing columns; treat those fields as absent.
  - Offline CPUs have no `cpuN` line. Per-CPU entries are keyed by N, not by position.
- **PLT-052 Linux, same read**: `ctxt`, `intr` (first number), `softirq` (first number)
  and `procs_running` come from the same `/proc/stat` read. `cpu_counters` (PLT-060)
  MUST reuse it when both are requested in one cycle.
- **PLT-053 macOS**
  - Total: `host_statistics(HOST_CPU_LOAD_INFO)` returns `cpu_ticks[USER, SYSTEM, IDLE, NICE]`.
  - Per-CPU: `host_processor_info(PROCESSOR_CPU_LOAD_INFO)`. The returned buffer MUST be
    released with `vm_deallocate` on every call, or memory leaks.
  - Ticks convert with `sysconf(_SC_CLK_TCK)`. Only ratios matter.
  - Present fields: user, system, idle, nice. Absent: iowait, irq, softirq, steal, guest,
    dpc, interrupt.
- **PLT-054 FreeBSD**
  - Total: sysctl `kern.cp_time` (`long[5]`: user, nice, sys, intr, idle).
  - Per-CPU: `kern.cp_times` (`long[5 × ncpu]`).
  - The unit is stathz ticks: `kern.clockrate` gives `stathz`.
  - `intr` maps to `irq`. Absent: iowait, softirq, steal, guest.
  - OpenBSD: `CPUSTATES` is 6 (user, nice, sys, spin, intr, idle); add `spin` into
    system. Per-CPU uses `kern.cp_time2.N` (`u_int64_t[6]`). On OpenBSD 6.4 and later
    (release number unverified),
    `kern.cpustats.N` (`struct cpustats`: `cs_time[6]` plus `cs_flags & CPUSTATS_ONLINE`)
    gives the same counters and an online flag; skip offline CPUs. `kern.cp_time`
    (`long[6]`) is the **average** over online CPUs, not the sum, so the total MUST be
    computed as the sum of the per-CPU values (psutil does the same).
  - NetBSD: `kern.cp_time` (`uint64_t[5]`) returns the sum over all CPUs, or per-CPU data
    when the buffer has room for `5 × ncpu` values (or with the CPU number appended to the
    MIB).
- **PLT-055 Windows**
  - Total: `GetSystemTimes` returns idle, kernel and user times as FILETIME (100 ns units).
  - Per-CPU: `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` returns
    `IdleTime, KernelTime, UserTime, DpcTime, InterruptTime, InterruptCount` per CPU.
    With more than 64 CPUs, use `NtQuerySystemInformationEx` with each processor group.
  - `KernelTime` includes idle. Set `system = kernel - idle - dpc - interrupt`, and
    report `dpc` and `interrupt` separately. (`KernelTime` also includes DPC and
    interrupt time; psutil does not subtract them, so its `system` overlaps them.)
  - Microsoft documents only `IdleTime`, `KernelTime` and `UserTime`; `DpcTime`,
    `InterruptTime` and `InterruptCount` sit in the `Reserved1`/`Reserved2` members and
    come from unofficial headers (stable since NT, used by psutil).
  - `sum = user + kernel`.
  - Absent: nice, iowait, softirq, steal, guest.
  - For the total, use the per-CPU sum, so that `dpc` and `interrupt` are available in
    the total too. `GetSystemTimes` is the fallback.

## 7. CPU counters (`cpu_counters`)

`CpuCounters { ctx_switches?, interrupts?, soft_interrupts?, syscalls? }`. All are
cumulative counts. Cost C1.

| Platform | Source | ctx_switches | interrupts | soft_interrupts | syscalls |
|---|---|---|---|---|---|
| Linux | `/proc/stat` (PLT-052) | `ctxt` | `intr` first number | `softirq` first number | absent |
| macOS | — | absent | absent | absent | absent |
| FreeBSD | sysctl `vm.stats.sys.v_swtch`, `v_intr`, `v_soft`, `v_syscall` (`uint64`; read with an 8-byte buffer, a 4-byte buffer gets a truncated value) | yes | yes | yes | yes |
| OpenBSD | sysctl `vm.uvmexp` (`struct uvmexp`: `swtch`, `intrs`, `softs`, `syscalls`; 32-bit `int`, widen per PLT-002) | yes | yes | yes | yes |
| NetBSD | sysctl `vm.uvmexp2` (`struct uvmexp_sysctl`, same names, 64-bit) | yes | yes | yes | yes |
| Windows | `SystemPerformanceInformation` (`ContextSwitches`, `SystemCalls`); interrupts = Σ per-CPU `InterruptCount` (PLT-055). Microsoft documents `SYSTEM_PERFORMANCE_INFORMATION` as opaque; the offsets come from unofficial headers, as in psutil | yes | yes | absent | yes |

- **PLT-060** Linux has no system-wide syscall counter, so `syscalls` is absent. Glances
  shows 0; ogle hides the field (GEN-011).
- **PLT-061** macOS: psutil (5.9.x) calls `host_statistics(HOST_VM_INFO)` but reads the
  result into a BSD `struct vmmeter`, so its four values are reinterpreted VM page
  counts, not context switches or interrupts. XNU exposes no system-wide counters for
  these. ogle marks all four fields absent on macOS rather than show wrong numbers.
- **PLT-062** Windows: psutil reports `soft_interrupts` as 0. ogle marks it absent. The
  DPC count (`SystemInterruptInformation.DpcCount`, which psutil sums) MAY be added
  later as a Windows-only field; it is not in phase 1. Microsoft documents
  `SYSTEM_INTERRUPT_INFORMATION` as opaque.

## 8. Load average (`load_avg`)

`LoadAvg { min1, min5, min15, emulated: bool }`. Cost C1.

- **PLT-070 Linux**: the first three fields of `/proc/loadavg`.
- **PLT-071 macOS / BSD**: `getloadavg(3)`.
- **PLT-072 Windows**: Windows has no load average. ogle emulates one, as psutil does,
  but cheaply:
  - Each cycle, count threads in state Running (2), Ready (1) or Standby (3) across all
    processes. These come from `SystemProcessInformation` thread records. If the process
    module is enabled this costs nothing extra, because PLT-155 reads the same data.
    Otherwise it costs one call.
  - Update three exponentially damped averages:
    `load = load * e + n * (1 - e)`, where `e = exp(-Δt / T)` and T = 60, 300 or 900 s.
    Δt is the real elapsed time, so the averages stay correct under any refresh interval.
  - Start from the first sample's `n`, not from 0. Glances shows zeros for minutes.
  - Set `emulated = true`. The load module shows emulated values exactly like real ones
    and adds no title marker (SYS-070 in `02-system-modules.md`).

## 9. Memory (`memory`)

`Memory { total, available, used, free, percent, active?, inactive?, buffers?, cached?,
shared?, wired? }`. All values are in bytes. `used = total - available` and
`percent = used / total × 100`, matching Glances (`mem/__init__.py:216-218`). The
platform returns `total`, `available`, `free` and the optional fields; `used` and
`percent` are derived above it. Cost C1.

- **PLT-080 Linux**: `/proc/meminfo` (values in kB; multiply by 1024).
  - `total` = `MemTotal`.
  - `available` = `MemAvailable`. On kernels without it (before 3.14), estimate
    `MemFree + Buffers + Cached + SReclaimable`.
  - `free` = `MemFree`, `buffers` = `Buffers`, `cached` = `Cached + SReclaimable`
    (psutil rule).
  - `active` = `Active`, `inactive` = `Inactive`, `shared` = `Shmem`.
  - `wired` is absent.
  - ZFS ARC: if `/proc/spl/kstat/zfs/arcstats` exists, add `size` to `cached` and add
    `size - c_min` to `available` (Glances rule). This costs one more C1 read; do it only
    when the file exists, checked once at start-up.
- **PLT-081 macOS**
  - `total` = sysctl `hw.memsize`.
  - The rest comes from `host_statistics64(HOST_VM_INFO64)`, which returns page counts.
    Multiply by `host_page_size` (16 KiB on Apple Silicon, 4 KiB on Intel).
    - `free` = `free_count - speculative_count` (`free_count` already includes the
      speculative pages)
    - `active` = `active_count`, `inactive` = `inactive_count`, `wired` = `wire_count`
    - `available` = `inactive_count + free_count`, using `free_count` *before* the
      speculative subtraction (psutil rule)
    - `cached` = `external_page_count` (file-backed pages)
  - Absent: buffers, shared.
- **PLT-082 FreeBSD**
  - `total` = `hw.physmem`.
  - Page counts come from sysctl `vm.stats.vm.v_free_count`, `v_active_count`,
    `v_inactive_count`, `v_wire_count` and `v_laundry_count`, times `hw.pagesize`.
  - `buffers` = `vfs.bufspace`.
  - `cached` = `v_cache_count` × page size where present (≤ FreeBSD 11). Absent on newer
    releases: the cache queue is gone, and with `COMPAT_FREEBSD11` the sysctl survives
    only as a dummy that reads 0, so it MUST NOT be used there.
  - `shared` is absent. `vm.vmtotal` (`t_rmshr`) has a figure, as psutil uses, but that
    sysctl walks every VM object and process (C3), so ogle does not read it.
  - `available` = `inactive + laundry + cached + free`. (psutil uses `inactive + free`.)
  - ZFS ARC: if sysctl `kstat.zfs.misc.arcstats.size` exists (checked once at start-up),
    read it and `kstat.zfs.misc.arcstats.c_min` (bytes, C1) and apply the SYS-081
    adjustment: add `size` to `cached` and `size - c_min` to `available`. On releases
    without `v_cache_count`, `cached` is then present and equals the ARC size.
  - OpenBSD: `vm.uvmexp` (`npages`, `free`, `active`, `inactive`, `wired`, in pages) and
    `hw.physmem64` (bytes). `available = free + inactive`. `buffers` = sysctl
    `{CTL_VFS, VFS_GENERIC, VFS_BCACHESTAT}` (`struct bcachestats`) `numbufpages` × page
    size (psutil rule). `cached` and `shared` are absent.
  - NetBSD: `vm.uvmexp2` (`struct uvmexp_sysctl`; pages, shift by `pageshift`) and
    `hw.physmem64`. `available = free + inactive`. `cached` = `filepages + execpages`
    (psutil rule). `buffers` and `shared` are absent.
- **PLT-083 Windows**
  - `GlobalMemoryStatusEx`: `total` = `ullTotalPhys`, `available` = `ullAvailPhys`.
  - `free` = `available`, because Windows has no separate free figure here.
  - `cached` = `GetPerformanceInfo().SystemCache × PageSize`.
  - Absent: active, inactive, buffers, shared, wired.

## 10. Swap (`swap`)

`Swap { total, used, free, swapped_in_bytes?, swapped_out_bytes? }`. Values are in
bytes. `percent` is derived as `used / total × 100`, or absent when `total = 0`. Cost C1.

- **PLT-090 Linux**
  - `/proc/meminfo`: `SwapTotal`, `SwapFree`; `used = total - free`. Values are in kB.
    The same read as PLT-080 MUST serve both domains in one cycle.
  - `/proc/vmstat` `pswpin` and `pswpout` are page counts; multiply by the page size.
    They are not displayed in phase 1, so ogle MUST NOT read `/proc/vmstat` unless a
    module asks for them.
- **PLT-091 macOS**
  - sysctl `vm.swapusage` (`struct xsw_usage`): `xsu_total`, `xsu_used`, `xsu_avail`.
  - Swap-in and swap-out come from `vm_statistics64.swapins` / `swapouts`, in pages
    (pages swapped through compressor segments).
  - Swap is dynamic on macOS: `total` changes over time.
- **PLT-092 FreeBSD**
  - `kvm_getswapinfo()` with a kvm handle opened on `/dev/null` (psutil uses
    `kvm_open(NULL, "/dev/null", NULL, O_RDONLY, …)`; `kvm_openfiles` works the same way),
    which works unprivileged. `ksw_total` / `ksw_used` are in pages.
    Fallback: sysctl `vm.swap_info`.
  - Swap-in and swap-out: `vm.stats.vm.v_swappgsin` / `v_swappgsout`, in pages.
  - OpenBSD/NetBSD: `swapctl(SWAP_STATS)` and sum the entries (`se_nblks`, `se_inuse`),
    which are in DEV_BSIZE (512-byte) blocks. `SWAP_STATS` needs no privilege.
- **PLT-093 Windows**
  - Use `NtQuerySystemInformation(SystemPageFileInformation)`: for each page file,
    `TotalSize` and `TotalInUse`, in pages. (Structure layout and units unverified:
    the class is not documented on Microsoft Learn.)
  - `total` and `used` = sums × page size.
  - Swap-in and swap-out are absent.
  - psutil uses `ullTotalPageFile - ullTotalPhys` plus a PDH counter; ogle avoids PDH
    entirely.

## 11. Network interfaces (`net_interfaces`)

`NetIf { name, display_name?, rx_bytes, tx_bytes, is_up: bool, speed_bps?,
has_ip?: bool }`.

- `speed_bps` is in bits per second, using the SI scale. Glances multiplies Mbps by
  1048576; ogle uses 10^6 (GEN-011).
- `has_ip` is filled only when `want_addrs` is true. It is true when the interface has
  at least one IPv4 or IPv6 address.

- **PLT-100 Linux**
  - Counters: `/proc/net/dev`, cost C1. Columns 1 and 9 after the colon are rx bytes
    and tx bytes. The name is the text before the colon, trimmed.
  - `is_up` and addresses: one `getifaddrs()` call. This is a single netlink dump.
    - `is_up` = `IFF_UP && IFF_RUNNING` on the `AF_PACKET` entry.
    - `has_ip` = at least one `AF_INET`/`AF_INET6` entry for the name.
    - Cost C1. Refresh every cycle, because it is cheap.
  - `speed_bps` = `/sys/class/net/<if>/speed` × 10^6.
    - The read fails with `EINVAL` when the link is down, and returns `-1` or an error
      for virtual interfaces; either case means absent.
    - Cost C2. Read it at start-up, when an interface appears, and when its `is_up`
      changes. Do not read it every cycle.
  - Alias names (`ifalias`) are not used. Display aliases come from config
    (`03-io-modules.md`).
- **PLT-101 macOS**
  - Counters: sysctl `{CTL_NET, PF_ROUTE, 0, 0, NET_RT_IFLIST2, 0}` returns `if_msghdr2`
    records with `if_data64` (64-bit `ifi_ibytes`, `ifi_obytes`, `ifi_baudrate`).
  - This one call gives every interface's counters, flags and speed: cost C1.
  - Names come from `if_indextoname` (C0, cached by index).
  - `is_up` = `IFF_UP && IFF_RUNNING` from `ifm_flags`.
  - `speed_bps` = `ifi_baudrate`; 0 means absent.
  - Addresses: `getifaddrs()`.
  - Do not use the `if_data` reached through `getifaddrs()` `AF_LINK` entries. Its byte
    counters are 32-bit.
- **PLT-102 FreeBSD/OpenBSD/NetBSD**
  - `getifaddrs()`. The `AF_LINK` entries carry `struct if_data` in `ifa_data`, with
    `ifi_ibytes`, `ifi_obytes` and `ifi_baudrate`. These are 64-bit on FreeBSD 11+,
    OpenBSD and NetBSD.
  - `is_up` = `IFF_UP && IFF_RUNNING`.
  - Addresses come from the same call.
  - Cost C1.
- **PLT-103 Windows**
  - The interface set and display names come from `GetAdaptersAddresses(AF_UNSPEC,
    GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER, …)`.
    - `name` = `FriendlyName`, for example "Ethernet" or "Wi-Fi".
    - `has_ip` = `FirstUnicastAddress != NULL`.
    - Cost C3. Re-run every 30 s, or when the counter call returns an unknown index.
  - Counters, state and speed: one `GetIfTable2` call per cycle (C1).
    - Keep only rows whose `InterfaceIndex` is in the cached adapter set. This drops the
      dozens of filter-driver pseudo-interfaces that `GetIfTable2` also returns.
    - `rx_bytes` = `InOctets`, `tx_bytes` = `OutOctets` (64-bit).
    - `is_up` = `OperStatus == IfOperStatusUp`.
    - `speed_bps` = `max(ReceiveLinkSpeed, TransmitLinkSpeed)`. `ULONG64_MAX` or 0 means
      absent.
    - Free the table with `FreeMibTable`.

## 12. Disk I/O (`disk_io`)

`DiskIo { name, kind: disk|partition|virtual, read_count, write_count, read_bytes,
write_bytes, read_time_ms?, write_time_ms? }`. All are cumulative counters. Cost C1.

- **PLT-110 Linux**: `/proc/diskstats`. Whitespace-separated fields:
  `major minor name rd_ios rd_merges rd_sectors rd_ticks wr_ios wr_merges wr_sectors
  wr_ticks in_flight io_ticks time_in_queue` (newer kernels add discard and flush
  columns; ignore them).
  - Bytes = sectors × 512. The kernel always reports 512-byte units, whatever the device
    sector size.
  - Times are in ms.
  - `kind`:
    - `disk` if `/sys/block/<name>` exists.
    - `partition` if `/sys/class/block/<name>/partition` exists.
    - `virtual` for `loop*`, `ram*`, `zram*`, `dm-*` and `md*` (the module decides
      whether to hide these).
    - Compute `kind` once per name (C0) and cache it.
- **PLT-111 macOS**: IOKit.
  - Match `IOServiceMatching("IOMedia")` with the property `Whole = true`. For each
    match, take the parent `IOBlockStorageDriver`, then its `Statistics` dictionary:
    `Bytes (Read)`, `Bytes (Write)`, `Operations (Read)`, `Operations (Write)`,
    `Total Time (Read)` and `Total Time (Write)`. The times are in ns; divide by 10^6.
  - `name` = the BSD name of the media, for example `disk0`. `kind` = `disk`.
  - Cache the driver `io_registry_entry_t` objects across cycles. Re-enumerate only
    every 30 s, or when a read fails. Re-enumeration is C3; reading cached entries is C2.
- **PLT-112 FreeBSD**
  - `devstat_getdevs()` (libdevstat), or the raw sysctl `kern.devstat.all`.
  - For each device: `bytes[DEVSTAT_READ/WRITE]`, `operations[...]` and
    `duration[...]` (a `struct bintime`; convert to ms).
  - `name` = `device_name` + `unit_number`, for example `ada0`.
  - Skip devices that are not `DEVSTAT_TYPE_DIRECT`, such as pass(4).
  - OpenBSD: sysctl `hw.diskstats` (`struct diskstats`: `ds_name`, `ds_rxfer`,
    `ds_wxfer`, `ds_rbytes`, `ds_wbytes`, `ds_time`). Per-direction times are absent.
  - NetBSD: sysctl `hw.iostats` (`struct io_sysctl`). Per-direction times are absent.
- **PLT-113 Windows**
  - Probe `\\.\PhysicalDrive0` up to `N`, stopping at the first missing index plus 4 to
    tolerate gaps. Open each with `CreateFileW` with access rights 0, which works
    unprivileged.
  - For each open handle, call `DeviceIoControl(IOCTL_DISK_PERFORMANCE)`:
    `BytesRead`, `BytesWritten`, `ReadCount`, `WriteCount`, and `ReadTime` /
    `WriteTime` (100 ns units; convert to ms).
  - `name` = `PhysicalDriveN`. `kind` = `disk`.
  - Keep the handles open between cycles; re-probe every 30 s.
  - psutil reports per volume letter. ogle reports per physical disk, which matches the
    other platforms. The fs module already shows volume letters.

## 13. File systems (`filesystems`, `fs_usage`)

`Mount { device, mount_point, fs_type, read_only: bool, is_network: bool,
is_physical: bool }` and `FsUsage { total, free_to_user, used }` in bytes.

- `used = total - free_total` (blocks minus free blocks, including reserved).
- `percent`, used for alerts, is derived above as `(total - free_to_user) / total × 100`
  (IO-065 in `03-io-modules.md`). Root-reserved blocks count as used, so the value
  reaches 100 % when unprivileged writes start to fail. This is the Glances formula; it
  is deliberately not `df`'s `used / (used + avail)`.

- **PLT-120 Linux**
  - The mount list comes from `/proc/self/mounts` (C1). Re-parse only when `poll()` on
    an open fd to that file reports `POLLPRI` (a mount table change). Otherwise the
    list is C0.
  - `is_physical`: fs_type is listed in `/proc/filesystems` without the `nodev` prefix,
    or is `zfs` (psutil rule). Read `/proc/filesystems` once (C0).
  - `is_network` when fs_type is one of `nfs`, `nfs4`, `cifs`, `smb3`, `smbfs`,
    `fuse.sshfs`, `9p`, `afs` or `ceph`.
  - `read_only` from the options field (`ro`).
  - Escape sequences in mount points (`\040`, `\011`, `\012`, `\134`) MUST be decoded.
  - Usage: `statvfs(mount_point)`.
    - `total = f_blocks × f_frsize`
    - `free_to_user = f_bavail × f_frsize`
    - `used = (f_blocks - f_bfree) × f_frsize`
    - Cost C2. It may block on network mounts (PLT-010).
- **PLT-121 macOS / FreeBSD / OpenBSD / NetBSD**
  - `getfsstat(NULL, 0, MNT_NOWAIT)`, then `getfsstat(buf, size, MNT_NOWAIT)`. NetBSD
    uses `getvfsstat(buf, bufsize, ST_NOWAIT)` instead, which fills `struct statvfs`
    (flags in `f_flag`: `ST_RDONLY`, `ST_LOCAL`); `bufsize` is in bytes.
  - One call returns every mount with `f_mntfromname`, `f_mntonname`, `f_fstypename`,
    `f_flags` (`MNT_RDONLY`, `MNT_LOCAL`) and the cached block counts.
  - `MNT_NOWAIT` / `ST_NOWAIT` returns the cached figures without asking each file
    system, so it does not block. The counts may lag by a few seconds, which is
    acceptable, so `fs_usage` is free here (C1 for the whole table) and PLT-010 does not
    apply.
  - Units for `f_blocks`/`f_bfree`/`f_bavail`: macOS `f_bsize` (fundamental block size);
    FreeBSD `f_bsize` (fragment size); OpenBSD `f_bsize` (fundamental block size);
    NetBSD `f_frsize` (fragment size), not `f_bsize`.
  - `is_network = !(f_flags & MNT_LOCAL)` (NetBSD: `ST_LOCAL`).
  - `is_physical = MNT_LOCAL`, excluding the types `devfs`, `autofs`, `nullfs`, `tmpfs`
    and `fdescfs`.
  - On macOS, also exclude mounts with `MNT_DONTBROWSE` and APFS system snapshots, which
    are mounted under `/System/Volumes/` and are not user-facing. `03-io-modules.md` owns
    the final filter.
- **PLT-122 Windows**
  - `GetLogicalDriveStringsW` lists the drive roots (C1).
  - For each root:
    - `GetDriveTypeW`: skip `DRIVE_NO_ROOT_DIR`. `is_network = DRIVE_REMOTE`.
      `is_physical = DRIVE_FIXED || DRIVE_REMOVABLE`.
    - `GetVolumeInformationW` gives `fs_type` (for example `NTFS`) and the read-only flag
      `FILE_READ_ONLY_VOLUME`. Cache it per root (C0) and refresh every 60 s.
  - Usage: `GetDiskFreeSpaceExW(root)`:
    - `total` = `TotalNumberOfBytes`
    - `free_to_user` = `FreeBytesAvailableToCaller`
    - `used` = `total - TotalNumberOfFreeBytes`
  - Call `SetErrorMode(SEM_FAILCRITICALERRORS)` once at start-up. Without it, an empty
    removable drive raises a "No disk" dialog.
  - Remote drives may block (PLT-010).
  - `device` = the root, for example `C:\`. `mount_point` = the same.
  - Mounted folders (volumes mounted into NTFS directories) are not listed in phase 1.

## 14. Sensors (`sensors`, `battery`)

`Sensor { kind: temperature|fan, chip, label, value, high?, critical? }`.
- Temperature values are in °C (f64).
- Fan values are in RPM.

`Battery { name, percent, state: charging|discharging|full|not_charging|unknown,
secs_left? }`.

- **PLT-130 Linux temperatures and fans**: hwmon.
  - Enumerate `/sys/class/hwmon/hwmon*` (C0, re-done every 60 s).
  - `chip` = contents of `name`.
  - For each `temp<N>_input` (millidegrees C): read `temp<N>_label` (C0, absent means
    `chip N`), `temp<N>_max` (high) and `temp<N>_crit` (critical). Labels, max and crit
    are static and MUST be read once (C0), not every cycle.
  - Fans: `fan<N>_input` (RPM) and `fan<N>_label`.
  - Per cycle, only the `*_input` files are read (C2).
  - Some hwmon drivers talk to slow I2C/SMBus or ACPI hardware, and a read can take
    milliseconds. This is why the sensors module defaults to a 10 s refresh (GEN-006).
    A read that fails with `EIO`/`ENODATA` makes that sensor absent for the cycle.
  - Fallback when no hwmon temperature exists:
    `/sys/class/thermal/thermal_zone*/temp` (millidegrees). `chip` = the `type` file.
  - The `drivetemp` hwmon driver exposes disk temperatures through the same path, which
    replaces the out-of-scope hddtemp source at no cost.
- **PLT-131 Linux battery**: `/sys/class/power_supply/*`. Use entries whose `type` is
  `Battery` and whose `scope`, if present, is not `Device`, which excludes peripheral
  batteries such as mice.
  - `percent` = `capacity`. Fallback: `energy_now / energy_full` or
    `charge_now / charge_full`.
  - `state` from `status`: `Charging`, `Discharging`, `Full`, `Not charging`, else
    `unknown`.
  - `secs_left` = `energy_now / power_now × 3600` while discharging. Fallback:
    `charge_now / current_now`. Absent when the inputs are missing or 0.
  - Cost C2.
- **PLT-132 macOS**
  - Battery: `IOPSCopyPowerSourcesInfo` plus `IOPSCopyPowerSourcesList`.
    - `percent` = `kIOPSCurrentCapacityKey / kIOPSMaxCapacityKey × 100`.
    - `state` from `kIOPSIsChargingKey`, `kIOPSIsChargedKey` and
      `kIOPSPowerSourceStateKey`.
    - `secs_left` = `kIOPSTimeToEmptyKey × 60` when > 0.
    - Cost C1. Release the CF objects every call.
  - Temperatures and fans:
    - Intel: AppleSMC through IOKit (`IOServiceOpen("AppleSMC")` with
      `kSMCReadKey`). Keys `TC0P`/`TC0D` (CPU) and `F<n>Ac` (fan actual RPM).
    - Apple Silicon: `IOHIDEventSystemClient` is a private API.
    - ogle MAY implement these as best effort. Phase 1 MAY ship with temperatures and
      fans absent on macOS. psutil does not support them either.
- **PLT-133 FreeBSD**
  - Temperatures: sysctl `dev.cpu.N.temperature` (coretemp(4)/amdtemp(4)) and
    `hw.acpi.thermal.tzN.temperature`. The value is an `int` in deci-Kelvin (format
    `IK`); °C = (v − 2731)/10 (psutil rule).
  - `high` = `dev.cpu.N.coretemp.tjmax` (same format) where present (psutil reads it).
  - `critical` = `hw.acpi.thermal.tzN._CRT` where present.
  - Fans are absent.
  - Battery: `hw.acpi.battery.life` (percent), `hw.acpi.battery.state` (bit 0
    discharging, bit 1 charging, bit 2 critical) and `hw.acpi.battery.time` (minutes;
    −1 whenever the battery is not discharging). These report all batteries combined.
    `hw.acpi.acline` is the fallback for state.
  - OpenBSD: the `hw.sensors` framework, through sysctl
    `{CTL_HW, HW_SENSORS, dev, type, n}`, giving `struct sensor` with `int64_t value`.
    `SENSOR_TEMP` is µK, so °C = v/10^6 − 273.15. `SENSOR_FANRPM` is RPM. The battery
    comes from the `acpibat*` sensors (`SENSOR_WATTHOUR` in µWh, remaining/full;
    `SENSOR_AMPHOUR` in µAh on some batteries) or from `apm(4)`: `APM_IOC_GETPOWER` on
    `/dev/apm`, opened read-only, giving `battery_life` (percent), `battery_state`,
    `ac_state` and `minutes_left`.
  - NetBSD: envsys requires proplib ioctls on `/dev/sysmon`. Sensors are absent in
    phase 1.
- **PLT-134 Windows**
  - Temperatures and fans are absent in phase 1. The only standard source is WMI
    `MSAcpi_ThermalZoneTemperature`, which needs administrator rights and the COM/WMI
    runtime; both conflict with GEN-014 and the performance budget.
  - Battery: `GetSystemPowerStatus` (C1).
    - `percent` = `BatteryLifePercent`; 255 means absent.
    - `state` from `BatteryFlag`: bit 8 charging, 128 no battery (omit the entry), and
      `ACLineStatus`.
    - `secs_left` = `BatteryLifeTime`; `0xFFFFFFFF` means absent.
    - `name` = `Battery`.

## 15. Processes (`processes`)

### 15.1 Record

```
Proc {
  pid, ppid?, start_time,            // start_time: Unix seconds (f64); with pid it forms the process identity
  name, cmdline?: [string],          // cmdline absent for kernel threads or when access is denied
  uid?, username?,                   // username resolved through the cache (PLT-127)
  status: char,                      // see PLT-150
  nice?, priority_class?,            // nice on Unix, priority_class on Windows
  num_threads,
  cpu_user, cpu_system,              // cumulative seconds
  rss, vms?,                         // bytes
  io_read_bytes?, io_write_bytes?,   // cumulative bytes; absent = '?' in the UI
  last_cpu?,                         // CPU core the process last ran on
  is_kernel_thread: bool,
}
```

The derived fields are computed above the platform layer:
- CPU% = Δ(cpu_user + cpu_system) / Δt × 100, per process identity;
- MEM% = rss / memory.total × 100;
- the I/O rates.

- **PLT-140 Identity.** The pair `(pid, start_time)` identifies a process. All per-process
  caches (cmdline, username, previous CPU and I/O counters) are keyed by it. A PID reused
  by a new process gets a new identity, so no rate is ever computed across two processes.
- **PLT-141 Exclusions.** These entries are never returned: Linux has none; macOS
  `kernel_task` (pid 0); FreeBSD, OpenBSD and NetBSD `idle` (the kernel idle process,
  `P_SYSTEM`, named `idle`; PROC-003); Windows "System Idle Process" (pid 0). This
  matches Glances.

### 15.2 Linux

Enumerate by `getdents64` on an open fd for `/proc`, keeping only entries whose names
are all digits. If `/proc` is mounted with `hidepid=1` or `hidepid=2`, only the user's
own processes are visible. This is not an error.

- **PLT-142 `/proc/<pid>/stat` (C2, the only per-cycle read for most fields).**
  - Field 2 (`comm`) is in parentheses and may itself contain spaces or `)`. Parse it as
    the text between the first `(` and the **last** `)`.
  - Fields after that are counted from 1:

    | Field | Name | Becomes |
    |---|---|---|
    | 3 | state | `status` |
    | 4 | ppid | `ppid` |
    | 9 | flags | `is_kernel_thread = flags & PF_KTHREAD (0x00200000)` |
    | 14 | utime | `cpu_user` |
    | 15 | stime | `cpu_system` |
    | 19 | nice | `nice` |
    | 20 | num_threads | `num_threads` |
    | 22 | starttime | ticks since boot; `start_time = boot_time + starttime / CLK_TCK` |
    | 23 | vsize | `vms` (bytes) |
    | 24 | rss | `rss` = rss × page size (pages) |
    | 39 | processor | `last_cpu` |

  - CPU times (fields 14 and 15) are in CLK_TCK ticks.
  - Glances detects kernel threads by `gids.real == 0` (`processes.py:495`), which also
    hides normal root-group processes. ogle uses `PF_KTHREAD` (GEN-011).
- **PLT-143 `uid`**: `fstatat(proc_fd, "<pid>", …).st_uid`. That is the owner of the
  `/proc/<pid>` directory, which is the process's effective UID. This is what `ps -o user`
  and `top` show. It avoids reading `/proc/<pid>/status` every cycle. Cost C2: one
  syscall, no file read. It is needed only for new identities, because the UID is cached
  per identity. A process that changes UID after start (`setuid`) keeps its old user
  until F5 / Ctrl-R resets the cache. `04-processes.md` decides whether to re-check
  periodically.
- **PLT-144 `name`**: `comm` is truncated to 15 bytes. If `comm` is exactly 15 bytes and
  the cmdline is present, use the basename of `cmdline[0]`, but only when it starts with
  `comm` (psutil rule). Otherwise use `comm`.
- **PLT-145 `cmdline`**: `/proc/<pid>/cmdline`, NUL-separated. A trailing NUL is
  dropped. If no NUL appears but spaces do (a process that rewrote its argv), split on
  spaces (psutil rule). An empty file means kernel thread or zombie, so cmdline is
  absent. Cost C2, but read **only for new identities**, or again when the process name
  changes (PLT-125).
- **PLT-146 I/O**: `/proc/<pid>/io`, lines `read_bytes:` and `write_bytes:`. These are
  storage-layer bytes, which is what psutil and Glances use. Reading another user's
  process requires `CAP_SYS_PTRACE` or the same UID; otherwise the read fails with
  `EACCES` and the fields are absent. Cost C2, one extra open+read per process. Read
  only when requested (PLT-011). When the I/O columns are hidden and the sort is not
  `io`, skip it.
- **PLT-147** Do not read `/proc/<pid>/status`, `statm`, `smaps` or `fd/` in the
  per-cycle path. They are only for `process_extended` (PLT-160).

### 15.3 macOS

- **PLT-148** Enumerate with sysctl `{CTL_KERN, KERN_PROC, KERN_PROC_ALL}`. It returns
  `struct kinfo_proc[]` in one call (C1, size the buffer with a first call and add 10%
  headroom). From each entry:
  - `pid` = `kp_proc.p_pid`, `ppid` = `kp_eproc.e_ppid`
  - `uid` = `kp_eproc.e_ucred.cr_uid` (effective)
  - `nice` = `kp_proc.p_nice`
  - status inputs from `kp_proc.p_stat`
  - `start_time` = `kp_proc.p_starttime`
  - fallback name = `kp_proc.p_comm` (`MAXCOMLEN` = 16 characters plus NUL)

  Then, per process, `proc_pidinfo(pid, PROC_PIDTASKINFO)` (C2) gives:
  - `pti_total_user`, `pti_total_system`: Mach absolute time units. Convert with
    `mach_timebase_info`. The ratio is 1/1 on Intel and 125/3 on Apple Silicon.
    psutil once got this wrong; ogle MUST apply the timebase. An x86_64 build running
    under Rosetta 2 sees `mach_timebase_info` = 1/1 while the kernel still reports
    native ticks; psutil therefore divides by sysctl `hw.tbfrequency` (ticks per second)
    instead, which Rosetta does not intercept. ogle SHOULD do the same.
  - `pti_resident_size` → `rss`, `pti_virtual_size` → `vms`.
    `vms` includes the shared cache, so values of hundreds of GB are normal.
  - `pti_threadnum` → `num_threads`.
  - `pti_numrunning`, used for status.

  For processes of other users, `PROC_PIDTASKINFO` fails with `EPERM` unless ogle runs as
  root (XNU applies `CHECK_SAME_USER` to every `proc_pidinfo` flavor except a few, such
  as `PROC_PIDT_SHORTBSDINFO` and `PROC_PIDPATHINFO`). CPU, memory and thread fields are
  then absent and show `?`. This is a platform limitation, not an error.
- **PLT-149 macOS other fields**
  - `name`: `proc_name(pid)` (up to 32 bytes) for new identities. Fall back to `p_comm`.
  - `cmdline`: sysctl `{CTL_KERN, KERN_PROCARGS2, pid}`. The buffer starts with `argc`,
    then the exec path, NUL padding, then argc NUL-separated strings. It fails for other
    users' processes (absent) unless root (exact kernel check unverified). Read for new
    identities, and again when the process name changes (PLT-125).
  - I/O: `proc_pid_rusage(pid, RUSAGE_INFO_V2)` gives `ri_diskio_bytesread` and
    `ri_diskio_byteswritten`. Same-UID or root only (XNU `CHECK_SAME_USER`, `EPERM`).
    Cost C2; read when requested.
  - `last_cpu` and `is_kernel_thread` are absent; there are no kernel threads as processes.

### 15.4 FreeBSD (OpenBSD/NetBSD notes)

- **PLT-151** Enumerate with sysctl `{CTL_KERN, KERN_PROC, KERN_PROC_PROC}`. It returns
  `struct kinfo_proc[]` with one entry per process, not per thread. All per-cycle fields
  come from this single call (C1, no per-process syscalls):
  - `ki_pid`, `ki_ppid`, `ki_comm` (19 bytes), `ki_uid` (effective)
  - `ki_stat` and `ki_wmesg`, for status
  - `ki_nice`, `ki_numthreads`
  - `ki_rusage.ru_utime` / `ru_stime` (`timeval`), `ki_start` (`timeval`)
  - `ki_size` (vms bytes), `ki_rssize` (pages × `hw.pagesize`)
  - `ki_lastcpu`
  - `ki_flag & P_KPROC`, which gives `is_kernel_thread`
- **PLT-152** `cmdline`: sysctl `{CTL_KERN, KERN_PROC, KERN_PROC_ARGS, pid}`,
  NUL-separated. Read for new identities, and again when the process name changes
  (PLT-125). OpenBSD and NetBSD use a different MIB:
  `{CTL_KERN, KERN_PROC_ARGS, pid, KERN_PROC_ARGV}`. OpenBSD returns an array of
  `char *` (NULL-terminated) followed by the strings; NetBSD returns NUL-separated
  strings.
- **PLT-153** I/O bytes are **absent** on the BSDs. `ki_rusage.ru_inblock` and
  `ru_oublock` count blocks, not bytes, and miss cached I/O. psutil returns those counts
  with bytes −1. ogle hides the R/s and W/s columns on the BSDs at every width
  (PROC-052) and reads nothing for them (PROC-026). The `io` sort key still works: absent
  values sort after every known value, in both directions (PROC-060). It does not fall
  back to name. See the decisions in section 19 (PLT-184).
- **PLT-154 OpenBSD**: `{CTL_KERN, KERN_PROC, KERN_PROC_ALL, 0, sizeof(struct
  kinfo_proc), count}`. Fields `p_pid`, `p_ppid`, `p_comm`, `p_uid`, `p_stat`, `p_nice`,
  `p_uutime_sec/usec`, `p_ustime_sec/usec`, `p_ustart_sec`, `p_vm_tsize + p_vm_dsize +
  p_vm_ssize` (vms in pages), `p_vm_rssize`, `p_cpuid`, `p_flag & P_SYSTEM`. Thread
  count comes from `KERN_PROC_SHOW_THREADS` and needs a second walk. Make it absent in
  phase 1. NetBSD: `{CTL_KERN, KERN_PROC2, KERN_PROC_ALL, 0, sizeof(struct
  kinfo_proc2), count}` with `struct kinfo_proc2`, using the same field names
  with a `p_` prefix; `p_nlwps` gives the thread count.

### 15.5 Windows

- **PLT-155** `NtQuerySystemInformation(SystemProcessInformation)` (C1, one call for all
  processes). Grow the buffer when the call returns `STATUS_INFO_LENGTH_MISMATCH`, and
  keep the buffer between cycles. Each `SYSTEM_PROCESS_INFORMATION` entry gives,
  with **no per-process handle**:

  | Entry field | Becomes |
  |---|---|
  | `UniqueProcessId` | `pid` |
  | `InheritedFromUniqueProcessId` | `ppid` |
  | `ImageName` | `name` |
  | `NumberOfThreads` | `num_threads` |
  | `UserTime`, `KernelTime` (100 ns) | `cpu_user`, `cpu_system` |
  | `CreateTime` (FILETIME) | `start_time` |
  | `WorkingSetSize` | `rss` |
  | `PagefileUsage` (private committed bytes, psutil's `vms`) | `vms` |
  | `BasePriority` | `priority_class` |
  | `HandleCount` | extended `handles` |
  | `ReadTransferCount`, `WriteTransferCount` | `io_read_bytes`, `io_write_bytes` |
  | trailing `SYSTEM_THREAD_INFORMATION[]` | status, plus load (PLT-072) |

  - `priority_class` from `BasePriority`: 4 Idle, 6 BelowNormal, 8 Normal,
    10 AboveNormal, 13 High, 24 Realtime. Other values map to the nearest class.
  - The I/O transfer counts include file, network and device I/O, not only disk.
    psutil uses the same values. The UI help says so.
  - This works for all processes without privileges, unlike psutil's per-process
    `OpenProcess` path. ogle MUST prefer it.
  - Microsoft documents only part of `SYSTEM_PROCESS_INFORMATION`. `CreateTime`,
    `UserTime`, `KernelTime` (inside `Reserved1[48]`), `ReadTransferCount` and
    `WriteTransferCount` (inside `Reserved7[6]`) and the `SYSTEM_THREAD_INFORMATION`
    records come from unofficial headers. psutil relies on the same layout.
- **PLT-156 Windows fields that need a handle (new identities only).** Open with
  `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`; it fails for protected processes,
  which leaves the field absent.
  - `cmdline`: `NtQueryInformationProcess(ProcessCommandLineInformation = 60)` on
    Windows 8.1+, giving a `UNICODE_STRING`. Split with `CommandLineToArgvW` rules.
  - `uid` / `username`: `OpenProcessToken(TOKEN_QUERY)`, then
    `GetTokenInformation(TokenUser)`, giving a SID, then `LookupAccountSidW`, cached per
    SID string (PLT-127). The display form is `DOMAIN\user`; `04-processes.md` decides
    any truncation. Services such as `SYSTEM` resolve without network access.
  - Close every handle immediately after use.
- **PLT-157** On Windows, `last_cpu` and `is_kernel_thread` are absent. `nice` is absent;
  `priority_class` is used instead. The System process (pid 4) and "Registry" /
  "Memory Compression" are normal entries.

### 15.6 Status letters

- **PLT-150** `status` is one ASCII letter. `04-processes.md` decides the colours and
  which letters count as "running" or "sleeping" in processcount.

  | Platform | Rule |
  |---|---|
  | Linux | `state` field of `/proc/<pid>/stat` verbatim: R S D Z T t X I P W K |
  | macOS | `p_stat`: SZOMB → Z, SSTOP → T, SIDL → I; else R if `pti_numrunning > 0`, else S. `p_stat` reports SRUN for almost every live process, so it cannot separate running from sleeping. If `PROC_PIDTASKINFO` is denied (other users), use S. |
  | FreeBSD | `ki_stat`: SRUN → R, SSLEEP → S, SSTOP → T, SZOMB → Z, SWAIT → W, SLOCK → L, SIDL → I |
  | OpenBSD/NetBSD | `p_stat`: SRUN/SONPROC → R, SSLEEP → S, SSTOP → T, SZOMB/SDEAD → Z, SIDL → I |
  | Windows | Any thread in state Running (2), Ready (1) or Standby (3) → R. All threads in Waiting (5) with `WaitReason == Suspended (5)` → T. Otherwise S. psutil reports every Windows process as "running"; ogle gives a useful distinction at no extra cost. |

### 15.7 Caches

- **PLT-125 cmdline cache.** cmdline and the derived `name` (Linux PLT-144) are read
  once per identity and kept until the identity disappears. Glances re-reads cmdline
  and username every 60 s (`processes.py:52`). ogle does not re-read on a timer, because
  argv rewrites are rare and cosmetic. The one exception: when the name reported by the
  per-cycle enumeration changes (on Linux the `comm` field of `stat`), as after an
  `exec` without a new PID, cmdline and the derived name are read again. The identity
  stays the same (PROC-023, PERF-022). F5 / Ctrl-R clears the cache
  (`01-runtime-ui.md`).
- **PLT-126 Cache eviction.** Entries for identities not seen in the latest enumeration
  are dropped at the end of the cycle, so memory stays proportional to the number of
  live processes.
- **PLT-127 Username cache.** UID → username (Unix, via `getpwuid_r`) and SID →
  name (Windows) are cached for the whole session. Entries are never evicted; the set of
  users is small. A failed lookup caches the numeric UID or SID string as the name. On
  Unix, NSS lookups may be slow (LDAP or SSSD). They happen once per UID.

### 15.8 PID limit

- **PLT-128 `pid_max`**
  - Linux: `/proc/sys/kernel/pid_max` (C0).
  - FreeBSD: sysctl `kern.pid_max`.
  - macOS: 99999 (constant `PID_MAX` in XNU `proc_internal.h`).
  - OpenBSD: 99999 (`PID_MAX`). NetBSD: 30000 (`PID_MAX` in `sys/proc.h`).
  - Windows: absent. The processlist sizes the PID column from the largest live PID.

## 16. Pinned-process extended data (`process_extended`)

`ProcExtended { cpu_affinity?: [int], ionice?: {class, value}, ctx_switches_voluntary?,
ctx_switches_involuntary?, num_fds?, num_handles?, swap_bytes?, tcp_count?,
udp_count? }`.

- Cost: C3.
- Called only for the single pinned process, at most once per cycle.
- The call MUST verify `start_time` first. If the identity no longer matches, it returns
  "gone" and the UI unpins.

| Field | Linux | macOS | FreeBSD | Windows |
|---|---|---|---|---|
| cpu_affinity | `sched_getaffinity(pid)` | absent | `cpuset_getaffinity(CPU_LEVEL_WHICH, CPU_WHICH_PID, pid)` | `GetProcessAffinityMask` (current group only); `GetProcessGroupAffinity` for multi-group |
| ionice | `ioprio_get(IOPRIO_WHO_PROCESS, pid)` raw syscall: class = v>>13 (none / realtime / best-effort / idle), value = v & 7 | absent | absent | absent in phase 1: PROC-122 shows IO nice on Linux only. A later phase MAY use `NtQueryInformationProcess(ProcessIoPriority = 33)` (very low / low / normal / high) |
| ctx switches | `/proc/<pid>/status` `voluntary_ctxt_switches` / `nonvoluntary_ctxt_switches` | `PROC_PIDTASKINFO.pti_csw` (total only, as voluntary; involuntary absent) | `ki_rusage.ru_nvcsw` / `ru_nivcsw` | Σ thread `ContextSwitches` from `SystemProcessInformation` (total only) |
| num_fds | count entries of `/proc/<pid>/fd` (same UID or `CAP_SYS_PTRACE`) | `proc_pidinfo(PROC_PIDLISTFDS)` size / `sizeof(proc_fdinfo)` (same UID or root) | `kinfo_getfile()` / sysctl `KERN_PROC_FILEDESC` count. The kernel requires `p_candebug` (`PGET_CANDEBUG`), so other users' processes fail unless root | absent |
| num_handles | absent | absent | absent | `HandleCount` (PLT-155) |
| swap_bytes | `/proc/<pid>/status` `VmSwap` (kB). Glances sums `smaps`, which is far more expensive; `VmSwap` gives the same total. | absent | absent | absent |
| shared_bytes | `/proc/<pid>/status` `RssShmem` (kB), read in the same read as `VmSwap`. Absent on kernels older than 4.5. | absent | absent | absent |
| tcp/udp count | collect socket inodes from `readlink(/proc/<pid>/fd/*)` (`socket:[N]`), then count matching inode columns in `/proc/net/tcp`, `tcp6`, `udp`, `udp6` | `PROC_PIDLISTFDS`, then `PROC_PIDFDSOCKETINFO` per socket fd; classify by `soi_kind` and protocol (same UID or root) | `kinfo_getfile()` socket entries; classify by `kf_sock_type` / `kf_sock_protocol` (same `p_candebug` rule as num_fds) | `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_ALL)` and `GetExtendedUdpTable(UDP_TABLE_OWNER_PID)` for AF_INET and AF_INET6; count rows whose `dwOwningPid` = pid |

- **PLT-160** On Linux, the reads for one pinned process are: `status` (one file,
  giving context switches and swap), the `fd/` directory, and the four `/proc/net`
  tables. The `/proc/net` tables can be large (tens of thousands of lines on busy
  servers). ogle SHOULD read the tcp/udp counts at most every 10 s, even when the cycle
  is faster.
- **PLT-161** Running min/max/mean CPU and memory for the pinned process are computed
  above the platform layer from the normal `Proc` samples. They need no extra reads.

## 17. Process actions

- **PLT-170 Identity check.** Both actions take `(pid, start_time)` and MUST re-read the
  start time immediately before acting. If it differs, the action is refused with
  "process no longer exists", which prevents killing a reused PID. Glances does not
  check this.
- **PLT-171 Signal / terminate.** `process_signal(pid, start_time, sig)` with
  `sig ∈ {terminate, kill}`.
  - Unix: `kill(pid, SIGTERM)` or `kill(pid, SIGKILL)`.
  - Windows: both values map to `OpenProcess(PROCESS_TERMINATE)` and
    `TerminateProcess(handle, 1)`.
  - Glances sends SIGKILL only. `04-processes.md` decides which signal the `k` key sends.
  - Errors are returned as `permission denied`, `no such process` or `other(code)`.
  - The action MUST NOT block waiting for the process to exit. Glances waits up to 3 s;
    ogle shows the result on the next cycle.
- **PLT-172 Set priority.** `process_set_priority(pid, start_time, delta)` with
  `delta = +1` (lower priority) or `-1`.
  - Unix: `setpriority(PRIO_PROCESS, pid, current_nice + delta)`, clamped to the range
    −20 to 19. Lowering nice (raising priority) needs privileges; `EACCES` or `EPERM`
    becomes `permission denied`.
  - Windows: step `priority_class` along Idle ↔ BelowNormal ↔ Normal ↔ AboveNormal ↔
    High with `OpenProcess(PROCESS_SET_INFORMATION)` and `SetPriorityClass`. ogle never
    steps into Realtime.
- **PLT-173** ogle MUST refuse both actions on its own PID, as Glances does.

## 18. Availability matrix

Legend: Y present; - absent; E emulated; B best effort (MAY be absent); P depends on
privilege (denied for other users' processes when unprivileged; the UI shows `?`,
PROC-011).

| Field | Linux | macOS | FreeBSD | OpenBSD/NetBSD | Windows |
|---|---|---|---|---|---|
| cpu user/system/idle | Y | Y | Y | Y | Y |
| cpu nice | Y | Y | Y | Y | - |
| cpu iowait | Y | - | - | - | - |
| cpu irq | Y | - | Y | Y | - |
| cpu softirq | Y | - | - | - | - |
| cpu steal, guest | Y | - | - | - | - |
| cpu dpc, interrupt | - | - | - | - | Y |
| ctx_switches | Y | - | Y | Y | Y |
| interrupts | Y | - | Y | Y | Y |
| soft_interrupts | Y | - | Y | Y | - |
| syscalls | - | - | Y | Y | Y |
| physical core count | Y | Y | Y (B) | - | Y |
| cpu freq current | Y (B in VMs) | Intel only | Y (B) | OpenBSD only | Y |
| cpu freq max | Y (B) | Intel Y, Apple Silicon B | Y (B) | - | Y |
| load average | Y | Y | Y | Y | E |
| mem active/inactive | Y | Y | Y | Y | - |
| mem buffers | Y | - | Y | OpenBSD Y, NetBSD - | - |
| mem cached | Y | Y | ≤11, or with ZFS | OpenBSD -, NetBSD Y | Y |
| mem shared | Y | - | - | - | - |
| mem wired | - | Y | Y | Y | - |
| swap in/out | Y | Y | Y | - | - |
| net speed | Y | Y | Y | Y | Y |
| disk read/write time | Y | Y | Y | - | Y |
| temperatures | Y | B | Y | OpenBSD Y, NetBSD - | - |
| fans | Y | B | - | OpenBSD Y, NetBSD - | - |
| battery | Y | Y | Y | OpenBSD Y, NetBSD - | Y |
| proc cpu/mem/threads | Y | P | Y | Y (threads -) | Y |
| proc cmdline | Y | P | Y | Y | P (protected processes) |
| proc username | Y | Y | Y | Y | P |
| proc io bytes | P | P | - | - | Y (includes non-disk I/O) |
| proc last_cpu | Y | - | Y | Y | - |
| proc kernel-thread flag | Y | - | Y | Y | - |
| proc nice | Y | Y | Y | Y | - (priority_class instead) |
| ext affinity | Y | - | Y | - | Y |
| ext ionice | Y | - | - | - | - |
| ext num_fds | P | P | P | - | - |
| ext num_handles | - | - | - | - | Y |
| ext swap | Y | - | - | - | - |
| ext tcp/udp counts | P | P | P | - | Y |

## 19. Decisions other files must match

- **PLT-180** `02-system-modules.md`: the cpu panel MUST lay out rows from the fields that
  are present. Examples: Windows has `dpc`/`interrupt` but no `nice`/`iowait`; macOS has
  no counter rows at all. Glances' fixed four-row layout with fallbacks
  (`cpu/__init__.py:296-374`) is the reference for slot order only.
- **PLT-181** `02-system-modules.md`: the load module on Windows shows emulated values
  (PLT-072). On every platform, load-per-core and quicklook `load` use
  `cpu_topology().logical`.
- **PLT-182** `03-io-modules.md`: network speed is in bits/s SI (×10^6). Disk entries
  carry `kind`. Windows disks are `PhysicalDriveN`, not volume letters. Mount filtering
  uses `is_physical`, `is_network` and `read_only` from this file.
- **PLT-183** `03-io-modules.md`: temperatures and fans are absent on Windows and NetBSD,
  and best effort on macOS. The sensors panel MUST hide empty groups. There is no
  hddtemp source; `drivetemp` covers it on Linux.
- **PLT-184** `04-processes.md`:
  - I/O cells show `?` when the value is denied (other users' processes on Linux and
    macOS, PROC-011).
  - On the BSDs I/O is always absent, so the R/s and W/s columns are hidden at every
    width (PROC-052). The `io` sort key MUST still work, with absent values sorting
    last in both directions (PROC-060).
  - The `NI` column on Windows shows the priority class label (as Glances does) from
    `priority_class`.
  - The CPU-core column is not hidden where `last_cpu` is absent. It follows the width
    rule for optional columns (PROC-051) and shows `-` in every row where `last_cpu` is
    absent (macOS, Windows; PROC-052).
  - Kernel-thread hiding uses `is_kernel_thread`. On platforms where it is absent, the
    option has no effect.
- **PLT-185** `04-processes.md`: processes are keyed by `(pid, start_time)`. cmdline and
  username are cached per identity without the Glances 60 s refresh; cmdline is re-read
  only when the process name changes (PLT-125). Kill and renice
  verify identity and never block.
- **PLT-186** `07-performance.md`: the per-cycle process cost is one `stat` read plus one
  `fstatat` per new identity on Linux, plus `io` only when requested. It is one
  `proc_pidinfo` per process on macOS, and a single syscall on FreeBSD and Windows.
  Everything in section 16 is C3 and limited to the pinned process.
