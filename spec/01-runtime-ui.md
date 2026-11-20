# ogle specification: runtime and user interface

Status: draft, phase 1
Requirement prefix: `UI-`
Binding decisions: `00-overview.md`, section 6 (GEN-001 to GEN-017)

This file covers everything between the OS data sources and the terminal: the command
line, the configuration file, the main loop and its timing, input decoding, hotkeys,
the layout engine, rendering, colours, popups and the help screen. Module panel content
belongs to `02-` to `05-`. Process actions (sort, filter, kill, renice, pin) are
described here only as UI. `04-processes.md` defines what they do.

Glances source references use the prefix `G/` for
`/usr/lib/python3.14/site-packages/glances/`. They are informative only.

---

## 1. Command line

### 1.1 General rules

- **UI-001** ogle accepts GNU-style options: long options with `--name` or `--name=value`,
  and short options where listed. Short options without values may be grouped (`-13`).
- **UI-002** Options override the configuration file, which overrides built-in defaults.
  Precedence order: built-in default < config file < command line.
- **UI-003** On a usage error ogle prints one line of the form `ogle: <message>` plus
  `Try 'ogle --help' for more information.` to stderr, and exits with status 2.
  It does not touch the terminal state in this case.
- **UI-004** If stdin or stdout is not a terminal, ogle prints
  `ogle: standard input and output must be a terminal` to stderr and exits with status 1.
- **UI-005** Comma-separated list values accept optional spaces around commas and are
  case-insensitive for module names and sort keys.

### 1.2 Options

| Option | Value | Default | Effect | Glances equivalent |
|---|---|---|---|---|
| `-h`, `--help` | | | Print usage and exit 0 | `-h` |
| `-V`, `--version` | | | Print `ogle <version>` and exit 0 | `-V` |
| `-C`, `--config` | PATH | search order of GEN-012 | Use this config file. A missing file given here IS an error (exit 2) | `-C` |
| `--no-config` | | | Ignore every config file | none |
| `--print-default-config` | | | Print the full default config as commented TOML and exit 0 | none |
| `-t`, `--refresh` | SECONDS (float) | 2.0 | Global refresh interval, 0.1 to 3600 | `-t/--time` |
| `--stop-after` | N (int ≥ 1) | none | Exit after N completed cycles (UI-024) | `--stop-after` |
| `--disable` | MODULE[,MODULE...] or `all` | | Disable modules at start | `--disable-plugin` |
| `--enable` | MODULE[,MODULE...] | | Enable modules at start. Applied after `--disable`, so `--disable all --enable cpu,mem` works | `--enable-plugin` |
| `--list-modules` | | | Print the module names, one per line, and exit 0 | `--modules-list` |
| `--light` | | off | Start with the left sidebar, the process modules and the alert panel off | `--light` |
| `-0`, `--normalize-cpu` | | off | Start with per-core normalisation on (key `0`, UI-043) | `-0/--disable-irix` |
| `-1`, `--percpu` | | off | Start in per-CPU mode | `-1/--percpu` |
| `-2`, `--no-left-sidebar` | | off | Start with the left sidebar hidden | `-2` |
| `-3`, `--no-quicklook` | | off | Start with quicklook hidden | `-3` |
| `-4`, `--full-quicklook` | | off | Start in full-quicklook mode | `-4` |
| `-5`, `--no-top` | | off | Start with the whole top row hidden | `-5` |
| `--sparkline` | | off | Quicklook draws sparklines instead of bars | `--sparkline` |
| `--no-history` | | off | Keep no history. Disables trend arrows and sparklines (GEN-008) | `--disable-history` |
| `--sort` | KEY | `auto` | Initial process sort key: `auto cpu mem io time name user core pid` (PROC-060) | `--sort-processes` (only 3 keys there) |
| `--programs` | | off | Start in program (grouped) view | `--programs` |
| `-f`, `--filter` | PATTERN | none | Initial process filter, same syntax as the filter popup | `-f/--process-filter` |
| `--focus` | PATTERN[,PATTERN...] | none | Show only processes that match any pattern. Combined with the filter: a process must match both (PROC-074) | `--process-focus` |
| `--long-names` | | off | Show the full command path in the process list | `--process-long-name` |
| `--hide-kernel-threads` | | off | Hide kernel threads (no effect on platforms without a kernel-thread flag: macOS, Windows; PROC-021) | `--hide-kernel-threads` |
| `--no-processes` | | off | Start with process collection off (key `z`) | `--disable-process` |
| `--arrow-keys-sort` | | off | Swap plain and shifted Left/Right (UI-047) | `--arrow-keys-sort` |
| `-b`, `--byte` | | off | Network rates in bytes instead of bits (`[network] bytes`) | `-b/--byte` |
| `--diskio-iops` | | off | Disk I/O starts in IOPS mode (overrides `[diskio] mode`; IO-049) | `--diskio-iops` |
| `--diskio-latency` | | off | Disk I/O starts in latency mode (overrides `[diskio] mode`). Giving both diskio mode options is a usage error | `--diskio-latency` |
| `--diskio-show-ramfs` | | off | Show `ram*` block devices (`[diskio] show_ramfs`) | `--diskio-show-ramfs` |
| `--fs-free-space` | | off | File system panel shows free instead of used (`[fs] free_space`) | `--fs-free-space` |
| `--fahrenheit` | | off | Temperatures in °F (`[sensors] fahrenheit`) | `--fahrenheit` |
| `--strftime` | FORMAT | `[now] format` (SYS-041) | Clock format; overrides `[now] format` | `--strftime` |
| `--color` | `auto`, `always`, `never` | `auto` | Colour mode (UI-090) | none |
| `--no-color` | | | Same as `--color=never` | none |
| `--no-bold` | | off | Never use the bold attribute (UI-093) | `--disable-bold` |
| `--no-bg` | | off | Never use background colours (UI-092) | `--disable-bg` |
| `--ascii` | | auto | Use ASCII glyphs only (GEN-017, UI-081) | `--disable-unicode` |
| `--no-separator` | | off | Do not draw separator lines | `--disable-separator` |
| `--log-file` | PATH | none | Write a debug log to PATH. Without it ogle writes no log | `-d/--debug` |

