#!/usr/bin/env bash
# Build the browser version and zip it (delegates to packaging/unix_package.rs).
# Usage: packaging/web/package.sh [--skip-build]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
run_unix_packager web ${1+"$@"}
