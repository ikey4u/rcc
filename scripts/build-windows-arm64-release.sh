#!/bin/sh
# Windows aarch64 MSVC-hosted controller. Same pack matrix as the x64 host.
set -eu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RCC_WINDOWS_HOST=aarch64-pc-windows-msvc
export RCC_WINDOWS_HOST
exec "$script_directory/build-windows-x64-release.sh" "$@"
