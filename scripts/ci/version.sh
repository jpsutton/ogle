#!/usr/bin/env bash
# Prints the release version: `version` from Cargo.toml.
# Fails if packaging/arch/PKGBUILD's pkgver disagrees, so a release can't ship
# packages with two different versions.
set -euo pipefail
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
pkgver=$(sed -n 's/^pkgver=//p' packaging/arch/PKGBUILD)
[[ -n $version ]] || { echo "version.sh: no version in Cargo.toml" >&2; exit 1; }
if [[ $version != "$pkgver" ]]; then
  echo "version.sh: Cargo.toml has $version but packaging/arch/PKGBUILD has $pkgver" >&2
  exit 1
fi
echo "$version"
