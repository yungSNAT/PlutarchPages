#!/usr/bin/env bash
# Build and package PlutarchPages for FreeBSD (delegates to packaging/unix_package.rs).
# Usage: packaging/freebsd/package.sh [--skip-build]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
run_unix_packager freebsd ${1+"$@"}
