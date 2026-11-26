"""Mabinogi Mobile voice pack: dump voices from one client, patch them into another.

  python tools/mmvoice.py dump    <jp_root> <pack> [--filter REGEX] [--jobs N]
  python tools/mmvoice.py patch   <tw_root> <pack> [--jobs N] [--dry-run]
  python tools/mmvoice.py restore <tw_root> [--keep-blobs]
  python tools/mmvoice.py status  <tw_root>

Pack format (.mmvpack = plain ZIP, stored, no compression since audio is already Vorbis):
  manifest.json  {"version":2,
                  "clips":   {sha1: AudioClip format fields},            every distinct JP content once
                  "bundles": {bundle_name: {container_path: sha1}},     exact JP variant per bundle
                  "default": {container_path: sha1}}                    most common variant (fallback)
  clips/<sha1>   raw FSB5 bytes, stored once per distinct content
  Bundle names (AssetBundle.m_Name, e.g. dedicated_bundle_7dbd4a02) are identical in JP and TW builds,
  so a TW copy of a clip gets the JP copy from the same bundle; only unknown bundles use "default".

patch (index redirect, original NNNN.blob files are never modified):
  * scans every original TW blob in parallel; a bundle needs patching if one of its AudioClips has a
    container path in the pack AND its bytes differ from the pack's bytes;
  * all patched bundles of a package go into one new blob, 9876.blob by default (--single N;
    re-runs alternate 9876/9877: new blob written first, index swapped, old blob deleted last);
    the game names blobs with "{0:0000}.blob", so the name must stay numeric;
  * a bundle that fails to parse or patch is skipped with a warning (stays TW original);
  * --dry-run only decides what to patch, it does not repack;
  * the package's `index` ((u16 blob, u16 slot) per asset) is rewritten to point at the new slots;
  * restore = put the original index back (and delete the redirect blobs). A launcher repair or
    game update that replaces `index` also reverts the mod; just run patch again.
  * backups and state.json live in --data (default ./mmvoice_data).
"""
import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import time
import zipfile
from concurrent.futures import ProcessPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(__file__))
from blob import BLOB_MAGIC, read_blob_table  # noqa: E402

REDIRECT_BASE = 5000  # redirect blob number = REDIRECT_BASE + original number (max original ~800)

PACKAGES = ["AssetPackages", "AssetPackages/Starter", "ResourcesPackage",
            "ResourcesPackage/Starter", "PlatformResourcePackages"]
DEFAULT_FILTER = r"(/voice/|_vo_)"
GAME_EXE = "MabinogiMobile.exe"


# ---------------------------------------------------------------- helpers
def sha1_bytes(b):
    return hashlib.sha1(b).hexdigest()


def sha1_file(p):
    h = hashlib.sha1()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def list_blobs(root):
    out = []
    for pkg in PACKAGES:
        for bp in sorted(glob.glob(os.path.join(root, pkg, "*.blob"))):
            stem = os.path.splitext(os.path.basename(bp))[0]
            if stem.isdigit() and int(stem) >= REDIRECT_BASE:
                continue  # our own redirect blobs
            out.append(os.path.relpath(bp, root))
    return out


def iter_audio_bundles(path):
    """Yield (idx, bundle_bytes) for UnityFS slices of a blob."""
    table = read_blob_table(path)
    with open(path, "rb") as f:
        for i, (s, e) in enumerate(table):
            f.seek(s)
            data = f.read(e - s)
            if data.startswith(b"UnityFS"):
                yield i, data


def game_running():
    out = subprocess.run(["tasklist", "/FI", f"IMAGENAME eq {GAME_EXE}"], capture_output=True, text=True).stdout
    return GAME_EXE in out


def run_parallel(fn, items, jobs, label):
    t0, total, results = time.time(), len(items), []
    with ProcessPoolExecutor(jobs) as ex:
        futs = [ex.submit(fn, *it) for it in items]
        for n, f in enumerate(as_completed(futs), 1):
            results.append(f.result())
            if n % 50 == 0 or n == total:
                print(f"[{label}] {n}/{total}  {time.time() - t0:.0f}s", file=sys.stderr, flush=True)
    return results


