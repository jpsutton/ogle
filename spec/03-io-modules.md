# ogle specification: I/O modules (network, diskio, fs, sensors)

Status: draft, phase 1
Binding decisions: `00-overview.md`. Related: `01-runtime-ui.md` (layout, hotkeys, CLI,
config loading), `02-system-modules.md` (auto-unit format, trend arrows),
`05-alerts.md` (threshold evaluation, decorations, events), `06-platform-sources.md`
(where each value comes from on each OS), `07-performance.md` (budgets).

This file defines four modules that live in the left sidebar: `network`, `diskio`, `fs`
and `sensors`. For each module it defines the fields, filters, panel rendering, display
modes, thresholds, config keys and refresh defaults.

Glances reference: `glances/plugins/{network,diskio,fs,sensors}/__init__.py`,
`glances/plugins/plugin/model.py` (filters, rates), `/etc/glances/glances.conf`.

---

## 1. Common rules for sidebar I/O modules

### 1.1 Panel geometry

- **IO-001** The layout engine (`01-runtime-ui.md`) gives each sidebar module a panel
  width `P` (in Glances terms the sidebar width plus 2). Every row a module draws MUST be
  exactly `P` cells wide or shorter. A module MUST NOT draw past `P`.
- **IO-002** Every panel in this file uses one of two row shapes:
  - *Two-column shape* (network in normal mode, diskio, fs): a name column `N = P - 14`
    cells wide, then two value columns of 7 cells each, right-aligned.
  - *One-column shape* (network in sum mode, sensors): a name column `N = P - 14` cells
    wide, then one value column of 14 cells, right-aligned.
- **IO-003** The first row of every panel is the header row. It holds the panel title in
  the `TITLE` style, left-aligned and padded to `N`, then the column headers, right-aligned
  in their columns and drawn in the default style.
- **IO-004** A displayed name MUST fit in `N - 1` cells, so at least one space always
  separates the name from the first value. Each module defines its own truncation rule
  (keep the head or keep the tail). The truncation marker is `_`, so the result,
  marker included, is at most `N - 1` cells. Measure width in terminal cells, not bytes
  or code points (East Asian wide characters count as 2).
- **IO-005** A panel with no rows to show (every item filtered out, or the platform
  reports none) MUST NOT be drawn, header included, and MUST NOT take vertical space.
  Glances does the same.
- **IO-006** The height a panel may use is decided by the layout engine. When rows do not
  fit, the panel shows as many whole rows as fit, in its sort order, and drops the rest.
  No scroll indicator in phase 1.

### 1.2 Values and units

- **IO-007** Byte and count values use the auto-unit format defined in
  `02-system-modules.md` (SYS-001 to SYS-004: binary 1024 steps, symbols
  `K M G T P E Z Y`, 0 to 2 decimals).
  This file says "auto-unit" for that format and "auto-unit, low precision" for its
  low-precision variant.
- **IO-008** Rates follow GEN-007: `(now - prev) / elapsed_seconds` as a float, using the
  monotonic clock. On the first sample of an item, or when a counter goes backwards, the
  rate is unknown and the cell shows `-` in the default style. ogle MUST still draw the
  row; Glances hides it.
- **IO-009** A value the platform cannot supply (absent, GEN-005) shows as `-`. A value
  the process may not read (permission) shows as `?` (GEN-014).

### 1.3 Filters: hide, show, alias

These rules apply to every module in this file. They mirror Glances
`model.py:1050-1106` with ogle's TOML syntax.

- **IO-010** `hide` is a list of regular expressions. An item is hidden when **any**
  pattern matches the item's match key in full (anchored at both ends) and
  case-insensitively.
- **IO-011** `show` is a list of regular expressions with the same matching rule. When
  `show` is non-empty, only matching items are shown and `hide` is ignored.
- **IO-012** Each module defines its *match key*: interface name for network, disk name
  for diskio, mount point **or** device for fs (either may match), lower-cased label for
  sensors. Patterns also match against the item's alias when it has one.
