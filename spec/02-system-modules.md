# ogle specification: system modules

Status: draft, phase 1
Covers: system, uptime, now, cpu, percpu, load, mem, memswap, quicklook, plus the
shared value-formatting rules (auto-unit, percent, trend arrows) that the other section
files reference.

`00-overview.md` binds this file. Other files own these topics:

- Hotkeys, the layout engine, colours and the config-file search path: `01-runtime-ui.md`.
- Threshold semantics, decorations and events: `05-alerts.md`.
- Where each field comes from on each OS: `06-platform-sources.md`.

Glances references point into `/usr/lib/python3.14/site-packages/glances/`. They are
informative only.

---

## 1. Shared formatting rules

Every section file uses the definitions in this chapter. Other files cite them by ID
(for example "formatted with auto-unit, SYS-003").

### 1.1 Auto-unit

- **SYS-001** Auto-unit turns a non-negative quantity (bytes, bits, counts) into a short
  string such as `585M` or `4.94G`. It uses binary steps (1 K = 1024) and the symbols
  `K M G T P E Z Y`. The output never contains a space or the letter `B`.
- **SYS-002** Inputs: `n` (a number, or *absent*), `min_symbol` (default `K`) and
  `low_precision` (default false).
  1. If `n` is absent, return `-`.
  2. If `n == 0`, return `0`.
  3. Walk the symbols from the largest (`Y`) down to `min_symbol`. For each symbol
     compute `v = n / 1024^k`. Take the **first** symbol where `v >= 1`.
  4. If no symbol qualifies (n < 1024^k(min_symbol)), return `n` as an integer. Use 2
     decimals only if `n` is a non-integer value.
  5. Choose the decimals for the qualifying symbol:
     - `v < 9.995`: 2 decimals; else `v < 99.95`: 1 decimal; else 0 decimals.
     - Symbol `K`: always 0 decimals.
     - If `low_precision`: symbols `K` and `M` get 0 decimals; every other symbol gets
       at most 1 decimal.
  6. Format `v` with that many decimals and append the symbol.
  7. Carry rule: if rounding makes the printed number reach `1024` (for example
     `1023.7K` printed with 0 decimals), restart at step 5 with the next larger symbol.
     So the integer part never shows `1024` or more for a symbol that has a larger
     neighbour.
- **SYS-003** Results are right-aligned in a 5-character cell unless a panel says
  otherwise. The algorithm keeps results to 5 characters or fewer for every realistic
  input (`1023K`, `9.99G`, `99.9T`, `999P`). A longer result is drawn in full and is
  never truncated, because a truncated number is wrong.
- **SYS-004** Examples (normative test vectors):

  | Input | `low_precision=false` | `low_precision=true` |
  |---|---|---|
  | 0 | `0` | `0` |
  | 1000 | `1000` | `1000` |
  | 1024 | `1K` | `1K` |
  | 613421788 | `585M` | `585M` |
  | 1073741824 | `1.00G` | `1.0G` |
  | 1181116006 | `1.10G` | `1.1G` |
  | 5307033647 | `4.94G` | `4.9G` |
  | 44968414685 | `41.9G` | `41.9G` |
  | 838471403472 | `781G` | `781G` |
  | 9683209690677 | `8.81T` | `8.8T` |
  | 1048575 | `1.00M` (carry from `1024K`) | `1M` |

### 1.2 Percentages and absent values

- **SYS-005** Percentages are held as floating-point values from 0 to 100. Display
  formats are given per panel, in Python-style notation such as `{:5.1f}%`, meaning
  width 5 with 1 decimal followed by `%`. Clamp values to 0 or more before display.
  Only per-core and quicklook values may exceed 100, and only where a panel says so.
- **SYS-006** An *absent* field (GEN-005) shows nothing: its label and value cells are
  replaced by blank space of the same width, so columns stay aligned. An *unknown*
  value (a field that exists but has no value yet, such as a rate on the first sample)
  shows `-` right-aligned in the value cell.

### 1.3 Labelled stat cells

- **SYS-007** Most panels are grids of *stat cells*. A stat cell is
  `<prefix><label padded to L><value right-aligned in V>`:
  - `prefix` is `""` for the first column of a panel and two spaces (`"  "`) for
    later columns.
  - `L` and `V` are given per panel.
  - The value is a percentage (`{:5.1f}%`, V = 6) or an auto-unit string (V = 5).
  - The label cell uses the panel's default style. Only the value cell carries the
    threshold decoration (`05-alerts.md`).

