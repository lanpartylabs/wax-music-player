"""The listener: a model that can hear is played a clip of what is still unknown.

    python3 -m librarian listen [--only FOLDER] [--limit N]

For each file neither the reference nor the judge could name, a short clip
is cut from it and given to a model that accepts sound. The model writes
down the words it hears and says what it recognises. A song it names goes
to MusicBrainz to be confirmed by name and length; when it hears words but
cannot place them, the words go to the judge's models, which often can.
What is confirmed is recorded; the rest keeps what was heard beside it.
"""

import argparse
import json
import os
import subprocess
import sys

from . import catalogue, config, identify, judge, llm, reference

SHORTEST = 20  # seconds; less is not worth a listen
WORDS = 6  # heard words from which a text model is asked which song they are from
GIVE_UP = 8  # questions in a row without an answer after which the models are taken to be out of order
HEARING_TOKENS = 2000  # ten times what a hearing usually runs to; past that the model is repeating a word it heard, at a price

HEARING = {
    "type": "object",
    "properties": {
        "what": {"type": "string", "enum": ["song", "instrumental", "dj mix", "speech", "sound effect", "silence or noise"]},
        "language": {"type": "string"},
        "lyrics": {"type": "string"},
        "style": {"type": "string"},
        "artist": {"type": "string"},
        "title": {"type": "string"},
    },
    "required": ["what", "language", "lyrics", "style", "artist", "title"],
    "additionalProperties": False,
}
PLACING = {
    "type": "object",
    "properties": {"known": {"type": "boolean"}, "artist": {"type": "string"}, "title": {"type": "string"}, "album": {"type": "string"}},
    "required": ["known", "artist", "title", "album"],
    "additionalProperties": False,
}


def clip(path, seconds, length):
    """A mono MP3 clip from a file, begun a third of the way in so it is past the introduction."""
    start = max(0.0, min(seconds * 0.3, seconds - length)) if seconds else 0.0
    out = subprocess.run(
        ["ffmpeg", "-v", "error", "-ss", f"{start:.1f}", "-t", str(length), "-i", path, "-vn", "-ac", "1", "-ar", "16000", "-b:a", "32k", "-f", "mp3", "-"],
        capture_output=True, timeout=180)
    if out.returncode != 0 or len(out.stdout) < 4000:
        raise OSError(out.stderr.decode("utf-8", "replace").strip()[:200] or "no sound decoded")
    return out.stdout


def hints(f):
    said = ", ".join(f"{label} \"{value}\"" for label, value in (("title", f["title"]), ("artist", f["artist"]), ("album", f["album"])) if value)
    return (f"The file is named \"{f['path'].rsplit('/', 1)[-1]}\", in the folder \"{f['dir']}\", and is {judge.clock(f['seconds'])} long. "
            + (f"Its tags say: {said}." if said else "It has no tags."))


def hearing_prompt(f, length):
    return (
        f"This is a {length}-second clip from a file in a personal music collection. {hints(f)} The names and tags may be wrong.\n\n"
        "Listen, then answer with:\n"
        "- what: \"song\" (it has singing or rapping), \"instrumental\", \"dj mix\", \"speech\", \"sound effect\" or \"silence or noise\".\n"
        "- language: the language of the words, or empty.\n"
        "- lyrics: the words you hear, written down exactly as sung or spoken, a hundred words at most; empty if there are none.\n"
        "- style: genre and instruments, in a few words.\n"
        "- artist and title: only if you recognise the recording, or the words leave no doubt which song it is. Otherwise leave both empty."
    )


def placing_prompt(f, heard):
    return (
        f"These words were heard in a clip of a file in a personal music collection. {hints(f)} The names and tags may be wrong.\n\n"
        f"Style: {heard['style'] or 'unknown'}. Language: {heard['language'] or 'unknown'}.\nWords heard:\n{heard['lyrics'][:1500]}\n\n"
        "Which song is it? Answer with known true and the artist, the official title and the album it first came out on; or known false, "
        "and the rest empty, if the words do not point to one song."
    )


