"""Which exported file an imported asset came from, and whether that file changed since (import_map.py's textures and
Interchange assets, import_demo.py's shared textures).

Unreal records each imported asset's source file in its import data: the path, the file's timestamp and its MD5,
listed in the asset registry's AssetImportData tag, so an asset can be checked without loading it.
"""

import hashlib
import json
import os

FILE_MD5 = {}


def file_md5(path):
    """A file's MD5 (hex, as Unreal records it), computed once per run."""
    if path not in FILE_MD5:
        digest = hashlib.md5()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 22), b""):
                digest.update(chunk)
        FILE_MD5[path] = digest.hexdigest()
    return FILE_MD5[path]


def recorded(data):
    """The source files an asset (registry AssetData) records in its AssetImportData tag: one entry per file, with
    "RelativeFilename" (relative to the engine's binaries folder), "Timestamp" and "FileMD5"; [] when none."""
    try:
        entries = json.loads(str(data.get_tag_value("AssetImportData") or "[]"))
    except (ValueError, TypeError):
        return []
    return [entry for entry in entries if isinstance(entry, dict)] if isinstance(entries, list) else []


def imported_from(data, path):
    """Whether the asset (registry AssetData) was imported from the file at `path` as it is now, by the source
    timestamp and MD5 its import data records (no need to load the asset). None when nothing is recorded. The
    exporter rewrites every PNG on each export, so after a re-export the timestamps differ and the MD5 decides
    (de_inferno's 1099 PNGs, 2.6 GB: 6 s)."""
    entries = recorded(data)
    try:
        timestamp, digest = int(entries[0].get("Timestamp") or 0), entries[0].get("FileMD5") or ""
    except (ValueError, IndexError, TypeError):
        return None
    if not digest:
        return None
    return timestamp == int(os.path.getmtime(path)) or digest == file_md5(path)