- **UI-006** The option table above is the complete phase-1 set. Options for keys that
  are reserved or for out-of-scope features (GEN-010, overview 2.2) are not accepted.
- **UI-007** These Glances options have no ogle equivalent in phase 1, because their
  features are out of scope: `-c -s -w -B -p -u --browser --username --password --snmp-*
  --cached-time --disable-autodiscover --disable-webui --enable-mcp --mcp-path
  --open-web-browser --export* --stdout* --fetch* --issue --api-doc --api-restful-doc
  --trace-malloc --memory-leak -6/--meangpu --enable-irq --hide-public-info
  --disable-check-update --disable-config-exec -P/--plugins -q/--quiet`.
  `--disable-cursor` and `--enable-process-extended` are also dropped. The cursor is
  always on in the TUI, and pinning is interactive only.
- **UI-008** An unknown module name in `--disable`, `--enable` or `--focus`-like lists is
  a usage error (UI-003). The message names the bad token.

---

## 2. Configuration file

- **UI-010** The file format, the search order and the unknown-key warning are as stated
  in GEN-012. A TOML syntax error is fatal: print `ogle: <path>:<line>: <message>` to
  stderr and exit 2. A value of the wrong type is fatal in the same way.
- **UI-011** Read the config file once at start-up. ogle does not watch or reload it.
- **UI-012** `[global]` keys:

| Key | Type | Default | Meaning | CLI override |
|---|---|---|---|---|
| `refresh` | float, seconds | 2.0 | Global refresh interval (0.1 to 3600) | `--refresh` |
| `history` | bool | true | Keep trend/sparkline history | `--no-history` |

  The clock format is not a `[global]` key. It is `[now] format` (SYS-041), which
  `--strftime` overrides.

- **UI-013** `[ui]` keys. Each key sets the start-up state; hotkeys can change it later.

| Key | Type | Default | Meaning | CLI override |
|---|---|---|---|---|
| `color` | `"auto"`, `"always"`, `"never"` | `"auto"` | UI-090 | `--color`, `--no-color` |
| `bold` | bool | true | UI-093 | `--no-bold` |
| `background` | bool | true | UI-092 | `--no-bg` |
| `unicode` | `"auto"`, `"always"`, `"never"` | `"auto"` | UI-081 | `--ascii` |
| `separator` | bool | true | Separator lines | `--no-separator` |
| `left_sidebar` | bool | true | Left sidebar shown | `-2` |
| `sidebar_order` | array of module names | `["network", "diskio", "fs", "sensors", "now"]` | Left sidebar modules, top to bottom. Modules not listed are not shown in the sidebar | none |
| `quicklook` | bool | true | Quicklook shown | `-3` |
| `full_quicklook` | bool | false | Full-quicklook mode | `-4` |
| `top` | bool | true | Top row shown | `-5` |
| `percpu` | bool | false | Per-CPU mode | `-1` |
| `sparkline` | bool | false | Sparklines in quicklook | `--sparkline` |
| `normalize_cpu` | bool | false | Key `0` state | `-0` |
| `arrow_keys_sort` | bool | false | UI-047 | `--arrow-keys-sort` |
| `escape_timeout_ms` | int | 30 | UI-033 | none |

- **UI-014** Every module table (`[cpu]`, `[network]`, ...) has two common keys:
  `enabled` (bool, default true for all phase-1 modules) and `refresh` (float seconds,
  default is the module default from GEN-006, or `[global] refresh`). All other keys of a
  module table are defined in that module's spec file. Threshold limits live only in the
  `[<module>.thresholds]` sub-table defined in `05-alerts.md` (ALR-011).
- **UI-015** `--disable`/`--enable` override `enabled` in the module tables.
- **UI-016** `sidebar_order` may only list sidebar-capable modules
  (`network`, `diskio`, `fs`, `sensors`, `now`). Other names are a config error (UI-010).
  `now` is always bottom-aligned (UI-066) wherever it appears in the list.

---

## 3. Main loop and timing

### 3.1 Start-up and shutdown

- **UI-020** Start-up order:
  1. Parse the CLI, then load and check the config.
  2. Detect the platform and terminal capabilities (colour depth, UTF-8).
  3. Enter the terminal UI state: alternate screen, cursor hidden, raw input mode,
     line wrap off. On Windows, enable VT processing and UTF-8 output for the console.
  4. Take the first sample of every enabled module.
  5. Draw the first frame. Rates show `-` (GEN-007).
  6. Take a priming second sample of every enabled module about 500 ms after step 4
     (PERF-012), then draw a frame with all rates filled in. Key presses during this wait
     are handled as in UI-026. The first normal deadline is `priming_start + refresh`;
     later cycles follow UI-024.
- **UI-021** On every exit path (normal quit, fatal error, SIGINT, SIGTERM, SIGHUP, panic
  or unhandled exception) ogle restores the terminal: original termios/console mode,
  main screen, cursor visible, all attributes reset. A fatal error message is printed to
  stderr after the terminal is restored, so it stays visible.
