#!/bin/sh
# Compile the official SQLite amalgamation with rcc cc for one profile.
# SQLite is not in the repo: the zip is pinned by
# toolchains/sqlite-amalgamation-3500400.lock.json and fetched into .cache.
#
# usage:
#   verify-sqlite.sh <profile>
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <profile>" >&2
    exit 64
fi

profile=$1
repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
lock=$repository/toolchains/sqlite-amalgamation-3500400.lock.json
cache_dir=${RCC_CACHE_DIR:-$repository/inner/rcc-cache}
archive_cache=${RCC_ARCHIVE_CACHE:-$repository/.cache}

if [ ! -f "$lock" ]; then
    echo "missing $lock" >&2
    exit 66
fi

eval "$(python3 -c '
import json, pathlib, shlex, sys

lock_path = pathlib.Path(sys.argv[1])
lock = json.loads(lock_path.read_text())
if lock.get("schema_version") != 1:
    sys.exit("unsupported lock schema_version")
for key in ("name", "source", "sha256"):
    value = lock.get(key)
    if not isinstance(value, str) or not value:
        sys.exit("lock missing " + key)
sha256 = lock["sha256"]
if len(sha256) != 64 or any(c not in "0123456789abcdef" for c in sha256):
    sys.exit("lock sha256 must be 64 lowercase hex chars")
print("name=" + shlex.quote(lock["name"]))
print("url=" + shlex.quote(lock["source"]))
print("sha256=" + shlex.quote(sha256))
' "$lock")"

if [ -z "${RCC:-}" ]; then
    if [ -x "$repository/dist/rcc-release/rcc" ]; then
        RCC=$repository/dist/rcc-release/rcc
    elif [ -x "$repository/dist/rcc-release/rcc.exe" ]; then
        RCC=$repository/dist/rcc-release/rcc.exe
    elif [ -x "$repository/target/release/rcc" ]; then
        RCC=$repository/target/release/rcc
    elif [ -x "$repository/target/release/rcc.exe" ]; then
        RCC=$repository/target/release/rcc.exe
    else
        echo "set RCC to a release rcc with $profile in payload" >&2
        exit 69
    fi
fi
if [ ! -x "$RCC" ]; then
    echo "RCC is not executable: $RCC" >&2
    exit 69
fi

file_sha256() {
    path=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$path" | awk '{print $1}'
        return 0
    fi
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$path" | awk '{print $1}'
        return 0
    fi
    echo "sha256sum or shasum is required" >&2
    exit 69
}

source_root=$archive_cache/sqlite-amalgamation-3500400/$sha256
out_dir=$archive_cache/verify-sqlite/$profile
mkdir -p "$archive_cache" "$source_root" "$out_dir" "$cache_dir"

archive=${RCC_SQLITE_ARCHIVE:-}
if [ -z "$archive" ] && [ -f "$archive_cache/$name" ]; then
    archive=$archive_cache/$name
fi
if [ -z "$archive" ] || [ ! -f "$archive" ]; then
    echo "fetching $name into $archive_cache"
    extra=
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*) extra=--ssl-no-revoke ;;
    esac
    # shellcheck disable=SC2086
    curl -L --fail $extra --retry 20 --retry-delay 3 --retry-all-errors \
        --speed-limit 1000 --speed-time 30 -o "$archive_cache/$name" "$url"
    archive=$archive_cache/$name
fi
actual=$(file_sha256 "$archive")
if [ "$actual" != "$sha256" ]; then
    echo "$name digest mismatch: expected $sha256, got $actual" >&2
    exit 65
fi

amalgamation=$source_root/sqlite-amalgamation-3500400
if [ ! -f "$amalgamation/sqlite3.c" ] || [ ! -f "$amalgamation/sqlite3.h" ]; then
    echo "extracting $name into $source_root"
    if command -v unzip >/dev/null 2>&1; then
        unzip -qo "$archive" -d "$source_root"
    else
        python3 -c '
import zipfile, sys
zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])
' "$archive" "$source_root"
    fi
fi
if [ ! -f "$amalgamation/sqlite3.c" ] || [ ! -f "$amalgamation/sqlite3.h" ]; then
    echo "amalgamation extract is missing sqlite3.c/sqlite3.h" >&2
    exit 65
fi