# ---------------------------------------------------------------- dump
def _dump_blob(root, rel, pattern):
    """-> rel, [(bundle_name, path, sha1)], {sha1: (fields, data)}"""
    import UnityPy
    from patch_bundle import bundle_info, clip_bytes
    rx = re.compile(pattern, re.I)
    refs, payloads = [], {}
    for i, data in iter_audio_bundles(os.path.join(root, rel)):
        try:
            env = UnityPy.load(data)
            clips = [o for o in env.objects if o.type.name == "AudioClip"]
            if not clips:
                continue
            name, cont = bundle_info(env)
            for obj in clips:
                path = cont.get(obj.path_id)
                if not path or not rx.search(path):
                    continue
                t = obj.read_typetree()
                raw = clip_bytes(obj, t)
                h = sha1_bytes(raw)
                refs.append((name, path, h))
                if h not in payloads:
                    from patch_bundle import COPY_FIELDS
                    payloads[h] = ({k: t[k] for k in COPY_FIELDS if k in t}, raw)
        except Exception as e:  # noqa: BLE001
            print(f"[warn] {rel}#{i}: {e}", file=sys.stderr)
    return rel, refs, payloads


def cmd_dump(a):
    blobs = list_blobs(a.root)
    print(f"dump: {len(blobs)} blobs from {a.root}, filter {a.filter!r}", file=sys.stderr)
    results = sorted(run_parallel(_dump_blob, [(a.root, r, a.filter) for r in blobs], a.jobs, "dump"),
                     key=lambda r: r[0])
    by_bundle = {}            # bundle_name -> {path: sha1}
    votes = {}                # path -> Counter(sha1) for the path-only fallback
    clips = {}                # sha1 -> fields
    tmp = a.pack + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_STORED) as z:
        for _rel, refs, payloads in results:
            for h, (fields, data) in payloads.items():
                if h not in clips:
                    z.writestr(f"clips/{h}", data)   # every distinct content stored exactly once
                    clips[h] = fields
            for name, path, h in refs:
                by_bundle.setdefault(name, {})[path] = h
                votes.setdefault(path, {}).setdefault(h, 0)
                votes[path][h] += 1
        # fallback = variant used by most JP bundles (ties: first seen)
        default = {p: max(c, key=c.get) for p, c in votes.items()}
        variants = sum(1 for c in votes.values() if len(c) > 1)
        manifest = {"version": 2, "source": os.path.abspath(a.root), "filter": a.filter,
                    "created": time.strftime("%Y-%m-%dT%H:%M:%S"),
                    "clips": clips, "default": dict(sorted(default.items())),
                    "bundles": {k: dict(sorted(v.items())) for k, v in sorted(by_bundle.items())}}
        z.writestr("manifest.json", json.dumps(manifest, ensure_ascii=False, separators=(",", ":")))
    os.replace(tmp, a.pack)
    print(f"dump: {len(default)} paths, {variants} with multiple JP variants, {len(clips)} unique payloads, "
          f"{len(by_bundle)} bundles -> {a.pack} ({os.path.getsize(a.pack) / 2**20:.1f} MB)")


# ---------------------------------------------------------------- patch
def load_manifest(pack):
    with zipfile.ZipFile(pack) as z:
        m = json.loads(z.read("manifest.json"))
    if m.get("version") != 2:
        sys.exit(f"{pack}: pack version {m.get('version')} not supported, re-run dump")
    return m


_MAN_CACHE = {}


def _cached_manifest(pack):
    """Worker processes are reused across blobs: parse the manifest once per process."""
    if pack not in _MAN_CACHE:
        _MAN_CACHE[pack] = load_manifest(pack)
    return _MAN_CACHE[pack]