- **UI-022** Exit status: 0 for a user quit, `--stop-after`, SIGINT, SIGTERM or SIGHUP.
  1 for a runtime fatal error. 2 for usage and config errors.
- **UI-023** Ctrl-Z (byte 0x1A) suspends ogle on Unix. Restore the terminal, then raise
  SIGTSTP. On SIGCONT, re-enter the UI state and force a full redraw. On Windows,
  Ctrl-Z is ignored.

### 3.2 Cycle timing

- **UI-024** ogle runs in cycles (overview section 5). Each cycle:
  1. Set `cycle_start` from a monotonic clock.
  2. Collect every enabled module whose own refresh interval has passed (GEN-006).
  3. Compute derived stats and thresholds (`05-alerts.md`).
  4. Draw the frame.
  5. Wait for input, a resize or the deadline `cycle_start + refresh`.
  With `--stop-after N`, exit with status 0 right after the frame of cycle N is drawn.
- **UI-025** The wait in step 5 uses one blocking call (poll/select on the input fd plus a
  self-pipe or signalfd for signals, or `WaitForMultipleObjects` on Windows) with a
  timeout equal to the time **remaining** until the deadline. ogle MUST NOT wake up on a
  fixed tick (Glances wakes every 100 ms). When idle, ogle makes no system calls between
  deadlines except to handle input and signals.
- **UI-026** A key press handled during the wait applies its action, redraws the frame
  from the stats already held, then waits again for the **same** deadline. Key presses
  do not trigger a new sample and do not move the deadline. Exceptions: F5 and Ctrl-R
  (UI-045) and toggles that enable a module. Enabling a module collects that module
  right away, so its panel never shows stale or empty data.
- **UI-027** If collecting and drawing take longer than the refresh interval, the next
  cycle starts at once. The deadline is always `cycle_start + refresh` for the current
  cycle. ogle does not try to catch up on missed cycles. The priming sample at start-up
  (UI-020 step 6) is the only cycle that is not on the `refresh` cadence.
- **UI-028** A clock jump (wall-clock change) does not affect cycle timing, because timing
  uses the monotonic clock. The clock module and the alert timestamps use wall time.

### 3.3 Resize

- **UI-029** On Unix, SIGWINCH sets a flag and wakes the wait (through the self-pipe).
  On Windows, `WINDOW_BUFFER_SIZE_EVENT` console input records do the same.
- **UI-030** On a resize, ogle reads the new size (`TIOCGWINSZ` or
  `GetConsoleScreenBufferInfo`), clears its frame buffer, and redraws the whole screen
  from the stats already held, within 50 ms of the event. Resizing does not resample
  and does not move the deadline.
- **UI-031** Several resize events that arrive before the redraw collapse into one redraw.

---

## 4. Input decoding

- **UI-032** Read input in non-canonical mode with echo off. Decode bytes as UTF-8.
  Turn input bytes into key events with a parser that understands at least:
  - C0 control bytes: Enter as CR (0x0D) or LF (0x0A), Tab, Backspace as DEL (0x7F) or
    BS (0x08), Ctrl-A to Ctrl-Z.
  - CSI sequences (`ESC [` ... final byte), including parameters and modifiers:
    arrows `A B C D`, Home and End (`H`, `F`, `1~`, `4~`, `7~`, `8~`), Insert `2~`,
    Delete `3~`, PageUp `5~`, PageDown `6~`, F5 `15~`. The modifier parameter
    (`1;2C` for Shift-Right and so on) decodes Shift, Alt and Ctrl.
  - SS3 sequences (`ESC O` ...): arrows, Home, End, F1 to F4, and keypad Enter (`ESC O M`).
  - Alt+key as `ESC` followed by a printable character.
  - On Windows, read `KEY_EVENT_RECORD`s and map virtual-key codes to the same key events.
- **UI-033** A lone `ESC` byte is the Esc key if no further byte arrives within
  `escape_timeout_ms` (default 30 ms).
- **UI-034** A sequence the parser does not know is dropped as a whole. Its bytes MUST NOT
  leak through as separate key presses. (Glances maps the raw bytes `A` (65) and `B` (66)
  to Up and Down, so `A` and `B` also move the cursor; see section 9.)
- **UI-035** Keypad Enter is the same as Enter.
- **UI-036** ogle does not enable mouse reporting or bracketed paste in phase 1.
- **UI-037** Ctrl-C (0x03) quits, the same as `q`. Raw mode means Ctrl-C arrives as a key,
  not as SIGINT. ogle still handles SIGINT for the case where it arrives from elsewhere.

---

## 5. Hotkeys

### 5.1 Rules

- **UI-040** One key table drives both the dispatcher and the help screen (UI-110), so they
  cannot drift apart. Each entry holds: key, action, a short help label, a help group and,
  for toggles, a function that reads the current state.
- **UI-041** Hotkeys are case-sensitive. They act only on the main screen. While a popup
  or the help screen is open, keys go to that popup (section 8).
- **UI-042** A toggle that disables a module stops its collection (GEN-004) and removes
  its panel at the next redraw. The layout reflows (section 6). Exception: `l` only
  hides or shows the alert panel; threshold evaluation and the event engine keep
  running (ALR-036).

### 5.2 Key table

Process-list keys act on the selected row (UI-046). "Module toggle" means flip that
module's enabled flag.

