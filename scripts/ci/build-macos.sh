#!/usr/bin/env bash
# CI (build, macOS runner): a universal (arm64 + x86_64) ogle binary, packed as
# out/ogle-<version>-macos-universal.tar.gz for scripts/install-macos.sh.
set -euo pipefail
cd "$(dirname "$0")/../.."
version=$(scripts/ci/version.sh)

rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked --target "$t"
done

stage=$(mktemp -d)/ogle-$version
mkdir -p "$stage"
lipo -create -output "$stage/ogle" \
  target/aarch64-apple-darwin/release/ogle target/x86_64-apple-darwin/release/ogle
# Apple silicon refuses to run unsigned code; lipo keeps each slice's ad-hoc
# signature, but sign the result again so the file as a whole verifies.
codesign --force --sign - "$stage/ogle"
codesign --verify --strict "$stage/ogle"
cp README.md LICENSE "$stage/"

mkdir -p out
COPYFILE_DISABLE=1 tar -czf "out/ogle-$version-macos-universal.tar.gz" -C "$(dirname "$stage")" "ogle-$version"
ls -la out
