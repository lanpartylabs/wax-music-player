#!/usr/bin/env python3
"""Make a small music library of made-up albums, to try wax without your own music.

    python3 tools/make_demo_library.py ~/wax-demo

Every album is a folder of short WAV files with tags (title, artist, album,
genre, year, track numbers; as RIFF INFO and as ID3) and a cover.png. The music is generated: simple
chord arpeggios, a few seconds each. The bands and songs are invented. Only
the Python standard library is needed.
"""

import argparse
import array
import math
import os
import random
import struct
import zlib

RATE = 22050

# Artist, album, year, genre, key (semitones above A3), tempo, cover colours, song titles.
ALBUMS = [
    ("Juniper Static", "Night Bus Hymns", 2021, "Indie Rock", 3, 112, ((32, 38, 92), (226, 92, 76)),
     ["Last Stop", "Sodium Lights", "Window Seat", "Fare Dodger", "Terminus"]),
    ("Marta Quell", "Paper Satellites", 2017, "Pop", 7, 118, ((250, 210, 120), (230, 90, 140)),
     ["Orbit", "Paper Satellites", "Say It Twice", "Static Hearts"]),
    ("The Low Orchard", "Copper Fields", 2012, "Folk", 0, 92, ((70, 110, 60), (230, 180, 100)),
     ["Copper Fields", "Old Mill Road", "Harvest Waltz", "Rain on the Barn"]),
    ("Selva Drift", "Tidewrack", 2019, "Electronic", 5, 124, ((10, 60, 80), (40, 220, 190)),
     ["Tidewrack", "Riptide", "Salt Glass", "Low Pressure"]),
    ("Ostra Lane", "Glass Weather", 2023, "Alternative", 10, 104, ((200, 210, 230), (90, 100, 140)),
     ["Glass Weather", "Fog Lamp", "Northbound"]),
    ("Harbor & Pine", "Long Light", 2008, "Jazz", 2, 84, ((40, 30, 30), (210, 150, 70)),
     ["Long Light", "Blue Hour", "Lantern", "Quiet Pier"]),
    ("Neon Cartography", "Grid City", 2015, "Electronic", 8, 128, ((20, 10, 40), (250, 60, 200)),
     ["Grid City", "Overpass", "Night Market", "Signal Kite", "Arcade"]),
    ("Willa Fenn", "Small Hours", 2020, "Singer-Songwriter", 4, 96, ((240, 230, 215), (120, 90, 80)),
     ["Small Hours", "Kitchen Light", "Postcard Ink", "Sunday Coat"]),
]

PROGRESSIONS = [[0, 7, 9, 5], [0, 5, 7, 5], [9, 5, 0, 7], [0, 9, 5, 7]]
MAJOR = [0, 4, 7]
MINOR = [0, 3, 7]


def note(freq, seconds, loud):
    """One plucked note: a sine with a soft octave above it, fading out."""
    n = int(seconds * RATE)
    return [loud * math.exp(-3.0 * i / n) * (math.sin(2 * math.pi * freq * i / RATE) + 0.25 * math.sin(4 * math.pi * freq * i / RATE))
            for i in range(n)]


def song(key, tempo, seed, seconds):
    """Arpeggios over a four-chord loop, with a bass note on each chord."""
    rng = random.Random(seed)
    progression = rng.choice(PROGRESSIONS)
    beat = 60.0 / tempo / 2  # eighth notes
    out = [0.0] * int(seconds * RATE)
    t, chord = 0.0, 0
    while t < seconds - 1:
        root = key + progression[chord % 4]
        shape = MINOR if progression[chord % 4] == 9 else MAJOR
        bass = note(110 * 2 ** ((root - 12) / 12), beat * 8, 0.35)
        start = int(t * RATE)
        for i, v in enumerate(bass[: max(0, len(out) - start)]):
            out[start + i] += v
        for step in range(8):
            pitch = root + shape[rng.choice([0, 1, 2, 1])] + 12 * rng.choice([0, 0, 1])
            plucked = note(220 * 2 ** (pitch / 12), beat * 2, 0.22)
            start = int((t + step * beat) * RATE)
            for i, v in enumerate(plucked[: max(0, len(out) - start)]):
                out[start + i] += v
        t += beat * 8
        chord += 1
    fade = int(1.5 * RATE)
    for i in range(fade):
        out[-1 - i] *= i / fade
    return array.array("h", (int(max(-1.0, min(1.0, v)) * 26000) for v in out))


