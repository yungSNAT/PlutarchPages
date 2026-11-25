#!/usr/bin/env bash
# Repackage a PlutarchPages Linux tarball as a single-file Flatpak bundle (delegates to packaging/unix_package.rs).
# Usage: packaging/linux/flatpak-bundle.sh [--no-test] [TARBALL]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
run_unix_packager flatpak "$@"
