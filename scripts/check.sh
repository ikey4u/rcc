#!/bin/sh
# Workspace quality gate: tests, clippy, script/lock metadata, and
# (when a release rcc is present) the product verify scripts.
# rustfmt --check is mise task format:check (nightly), run by `mise check`.
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository"

resolve_rcc() {
    if [ -n "${RCC:-}" ]; then
        if [ -x "$RCC" ]; then
            return 0
        fi
        echo "RCC is not executable: $RCC" >&2
        exit 69
    fi
    for candidate in \
        "$repository/dist/rcc-release/rcc" \
        "$repository/dist/rcc-release/rcc.exe" \
        "$repository/dist/bin/rcc" \
        "$repository/dist/bin/rcc.exe" \
        "$repository/target/release/rcc" \
        "$repository/target/release/rcc.exe"
    do
        if [ -x "$candidate" ]; then
            RCC=$candidate
            return 0
        fi
    done
    return 1
}

default_rcc_home() {
    if [ -n "${RCC_HOME_DIR:-}" ]; then
        printf '%s\n' "$RCC_HOME_DIR"
        return 0
    fi
    case "$(uname -s)" in
        Darwin)
            printf '%s\n' "$HOME/Library/Application Support/rcc"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            printf '%s\n' "${LOCALAPPDATA:-$HOME/AppData/Local}/rcc"
            ;;
        *)
            printf '%s\n' "${XDG_DATA_HOME:-$HOME/.local/share}/rcc"
            ;;
    esac
}

apple_sdk_ready() {
    [ -n "${RCC_APPLE_SDK_ROOT:-}" ] && return 0
    home=$(default_rcc_home)
    [ -d "$home/vendor/macos/usr/include" ] && return 0
    case "$(uname -s)" in
        Darwin) return 0 ;;
    esac
    return 1
}

profile_in_payload() {
    profile=$1
    "$RCC" targets | awk -v id="$profile" '$1 == id && $5 == "yes" { found = 1 } END { exit !found }'
}

doctor_binds() {
    profile=$1
    "$RCC" --cache-dir "${RCC_CACHE_DIR:-$repository/inner/rcc-cache}" \
        doctor --profile "$profile" >/dev/null 2>&1
}

verify_windows_if_ready() {
    profile=$1
    if ! profile_in_payload "$profile"; then
        echo "skipping $profile verify (not in payload)"
        return 0
    fi
    if ! doctor_binds "$profile"; then
        echo "skipping $profile verify (Windows SDK / MSVC toolset for this arch is missing)"
        return 0
    fi
    "$repository/scripts/verify-windows.sh" "$profile"
}

verify_sqlite_if_ready() {
    profile=$1
    if ! profile_in_payload "$profile"; then
        echo "skipping sqlite $profile (not in payload)"
        return 0
    fi
    if ! doctor_binds "$profile"; then
        echo "skipping sqlite $profile (doctor cannot bind profile / SDK)"
        return 0
    fi
    "$repository/scripts/verify-sqlite.sh" "$profile"
}

echo "==> cargo test"
cargo test --workspace --locked

echo "==> clippy"
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings -D clippy::string_slice

echo "==> metadata"
"$repository/scripts/check-metadata.sh"

if resolve_rcc; then
    export RCC
    echo "==> product verify ($RCC)"
    "$repository/scripts/verify-linux-x86_64-musl.sh"
    "$repository/scripts/verify-linux-x86_64-gnu.sh"
    "$repository/scripts/verify-linux-aarch64-musl.sh"
    "$repository/scripts/verify-linux-aarch64-gnu.sh"
    "$repository/scripts/verify-windows.sh" windows-x86_64-gnu
    "$repository/scripts/verify-windows.sh" windows-x86_64-gnullvm
    "$repository/scripts/verify-windows.sh" windows-aarch64-gnullvm
    verify_windows_if_ready windows-x86_64-msvc
    verify_windows_if_ready windows-aarch64-msvc
    if apple_sdk_ready; then
        "$repository/scripts/verify-macos.sh" macos-aarch64
        if profile_in_payload macos-x86_64; then
            "$repository/scripts/verify-macos.sh" macos-x86_64
        fi
    else
        echo "skipping macos verify (set RCC_APPLE_SDK_ROOT, run scripts/setup-env.sh, or use macOS xcrun)"
    fi
    echo "==> sqlite amalgamation"
    for sqlite_profile in \
        linux-x86_64-musl-static \
        linux-x86_64-gnu-glibc217 \
        linux-aarch64-musl-static \
        linux-aarch64-gnu-glibc217 \
        windows-x86_64-gnu \
        windows-x86_64-gnullvm \
        windows-aarch64-gnullvm \
        windows-x86_64-msvc \
        windows-aarch64-msvc \
        macos-aarch64 \
        macos-x86_64
    do
        verify_sqlite_if_ready "$sqlite_profile"
    done
else
    echo "skipping product verify (no release rcc; set RCC or mise release)"
fi
