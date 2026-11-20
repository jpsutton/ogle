#!/usr/bin/env bash
# CI (checks): the fast checks run on every pull request.
#   - shellcheck on every shell script;
#   - cargo check for the host (Linux), which catches compile errors in the
#     shared code and the Linux backend in well under a minute.
# The full build (tests, every package, other platforms) runs only on pull
# requests into release (build.yml).
set -euo pipefail
cd "$(dirname "$0")/../.."

mapfile -t scripts < <(git ls-files '*.sh')
shellcheck -x "${scripts[@]}"

cargo check --locked --all-targets