### 1.4 Trend arrows

- **SYS-008** A trend is computed for a single history series (see GEN-008):
  1. If the series holds fewer than 30 samples, the trend is *none*.
  2. Otherwise `trend = mean(newest 15) - mean(oldest 15)` over the newest 30 samples.
- **SYS-009** The trend glyph is:
  - a space if the trend is none
  - `↑` if trend > 1
  - `↓` if trend < -1
  - `-` otherwise

  The ASCII fallbacks (GEN-017) are `^` and `v`. The threshold `1` is in the series'
  own unit (percentage points for mem and swap, load units for load).
- **SYS-010** Trend series are recorded once per module refresh. They are not recorded
  per redraw. The ring buffer holds exactly 30 entries.

---

## 2. Data sharing between modules

- **SYS-011** cpu, percpu and quicklook MUST share one CPU sample per cycle. Glances
  takes separate, separately cached samples (`cpu_percent.py:222-340`). ogle reads the
  CPU counters at most once per cycle and derives all three modules' values from that
  one sample pair.
- **SYS-012** quicklook reads its `mem`, `swap` and `load` values from the same samples
  the mem, memswap and load modules use. A data source is collected when **at least one
  enabled consumer** needs it. Example: with the mem panel hidden (key `4`) and
  quicklook showing a `mem` bar, memory is still sampled. When no enabled consumer
  needs a source, it is not collected (GEN-004).
- **SYS-013** The logical and physical core counts are read once at startup and cached.
  They are re-read only when an explicit refresh (`F5`/Ctrl-R, see `01-runtime-ui.md`)
  is requested. Glances' `core` module never displays anything, so ogle has no `core`
  module. The counts are a platform helper.

---

## 3. CPU accounting rule

- **SYS-014** For each sample pair (whole machine or one core), each CPU time field is
  shown as a percentage:
  `field% = Δfield / Δtotal × 100`, where `Δtotal` is the sum of the deltas of all time
  fields the platform reports. On Linux, `guest` and `guest_nice` are excluded from
  `Δtotal` because the kernel already counts them in `user` and `nice`.
- **SYS-015** Busy percentage (`total`) is `100 - idle% - iowait%`. **iowait counts as
  idle.** This one rule applies to the cpu panel total, every per-core `total`, quicklook
  bars and the percpu ordering. On platforms with no iowait, `total = 100 - idle%`.
  (Glances quirk: its cpu total treats iowait as idle, but its per-core total
  `100 - idle` treats iowait as busy. ogle uses one rule.)
- **SYS-016** All percentages are clamped to `[0, 100]`. If `Δtotal == 0` (two samples
  in the same tick), the module keeps its previous percentages.

---

## 4. Module: system

Header-line module showing hostname and OS description.

- **SYS-020** Fields:
  - `hostname`
  - `os_name`: `Linux`, `macOS`, `FreeBSD`, `OpenBSD`, `NetBSD` or `Windows`
  - `os_version`: kernel release on Linux and BSD, product version on macOS (for
    example `14.5`), `<release> <build>` on Windows
  - `arch`: the machine architecture string of the OS (not of the ogle binary), for
    example `x86_64` or `aarch64` (`06-platform-sources.md`, `arch`). Glances shows
    `64bit`/`32bit` instead.
  - `linux_distro` (Linux only; may be absent): `NAME` + `" "` + `VERSION_ID` from
    os-release, quotes stripped (the platform field `distro`)
- **SYS-021** Rendering, on one line:
  1. `hostname` in TITLE style.
  2. A space, then the formatted description, then a trailing space. The description is
     an *optional* item: the layout engine drops it when the header line is too narrow
     (`01-runtime-ui.md`).
- **SYS-022** The description comes from the `[system] info_format` template. Its
  placeholders are `{hostname} {os_name} {os_version} {arch} {linux_distro}`.
  - Default on Linux when `linux_distro` is present:
    `{linux_distro} {arch} / {os_name} {os_version}`. Example:
    `Arch Linux x86_64 / Linux 6.17.8-arch1-1`.
  - Default elsewhere: `{os_name} {os_version} {arch}`.
  - A template that names an absent or unknown placeholder falls back to the default for
    that platform. ogle prints one stderr warning at startup.
- **SYS-023** Refresh: once at startup, then every `[system] refresh` seconds (default
  60). No thresholds.

```toml
[system]
# info_format = "{linux_distro} {arch} / {os_name} {os_version}"
refresh = 60
```

---

## 5. Module: uptime