| Key | Action | Group |
|---|---|---|
| `Enter` | Open the process filter popup (UI-103) | Processes |
| `E` | Clear the process filter | Processes |
| `a` | Sort processes automatically | Sort |
| `c` | Sort by CPU% | Sort |
| `m` | Sort by MEM% | Sort |
| `i` | Sort by I/O rate | Sort |
| `t` | Sort by CPU time | Sort |
| `p` | Sort by name | Sort |
| `u` | Sort by user | Sort |
| `o` | Sort by CPU core number (`core`) | Sort |
| `P` | Sort by PID (`pid`) | Sort |
| `r` | Reverse the current sort direction (PROC-064) | Sort |
| `Shift-Left` / `Shift-Right` | Previous / next sort key in cycle order, every key except `auto` (UI-047) | Sort |
| `Up` / `Down` | Move the selection one row | Processes |
| `PageUp` / `PageDown` | Move the selection by one visible page (PROC-092) | Processes |
| `Home` / `End` | Select the first / last process | Processes |
| `Left` / `Right` | Scroll the arguments in the Command column of every row by 1 character (PROC-046, UI-047) | Processes |
| `<` / `>` | Scroll the arguments in the Command column of every row by 8 characters (PROC-046) | Processes |
| `k` | Open the kill popup for the selection (UI-104) | Processes |
| `+` | Renice the selection by +1 (lower priority) | Processes |
| `-` | Renice the selection by -1 (higher priority, usually needs privileges) | Processes |
| `e` | Pin the selection for the extended view; on the pinned process, unpin (PROC-120) | Processes |
| `j` | Toggle the process view and the program (grouped) view | Processes |
| `/` | Toggle short and long command names | Processes |
| `0` | Toggle per-core normalisation: process CPU% and load as percent (UI-043) | Display |
| `M` | Reset the min/max rows of the filter summary | Processes |
| `z` | Toggle process collection (processcount, processlist and programlist together) | Modules |
| `1` | Toggle per-CPU mode | Display |
| `2` | Toggle the left sidebar | Display |
| `3` | Toggle quicklook | Display |
| `4` | Toggle full-quicklook mode: hides cpu, mem and memswap, keeps load (UI-044) | Display |
| `5` | Toggle the whole top row (UI-044) | Display |
| `S` | Toggle quicklook bars and sparklines | Display |
| `b` | Toggle network bits and bytes | Display |
| `T` | Toggle network combined Rx+Tx column | Display |
| `U` | Toggle network cumulative totals | Display |
| `B` | Toggle disk I/O operations per second (turns latency off) | Display |
| `L` | Toggle disk I/O latency (turns IOPS off) | Display |
| `F` | Toggle file system used or free | Display |
| `n` | Module toggle: network | Modules |
| `d` | Module toggle: diskio | Modules |
| `f` | Module toggle: fs | Modules |
| `s` | Module toggle: sensors | Modules |
| `N` | Module toggle: now (clock) | Modules |
| `l` | Show or hide the alert panel; alerts keep being evaluated (ALR-036) | Modules |
| `w` | Delete finished events from the alert list, except finished CRITICAL ones (ALR-034) | Alerts |
| `x` | Delete all finished events from the alert list, including CRITICAL ones (ALR-035) | Alerts |
| `h` | Open the help screen | Other |
| `F5`, `Ctrl-R` | Clear the caches, resample everything now and fully redraw (UI-045) | Other |
| `q`, `Esc`, `Ctrl-C` | Quit | Other |
| `Ctrl-Z` | Suspend (Unix, UI-023) | Other |

- **UI-043** Key `0` (normalise per core) affects two panels, as in Glances: the process
  CPU% column is divided by the logical core count, and the load panel shows load as a
  percentage of the core count. The exact formats are in `02-` and `04-`.
- **UI-044** Key `4` turns on quicklook in full-quicklook mode (no width cap, UI-055) and
  hides cpu (or percpu in per-CPU mode), mem and memswap. Load stays visible (SYS-099).
  Turning it off again restores the earlier visibility of each module. Key `5` hides quicklook, cpu,
  percpu, mem, memswap and load together. Turning it off restores their earlier
  visibility. ogle restores the earlier state of each module rather than blindly
  re-enabling it (Glances re-enables everything).
- **UI-045** F5 and Ctrl-R end the current wait. They clear the process cmdline and
  username caches (PROC-030), re-read the cached core counts (SYS-013), start a new
  cycle at once and redraw the full screen.
- **UI-046** Selection model (the process side is defined in `04-processes.md`):
  - The selection is a process (or program) identity, not a row index. It stays on the
    same process when the list is re-sorted. If that process disappears, the selection
    moves to the row that now has the same index, clamped to the list.
  - The selection can reach every row of the filtered list (no 50-row cap). The list
    scrolls to keep the selected row visible.
  - The selected row starts as the first row. The `>` marker is always shown.
  - Pinning a process (`e`) does not lock the selection and does not hide the cursor
    (PROC-094).
- **UI-047** Sort cycle order for Shift-Left/Shift-Right:
  `cpu, mem, io, time, name, user, core, pid` (every sort key except `auto`, PROC-065),
  wrapping round. Choosing a key by cycling turns
  automatic sorting off. With `arrow_keys_sort` on, plain Left/Right cycle the sort key
  and Shift-Left/Shift-Right scroll the arguments by 1 character. `<` and `>` always
  scroll by 8.
- **UI-048** Reserved keys: `6 7 8 A C D G I K Q R V W g` are bound to out-of-scope
  Glances modules (GEN-010). In phase 1 they do nothing on the main screen. They are not
  listed on the help screen. (`K` is still a key inside the kill popup, UI-104, because
  popups receive keys first, UI-041.)
- **UI-049** Unbound keys do nothing and are not reported.

---

## 6. Layout engine

