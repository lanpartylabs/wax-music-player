"""Read what a file is and every tag it carries, in one shape whatever the format.

`read(path)` returns a dict of evidence columns. It tries mutagen first, which
reads every tag format in the collection; if mutagen cannot parse the file it
asks ffprobe, which is more forgiving of damaged files.
"""

import json
import re
import subprocess

import mutagen
from mutagen.apev2 import APEv2
from mutagen.id3 import ID3

LONGEST = 500  # characters kept of any one tag value

ID3_KEYS = {
    "tit2": "title", "tpe1": "artist", "tpe2": "albumartist", "talb": "album", "trck": "track", "tpos": "disc",
    "tdrc": "date", "tdor": "originaldate", "tcon": "genre", "tcmp": "compilation", "tsrc": "isrc",
    "ufid:http://musicbrainz.org": "mb_recording", "txxx:musicbrainz album id": "mb_release",
    "txxx:musicbrainz release group id": "mb_releasegroup", "txxx:musicbrainz release track id": "mb_track",
    "txxx:acoustid id": "acoustid",
}
MP4_KEYS = {
    "\xa9nam": "title", "\xa9art": "artist", "aart": "albumartist", "\xa9alb": "album", "trkn": "track", "disk": "disc",
    "\xa9day": "date", "\xa9gen": "genre", "cpil": "compilation", "----:com.apple.itunes:isrc": "isrc",
    "----:com.apple.itunes:originaldate": "originaldate", "----:com.apple.itunes:musicbrainz track id": "mb_recording",
    "----:com.apple.itunes:musicbrainz album id": "mb_release",
    "----:com.apple.itunes:musicbrainz release group id": "mb_releasegroup",
    "----:com.apple.itunes:musicbrainz release track id": "mb_track", "----:com.apple.itunes:acoustid id": "acoustid",
}
VORBIS_KEYS = {
    "title": "title", "artist": "artist", "albumartist": "albumartist", "album artist": "albumartist",
    "album_artist": "albumartist", "album": "album", "tracknumber": "track", "tracktotal": "tracktotal",
    "totaltracks": "tracktotal", "discnumber": "disc", "disctotal": "disctotal", "totaldiscs": "disctotal",
    "date": "date", "year": "date", "originaldate": "originaldate", "genre": "genre", "compilation": "compilation",
    "isrc": "isrc", "musicbrainz_trackid": "mb_recording", "musicbrainz_albumid": "mb_release",
    "musicbrainz_releasegroupid": "mb_releasegroup", "musicbrainz_releasetrackid": "mb_track", "acoustid_id": "acoustid",
}
ASF_KEYS = {
    "title": "title", "author": "artist", "wm/albumartist": "albumartist", "wm/albumtitle": "album",
    "wm/tracknumber": "track", "wm/partofset": "disc", "wm/year": "date", "wm/originalreleaseyear": "originaldate",
    "wm/genre": "genre", "wm/iscompilation": "compilation", "wm/isrc": "isrc", "musicbrainz/track id": "mb_recording",
    "musicbrainz/album id": "mb_release", "musicbrainz/release group id": "mb_releasegroup",
    "musicbrainz/release track id": "mb_track", "acoustid/id": "acoustid",
}
APE_KEYS = {**VORBIS_KEYS, "track": "track", "disc": "disc"}
FFPROBE_KEYS = {**VORBIS_KEYS, "track": "track", "disc": "disc", "tsrc": "isrc"}

# mutagen class -> (container, codec, lossless); None where the file's own info decides
FORMATS = {
    "MP3": ("mp3", "mp3", 0), "MP4": ("mp4", None, None), "FLAC": ("flac", "flac", 1), "ASF": ("asf", None, None),
    "OggVorbis": ("ogg", "vorbis", 0), "OggOpus": ("ogg", "opus", 0), "OggFLAC": ("ogg", "flac", 1),
    "MonkeysAudio": ("ape", "ape", 1), "WavPack": ("wavpack", "wavpack", 1), "Musepack": ("musepack", "musepack", 0),
    "WAVE": ("wav", "pcm", 1), "AIFF": ("aiff", "pcm", 1), "AAC": ("aac", "aac", 0), "AC3": ("ac3", "ac3", 0),
}


def clean(value):
    return " ".join(str(value).replace("\0", " ").split())[:LONGEST]


def number(text):
    """The number, and the total when written "5/10" or "5 of 10"."""
    match = re.match(r"\s*(\d+)(?:\s*(?:/|of)\s*(\d+))?", text or "")
    if not match:
        return None, None
    return int(match[1]) or None, (int(match[2]) or None) if match[2] else None