- **IO-013** An invalid regular expression in `hide` or `show` is reported as one warning
  line on stderr before the UI starts. That pattern is ignored and the others still apply.
- **IO-014** Filtering MUST happen before any per-item work that costs a system call
  (for example `statvfs` for fs, or reading sensor files). A hidden item costs at most
  the enumeration needed to learn its name.
- **IO-015** `alias` is a TOML table that maps an item's real name to a display name, for
  example `alias = { wlp0s20f3 = "WIFI" }`. Lookup is case-insensitive on the real name.
  The alias replaces the name in the panel, in sorting and in event text. `hide`/`show`
  match both the real name and the alias (IO-012). Threshold lookups try the alias first
  and then the real name, with exact, case-sensitive matching (ALR-013).
- **IO-016** Name sorting uses *natural order*: split the name into digit and non-digit
  runs, compare digit runs as integers and other runs as case-insensitive text. So `eth2`
  comes before `eth10`, and `sda` before `sdb`.

### 1.4 hide_zero

- **IO-017** `hide_zero` (bool, default `false`) and `hide_threshold_bytes` (integer bytes
  per second, default `0`) are available on network and diskio. When `hide_zero` is true,
  each of an item's watched rates starts hidden. A watched rate becomes visible the first
  time it is greater than `hide_threshold_bytes`, and stays visible for the rest of the
  ogle session (latched). The item's row is hidden while **all** its watched rates are
  still hidden.
- **IO-018** Watched rates: network `rx` and `tx` byte rates; diskio read and write byte
  rates.

### 1.5 Thresholds

- **IO-019** The evaluation (careful/warning/critical, percent of maximum, `_LOG`
  decorations, events) is defined in `05-alerts.md`. This file only says which value is
  evaluated, against which maximum, which limits apply and in what order.
- **IO-020** Limits use the `[<module>.thresholds]` layout of `05-alerts.md` (ALR-011,
  ALR-012). Each limit set is an inline table
  `{ careful = <num>, warning = <num>, critical = <num>, log = <bool> }`. Every key is
  optional. A missing level never triggers. `log` defaults to `false` for every
  threshold in this file. Glances does not log any of these stats by default either.
  The module-wide set (plain keys of `[<module>.thresholds]`) applies to every field of
  the module. diskio and sensors have fields in different units, so their limits SHOULD
  be set per field.
- **IO-021** Per-item limits are item sets, `items."<name>" = { ... }`, and item + field
  sets, `items."<name>".<field> = { ... }`, inside `[<module>.thresholds]`. `<name>` is the
  item's real name or its alias (ALR-013). Quote it when it holds characters TOML bare
  keys do not allow. These replace Glances' flat `<item>_<stat>_<level>` keys
  (`wlan0_rx_careful`, `/_careful`, `dm-0_rx_latency_warning`). ogle has no
  `[<module>.items]` tables.
- **IO-022** A rate cell whose rate is unknown (IO-008) is not evaluated and draws in the
  default style.

### 1.6 Refresh

- **IO-023** Each module has a `refresh` key in seconds. When it is absent, the module
  uses the global refresh interval or its own default from GEN-006. Defaults:
  network = global (2 s), diskio = global (2 s), fs = 60 s, sensors = 10 s.
- **IO-024** A module with `enabled = false`, or one toggled off with its hotkey, MUST NOT
  collect (GEN-004). When it is toggled back on, its first sample has unknown rates
  (IO-008). The previous counters are thrown away, not reused.

---

## 2. network

### 2.1 Fields (per interface)

| Field | Meaning |
|---|---|
| `name` | Kernel interface name |
| `alias` | Display name from config, optional |
| `rx_bytes`, `tx_bytes` | Lifetime byte counters (monotonic, 64-bit where the OS allows) |
| `rx_rate`, `tx_rate` | Bytes/s from the counters (IO-008) |
| `all_rate` | `rx_rate + tx_rate` |
| `is_up` | Link is up and running |
| `has_ip` | Interface has at least one IPv4 or IPv6 address |
| `speed` | Link speed in bits/s. Absent when unknown; a platform value of 0 means unknown and is reported as absent (PLT-001) |

