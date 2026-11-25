# shellcheck shell=bash
# Shared setup for the packaging scripts. Source it: `. "$(dirname "$0")/../env.sh"`.
#
# Exports:
#   ROOT                    workspace root
#   VERSION                 [workspace.package] version from Cargo.toml (override: PDFCRAFT_VERSION)
#   PDFCRAFT_BUILD_LABEL    optional label for a local or patched build (letters, digits, dots)
#   DIST                    output directory for release artifacts (default: $ROOT/dist/release)
#   PDFCRAFT_BUILD_SHA      git commit hash
#   PDFCRAFT_BUILD_DATE     UTC build date, YYYY-MM-DD
#   CARGO_TARGET_DIR        cargo's target dir (default: $ROOT/target)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT

workspace_version() {
  awk '
    /^\[/ { in_pkg = ($0 == "[workspace.package]") ; next }
    in_pkg && $1 == "version" { gsub(/[" ]/, "", $3); print $3; exit }
  ' "$ROOT/Cargo.toml"
  return
}

VERSION="${PDFCRAFT_VERSION:-$(workspace_version)}"
if [[ -z "$VERSION" ]]; then
  echo "error: could not read [workspace.package] version from $ROOT/Cargo.toml" >&2
  exit 1
fi
export VERSION

if [[ -n "${PDFCRAFT_BUILD_LABEL:-}" ]]; then
  case "$PDFCRAFT_BUILD_LABEL" in
    *[!A-Za-z0-9.]* | .* | *.)
      echo "error: PDFCRAFT_BUILD_LABEL may hold only letters, digits and inner dots: $PDFCRAFT_BUILD_LABEL" >&2
      exit 1
      ;;
    *) ;;
  esac
  export PDFCRAFT_BUILD_LABEL
fi

DIST="${DIST:-$ROOT/dist/release}"
mkdir -p "$DIST"
export DIST

if [[ -z "${PDFCRAFT_BUILD_SHA:-}" ]]; then
  PDFCRAFT_BUILD_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)"
fi
export PDFCRAFT_BUILD_SHA
export PDFCRAFT_BUILD_DATE="${PDFCRAFT_BUILD_DATE:-$(date -u +%Y-%m-%d)}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

_PACKAGER_WORK=""
_cleanup_packager() {
  if [[ -n "${_PACKAGER_WORK:-}" ]]; then
    rm -rf "$_PACKAGER_WORK"
  fi
}

run_unix_packager() {
  local subcmd="$1"
  shift
  command -v rustc >/dev/null || {
    echo "error: rustc not found (install a stable Rust toolchain)" >&2
    exit 1
  }
  _PACKAGER_WORK="$(umask 077 && mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pdfcraft-packager.XXXXXXXX")"
  trap _cleanup_packager EXIT
  local bin="$_PACKAGER_WORK/unix_package"
  rustc --edition=2024 -D warnings "$ROOT/packaging/unix_package.rs" -o "$bin"
  local status=0
  "$bin" --root "$ROOT" "$subcmd" "$@" || status=$?
  rm -rf "$_PACKAGER_WORK"
  _PACKAGER_WORK=""
  trap - EXIT
  return "$status"
}