def lookup(man, bundle_name, path):
    """JP variant for this exact bundle if JP has the same bundle, else the most common variant."""
    h = man["bundles"].get(bundle_name, {}).get(path)
    return h or man["default"].get(path)


def redirect_no(blob_no):
    return REDIRECT_BASE + blob_no


def blob_no(rel):
    return int(os.path.splitext(os.path.basename(rel))[0])


def _patch_blob(root, rel, pack, stage, dry_run=False):
    """Scan one original TW blob. If any bundle needs JP audio, write the patched bundles (only those)
    to stage/<pkg>/<REDIRECT_BASE+N>.blob and return the slot mapping {orig_slot: new_slot}.
    A bundle that fails to parse/patch is skipped (left as TW original) and reported, not fatal.
    dry_run: only decide what needs patching, skip the expensive repack."""
    from patch_bundle import ClipData, bundle_info, clip_bytes, patch_bundle
    import UnityPy

    man = _cached_manifest(pack)
    src = os.path.join(root, rel)
    patched, n_clips, warns = [], 0, []
    with zipfile.ZipFile(pack) as z:
        for idx, data in iter_audio_bundles(src):
            try:
                env = UnityPy.load(data)
                clips = [o for o in env.objects if o.type.name == "AudioClip"]
                if not clips:
                    continue
                name, cont = bundle_info(env)
                need = {}
                for obj in clips:
                    path = cont.get(obj.path_id)
                    h = lookup(man, name, path) if path else None
                    if not h:
                        continue
                    if sha1_bytes(clip_bytes(obj, obj.read_typetree())) != h:
                        need[path] = ClipData(man["clips"][h], None if dry_run else z.read(f"clips/{h}"))
                if not need:
                    continue
                if dry_run:
                    patched.append((idx, None))
                    n_clips += len(need)
                else:
                    new, n = patch_bundle(data, need)
                    patched.append((idx, new))
                    n_clips += n
            except Exception as e:  # noqa: BLE001
                warns.append(f"{rel}#{idx}: {e!r}")
    for w in warns:
        print(f"[warn] skipped bundle {w}", file=sys.stderr)
    if not patched:
        return rel, {}, 0, None
    if dry_run:
        return rel, {i: k for k, (i, _b) in enumerate(patched)}, n_clips, "dry-run"
    pkg = os.path.dirname(rel)
    out = os.path.join(stage, pkg, f"{redirect_no(blob_no(rel)):04d}.blob")
    write_blob_from(out, [b for _i, b in patched])
    return rel, {i: k for k, (i, _b) in enumerate(patched)}, n_clips, out


def write_blob_from(dst, bundles):
    hdr_end = 8 + 4 * len(bundles)
    ends, pos = [], hdr_end
    for b in bundles:
        pos += len(b)
        ends.append(pos)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    with open(dst + ".tmp", "wb") as w:
        w.write(struct.pack("<II", BLOB_MAGIC, hdr_end))
        w.write(struct.pack(f"<{len(ends)}I", *ends))
        for b in bundles:
            w.write(b)
    os.replace(dst + ".tmp", dst)


def concat_blobs(srcs, dst):
    """Concatenate the bundles of several blobs (in order) into one blob, streaming."""
    tables = [read_blob_table(s) for s in srcs]
    sizes = [e - s for t in tables for s, e in t]
    hdr_end = 8 + 4 * len(sizes)
    ends, pos = [], hdr_end
    for n in sizes:
        pos += n
        ends.append(pos)
    if pos >= 1 << 32:
        raise ValueError("merged blob would exceed 4 GiB; use per-blob mode (--single 0)")
    if len(sizes) > 0xFFFF:
        raise ValueError("more than 65535 bundles; use per-blob mode (--single 0)")
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    with open(dst + ".tmp", "wb") as w:
        w.write(struct.pack("<II", BLOB_MAGIC, hdr_end))
        w.write(struct.pack(f"<{len(ends)}I", *ends))
        for src, t in zip(srcs, tables):
            with open(src, "rb") as f:
                f.seek(t[0][0])
                shutil.copyfileobj(f, w, 1 << 20)
    os.replace(dst + ".tmp", dst)


