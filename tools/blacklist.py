"""Build a blacklist of AudioClips that need no porting (JP and TW already sound the same).

Usage: python tools/blacklist.py <jp_root> <tw_root> <out.tsv> [--jobs N] [--max-dlen 0.05] [--null-db -30]

Phase 1 (parallel per blob, both installs): container path -> SHA1 of raw FSB5 bytes + length.
  * same SHA1                                  -> blacklist, reason "identical"
Phase 2 (parallel per TW bundle): for paths whose SHA1 differs but |len_jp - len_tw| <= max-dlen,
  decode both to PCM and do a null test: align by cross-correlation (+-4096 samples), subtract with
  least-squares gain, residual energy relative to the TW signal in dB.
  * residual <= null-db                        -> blacklist, reason "null"
Everything else is a real difference and stays portable.

Output TSV: container_path  reason  null_db  gain  lag  jp_len  tw_len
  reason: identical | null | different | different_len | undecodable
"""
import argparse
import hashlib
import io
import os
import sys
import wave
from collections import defaultdict

import numpy as np

sys.path.insert(0, os.path.dirname(__file__))
from mmvoice import iter_audio_bundles, list_blobs, run_parallel  # noqa: E402

MAX_LAG = 4096


# ---------------------------------------------------------------- phase 1
def _hash_blob(root, rel):
    import UnityPy
    from UnityPy.helpers.ResourceReader import get_resource_data
    from patch_bundle import container_map
    out = {}
    for idx, data in iter_audio_bundles(os.path.join(root, rel)):
        env = UnityPy.load(data)
        clips = [o for o in env.objects if o.type.name == "AudioClip"]
        if not clips:
            continue
        cont = container_map(env)
        for obj in clips:
            path = cont.get(obj.path_id)
            if not path or path in out:
                continue
            t = obj.read_typetree()
            r = t["m_Resource"]
            raw = bytes(get_resource_data(r["m_Source"], obj.assets_file, r["m_Offset"], r["m_Size"]))
            out[path] = (hashlib.sha1(raw).hexdigest(), float(t["m_Length"]), rel, idx)
    return rel, out


def hash_install(root, jobs, label):
    res = run_parallel(_hash_blob, [(root, r) for r in list_blobs(root)], jobs, label)
    merged = {}
    for _rel, d in sorted(res, key=lambda r: r[0]):  # deterministic: first blob wins
        for k, v in d.items():
            merged.setdefault(k, v)
    return merged


# ---------------------------------------------------------------- phase 2
def wav_to_mono(wav_bytes):
    with wave.open(io.BytesIO(wav_bytes)) as w:
        n, ch, sw, sr = w.getnframes(), w.getnchannels(), w.getsampwidth(), w.getframerate()
        raw = w.readframes(n)
    dt = {1: np.int8, 2: np.int16, 4: np.int32}[sw]
    x = np.frombuffer(raw, dtype=dt).astype(np.float64)
    if sw == 4 and np.abs(x).max() <= 1.5:  # float wav guard (unlikely)
        pass
    x = x.reshape(-1, ch).mean(axis=1) if ch > 1 else x
    return x / float(np.iinfo(dt).max), sr


def decode_clips(root, rel, idx, wanted):
    import UnityPy
    from patch_bundle import container_map
    data = dict(iter_audio_bundles(os.path.join(root, rel)))[idx]
    env = UnityPy.load(data)
    cont = container_map(env)
    out = {}
    for obj in env.objects:
        if obj.type.name != "AudioClip":
            continue
        path = cont.get(obj.path_id)
        if path in wanted and path not in out:
            samples = obj.read().samples
            if samples:
                out[path] = wav_to_mono(next(iter(samples.values())))
    return out