- **SYS-030** Field: `boot_time`, the wall-clock boot instant. Read once at startup and
  on explicit refresh. Each cycle, uptime = `now - boot_time`. This costs no system call
  beyond the clock read. (On Windows the platform may supply uptime directly. See
  `06-platform-sources.md`.)
- **SYS-031** Rendering: `Uptime: ` followed by the duration, right-aligned on the header
  line. Duration format:
  - under one day: `H:MM:SS` (hours not zero-padded), for example `5:03:12`
  - one day: `1 day, H:MM:SS`
  - more than one day: `N days, H:MM:SS`

  Fractional seconds are dropped.
- **SYS-032** Refresh: every cycle. No thresholds. No config keys.

---

## 6. Module: now (clock)

- **SYS-040** Field: the current local time, taken at the start of each redraw.
- **SYS-041** Format:
  - The `[now] format` strftime string, or the `--strftime` CLI option (the CLI option
    wins).
  - Default: `%Y-%m-%d %H:%M:%S %Z`. If the platform's zone abbreviation is longer than
    6 characters (for example a full Windows zone name), use `%z` in place of `%Z`.
- **SYS-042** Rendering: one line, padded to 23 characters, bottom-aligned at the end of
  the left sidebar (`01-runtime-ui.md` owns placement).
- **SYS-043** Refresh: every redraw. Hotkey `N` toggles the module (`01-runtime-ui.md`).

```toml
[now]
# format = "%Y-%m-%d %H:%M:%S %Z"
```

---

## 7. Module: cpu

Shows whole-machine CPU use. It is hidden while percpu mode is on (SYS-061).

### 7.1 Fields

- **SYS-050** Percentage fields from SYS-014 and SYS-015. Each may be absent per
  platform (see `06-platform-sources.md` for the matrix):

  | Field | Label | Typical platforms |
  |---|---|---|
  | `total` | (title value) | all |
  | `user` | `user` | all |
  | `system` | `system` | all |
  | `idle` | `idle` | all |
  | `nice` | `nice` | Unix |
  | `iowait` | `iowait` | Linux |
  | `irq` | `irq` | Linux, BSD; Windows "interrupt time" |
  | `steal` | `steal` | Linux |
  | `guest` | `guest` | Linux |
  | `dpc` | `dpc` | Windows |

  `softirq` and `guest_nice` are collected on Linux because they are needed for `Δtotal`.
  They are not displayed.
- **SYS-051** Rate fields: per-second rates of cumulative counters, computed per GEN-007.

  | Field | Label | Notes |
  |---|---|---|
  | `ctx_switches` | `ctx_sw` | all platforms that report it |
  | `interrupts` | `inter` | |
  | `soft_interrupts` | `sw_int` | absent on Windows and wherever the OS reports none |
  | `syscalls` | `sys_call` | absent on Linux and macOS, which do not report it; present on BSD and Windows |

  A counter the platform does not provide is **absent**. It is never shown as a
  constant 0. (Glances quirk: it always shows `syscalls`, even on Linux where psutil
  returns 0, because of the always-true condition `not LINUX or not MACOS`.)

### 7.2 Rendering

- **SYS-052** The panel shows only the fields the platform has (GEN-005,
  `06-platform-sources.md` PLT-180). It has up to 3 columns of stat cells (SYS-007) and
  at most 4 lines. Each column has a slot list in priority order. A column shows the
  first 4 fields of its list that are present on the running platform, top to bottom,
  with no gaps. An absent field takes no line. A column with no present field is not
  drawn and takes no width. The panel height is the length of the longest shown column.

  | Column | Width | Slot list, in priority order |
  |---|---|---|
  | 1 | 14: label L = 8, percentage V = 6 | `CPU` title + `total`, `user`, `system`, `iowait`, `dpc` |
  | 2 | 15: prefix 2, L = 7, V = 6 | `idle`, `irq`, `nice`, `steal` |
  | 3 | 15: prefix 2, L = 8, V = 5 | `ctx_sw`, `inter`, `sw_int`, `guest`, `sys_call` |

  - Line 1 of column 1 is always the `CPU` title (TITLE style, padded to 8) followed by
    `total` as `{:5.1f}%`. `total` is present on every platform.
  - The `idle` cell uses format `{:4.1f}%` with L = 8, so it is also 15 characters wide.
  - Column 3 rates are per second, formatted with auto-unit (SYS-002,
    `min_symbol=K`). `guest`, the only percentage in column 3, uses `{:4.1f}%` so the
    cell stays 15 characters wide.
  - Resulting layouts (informative, from the availability matrix in `06-`):

    | Platform | Column 1 | Column 2 | Column 3 |
    |---|---|---|---|
    | Linux | total, user, system, iowait | idle, irq, nice, steal | ctx_sw, inter, sw_int, guest |
    | macOS | total, user, system | idle, nice | (not drawn) |
    | FreeBSD, OpenBSD, NetBSD | total, user, system | idle, irq, nice | ctx_sw, inter, sw_int, sys_call |
    | Windows | total, user, system, dpc | idle, irq | ctx_sw, inter, sys_call |

  (Glances quirk: Glances uses a fixed 4 × 3 grid with fallbacks and repeats `ctx_sw` on
  line 3 when `sw_int` is missing. ogle compacts each column instead.)