def id3_tags(tags):
    raw, pictures, lyrics = {}, 0, False
    for key, frame in tags.items():
        kind = key[:4]
        if kind == "APIC":
            pictures += 1
        elif kind == "USLT":
            lyrics = lyrics or bool(str(getattr(frame, "text", "")).strip())
        elif kind == "UFID":
            raw[key] = [frame.data.decode("latin-1")]
        elif kind == "TCON":
            try:
                raw[key] = list(frame.genres)
            except Exception:  # a genre number mutagen cannot make sense of
                raw[key] = [str(value) for value in frame.text]
        elif hasattr(frame, "text"):
            raw[key] = [str(value) for value in frame.text] if isinstance(frame.text, list) else [str(frame.text)]
        elif hasattr(frame, "url"):
            raw[key] = [frame.url]
        else:
            raw[key] = []  # present, but not text
    return raw, pictures, lyrics


def mp4_tags(tags):
    raw, pictures, lyrics = {}, 0, False
    for key, values in tags.items():
        if key == "covr":
            pictures += len(values)
        elif key == "\xa9lyr":
            lyrics = True
        elif key in ("trkn", "disk"):
            raw[key] = ["/".join(str(part) for part in pair) for pair in values]
        elif key.startswith("----"):
            raw[key] = [bytes(value).decode("utf-8", "replace") for value in values]
        elif isinstance(values, list):
            raw[key] = [str(value) for value in values if not isinstance(value, bytes)]
        else:
            raw[key] = [str(int(values)) if isinstance(values, bool) else str(values)]
    return raw, pictures, lyrics


def vorbis_tags(tags, audio):
    raw = {key.lower(): [str(value) for value in tags[key]] for key in tags.keys()}
    pictures = len(getattr(audio, "pictures", [])) + len(raw.pop("metadata_block_picture", []))
    return raw, pictures, bool(raw.get("lyrics") or raw.get("unsyncedlyrics"))


def asf_tags(tags):
    raw, pictures = {}, 0
    for key in tags.keys():
        values = [getattr(value, "value", value) for value in tags[key]]
        if key == "WM/Picture":
            pictures += len(values)
        else:
            raw[key] = [str(int(value)) if isinstance(value, bool) else str(value) for value in values if not isinstance(value, bytes)]
    return raw, pictures, bool(raw.get("WM/Lyrics"))


def ape_tags(tags):
    raw, pictures = {}, 0
    for key, value in tags.items():
        if getattr(value, "kind", 0) == 0:
            raw[key] = str(value).split("\0")
        elif key.lower().startswith("cover art"):
            pictures += 1
    return raw, pictures, bool(raw.get("Lyrics"))


def fields(raw, keys):
    """The catalogue's columns from one tag format's own names. A tag written several times is joined."""
    out = {}
    lowered = {key.lower(): values for key, values in raw.items()}
    for key, column in keys.items():
        values = [clean(value) for value in lowered.get(key, []) if clean(value)]
        if not values or column in out:
            continue
        # several artists or genres are all meant; a second date or title is a leftover
        text = "; ".join(dict.fromkeys(values)) if column in ("artist", "albumartist", "genre") else values[0]
        if column in ("track", "disc"):
            out[column], total = number(text)
            if total:
                out[column + "total"] = total
        elif column in ("tracktotal", "disctotal"):
            out[column] = number(text)[0]
        elif column == "compilation":
            out[column] = int(text.lower() in ("1", "true", "yes"))
        else:
            out[column] = text
    if "date" not in out:  # ID3v2.3 keeps the year apart; mutagen normally folds it into TDRC
        year = [clean(value) for value in lowered.get("tyer", []) if clean(value)]
        if year:
            out["date"] = year[0]
    return out


def merge(row, raw, pictures, lyrics, keys, kind):
    """Add one tag block's contents; a block read earlier keeps the columns it already filled."""
    for column, value in fields(raw, keys).items():
        row.setdefault(column, value)
    row["pictures"] = row.get("pictures", 0) + pictures
    row["lyrics"] = int(row.get("lyrics", 0) or lyrics)
    row["kinds"].append(kind)
    row["raw"].update({f"{kind}:{key}" if key in row["raw"] else key: [clean(value) for value in values] for key, values in raw.items()})


def id3_kind(tags):
    """"id3v1" when only the old 30-character block was found, else the ID3v2 revision."""
    return "id3v1" if tags.version[0] == 1 else f"id3v2.{tags.version[1]}"


def id3v1_present(path):
    try:
        with open(path, "rb") as f:
            f.seek(-128, 2)
            return f.read(3) == b"TAG"
    except OSError:
        return False


