"""Mabinogi Mobile voice pack: dump voices from one client, patch them into another.

  python tools/mmvoice.py dump    <jp_root> <pack> [--filter REGEX] [--blacklist TSV] [--jobs N]
  python tools/mmvoice.py patch   <tw_root> <pack> [--jobs N] [--dry-run]
  python tools/mmvoice.py restore <tw_root> [--keep-blobs]
  python tools/mmvoice.py status  <tw_root>

Pack format (.mmvpack = plain ZIP, stored, no compression since audio is already Vorbis):
  manifest.json  {"version":1, "source":..., "clips": {container_path: {"sha1", "fields"}}}
  clips/<sha1>   raw FSB5 bytes of each AudioClip (deduplicated by content)

patch (index redirect, original NNNN.blob files are never modified):
  * scans every original TW blob in parallel; a bundle needs patching if one of its AudioClips has a
    container path in the pack AND its bytes differ from the pack's bytes;
  * all patched bundles of a package go into one new blob, 9876.blob by default (--single N);
    with --single 0 each NNNN.blob gets its own REDIRECT_BASE+NNNN.blob instead (e.g. 5260.blob);
    the game names blobs with "{0:0000}.blob", so the name must stay numeric;
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
    from patch_bundle import read_clips
    rx = re.compile(pattern, re.I)
    found = {}
    for _i, data in iter_audio_bundles(os.path.join(root, rel)):
        try:
            clips = read_clips(data)
        except Exception as e:  # noqa: BLE001
            print(f"[warn] {rel}#{_i}: {e}", file=sys.stderr)
            continue
        for path, c in clips.items():
            if rx.search(path) and path not in found:
                found[path] = (c.fields, c.data)
    return rel, found


def cmd_dump(a):
    blobs = list_blobs(a.root)
    print(f"dump: {len(blobs)} blobs from {a.root}, filter {a.filter!r}", file=sys.stderr)
    results = run_parallel(_dump_blob, [(a.root, r, a.filter) for r in blobs], a.jobs, "dump")
    skip = set()
    if a.blacklist:
        import csv
        with open(a.blacklist, encoding="utf-8") as f:
            skip = {r["container_path"] for r in csv.DictReader(f, delimiter="\t")
                    if r["reason"] in ("identical", "null")}
        print(f"dump: blacklist {a.blacklist}: {len(skip)} paths excluded", file=sys.stderr)
    clips, written = {}, set()
    tmp = a.pack + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_STORED) as z:
        for _rel, found in sorted(results, key=lambda r: r[0]):  # deterministic: first blob wins
            for path, (fields, data) in found.items():
                if path in clips or path in skip:
                    continue
                h = sha1_bytes(data)
                if h not in written:
                    z.writestr(f"clips/{h}", data)
                    written.add(h)
                clips[path] = {"sha1": h, "fields": fields}
        manifest = {"version": 1, "source": os.path.abspath(a.root), "filter": a.filter,
                    "blacklist": os.path.basename(a.blacklist) if a.blacklist else None,
                    "created": time.strftime("%Y-%m-%dT%H:%M:%S"), "clips": dict(sorted(clips.items()))}
        z.writestr("manifest.json", json.dumps(manifest, ensure_ascii=False, indent=1))
    os.replace(tmp, a.pack)
    print(f"dump: {len(clips)} clips ({len(written)} unique payloads) -> {a.pack} "
          f"({os.path.getsize(a.pack) / 2**20:.1f} MB)")


# ---------------------------------------------------------------- patch
def load_manifest(pack):
    with zipfile.ZipFile(pack) as z:
        return json.loads(z.read("manifest.json"))


def redirect_no(blob_no):
    return REDIRECT_BASE + blob_no


def blob_no(rel):
    return int(os.path.splitext(os.path.basename(rel))[0])


def _patch_blob(root, rel, pack, stage):
    """Scan one original TW blob. If any bundle needs JP audio, write the patched bundles (only those)
    to stage/<pkg>/<REDIRECT_BASE+N>.blob and return the slot mapping {orig_slot: new_slot}."""
    from patch_bundle import ClipData, container_map, patch_bundle
    import UnityPy
    from UnityPy.helpers.ResourceReader import get_resource_data

    man = load_manifest(pack)["clips"]
    src = os.path.join(root, rel)
    patched, n_clips = [], 0
    with zipfile.ZipFile(pack) as z:
        for idx, data in iter_audio_bundles(src):
            env = UnityPy.load(data)
            clips = [o for o in env.objects if o.type.name == "AudioClip"]
            if not clips:
                continue
            cont = container_map(env)
            need = {}
            for obj in clips:
                path = cont.get(obj.path_id)
                entry = man.get(path)
                if not entry:
                    continue
                t = obj.read_typetree()
                r = t["m_Resource"]
                cur = bytes(get_resource_data(r["m_Source"], obj.assets_file, r["m_Offset"], r["m_Size"]))
                if sha1_bytes(cur) != entry["sha1"]:
                    need[path] = ClipData(entry["fields"], z.read(f"clips/{entry['sha1']}"))
            if need:
                new, n = patch_bundle(data, need)
                patched.append((idx, new))
                n_clips += n
    if not patched:
        return rel, {}, 0, None
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
    results = run_parallel(_patch_blob, [(a.root, r, a.pack, stage) for r in blobs], a.jobs, "scan+patch")
    todo = sorted((r for r in results if r[3]), key=lambda r: r[0])
    print(f"patch: {len(todo)} blobs / {sum(len(r[1]) for r in todo)} bundles / "
          f"{sum(r[2] for r in todo)} clips need JP audio")
    if a.dry_run:
        shutil.rmtree(stage, ignore_errors=True)
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
        # remove redirect blobs from a previous run
        for old in rec.get("blobs", []):
            p = os.path.join(a.root, pkg, old)
            if os.path.exists(p):
                os.remove(p)
        redirect, new_blobs = {}, []
        items = by_pkg.get(pkg, [])
        if a.single and items:
            # merge every staged redirect blob of this package into one <single>.blob
            name = f"{a.single:04d}.blob"
            merged, base_slot = [], 0
            for rel, slots, staged in items:
                n = blob_no(rel)
                for s, ns in slots.items():
                    redirect[(n, s)] = (a.single, base_slot + ns)
                merged.append(staged)
                base_slot += len(slots)
            concat_blobs(merged, os.path.join(a.root, pkg, name))
            new_blobs.append(name)
        else:
            for rel, slots, staged in items:
                n, rn = blob_no(rel), redirect_no(blob_no(rel))
                for s, ns in slots.items():
                    redirect[(n, s)] = (rn, ns)
                name = os.path.basename(staged)
                shutil.copyfile(staged, os.path.join(a.root, pkg, name))
                new_blobs.append(name)
        new_idx, changed = patch_index(orig, redirect)
        with open(idx_path + ".mmvoice.tmp", "wb") as w:
            w.write(new_idx)
        os.replace(idx_path + ".mmvoice.tmp", idx_path)
        rec.update(patched_sha1=sha1_file(idx_path), blobs=new_blobs)
        st[pkg] = rec
        save_state(a.data, st)
        print(f"patch: {pkg}: {len(new_blobs)} redirect blobs, {changed} index entries redirected")
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
    d.add_argument("--blacklist", help="TSV from tools/blacklist.py; identical/null paths are skipped")
    d.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    p = sub.add_parser("patch")
    p.add_argument("root")
    p.add_argument("pack")
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--single", type=int, default=9876,
                   help="put all patched bundles of a package into one <N>.blob (default 9876); "
                        "0 = one redirect blob per original (5000+NNNN.blob)")
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
