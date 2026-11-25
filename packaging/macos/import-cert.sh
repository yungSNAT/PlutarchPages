#!/usr/bin/env bash
# CI only: import the Developer ID certificate into a temporary keychain (delegates to packaging/unix_package.rs).
set -euo pipefail
umask 077
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
run_unix_packager import-cert "$@"
