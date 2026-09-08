#!/usr/bin/env python3
"""Carve the embedded .rccpack out of a release rcc executable."""
import json
import struct
import sys

MAGIC = b"RCCPACK\0"
MAX_MANIFEST = 64 * 1024 * 1024


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <rcc-executable> <output.rccpack>", file=sys.stderr)
        return 64
    source, destination = sys.argv[1], sys.argv[2]
    data = open(source, "rb").read()
    candidates = []
    start = 0
    while True:
        index = data.find(MAGIC, start)
        if index < 0:
            break
        if index + 20 <= len(data):
            version, manifest_len = struct.unpack_from("<IQ", data, index + 8)
            manifest_end = index + 20 + manifest_len
            if (
                version == 2
                and 0 < manifest_len <= MAX_MANIFEST
                and manifest_end <= len(data)
            ):
                try:
                    manifest = json.loads(data[index + 20 : manifest_end])
                    payload = sum(int(file["compressed_len"]) for file in manifest["files"])
                    end = manifest_end + payload
                    if end <= len(data) and manifest.get("profiles"):
                        candidates.append((index, end, manifest.get("pack_id"), list(manifest["profiles"])))
                except (ValueError, KeyError, TypeError, json.JSONDecodeError):
                    pass
        start = index + 1
    if not candidates:
        print("no embedded RCC pack found", file=sys.stderr)
        return 65
    candidates.sort(key=lambda item: item[1] - item[0], reverse=True)
    index, end, pack_id, profiles = candidates[0]
    open(destination, "wb").write(data[index:end])
    print(f"wrote {destination} ({end - index} bytes, pack_id={pack_id})")
    print("profiles: " + ", ".join(sorted(profiles)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
