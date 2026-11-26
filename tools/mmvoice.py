"""Mabinogi Mobile voice pack: dump voices from one client, patch them into another.

  python tools/mmvoice.py dump    <jp_root> <pack> [--filter REGEX] [--jobs N]
  python tools/mmvoice.py patch   <tw_root> <pack> [--jobs N] [--dry-run]
  python tools/mmvoice.py restore <tw_root> [files ...]       (default: all backed-up files)
  python tools/mmvoice.py status  <tw_root>

Pack format (.mmvpack = plain ZIP, stored, no compression since audio is already Vorbis):
  manifest.json  {"version":1, "source":..., "clips": {container_path: {"file", "sha1", "fields"}}}
  clips/<sha1>   raw FSB5 bytes of each AudioClip (deduplicated by content)

patch:
  * scans every TW blob in parallel; a bundle is rewritten only if one of its AudioClips has a
    container path in the pack AND its current bytes differ from the pack's bytes;
  * so it is idempotent: re-running on an already patched install changes nothing;
  * before overwriting a blob the original is copied to <data>/backup/<rel> (--data, default ./mmvoice_data)
    (unless the current file is one we wrote ourselves), and state.json records both SHA1s,
    so after a game update / launcher repair you can just run patch again.
"""
import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import zipfile
from concurrent.futures import ProcessPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(__file__))
from blob import read_blob_table  # noqa: E402
from build import write_blob  # noqa: E402

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
    clips, written = {}, set()
    tmp = a.pack + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_STORED) as z:
        for _rel, found in sorted(results, key=lambda r: r[0]):  # deterministic: first blob wins
            for path, (fields, data) in found.items():
                if path in clips:
                    continue
                h = sha1_bytes(data)
                if h not in written:
                    z.writestr(f"clips/{h}", data)
                    written.add(h)
                clips[path] = {"sha1": h, "fields": fields}
        manifest = {"version": 1, "source": os.path.abspath(a.root), "filter": a.filter,
                    "created": time.strftime("%Y-%m-%dT%H:%M:%S"), "clips": dict(sorted(clips.items()))}
        z.writestr("manifest.json", json.dumps(manifest, ensure_ascii=False, indent=1))
    os.replace(tmp, a.pack)
    print(f"dump: {len(clips)} clips ({len(written)} unique payloads) -> {a.pack} "
          f"({os.path.getsize(a.pack) / 2**20:.1f} MB)")


# ---------------------------------------------------------------- patch
def load_manifest(pack):
    with zipfile.ZipFile(pack) as z:
        return json.loads(z.read("manifest.json"))


def _patch_blob(root, rel, pack, stage):
    """Scan one TW blob; if anything needs replacing write a patched copy to stage/rel."""
    from patch_bundle import ClipData, container_map, patch_bundle
    import UnityPy
    from UnityPy.helpers.ResourceReader import get_resource_data

    man = load_manifest(pack)["clips"]
    src = os.path.join(root, rel)
    replacements, n_clips = {}, 0
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
                replacements[idx] = new
                n_clips += n
    if not replacements:
        return rel, 0, 0, None
    out = os.path.join(stage, rel)
    write_blob(src, out, replacements)
    return rel, len(replacements), n_clips, out


def state_paths(data_dir):
    base = os.path.join(data_dir, "backup")
    return base, os.path.join(data_dir, "state.json")


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
    print(f"patch: scanning {len(blobs)} blobs in {a.root}", file=sys.stderr)
    results = run_parallel(_patch_blob, [(a.root, r, a.pack, stage) for r in blobs], a.jobs, "scan+patch")
    todo = sorted((r for r in results if r[3]), key=lambda r: r[0])
    print(f"patch: {len(todo)} blobs / {sum(r[1] for r in todo)} bundles / {sum(r[2] for r in todo)} clips need patching")
    if a.dry_run or not todo:
        shutil.rmtree(stage, ignore_errors=True)
        return

    base, _ = state_paths(a.data)
    st = load_state(a.data)
    for rel, _nb, _nc, staged in todo:
        dst = os.path.join(a.root, rel)
        cur = sha1_file(dst)
        rec = st.get(rel)
        bak = os.path.join(base, rel)
        if not rec or cur != rec.get("patched_sha1"):
            # current file is an original (fresh install, game update, or launcher repair) -> back it up
            os.makedirs(os.path.dirname(bak), exist_ok=True)
            shutil.copy2(dst, bak)
            rec = {"orig_sha1": cur}
        shutil.copyfile(staged, dst)
        rec["patched_sha1"] = sha1_file(dst)
        if rec["patched_sha1"] != sha1_file(staged):
            sys.exit(f"verify failed for {rel}")
        st[rel] = rec
        save_state(a.data, st)  # save after every file so an interruption stays restorable
    shutil.rmtree(stage, ignore_errors=True)
    print(f"patch: done, backups in {base}")


def cmd_restore(a):
    if game_running():
        sys.exit(f"{GAME_EXE} is running; close the game first.")
    base, _ = state_paths(a.data)
    st = load_state(a.data)
    rels = [f.replace("/", os.sep) for f in a.files] if a.files else sorted(st)
    for rel in rels:
        rec, dst, bak = st.get(rel), os.path.join(a.root, rel), os.path.join(base, rel)
        if not rec:
            print(f"skip {rel}: not patched by mmvoice")
            continue
        cur = sha1_file(dst)
        if cur != rec["patched_sha1"]:
            print(f"skip {rel}: file changed since patch (game update?), leaving it alone")
        else:
            shutil.copyfile(bak, dst)
            if sha1_file(dst) != rec["orig_sha1"]:
                sys.exit(f"restore verify failed for {rel}")
            print(f"restored {rel}")
        del st[rel]
        os.remove(bak)
        save_state(a.data, st)


def cmd_status(a):
    st = load_state(a.data)
    patched = changed = 0
    for rel, rec in sorted(st.items()):
        cur = sha1_file(os.path.join(a.root, rel))
        if cur == rec["patched_sha1"]:
            patched += 1
        else:
            changed += 1
            print(f"reverted/changed: {rel}")
    print(f"{patched} blobs patched, {changed} reverted or changed (run patch again)")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("dump")
    d.add_argument("root")
    d.add_argument("pack")
    d.add_argument("--filter", default=DEFAULT_FILTER)
    d.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    p = sub.add_parser("patch")
    p.add_argument("root")
    p.add_argument("pack")
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    p.add_argument("--dry-run", action="store_true")
    r = sub.add_parser("restore")
    r.add_argument("root")
    r.add_argument("files", nargs="*")
    s = sub.add_parser("status")
    s.add_argument("root")
    for x in (p, r, s):
        x.add_argument("--data", default="mmvoice_data",
                       help="where backups, state.json and staging live (default: ./mmvoice_data)")
    a = ap.parse_args()
    {"dump": cmd_dump, "patch": cmd_patch, "restore": cmd_restore, "status": cmd_status}[a.cmd](a)


if __name__ == "__main__":
    main()