- **IO-025** ogle MUST convert the platform's link speed to bits per second with SI
  scaling: Mbit/s × 1 000 000. Glances multiplies by 1 048 576, which is a bug.
- **IO-026** `is_up`, `has_ip` and `speed` change rarely. ogle MAY refresh them at most
  every 10 s, and MUST refresh them at once when an interface appears or disappears.
  Byte counters are read every network cycle.

### 2.2 Filters

- **IO-027** Match key: interface name (and alias, IO-012). Compiled-in default
  `hide = ["lo", "docker.*", "veth.*"]`. Glances ships `docker.*,lo`. ogle adds `veth.*`
  because per-container veth pairs fill the panel.
- **IO-028** `hide_no_up` (bool, default `true`): hide interfaces whose `is_up` is false.
  When `hide_no_up` is false, down interfaces are shown with their normal values.
  (Glances also hides down interfaces at display time whatever this setting says. ogle
  does not.)
- **IO-029** `hide_no_ip` (bool, default `true`): hide interfaces with no IPv4 or IPv6
  address. A link-layer address alone does not count.
- **IO-030** `hide_zero` and `hide_threshold_bytes` follow IO-017.

### 2.3 Panel

- **IO-031** Title `NETWORK`. Rows sorted by display name in natural order (IO-016).
- **IO-032** Name truncation keeps the **tail**. If the display name is longer than
  `N - 1`, show `_` followed by its last `N - 2` cells. Interface names differ mostly at
  the end, for example `enp0s31f6`.
- **IO-033** Display modes and column headers (two-column shape unless noted):

| Mode | Headers | Cells |
|---|---|---|
| Rate (default) | `Rx/s`, `Tx/s` | `rx_rate`, `tx_rate` |
| Rate + sum | `Rx+Tx/s` (one 14-cell column) | `all_rate` |
| Cumulative | `Rx`, `Tx` | `rx_bytes`, `tx_bytes` (lifetime totals) |
| Cumulative + sum | `Rx+Tx` (one 14-cell column) | `rx_bytes + tx_bytes` |

- **IO-034** Unit mode: bits by default. Each value is multiplied by 8, shown with
  auto-unit and given the suffix `b` (for example `12.3Mb`). In bytes mode the value is
  shown with auto-unit and no suffix (`1.54M`). Bits/bytes applies to every network mode.
- **IO-035** Cumulative mode MUST show the lifetime counters as the OS reports them,
  normally since boot or since the interface was created. Glances shows the change since
  the last refresh in this mode, which is a bug.
- **IO-036** Hotkeys (owned by `01-runtime-ui.md`): `b` toggles bits/bytes, `T` toggles
  sum, `U` toggles cumulative, `n` toggles the module. CLI and config may set each start
  state: config keys `[network] bytes`, `sum` and `cumulative`, all default `false`.

### 2.4 Thresholds

- **IO-037** Evaluate `rx_rate × 8` and `tx_rate × 8` (bits/s) separately. The rx result
  decorates the Rx cell and the tx result decorates the Tx cell. Thresholds do not apply
  in sum mode or cumulative mode. Those cells use the default style.
- **IO-038** Stats `network[<name>].rx` and `network[<name>].tx`. The ALR-012 lookup is
  split into two groups (ALR-015):
  1. Absolute limits in bits/s: the item + field set `items."<name>".rx` / `.tx` and the
     item set `items."<name>"` (lookup steps 1 and 2). Maximum = 100, so value% equals
     the raw bits/s value, as in Glances.
  2. Percent of link speed: the field sets `rx` / `tx`, the module-wide set and the
     compiled-in default `{ careful = 70, warning = 80, critical = 90 }` for both rx and
     tx (steps 3 to 5). Maximum = `speed`, so value% = `bits_per_s × 100 / speed`. This
     group is used only when `speed` is known and non-zero.

  If group 1 defines at least one of `careful`, `warning` or `critical`, only group 1
  supplies those levels. Otherwise group 2 does. If neither applies, the cell is not
  evaluated and uses the default style. `log` and `hysteresis` follow the plain ALR-012
  lookup across all steps.