### 6.1 Model

- **UI-050** Each module's `render(width, max_height)` returns a list of lines. Each line is
  a list of **items**. An item has: text, a colour role (section 7), and three flags:
  - `optional`: drop the item when the layout says the panel is narrow (UI-058).
  - `splittable`: the item may be cut at the right edge. Items that are not splittable
    are shown whole or not at all.
  - `right`: on the header line only, right-align the item (used by uptime).
- **UI-051** Text width means terminal display width (East Asian Width; zero-width marks
  count as 0, wide characters as 2), not bytes or code points. Truncation never splits a
  wide character or a grapheme cluster.
- **UI-052** Coordinates: the screen is `W` columns by `H` rows. `(0, 0)` is the top-left.

### 6.2 Regions

The screen, top to bottom:

```
row 0           header: system ...................................... uptime
row 1           separator
rows 2..        top row: quicklook | cpu or percpu | mem | memswap | load
                separator
rest            left sidebar          | right column
                network               | processcount
                diskio                | processlist or programlist
                fs                    |
                sensors               |
                now (bottom)          | alert (bottom)
```

- **UI-053** Header (row 0): the system panel on the left and the uptime panel on the right.
  Draw the optional part of the system panel only when
  `W >= width(system, full) + width(uptime) + 1`. If the system hostname and uptime still
  do not fit, drop uptime. When separators are on, row 1 is a separator line.
- **UI-054** Top row: the visible modules among quicklook, cpu (or percpu in per-CPU
  mode), mem, memswap and load, in that order, placed side by side starting at column 0.
  Its height is the height of the tallest panel. When separators are on, a separator line
  follows it. If no top-row module is visible, the top row and its separator are absent.
