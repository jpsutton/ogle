# ogle specification: thresholds, decorations and alerts

Status: draft, phase 1
Requirement ID prefix: `ALR-`

This file is the single source of truth for:

- threshold evaluation;
- decoration names;
- the event list;
- the alert panel.

Module files (`02-`, `03-`, `04-`) state only three things for each evaluated field:

- the current value;
- the maximum the value is compared against;
- the default limits.

Colour values for each decoration live in `01-runtime-ui.md`.

Glances references point into `/usr/lib/python3.14/site-packages/glances` (Glances 4.5.9). They are informative only.

## 1. Concepts

- **Stat**: one evaluated quantity. It is identified by a *stat name* built from `module`, an optional `item` and an optional `field`. Examples: `cpu.total`, `load` (module only), `fs[/]`, `network[wlan0].rx`.
- **Value%**: the value used in every comparison, `value% = current * 100 / maximum`. When a module gives no maximum, the maximum is 100, so `value% = current`.
- **State**: one of `DEFAULT`, `OK`, `CAREFUL`, `WARNING`, `CRITICAL`. They are ordered DEFAULT < OK < CAREFUL < WARNING < CRITICAL for display, but DEFAULT means "not evaluated".
- **Limits**: up to three numbers, `careful`, `warning` and `critical`, in value% units. Any of them may be absent.
- **Log flag**: a boolean for each stat. When it is true, the stat feeds the event list and its decoration gets the `_LOG` variant.
- **Event**: one record in the event list that describes a period when a logged stat was at WARNING or CRITICAL.

## 2. Threshold evaluation

- **ALR-001** Evaluation is a pure function:
  `evaluate(current, maximum = 100, minimum = 0, highlight_zero = true, limits) -> state`.
  It has no side effects. Section 3 defines registration, which is the only thing that has side effects.
- **ALR-002** Evaluation order:
  1. If `current` is absent or not numeric, or `maximum` is absent, zero or not numeric, return `DEFAULT`.
  2. If `highlight_zero` is false and `current == 0`, return `DEFAULT`.
  3. Compute `value% = current * 100 / maximum` in floating point.
  4. If `critical` is present and `value% >= critical`, the state is `CRITICAL`.
  5. Otherwise, if `warning` is present and `value% >= warning`, the state is `WARNING`.
  6. Otherwise, if `careful` is present and `value% >= careful`, the state is `CAREFUL`.
  7. Otherwise, if none of the three limits is present, the state is `DEFAULT`.
  8. Otherwise the state is `OK`.
  9. Minimum rule: if `current < minimum`, the state becomes `CAREFUL`. This overrides steps 4 to 8, but not steps 1, 2 or 7.

  All comparisons use `>=`. They are inclusive, as in Glances (`plugins/plugin/model.py:826-911`).
- **ALR-003** Limits do not have to be ordered. Evaluation checks critical first, then warning, then careful, whatever the numbers are. ogle MUST NOT reject or reorder limits. At startup, ogle SHOULD print one warning line on stderr for each limit set where `careful > warning` or `warning > critical`.
- **ALR-004** Hysteresis is optional. By default `hysteresis = 0`, which reproduces Glances exactly: each evaluation stands alone and has no memory. When a stat's effective hysteresis `h > 0`, ogle keeps that stat's last registered state. A state only moves *down* when `value%` is below the lower bound of the previous state minus `h`. For example, take careful=50, warning=70, critical=90 and h=5. A stat at WARNING stays WARNING until `value% < 65`. Moving up is not affected. Hysteresis applies only to registered stats (section 3), because pure evaluations have no memory. Hysteresis is in value% units.
- **ALR-005** Rate fields whose rate is still unknown (GEN-007: first sample, or a counter that went backwards) evaluate to `DEFAULT` and are not registered.

## 3. Registration

- **ALR-006** A module *registers* a stat when the stat's result should affect global state. In phase 1 every evaluated stat is registered unless its module file says otherwise. Registering a stat does three things:
  1. It evaluates the stat, applying hysteresis if configured.
  2. It stores the resulting state in the **threshold state table**, keyed by stat name. Section 7 uses this table.
  3. If the stat's log flag is true and the state is not `DEFAULT`, it passes `(type, state, value%)` to the event engine (section 5).