main_c=$out_dir/main.c
cat > "$main_c" <<'EOF'
#include "sqlite3.h"

#include <stdio.h>
#include <stdlib.h>

int main(void) {
    sqlite3 *database = NULL;
    char *error = NULL;
    sqlite3_stmt *statement = NULL;
    const unsigned char *name = NULL;

    if (sqlite3_open(":memory:", &database) != SQLITE_OK) {
        fprintf(stderr, "open failed: %s\n", sqlite3_errmsg(database));
        sqlite3_close(database);
        return 1;
    }
    if (sqlite3_exec(
            database,
            "CREATE TABLE artifacts (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
            NULL,
            NULL,
            &error
        )
        != SQLITE_OK)
    {
        fprintf(stderr, "create failed: %s\n", error ? error : sqlite3_errmsg(database));
        sqlite3_free(error);
        sqlite3_close(database);
        return 1;
    }
    if (sqlite3_exec(
            database,
            "INSERT INTO artifacts (name) VALUES ('rcc-ok');",
            NULL,
            NULL,
            &error
        )
        != SQLITE_OK)
    {
        fprintf(stderr, "insert failed: %s\n", error ? error : sqlite3_errmsg(database));
        sqlite3_free(error);
        sqlite3_close(database);
        return 1;
    }
    if (sqlite3_prepare_v2(database, "SELECT name FROM artifacts", -1, &statement, NULL)
        != SQLITE_OK)
    {
        fprintf(stderr, "prepare failed: %s\n", sqlite3_errmsg(database));
        sqlite3_close(database);
        return 1;
    }
    if (sqlite3_step(statement) != SQLITE_ROW) {
        fprintf(stderr, "select failed: %s\n", sqlite3_errmsg(database));
        sqlite3_finalize(statement);
        sqlite3_close(database);
        return 1;
    }
    name = sqlite3_column_text(statement, 0);
    printf("sqlite=%s\n", name ? (const char *)name : "");
    sqlite3_finalize(statement);
    sqlite3_close(database);
    return 0;
}
EOF

export RCC
export RCC_CACHE_DIR=$cache_dir

echo "==> rcc doctor ($profile) sqlite amalgamation"
"$RCC" --cache-dir "$cache_dir" doctor --profile "$profile"

echo "==> compile sqlite3.c"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -I "$amalgamation" -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_THREADSAFE=1 \
    -c "$amalgamation/sqlite3.c" -o "$out_dir/sqlite3.o"
echo "==> compile main.c"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -I "$amalgamation" -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_THREADSAFE=1 \
    -c "$main_c" -o "$out_dir/main.o"

bin=$out_dir/sqlite-verify
case "$profile" in
    windows-*) bin=$out_dir/sqlite-verify.exe ;;
esac

echo "==> link sqlite-verify"
set -- "$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    "$out_dir/main.o" "$out_dir/sqlite3.o" -o "$bin"
case "$profile" in
    linux-*)
        set -- "$@" -ldl -lpthread -lm
        ;;
esac
"$@"

echo "==> rcc verify sqlite ($profile)"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$bin"

expect_sqlite_ok() {
    runner=$1
    output=$2
    echo "$output"
    case "$output" in
        *sqlite=rcc-ok*) ;;
        *)
            echo "$runner output did not contain sqlite=rcc-ok: $output" >&2
            exit 1
            ;;
    esac
}

lima_running() {
    instance=$1
    lima_config=$HOME/.lima/$instance/ssh.config
    [ -f "$lima_config" ] || return 1
    limactl list 2>/dev/null | awk -v name="$instance" '
        $1 == name && $2 == "Running" { found = 1 }
        END { exit !found }
    '
}

run_lima() {
    instance=$1
    copy_tmp=$2
    lima_config=$HOME/.lima/$instance/ssh.config
    echo "==> lima $instance $bin"
    if [ "$copy_tmp" = 1 ]; then
        remote=/tmp/rcc-run-$(basename "$bin")
        scp -q -o ControlMaster=no -o ControlPath=none -F "$lima_config" \
            "$bin" "lima-$instance:$remote"
        output=$(ssh -o ControlMaster=no -o ControlPath=none -F "$lima_config" \
            "lima-$instance" -- "chmod 755 $remote && exec $remote")
    else
        output=$(ssh -o ControlMaster=no -o ControlPath=none -F "$lima_config" \
            "lima-$instance" -- "$bin")
    fi
    expect_sqlite_ok "lima $instance" "$output"
}