def read_mutagen(path):
    audio = mutagen.File(path)
    if audio is None:
        return None
    name = type(audio).__name__
    info = audio.info
    container, codec, lossless = FORMATS.get(name, (name.lower(), None, None))
    if name == "MP4":
        codec = getattr(info, "codec", "") or "aac"
        lossless = int(codec == "alac")
    elif name == "ASF":
        codec = getattr(info, "codec_name", "") or "wma"
        lossless = int("lossless" in codec.lower())
    elif name == "MP3":
        mode = str(getattr(info, "bitrate_mode", "")).rsplit(".", 1)[-1].lower()
        codec = " ".join(part for part in ("mp3", mode if mode != "unknown" else "", getattr(info, "encoder_info", "")) if part)
    row = {
        "reader": "mutagen", "container": container, "codec": codec, "lossless": lossless,
        "bitrate": round(info.bitrate / 1000) if getattr(info, "bitrate", 0) else None,
        "sample_rate": getattr(info, "sample_rate", None), "channels": getattr(info, "channels", None),
        "bits": getattr(info, "bits_per_sample", None),
        "duration": round(info.length, 3) if getattr(info, "length", 0) else None,
        "doubtful": int(bool(getattr(info, "sketchy", False))),
        "pictures": 0, "lyrics": 0, "kinds": [], "raw": {},
    }
    tags = audio.tags
    if isinstance(tags, ID3):
        merge(row, *id3_tags(tags), ID3_KEYS, id3_kind(tags))
        if tags.version[0] == 2 and id3v1_present(path):
            row["kinds"].append("id3v1")
    elif name == "MP4" and tags is not None:
        merge(row, *mp4_tags(tags), MP4_KEYS, "mp4")
    elif name == "ASF" and tags is not None:
        merge(row, *asf_tags(tags), ASF_KEYS, "asf")
    elif isinstance(tags, APEv2):
        merge(row, *ape_tags(tags), APE_KEYS, "ape")
    elif tags is not None:
        merge(row, *vorbis_tags(tags, audio), VORBIS_KEYS, "vorbis")
    if name in ("MP3", "AAC"):  # these can carry an APE block that mutagen leaves alone
        if name == "AAC":
            try:
                id3 = ID3(path)
                merge(row, *id3_tags(id3), ID3_KEYS, id3_kind(id3))
            except mutagen.MutagenError:
                pass
        try:
            merge(row, *ape_tags(APEv2(path)), APE_KEYS, "ape")
        except mutagen.MutagenError:
            pass
    return row


def read_ffprobe(path):
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=format_name,duration,bit_rate:format_tags:stream=codec_type,codec_name,sample_rate,channels",
         "-of", "json", path],
        capture_output=True, timeout=60,
    )
    data = json.loads(out.stdout or b"{}")
    form = data.get("format")
    stream = next((s for s in data.get("streams", []) if s.get("codec_type") == "audio"), None)
    if not form or not stream:
        return None
    raw = {key: [value] for key, value in (form.get("tags") or {}).items()}
    row = {
        "reader": "ffprobe", "container": form.get("format_name", "").split(",")[0], "codec": stream.get("codec_name"),
        "bitrate": round(int(form["bit_rate"]) / 1000) if form.get("bit_rate", "").isdigit() else None,
        "sample_rate": int(stream["sample_rate"]) if stream.get("sample_rate") else None, "channels": stream.get("channels"),
        "duration": round(float(form["duration"]), 3) if form.get("duration") else None,
        "pictures": 0, "lyrics": 0, "kinds": [], "raw": {},
    }
    if raw:
        merge(row, raw, 0, False, FFPROBE_KEYS, "ffprobe")
    return row


def read(path):
    """Evidence columns for one audio file. `error` is set when no reader could make sense of it."""
    error, row = None, None
    try:
        row = read_mutagen(path)
    except Exception as e:  # mutagen raises many kinds of error on damaged files
        error = f"mutagen: {type(e).__name__}: {e}"[:300]
    if row is None:
        try:
            row = read_ffprobe(path)
        except Exception as e:
            error = f"{error + '; ' if error else ''}ffprobe: {type(e).__name__}: {e}"[:300]
    if row is None:
        return {"error": error or "not recognised as audio"}
    row["error"] = error  # kept when ffprobe succeeded where mutagen failed
    row["tag_kinds"] = " ".join(dict.fromkeys(row.pop("kinds")))
    row["tags"] = json.dumps(row.pop("raw"), ensure_ascii=False, sort_keys=True)
    return row
