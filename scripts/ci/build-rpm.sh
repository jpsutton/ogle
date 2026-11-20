#!/usr/bin/env bash
# CI (build): the .rpm for the Fedora or Rocky Linux release this runs on, written to out/.
#   docker run --rm -v "$PWD:/src" -w /src fedora:latest scripts/ci/build-rpm.sh
# Built on each target release so the binary links against that release's glibc;
# rpmbuild adds the matching library requirements and the dist tag (fc41, el9, ...).
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=scripts/ci/rust.sh
. scripts/ci/rust.sh

version=$(scripts/ci/version.sh)
cargo build --release --locked

top=$(mktemp -d)
mkdir -p "$top/SOURCES"
cp target/release/ogle README.md LICENSE "$top/SOURCES/"
rpmbuild -bb --quiet \
  --define "_topdir $top" \
  --define "ogle_version $version" \
  packaging/rpm/ogle.spec

mkdir -p out
find "$top/RPMS" -name '*.rpm' -exec cp {} out/ \;
ls -la out
