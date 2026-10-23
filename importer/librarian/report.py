"""Write up the state of the collection.

    python3 -m librarian report

Prints a summary and writes it, with the lists behind it, to the report
folder: the records and how complete each is, the tracks wanted to complete
them, the cover of each release, what is still unknown, and the files that
are damaged or out of reach.
"""

import argparse
import collections
import csv
import json
import os
import sys
import time

from . import catalogue, config, ready, reference
from .artwork import SMALL, SOURCES, SQUARE
from .match import PART

RELEASE = "https://musicbrainz.org/release/"
DAMAGED = "(e.reader IS NULL OR (e.duration IS NULL AND e.fingerprint IS NULL))"  # nothing in the file plays
# Each release certain files sit on, with its cover if it has been looked for.
COVERS = ("SELECT r.*, c.looked, c.source, c.origin, c.file AS cover, c.width, c.height FROM (SELECT i.release, max(i.album_artist) AS artist, "
          "max(i.album) AS album, max(i.year) AS year, max(i.kind) AS kind, count(*) AS files, min(f.name) AS first FROM identity i "
          "JOIN files f ON f.id = i.file WHERE i.state = 'certain' AND i.release IS NOT NULL GROUP BY i.release) r "
          "LEFT JOIN covers c ON c.release = r.release")


def table(db, query, args=()):
    return db.execute(query, args).fetchall()


def counts(db, query):
    return ", ".join(f"{row[0] or 'none'} {row[1]:,}" for row in table(db, query)) or "none"


