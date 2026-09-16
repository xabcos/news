#!/usr/bin/env python3
"""Read-only verification of Along identity archives and restored backup copies.

Usage: python3 ops/along-archive-verify.py /absolute/archive/root [--companion UUID]
No image, person description, or uploaded filename is printed. The checker never
repairs or deletes anything. Preserve 0700 directories and 0600 files on restore.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
import uuid

MAX_MANIFEST = 1024 * 1024
MAX_IMAGE = 64 * 1024 * 1024
MANIFEST_KEYS = {"schema_version", "companion_id", "identity", "accessories", "assets"}
ASSET_KEYS = {
    "id", "role", "ordinal", "content_type", "image_content_type", "original_file",
    "image_file", "sha256", "image_sha256", "original_size", "image_size",
}


class InvalidArchive(ValueError):
    pass


def canonical_uuid(value):
    if not isinstance(value, str) or str(uuid.UUID(value)) != value:
        raise InvalidArchive("invalid canonical UUID")
    return value


def check_directory(path, private=True):
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode):
        raise InvalidArchive("directory is missing, not a directory, or a symlink")
    if private and stat.S_IMODE(metadata.st_mode) != 0o700:
        raise InvalidArchive("directory permission must be 0700")


def check_root(path):
    if not path.is_absolute() or path.parent == path or ".." in path.parts:
        raise InvalidArchive("use a dedicated absolute archive directory")
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current /= part
        check_directory(current, private=False)
    check_directory(path)


def open_private_file(path, max_size):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o600:
        raise InvalidArchive("file must be a regular private 0600 file, never a symlink")
    if metadata.st_size > max_size:
        raise InvalidArchive("file exceeds recorded size")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino) != (metadata.st_dev, metadata.st_ino):
            raise InvalidArchive("file changed during verification")
        return os.fdopen(descriptor, "rb")
    except BaseException:
        os.close(descriptor)
        raise


def read_small(path, max_size):
    with open_private_file(path, max_size) as stream:
        data = stream.read(max_size + 1)
    if len(data) > max_size:
        raise InvalidArchive("file exceeds allowed size")
    return data


def verify_image(path, expected_size, expected_hash):
    if type(expected_size) is not int or not 1 <= expected_size <= MAX_IMAGE:
        raise InvalidArchive("invalid image size")
    if not isinstance(expected_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_hash):
        raise InvalidArchive("invalid SHA-256 format")
    digest = hashlib.sha256()
    size = 0
    with open_private_file(path, expected_size) as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
            size += len(block)
            if size > expected_size:
                raise InvalidArchive("image is larger than recorded")
    if size != expected_size or digest.hexdigest() != expected_hash:
        raise InvalidArchive("image size or SHA-256 does not match")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise InvalidArchive("duplicate JSON field")
        result[key] = value
    return result


def verify_archive(root, companion_id):
    companion_id = canonical_uuid(companion_id)
    directory = root / companion_id
    check_directory(directory)
    encoded = read_small(directory / "manifest.json", MAX_MANIFEST)
    expected_checksum = (hashlib.sha256(encoded).hexdigest() + "\n").encode("ascii")
    if read_small(directory / "manifest.sha256", 65) != expected_checksum:
        raise InvalidArchive("manifest SHA-256 does not match")
    manifest = json.loads(encoded, object_pairs_hook=unique_object)
    if not isinstance(manifest, dict) or set(manifest) != MANIFEST_KEYS:
        raise InvalidArchive("invalid manifest fields")
    if type(manifest["schema_version"]) is not int or manifest["schema_version"] != 1:
        raise InvalidArchive("unsupported archive version")
    if manifest["companion_id"] != companion_id:
        raise InvalidArchive("manifest companion ID does not match directory")
    if not isinstance(manifest["identity"], dict) or not isinstance(manifest["accessories"], list):
        raise InvalidArchive("invalid identity or accessories")
    assets = manifest["assets"]
    if not isinstance(assets, list) or not 1 <= len(assets) <= 8:
        raise InvalidArchive("invalid asset count")
    expected_files = {"manifest.json", "manifest.sha256"}
    positions = set()
    for asset in assets:
        if not isinstance(asset, dict) or set(asset) != ASSET_KEYS:
            raise InvalidArchive("invalid asset fields")
        asset_id = canonical_uuid(asset["id"])
        role, ordinal = asset["role"], asset["ordinal"]
        if role not in ("source", "portrait") or type(ordinal) is not int or not 0 <= ordinal <= 32767:
            raise InvalidArchive("invalid asset role or ordinal")
        if (role, ordinal) in positions:
            raise InvalidArchive("duplicate asset position")
        positions.add((role, ordinal))
        for key in ("content_type", "image_content_type"):
            mime = asset[key]
            if not isinstance(mime, str) or len(mime) > 80 or not re.fullmatch(r"image/[A-Za-z0-9+.-]+", mime):
                raise InvalidArchive("invalid image content type")
        for suffix, filename_key, size_key, hash_key in (
            ("original", "original_file", "original_size", "sha256"),
            ("image", "image_file", "image_size", "image_sha256"),
        ):
            filename = f"{asset_id}.{suffix}"
            if asset[filename_key] != filename or filename in expected_files:
                raise InvalidArchive("invalid or duplicate asset filename")
            expected_files.add(filename)
            verify_image(directory / filename, asset[size_key], asset[hash_key])
    if {entry.name for entry in directory.iterdir()} != expected_files:
        raise InvalidArchive("archive has missing or unrecorded files")
    return len(assets)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--companion", help="Verify a single canonical companion UUID")
    arguments = parser.parse_args()
    try:
        check_root(arguments.directory)
        if arguments.companion:
            companions = [canonical_uuid(arguments.companion)]
        else:
            companions = []
            for entry in arguments.directory.iterdir():
                if entry.name.startswith(".tmp-"):
                    # Not published: never treat a partially written staging
                    # directory as a restored or confirmed identity archive.
                    print(f"SKIP unpublished staging directory: {entry.name}", file=sys.stderr)
                    continue
                companions.append(canonical_uuid(entry.name))
            companions.sort()
        failures = 0
        for companion_id in companions:
            try:
                count = verify_archive(arguments.directory, companion_id)
                print(f"OK {companion_id}: {count} assets, identity and all hashes verified")
            except (OSError, ValueError, TypeError, KeyError) as error:
                failures += 1
                print(f"FAIL {companion_id}: {error}", file=sys.stderr)
        print(f"Verified {len(companions)} archives; failures: {failures}")
        return 1 if failures else 0
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"FAIL archive root: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