- **IO-039** Each direction is evaluated on its own. A zero rate in one direction is a
  real measurement and is still evaluated. Glances 4.5.9 also does this, after fixing an
  earlier bug.

### 2.5 Config (TOML)

```toml
[network]
enabled = true
refresh = 2              # seconds; default: global refresh
hide = ["lo", "docker.*", "veth.*"]
show = []
hide_no_up = true
hide_no_ip = true
hide_zero = false
hide_threshold_bytes = 0
bytes = false            # start in bytes mode instead of bits
sum = false
cumulative = false
alias = { wlp0s20f3 = "WIFI" }

[network.thresholds]
rx = { careful = 70, warning = 80, critical = 90 }   # % of link speed
tx = { careful = 70, warning = 80, critical = 90 }
items."wlan0".rx = { careful = 4_000_000, warning = 5_000_000, critical = 6_000_000, log = true }  # bits/s
items."wlan0".tx = { careful = 700_000, warning = 900_000, critical = 1_000_000 }
```

---

## 3. diskio

### 3.1 Fields (per block device)

| Field | Meaning |
|---|---|
| `name` | Device name as the OS reports it (`sda`, `nvme0n1p2`, `dm-0`, `disk0`, `PhysicalDrive0`) |
| `alias` | Display name from config, optional |
| `read_bytes`, `write_bytes` | Lifetime byte counters |
| `read_count`, `write_count` | Lifetime completed-operation counters |
| `read_time_ms`, `write_time_ms` | Lifetime milliseconds spent on reads/writes, where available |
| `read_rate`, `write_rate` | Bytes/s |
| `read_iops`, `write_iops` | Operations/s |
| `read_latency`, `write_latency` | Mean ms per operation over the last interval |

- **IO-040** Latency is `Δread_time_ms / Δread_count` over the last interval (writes the
  same way), computed from raw deltas in floating point. When `Δcount` is 0 (no
  operations in the interval) the latency is 0. When the platform does not provide busy
  time, latency is absent (`-`). Glances divides two floor-rounded per-second rates and
  truncates to an integer, so sub-millisecond latencies show as 0. ogle MUST NOT do
  that.
- **IO-041** Both whole disks and partitions are listed when the platform reports both
  (Linux does, like Glances). Use `hide`/`show` to choose.

### 3.2 Filters

- **IO-042** Match key: device name (and alias). Compiled-in default
  `hide = ["loop.*", "/dev/loop.*"]`, as Glances ships.
- **IO-043** RAM disks: devices whose name starts with `ram` are hidden unless
  `show_ramfs = true` (config) or the matching CLI option is given (`01-runtime-ui.md`;
  Glances `--diskio-show-ramfs`). This rule is applied before `hide`/`show`.
- **IO-044** `hide_zero` and `hide_threshold_bytes` follow IO-017.

### 3.3 Panel

- **IO-045** Title `DISK I/O`. Rows sorted by display name in natural order.
- **IO-046** Name truncation keeps the **head**. If the display name is longer than
  `N - 1`, show its first `N - 2` cells followed by `_`.
- **IO-047** Display modes (two-column shape):

| Mode | Headers | Cells | Format |
|---|---|---|---|
| Throughput (default) | `R/s`, `W/s` | `read_rate`, `write_rate` | auto-unit, no suffix (bytes) |
| IOPS | `IOR/s`, `IOW/s` | `read_iops`, `write_iops` | auto-unit |
| Latency | `ms/opR`, `ms/opW` | `read_latency`, `write_latency` | `0.0` to `9.9` with one decimal, `10` and above as integers, then auto-unit low precision from 1000 up |

- **IO-048** The network bits/bytes toggle does not affect diskio. Diskio throughput is
  always in bytes.
