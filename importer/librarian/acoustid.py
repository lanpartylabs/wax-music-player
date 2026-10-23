"""Ask AcoustID which recordings each file's fingerprint belongs to.

    python3 -m librarian hear [--limit N]

AcoustID matches the sound itself, so it knows a file whatever its tags and
name say. Needs ACOUSTID_KEY in secrets.env (free, from
https://acoustid.org/new-application). Three lookups a second at most.
"""

import argparse
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib

from . import catalogue, config

API = "https://api.acoustid.org/v2/lookup"
PAUSE = 0.35
KEEP_RELEASES = 150  # per recording; a hit single is on thousands of compilations


class Refused(Exception):
    """AcoustID turned the request down for a reason that will not go away by itself, such as a bad key."""


def lookup(key, fingerprint, seconds):
    """What AcoustID holds for one fingerprint, cut down to recordings, their artists and the releases they are on."""
    form = urllib.parse.urlencode({
        "client": key, "format": "json", "duration": int(round(seconds)), "fingerprint": fingerprint,
        "meta": "recordings releasegroups releases compress",
    }).encode()
    try:
        with urllib.request.urlopen(urllib.request.Request(API, data=form), timeout=40) as response:
            data = json.load(response)
    except urllib.error.HTTPError as e:
        try:
            data = json.load(e)
        except ValueError:
            raise OSError(f"AcoustID answered {e.code}") from e
    if data.get("status") != "ok":
        error = data.get("error") or {}
        if error.get("code") in (4, 5, 6):  # invalid key, invalid user key, invalid client
            raise Refused(error.get("message", "refused"))
        raise OSError(error.get("message", "AcoustID error"))
    return [
        {
            "score": round(result.get("score", 0), 3),
            "recordings": [
                {
                    "id": recording["id"],
                    "title": recording.get("title", ""),
                    "artist": "".join(a.get("name", "") + a.get("joinphrase", "") for a in recording.get("artists", [])),
                    "seconds": recording.get("duration"),
                    "releases": [release["id"] for group in recording.get("releasegroups", [])
                                 for release in group.get("releases", [])][:KEEP_RELEASES],
                }
                for recording in result.get("recordings", []) if recording.get("id")
            ],
        }
        for result in data.get("results", [])
    ]


def load(db):
    """What was heard, by file: {"recordings": {recording id: score}, "releases": [release ids]}."""
    heard = {}
    for row in db.execute("SELECT file, results FROM heard WHERE results IS NOT NULL"):
        recordings, releases = {}, []
        for result in json.loads(zlib.decompress(row["results"])):
            for recording in result["recordings"]:
                recordings[recording["id"]] = max(recordings.get(recording["id"], 0), result["score"])
                if result["score"] >= 0.8:
                    releases += recording["releases"]
        if recordings:
            heard[row["file"]] = {"recordings": recordings, "releases": list(dict.fromkeys(releases))}
    return heard


def run(db, key, limit=None, log=print):
    todo = db.execute(
        "SELECT e.file, e.fingerprint, coalesce(e.fp_duration, e.duration) AS seconds, e.gathered FROM evidence e "
        "JOIN files f ON f.id = e.file LEFT JOIN heard h ON h.file = e.file "
        "WHERE e.fingerprint IS NOT NULL AND coalesce(e.fp_duration, e.duration) > 0 "
        "AND (h.file IS NULL OR h.fingerprint_of != e.gathered OR h.error IS NOT NULL) ORDER BY f.name"
    ).fetchall()
    todo = todo[:limit] if limit else todo
    log(f"{len(todo):,} fingerprints to look up")
    known = failed = 0
    began = last = time.monotonic()
    for n, row in enumerate(todo, 1):
        time.sleep(max(0.0, last + PAUSE - time.monotonic()))
        last = time.monotonic()
        results = error = None
        try:
            results = lookup(key, row["fingerprint"], row["seconds"])
        except Refused as e:
            db.commit()
            log(f"AcoustID refused the key: {e}. Stopping.")
            return
        except (OSError, ValueError, TimeoutError) as e:
            error, failed = str(e)[:200], failed + 1
            if failed >= 20 and failed > n / 2:
                db.commit()
                log("AcoustID keeps failing; stopping here. The next run carries on.")
                return
        known += bool(results and any(result["recordings"] for result in results))
        db.execute("INSERT OR REPLACE INTO heard (file, looked, fingerprint_of, error, results) VALUES (?, ?, ?, ?, ?)",
                   (row["file"], int(time.time()), row["gathered"], error, zlib.compress(json.dumps(results).encode()) if results is not None else None))
        if n % 50 == 0:
            db.commit()
        if n % 1000 == 0:
            rate = n / (time.monotonic() - began)
            log(f"{n:,} of {len(todo):,} looked up, {known:,} known to AcoustID, about {(len(todo) - n) / rate / 60:.0f} min left")
    db.commit()
    log(f"looked up {len(todo):,} fingerprints: {known:,} known to AcoustID, {failed:,} failed")


def main(argv=None):
    parser = argparse.ArgumentParser(prog="librarian hear", description=__doc__.split("\n\n")[0])
    parser.add_argument("--limit", type=int, help="look up at most this many")
    args = parser.parse_args(argv)
    key = config.secrets().get("ACOUSTID_KEY")
    if not key:
        sys.exit(f"no ACOUSTID_KEY in {config.path('secrets.env')}")
    run(catalogue.connect(config.path("catalogue.db")), key, args.limit, log=lambda text: print(text, flush=True))


if __name__ == "__main__":
    main()
