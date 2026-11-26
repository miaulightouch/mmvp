"""Prototype: patch one TW bundle offline and verify the result.

Usage: python tools/proto_one.py <tw_package> <tw_blob> <tw_bundle_idx>
Writes work/proto/<blob>_<idx>.bundle and decoded WAVs of the replaced clips.
"""
import csv
import os
import sys

import UnityPy

sys.path.insert(0, os.path.dirname(__file__))
from blob import read_blob_table  # noqa: E402
from patch_bundle import container_map, patch_bundle, read_clips  # noqa: E402

TW_ROOT = r"D:\MabinogiMobile_TW\appdata"
JP_ROOT = r"D:\Nexon\MabinogiMobile_JP"


def slice_bundle(path, idx):
    s, e = read_blob_table(path)[int(idx)]
    with open(path, "rb") as f:
        f.seek(s)
        return f.read(e - s)


def main(pkg, blob, idx):
    rows = [r for r in csv.DictReader(open("work/port_map.tsv", encoding="utf-8"), delimiter="\t")
            if (r["tw_package"], r["tw_blob"], r["tw_bundle"]) == (pkg, blob, idx)]
    wanted = {r["container_path"] for r in rows}
    jp_clips = {}
    for (jb, ji) in {(r["jp_blob"], r["jp_bundle"]) for r in rows}:
        jp_clips.update(read_clips(slice_bundle(os.path.join(JP_ROOT, jb), ji), wanted))
    print(f"wanted {len(wanted)}  jp clips loaded {len(jp_clips)}")

    tw = slice_bundle(os.path.join(TW_ROOT, pkg, blob), idx)
    new, n = patch_bundle(tw, jp_clips)
    print(f"replaced {n}  size {len(tw)} -> {len(new)}")
    os.makedirs("work/proto", exist_ok=True)
    out = f"work/proto/{blob}_{idx}.bundle"
    open(out, "wb").write(new)

    # verify: reload, every replaced clip must byte-match JP and decode
    env = UnityPy.load(new)
    cont = container_map(env)
    ok = bad = 0
    check = read_clips(new, wanted)
    for path, c in check.items():
        if c.data == jp_clips[path].data and c.fields["m_Length"] == jp_clips[path].fields["m_Length"]:
            ok += 1
        else:
            bad += 1
            print("MISMATCH", path)
    for obj in env.objects:
        if obj.type.name == "AudioClip" and cont.get(obj.path_id) in wanted:
            for name, wav in obj.read().samples.items():
                fn = name if name.lower().endswith(".wav") else name + ".wav"
                open(os.path.join("work/proto", fn), "wb").write(wav)
            break
    print(f"verify ok {ok} bad {bad}")


if __name__ == "__main__":
    main(*sys.argv[1:])