- **SYS-053** Optional cells, which the layout engine may drop when the top row is
  narrow (`01-runtime-ui.md`): every cell in columns 2 and 3. Column 1 (`total`, `user`,
  `system`, `iowait`/`dpc`) is never dropped. With the optional cells dropped, the panel
  is 14 characters wide and keeps the height from SYS-052.
- **SYS-054** Rates show `-` until two samples exist.

### 7.3 Thresholds

- **SYS-055** Evaluated fields. Semantics are in `05-alerts.md`; maximum = 100 unless
  stated.

  | Stat name | Value | Careful / Warning / Critical | Logged (event type) |
  |---|---|---|---|
  | `cpu.total` | `total` | 65 / 75 / 85 | yes (`CPU_TOTAL`) |
  | `cpu.user` | `user` | 50 / 70 / 90 | no |
  | `cpu.system` | `system` | 50 / 70 / 90 | no |
  | `cpu.iowait` | `iowait` | 50 / 70 / 90 | yes (`CPU_IOWAIT`) |
  | `cpu.steal` | `steal` | 50 / 70 / 90 | no |
  | `cpu.ctx_switches` | `ctx_switches` rate ÷ logical cores (switches per second per core) | 40000 / 45000 / 50000 | no |

  All other fields use the DEFAULT style unless the user configures limits for them
  (for example `nice`, see ALR-027). A field that is absent on the platform is not
  evaluated.
- **SYS-056** The `ctx_switches` decoration is computed only after a rate exists.
  (Glances quirk: Glances compares the per-refresh delta divided by cores with limits
  that scale with the core count, so the result depends on both the refresh interval
  and the core count. ogle compares a per-second, per-core rate with fixed limits.)

### 7.4 Config and refresh

- **SYS-057** Refresh: the global interval, unless `[cpu] refresh` is set.
- **SYS-058** History: `user` and `system` series exist only if sparklines need them.
  The cpu panel shows no trend arrow.

```toml
[cpu]
# refresh = 2

[cpu.thresholds]
total        = { careful = 65, warning = 75, critical = 85, log = true }
user         = { careful = 50, warning = 70, critical = 90, log = false }
system       = { careful = 50, warning = 70, critical = 90, log = false }
iowait       = { careful = 50, warning = 70, critical = 90, log = true }
steal        = { careful = 50, warning = 70, critical = 90 }
ctx_switches = { careful = 40000, warning = 45000, critical = 50000 }
```

Every threshold block in this file uses the `[<module>.thresholds]` layout from
`05-alerts.md` (ALR-011). `05-alerts.md` also defines lookup and fallback (ALR-012).

---

## 8. Module: percpu

Per-core breakdown. Shown only in percpu mode.

- **SYS-060** Fields per logical core: `cpu_number` plus the percentage fields of
  SYS-050 (`total`, `user`, `system`, `idle`, `nice`, `iowait`, `irq`, `steal`, `guest`,
  `dpc`). `total` follows SYS-015.
- **SYS-061** Percpu mode is toggled by hotkey `1` and the `--percpu` CLI option
  (`01-runtime-ui.md`). Default off.
  - When percpu mode is on: percpu is visible, cpu is hidden, and the quicklook `cpu`
    bar becomes per-core bars (SYS-093).
  - When percpu mode is off: percpu is hidden and not collected per core. The
    whole-machine counters are enough for cpu and quicklook.
- **SYS-062** Columns by platform, in this order:

  | Platform | Columns |
  |---|---|
  | Linux | user, system, iowait, idle, irq, nice, steal, guest |
  | macOS | user, system, idle, nice |
  | BSD | user, system, idle, irq, nice |
  | Windows | user, system, dpc, irq |

  A column whose field is absent on the running machine is omitted entirely, header
  included. (On Windows, Glances labels the interrupt column `interrupt`. ogle uses
  `irq` everywhere.)