# ---------------------------------------------------------------- index
INDEX_MAGIC = 0xE6A9C3BA


def patch_index(buf, redirect):
    """redirect: {(blob_no, slot): (new_blob_no, new_slot)} -> (new_bytes, entries_changed)."""
    magic, n = struct.unpack_from("<II", buf, 0)
    if magic != INDEX_MAGIC:
        raise ValueError("bad index magic")
    pairs = list(struct.unpack_from(f"<{n * 2}H", buf, 8))
    changed = 0
    for k in range(n):
        key = (pairs[2 * k], pairs[2 * k + 1])
        if key in redirect:
            pairs[2 * k], pairs[2 * k + 1] = redirect[key]
            changed += 1
    out = bytearray(buf)
    struct.pack_into(f"<{n * 2}H", out, 8, *pairs)
    return bytes(out), changed


def state_paths(data_dir):
    return os.path.join(data_dir, "backup"), os.path.join(data_dir, "state.json")


def load_state(data_dir):
    _, sp = state_paths(data_dir)
    if os.path.exists(sp):
        with open(sp, encoding="utf-8") as f:
            return json.load(f)
    return {}


def save_state(data_dir, st):
    _, sp = state_paths(data_dir)
    os.makedirs(data_dir, exist_ok=True)
    with open(sp + ".tmp", "w", encoding="utf-8") as f:
        json.dump(st, f, indent=1, sort_keys=True)
    os.replace(sp + ".tmp", sp)


def cmd_patch(a):
    if not a.dry_run and game_running():
        sys.exit(f"{GAME_EXE} is running; close the game first.")
    blobs = list_blobs(a.root)
    stage = os.path.join(a.data, "stage")
    shutil.rmtree(stage, ignore_errors=True)
    print(f"patch: scanning {len(blobs)} original blobs in {a.root}", file=sys.stderr)
    results = run_parallel(_patch_blob, [(a.root, r, a.pack, stage, a.dry_run) for r in blobs], a.jobs,
                           "scan" if a.dry_run else "scan+patch")
    todo = sorted((r for r in results if r[3]), key=lambda r: r[0])
    print(f"patch: {len(todo)} blobs / {sum(len(r[1]) for r in todo)} bundles / "
          f"{sum(r[2] for r in todo)} clips need JP audio")
    if a.dry_run:
        return

    base, _ = state_paths(a.data)
    st = load_state(a.data)
    by_pkg = {}
    for rel, slots, _n, staged in todo:
        by_pkg.setdefault(os.path.dirname(rel), []).append((rel, slots, staged))
    # packages patched before but needing nothing now still get their index restored
    pkgs = sorted(set(by_pkg) | set(st))
    for pkg in pkgs:
        idx_rel = os.path.join(pkg, "index")
        idx_path = os.path.join(a.root, idx_rel)
        bak = os.path.join(base, idx_rel)
        rec = st.get(pkg)
        cur = sha1_file(idx_path)
        if rec and cur == rec["patched_sha1"]:
            orig = open(bak, "rb").read()           # we patched it: start from our backup
        else:
            os.makedirs(os.path.dirname(bak), exist_ok=True)
            shutil.copy2(idx_path, bak)              # fresh original (install / update / repair)
            orig = open(bak, "rb").read()
            rec = {"orig_sha1": cur, "blobs": (rec or {}).get("blobs", [])}
        old_blobs = list(rec.get("blobs", []))
        # Crash-safe order: 1) write the new blob under a name the current index does not use,
        # 2) swap the index atomically, 3) only then delete the previous run's blob.
        # Two slots (N, N+1) alternate so step 1 never overwrites a file the live index points at.
        number = a.single if f"{a.single:04d}.blob" not in old_blobs else a.single + 1
        name = f"{number:04d}.blob"
        redirect, new_blobs = {}, []
        items = by_pkg.get(pkg, [])
        if items:
            merged, base_slot = [], 0
            for rel, slots, staged in items:
                n = blob_no(rel)
                for s, ns in slots.items():
                    redirect[(n, s)] = (number, base_slot + ns)
                merged.append(staged)
                base_slot += len(slots)
            concat_blobs(merged, os.path.join(a.root, pkg, name))
            new_blobs.append(name)
        new_idx, changed = patch_index(orig, redirect)
        with open(idx_path + ".mmvoice.tmp", "wb") as w:
            w.write(new_idx)
        os.replace(idx_path + ".mmvoice.tmp", idx_path)
        rec.update(patched_sha1=sha1_file(idx_path), blobs=new_blobs)
        st[pkg] = rec
        save_state(a.data, st)
        for old in old_blobs:
            p = os.path.join(a.root, pkg, old)
            if old not in new_blobs and os.path.exists(p):
                os.remove(p)
        print(f"patch: {pkg}: {name} ({len(redirect)} bundles), {changed} index entries redirected")
    shutil.rmtree(stage, ignore_errors=True)
    print(f"patch: done. Original blobs untouched; index backups in {base}")