def riff_info(tags):
    """A LIST INFO chunk of the tags, as WAV files carry them."""
    body = b"INFO"
    for key, value in tags.items():
        data = value.encode("utf-8") + b"\0"
        if len(data) % 2:
            data += b"\0"
        body += key.encode() + struct.pack("<I", len(data)) + data
    return b"LIST" + struct.pack("<I", len(body)) + body


def id3(tags):
    """The same tags as an ID3v2.4 block, which more programs read in a WAV file than its own LIST INFO."""
    frames = {"INAM": "TIT2", "IART": "TPE1", "IPRD": "TALB", "IGNR": "TCON", "ICRD": "TDRC"}
    syncsafe = lambda n: bytes((n >> shift) & 0x7F for shift in (21, 14, 7, 0))
    body = b""
    for key, value in [*((frames[k], v) for k, v in tags.items() if k in frames), ("TRCK", f"{tags['IPRT']}/{tags['IFRM']}")]:
        data = b"\x03" + value.encode("utf-8")
        body += key.encode() + syncsafe(len(data)) + b"\0\0" + data
    return b"ID3\x04\x00\x00" + syncsafe(len(body)) + body


def write_wav(path, samples, tags):
    data = samples.tobytes()
    fmt = struct.pack("<HHIIHH", 1, 1, RATE, RATE * 2, 2, 16)
    tagged = id3(tags) + (b"\0" if len(id3(tags)) % 2 else b"")
    chunks = (b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data + riff_info(tags)
              + b"id3 " + struct.pack("<I", len(tagged)) + tagged)
    with open(path, "wb") as f:
        f.write(b"RIFF" + struct.pack("<I", 4 + len(chunks)) + b"WAVE" + chunks)


def write_cover(path, colours, style, size=400):
    """A square of two colours: a diagonal blend, with rings, a sun or stripes."""
    (r1, g1, b1), (r2, g2, b2) = colours
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            share = (x + y) / (2 * size)
            cx, cy = x - size * 0.5, y - size * 0.55
            distance = math.hypot(cx, cy) / size
            if style == 0:
                share = 0.5 + 0.5 * math.sin(distance * 40)
            elif style == 1:
                share = 1.0 if distance < 0.22 else share * 0.6
            elif style == 2:
                share = 1.0 if (y // 25) % 2 and y > size * 0.45 else share * 0.5
            row += bytes((int(r1 + (r2 - r1) * share), int(g1 + (g2 - g1) * share), int(b1 + (b2 - b1) * share)))
        rows.append(bytes(row))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("folder", help="where to make the library; made if it is not there")
    parser.add_argument("--seconds", type=float, default=14.0, help="length of each song (default 14)")
    args = parser.parse_args()
    for n, (artist, album, year, genre, key, tempo, colours, songs) in enumerate(ALBUMS):
        folder = os.path.join(os.path.expanduser(args.folder), artist, album)
        os.makedirs(folder, exist_ok=True)
        write_cover(os.path.join(folder, "cover.png"), colours, n % 4)
        for number, title in enumerate(songs, 1):
            tags = {"INAM": title, "IART": artist, "IPRD": album, "IGNR": genre, "ICRD": str(year),
                    "IPRT": str(number), "IFRM": str(len(songs))}
            samples = song(key, tempo, f"{album}/{number}", args.seconds + 3 * ((number * 7) % 3))
            write_wav(os.path.join(folder, f"{number:02d} {title}.wav"), samples, tags)
        print(f"{artist} - {album}: {len(songs)} songs")


if __name__ == "__main__":
    main()
