#!/bin/sh
# Copy release binaries from dist/bin into INSTALL_DIR.
#
# Destination follows GNU make install:
#   INSTALL_DIR  default ${CARGO_HOME:-$HOME/.cargo}/bin
#   DESTDIR      prepended: $(DESTDIR)$(INSTALL_DIR)
#
# Examples:
#   mise run release:install
#   INSTALL_DIR=/usr/local/bin mise run release:install
#   DESTDIR=/tmp/stage INSTALL_DIR=/usr/local/bin mise run release:install
set -eu

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)
bin_dir=$repository/dist/bin

if [ "${INSTALL_DIR+x}" = x ]; then
    if [ -z "$INSTALL_DIR" ]; then
        echo "INSTALL_DIR is set but empty" >&2
        exit 64
    fi
    install_dir=$INSTALL_DIR
else
    cargo_home=${CARGO_HOME:-${HOME:?HOME is unset}/.cargo}
    install_dir=$cargo_home/bin
fi

destination=${DESTDIR:-}$install_dir

for name in cargo-rcc rcc; do
    if [ ! -f "$bin_dir/$name" ]; then
        echo "missing $bin_dir/$name; run mise release first" >&2
        exit 66
    fi
    if [ ! -x "$bin_dir/$name" ]; then
        echo "not executable: $bin_dir/$name" >&2
        exit 65
    fi
done

mkdir -p "$destination"
install -m 755 "$bin_dir/cargo-rcc" "$destination/cargo-rcc"
install -m 755 "$bin_dir/rcc" "$destination/rcc"

echo "installed $destination/cargo-rcc"
echo "installed $destination/rcc"
