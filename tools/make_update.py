"""Write the update.json the installer polls (mmvoice.json "update_url" points at it).

Usage: python tools/make_update.py <pack.mmvpack> <pack_download_url> <out update.json>
                                   [--app-version X.Y.Z --app-url URL]
pack.id is the SHA1 of manifest.json inside the pack (what the installer shows and compares);
pack.sha1 is the SHA1 of the whole file (checked after download).
"""
import argparse
import hashlib
import json
import os
import zipfile


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pack")
    ap.add_argument("url")
    ap.add_argument("out")
    ap.add_argument("--app-version")
    ap.add_argument("--app-url")
    a = ap.parse_args()
    with zipfile.ZipFile(a.pack) as z:
        raw = z.read("manifest.json")
    man = json.loads(raw)
    h = hashlib.sha1()
    with open(a.pack, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    out = {"pack": {"id": hashlib.sha1(raw).hexdigest(), "created": man.get("created", ""), "url": a.url,
                    "size": os.path.getsize(a.pack), "sha1": h.hexdigest()}}
    if a.app_version and a.app_url:
        out["app"] = {"version": a.app_version, "url": a.app_url}
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)
    print(json.dumps(out, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
