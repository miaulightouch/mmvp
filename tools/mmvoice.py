"""Mabinogi Mobile voice pack: dump voices from one client, patch them into another.

  python tools/mmvoice.py dump    <jp_root> <pack> [--filter REGEX] [--jobs N]
  python tools/mmvoice.py patch   <tw_root> <pack> [--jobs N] [--dry-run] [--rescan]
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
  1. scan    parallel, one task per original TW blob: for every bundle with AudioClips record
             (slot, bundle name, {container path: sha1}). Pack-independent, cached in
             <data>/scan_cache.json keyed by (blob path, size, mtime); later runs rescan only changed
             blobs. A bundle that fails to parse is skipped with a warning (stays TW original).
  2. plan    in memory: a bundle needs patching if any clip's JP sha1 (pack lookup) differs from TW's.
             --dry-run stops here.
  3. rebuild parallel, one task per bundle to patch; workers return the repacked bundle and the main
             process appends it straight into <pkg>/<N>.blob.mmvoice.tmp (new slot = arrival order,
             offset table written last). No staging copies. Any failure aborts before the game
             directory is changed (the .tmp files are removed).
  4. commit  per package: rename the new blob into place under the number the live index does NOT
             use (N / N+1 alternate, default 9876/9877), atomically swap `index`, then delete the
             previous run's blob. An interruption at any point leaves the index pointing at files
             that exist.
  The game names blobs with "{0:0000}.blob", so the name must stay numeric.
  restore = put the original index back (and delete the redirect blobs). A launcher repair or game
  update that replaces `index` also reverts the mod; just run patch again.
  Backups, state.json and the scan cache live in --data (default ./mmvoice_data).
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

REDIRECT_BASE = 5000  # blobs numbered >= this are ours and never scanned (max original ~800)
SCAN_VERSION = 1

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


def read_slot(path, slot):
    s, e = read_blob_table(path)[slot]
    with open(path, "rb") as f:
        f.seek(s)
        return f.read(e - s)


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


def write_json_atomic(path, obj, **kw):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path + ".tmp", "w", encoding="utf-8") as f:
        json.dump(obj, f, **kw)
    os.replace(path + ".tmp", path)


# ---------------------------------------------------------------- dump
def _dump_blob(root, rel, pattern):
    """-> rel, [(bundle_name, path, sha1)], {sha1: (fields, data)}"""
    import UnityPy
    from patch_bundle import COPY_FIELDS, bundle_info, clip_bytes
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


# ---------------------------------------------------------------- pack access
def load_manifest(pack):
    with zipfile.ZipFile(pack) as z:
        m = json.loads(z.read("manifest.json"))
    if m.get("version") != 2:
        sys.exit(f"{pack}: pack version {m.get('version')} not supported, re-run dump")
    return m


_WORKER = {}


def _worker_pack(pack):
    """Worker processes are reused across tasks: parse the manifest / open the zip once per process."""
    if _WORKER.get("pack") != pack:
        _WORKER.update(pack=pack, man=load_manifest(pack), zip=zipfile.ZipFile(pack))
    return _WORKER["man"], _WORKER["zip"]


def lookup(man, bundle_name, path):
    """JP variant for this exact bundle if JP has the same bundle, else the most common variant."""
    h = man["bundles"].get(bundle_name, {}).get(path)
    return h or man["default"].get(path)


def blob_no(rel):
    return int(os.path.splitext(os.path.basename(rel))[0])


# ---------------------------------------------------------------- patch 1: scan (cached)
def _scan_blob(root, rel):
    """Pack-independent facts about one original TW blob.
    -> rel, {"size", "mtime_ns", "bundles": [[slot, bundle_name, {path: sha1}], ...], "warns": [...]}
    Only bundles holding AudioClips with a container path are listed; all audio is hashed (no path
    filter) so the cache stays valid for any pack."""
    import UnityPy
    from patch_bundle import bundle_info, clip_bytes
    src = os.path.join(root, rel)
    st = os.stat(src)  # taken before reading: a concurrent rewrite changes mtime -> rescanned next time
    bundles, warns = [], []
    for idx, data in iter_audio_bundles(src):
        try:
            env = UnityPy.load(data)
            clips = [o for o in env.objects if o.type.name == "AudioClip"]
            if not clips:
                continue
            name, cont = bundle_info(env)
            hashes = {}
            for obj in clips:
                path = cont.get(obj.path_id)
                if path and path not in hashes:
                    hashes[path] = sha1_bytes(clip_bytes(obj, obj.read_typetree()))
            if hashes:
                bundles.append([idx, name, hashes])
        except Exception as e:  # noqa: BLE001
            warns.append(f"{rel}#{idx}: {e!r}")
    return rel, {"size": st.st_size, "mtime_ns": st.st_mtime_ns, "bundles": bundles, "warns": warns}


def scan_install(root, data_dir, jobs, rescan=False):
    """-> {rel: scan entry} for every original blob, reusing <data>/scan_cache.json where valid."""
    cache_path = os.path.join(data_dir, "scan_cache.json")
    cached = {}
    if not rescan and os.path.exists(cache_path):
        with open(cache_path, encoding="utf-8") as f:
            c = json.load(f)
        if c.get("version") == SCAN_VERSION and c.get("root") == os.path.abspath(root):
            cached = c["blobs"]
    blobs = list_blobs(root)
    scan, todo = {}, []
    for rel in blobs:
        e, s = cached.get(rel), os.stat(os.path.join(root, rel))
        if e and e["size"] == s.st_size and e["mtime_ns"] == s.st_mtime_ns:
            scan[rel] = e
        else:
            todo.append(rel)
    print(f"scan: {len(blobs)} blobs, {len(scan)} cached, {len(todo)} to scan", file=sys.stderr)
    if todo:
        for rel, entry in run_parallel(_scan_blob, [(root, r) for r in todo], jobs, "scan"):
            scan[rel] = entry
    if todo or len(scan) != len(cached):
        write_json_atomic(cache_path, {"version": SCAN_VERSION, "root": os.path.abspath(root),
                                       "blobs": dict(sorted(scan.items()))}, separators=(",", ":"))
    for rel in sorted(scan):
        for w in scan[rel].get("warns", []):
            print(f"[warn] skipped bundle {w}", file=sys.stderr)
    return scan


# ---------------------------------------------------------------- patch 2: plan
def index_slots(idx_bytes):
    """{(blob_no, slot)} referenced by an index (0x8000 flag masked off)."""
    magic, n = struct.unpack_from("<II", idx_bytes, 0)
    if magic != INDEX_MAGIC:
        raise ValueError("bad index magic")
    pairs = struct.unpack_from(f"<{n * 2}H", idx_bytes, 8)
    return {(pairs[2 * k] & BLOB_MASK, pairs[2 * k + 1]) for k in range(n)}


def plan_patch(scan, man, live=None):
    """-> {pkg: [(rel, slot, {path: jp_sha1}), ...]} in (blob, slot) order.
    live: {pkg: {(blob_no, slot)}} from the original indexes; slots no index entry points at are
    skipped. [確認] AssetPackages has ~14.6k such slots (160 with audio); every voice path in them also
    exists in a referenced bundle. [推論] stale copies left behind by incremental updates; the game
    cannot reach them, so patching them would only grow the new blob."""
    plan, dead = {}, 0
    for rel in sorted(scan):
        pkg, no = os.path.dirname(rel), blob_no(rel)
        for slot, name, hashes in scan[rel]["bundles"]:
            if live is not None and (no, slot) not in live.get(pkg, ()):
                dead += 1
                continue
            need = {}
            for path, cur in hashes.items():
                h = lookup(man, name, path)
                if h and h != cur:
                    need[path] = h
            if need:
                plan.setdefault(pkg, []).append((rel, slot, need))
    if dead:
        print(f"plan: {dead} audio bundles skipped (not referenced by their index)", file=sys.stderr)
    for pkg, items in plan.items():
        if len(items) > 0xFFFF:
            sys.exit(f"{pkg}: {len(items)} bundles to patch, more than one blob can address (65535)")
    return plan


# ---------------------------------------------------------------- patch 3: rebuild
def _rebuild_bundle(root, rel, slot, need, pack):
    """Repack one TW bundle with JP audio. -> (rel, slot, new_bundle_bytes)"""
    from patch_bundle import ClipData, patch_bundle
    man, z = _worker_pack(pack)
    jp = {p: ClipData(man["clips"][h], z.read(f"clips/{h}")) for p, h in need.items()}
    new, n = patch_bundle(read_slot(os.path.join(root, rel), slot), jp)
    if n < len(need):
        raise RuntimeError(f"{rel}#{slot}: replaced {n} of {len(need)} clips (blob changed since scan?)")
    return rel, slot, new


class BlobWriter:
    """Append-only .blob writer: bundles go in as they arrive, the offset table is written on finish()."""

    def __init__(self, path, count):
        self.path, self.tmp, self.count = path, path + ".mmvoice.tmp", count
        self.hdr_end = 8 + 4 * count
        self.ends, self.pos = [], self.hdr_end
        self.f = open(self.tmp, "wb")
        self.f.seek(self.hdr_end)

    def append(self, data):
        self.f.write(data)
        self.pos += len(data)
        if self.pos >= 1 << 32:
            raise ValueError(f"{self.path}: would exceed 4 GiB")
        self.ends.append(self.pos)
        return len(self.ends) - 1  # slot

    def finish(self):
        if len(self.ends) != self.count:
            raise RuntimeError(f"{self.path}: {len(self.ends)} of {self.count} bundles written")
        self.f.seek(0)
        self.f.write(struct.pack("<II", BLOB_MAGIC, self.hdr_end))
        self.f.write(struct.pack(f"<{self.count}I", *self.ends))
        self.f.close()

    def abort(self):
        self.f.close()
        if os.path.exists(self.tmp):
            os.remove(self.tmp)


def rebuild(a, plan, numbers):
    """Rebuild every planned bundle in parallel straight into <pkg>/<numbers[pkg]>.blob.mmvoice.tmp.
    -> ({pkg: BlobWriter}, {pkg: {(blob_no, slot): (new_blob_no, new_slot)}})"""
    writers, redirect = {}, {pkg: {} for pkg in plan}
    tasks = [(a.root, rel, slot, need, a.pack) for items in plan.values() for rel, slot, need in items]
    t0, total = time.time(), len(tasks)
    try:
        for pkg, items in plan.items():
            writers[pkg] = BlobWriter(os.path.join(a.root, pkg, f"{numbers[pkg]:04d}.blob"), len(items))
        with ProcessPoolExecutor(a.jobs) as ex:
            futs = {ex.submit(_rebuild_bundle, *t) for t in tasks}
            try:
                for n, f in enumerate(as_completed(futs), 1):
                    rel, slot, new = f.result()
                    futs.discard(f)  # drop the finished future so its bytes can be freed
                    pkg = os.path.dirname(rel)
                    redirect[pkg][(blob_no(rel), slot)] = (numbers[pkg], writers[pkg].append(new))
                    del new
                    if n % 50 == 0 or n == total:
                        print(f"[rebuild] {n}/{total}  {time.time() - t0:.0f}s", file=sys.stderr, flush=True)
            except BaseException:
                ex.shutdown(wait=False, cancel_futures=True)
                raise
        for w in writers.values():
            w.finish()
    except BaseException:
        for w in writers.values():
            w.abort()
        raise
    return writers, redirect


# ---------------------------------------------------------------- index / state
INDEX_MAGIC = 0xE6A9C3BA


INDEX_FLAG = 0x8000  # [確認] set on every entry of the */Starter indexes (blob = 0x8000 | NNNN); meaning unknown
BLOB_MASK = 0x7FFF


def patch_index(buf, redirect):
    """redirect: {(blob_no, slot): (new_blob_no, new_slot)} -> (new_bytes, entries_changed).
    The 0x8000 flag of the blob field is ignored for matching and kept on the rewritten entry."""
    magic, n = struct.unpack_from("<II", buf, 0)
    if magic != INDEX_MAGIC:
        raise ValueError("bad index magic")
    pairs = list(struct.unpack_from(f"<{n * 2}H", buf, 8))
    changed = 0
    for k in range(n):
        b = pairs[2 * k]
        hit = redirect.get((b & BLOB_MASK, pairs[2 * k + 1]))
        if hit:
            pairs[2 * k], pairs[2 * k + 1] = (b & INDEX_FLAG) | hit[0], hit[1]
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
    write_json_atomic(state_paths(data_dir)[1], st, indent=1, sort_keys=True)


def is_patched_index(buf):
    """True if any entry points at a blob >= REDIRECT_BASE, i.e. the index was written by us
    (original indexes only reference NNNN.blob below ~800)."""
    magic, n = struct.unpack_from("<II", buf, 0)
    if magic != INDEX_MAGIC:
        raise ValueError("bad index magic")
    return any((b & BLOB_MASK) >= REDIRECT_BASE for b in struct.unpack_from(f"<{n * 2}H", buf, 8)[0::2])


def peek_original_index(a, pkg, st):
    """Read-only. -> (live index is ours, original index bytes). Exits if the index is missing or is
    patched without a matching state record."""
    idx_path = os.path.join(a.root, pkg, "index")
    if not os.path.exists(idx_path):
        sys.exit(f"{pkg}/index missing (launcher updating/repairing?); nothing was changed")
    rec = st.get(pkg)
    if rec and sha1_file(idx_path) == rec["patched_sha1"]:
        with open(os.path.join(state_paths(a.data)[0], pkg, "index"), "rb") as f:
            return True, f.read()
    with open(idx_path, "rb") as f:
        live = f.read()
    if is_patched_index(live):
        sys.exit(f"{pkg}/index is already patched but not recorded in {a.data}/state.json "
                 f"(different --data?); restore {pkg}/index.bak first. Nothing was changed.")
    return False, live


def original_index(a, pkg, st):
    """-> (rec, original index bytes). Uses our backup if the live index is the one we wrote,
    otherwise treats the live index as a fresh original (install / update / repair) and backs it up,
    both to <data>/backup and in place as <pkg>/index.bak. A patched index is never backed up:
    detected by content (redirected entries), not only by state.json."""
    base, _ = state_paths(a.data)
    idx_path = os.path.join(a.root, pkg, "index")
    bak = os.path.join(base, pkg, "index")
    ours, _orig = peek_original_index(a, pkg, st)
    rec, cur = st.get(pkg), sha1_file(idx_path)
    if not ours:
        os.makedirs(os.path.dirname(bak), exist_ok=True)
        shutil.copy2(idx_path, bak)
        shutil.copy2(idx_path, idx_path + ".bak.mmvoice.tmp")
        os.replace(idx_path + ".bak.mmvoice.tmp", idx_path + ".bak")
        rec = {"orig_sha1": cur, "blobs": (rec or {}).get("blobs", [])}
    with open(bak, "rb") as f:
        return rec, f.read()


# ---------------------------------------------------------------- patch (entry)
def cmd_patch(a):
    if not (REDIRECT_BASE <= a.single and a.single + 1 <= BLOB_MASK):
        sys.exit(f"--blob-no must be in [{REDIRECT_BASE}, {BLOB_MASK - 1}] "
                 "(lower numbers would be scanned as originals; bit 15 is an index flag)")
    if not a.dry_run and game_running():
        sys.exit(f"{GAME_EXE} is running; close the game first.")
    man = load_manifest(a.pack)
    scan = scan_install(a.root, a.data, a.jobs, a.rescan)
    st = load_state(a.data)
    live = {pkg: index_slots(peek_original_index(a, pkg, st)[1])
            for pkg in sorted({os.path.dirname(r) for r in scan})}
    plan = plan_patch(scan, man, live)
    n_blobs = len({rel for items in plan.values() for rel, _s, _n in items})
    print(f"patch: {n_blobs} blobs / {sum(map(len, plan.values()))} bundles / "
          f"{sum(len(n) for items in plan.values() for _r, _s, n in items)} clips need JP audio")
    if a.dry_run:
        return

    shutil.rmtree(os.path.join(a.data, "stage"), ignore_errors=True)  # leftovers of the old staging design
    pkgs = sorted(set(plan) | set(st))  # previously patched packages that need nothing now get restored
    ctx, numbers = {}, {}
    for pkg in pkgs:
        rec, orig = original_index(a, pkg, st)
        old_blobs = list(rec.get("blobs", []))
        # the live index may point at <single>.blob: then write <single+1>.blob, and vice versa
        numbers[pkg] = a.single if f"{a.single:04d}.blob" not in old_blobs else a.single + 1
        ctx[pkg] = (rec, orig, old_blobs)

    writers, redirect = rebuild(a, plan, numbers) if plan else ({}, {})

    # build every new index before touching anything; each rebuilt bundle must be referenced
    new_indexes = {}
    try:
        for pkg in pkgs:
            orig = ctx[pkg][1]
            n = struct.unpack_from("<I", orig, 4)[0]
            pairs = struct.unpack_from(f"<{2 * n}H", orig, 8)
            present = {(pairs[2 * k] & BLOB_MASK, pairs[2 * k + 1]) for k in range(n)}
            unref = [k for k in redirect.get(pkg, {}) if k not in present]
            if unref:
                raise RuntimeError(f"{pkg}: {len(unref)} rebuilt bundles not referenced by index, e.g. {unref[:3]}")
            new_indexes[pkg] = patch_index(orig, redirect.get(pkg, {}))
    except BaseException:
        for w in writers.values():
            w.abort()
        raise

    for pkg in pkgs:
        rec, orig, old_blobs = ctx[pkg]
        new_blobs = []
        if pkg in writers:
            name = f"{numbers[pkg]:04d}.blob"
            os.replace(writers[pkg].tmp, os.path.join(a.root, pkg, name))
            new_blobs.append(name)
        new_idx, changed = new_indexes[pkg]
        idx_path = os.path.join(a.root, pkg, "index")
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
        print(f"patch: {pkg}: {new_blobs[0] if new_blobs else 'no blob'} "
              f"({len(redirect.get(pkg, {}))} bundles), {changed} index entries redirected")
    print(f"patch: done. Original blobs untouched; index backups in {state_paths(a.data)[0]}")


def cmd_restore(a):
    if game_running():
        sys.exit(f"{GAME_EXE} is running; close the game first.")
    base, _ = state_paths(a.data)
    st = load_state(a.data)
    for pkg in sorted(st):
        rec = st[pkg]
        idx_path = os.path.join(a.root, pkg, "index")
        bak = os.path.join(base, pkg, "index")
        if os.path.exists(idx_path) and sha1_file(idx_path) == rec["patched_sha1"]:
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
        if os.path.exists(bak):
            os.remove(bak)
        save_state(a.data, st)


def cmd_status(a):
    st = load_state(a.data)
    if not st:
        print("not patched")
    for pkg, rec in sorted(st.items()):
        idx_path = os.path.join(a.root, pkg, "index")
        if not os.path.exists(idx_path):
            state = "MISSING (launcher updating?)"
        elif sha1_file(idx_path) == rec["patched_sha1"]:
            state = "patched"
        else:
            state = "REVERTED/CHANGED (run patch again)"
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
    p.add_argument("--dry-run", action="store_true", help="scan + plan only, change nothing")
    p.add_argument("--rescan", action="store_true", help="ignore the scan cache")
    p.add_argument("--blob-no", "--single", dest="single", type=int, default=9876,
                   help="blob number for the patched bundles of each package (default 9876; "
                        "N and N+1 alternate between runs so the live index never points at a half-written file)")
    r = sub.add_parser("restore")
    r.add_argument("root")
    r.add_argument("--keep-blobs", action="store_true", help="only restore index, leave redirect blobs")
    s = sub.add_parser("status")
    s.add_argument("root")
    for x in (p, r, s):
        x.add_argument("--data", default="mmvoice_data",
                       help="where backups, state.json and the scan cache live (default: ./mmvoice_data)")
    a = ap.parse_args()
    {"dump": cmd_dump, "patch": cmd_patch, "restore": cmd_restore, "status": cmd_status}[a.cmd](a)


if __name__ == "__main__":
    main()
