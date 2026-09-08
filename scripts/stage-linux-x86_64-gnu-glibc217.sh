#!/bin/sh
# Compatibility wrapper: linux x86_64 gnu glibc 2.17 sysroot.
set -eu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$script_directory/stage-linux-gnu-glibc217.sh" x86_64 "$@"
