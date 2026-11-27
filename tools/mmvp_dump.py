"""Dump the JP client's voices into the static site the patcher downloads from (Cloudflare Pages).

Usage: python tools/mmvp_dump.py <jp_root> [--repo OWNER/REPO] [--out work/pages] [--date YYYY-MM-DD] [--jobs N]

Writes <out>/<date>/:
  latest.json     {"version": 3, "date", "manifest", "manifest_sha1", "clips", "bytes"}
  manifest.json   {"version": 3, "filter", "clips": {sha1: {"size", "fields"}},
                   "bundles": {bundle name: {container path: sha1}}, "default": {container path: sha1}}
                  content only: identical voices give an identical SHA1 (the voice version id)
  clips/<sha1>    raw FSB5 audio of every distinct JP clip
  _headers        Cloudflare Pages cache rules
  index.html      landing page from tools/site/index.html (with --repo)
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import sys
import time
from concurrent.futures import ProcessPoolExecutor

PACKAGES = ["AssetPackages", "AssetPackages/Starter", "ResourcesPackage", "ResourcesPackage/Starter"]
VOICE = re.compile(r"(/voice/|_vo_)", re.I)
FIELDS = ("m_LoadType", "m_Channels", "m_Frequency", "m_BitsPerSample", "m_Length",
          "m_IsTrackerFormat", "m_Ambisonic", "m_SubsoundIndex", "m_CompressionFormat")
HEADERS = """/clips/*
  Cache-Control: public, max-age=31536000, immutable
  Content-Type: application/octet-stream
/*.json
  Cache-Control: no-cache
/
  Cache-Control: no-cache
/index.html
  Cache-Control: no-cache
"""


def slices(path):
    """Yield the UnityFS bundles of a .blob (u32 magic, u32 header end, u32 cumulative end offsets)."""
    with open(path, "rb") as f:
        data = f.read()
    hdr_end = struct.unpack_from("<I", data, 4)[0]
    start = hdr_end
    for (end,) in struct.iter_unpack("<I", data[8:hdr_end]):
        if data.startswith(b"UnityFS", start):
            yield data[start:end]
        start = end


def dump_blob(path):
    """-> [(bundle name, container path, sha1)], {sha1: (fields, raw audio)}"""
    import UnityPy
    from UnityPy.helpers.ResourceReader import get_resource_data
    refs, clips = [], {}
    for raw in slices(path):
        env = UnityPy.load(raw)
        audio = [o for o in env.objects if o.type.name == "AudioClip"]
        if not audio:
            continue
        name, paths = "", {}
        for o in env.objects:
            if o.type.name == "AssetBundle":
                t = o.read_typetree()
                name = t["m_Name"]
                for path, info in t["m_Container"]:
                    if info["asset"]["m_FileID"] == 0:
                        paths.setdefault(info["asset"]["m_PathID"], path)
        for o in audio:
            path = paths.get(o.path_id)
            if not path or not VOICE.search(path):
                continue
            t = o.read_typetree()
            r = t["m_Resource"]
            data = bytes(get_resource_data(r["m_Source"], o.assets_file, r["m_Offset"], r["m_Size"]))
            h = hashlib.sha1(data).hexdigest()
            refs.append((name, path, h))
            clips.setdefault(h, ({k: t[k] for k in FIELDS if k in t}, data))
    return refs, clips


def write_index(site, latest, repo):
    url = repo if repo.startswith("https://") else f"https://github.com/{repo.strip('/')}"
    with open(os.path.join(os.path.dirname(__file__), "site", "index.html"), encoding="utf-8") as f:
        html = f.read()
    for k, v in {"REPO_URL": url.rstrip("/"), "DATE": latest["date"], "CLIPS": f"{latest['clips']:,}",
                 "MB": f"{latest['bytes'] / 2**20:.0f}", "ID": latest["manifest_sha1"]}.items():
        html = html.replace("{{" + k + "}}", v)
    with open(os.path.join(site, "index.html"), "w", encoding="utf-8", newline="\n") as f:
        f.write(html)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("root")
    ap.add_argument("--repo", help="GitHub OWNER/REPO linked from index.html")
    ap.add_argument("--out", default=os.path.join("work", "pages"))
    ap.add_argument("--date", default=time.strftime("%Y-%m-%d"))
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    a = ap.parse_args()

    blobs = sorted(os.path.join(a.root, pkg, n) for pkg in PACKAGES if os.path.isdir(os.path.join(a.root, pkg))
                   for n in os.listdir(os.path.join(a.root, pkg)) if re.fullmatch(r"\d{4}\.blob", n))
    t0 = time.time()
    with ProcessPoolExecutor(a.jobs) as ex:
        results = list(ex.map(dump_blob, blobs, chunksize=4))  # in blob order: deterministic output
    site = os.path.join(a.out, a.date)
    shutil.rmtree(site, ignore_errors=True)
    os.makedirs(os.path.join(site, "clips"))
    clips, bundles, votes, total = {}, {}, {}, 0
    for refs, payloads in results:
        for h, (fields, data) in payloads.items():
            if h not in clips:
                with open(os.path.join(site, "clips", h), "wb") as f:
                    f.write(data)
                clips[h] = {"size": len(data), "fields": fields}
                total += len(data)
        for name, path, h in refs:
            bundles.setdefault(name, {})[path] = h
            votes.setdefault(path, {}).setdefault(h, 0)
            votes[path][h] += 1
    manifest = {"version": 3, "filter": VOICE.pattern, "clips": dict(sorted(clips.items())),
                "default": {p: max(c, key=c.get) for p, c in sorted(votes.items())},
                "bundles": {k: dict(sorted(v.items())) for k, v in sorted(bundles.items())}}
    raw = json.dumps(manifest, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    with open(os.path.join(site, "manifest.json"), "wb") as f:
        f.write(raw)
    latest = {"version": 3, "date": a.date, "manifest": "manifest.json", "manifest_sha1": hashlib.sha1(raw).hexdigest(),
              "clips": len(clips), "bytes": total}
    with open(os.path.join(site, "latest.json"), "w", encoding="utf-8") as f:
        json.dump(latest, f, indent=1)
    with open(os.path.join(site, "_headers"), "w", encoding="utf-8", newline="\n") as f:
        f.write(HEADERS)
    if a.repo:
        write_index(site, latest, a.repo)
    print(f"{len(blobs)} blobs, {len(votes)} paths, {len(clips)} clips ({total / 2**20:.1f} MB), {len(bundles)} bundles "
          f"in {time.time() - t0:.0f}s -> {site}", file=sys.stderr)
    if len(clips) + 4 > 20000:
        sys.exit("Cloudflare Pages allows at most 20000 files per deployment")


if __name__ == "__main__":
    main()