- **SYS-063** Core selection and ordering, shared with quicklook (SYS-093):
  - Let `N = [percpu] max_cpu_display` (default 4).
  - If the core count is N or less, show every core in core-number order.
  - Otherwise, sort the cores by `total` descending (ties by core number ascending)
    and show the first N. Add one summary row, `CPU*`, that holds the mean of each
    column over the cores that are not shown.
  - Compute the ordering once per cycle. percpu and quicklook MUST use the same
    ordering, so their rows line up.
- **SYS-064** Rendering when quicklook is **enabled** (rows sit to the right of
  quicklook's per-core bars):
  - Line 1: each column name right-aligned in 7 characters (`{:>7}`).
  - One line per shown core, then the `CPU*` line if needed: each cell `{:6.1f}%`
    (7 characters). No row labels; quicklook provides them.
- **SYS-065** Rendering when quicklook is **disabled**:
  - Line 1: `CPU` title (TITLE style, padded to 5), then `total` and the platform
    columns, each `{:>7}`.
  - Core lines: a 5-character label, then the cells. The label is `CPU<n> ` for n < 10
    and `{n:>4} ` otherwise (for example `  12 `).
  - Summary line label: `CPU* `.
  - An unknown cell shows `     ?%`.
- **SYS-066** Thresholds: each cell is evaluated as stat `percpu.<column>` with
  maximum 100. Never logged.

  | Column | Careful / Warning / Critical |
  |---|---|
  | `user` | 50 / 70 / 90 |
  | `system` | 50 / 70 / 90 |
  | `iowait` | 50 / 70 / 90 |
  | other columns | none (DEFAULT style) |

  The `CPU*` summary cells are evaluated the same way.
- **SYS-067** Refresh: same as cpu (one shared sample, SYS-011).

```toml
[percpu]
max_cpu_display = 4

[percpu.thresholds]
user   = { careful = 50, warning = 70, critical = 90 }
system = { careful = 50, warning = 70, critical = 90 }
iowait = { careful = 50, warning = 70, critical = 90 }
```

---

## 9. Module: load

- **SYS-070** Fields: `min1`, `min5`, `min15` (load averages) and `cores` (the logical
  core count, SYS-013).
  - The module is absent on platforms with no load average. Every phase-1 platform has
    one.
  - On Windows, load is always emulated as exponentially damped averages of the
    number of ready and running threads (`06-platform-sources.md`, PLT-072). The panel
    shows the emulated values exactly like real ones and adds no title marker.
- **SYS-071** Rendering, 4 lines, 13 characters wide:
  1. `LOAD` (TITLE), then a space, the trend glyph for `min1` (SYS-009, 1 character),
     then `{:>3}core` (for example `  8core`). The core count is omitted when unknown.
  2. `1 min` padded to 7, then the value.
  3. `5 min` padded to 7, then the value.
  4. `15 min` padded to 7, then the value.

  Value formats:
  - Normal mode: `{:>6.2f}`.
  - Per-core percent mode: `load ÷ cores × 100`, formatted `{:>5.1f}%`.
- **SYS-072** Per-core percent mode follows hotkey `0` (per-core normalisation, see
  UI-043 in `01-runtime-ui.md` and PROC-044 in `04-processes.md`). `0` affects both the
  process CPU% column and load-as-percent. There is no separate key.
- **SYS-073** Thresholds: stat name `load`, current value `min15`, maximum = the logical
  core count (SYS-013). So value% = `min15 ÷ cores × 100`, the load per core as a
  percentage, and the limits are percent-of-maximum like every other stat (ALR-043).

  | Row | Evaluated | Logged |
  |---|---|---|
  | `min15` | yes; drives the module's threshold state | yes (event type `LOAD`, event value = value%) |
  | `min5` | decoration only (pure evaluation against the same limits and maximum) | no; MUST NOT change the `load` threshold state used by the global alert message |
  | `min1` | no (DEFAULT style) | no |

  Defaults: careful 70, warning 100, critical 500 (that is 0.7, 1.0 and 5.0 per core).
  Both display modes (SYS-071) use the same evaluation. (Glances quirk: `min5` is
  evaluated after `min15` and overwrites the global `load` threshold state, so the
  "System overloaded" message follows `min5`. ogle uses `min15` only.)
- **SYS-074** History: a 30-sample `min1` series for the trend arrow.
- **SYS-075** Refresh: the global interval, unless `[load] refresh` is set.

```toml
[load]
# refresh = 2

[load.thresholds]       # percent of the core count: 70 = 0.7 load per core
careful = 70
warning = 100
critical = 500
log = true
```

---

## 10. Module: mem

### 10.1 Fields

- **SYS-080** Fields (bytes unless stated). Each may be absent per platform except
  `total`, `available`, `used`, `free` and `percent`:

  | Field | Label | Meaning |
  |---|---|---|
  | `total` | `total` | physical RAM |
  | `available` | `avail` | memory that can be given to processes without swapping |
  | `used` | `used` | `max(0, total - available)` on every platform |
  | `free` | `free` | completely unused memory |
  | `percent` | (title value) | `used / total × 100`, clamped to [0, 100] |
  | `active` | `active` | Unix only (absent on Windows) |
  | `inactive` | `inacti` | Unix only (absent on Windows) |
  | `buffers` | `buffer` | Linux, FreeBSD, OpenBSD |
  | `cached` | `cached` | Linux, macOS, Windows, NetBSD, FreeBSD 11 and older; includes ZFS ARC size where present |
  | `wired` | `wired` | macOS, BSD |

- **SYS-081** ZFS adjustment (Linux and FreeBSD with ZFS loaded):
  - `cached += arc_size`
  - `available += arc_size - arc_min`
  - Recompute `used` and `percent` afterwards.

  (Glances does this at `plugins/mem/__init__.py:180-210`.)

### 10.2 Rendering

- **SYS-082** 4 lines, 2 columns, 29 characters wide:
  - Column 1 is 13 wide (L = 8, V = 5).
  - Column 2 is 16 wide (prefix 2, L = 9, V = 5).
  - Values use auto-unit (SYS-002, `min_symbol=K`).

  | Line | Column 1 | Column 2 |
  |---|---|---|
  | 1 | `MEM` (TITLE) + space + trend glyph padded to 2 + `percent` as `{:>6.1f}%` | `active` |
  | 2 | `total` | `inacti` |
  | 3 | `used`, or `avail` when `[mem] show_available = true` | `buffer`, or `wired` if buffers is absent |
  | 4 | `free` | `cached` |

  Absent fields leave blank slots (SYS-006).
- **SYS-083** Optional cells (droppable when the top row is narrow): all of column 2.

### 10.3 Thresholds, history, refresh

- **SYS-084** Stat `mem`, value `used`, maximum `total` (so the percentage is
  `percent`). Defaults 50 / 70 / 90. Logged by default, event type `MEM`. The decoration
  applies to the `percent` value on line 1.
- **SYS-085** History: a 30-sample `percent` series for the trend arrow, plus the
  sparkline series if quicklook needs it.
- **SYS-086** Refresh: the global interval, unless `[mem] refresh` is set.

```toml
[mem]
show_available = false

[mem.thresholds]
careful = 50
warning = 70
critical = 90
log = true
```

---

## 11. Module: memswap

- **SYS-087** Fields: `total`, `used`, `free` (bytes) and `percent`
  (`used / total × 100`). `sin` and `sout` (pages swapped in and out) are **not**
  collected in phase 1 because nothing displays them.
  - If the platform reports no swap device or zero total swap, the panel still shows,
    with `total 0`, and `percent` shows `-`.
  - If the platform cannot report swap at all, the module is absent.
- **SYS-088** Rendering, 4 lines, 13 characters wide:
  1. `SWAP` (TITLE), a space, the trend glyph padded to 2, then `percent` as
     `{:>5.1f}%`.
  2. `total` stat cell (L = 8, V = 5, auto-unit).
  3. `used` stat cell.
  4. `free` stat cell.
- **SYS-089** Thresholds: stat `memswap`, value `used`, maximum `total`. Defaults
  50 / 70 / 90. Logged by default, event type `MEMSWAP`. Not evaluated when
  `total == 0`.
- **SYS-090** History: a 30-sample `percent` series for the trend arrow. Refresh: the
  global interval, unless `[memswap] refresh` is set.

```toml
[memswap.thresholds]
careful = 50
warning = 70
critical = 90
log = true
```

---

## 12. Module: quicklook

The leftmost panel of the top row. It shows the CPU model and frequency line, then one
gauge row per configured metric. A gauge is either a bar or a sparkline.

### 12.1 Fields

- **SYS-091** Fields:
  - `cpu_name`: the CPU model string. Read once at startup.
  - `freq_current`, `freq_max`: frequencies in Hz.
    - `freq_current` is the mean of the per-core current frequencies.
    - `freq_max` is read once at startup.
    - Either may be absent. On Apple Silicon `freq_current` is always absent and
      `freq_max` is best effort: it comes from undocumented IOKit properties and may be
      absent (PLT-042).
    - `freq_current` is sampled only while quicklook is visible, at most once per cycle.
  - The gauge values, each a percentage:
    - `cpu`: the shared CPU total (SYS-015).
    - `mem`: mem `percent`.
    - `swap`: memswap `percent`.
    - `load`: `min15 ÷ cores × 100`. May exceed 100.
    - Per-core `total` values in percpu mode.
- **SYS-092** `[quicklook] list` sets which gauges appear, in order. Allowed entries:
  `cpu`, `mem`, `swap`, `load`. Default `["cpu", "mem", "load"]`.
  - An unknown entry is dropped with a stderr warning at startup. The other entries are
    kept. (Glances quirk: one unknown entry makes Glances fall back to the whole
    default list.)
  - An entry whose source is absent on the platform (for example `swap` where the
    platform cannot report swap) is skipped silently.

### 12.2 Layout

The layout engine gives quicklook a width `W` (`01-runtime-ui.md`; Glances caps it at
43 columns unless full-quicklook mode is on). Every line is exactly `W` characters wide.

- **SYS-093** Line 1, the CPU line:
  - The frequency text is ` {cur:.2f}/{max:.2f}GHz` when both frequencies are known,
    ` {cur:.2f}GHz` when only the current one is, else empty.
  - The frequency text is right-aligned. `cpu_name` fills the space on its left.
  - If `cpu_name` does not fit, truncate it and end it with `…` (ASCII fallback `~`).
    (Glances quirk: it pads the name but never truncates it, so a long name overflows
    the panel.)
  - If `W` cannot hold even the frequency text, show only the truncated name.
- **SYS-094** Gauge rows, one per entry in `list`:
  `<label padded to 5>[<gauge><value>]`
  - The label is the entry name in upper case, padded to 5: `CPU  `, `MEM  `, `SWAP `,
    `LOAD `.
  - The gauge area is `G = W - 13` characters (5 label + `[` + 6 value + `]` = 13).
  - The value is 6 characters: `{:5.1f}%` below 100, `  100%` at exactly 100, and
    ` >100%` above 100.
  - `[` and `]` use BOLD style.
  - The gauge and the value use the entry's threshold decoration (SYS-097).
  - If `G < 1`, the gauge rows show only the label and the value, without brackets.
- **SYS-095** Bar gauge (the default):
  - Let `fill = G × min(pct, 100) / 100`.
  - Draw `floor(fill)` copies of `bar_char`.
  - If the fractional part of `fill` is 0.5 or more, add one more `bar_char`.
  - Pad with spaces to `G`.
  - `[quicklook] bar_char` defaults to `|`; any single-column glyph is allowed (Glances'
    shipped config uses `▪`).
  - (Glances rounds the fractional cell via `int(frac*8) >= 5`, which is the same as
    frac ≥ 0.625. ogle uses plain rounding at 0.5.)
- **SYS-096** Sparkline gauge:
  - Toggled by hotkey `S` and the `--sparkline` option (`01-runtime-ui.md`). Default off.
  - Needs a UTF-8 terminal. In ASCII mode (GEN-017) the toggle is refused and the UI
    shows a short message in the status area (UI-114) instead.
  - Each gauge shows the newest `G` samples of that metric's history, oldest on the
    left and right-aligned, one glyph per sample.
  - Glyph for value v (clamped to 0–100): `" ▁▂▃▄▅▆▇█"[round(v / 100 × 8)]`. An empty
    history slot is a space.
  - The value cell shows the newest sample.
  - The history depth is G samples per gauge (GEN-008). The ring buffers are allocated
    when sparklines are first enabled or W changes, and freed when sparklines are
    turned off.
- **SYS-097** Thresholds (decoration only, never logged): stat `quicklook.<entry>`,
  maximum 100.

  | Entry | Careful / Warning / Critical |
  |---|---|
  | `cpu` | 50 / 70 / 90 |
  | `mem` | 50 / 70 / 90 |
  | `swap` | 50 / 70 / 90 |
  | `load` | 70 / 100 / 500 (value% = `min15 ÷ cores × 100`, as SYS-073) |

  These evaluations MUST NOT create events or change the threshold state that
  `05-alerts.md` uses for global messages.

### 12.3 Per-core mode

- **SYS-098** In percpu mode (SYS-061), the `cpu` entry is replaced by one gauge row per
  shown core, then the `CPU*` summary row if needed, using the ordering from SYS-063.
  - Row labels: `CPU<n> ` for n < 10, else `{n:>4} `. The summary label is `CPU* `.
  - Each core row and the summary row use the same `G` as the other gauges.
  - Decoration uses the `quicklook.cpu` limits, without events.
  - In sparkline mode, each shown core's sparkline draws that core's own history.
  - The `CPU*` sparkline draws, for each past sample, the mean of the cores that were
    not shown **in that sample**. ogle records this value as its own series each
    cycle. (Glances quirk: its summary bar averages the cores outside the top N, but its
    summary sparkline averages cores whose *number* is N or more, which is a different
    set. Its summary bar is also a different width from the other rows.)
  - Per-core histories are kept only while percpu mode and sparkline mode are both on.

### 12.4 Modes and config

- **SYS-099** Hotkeys (`01-runtime-ui.md` owns the table):

  | Key | Effect |
  |---|---|
  | `3` | toggles quicklook |
  | `4` | toggles full-quicklook mode: quicklook is shown with no width cap; cpu (or percpu), mem and memswap are hidden; load stays (UI-044) |
  | `5` | toggles the whole top row |
  | `S` | toggles sparklines |
  | `1` | toggles percpu mode |

- **SYS-100** Refresh: every cycle. Quicklook has no sampling of its own except
  `freq_current`. It reuses the other modules' samples (SYS-011, SYS-012).

```toml
[quicklook]
list = ["cpu", "mem", "load"]
bar_char = "|"

[quicklook.thresholds]
cpu  = { careful = 50, warning = 70, critical = 90 }
mem  = { careful = 50, warning = 70, critical = 90 }
swap = { careful = 50, warning = 70, critical = 90 }
load = { careful = 70, warning = 100, critical = 500 }
```

---

## 13. Panel summary

Widths below are without the optional cells. The layout engine (`01-runtime-ui.md`) adds
inter-panel spacing.

| Module | Lines | Full width | Narrow width | Default refresh |
|---|---|---|---|---|
| system | 1 | variable | hostname only | 60 s |
| uptime | 1 | variable | same | every cycle |
| now | 1 | 23 | 23 | every redraw |
| quicklook | 1 + gauges | W (≤ 43, no cap in full mode) | W | every cycle |
| cpu | up to 4 (SYS-052) | up to 44 (SYS-052) | 14 | global |
| percpu | 1 + N + (1) | 5 + 7 × columns | same | global |
| load | 4 | 13 | 13 | global |
| mem | 4 | 29 | 13 | global |
| memswap | 4 | 13 | 13 | global |

---

## 14. Glances quirks fixed in this file

| Glances behaviour | ogle rule |
|---|---|
| `auto_unit` uses `value > 1`, so 1 GiB prints `1024M` | `>= 1` plus the carry rule (SYS-002) |
| cpu total counts iowait as idle, percpu total counts it as busy | iowait is idle everywhere (SYS-015) |
| `syscalls` shown on Linux as a constant 0 (always-true `not LINUX or not MACOS`) | absent where the OS does not report it (SYS-051) |
| cpu panel: fixed grid with fallbacks; line 3 repeats `ctx_sw` when `sw_int` is missing | only present fields shown, each column compacted (SYS-052) |
| ctx-switch alert compares a per-refresh delta with core-scaled limits | per-second, per-core rate against fixed limits (SYS-056) |
| `min5` overwrites the `load` threshold state set by `min15` | only `min15` drives state and events (SYS-073) |
| quicklook CPU name padded but never truncated | truncated with `…` (SYS-093) |
| one unknown `quicklook list` entry discards the whole list | only the unknown entry is dropped (SYS-092) |
| `CPU*` sparkline averages a different set of cores from the `CPU*` bar | the same "not shown this sample" set (SYS-098) |
| separate cached CPU samples per module (2 s cache, decoupled from refresh) | one shared sample per cycle (SYS-011) |
| 1200–28800-sample histories kept for every field | 30-sample trend rings, plus sparkline rings only when enabled (SYS-010, SYS-096) |
| rates use floor division and show 0 on the first sample | float rates, `-` until known (GEN-007, SYS-054) |
| Windows percpu column labelled `interrupt` | `irq` everywhere (SYS-062) |
