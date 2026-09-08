#!/bin/sh
# Compatibility wrapper: linux x86_64 musl sysroot.
set -eu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$script_directory/stage-linux-musl.sh" x86_64 "$@"
