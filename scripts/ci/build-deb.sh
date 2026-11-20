#!/usr/bin/env bash
# CI (build): the .deb for the Debian or Ubuntu release this runs on, written to out/.
#   docker run --rm -v "$PWD:/src" -w /src debian:12 scripts/ci/build-deb.sh
# Built on each target release so the binary links against that release's glibc.
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=scripts/ci/rust.sh
. scripts/ci/rust.sh

version=$(scripts/ci/version.sh)
. /etc/os-release
arch=$(dpkg --print-architecture)
# e.g. ogle_0.1.0-1~debian12_amd64.deb: the ~ suffix keeps one file per release apart.
debver="$version-1~$ID$VERSION_ID"

cargo build --release --locked

root=$(mktemp -d)
install -Dm755 target/release/ogle "$root/usr/bin/ogle"
install -Dm644 README.md "$root/usr/share/doc/ogle/README.md"
install -Dm644 LICENSE "$root/usr/share/doc/ogle/copyright"
glibc=$(dpkg-query -W -f='${Version}' libc6 | sed 's/-.*//')
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<EOF
Package: ogle
Version: $debver
Architecture: $arch
Maintainer: Jared Sutton <jpsutton@protonmail.com>
Depends: libc6 (>= $glibc)
Section: admin
Priority: optional
Homepage: https://github.com/jpsutton/ogle
Description: Resource-efficient terminal system monitor
 ogle shows CPU, memory, swap, load, network, disk I/O, file systems,
 sensors and processes on one screen, inspired by Glances, at a small
 fraction of its CPU and memory cost.
EOF

mkdir -p out
dpkg-deb --root-owner-group --build "$root" "out/ogle_${debver}_${arch}.deb"
ls -la out
