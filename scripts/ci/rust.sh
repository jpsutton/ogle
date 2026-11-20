#!/usr/bin/env bash
# CI helper: installs a minimal stable Rust toolchain with rustup (distribution
# packages are too old on Debian and Rocky) and the C compiler cargo links with.
# Source it: `. scripts/ci/rust.sh`. Works in fedora, rockylinux, debian, ubuntu
# and archlinux containers, as root.
set -euo pipefail

if command -v dnf >/dev/null; then
  dnf -y -q install gcc curl ca-certificates rpm-build >/dev/null
elif command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq >/dev/null
  apt-get install -y -qq --no-install-recommends gcc libc6-dev curl ca-certificates dpkg-dev >/dev/null
elif command -v pacman >/dev/null; then
  pacman -Syu --noconfirm --needed base-devel git rustup >/dev/null
fi

if ! command -v cargo >/dev/null || ! cargo --version >/dev/null 2>&1; then
  if command -v rustup >/dev/null; then
    rustup default stable >/dev/null
  else
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
fi
cargo --version