def listen(db, mb, models, listener, tiers, root, f, length):
    """Play one file to the listener. Returns "confirmed", "heard" or "failed"."""
    path = db.execute("SELECT path FROM files WHERE id = ?", (f["id"],)).fetchone()["path"]
    try:
        sound = clip(os.path.join(root, path), f["seconds"], length)
        heard = models.ask(listener, judge.SYSTEM, hearing_prompt(f, length), HEARING, audio=sound, most=HEARING_TOKENS)
    except llm.Unusable:  # it listened and made nothing of it, and would again: the file is not played a second time
        db.execute("UPDATE identity SET judged = judged | 2 WHERE file = ?", (f["id"],))
        return "failed"
    except (OSError, subprocess.TimeoutExpired, llm.Failed):
        return "failed"

    proposal, found = None, None
    if heard["title"].strip() and heard["artist"].strip():
        proposal = {"artist": heard["artist"], "title": heard["title"], "by": listener}
    elif len(heard["lyrics"].split()) >= WORDS:
        for tier in tiers:  # heard, but not placed: someone who knows more lyrics may place the words
            if not models.usable(tier):
                continue
            try:
                placed = models.ask(tier, judge.SYSTEM, placing_prompt(f, heard), PLACING)
            except (llm.Failed, llm.Unavailable):
                continue
            if placed["known"] and placed["title"].strip() and placed["artist"].strip():
                proposal = {"artist": placed["artist"], "title": placed["title"], "album": placed["album"], "by": tier}
                break
    if proposal:
        judge.propose(f, proposal["title"], proposal["artist"])
        if proposal.get("album") and not f["album"]:
            f["album"] = proposal["album"]
        found = identify.identify_song(mb, f)
    if found:
        found["how"] = "listen"
        found["evidence"] = dict(found.get("evidence") or {}, heard={"what": heard["what"], "lyrics": heard["lyrics"][:300]})
        identify.save_identity(db, f["id"], f["unit"], found, judged=3)
        return "confirmed"
    before = db.execute("SELECT evidence FROM identity WHERE file = ?", (f["id"],)).fetchone()
    evidence = json.loads(before["evidence"]) if before and before["evidence"] else {}
    evidence["heard"] = {key: heard[key][:600] for key in ("what", "language", "lyrics", "style")}
    if proposal:
        evidence["proposal"] = proposal
    identify.save_identity(db, f["id"], f["unit"], {"state": "unknown", "evidence": evidence}, judged=3)
    return "heard"


def run(db, mb, models, listener, tiers, root, length, only=None, limit=None, log=print):
    if not models.usable(listener):
        log(f"{listener} cannot be asked: {models.why_not(listener)}; nothing listened to")
        return
    files = {f["id"]: f for f in identify.load_files(db)}
    todo = []
    for row in db.execute(
        "SELECT i.file, i.unit FROM identity i JOIN files f ON f.id = i.file JOIN evidence e ON e.file = i.file "
        "WHERE i.state = 'unknown' AND i.judged & 2 = 0 AND e.opens = 1 AND e.reader IS NOT NULL ORDER BY f.name"
    ):
        f = files.get(row["file"])
        if f and f["seconds"] >= SHORTEST and (not only or f["dir"].startswith(only)):
            f["unit"] = row["unit"]
            todo.append(f)
    todo = todo[:limit] if limit else todo
    log(f"{len(todo):,} unknown files to listen to with {listener}")
    outcomes = {"confirmed": 0, "heard": 0, "failed": 0}
    try:
        for n, f in enumerate(todo, 1):
            try:
                outcomes[listen(db, mb, models, listener, tiers, root, f, length)] += 1
            except llm.Unavailable as e:
                log(f"stopping: {e}")
                break
            db.commit()
            if models.unanswered >= GIVE_UP:
                log(f"stopping: the last {models.unanswered} questions got no answer")
                break
            if n % 25 == 0:
                log(f"{n:,} of {len(todo):,}: {outcomes['confirmed']} confirmed, {outcomes['heard']} heard but not placed, "
                    f"{outcomes['failed']} failed | ${models.spent:.2f}")
    except reference.Unavailable:
        log("MusicBrainz is not answering; stopping here. The next run carries on from this point.")
    db.commit()
    log(f"listened to {sum(outcomes.values()):,}: {outcomes['confirmed']} confirmed by the reference, {outcomes['heard']} heard but not placed, "
        f"{outcomes['failed']} failed; {models.calls} questions asked, ${models.spent:.2f} spent")


def main(argv=None):
    parser = argparse.ArgumentParser(prog="librarian listen", description=__doc__.split("\n\n")[0])
    parser.add_argument("--only", help="listen only to what is under this folder")
    parser.add_argument("--limit", type=int, help="listen to at most this many files")
    args = parser.parse_args(argv)
    settings = config.load()
    if not settings["listener"]:
        sys.exit("no listener in librarian.toml: nothing listened to")
    db = catalogue.connect(config.path("catalogue.db"))
    run(db, reference.MusicBrainz(config.path("reference.db")), llm.Models(db, "listen"), settings["listener"], settings["judges"],
        os.fsencode(config.library()), settings["clip_seconds"], args.only, args.limit, log=lambda text: print(text, flush=True))


if __name__ == "__main__":
    sys.exit(main())
