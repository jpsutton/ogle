# ogle

ogle is a terminal system monitor inspired by [Glances](https://github.com/nicolargo/glances).
It shows CPU, memory, swap, load, network, disk I/O, file systems, sensors and processes on one
screen, raises alerts when values cross thresholds, and lets you sort, filter, renice and kill
processes. It uses a small fraction of the CPU and memory that Glances needs.

Measured on Linux (32 CPUs, about 750 processes, 200x50 terminal, 2 s refresh):

| | Glances 4.5.9 | ogle |
|---|---|---|
| CPU, % of one core | 4.99% | 0.47% |
| Memory (RSS) | 74 MiB | 4.3 MiB |
| Read syscalls per refresh | 7305 | 816 |
| Time to first screen | 608 ms | 2 ms |

ogle is a single native binary with no runtime dependencies. It is written in Rust.

## Status

ogle is new. Linux is the primary platform and is tested. The macOS, FreeBSD, OpenBSD, NetBSD
and Windows backends are written but not yet tested on real systems. [GAPS.md](GAPS.md) lists
known gaps.

## Install

Packages are attached to each [GitHub release](https://github.com/jpsutton/ogle/releases/latest),
signed with the ogle release key (`ogle-signing-key.asc` in the release).

### Arch Linux

ogle is in the signed `[jpsutton]` pacman repository. See
[jpsutton/arch-repo](https://github.com/jpsutton/arch-repo) for setup, then:

```sh
sudo pacman -Sy ogle
```

### Fedora, Rocky Linux

Download the `.rpm` for your release (`fc*` for Fedora, `el9` for Rocky Linux and other
EL9 systems) and install it:

```sh
sudo dnf install ./ogle-*.rpm
```

### Debian, Ubuntu

Download the `.deb` for your release (`debian12` or `ubuntu24.04`) and install it:

```sh
sudo apt install ./ogle_*.deb
```

### macOS

The easiest way to install or update is the install script. It downloads the latest universal
binary (Apple silicon and Intel), checks it against the release's `SHA256SUMS`, and installs it
into `/usr/local/bin` (or `~/.local/bin` when that isn't writable):

```sh
curl -fsSL https://raw.githubusercontent.com/jpsutton/ogle/release/scripts/install-macos.sh | bash
```

Files downloaded with `curl` aren't quarantined, so ogle runs without a Gatekeeper prompt. Run
the same command again to update. Set `OGLE_INSTALL_DIR` to install somewhere else.

If you download the tarball in a browser instead, macOS refuses to run the binary the first
time. Clear the quarantine from a terminal:

```sh
xattr -d com.apple.quarantine /usr/local/bin/ogle
```

### Windows

Download `ogle-*-windows-x86_64.zip` from the release and run `ogle.exe` in Windows Terminal.

### From source

```sh
cargo build --release
./target/release/ogle
```

## Usage

```sh
ogle                # start with a 2 s refresh
ogle -t 1           # refresh every second
ogle --percpu       # per-CPU view
ogle --help         # every option
```

Press `h` inside ogle for the full key list. Common keys:

| Key | Action |
|---|---|
| `q` / `Esc` | Quit |
| `c` `m` `i` `t` `p` `u` `a` | Sort processes by CPU, memory, I/O, time, name, user, or automatically |
| `Enter` / `E` | Edit / clear the process filter |
| `Up` / `Down` / `PgUp` / `PgDn` | Move the selection |
| `k` | Kill the selected process (asks first) |
| `e` | Pin the selected process and show extended details |
| `j` | Group processes by program |
| `1` | Per-CPU mode |
| `w` / `x` | Clear finished alerts |

## Configuration

ogle reads `~/.config/ogle/config.toml` (`%APPDATA%\ogle\config.toml` on Windows). To see every
option with its default:

```sh
ogle --print-default-config
```

## Specification

The behaviour is specified in [spec/](spec/README.md), including the differences from Glances
and the performance budgets.

## License

LGPL-3.0-or-later, the same license as Glances. See [LICENSE](LICENSE).