- **ALR-007** Some evaluations exist only to colour a cell. Examples are the per-core quicklook bars and the per-CPU cells, which reuse the `quicklook.cpu` or `percpu.*` limits. These use pure evaluation and are not registered. This matches Glances' `_decoration_for_percent`. Module files mark such fields as "decoration only".
- **ALR-008** Event type names are the stat name in upper case, with item and field joined by `_`. Examples: `CPU_TOTAL`, `CPU_IOWAIT`, `LOAD`, `MEM`, `MEMSWAP`, `NETWORK_WLAN0_RX`, `FS_/`. Characters that are not printable ASCII are kept unchanged. Truncating long names for display is a rendering concern only (section 8).
- **ALR-009** Quirk fix: Glances calls the event engine on every update for every logged stat, even in the OK state (`events_list.py:245`), and rebuilds the global message each time. ogle MUST produce the same visible results, but SHOULD skip the event engine when the state is OK or CAREFUL and that type has no ongoing event. In that case the call would do nothing anyway.
- **ALR-010** Quirk fix (load): in Glances the `load` module evaluates `min15` (logged) and then `min5` (not logged). Both write to the same threshold key, so `min5` overwrites the `min15` state (`plugins/load/__init__.py:127-142`). In ogle:
  - `load.min15` is the registered and logged stat. Its stat name for the threshold table is `load`.
  - `load.min5` is decorated with a pure evaluation against the same limits and is not registered.
  - `load.min1` is not evaluated.

## 4. Limits and log flags in the configuration file

