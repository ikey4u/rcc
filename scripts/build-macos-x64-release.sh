#!/bin/sh
# macOS x86_64 controller. On Apple Silicon this cross-compiles with the
# ARM64 LLVM bootstrap; on Intel it bootstraps from Xcode clang (LLVM 22.1.8
# does not ship a macOS x64 binary archive).
set -eu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RCC_MACOS_HOST=x86_64-apple-darwin
export RCC_MACOS_HOST
exec "$script_directory/build-macos-arm64-release.sh" "$@"
