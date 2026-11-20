#!/usr/bin/env bash
# Installs the latest ogle release on macOS (Apple silicon or Intel):
#
#   curl -fsSL https://raw.githubusercontent.com/jpsutton/ogle/release/scripts/install-macos.sh | bash
#
# The binary is ad-hoc signed but not notarized by Apple, so a file downloaded in a
# browser gets the quarantine attribute and Gatekeeper blocks the first run. curl
# doesn't set that attribute, so the binary this script installs runs without a prompt.
#
# The download is checked against the release's SHA256SUMS. Installs to /usr/local/bin,
# or ~/.local/bin when /usr/local/bin isn't writable. Set OGLE_INSTALL_DIR to choose
# another folder. Run it again to update.
set -euo pipefail

base=https://github.com/jpsutton/ogle/releases/latest/download

die() { echo "install-macos: $*" >&2; exit 1; }

[[ $(uname -s) == Darwin ]] || die "this script is for macOS"

if [[ -n ${OGLE_INSTALL_DIR:-} ]]; then
  dest=$OGLE_INSTALL_DIR
elif [[ -w /usr/local/bin ]]; then
  dest=/usr/local/bin
else
  dest=$HOME/.local/bin
fi
mkdir -p "$dest"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS"
tarball=$(grep -oE 'ogle-[0-9][^ ]*-macos-universal\.tar\.gz$' "$work/SHA256SUMS" | head -n 1) \
  || die "no macOS build in the latest release"

echo "Downloading $tarball"
curl -fL --progress-bar -o "$work/$tarball" "$base/$tarball"
(cd "$work" && grep " $tarball\$" SHA256SUMS | shasum -a 256 -c -s) || die "checksum mismatch for $tarball"

tar -xzf "$work/$tarball" -C "$work"
bin=$(find "$work" -path '*/ogle-*/ogle' -type f | head -n 1)
[[ -n $bin ]] || die "ogle not found in $tarball"

install -m 755 "$bin" "$dest/ogle"
echo "Installed $dest/ogle"
case ":$PATH:" in
  *":$dest:"*) ;;
  *) echo "Add $dest to your PATH to run ogle." ;;
esac
