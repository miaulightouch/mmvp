"""Dump JP voices into a static site for Cloudflare Pages (the patcher's download source).

Usage: python tools/dump_site.py <jp_root> [--out work/pages] [--date YYYY-MM-DD] [--filter REGEX] [--jobs N]

Writes <out>/<date>/ (a fresh directory; deploy it with tools/deploy_pages.ps1):
  latest.json                 {"version": 3, "date", "manifest": "manifest.json", "manifest_sha1", "clips", "bytes"}
  manifest.json               {"version": 3, "filter",   (content only: its SHA1 is the voice version id)
                               "clips":   {sha1: {"size": n, "fields": {AudioClip format fields}}},
                               "bundles": {bundle_name: {container_path: sha1}},
                               "default": {container_path: sha1}}
  clips/<sha1>                raw FSB5 bytes of every distinct JP clip (content-addressed, immutable)
  _headers                    Cloudflare Pages cache rules (clips immutable, json no-cache)
Nothing here is ever committed: the JP content stays on this machine and on Pages.
"""
import argparse
import hashlib
import json
import os
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from mmvoice import DEFAULT_FILTER, _dump_blob, list_blobs, run_parallel  # noqa: E402

HEADERS = """/clips/*
  Cache-Control: public, max-age=31536000, immutable
  Content-Type: application/octet-stream
/*.json
  Cache-Control: no-cache
  Content-Type: application/json; charset=utf-8
"""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("root")
    ap.add_argument("--out", default=os.path.join("work", "pages"))
    ap.add_argument("--date", default=time.strftime("%Y-%m-%d"))
    ap.add_argument("--filter", default=DEFAULT_FILTER, help="regex on container path")
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    a = ap.parse_args()

    site = os.path.join(a.out, a.date)
    if os.path.exists(site):
        shutil.rmtree(site)
    os.makedirs(os.path.join(site, "clips"))
    blobs = list_blobs(a.root)
    print(f"dump: {len(blobs)} blobs from {a.root}, filter {a.filter!r}", file=sys.stderr)
    results = sorted(run_parallel(_dump_blob, [(a.root, r, a.filter) for r in blobs], a.jobs, "dump"),
                     key=lambda r: r[0])
    clips, by_bundle, votes, total = {}, {}, {}, 0
    for _rel, refs, payloads in results:
        for h, (fields, data) in payloads.items():
            if h in clips:
                continue
            with open(os.path.join(site, "clips", h), "wb") as f:
                f.write(data)
            clips[h] = {"size": len(data), "fields": fields}
            total += len(data)
        for name, path, h in refs:
            by_bundle.setdefault(name, {})[path] = h
            votes.setdefault(path, {}).setdefault(h, 0)
            votes[path][h] += 1
    default = {p: max(c, key=c.get) for p, c in votes.items()}
    # no date / timestamp in the manifest: identical voices give an identical manifest SHA1 (= voice version id)
    manifest = {"version": 3, "filter": a.filter,
                "clips": dict(sorted(clips.items())), "default": dict(sorted(default.items())),
                "bundles": {k: dict(sorted(v.items())) for k, v in sorted(by_bundle.items())}}
    raw = json.dumps(manifest, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    with open(os.path.join(site, "manifest.json"), "wb") as f:
        f.write(raw)
    latest = {"version": 3, "date": a.date, "manifest": "manifest.json", "manifest_sha1": hashlib.sha1(raw).hexdigest(),
              "clips": len(clips), "bytes": total}
    with open(os.path.join(site, "latest.json"), "w", encoding="utf-8") as f:
        json.dump(latest, f, indent=1)
    with open(os.path.join(site, "_headers"), "w", encoding="utf-8", newline="\n") as f:
        f.write(HEADERS)
    n_files = len(clips) + 3
    print(f"dump: {len(default)} paths, {len(clips)} clips ({total / 2**20:.1f} MB), {len(by_bundle)} bundles, "
          f"{n_files} files -> {site}")
    if n_files > 20000:
        print("[warn] Cloudflare Pages allows at most 20000 files per deployment", file=sys.stderr)


if __name__ == "__main__":
    main()