def cmd_restore(a):
    if game_running():
        sys.exit(f"{GAME_EXE} is running; close the game first.")
    base, _ = state_paths(a.data)
    st = load_state(a.data)
    for pkg in sorted(st):
        rec = st[pkg]
        idx_path = os.path.join(a.root, pkg, "index")
        bak = os.path.join(base, pkg, "index")
        if sha1_file(idx_path) == rec["patched_sha1"]:
            shutil.copyfile(bak, idx_path)
            if sha1_file(idx_path) != rec["orig_sha1"]:
                sys.exit(f"restore verify failed for {pkg}/index")
            print(f"restored {pkg}/index")
        else:
            print(f"{pkg}/index changed since patch (game update / repair): left as is")
        if not a.keep_blobs:
            for b in rec.get("blobs", []):
                p = os.path.join(a.root, pkg, b)
                if os.path.exists(p):
                    os.remove(p)
            print(f"removed {len(rec.get('blobs', []))} redirect blobs in {pkg}")
        del st[pkg]
        os.remove(bak)
        save_state(a.data, st)


def cmd_status(a):
    st = load_state(a.data)
    if not st:
        print("not patched")
    for pkg, rec in sorted(st.items()):
        cur = sha1_file(os.path.join(a.root, pkg, "index"))
        state = "patched" if cur == rec["patched_sha1"] else "REVERTED/CHANGED (run patch again)"
        missing = [b for b in rec.get("blobs", []) if not os.path.exists(os.path.join(a.root, pkg, b))]
        print(f"{pkg}: index {state}; {len(rec.get('blobs', []))} redirect blobs"
              + (f", MISSING {missing}" if missing else ""))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("dump")
    d.add_argument("root")
    d.add_argument("pack")
    d.add_argument("--filter", default=DEFAULT_FILTER, help="regex on container path ('.' = all audio)")
    d.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    p = sub.add_parser("patch")
    p.add_argument("root")
    p.add_argument("pack")
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--single", type=int, default=9876,
                   help="blob number for the patched bundles of each package (default 9876; "
                        "N and N+1 alternate between runs so the live index never points at a half-written file)")
    r = sub.add_parser("restore")
    r.add_argument("root")
    r.add_argument("--keep-blobs", action="store_true", help="only restore index, leave redirect blobs")
    s = sub.add_parser("status")
    s.add_argument("root")
    for x in (p, r, s):
        x.add_argument("--data", default="mmvoice_data",
                       help="where backups, state.json and staging live (default: ./mmvoice_data)")
    a = ap.parse_args()
    {"dump": cmd_dump, "patch": cmd_patch, "restore": cmd_restore, "status": cmd_status}[a.cmd](a)


if __name__ == "__main__":
    main()
