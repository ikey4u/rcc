#!/bin/sh
# Linux aarch64 controller. Same pack matrix as the x86_64 Linux host.
set -eu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RCC_LINUX_HOST=aarch64-unknown-linux-gnu
export RCC_LINUX_HOST
exec "$script_directory/build-linux-x86_64-release.sh" "$@"
