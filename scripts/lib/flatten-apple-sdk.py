#!/usr/bin/env python3
"""Unpack a MacOSX*.sdk tarball into a real directory.

Darwin framework aliases (Versions/Current, Headers -> ...) become copies so
the tree can live on Windows, where NTFS cannot store those symlinks and some
SDK filenames (Tcl man pages with ':') are illegal.
"""
from __future__ import annotations

import os
import shutil
import sys
import tarfile
from pathlib import Path, PurePosixPath

INVALID_WIN_CHARS = set('<>:"/\\|?*')


def rel_of(name: str, sdk_prefix: str):
    prefix = sdk_prefix.rstrip("/") + "/"
    if name.rstrip("/") == sdk_prefix.rstrip("/"):
        return ""
    if name.startswith(prefix):
        return name[len(prefix) :]
    return None


def find_sdk_prefix(members) -> str:
    for member in members:
        parts = PurePosixPath(member.name).parts
        for index, part in enumerate(parts):
            if part.startswith("MacOSX") and part.endswith(".sdk"):
                return str(PurePosixPath(*parts[: index + 1]))
    raise SystemExit("archive did not contain a MacOSX*.sdk directory")


def win_legal(rel: str) -> bool:
    for part in PurePosixPath(rel).parts:
        if not part or part in (".", ".."):
            continue
        if part.endswith(" ") or part.endswith("."):
            return False
        if any(ch in INVALID_WIN_CHARS or ord(ch) < 32 for ch in part):
            return False
    return True


def posix_join(rel: str, linkname: str) -> Path:
    parent = PurePosixPath(rel).parent
    resolved = parent / linkname
    parts = []
    for part in resolved.parts:
        if part == "..":
            if parts:
                parts.pop()
        elif part not in (".", ""):
            parts.append(part)
    return Path(*parts) if parts else Path()


def looks_like_apple_sdk(root: Path) -> bool:
    if not (root / "SDKSettings.json").is_file() and not (root / "SDKSettings.plist").is_file():
        return False
    return all(
        (root / relative).is_dir()
        for relative in ("usr/include", "usr/lib", "System/Library/Frameworks")
    )


def resync_version_current(dest: Path) -> None:
    for versions in dest.rglob("Versions"):
        if not versions.is_dir():
            continue
        current = versions / "Current"
        for name in ("A", "B"):
            source = versions / name
            if source.is_dir() and current.is_dir():
                shutil.copytree(source, current, symlinks=False, dirs_exist_ok=True)
                break


def resolve_pass(dest: Path, remaining, files_only: bool):
    next_remaining = []
    progressed = 0
    for rel, linkname in remaining:
        link_path = dest / rel.replace("/", os.sep)
        src = dest / posix_join(rel, linkname)
        if link_path.exists() and (not link_path.is_dir() or any(link_path.iterdir())):
            progressed += 1
            continue
        if not src.exists():
            next_remaining.append((rel, linkname))
            continue
        if files_only and src.is_dir():
            next_remaining.append((rel, linkname))
            continue
        link_path.parent.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            if link_path.exists():
                shutil.copytree(src, link_path, symlinks=False, dirs_exist_ok=True)
            else:
                shutil.copytree(src, link_path, symlinks=False)
        else:
            shutil.copy2(src, link_path)
        progressed += 1
    return next_remaining, progressed


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: flatten-apple-sdk.py <MacOSX*.sdk.tar.xz> <dest-dir>")
    archive = Path(sys.argv[1])
    dest = Path(sys.argv[2])
    restrict_names = os.name == "nt"
    if dest.exists():
        raise SystemExit(f"destination already exists: {dest}")
    dest.mkdir(parents=True)

    symlinks = []
    files = 0
    dirs = 0
    skipped = 0
    with tarfile.open(archive, "r:xz") as tf:
        members = tf.getmembers()
        sdk_prefix = find_sdk_prefix(members)
        print(f"sdk prefix: {sdk_prefix}", flush=True)
        for member in members:
            rel = rel_of(member.name, sdk_prefix)
            if rel is None:
                continue
            if rel and restrict_names and not win_legal(rel):
                skipped += 1
                continue
            target = dest / rel.replace("/", os.sep) if rel else dest
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                dirs += 1
            elif member.issym() or member.islnk():
                if rel:
                    symlinks.append((rel, member.linkname))
            elif member.isfile():
                try:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    source = tf.extractfile(member)
                    if source is None:
                        continue
                    with open(target, "wb") as out:
                        shutil.copyfileobj(source, out)
                    files += 1
                except OSError as error:
                    skipped += 1
                    print(f"skip file {rel}: {error}", flush=True)
            else:
                print(f"skip {member.name} type={member.type!r}", flush=True)

    print(
        f"extracted files={files} dirs={dirs} aliases={len(symlinks)} skipped={skipped}",
        flush=True,
    )

    remaining = list(symlinks)
    for pass_no in range(1, 30):
        if not remaining:
            break
        files_only = pass_no <= 12
        next_remaining, progressed = resolve_pass(dest, remaining, files_only)
        print(
            f"alias pass {pass_no}: resolved={progressed} remaining={len(next_remaining)}",
            flush=True,
        )
        if next_remaining == remaining:
            if files_only:
                continue
            break
        remaining = next_remaining

    resync_version_current(dest)
    leftover = []
    for rel, linkname in remaining:
        link_path = dest / rel.replace("/", os.sep)
        if not link_path.exists():
            leftover.append((rel, linkname))
    remaining = leftover

    private = "System/Library/PrivateFrameworks/"
    blocking = [item for item in remaining if not item[0].startswith(private)]
    if blocking:
        print("unresolved aliases:", len(blocking), flush=True)
        for rel, linkname in blocking[:20]:
            print(f"  {rel} -> {linkname}", flush=True)
        raise SystemExit(1)
    if remaining:
        print(
            f"ignoring {len(remaining)} unresolved PrivateFrameworks aliases",
            flush=True,
        )

    if not looks_like_apple_sdk(dest):
        raise SystemExit(f"staged tree is not an Apple SDK: {dest}")
    print(f"staged Apple SDK at {dest}", flush=True)


if __name__ == "__main__":
    main()
