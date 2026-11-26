"""Scan a game install's *.blob packages and list every AudioClip (multi-process).

Usage: python tools/scan_audio.py <install_root> <out.tsv> [--jobs N] [--glob PATTERN]
Each blob is scanned by its own worker process; results are merged into one TSV.
Output TSV columns:
  package  blob  bundle_idx  cab  container_path  clip_name  length_s  channels  freq  load_type  res_size
"""
import argparse
import glob
import os
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(__file__))
from blob import iter_bundles  # noqa: E402

PACKAGES = ["AssetPackages", "AssetPackages/Starter", "ResourcesPackage",
            "ResourcesPackage/Starter", "PlatformResourcePackages"]
HEADER = "package\tblob\tbundle_idx\tcab\tcontainer_path\tclip_name\tlength_s\tchannels\tfreq\tload_type\tres_size\n"


def scan_blob(pkg, bp):
    import UnityPy  # imported in worker
    rows, errs = [], []
    try:
        for i, _start, data in iter_bundles(bp):
            if not data.startswith(b"UnityFS"):
                continue  # non-bundle slice (obfuscated gzip-like)
            # cheap pre-filter: AudioClips always reference a .resS / .resource stream
            try:
                env = UnityPy.load(data)
            except Exception as e:  # noqa: BLE001
                errs.append(f"[load-fail] {bp}#{i}: {e}")
                continue
            clips = [o for o in env.objects if o.type.name == "AudioClip"]
            if not clips:
                continue
            cont = {}
            for path, ptr in env.container.items():
                try:
                    cont[ptr.path_id] = path
                except Exception:  # noqa: BLE001
                    pass
            cab = ",".join(env.files.keys())
            for obj in clips:
                c = obj.read()
                res = getattr(c, "m_Resource", None)
                rows.append("\t".join(str(x) for x in (
                    pkg, os.path.basename(bp), i, cab,
                    cont.get(obj.path_id, ""), c.m_Name,
                    getattr(c, "m_Length", ""), getattr(c, "m_Channels", ""),
                    getattr(c, "m_Frequency", ""), getattr(c, "m_LoadType", ""),
                    getattr(res, "m_Size", "") if res else "")) + "\n")
    except Exception as e:  # noqa: BLE001
        errs.append(f"[blob-fail] {bp}: {e!r}")
    return bp, rows, errs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("out")
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    ap.add_argument("--glob", default="*.blob")
    a = ap.parse_args()

    tasks = [(pkg, bp) for pkg in PACKAGES
             for bp in sorted(glob.glob(os.path.join(a.root, pkg, a.glob)))]
    total, t0 = len(tasks), time.time()
    print(f"{total} blobs, {a.jobs} workers", file=sys.stderr, flush=True)
    log = open(a.out + ".log", "w", encoding="utf-8")
    with open(a.out, "w", encoding="utf-8", newline="\n") as w, \
            ProcessPoolExecutor(a.jobs) as ex:
        w.write(HEADER)
        futs = [ex.submit(scan_blob, pkg, bp) for pkg, bp in tasks]
        for n, f in enumerate(as_completed(futs), 1):
            bp, rows, errs = f.result()
            w.writelines(rows)
            for e in errs:
                log.write(e + "\n")
            if n % 20 == 0 or n == total:
                print(f"{n}/{total} blobs  {time.time() - t0:.0f}s", file=sys.stderr, flush=True)
    log.close()


if __name__ == "__main__":
    main()
