#!/usr/bin/env bash
# Build and package PlutarchPages for Linux (delegates to packaging/unix_package.rs).
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar cli"]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
run_unix_packager linux "$@"