- **UI-055** Quicklook width (`ql`). Let `n` = the number of visible top-row modules
  including quicklook, and `others` = the sum of the full widths of the other visible
  top-row panels. Then:
  - Normal mode: `ql = min(W - (others + 8 + 3*n), 43)`.
  - Full-quicklook mode: `ql = W - (others + 8 + 3*n)`, with no 43 cap.
  - If `ql < 20`, hide quicklook for this frame and recompute the row without it.
  (Glances: `G/outputs/glances_curses.py:793-801`. The hide rule is ogle's addition.)
- **UI-056** Top-row spacing. Let `total` = the sum of the widths of the visible top-row
  panels and `n` their count:
  - If `n == 1`, the spacing is 0.
  - Otherwise `gap = max(1, floor((W - total) / (n - 1)))`, so the row spreads out to fill
    the full width.
  - Quicklook is followed by exactly 1 space, not by `gap`.
- **UI-057** Narrow top row. If `gap < 3`:
  1. Re-render mem in narrow mode (optional items dropped), then recompute `gap`.
  2. If still `gap < 3`, re-render cpu (or percpu) in narrow mode, then recompute.
  3. If still `total + (n - 1) > W`, hide quicklook if it is visible, then recompute.
  4. If it still does not fit, the rightmost panels that do not fit are not drawn.
     A panel is never drawn cut in half.
- **UI-058** "Narrow mode" for a panel means `render` drops items flagged `optional`.
  Which items are optional is defined per module in `02-` and `03-`.
- **UI-059** Left sidebar. Its width is `SW = min(34, max(23, W - 105))`
  (`G/outputs/glances_curses.py:131-132,547-551`). It starts at column 0 on the first row
  after the top-row separator. The sidebar modules from `sidebar_order` are stacked
  vertically, with one blank row between panels. The sidebar is hidden when
  `left_sidebar` is off, or automatically when `W < 70`.
- **UI-060** Right column. It starts at column `SW + 3` when the sidebar is shown, else at
  column 0, on the same row as the sidebar. Its width is the rest of the screen. It holds
  processcount, then processlist (or programlist in program view), then the alert panel,
  with one blank row between panels. If the right column would be narrower than 23
  columns, it is not drawn.
- **UI-061** ogle uses the computed sidebar width `SW`, not the widest drawn sidebar
  panel, to place the right column. The process list therefore does not move sideways
  when sidebar content changes width. (Glances uses the widest drawn panel.)
- **UI-062** Processlist height. The process list gets every row between the end of
  processcount (plus its blank row) and the top of the alert panel (minus one blank row).
  The list passes the visible row count back to the selection logic (PageUp/PageDown,
  UI-046).
- **UI-063** Processlist width. The layout passes the right-column width to
  `processlist.render`. The process module decides which columns to drop
  (`04-processes.md`). Glances shows the optional columns only when `W > 102`.
- **UI-064** Sidebar height. Each sidebar panel gets at most the rows left between its
  start and the top of the bottom-aligned `now` panel (minus one blank row). A panel that
  gets fewer rows than it needs shows its title and as many rows as fit. It must not
  overwrite `now`.
- **UI-065** If a sidebar or right-column panel gets fewer than 2 rows, it is not drawn.
- **UI-066** Bottom alignment. `now` sits on the last row of the left sidebar. The alert
  panel sits on the last rows of the right column. Both are placed first, so other panels
  in the same column flow above them.

### 6.3 Clipping

- **UI-067** Items are placed left to right. If a non-splittable item would cross the
  panel's right edge, it is not drawn, and neither is any later item on that line.
  (Glances drops only that item and may still draw later, shorter items, which can show
  values next to the wrong label.)
- **UI-068** A splittable item that crosses the right edge is cut to fit. ogle does not add
  an ellipsis when it cuts, except where a module spec asks for one.
- **UI-069** Lines beyond the panel's `max_height` are not drawn.
- **UI-070** Panels never draw outside their own rectangle. The renderer enforces this
  with a clip rectangle, independent of how well each module behaves.

### 6.4 Minimum size

- **UI-071** The minimum usable size is 50 columns by 12 rows. Below it, ogle clears the
  screen and shows, centred: `Terminal too small: WxH (need at least 50x12)`. Sampling
  continues and `q`, `Esc`, Ctrl-C and Ctrl-Z still work. When the terminal is large
  enough again, the normal layout comes back on the next redraw. (Glances has no minimum
  and draws whatever fits.)

---

## 7. Rendering, glyphs and colour

### 7.1 Frame output

- **UI-080** ogle renders into an off-screen cell buffer (glyph, foreground, background,
  attributes) and compares it with the previous frame. It writes only the changed cells,
  with cursor-motion and SGR sequences, and sends one frame with a single `write`
  (or `WriteConsoleW`) call where the platform allows. A full clear-and-redraw happens
  only on start-up, resize, resume from suspend, and F5/Ctrl-R. Closing a popup is not a
  full redraw: the cells under the popup are simply different from the last frame, so the
  normal diff restores them.
- **UI-081** Glyph set. ogle uses UTF-8 glyphs when `unicode` is `always`, or when it is
  `auto` and the locale is UTF-8 (`LC_ALL`, then `LC_CTYPE`, then `LANG` contains
  `UTF-8`/`utf8`, case-insensitive; on Windows always after setting the console code page
  to UTF-8). Otherwise it uses the ASCII set:

| Purpose | UTF-8 | ASCII |
|---|---|---|
| Separator line | `─` U+2500 | `-` |
| Trend up / down / flat | `↑` / `↓` / `-` | `^` / `v` / `-` |
| Battery full | `✓` | `=` |
| Scrolled command marker | `…` | `~` |
| Quicklook bar fill | `\|` (configurable in `02-`) | `\|` |
| Sparkline blocks | `▁▂▃▄▅▆▇█` | bar mode is used instead |
| Popup border | `┌─┐│└┘` | `+-+\|++` |

- **UI-082** Separator lines span the full screen width and use the `SEPARATOR` role.
  They are drawn after the header and after the top row, and only when `separator` is on.
  When separators are off, the rows stay in place but are left blank, so the layout does
  not shift when the setting changes.

### 7.2 Colour roles

- **UI-083** Modules refer only to colour **roles**, never to concrete colours.
  The phase-1 roles are:

| Role | Use | 8/16 colours | 256 colours | Monochrome |
|---|---|---|---|---|
| `DEFAULT` | Normal text | default fg/bg | same | normal |
| `TITLE` | Panel titles | bold | bold | bold |
| `OK` | Value below careful | green fg | same | normal |
| `CAREFUL` | Value ≥ careful | blue fg | same | bold |
| `WARNING` | Value ≥ warning | magenta fg, bold | same | underline |
| `CRITICAL` | Value ≥ critical | red fg, bold | same | reverse |
| `OK_LOG` | Logged stat, OK | white on green, bold | same | normal |
| `CAREFUL_LOG` | Logged stat, careful | white on blue, bold | same | bold |
| `WARNING_LOG` | Logged stat, warning | white on magenta, bold | same | underline + bold |
| `CRITICAL_LOG` | Logged stat, critical | white on red, bold | same | reverse + bold |
| `MAX` | Highest process value in a column | green fg, bold | same | bold |
| `PROCESS` | Process command name | green fg | same | normal |
| `PROCESS_SELECTED` | Command name of the selected row | green fg, underline | same | underline |
| `STATUS` | Process status `R` | green fg | same | normal |
| `CPU_TIME` | Process time ≥ 1 hour | magenta fg | same | normal |
| `SORT` | Header of the active sort column | bold, underline | same | bold, underline |
| `SELECTED` | Selection marker `>` | bold | yellow fg, bold | bold |
| `FILTER` | Filter text and filter summary rows | bold | cyan fg, bold | bold |
| `INFO` | Neutral notices | blue fg | same | bold |
| `ERROR` | Error messages in popups | red fg, bold | yellow fg, bold | reverse |
| `SEPARATOR` | Separator lines and popup borders | default fg | grey (palette index 244) | normal |

  Sources: `G/outputs/glances_colors.py:62-169`.
- **UI-084** ogle uses the terminal's default foreground and background (SGR 39/49) as the
  base. It never paints a full-screen background.
- **UI-085** "8/16 colours" uses only SGR 30-37 and 40-47. "256 colours" adds `38;5;n`
  where the table says so. ogle does not use 24-bit colour in phase 1.
- **UI-086** Colour depth detection in `auto` mode:
  - 256 colours if `COLORTERM` is `truecolor` or `24bit`, or `TERM` contains `256color`.
  - Monochrome if `TERM` is `dumb` or unset (Unix).
  - Otherwise 8/16 colours.
  - On Windows 10 and later with VT mode enabled: 256 colours.
- **UI-087** `CAREFUL_LOG` is white on blue in every colour depth. (Glances uses a blue
  foreground without a background on 8-colour terminals, which makes it look like
  `CAREFUL`.)

### 7.3 Colour options

- **UI-090** Colour mode: `color = "never"`, `--no-color`, or a non-empty `NO_COLOR`
  environment variable (https://no-color.org) selects the monochrome column of UI-083.
  `--color=always` overrides `NO_COLOR`. `auto` follows UI-086.
- **UI-091** In monochrome mode ogle emits no colour SGR codes at all, only attributes.
- **UI-092** `--no-bg` (`background = false`): the four `*_LOG` roles keep their
  foreground colour (green, blue, magenta, red) and bold, and lose their background.
  No other role uses a background.
- **UI-093** `--no-bold` (`bold = false`): remove the bold attribute from every role, in
  colour modes only. In monochrome mode bold carries meaning (GEN-016), so it stays.
  Glances removes bold everywhere (`G/outputs/glances_colors.py:31`).

---

## 8. Popups and overlays

### 8.1 Common behaviour

- **UI-100** A popup is a box drawn over the centre of the frame:
  - Width = longest content line + 4. Height = number of content lines + 4. Each is clamped
    to the screen size.
  - Border in the `SEPARATOR` role (UI-081 glyphs). Content starts at row 2, column 2,
    inside the box.
  - Content lines longer than the clamped width are cut (splittable).
  Glances silently skips a popup that does not fit the screen. ogle always shows it,
  because UI-071 guarantees at least 50x12.
- **UI-101** Popups are modal for input, but not for sampling. The main loop keeps its
  cycle (UI-024), and the frame behind the popup updates every cycle. (Glances blocks its
  whole loop while a text box is open.)
- **UI-102** In every popup, Esc closes the popup without effect. Ctrl-C still quits ogle.

### 8.2 Filter popup (`Enter`)

- **UI-103** Content:

```
Process filter pattern:
[____________________________________]

Examples:
  python            name or command line contains "python"
  ^/usr/lib         command line starts with /usr/lib
  name:^nginx$      name is exactly nginx
  cmdline:--config  match on the full command line only
  username:^root$   match on the user name
  pid:^12           PID starts with 12

Enter: apply   Esc: cancel   empty: clear filter
```

  - The input field is 40 columns wide and starts filled with the current filter.
    The text cursor is visible inside the field and starts at the end of the text.
  - Editing keys: printable characters insert at the cursor. Left/Right and
    Ctrl-B/Ctrl-F move one character. Home/End and Ctrl-A/Ctrl-E jump to the ends.
    Backspace deletes before the cursor. Delete and Ctrl-D delete under the cursor.
    Ctrl-U deletes to the start. Ctrl-K deletes to the end. Ctrl-W deletes the word
    before the cursor.
  - Text longer than the field scrolls sideways to keep the cursor visible.
    The maximum length is 256 characters.
  - Enter or keypad Enter applies the text. Empty text clears the filter.
  - The examples are the ones PROC-073 lists. Patterns are unanchored searches with
    smart case (PROC-072).
  - If the pattern is invalid (`04-processes.md` defines the syntax), the popup stays
    open, keeps the text, and shows the error on the line below the field in the `ERROR`
    role. (Glances closes the popup and silently clears the filter.)
  - Esc closes the popup and keeps the earlier filter.

### 8.3 Kill popup (`k`)

- **UI-104** Content, for a process:

```
Kill process: <name> (pid <pid>)?

[t/y] terminate (SIGTERM)   [K] kill (SIGKILL)   [n/Esc] cancel
```

  In program view, the first line is `Kill program: <name> (<count> processes)?` and the
  action applies to every process of the group (PROC-104).
  - Each key acts on a single keypress; no Enter is needed. `t` or `y` sends SIGTERM,
    the default action. `K` sends SIGKILL. `n` or Esc cancels. Any other key does
    nothing and the popup stays open.
  - On Windows, `t`, `y` and `K` all call `TerminateProcess`. Windows has no graceful
    equivalent, so the popup shows only `[t/y] terminate   [n/Esc] cancel`.
  - The popup captures the target identity (PID plus process start time) when it opens.
    Before signalling, ogle checks that the identity still matches, so a reused PID is
    never signalled. If it no longer matches, the status area (UI-114) shows
    `Process <pid> has already exited`.
  - ogle refuses to kill itself. Instead the status area shows
    `Refusing to kill ogle itself`.
  - A failure (for example, permission denied) shows the OS error text in the status
    area, for example `Cannot kill 1234: Operation not permitted`.
  (Glances always sends SIGKILL, accepts any answer starting with `y`, only logs errors,
  and does not check for PID reuse.)
- **UI-105** `k` with an empty process list, or while process collection is off, does
  nothing.

### 8.4 Renice feedback

- **UI-106** `+` and `-` have no confirmation popup. On failure ogle shows the OS error in
  the status area (UI-114), for example `Cannot renice 1234: Permission denied`.
  (Glances only logs the error.)

### 8.5 Status area

- **UI-107** Withdrawn. Info popups are replaced by the status area (UI-114).
- **UI-108** Only one popup is open at a time (filter popup or kill popup). The help
  screen is not a popup (UI-110). Status messages (UI-114) never open a popup and never
  wait for one to close.
- **UI-114** Status message area. One-line messages from actions (kill and renice
  results, PROC-103, PROC-105, PROC-110; refused toggles such as sparklines in ASCII
  mode, SYS-096) are shown in the status area:
  - The status area is the last screen row. While a message is active, it is drawn
    across the full width over whatever the layout placed on that row, in the `ERROR`
    role for failures and the `INFO` role for other notices. The text is cut at the
    right edge (splittable).
  - A message stays for 3 seconds. The main-loop wait (UI-025) also wakes at the
    message's expiry, then redraws without it. Expiry does not start a new cycle.
  - A key press does not clear the message early and is not consumed; it runs its
    normal action.
  - A new message replaces the current one and restarts the 3 seconds.
  - Messages are shown on the main screen, under open popups, and on the "Terminal too
    small" screen (UI-071). They are not shown while the help screen is open; a
    message that expires during help is dropped.
  - While the UI runs, messages are never written to stderr. (Glances shows info popups
    that block its loop for 3 seconds.)

### 8.6 Help screen (`h`)

- **UI-110** The help screen replaces the whole frame (it is not a box). Content, top to
  bottom:
  1. `ogle <version>`, on the left, in the `TITLE` role.
  2. `Configuration file: <path>`, or `Configuration file: none`.
  3. A blank line.
  4. The key table, generated from the dispatch table (UI-040). Bindings are grouped in
     the order Sort, Processes, Display, Modules, Alerts, Other. Each line is
     `<key>  <label>`, where the key is left-padded to a common width. Toggles show their
     current state after the label as `[on]` or `[off]`. The groups are laid out in
     two columns when `W >= 100`, else in one column. Reserved keys are not listed.
  5. A blank line.
  6. A colour legend: one sample line per alert role (`OK`, `CAREFUL`, `WARNING`,
     `CRITICAL` and their `_LOG` variants), each drawn in its own role.
  7. The bottom row: `h/q/Esc: close   Up/Down/PgUp/PgDn: scroll`.
- **UI-111** If the content is taller than the screen, Up, Down, PageUp, PageDown, Home
  and End scroll it.
- **UI-112** `h`, `q` and Esc close the help screen and return to the main screen. They do
  not quit ogle. Ctrl-C still quits. (In Glances, `q` and Esc on the help screen quit the
  program.)
- **UI-113** Sampling continues while help is open (UI-101), so toggle states and the
  frame behind are current when help closes.

---

## 9. Glances quirks fixed

Informative summary. The normative text is in the requirement named in each item.

| Glances behaviour | Source | ogle |
|---|---|---|
| Raw codes 65/66/67/68 treated as arrows, so `A`, `B`, `C`, `D` also move the cursor or reset the timer | `G/outputs/glances_curses.py:289-290,1346` | Real escape-sequence parser. Unknown sequences dropped whole (UI-034) |
| After a key press, the wait uses `Timer.get()`, which returns the **elapsed** time, so the cycle runs longer than the refresh interval | `G/outputs/glances_curses.py:1302-1362`, `G/timer.py:46-47` | One fixed deadline per cycle; waits use the remaining time (UI-025, UI-026) |
| Polls for input every 100 ms (`napms(100)`) | `G/outputs/glances_curses.py:1302-1362` | Blocks on input with the remaining timeout (UI-025) |
| No resize handler; relies on curses side effects | `G/outputs/glances_curses.py` | SIGWINCH / console resize events, debounced redraw (UI-029 to UI-031) |
| Keypad Enter not mapped; no PageUp/PageDown/Home/End | `G/outputs/glances_curses.py:41-105,277-293` | Mapped (UI-032, UI-035, section 5.2) |
| Cursor capped at index 48 (`max_processes - 2`), index-based | `G/outputs/glances_curses.py:414-420`, `G/processes.py:247-254` | Whole list reachable, scrolls, selection follows the process (UI-046) |
| Help screen does not match the bindings (`r`, `4`, `0` wrong; several keys missing) | `G/plugins/help/__init__.py:62-143` | Help generated from the dispatch table (UI-040, UI-110) |
| `q`/Esc on the help screen quit the program | `G/outputs/glances_curses.py:422-430,1351-1354` | They close help (UI-112) |
| Shift-arrows cycle only cpu, mem, user | `G/processes.py:33` | Cycle over all sort keys (UI-047) |
| `--sort-processes` accepts only 3 keys | `G/processes.py:33` | `--sort` accepts every key (section 1.2) |
| Popup silently skipped when the screen is too small | `G/outputs/glances_curses.py:982-1075` | Popup clamped; minimum screen size enforced (UI-071, UI-100) |
| Text-box popups and info popups block the whole loop (info: 3 s `napms`) | `G/outputs/glances_curses.py:1077-1114` | Popups modal for input only; notices go to a non-blocking status area (UI-101, UI-114) |
| Bad filter regex silently clears the filter | `G/filter.py:141-145` | Error shown, popup stays open (UI-103) |
| Kill always SIGKILL, any `y…` answer, no PID-reuse check, errors only logged | `G/outputs/glances_curses.py:671-699`, `G/processes.py:785-791` | TERM or KILL choice, identity check, visible errors (UI-104) |
| Renice errors only logged | `G/processes.py:763-783` | Status-area message (UI-106, UI-114) |
| Pinning disables the cursor | `G/outputs/glances_curses.py:350-356` | Cursor stays active (UI-046) |
| Toggling `4`/`5` off re-enables every module, even ones the user had disabled | `G/outputs/glances_curses.py:336-348,442-461` | Restores the earlier per-module state (UI-044) |
| Non-splittable overflow drops one item, later items may still draw | `G/outputs/glances_curses.py:1250-1253` | Rest of the line dropped (UI-067) |
| `CAREFUL_LOG` has no background on 8-colour terminals | `G/outputs/glances_colors.py:77-122` | White on blue everywhere (UI-087) |
| `--disable-bold` also removes bold in monochrome, where it carries meaning | `G/outputs/glances_colors.py:31,124-141` | Bold kept in monochrome (UI-093) |
| Separator grey via `init_color`, which changes the user's palette | `G/outputs/glances_colors.py:100-110` | Fixed 256-colour index 244; the palette is never modified (UI-083) |
| `-t 2` cannot override a config `refresh` | `G/main.py:717-728` | The CLI always wins over the config (UI-002) |
| Right column x position depends on the widest drawn sidebar panel | `G/outputs/glances_curses.py:951` | Fixed at `SW + 3` (UI-061) |
