"""Gather evidence on every file under the music folders into the catalogue.

    python3 -m librarian.gather --root ~/Music --include FOLDER [--include ...] --db ~/wax/catalogue.db
                                [--fpcalc ~/wax/bin/fpcalc] [--jobs 2] [--only PREFIX] [--limit N]

Lists every file, then for each audio file that is new or has changed reads
its tags and properties and takes an audio fingerprint. Only reads the music.
Safe to stop and run again: files already gathered are skipped.
"""

import argparse
import concurrent.futures
import json
import os
import subprocess
import sys
import time

from . import catalogue, tags

AUDIO = set("mp3 m4a m4b m4p aac flac wma ape wav ogg oga opus wv mpc aif aiff mp2 ac3".split())
IMAGE = set("jpg jpeg png gif bmp webp tif tiff".split())
PLAYLIST = set("m3u m3u8 pls cue wpl".split())
FINGERPRINT_SECONDS = 120  # what AcoustID matches on


def kind_of(name):
    ext = name.rsplit(b".", 1)[-1].lower().decode("ascii", "replace") if b"." in name else ""
    return "audio" if ext in AUDIO else "image" if ext in IMAGE else "playlist" if ext in PLAYLIST else "other"


def walk(root, folder):
    """Every file under root/folder as (relative path, size, mtime); hidden entries and links are left out."""
    pending = [folder]
    while pending:
        here = pending.pop()
        try:
            entries = list(os.scandir(os.path.join(root, here)))
        except OSError as e:
            print(f"cannot list {os.fsdecode(here)}: {e.strerror}", file=sys.stderr)
            continue
        for entry in entries:
            if entry.name.startswith(b".") or entry.is_symlink():
                continue
            path = os.path.join(here, entry.name)
            try:
                if entry.is_dir():
                    pending.append(path)
                else:
                    stat = entry.stat()
                    yield path, stat.st_size, int(stat.st_mtime)
            except OSError as e:
                print(f"cannot stat {os.fsdecode(path)}: {e.strerror}", file=sys.stderr)


def fingerprint(fpcalc, path):
    """(decoded duration, Chromaprint, error) for the start of a file."""
    try:
        out = subprocess.run([fpcalc, "-json", "-length", str(FINGERPRINT_SECONDS), path], capture_output=True, timeout=300)
    except (OSError, subprocess.TimeoutExpired) as e:
        return None, None, f"{type(e).__name__}: {e}"[:300]
    if out.returncode != 0:
        return None, None, (out.stderr.decode("utf-8", "replace").strip() or f"fpcalc exited with {out.returncode}")[:300]
    try:
        data = json.loads(out.stdout)
        return data.get("duration"), data.get("fingerprint") or None, None if data.get("fingerprint") else "empty fingerprint"
    except ValueError:
        return None, None, "fpcalc gave no JSON"


def inspect(root, fpcalc, file):
    """All the evidence for one audio file."""
    path = os.path.join(root, file["path"])
    row = {"file": file["id"], "gathered": int(time.time()), "size": file["size"], "mtime": file["mtime"], "opens": 1}
    try:
        with open(path, "rb") as f:
            f.read(16)
    except OSError as e:
        return {**row, "opens": 0, "error": e.strerror}
    if file["size"] == 0:
        return {**row, "error": "empty file"}
    row.update(tags.read(path))
    if fpcalc:
        row["fp_duration"], row["fingerprint"], row["fp_error"] = fingerprint(fpcalc, path)
    return row


def main(argv=None):
    parser = argparse.ArgumentParser(prog="librarian gather", description=__doc__.split("\n\n")[0])
    parser.add_argument("--root", required=True, help="folder the music folders live in")
    parser.add_argument("--include", action="append", required=True, help="music folder under the root; repeat for several")
    parser.add_argument("--db", required=True, help="catalogue file")
    parser.add_argument("--fpcalc", default="~/wax/bin/fpcalc", help="Chromaprint's fpcalc; fingerprints are skipped when it is missing")
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--only", help="gather only files whose path starts with this")
    parser.add_argument("--limit", type=int, help="gather at most this many files")
    args = parser.parse_args(argv)

    root = os.fsencode(os.path.abspath(args.root))
    fpcalc = os.path.expanduser(args.fpcalc)
    if not os.access(fpcalc, os.X_OK):
        print(f"no fpcalc at {fpcalc}: gathering without fingerprints", file=sys.stderr)
        fpcalc = None
    db = catalogue.connect(args.db)
    started = int(time.time())

    listed = 0
    for folder in args.include:
        for path, size, mtime in walk(root, os.fsencode(folder)):
            db.execute(
                "INSERT INTO files (path, name, kind, size, mtime, seen) VALUES (?, ?, ?, ?, ?, ?) "
                "ON CONFLICT (path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, seen = excluded.seen",
                (path, path.decode("utf-8", "replace"), kind_of(path), size, mtime, started),
            )
            listed += 1
    db.commit()
    gone = db.execute("SELECT count(*) FROM files WHERE seen < ?", (started,)).fetchone()[0]
    print(f"listed {listed:,} files in {time.time() - started:.0f}s; {gone:,} catalogued earlier are no longer there", flush=True)

    todo = db.execute(
        "SELECT f.id, f.path, f.size, f.mtime FROM files f LEFT JOIN evidence e ON e.file = f.id "
        "WHERE f.kind = 'audio' AND f.seen = ? AND (e.file IS NULL OR e.size != f.size OR e.mtime != f.mtime) ORDER BY f.path",
        (started,),
    ).fetchall()
    if args.only:
        todo = [file for file in todo if file["path"].startswith(os.fsencode(args.only))]
    todo = todo[: args.limit] if args.limit else todo
    print(f"{len(todo):,} audio files to gather", flush=True)

    done, began = 0, time.time()
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for row in pool.map(lambda file: inspect(root, fpcalc, file), todo):
            catalogue.save_evidence(db, row)
            done += 1
            if done % 100 == 0:
                db.commit()
            if done % 1000 == 0:
                rate = done / (time.time() - began)
                print(f"{done:,} of {len(todo):,} gathered, {rate:.1f} files/s, about {(len(todo) - done) / rate / 60:.0f} min left", flush=True)
    db.commit()

    counts = db.execute(
        "SELECT count(*) AS files, sum(opens = 0) AS unopenable, sum(opens = 1 AND reader IS NULL) AS unreadable, "
        "sum(reader = 'ffprobe') AS rescued, sum(title IS NULL AND opens = 1 AND reader IS NOT NULL) AS untitled, "
        "sum(fingerprint IS NOT NULL) AS fingerprinted FROM evidence"
    ).fetchone()
    print(f"gathered {done:,} files in {(time.time() - began) / 60:.1f} min. Catalogue now holds evidence on {counts['files']:,} audio files: "
          f"{counts['fingerprinted']:,} fingerprinted, {counts['unopenable']:,} cannot be opened, {counts['unreadable']:,} not readable as audio, "
          f"{counts['rescued']:,} read by ffprobe after mutagen failed, {counts['untitled']:,} with no title tag", flush=True)


if __name__ == "__main__":
    main()
