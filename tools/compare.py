"""Compare JP vs TW AudioClip inventories produced by scan_audio.py.

Usage: python tools/compare.py work/jp_audio.tsv work/tw_audio.tsv [out_map.tsv]
Prints a category summary; optionally writes the JP<->TW mapping for clips whose
audio differs (the porting candidates).
"""
import csv
import sys
from collections import Counter


def load(p):
    with open(p, encoding="utf-8") as f:
        return list(csv.DictReader(f, delimiter="\t"))


def cat(path, depth=4):
    parts = path.split("/")
    return "/".join(parts[2:2 + depth]) if len(parts) > 3 else (path or "<no container path>")


def is_voice(path):
    p = path.lower()
    return "/voice/" in p or "_vo_" in p or "/vo/" in p


def main(jp_p, tw_p, out=None):
    jp, tw = load(jp_p), load(tw_p)
    jpm = {r["container_path"]: r for r in jp if r["container_path"]}
    twm = {r["container_path"]: r for r in tw if r["container_path"]}
    print(f"clip rows  JP {len(jp)}  TW {len(tw)}  unique paths  JP {len(jpm)}  TW {len(twm)}"
          "  (same clip is often embedded in several bundles)")
    common = [k for k in twm if k in jpm]
    differ = [k for k in common if twm[k]["res_size"] != jpm[k]["res_size"]]
    print(f"common paths {len(common)}  differing audio {len(differ)}  TW-only {len(twm)-len(common)}  JP-only {len(jpm)-len(common)}")
    for title, keys in [("JP all", list(jpm)), ("differing (port candidates)", differ),
                        ("TW-only", [k for k in twm if k not in jpm]),
                        ("JP-only", [k for k in jpm if k not in twm])]:
        print(f"\n--- {title} by category (voice-like = path has /voice/ or _vo_)")
        c = Counter(cat(k) for k in keys)
        v = Counter(cat(k) for k in keys if is_voice(k))
        for k, n in c.most_common(30):
            print(f"{n:6d}  voice {v[k]:6d}  {k}")
    if out:
        with open(out, "w", encoding="utf-8", newline="\n") as w:
            w.write("container_path\tjp_blob\tjp_bundle\tjp_len\ttw_package\ttw_blob\ttw_bundle\ttw_len\n")
            # one line per TW copy: every bundle that embeds the clip must be patched
            dset = set(differ)
            for t in sorted((r for r in tw if r["container_path"] in dset),
                            key=lambda r: (r["container_path"], r["package"], r["blob"])):
                j = jpm[t["container_path"]]
                w.write("\t".join([t["container_path"], f'{j["package"]}/{j["blob"]}', j["bundle_idx"],
                                   j["length_s"], t["package"], t["blob"], t["bundle_idx"],
                                   t["length_s"]]) + "\n")


if __name__ == "__main__":
    main(*sys.argv[1:])