- **IO-049** Hotkeys (owned by `01-runtime-ui.md`): `B` toggles IOPS mode, `L` toggles
  latency mode, `d` toggles the module. IOPS and latency exclude each other. Turning one
  on turns the other off. Turning the active mode off returns to throughput. Start
  state: config key `[diskio] mode = "throughput" | "iops" | "latency"`, default
  `"throughput"`. The CLI options `--diskio-iops` and `--diskio-latency`
  (`01-runtime-ui.md`) override `mode`. Giving both is a usage error.

### 3.4 Thresholds

- **IO-050** Throughput mode: evaluate stats `diskio[<name>].read` and
  `diskio[<name>].write` (bytes/s) as absolute values, maximum = 100 (raw bytes/s).
  There is no compiled-in default, because a disk's speed is unknown. Users normally set
  them per disk, `items."<name>".read = { ... }`. With no limits found by ALR-012, the
  cells use the default style.
- **IO-051** Latency mode: evaluate stats `diskio[<name>].read_latency` and
  `diskio[<name>].write_latency` (ms), maximum = 100 (raw ms). Limits come from the
  ALR-012 lookup. The compiled-in default for both fields is
  `{ careful = 10, warning = 20, critical = 50 }`.
- **IO-052** IOPS mode: no thresholds. Cells use the default style, as in Glances.
- **IO-053** Glances names these limits `rx`/`tx`. ogle uses `read`/`write`, because rx
  and tx mean little for disks.

### 3.5 Config (TOML)

```toml
[diskio]
enabled = true
refresh = 2
hide = ["loop.*", "/dev/loop.*"]
show = []
show_ramfs = false
hide_zero = false
hide_threshold_bytes = 0
mode = "throughput"
alias = { sda1 = "SystemDisk" }

[diskio.thresholds]
read_latency  = { careful = 10, warning = 20, critical = 50 }   # ms
write_latency = { careful = 10, warning = 20, critical = 50 }
items."dm-0".read  = { careful = 4_000_000_000, warning = 5_000_000_000, critical = 6_000_000_000 }  # bytes/s
items."dm-0".write = { careful = 700_000_000, warning = 900_000_000, critical = 1_000_000_000 }
items."dm-0".read_latency = { warning = 30, critical = 100 }
```

---

## 4. fs

### 4.1 Fields (per mounted file system)

