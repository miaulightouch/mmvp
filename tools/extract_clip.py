"""Extract AudioClips from one blob bundle to WAV/raw.

Usage: python tools/extract_clip.py <blob> <bundle_idx> <out_dir> [name_substring]
Writes decoded audio (via UnityPy/FMOD). If the clip's data is inline, also dumps it as .fsb
(most clips stream from the bundle's .resS, so the .fsb dump is usually skipped).
"""
import os
import sys

import UnityPy

sys.path.insert(0, os.path.dirname(__file__))
from blob import read_blob_table  # noqa: E402


def main(blob, idx, out_dir, needle=""):
    idx = int(idx)
    s, e = read_blob_table(blob)[idx]
    with open(blob, "rb") as f:
        f.seek(s)
        data = f.read(e - s)
    env = UnityPy.load(data)
    os.makedirs(out_dir, exist_ok=True)
    for obj in env.objects:
        if obj.type.name != "AudioClip":
            continue
        c = obj.read()
        if needle and needle.lower() not in c.m_Name.lower():
            continue
        print(c.m_Name, "fmt=", getattr(c, "m_CompressionFormat", "?"))
        raw = c.m_AudioData
        if raw:
            with open(os.path.join(out_dir, c.m_Name + ".fsb"), "wb") as w:
                w.write(raw)
        try:
            for name, wav in c.samples.items():
                fn = name if name.lower().endswith(".wav") else name + ".wav"
                with open(os.path.join(out_dir, fn), "wb") as w:
                    w.write(wav)
        except Exception as ex:  # noqa: BLE001
            print("  decode failed:", ex)


if __name__ == "__main__":
    main(*sys.argv[1:])
