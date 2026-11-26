"""Mabinogi Mobile .blob container reader.

Observed layout (little-endian):
  0x00  u32 magic   = 0xE5A9C3BA  (bytes BA C3 A9 E5)
  0x04  u32 hdr_end = offset where the first UnityFS bundle starts
  0x08  u32[]       = cumulative END offsets of each bundle ((hdr_end-8)/4 entries);
                      the last entry equals the file size.
Each slice [prev_end, end) is a plain UnityFS asset bundle.
"""
import struct
import sys

BLOB_MAGIC = 0xE5A9C3BA


def read_blob_table(path):
    with open(path, "rb") as f:
        magic, hdr_end = struct.unpack("<II", f.read(8))
        if magic != BLOB_MAGIC:
            raise ValueError(f"{path}: bad magic {magic:#x}")
        n = (hdr_end - 8) // 4
        ends = struct.unpack(f"<{n}I", f.read(4 * n))
    starts = (hdr_end,) + ends[:-1]
    return list(zip(starts, ends))


def iter_bundles(path):
    """Yield (index, start, bytes) for each UnityFS bundle in the blob."""
    table = read_blob_table(path)
    with open(path, "rb") as f:
        for i, (s, e) in enumerate(table):
            f.seek(s)
            yield i, s, f.read(e - s)


if __name__ == "__main__":
    for p in sys.argv[1:]:
        t = read_blob_table(p)
        print(p, len(t), "bundles")