| Field | Meaning |
|---|---|
| `mount_point` | Mount path. On Linux, decode octal escapes such as `\040`. Non-breaking spaces become plain spaces (Glances issue #1065) |
| `device` | Device or source string as the mount table reports it |
| `fs_type` | File system type string |
| `options` | Mount options. Only the read-only flag is used |
| `alias` | Display name from config, optional |
| `total` | Total size in bytes |
| `free_all` | Free bytes, including blocks reserved for root |
| `avail` | Bytes available to unprivileged users |
| `used` | `total - free_all` |
| `percent` | `(total - avail) × 100 / total`, the value used for thresholds (IO-061) |

### 4.2 Which file systems are listed

- **IO-054** By default ogle lists only *physical* file systems: those backed by a local
  block device or an equivalent storage pool. Each platform defines this predicate in
  `06-platform-sources.md`. On Linux it is the Glances/psutil rule: the type appears in
  `/proc/filesystems` without the `nodev` flag, plus `zfs`. This excludes `tmpfs`, `proc`,
  `sysfs`, `cgroup`, `overlay`, network file systems and FUSE by default.
- **IO-055** `allow` (list of strings, default empty) adds non-physical mounts whose
  `fs_type` contains any listed string as a substring, as Glances does. For example
  `allow = ["nfs"]` adds `nfs` and `nfs4`. ogle reads the full mount table only when
  `allow` is non-empty.
- **IO-056** When the same mount point appears more than once (stacked mounts), list it
  once, using the entry mounted last (the visible one). Distinct mount points on the same
  device (bind mounts, btrfs subvolumes) are each listed.
- **IO-057** Match key for `hide`/`show`: the mount point **or** the device. Either may
  match (Glances `is_display_any`). Compiled-in default
  `hide = ["/boot.*", ".*/snap.*"]`, as Glances ships.
- **IO-058** Usage queries (`statvfs` or the platform equivalent) MUST NOT block the main
  loop. Run a query that may block (any non-physical type added through `allow`, which
  includes network and FUSE file systems) off the UI thread with a 2 s timeout, as
  Glances does. A mount whose query timed out or failed keeps its row, with `?` in the
  value cells, and is retried on the next fs cycle. Only one query per mount may be in
  flight at a time.
- **IO-059** Mounts with `total = 0` (for example some pseudo file systems added through
  `allow`) are skipped.

### 4.3 Panel

- **IO-060** Title `FILE SYS`. Headers `Used` (or `Free`) and `Total`. Rows sorted by
  **real mount point** in plain code-point order, so `/` comes first. This matches
  Glances, which sorts on `mnt_point`, not on the alias.
- **IO-061** Label: the alias, or else the mount point. If
  `len(label) + len(basename(device)) <= N - 4`, append ` (<basename(device)>)`, for
  example `/home (nvme0n1p3)`. Otherwise, if the label is longer than `N - 1`, keep the
  head: the first `N - 2` cells followed by `_`.
- **IO-062** Cells: `used` (or `avail` in free mode) and `total`, both auto-unit.
- **IO-063** Hotkeys (owned by `01-runtime-ui.md`): `F` toggles used/free, `f` toggles the
  module. Start state: config `[fs] free_space = false`, or the CLI option (Glances
  `--fs-free-space`). Either one turns free mode on.

### 4.4 Thresholds

- **IO-064** Evaluate stat `fs[<mount point>]` for each mount: current = `total - avail`,
  maximum = `total`, so value% = `percent` (table 4.1). The stat has no field. The result
  decorates the Used/Free cell in both modes. Limits come from the ALR-012 lookup:
  per-mount item set `items."<mount point>" = { ... }`, then the module-wide set. The
  compiled-in default is `careful = 50, warning = 70, critical = 90`.
- **IO-065** `percent` uses `total - avail`, so it reaches 100 % exactly when unprivileged
  writes start to fail. Root-reserved blocks count as used. This is the Glances formula
  (`(size - free) / size`, where psutil's `free` is the unprivileged figure). It is
  deliberately not `df`'s `used / (used + avail)`.
- **IO-066** Read-only mounts (the `ro` option, or the platform equivalent) are not
  evaluated and draw in the default style (Glances issue #3143).

### 4.5 Config (TOML)

```toml
[fs]
enabled = true
refresh = 60
hide = ["/boot.*", ".*/snap.*"]
show = []
allow = []               # e.g. ["nfs", "cifs", "fuse.sshfs"]
free_space = false
alias = { "/" = "root" }

[fs.thresholds]          # percent of the file system size
careful = 50
warning = 70
critical = 90
items."/" = { careful = 70, warning = 85, critical = 95 }
```

---

## 5. sensors

Phase 1 covers three sensor kinds: temperature, fan and battery. Hard-disk temperature
through the hddtemp daemon is out of scope (GEN-015: no sockets). Drive temperatures the
OS exposes as ordinary hardware-monitor sensors (for example the Linux `drivetemp` or
`nvme` hwmon drivers) are plain temperature sensors and are in scope.

### 5.1 Fields (per sensor)

| Field | Meaning |
|---|---|
| `kind` | `temperature`, `fan` or `battery` |
| `label` | Display label (IO-067) |
| `value` | Temperature in °C, fan speed in RPM, battery charge in % |
| `high` | Hardware "high" temperature limit in °C, if the platform reports one |
| `critical` | Hardware "critical" temperature limit in °C, if the platform reports one |
| `status` | Battery only: `charging`, `discharging`, `full` or `unknown` |

- **IO-067** Label rules:
  - Use the platform's sensor label (for example the hwmon `temp1_label`, `Package id 0`).
  - With no label, use `<chip name> <index>`, for example `nvme 1`.
  - When two sensors end up with the same label, give the second and later ones the
    suffix ` 2`, ` 3`, ... in discovery order (Glances does this too).
  - Batteries use the platform's battery name (for example `BAT0`), or `Battery` when
    there is none.
- **IO-068** Sensor values are numbers. A sensor whose reading fails or is not numeric is
  dropped from that cycle's list. Glances compares such readings with bytes literals,
  which never match because the values are strings, so they are dropped silently there
  too. ogle drops them on purpose and documents it.
- **IO-069** On systems with no battery, or when the platform reports no battery state,
  no battery rows appear.

### 5.2 Filters and order

- **IO-070** Match key: the label in lower case (and the alias). Compiled-in default
  `hide = ["unknown.*"]`, as Glances ships.
- **IO-071** Alias lookup tries the label first, then `<label>_<kind>`, both
  case-insensitive. Example: `alias = { "package id 0" = "CPU", "core 0" = "CPU core 0" }`.
- **IO-072** Rows are grouped by kind in the order temperature, fan, battery. Within a
  kind they are sorted by display label in natural order (IO-016).
- **IO-073** With the default `sidebar_order` (UI-013), the sensors panel is the last
  data panel in the sidebar before the clock. Its height is limited by the space above
  the clock (`01-runtime-ui.md`, UI-064).

### 5.3 Panel

- **IO-074** Title `SENSORS`, with no column headers. One-column shape: the label is cut
  to `N - 1` cells (head kept, **no** `_` marker, as in Glances). The value is
  right-aligned in 14 cells.
- **IO-075** Value format, as an integer with no decimals followed by the unit:
  - Temperature: `<v>C`, or `<v>F` in Fahrenheit mode, where
    `F = C × 1.8 + 32`, rounded to the nearest integer after conversion.
  - Fan: `<v>RPM`.
  - Battery: `<v>%` followed by a status glyph: `↑` charging, `↓` discharging,
    `✓` full, nothing for unknown or not charging (the platform state `not_charging`,
    `06-platform-sources.md`). ASCII mode (GEN-017) uses `^`, `v` and `=`. Glances
    has no ASCII fallback for full (it prints nothing). ogle uses `=`.
- **IO-076** Fahrenheit mode is set by config `[sensors] fahrenheit = true` or the CLI
  option (Glances `--fahrenheit`). Phase 1 has no hotkey for it, as in Glances. It
  affects temperature display only. Thresholds are always evaluated in °C (IO-078).

### 5.4 Thresholds

- **IO-077** Temperature: stat `sensors[<label>].temperature`, maximum = 100, so value%
  is the raw °C value. Limits are chosen in this order:
  1. The ALR-012 lookup, steps 1 to 4: per-sensor `items."<label>".temperature`, then
     `items."<label>"`, then the field set `temperature = { ... }`, then the module-wide
     set. `<label>` matches as ALR-013 says (alias first, then the real label; exact and
     case-sensitive).
  2. Step 5 has no compiled-in default for temperature. Glances' shipped config sets
     45/65/80, which overrides the hardware limits. ogle deliberately leaves it unset so
     the hardware's own limits apply by default.
  3. If step 1 defines none of `careful`, `warning` and `critical`, use the hardware
     limits: `value >= critical` → CRITICAL, `value >= high` → WARNING, else OK. There
     is no CAREFUL level. With no hardware `critical`, the result is DEFAULT. With a
     `critical` but no `high`, values below `critical` are DEFAULT. This copies Glances
     `__get_system_thresholds`.
- **IO-078** Thresholds compare °C values even in Fahrenheit mode. Limits in config are
  always written in °C.
- **IO-079** Battery: stat `sensors[<name>].battery`. Evaluate `100 - value` (the
  depleted share), maximum = 100. The compiled-in default for the `battery` field is
  `{ careful = 70, warning = 80, critical = 90 }`. So 30 % charge is CAREFUL, 20 %
  WARNING and 10 % CRITICAL. A battery that is charging or full is still evaluated, as
  in Glances.
- **IO-080** Fan: stat `sensors[<label>].fan`, raw RPM, maximum = 100. Limits come from
  the ALR-012 lookup (for example `fan = { ... }` or `items."<label>".fan = { ... }`).
  There is no compiled-in default. Higher RPM is worse, as in Glances. A stopped fan
  (0 RPM) is a valid reading and is evaluated.
- **IO-081** The decoration applies to the value cell only.

### 5.5 Config (TOML)

```toml
[sensors]
enabled = true
refresh = 10
hide = ["unknown.*"]
show = []
fahrenheit = false
alias = { "package id 0" = "CPU", "composite" = "NVMe" }

[sensors.thresholds]
# temperature = { careful = 45, warning = 65, critical = 80 }   # unset: hardware limits
battery = { careful = 70, warning = 80, critical = 90 }          # applies to 100 - charge
# fan = { careful = 4000, warning = 5000, critical = 6000 }       # RPM
items."Package id 0".temperature = { careful = 70, warning = 85, critical = 95, log = true }
```

- **IO-082** Hotkey `s` toggles the module (`01-runtime-ui.md`).
- **IO-083** Reading sensors can be slow on some hardware (I²C and ACPI sources). Read them
  only on the module's own cycle (default 10 s). Discover the set of sensor sources at
  start-up and when the set may have changed (hot-plug), not every cycle.

---

## 6. Glances quirks fixed

| Glances 4.5.9 behaviour | ogle behaviour | ID |
|---|---|---|
| Link speed in Mbit/s × 1 048 576 (`network/__init__.py:199`), so percent-of-speed alerts fire about 5 % late | × 1 000 000 | IO-025 |
| "Cumulative" mode (`U`) shows the bytes moved since the last refresh, because `_manage_rate` overwrites the counters with deltas | Lifetime counters | IO-035 |
| Down interfaces are hidden at display time even with `hide_no_up=False` | `hide_no_up` has full control | IO-028 |
| A new interface, or one with no rate yet, is hidden until its second sample | Row shown with `-` until the rate is known | IO-008 |
| Rates use floor division (`delta // seconds`), and a counter of 0 gives rate 0 | Float division; first sample or counter going backwards gives unknown | IO-008, GEN-007 |
| Disk latency = `int(floor_rate(time) / floor_rate(count))`, so sub-ms values show 0 | Float ratio of raw deltas, one decimal below 10 ms | IO-040, IO-047 |
| hddtemp placeholder values (`ERR`, `SLP`, ...) compared as bytes against str values, so the branch is dead code and the rows vanish silently | hddtemp out of scope; non-numeric readings dropped on purpose | IO-068 |
| Shipped config sets type-wide temperature limits 45/65/80 that override hardware limits | No type-wide default; hardware limits used unless the user sets them | IO-077 |
| Battery full glyph has no ASCII fallback (empty string) | `=` | IO-075 |
| Flat config keys such as `wlan0_rx_careful`, `/_careful`, `dm-0_rx_latency_log` | Item and item + field sets in `[<module>.thresholds]` | IO-021 |
| Disk limits named `rx`/`tx` | `read`/`write` | IO-053 |

## 7. Requirements other files must match

- `01-runtime-ui.md`: hotkeys `n b T U` (network), `d B L` (diskio), `f F` (fs),
  `s` (sensors); CLI options for bytes mode, diskio RAM disks, fs free space and
  Fahrenheit; the sidebar width rule that gives `P`; and that the sensors panel height
  is limited by the clock below it.
- `02-system-modules.md`: the auto-unit format and its low-precision variant.
- `05-alerts.md`: the `[<module>.thresholds]` layout with inline limit sets
  `{ careful, warning, critical, log }`, item sets `items."<name>"`, the per-level lookup
  (ALR-011, ALR-012), the network and sensors fallbacks (ALR-015), and "maximum = 100
  means compare raw values".
- `06-platform-sources.md`: a per-platform "physical file system" predicate (IO-054),
  64-bit network counters where possible, link speed units, disk busy-time
  availability, sensor high/critical limits and battery status.