def null_test(a, b):
    """Return (residual_db, gain, lag). a = TW reference, b = JP."""
    n = 1 << int(np.ceil(np.log2(len(a) + len(b) + 1)))
    corr = np.fft.irfft(np.fft.rfft(a, n) * np.conj(np.fft.rfft(b, n)), n)
    lags = np.r_[np.arange(0, MAX_LAG + 1), np.arange(-MAX_LAG, 0)]
    vals = np.r_[corr[:MAX_LAG + 1], corr[-MAX_LAG:]]
    lag = int(lags[np.argmax(vals)])
    if lag >= 0:
        a2, b2 = a[lag:], b
    else:
        a2, b2 = a, b[-lag:]
    m = min(len(a2), len(b2))
    a2, b2 = a2[:m], b2[:m]
    eb = float(np.dot(b2, b2))
    g = float(np.dot(a2, b2) / eb) if eb > 0 else 0.0
    res = a2 - g * b2
    ea = float(np.dot(a, a))
    # penalise unmatched tails so a truncated clip can't pass
    tail = ea - float(np.dot(a2, a2))
    if ea <= 1e-12:
        return (0.0 if eb <= 1e-12 else 99.0), g, lag
    return 10 * np.log10((float(np.dot(res, res)) + tail) / ea + 1e-15), g, lag


def _null_bundle(jp_root, tw_root, tw_key, items):
    """items: list of (path, jp_rel, jp_idx). Returns rows."""
    rows = []
    paths = {p for p, _, _ in items}
    try:
        tw_pcm = decode_clips(tw_root, tw_key[0], tw_key[1], paths)
    except Exception as e:  # noqa: BLE001
        return [(p, "undecodable", "", "", "", str(e)[:60]) for p in paths]
    by_jp = defaultdict(set)
    for p, jr, ji in items:
        by_jp[(jr, ji)].add(p)
    jp_pcm = {}
    for (jr, ji), ps in by_jp.items():
        try:
            jp_pcm.update(decode_clips(jp_root, jr, ji, ps))
        except Exception:  # noqa: BLE001
            pass
    for p in sorted(paths):
        if p not in tw_pcm or p not in jp_pcm:
            rows.append((p, "undecodable", "", "", "", ""))
            continue
        (a, sra), (b, srb) = tw_pcm[p], jp_pcm[p]
        if sra != srb:
            rows.append((p, "different", "", "", "", f"sr {srb}->{sra}"))
            continue
        db, g, lag = null_test(a, b)
        rows.append((p, None, f"{db:.1f}", f"{g:.3f}", str(lag), ""))
    return rows


# ---------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("jp_root")
    ap.add_argument("tw_root")
    ap.add_argument("out")
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    ap.add_argument("--max-dlen", type=float, default=0.05, help="seconds; larger length gaps are 'different_len'")
    ap.add_argument("--null-db", type=float, default=-30.0)
    a = ap.parse_args()

    jp = hash_install(a.jp_root, a.jobs, "hash JP")
    tw = hash_install(a.tw_root, a.jobs, "hash TW")
    common = sorted(set(jp) & set(tw))
    rows = {}
    cand = defaultdict(list)
    for p in common:
        (hj, lj, rj, ij), (ht, lt, rt, it) = jp[p], tw[p]
        if hj == ht:
            rows[p] = ("identical", "", "", "")
        elif abs(lj - lt) > a.max_dlen:
            rows[p] = ("different_len", "", "", "")
        else:
            cand[(rt, it)].append((p, rj, ij))
    print(f"common {len(common)}  identical {sum(r[0] == 'identical' for r in rows.values())}  "
          f"different_len {sum(r[0] == 'different_len' for r in rows.values())}  "
          f"null-test candidates {sum(map(len, cand.values()))} in {len(cand)} TW bundles", file=sys.stderr)

    res = run_parallel(_null_bundle, [(a.jp_root, a.tw_root, k, v) for k, v in cand.items()], a.jobs, "null test")
    for chunk in res:
        for p, reason, db, g, lag, _note in chunk:
            if reason is None:
                reason = "null" if float(db) <= a.null_db else "different"
            rows[p] = (reason, db, g, lag)

    with open(a.out, "w", encoding="utf-8", newline="\n") as w:
        w.write("container_path\treason\tnull_db\tgain\tlag\tjp_len\ttw_len\n")
        for p in sorted(rows):
            r = rows[p]
            w.write("\t".join([p, *r, f"{jp[p][1]:.3f}", f"{tw[p][1]:.3f}"]) + "\n")
    from collections import Counter
    print("result:", dict(Counter(r[0] for r in rows.values())))


if __name__ == "__main__":
    main()
