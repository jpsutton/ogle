#!/usr/bin/env bash
# CI (build): Arch package via packaging/arch/PKGBUILD, written to out/.
#   docker run --rm -v "$PWD:/src" -w /src archlinux:latest scripts/ci/build-arch.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
scripts/ci/version.sh >/dev/null

pacman -Syu --noconfirm --needed base-devel git rustup >/dev/null

# makepkg refuses to run as root.
id builder >/dev/null 2>&1 || useradd -m builder
chown -R builder: "$PWD"
su builder -c 'git config --global --add safe.directory "*" \
  && rustup default stable >/dev/null \
  && cd packaging/arch && makepkg -f --noconfirm'
mkdir -p out
mv packaging/arch/*.pkg.tar.zst out/
ls -la out
