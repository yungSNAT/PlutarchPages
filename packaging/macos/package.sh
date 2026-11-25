#!/usr/bin/env bash
# Build, sign and (optionally) notarize macOS release artifacts (delegates to packaging/unix_package.rs).
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
run_unix_packager macos "$@"
