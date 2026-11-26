"""Apply / restore patched blobs to the TW install.

  python tools/apply.py apply   AssetPackages/0260.blob [...]   (or 'all' for everything in out/)
  python tools/apply.py restore AssetPackages/0260.blob [...]   (or 'all' for everything in work/backup/)

apply: backs up the original to work/backup/<rel> (only if no backup exists yet), then copies out/<rel>.
restore: copies work/backup/<rel> back. Refuses to run while MabinogiMobile.exe is running.
"""
import glob
import hashlib
import os
import shutil
import subprocess
import sys

TW_ROOT = r"D:\MabinogiMobile_TW\appdata"


def sha1(p):
    h = hashlib.sha1()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def game_running():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq MabinogiMobile.exe"],
                         capture_output=True, text=True).stdout
    return "MabinogiMobile.exe" in out


def rels(base, args):
    if args == ["all"]:
        return [os.path.relpath(p, base) for p in glob.glob(os.path.join(base, "**", "*.blob"), recursive=True)]
    return [a.replace("/", os.sep) for a in args]


def main(cmd, *args):
    if game_running():
        sys.exit("MabinogiMobile.exe is running; close the game first.")
    if cmd == "apply":
        for rel in rels("out", list(args)):
            dst, src, bak = os.path.join(TW_ROOT, rel), os.path.join("out", rel), os.path.join("work", "backup", rel)
            if not os.path.exists(bak):
                os.makedirs(os.path.dirname(bak), exist_ok=True)
                shutil.copy2(dst, bak)
            shutil.copyfile(src, dst)
            ok = sha1(src) == sha1(dst)
            print(f"applied {rel}  verify={'ok' if ok else 'FAIL'}  backup={bak}")
    elif cmd == "restore":
        for rel in rels(os.path.join("work", "backup"), list(args)):
            dst, bak = os.path.join(TW_ROOT, rel), os.path.join("work", "backup", rel)
            shutil.copyfile(bak, dst)
            print(f"restored {rel}  verify={'ok' if sha1(bak) == sha1(dst) else 'FAIL'}")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(*sys.argv[1:])