def write(folder, name, header, rows):
    with open(os.path.join(folder, name), "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f, delimiter="\t")
        writer.writerow(header)
        writer.writerows(rows)


def cover_state(row):
    if not row["cover"]:
        return "none" if row["looked"] else "not looked for"
    square, side = max(row["width"], row["height"]) <= SQUARE * min(row["width"], row["height"]), max(row["width"], row["height"])
    return "small" if side < SMALL else "held" if square else "not square"


def summary(db):
    latest = "f.seen = (SELECT max(seen) FROM files)"
    files = db.execute(
        f"SELECT count(*) AS audio, sum(e.opens = 0) AS unopenable, sum(e.opens = 1 AND {DAMAGED}) AS damaged, "
        f"sum(e.file IS NULL) AS waiting FROM files f LEFT JOIN evidence e ON e.file = f.id WHERE f.kind = 'audio' AND {latest}"
    ).fetchone()
    identified = db.execute("SELECT count(*) AS n, sum(state = 'certain') AS certain, sum(state = 'likely') AS likely, "
                            "sum(state = 'unknown') AS unknown, sum(copy) AS copies FROM identity").fetchone()
    lines = [f"The collection on {time.strftime('%Y-%m-%d %H:%M')}", ""]
    lines.append(f"Files: {files['audio']:,} audio files. {files['unopenable'] or 0:,} cannot be opened, "
                 f"{files['damaged'] or 0:,} are not readable as audio, {files['waiting'] or 0:,} not yet examined.")
    if identified["n"]:
        share = lambda part: f"{part or 0:,} ({100 * (part or 0) / identified['n']:.0f}%)"
        lines.append(f"Identified: {identified['n']:,} files looked at. Certain {share(identified['certain'])}, likely {share(identified['likely'])}, "
                     f"unknown {share(identified['unknown'])}.")
        lines.append("  settled by: " + counts(db, "SELECT how, count(*) FROM identity WHERE state != 'unknown' GROUP BY how ORDER BY 2 DESC"))
        lines.append(f"  second copies inside their own folder: {identified['copies'] or 0:,}")
        surplus = db.execute("SELECT coalesce(sum(n - 1), 0) FROM (SELECT count(*) AS n FROM identity WHERE state = 'certain' "
                             "AND recording IS NOT NULL GROUP BY recording HAVING n > 1)").fetchone()[0]
        lines.append(f"  the same recording held more than once across the collection: {surplus:,} surplus files")
        unplayable = db.execute(f"SELECT count(*) FROM identity i JOIN evidence e ON e.file = i.file WHERE i.state != 'unknown' "
                                f"AND (e.opens = 0 OR {DAMAGED})").fetchone()[0]
        lines.append(f"  identified but cannot be played, so to be replaced or reached another way: {unplayable:,}")
        proposals = db.execute("SELECT count(*) FROM identity WHERE state = 'unknown' AND evidence LIKE '%\"proposal\"%'").fetchone()[0]
        lines.append(f"  unknown files with a model's proposal waiting for a person: {proposals:,}")
    records = db.execute("SELECT count(*) FROM units WHERE shape = 'record'").fetchone()[0]
    if records:
        lines += ["", f"Records: {records:,} folders that look like one record."]
        lines.append("  " + counts(db, "SELECT state, count(*) FROM units WHERE shape = 'record' GROUP BY state ORDER BY 2 DESC"))
        lines.append("  of those matched, how complete: "
                     + counts(db, "SELECT shelf, count(*) FROM units WHERE state IN ('certain', 'likely') GROUP BY shelf ORDER BY 2 DESC"))
        lines.append("  of those matched, what kind: "
                     + counts(db, "SELECT kind, count(*) FROM units WHERE state IN ('certain', 'likely') GROUP BY kind ORDER BY 2 DESC"))
        lines.append("  release found through: "
                     + counts(db, "SELECT how, count(*) FROM units WHERE state IN ('certain', 'likely') GROUP BY how ORDER BY 2 DESC"))
        wanted = db.execute("SELECT count(*) AS n, sum(held IS NOT NULL) AS held FROM wanted").fetchone()
        lines.append(f"  tracks missing from records one or two short: {wanted['n']:,}, of which {wanted['held'] or 0:,} are elsewhere in the collection")
    covers = table(db, COVERS)
    if any(row["looked"] for row in covers):
        found = collections.Counter("not looked for yet" if not row["looked"] else SOURCES[row["source"]] for row in covers)
        lines += ["", f"Covers: {len(covers):,} releases that certain files sit on."]
        lines.append("  from: " + ", ".join(f"{name} {count:,}" for name, count in found.most_common()))
        states = collections.Counter(cover_state(row) for row in covers)
        lines.append(f"  smaller than {SMALL} pixels: {states['small']:,}; not square: {states['not square']:,}")
    genres = table(db, "SELECT coalesce(filed, CASE WHEN checked IS NULL THEN 'not yet checked' ELSE 'none' END) AS genre, count(*) AS n "
                       "FROM genres GROUP BY 1 ORDER BY 2 DESC")
    if genres:
        lines += ["", "Genres: " + ", ".join(f"{row['genre']} {row['n']:,}" for row in genres)]
        changed = db.execute("SELECT count(*) FROM genres WHERE filed IS NOT NULL AND genre IS NOT NULL AND filed != genre").fetchone()[0]
        lines.append(f"  filed by a model under another name than the first reading from MusicBrainz gave: {changed:,}")
    # The releases compared are in the reference's cache from when they were matched, so this asks nothing of it.
    lines += ["", ready.summary(ready.assess(db, reference.MusicBrainz(config.path("reference.db"))))]
    dropped = db.execute("SELECT count(*) AS n, coalesce(sum(f.size), 0) AS bytes FROM discarded d LEFT JOIN files f ON f.name = d.name").fetchone()
    if dropped["n"]:
        lines += ["", f"Dropped from the library in the player: {dropped['n']:,} files, {dropped['bytes'] / 1e9:.1f} GB. "
                      "They are still in the music folders; discarded.tsv lists them."]
    asked = table(db, "SELECT model, count(*) AS n, sum(error IS NOT NULL) AS failed, coalesce(sum(usd), 0) AS usd FROM asked GROUP BY model")
    if asked:
        lines += ["", "Models asked: " + "; ".join(f"{row['model']} {row['n']:,} questions ({row['failed'] or 0} failed), ${row['usd']:.2f}" for row in asked)]
    return "\n".join(lines)


def lists(db, folder):
    write(folder, "records.tsv",
          ["state", "how complete", "kind", "folder", "files", "reads as artist", "reads as album", "is artist", "is release", "year",
           "tracks", "missing", "second copies", "found through", "release", "model proposed"],
          [[u["state"], u["shelf"], u["kind"], u["key"].replace(PART, " # "), u["files"], u["artist"], u["album"], u["credited"], u["title"],
            u["year"], u["tracks"], u["missing"], u["copies"], u["how"], RELEASE + u["release"] if u["release"] else "",
            (lambda p: f"{p.get('kind')}: {p.get('artist')} / {p.get('album')} ({p.get('by')})")(json.loads(u["proposal"])) if u["proposal"] else ""]
           for u in table(db, "SELECT * FROM units WHERE shape = 'record' ORDER BY state, key")])
    write(folder, "wanted.tsv", ["artist", "release", "disc", "track", "title", "length", "in the collection at", "folder", "release link"],
          [[w["credited"], w["album"], w["disc"], w["position"], w["title"], f"{int(w['seconds'] or 0) // 60}:{int(w['seconds'] or 0) % 60:02d}",
            w["held_at"] or "", w["folder"], RELEASE + w["release"]]
           for w in table(db, "SELECT w.*, u.credited, u.title AS album, u.folder, f.name AS held_at FROM wanted w JOIN units u ON u.key = w.unit "
                              "LEFT JOIN files f ON f.id = w.held ORDER BY u.credited, u.title, w.disc, w.position")])
    unknown = []
    for row in table(db, "SELECT f.name, e.title, e.artist, e.album, e.duration, i.evidence FROM identity i JOIN files f ON f.id = i.file "
                         "JOIN evidence e ON e.file = i.file WHERE i.state = 'unknown' ORDER BY f.name"):
        evidence = json.loads(row["evidence"]) if row["evidence"] else {}
        proposal, heard = evidence.get("proposal") or {}, evidence.get("heard") or {}
        unknown.append([row["name"], row["title"], row["artist"], row["album"], f"{int(row['duration'] or 0) // 60}:{int(row['duration'] or 0) % 60:02d}",
                        proposal.get("what", ""), proposal.get("artist", ""), proposal.get("title", ""), proposal.get("by", ""),
                        heard.get("what", ""), heard.get("style", ""), (heard.get("lyrics") or "")[:200]])
    write(folder, "unknown.tsv", ["file", "title tag", "artist tag", "album tag", "length", "model says it is", "proposed artist", "proposed title",
                                  "proposed by", "heard as", "style", "words heard"], unknown)
    write(folder, "problems.tsv", ["file", "problem", "detail", "is artist", "is release", "track", "is title"],
          [[row["name"], "cannot be opened" if not row["opens"] else "not readable as audio", row["error"], row["album_artist"],
            row["album"], row["position"], row["title"]]
           for row in table(db, f"SELECT f.name, e.opens, e.error, i.album_artist, i.album, i.position, i.title FROM evidence e "
                                f"JOIN files f ON f.id = e.file LEFT JOIN identity i ON i.file = e.file WHERE e.opens = 0 OR {DAMAGED} ORDER BY f.name")])
    write(folder, "covers.tsv", ["cover", "artist", "release", "year", "kind", "files", "width", "height", "from", "taken from", "first file", "release link"],
          [[cover_state(row), row["artist"], row["album"], row["year"], row["kind"], row["files"], row["width"], row["height"],
            SOURCES[row["source"]] if row["cover"] else "", row["origin"] or "", row["first"], RELEASE + row["release"]]
           for row in sorted(table(db, COVERS), key=lambda row: (("none", "small", "not square", "not looked for", "held").index(cover_state(row)),
                                                                  (row["artist"] or "").lower(), (row["album"] or "").lower()))])
    write(folder, "discarded.tsv", ["file", "dropped on", "size in MB"],
          [[row["name"], time.strftime("%Y-%m-%d %H:%M", time.localtime(row["at"])), round((row["size"] or 0) / 1e6, 1)]
           for row in table(db, "SELECT d.name, d.at, f.size FROM discarded d LEFT JOIN files f ON f.name = d.name ORDER BY d.name")])
    write(folder, "likely.tsv", ["file", "is artist", "is title", "is release", "settled by", "evidence"],
          [[row["name"], row["artist"], row["title"], row["album"], row["how"], row["evidence"]]
           for row in table(db, "SELECT f.name, i.* FROM identity i JOIN files f ON f.id = i.file WHERE i.state = 'likely' ORDER BY f.name")])


def main(argv=None):
    argparse.ArgumentParser(prog="librarian report", description=__doc__.split("\n\n")[0]).parse_args(argv)
    db = catalogue.connect(config.path("catalogue.db"))
    folder = config.path("report")
    os.makedirs(folder, exist_ok=True)
    text = summary(db)
    with open(os.path.join(folder, "summary.txt"), "w", encoding="utf-8") as f:
        f.write(text + "\n")
    lists(db, folder)
    print(text + f"\n\nLists written to {folder}", flush=True)


if __name__ == "__main__":
    sys.exit(main())