ran=0
case "$profile" in
    macos-aarch64)
        if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
            echo "==> run $bin"
            expect_sqlite_ok native "$("$bin")"
            ran=1
        fi
        ;;
    macos-x86_64)
        if [ "$(uname -s)" = Darwin ]; then
            if [ "$(uname -m)" = x86_64 ]; then
                echo "==> run $bin"
                expect_sqlite_ok native "$("$bin")"
                ran=1
            elif [ "$(uname -m)" = arm64 ] \
                && [ -x /usr/bin/arch ] \
                && /usr/bin/arch -x86_64 /usr/bin/true >/dev/null 2>&1
            then
                echo "==> run $bin"
                expect_sqlite_ok native "$("$bin")"
                ran=1
            fi
        fi
        ;;
    linux-x86_64-musl-static)
        if [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ]; then
            echo "==> native $bin"
            expect_sqlite_ok native "$("$bin")"
            ran=1
        elif command -v qemu-x86_64 >/dev/null 2>&1; then
            echo "==> qemu-user $bin"
            expect_sqlite_ok qemu-x86_64 "$(qemu-x86_64 "$bin")"
            ran=1
        elif command -v qemu-x86_64-static >/dev/null 2>&1; then
            echo "==> qemu-user $bin"
            expect_sqlite_ok qemu-x86_64-static "$(qemu-x86_64-static "$bin")"
            ran=1
        elif lima_running "${LIMA_INSTANCE:-rcc-x64}"; then
            run_lima "${LIMA_INSTANCE:-rcc-x64}" 0
            ran=1
        fi
        ;;
    linux-x86_64-gnu-glibc217)
        if [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ]; then
            echo "==> native $bin"
            expect_sqlite_ok native "$("$bin")"
            ran=1
        elif command -v qemu-x86_64 >/dev/null 2>&1; then
            echo "==> qemu-user $bin"
            expect_sqlite_ok qemu-x86_64 "$(qemu-x86_64 "$bin")"
            ran=1
        elif command -v qemu-x86_64-static >/dev/null 2>&1; then
            echo "==> qemu-user $bin"
            expect_sqlite_ok qemu-x86_64-static "$(qemu-x86_64-static "$bin")"
            ran=1
        elif lima_running "${LIMA_GLIBC_INSTANCE:-rcc-x64-glibc217}"; then
            run_lima "${LIMA_GLIBC_INSTANCE:-rcc-x64-glibc217}" 1
            ran=1
        fi
        ;;
    linux-aarch64-musl-static)
        if [ "$(uname -s)" = Linux ]; then
            case "$(uname -m)" in
                aarch64|arm64)
                    echo "==> native $bin"
                    expect_sqlite_ok native "$("$bin")"
                    ran=1
                    ;;
            esac
        fi
        if [ "$ran" -eq 0 ] && command -v qemu-aarch64 >/dev/null 2>&1; then
            echo "==> qemu-user $bin"
            expect_sqlite_ok qemu-aarch64 "$(qemu-aarch64 "$bin")"
            ran=1
        elif [ "$ran" -eq 0 ] && command -v qemu-aarch64-static >/dev/null 2>&1; then
            echo "==> qemu-user $bin"
            expect_sqlite_ok qemu-aarch64-static "$(qemu-aarch64-static "$bin")"
            ran=1
        elif [ "$ran" -eq 0 ] && lima_running "${LIMA_INSTANCE:-rcc-arm64}"; then
            run_lima "${LIMA_INSTANCE:-rcc-arm64}" 0
            ran=1
        fi
        ;;
    windows-x86_64-gnu|windows-x86_64-gnullvm)
        if command -v wine64 >/dev/null 2>&1; then
            echo "==> wine64 $bin"
            expect_sqlite_ok wine64 "$(wine64 "$bin")"
            ran=1
        fi
        ;;
esac
if [ "$ran" -eq 0 ]; then
    echo "runtime checks were skipped for $profile"
    if [ "${RCC_REQUIRE_RUNTIME:-}" = 1 ]; then
        echo "RCC_REQUIRE_RUNTIME=1 requires a runnable host for $profile" >&2
        exit 1
    fi
fi

echo "sqlite amalgamation $profile compile+verify ok"