- **ALR-011** Limits live in one `thresholds` sub-table of each module's TOML table, `[<module>.thresholds]`. That table holds every level for the module:

  - **Module-wide set**: the plain keys of `[<module>.thresholds]` itself (stat `module`).
  - **Field set**: `<field> = { ... }`, an inline table (stat `module.field`).
  - **Item set**: `items."<item>" = { ... }`, an inline table (stat `module[item]`).
  - **Item + field set**: a `<field> = { ... }` inline table inside the item's inline table (stat `module[item].field`). The dotted form `items."<item>".<field> = { ... }` is the same TOML and may be used when the item has no item-level keys.

  ```toml
  [mem.thresholds]            # module-wide limits (stat "mem")
  careful = 50
  warning = 70
  critical = 90
  log = true

  [cpu.thresholds]            # field-level limits (stats "cpu.total", "cpu.iowait")
  total  = { careful = 65, warning = 75, critical = 85, log = true }
  iowait = { careful = 50, warning = 70, critical = 90, log = true }

  [fs.thresholds]
  careful = 50                # module-wide limits for every mount
  warning = 70
  critical = 90
  items."/" = { careful = 80, warning = 90, critical = 95 }       # item-level (stat "fs[/]")

  [network.thresholds]
  rx = { careful = 70, warning = 80, critical = 90 }              # field-level (stat "network.rx")
  items."wlan0" = { rx = { careful = 50_000_000, log = true } }   # item + field (stat "network[wlan0].rx")
  ```

  Each limit set may contain `careful`, `warning`, `critical` (numbers), `log` (boolean) and `hysteresis` (number ≥ 0). Every key is optional. Inside an item's inline table, those five keys form the item set and every table-valued key is an item + field set. The key `items` is reserved, and no module may name a field `careful`, `warning`, `critical`, `log`, `hysteresis` or `items`.

  All limits are in value% units (percent of the stat's maximum). Where a module file says "maximum = 100", value% equals the raw value, so the limits are written in the raw unit (for example bits/s or °C).

  Keys that are not thresholds (for example `alias`, `hide`, `show`) stay in the module's own table (`[<module>]`), never under `thresholds`. Module files MUST NOT define any other place for limits, such as flat `<field>_careful` keys or `[<module>.items."<name>"]` tables.
- **ALR-012** Limit lookup is done separately for each key (careful, warning, critical, log, hysteresis). For a stat `module[item].field`, ogle uses the first set that defines that key, in this order:
  1. The item + field set: `items."<item>".<field>` in `[module.thresholds]`
  2. The item set: `items."<item>"`
  3. The field set: `<field>`
  4. The module-wide set: the plain keys of `[module.thresholds]`
  5. The compiled-in default for that stat (from the module file)
  6. For `log` and `hysteresis` only: the `[alert]` table (`log` has no `[alert]` key and defaults to false; `hysteresis` defaults to `[alert] hysteresis`)

  Steps that do not apply to a stat are skipped (a stat with no item skips steps 1 and 2; a stat with no field skips steps 1 and 3). Levels can be overridden one at a time, so a user can raise only `critical` for one mount point.

  This matches Glances' `<plugin>_<header>_<level>` then `<plugin>_<level>` lookup (`model.py:978-992`), extended with the item level.
- **ALR-013** Item names are matched exactly and are case-sensitive. When an item has an alias, ogle tries the alias first and then the real name. Item names are the item's real names as the module defines them (the mount point, the interface name, the disk name or the sensor label), not the truncated display text.
- **ALR-014** Setting a level to `false` removes that level, even when a default exists. For example, `critical = false` disables CRITICAL for that stat. A table with all three levels set to `false` makes the stat evaluate to `DEFAULT`.
- **ALR-015** Some modules have special lookup rules. For example, network uses absolute limits from the item levels (steps 1 and 2) and falls back to percent-of-link-speed limits from the field and module levels (steps 3 to 5) when no item level applies, and sensors fall back to the hardware's own limits. The module file defines these rules, and it MUST express them in terms of this lookup and this table layout.

## 5. Event engine

- **ALR-016** Event record fields:

  | Field | Meaning |
  |---|---|
  | `type` | Event type name (ALR-008) |
  | `state` | `WARNING` or `CRITICAL` |
  | `begin` | Wall-clock time of creation (Unix seconds), for display |
  | `end` | Wall-clock time of closing, or "ongoing" |
  | `begin_mono` / `end_mono` | Monotonic timestamps, used for every duration and interval check |
  | `min` / `max` / `sum` / `count` | Statistics over the value% values received while the event was open; `mean = sum / count` |
  | `top` | Up to 3 process names (ALR-021) |
  | `top_counts` | Internal map from process name to count |

  ogle uses monotonic time for the duration checks in ALR-019 and ALR-020 and for the displayed duration. Glances uses wall-clock time, which breaks when the clock jumps. That is a quirk fix.
- **ALR-017** Matching. For each incoming `(type, state, value%)`, the engine looks for an existing event of the same `type` that is either ongoing or finished less than `min_interval` seconds ago (monotonic). At most one match is possible.
- **ALR-018** No match:
  - If the state is `WARNING` or `CRITICAL`, create a new ongoing event with `min = max = sum = value%`, `count = 1`, `state = state`, and an empty `top`. If the new state is CRITICAL, also apply the top-process step (ALR-021).
  - Insert the new event at the front of the list, so the list is newest first.
  - In any other state, do nothing.

  The top-process step on creation is a deliberate fix: Glances only fills `top` on later CRITICAL updates.
- **ALR-019** Match found, state `OK`, `CAREFUL` or `DEFAULT`:
  - If the event is ongoing, close it. If `now_mono - begin_mono >= min_duration`, set `end` to now. Otherwise delete the event from the list. Events shorter than `min_duration` are noise.
  - If the event is already finished, do nothing.
- **ALR-020** Match found, state `WARNING` or `CRITICAL`:
  - Reopen the event if it was finished, which merges it with the recent one.
  - Update `min`, `max`, `sum` and `count` with value%.
  - State only escalates: WARNING may become CRITICAL, but CRITICAL never goes back to WARNING.
  - If the incoming state is CRITICAL, apply ALR-021. If the incoming state is WARNING, clear `top_counts`, but keep the existing `top` list. This matches `event.py:73-112`.
- **ALR-021** Top processes, on each CRITICAL input:
  1. Take the first 6 processes of the *unfiltered* process list, sorted descending by the event's auto-sort key (ALR-024).
  2. Add 1 to `top_counts` for each of their names. Use the program name the process list displays: the short name, not the full command line.
  3. Set `top` to the 3 names with the highest counts. Break ties by first appearance.

  If process collection is disabled (`z` key or config), leave `top` unchanged. The event engine MUST NOT start process collection itself.

  Note: Glances uses the filtered list. ogle uses the unfiltered list, so the result does not depend on the user's current filter.
- **ALR-022** List size: when the list grows above `max_events`, remove the oldest *finished* event. If every event is ongoing, remove the oldest ongoing event.

  Glances always pops the last element, which can drop an ongoing event. That is a fix.
- **ALR-023** The engine runs on the UI thread, or at least serialised with rendering. Each registration is processed straight away, in module refresh order. Event timestamps therefore have the granularity of the refresh interval of the module that produced them.

## 6. Process auto-sort contract

- **ALR-024** Each event type maps to an auto-sort key:
  - Types starting with `MEM` (which covers `MEM` and `MEMSWAP`) map to memory percent.
  - Types starting with `CPU_IOWAIT` map to disk I/O.
  - Every other type maps to CPU percent.
- **ALR-025** The processes module (`04-processes.md`) MUST implement this contract. When, and only when, the user's sort mode is *automatic*, the effective sort key is:
  - the auto-sort key of the **most recently created ongoing** event, if any event is ongoing;
  - otherwise CPU percent.

  The processes module recomputes the key after each cycle's registrations. A manual sort choice is never changed by events.

  Glances uses "last update wins" and resets to CPU when *any* event closes, even if another is still ongoing (`events_list.py:221-243`). ogle derives the key from the set of ongoing events instead, which is a fix.
- **ALR-026** The event engine exposes a read-only query, `auto_sort_key() -> {cpu, memory, io}`. It holds no reference to the processes module apart from the read in ALR-021. That read goes through a provider function, so the event engine can be tested without processes.

## 7. Global alert message

- **ALR-027** The alert panel title is computed from the threshold state table. Weights: OK = 0, CAREFUL = 1, WARNING = 2, CRITICAL = 3. DEFAULT or a missing stat counts as 0. For each row of the table below, sum the weights of its stats. Take the row with the highest sum; on a tie, the earlier row wins. If that sum is ≥ the row's minimum weight, the title is that row's message. Otherwise the title is "EVENTS history".

  | # | Message | Stats | Minimum weight |
  |---|---|---|---|
  | 0 | EVENTS history | (none) | 0 |
  | 1 | High CPU user mode | `cpu.user` | 2 |
  | 2 | High CPU kernel usage | `cpu.system` | 2 |
  | 3 | High CPU I/O waiting | `cpu.iowait` | 2 |
  | 4 | Large CPU stolen time. System running the hypervisor is too busy. | `cpu.steal` | 2 |
  | 5 | High CPU niced value | `cpu.nice` | 2 |
  | 6 | System overloaded in the last 5 minutes | `load` | 2 |
  | 7 | High swap (paging) usage | `memswap` | 2 |
  | 8 | High memory consumption | `mem` | 2 |

  The table and its messages come from `events_list.py:27-41`. `cpu.nice` has no default limits, so row 5 is only reachable when the user configures `nice = { ... }` in `[cpu.thresholds]`. In Glances the row is never reachable.
- **ALR-028** The title shows the computed global message only while at least one event is ongoing. Otherwise the title is "EVENTS history". The title is computed live at render time. It is not stored on events, as Glances does.

## 8. Alert panel

- **ALR-029** Placement: the alert panel is the last item of the right column. It is bottom-aligned to the last terminal row, and its height is `1 + number of events`. The layout engine (`01-runtime-ui.md`) gives it priority over process rows: the process list loses rows first. If the panel still does not fit, it shows the title and as many of the newest events as fit.
- **ALR-030** When the event list is empty, the panel takes no lines.
- **ALR-031** Title line: the title from ALR-027/ALR-028, drawn with the `TITLE` decoration.
- **ALR-032** One line per event, newest first. The parts are concatenated with no separators except the ones shown:
  1. `begin` in local time, formatted `%Y-%m-%d %H:%M:%S(%z)`, for example `2026-11-19 18:43:07(+0100)`. No decoration.
  2. For a finished event: ` (<duration>)`, where the duration is `end_mono - begin_mono` rounded down to whole seconds and formatted `H:MM:SS`, or `D day(s), H:MM:SS` when it is 1 day or more. This matches Python `timedelta` output. For an ongoing event: ` (ongoing)`.
  3. ` - `
  4. For a finished event: `<STATE> on <TYPE>`, with no decoration. For an ongoing event: `<TYPE>`, with the decoration `<STATE>` (`WARNING` or `CRITICAL`, without `_LOG`).
  5. The statistics. If `|max - min| <= 0.1 * max(|min|, |max|)`, show ` (<mean>)`. Otherwise show ` (Min:<min> Mean:<mean> Max:<max>)`. Every number uses one decimal place.
  6. If `top` is not empty: `: name1, name2, name3`.

  The line is splittable: the renderer truncates it at the right edge.
- **ALR-033** Values in event lines are in value% units, as in Glances. For `load`, that is the 15-minute load per logical core × 100.

## 9. Keys and toggles

- **ALR-034** `w`: delete all *finished* events except finished CRITICAL ones. Ongoing events are always kept.
- **ALR-035** `x`: delete all finished events, including CRITICAL ones. Ongoing events are kept.
- **ALR-036** `l`: show or hide the alert panel. Hiding the panel MUST NOT stop threshold evaluation, registration or the event engine. Auto-sort and decorations keep working, and the events are there again when the panel is shown. This matches Glances, where `disable_alert` only stops the display.
- **ALR-037** The help screen (`01-runtime-ui.md`) MUST describe `w`, `x` and `l` as stated here.

## 10. Decorations

- **ALR-038** A decoration is a semantic style name attached to a span of text. The renderer maps each name to a colour role (`01-runtime-ui.md`). Names used by threshold logic:

  | Decoration | Produced when |
  |---|---|
  | `DEFAULT` | State DEFAULT, or a field that is not evaluated |
  | `OK`, `CAREFUL`, `WARNING`, `CRITICAL` | The registered or pure evaluation result, and the log flag is false |
  | `OK_LOG`, `CAREFUL_LOG`, `WARNING_LOG`, `CRITICAL_LOG` | The registered evaluation result, and the log flag is true |
  | `MAX` | ALR-040 |

  Other decoration names used by the UI (`TITLE`, `BOLD`, `UNDERLINE`, `SORT`, `FILTER`, `SELECTED`, `PROCESS`, `PROCESS_SELECTED`, `STATUS`, `CPU_TIME`, `INFO`, `ERROR`, `SEPARATOR`) are owned by `01-runtime-ui.md` and the module files.
- **ALR-039** Colour-role mapping, owned here. The values are in `01-runtime-ui.md`:
  - `OK` uses the "ok" role, `CAREFUL` the "careful" role, `WARNING` the "warning" role, and `CRITICAL` the "critical" role. These are foreground colours; WARNING and CRITICAL are also bold.
  - Each `*_LOG` uses the same hue as a background (inverse style), and is bold.
  - When the background is disabled, each `*_LOG` falls back to its foreground role plus bold.
  - Monochrome fallback (GEN-016): `OK` plain, `CAREFUL` bold, `WARNING` underline, `CRITICAL` reverse. The `_LOG` variants add bold.
- **ALR-040** `MAX` highlight. This fixes `is_max` in Glances, which has no effect (`model.py:875-890`). In the process list (and the program list), among the rows the list shows after filtering (filter, focus and kernel-thread hiding), the CPU% cell with the highest value and the MEM% cell with the highest value get the `MAX` decoration. A cell gets `MAX` only when both hold:
  - The cell would otherwise be `OK` (its own threshold state is `OK`). `DEFAULT`, `CAREFUL`, `WARNING` and `CRITICAL` always win over `MAX`.
  - Its value is greater than 0.

  On a tie, the first row in display order gets it. The two columns are judged separately, so the CPU% and MEM% highlights may fall on different rows.

  `MAX` is the "ok" role plus bold. The processes module (`04-processes.md`, PROC-041) MUST implement exactly this rule.

## 11. Configuration: `[alert]`

- **ALR-041** Keys of the `[alert]` table:

  | Key | Type | Default | Meaning |
  |---|---|---|---|
  | `max_events` | integer ≥ 1 | 10 | Maximum number of events kept (ALR-022) |
  | `min_duration` | seconds ≥ 0 | 6 | Ongoing events shorter than this are discarded when they close (ALR-019) |
  | `min_interval` | seconds ≥ 0 | 6 | A new trigger of the same type within this time after an event closes reopens that event (ALR-017) |
  | `hysteresis` | number ≥ 0 | 0 | Default hysteresis in value% units for every registered stat (ALR-004) |

  Showing or hiding the panel uses the generic module enable key defined in `01-runtime-ui.md`.
- **ALR-042** Out-of-range values: ogle prints a warning on stderr and uses the default.

## 12. Logged stats by default

- **ALR-043** These stats are registered with `log = true` by default, so they can create events. The default limits shown are informative here; the module files are authoritative and MUST match.

  | Stat | Event type | Maximum used for value% | Default limits | Module file |
  |---|---|---|---|---|
  | `cpu.total` | `CPU_TOTAL` | 100 | 65 / 75 / 85 | 02 |
  | `cpu.iowait` | `CPU_IOWAIT` | 100 | 50 / 70 / 90 | 02 |
  | `load` (min15) | `LOAD` | logical cores (value% = load/cores × 100) | 70 / 100 / 500 | 02 |
  | `mem` | `MEM` | total memory (value% = used/total × 100) | 50 / 70 / 90 | 02 |
  | `memswap` | `MEMSWAP` | total swap | 50 / 70 / 90 | 02 |

  Note on load units: Glances writes load limits as 0.7/1.0/5.0. Combined with `maximum = 100 × cores`, those are compared against load/cores, while every other stat is compared in percent. ogle uses one unit everywhere, value%, so the default load limits are written as **70 / 100 / 500**. The behaviour is the same.

  Other stats (`cpu.user`, `cpu.system`, `cpu.steal`, `cpu.ctx_switches`, network, diskio, fs, sensors) are registered with `log = false` by default. Decoration-only stats (ALR-007: `percpu.*`, `quicklook.*`, `load` min5, and the process-list cells `processlist.cpu` and `processlist.mem`) are never registered and never log. Users can turn logging on per stat with `log = true` (ALR-011/012). `cpu.user` and `cpu.system` are marked `log` in Glances' field descriptions, but the shipped `glances.conf` turns them off, and ogle's defaults follow the shipped behaviour.
- **ALR-044** Glances skips the `dpc` field (Windows CPU) when it has no limits; ogle does the same. Unconfigured fields evaluate to DEFAULT and never log.

## 13. Glances quirks: summary

| Glances behaviour | ogle | ID |
|---|---|---|
| Event engine called in every state, global message rebuilt each time | Skipped when it would do nothing | ALR-009 |
| `is_max` has no effect | `MAX` highlight defined | ALR-040 |
| `min5` overwrites the `min15` load threshold state | Only min15 registers | ALR-010 |
| Wall-clock durations | Monotonic durations | ALR-016 |
| `top` only filled after the first CRITICAL update; uses the filtered list | Filled at creation; unfiltered list | ALR-018, ALR-021 |
| List overflow can drop an ongoing event | Drops the oldest finished event first | ALR-022 |
| Auto-sort reset by any event closing | Derived from the set of ongoing events | ALR-025 |
| `cpu_niced` message unreachable | Reachable when limits are configured | ALR-027 |
| Load limits in a different unit from everything else | All limits in value% | ALR-043 |
| Global message stored on the event | Computed live | ALR-028 |
