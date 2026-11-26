"""Replace AudioClip data inside a UnityFS bundle with clips taken from another bundle set.

Core function: patch_bundle(tw_bytes, jp_clips) -> (new_bytes, n_replaced)
  jp_clips: {container_path: ClipData}

The bundle's audio stream (.resource) is rebuilt from scratch: replaced clips get JP bytes,
untouched clips keep their original bytes. Every AudioClip's m_Resource offset/size is rewritten,
and replaced clips also take JP's format fields (length, channels, frequency, ...).
"""
from dataclasses import dataclass

import UnityPy
from UnityPy.helpers.ResourceReader import get_resource_data
from UnityPy.streams import EndianBinaryReader

COPY_FIELDS = ("m_LoadType", "m_Channels", "m_Frequency", "m_BitsPerSample", "m_Length",
               "m_IsTrackerFormat", "m_Ambisonic", "m_SubsoundIndex", "m_CompressionFormat")
ALIGN = 32  # [推論] conservative alignment for streamed audio chunks


@dataclass
class ClipData:
    fields: dict
    data: bytes


def clip_bytes(obj, tree):
    r = tree["m_Resource"]
    return bytes(get_resource_data(r["m_Source"], obj.assets_file, r["m_Offset"], r["m_Size"]))


def container_map(env):
    m = {}
    for path, ptr in env.container.items():
        try:
            m[ptr.path_id] = path
        except Exception:  # noqa: BLE001
            pass
    return m


def read_clips(bundle_bytes, wanted=None):
    """Return {container_path: ClipData} for AudioClips in a bundle."""
    env = UnityPy.load(bundle_bytes)
    cont = container_map(env)
    out = {}
    for obj in env.objects:
        if obj.type.name != "AudioClip":
            continue
        path = cont.get(obj.path_id)
        if not path or (wanted is not None and path not in wanted):
            continue
        t = obj.read_typetree()
        out[path] = ClipData({k: t[k] for k in COPY_FIELDS if k in t}, clip_bytes(obj, t))
    return out


def patch_bundle(tw_bytes, jp_clips):
    env = UnityPy.load(tw_bytes)
    bf = env.file
    cont = container_map(env)
    clips = [o for o in env.objects if o.type.name == "AudioClip"]
    if not any(cont.get(o.path_id) in jp_clips for o in clips):
        return tw_bytes, 0

    # group clips by the resource stream they live in
    streams = {}
    for o in clips:
        t = o.read_typetree()
        name = t["m_Resource"]["m_Source"].rsplit("/", 1)[-1]
        streams.setdefault(name, []).append((o, t))

    replaced = 0
    for name, items in streams.items():
        if name not in bf.files:
            raise RuntimeError(f"resource stream {name} not inside bundle")
        buf = bytearray()
        # keep original order of offsets so untouched layout stays similar
        items.sort(key=lambda it: it[1]["m_Resource"]["m_Offset"])
        for o, t in items:
            path = cont.get(o.path_id)
            jp = jp_clips.get(path)
            if jp is not None:
                data = jp.data
                t.update(jp.fields)
                replaced += 1
            else:
                data = clip_bytes(o, t)
            pad = (-len(buf)) % ALIGN
            buf += b"\0" * pad
            t["m_Resource"]["m_Offset"] = len(buf)
            t["m_Resource"]["m_Size"] = len(data)
            buf += data
            o.save_typetree(t)
        old = bf.files[name]
        new = EndianBinaryReader(bytes(buf))
        for attr in ("flags", "name"):  # BundleFile.save reads these from each entry
            if hasattr(old, attr):
                setattr(new, attr, getattr(old, attr))
        bf.files[name] = new
    bf.is_changed = True
    return bf.save(packer="lz4"), replaced
