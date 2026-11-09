#!/usr/bin/env bash
# Copyright 2026 Ryan Duguid
# SPDX-License-Identifier: MIT OR Apache-2.0

# Repackage a PlutarchPages Linux tarball as an Arch Linux package:
#
#   $DIST/pdfcraft-<version>-linux-<arch>.pkg.tar.zst
#
# Usage: packaging/linux/arch-package.sh <x86_64|aarch64> [TARBALL]
#
# TARBALL defaults to $DIST/pdfcraft-<version>-linux-<arch>.tar.gz from package.sh. Needs makepkg
# and rustc, or a native helper at PDFCRAFT_ARCH_HELPER. Nothing in the tarball runs here, so an
# x86_64 machine can package aarch64. On another distribution run this in an archlinux:base-devel
# container with a statically linked helper. makepkg refuses root, so this script re-runs itself
# as an unprivileged user when necessary.
#
# The package is `plutarchpages`, not upstream's `ai.storyteller.pdfcraft`. Both install
# /usr/bin/pdfcraft, so it provides and conflicts with `pdfcraft`, which upstream's provides too.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

ARCH="${1:?usage: arch-package.sh <x86_64|aarch64> [TARBALL]}"
case "$ARCH" in
  x86_64 | aarch64) ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
TARBALL="${2:-$DIST/pdfcraft-$VERSION-linux-$ARCH.tar.gz}"
[[ -f "$TARBALL" ]] || { echo "error: $TARBALL not found (run packaging/linux/package.sh --formats tar first)" >&2; exit 1; }

if [[ "$(id -u)" = 0 ]]; then
  id -u builder >/dev/null 2>&1 || useradd -m builder
  chmod 777 "$DIST"
  exec runuser -u builder -- env HOME=/home/builder bash "${BASH_SOURCE[0]}" "$@"
fi

work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT

helper="${PDFCRAFT_ARCH_HELPER:-$work/arch-package}"
if [[ -z "${PDFCRAFT_ARCH_HELPER:-}" ]]; then
  rustc --edition=2024 -D warnings "$ROOT/packaging/linux/arch-package.rs" -o "$helper"
fi
"$helper" "$ARCH" "$TARBALL" "$work"
