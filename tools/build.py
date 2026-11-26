"""Build patched TW blobs with JP voice, in parallel. Never writes to the game installs.

Usage: python tools/build.py [--jobs N] [--only AssetPackages/0517.blob ...]
Reads work/port_map.tsv, writes out/<package>/<blob> (same relative layout as the TW install)
plus out/build_report.tsv.
"""
import argparse
import csv
import os
import struct
import sys
import time
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(__file__))
from blob import BLOB_MAGIC, read_blob_table  # noqa: E402

TW_ROOT = r"D:\MabinogiMobile_TW\appdata"
JP_ROOT = r"D:\Nexon\MabinogiMobile_JP"


def write_blob(src_path, dst_path, replacements):
    """Copy src blob to dst, swapping bundle slices given in replacements {idx: bytes}."""
    table = read_blob_table(src_path)
    hdr_end = 8 + 4 * len(table)
    ends, pos = [], hdr_end
    with open(src_path, "rb") as f:
        parts = []
        for i, (s, e) in enumerate(table):
            if i in replacements:
                data = replacements[i]
            else:
                f.seek(s)
                data = f.read(e - s)
            parts.append(data)
            pos += len(data)
            ends.append(pos)
    os.makedirs(os.path.dirname(dst_path), exist_ok=True)
    tmp = dst_path + ".tmp"
    with open(tmp, "wb") as w:
        w.write(struct.pack("<II", BLOB_MAGIC, hdr_end))
        w.write(struct.pack(f"<{len(ends)}I", *ends))
        for p in parts:
            w.write(p)
    os.replace(tmp, dst_path)
    return len(table)


def build_one(pkg, blob, rows):
    from patch_bundle import patch_bundle, read_clips  # worker-side import
    from proto_one import slice_bundle

    src = os.path.join(TW_ROOT, pkg, blob)
    by_bundle = defaultdict(list)
    for r in rows:
        by_bundle[int(r["tw_bundle"])].append(r)

    # load needed JP clips once per JP bundle
    jp_cache = {}
    replacements, report = {}, []
    for idx, rs in sorted(by_bundle.items()):
        wanted = {r["container_path"] for r in rs}
        jp_clips = {}
        for key in {(r["jp_blob"], r["jp_bundle"]) for r in rs}:
            if key not in jp_cache:
                jp_cache[key] = read_clips(slice_bundle(os.path.join(JP_ROOT, key[0]), key[1]))
            jp_clips.update({k: v for k, v in jp_cache[key].items() if k in wanted})
        new, n = patch_bundle(slice_bundle(src, idx), jp_clips)
        replacements[idx] = new
        report.append((pkg, blob, idx, len(wanted), n))
    write_blob(src, os.path.join("out", pkg, blob), replacements)
    return report


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    ap.add_argument("--only", nargs="*")
    a = ap.parse_args()

    groups = defaultdict(list)
    with open("work/port_map.tsv", encoding="utf-8") as f:
        for r in csv.DictReader(f, delimiter="\t"):
            groups[(r["tw_package"], r["tw_blob"])].append(r)
    if a.only:
        keep = {tuple(x.replace("\\", "/").rsplit("/", 1)) for x in a.only}
        groups = {k: v for k, v in groups.items() if k in keep}

    t0, total = time.time(), len(groups)
    print(f"{total} blobs, {a.jobs} workers", file=sys.stderr, flush=True)
    with ProcessPoolExecutor(a.jobs) as ex, open("out/build_report.tsv", "w", encoding="utf-8") as rep:
        rep.write("package\tblob\tbundle\twanted\treplaced\n")
        futs = {ex.submit(build_one, pkg, blob, rows): (pkg, blob) for (pkg, blob), rows in groups.items()}
        for n, fut in enumerate(as_completed(futs), 1):
            for row in fut.result():
                rep.write("\t".join(map(str, row)) + "\n")
            print(f"{n}/{total}  {time.time() - t0:.0f}s  {'/'.join(futs[fut])}", file=sys.stderr, flush=True)


if __name__ == "__main__":
    os.makedirs("out", exist_ok=True)
    main()
