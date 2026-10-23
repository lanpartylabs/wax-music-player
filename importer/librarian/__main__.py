"""Run the librarian, or one of its stages.

    python3 -m librarian nightly     every stage listed in librarian.toml, in order
    python3 -m librarian gather      examine new and changed files
    python3 -m librarian hear        look fingerprints up at AcoustID
    python3 -m librarian identify    match files to releases in MusicBrainz
    python3 -m librarian judge       have a model look at what is unsettled
    python3 -m librarian listen      play a clip of what is still unknown to a model
    python3 -m librarian genre       give every album held with certainty its genre
    python3 -m librarian artwork     fetch the front cover of every release held with certainty
    python3 -m librarian ready       write the catalogue of finished releases for the player
    python3 -m librarian report      write up where the collection stands
    python3 -m librarian file PATH   write one file of the library to standard output, for the player
    python3 -m librarian discard     note the files, named on standard input, that were dropped in the player

Each stage takes --help.
"""

import fcntl
import os
import shutil
import sys
import time
import traceback

from . import acoustid, artwork, catalogue, config, gather, genre, identify, judge, listen, ready, report


def gather_stage(argv):
    settings = config.load()
    command = ["--root", config.library(), "--db", config.path("catalogue.db"), "--fpcalc", settings["fpcalc"], "--jobs", str(settings["jobs"])]
    for folder in config.music_folders():
        command += ["--include", folder]
    return gather.main(command + list(argv))


STAGES = {"gather": gather_stage, "hear": acoustid.main, "identify": identify.main, "judge": judge.main, "listen": listen.main,
          "genre": genre.main, "artwork": artwork.main, "ready": ready.main, "report": report.main}


def file(argv):
    """Hand the player one file. Only a file inside the music folders is given, whatever the path asks for."""
    root = os.path.realpath(config.library())
    path = os.path.realpath(os.path.join(root, argv[0])) if len(argv) == 1 else ""
    inside = any(path.startswith(os.path.join(root, folder, "")) for folder in config.music_folders())
    if not inside or not os.path.isfile(path):
        sys.exit("there is no such file in the library")
    try:
        with open(path, "rb") as f:
            shutil.copyfileobj(f, sys.stdout.buffer, 1 << 20)
        sys.stdout.buffer.flush()
    except BrokenPipeError:  # the player stopped listening: it asked for something else
        os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())


def discard(argv):
    """Note the files dropped from the library in the player: one path of the index a line, on standard input.

    Nothing is deleted. They are left out of the catalogue the next time it is written, and listed in the report.
    """
    db = catalogue.connect(config.path("catalogue.db"))
    names = [line.rstrip("\n") for line in sys.stdin if line.strip()]
    noted = 0
    for name in names:
        if db.execute("SELECT 1 FROM files WHERE name = ?", (name,)).fetchone():  # only what is in the collection
            db.execute("INSERT OR IGNORE INTO discarded (name, at) VALUES (?, ?)", (name, int(time.time())))
            noted += 1
    db.commit()
    print(f"{noted} of {len(names)} noted; {db.execute('SELECT count(*) FROM discarded').fetchone()[0]} dropped in all")


def nightly(argv):
    lock = open(config.path("run.lock"), "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit("the librarian is already at work; not starting a second run")
    for stage in config.load()["stages"]:
        print(f"\n== {time.strftime('%Y-%m-%d %H:%M:%S')} {stage}", flush=True)
        try:
            STAGES[stage]([])
        except SystemExit as e:  # a stage that cannot run, such as one with no key, says why and the rest carry on
            if e.code:
                print(f"{stage} did not run: {e.code}", flush=True)
        except Exception:
            traceback.print_exc()
            print(f"{stage} failed; carrying on with the next stage", flush=True)
    print(f"\n== {time.strftime('%Y-%m-%d %H:%M:%S')} done", flush=True)


def main():
    stage, argv = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ("", [])
    if stage == "nightly":
        return nightly(argv)
    if stage == "file":
        return file(argv)
    if stage == "discard":
        return discard(argv)
    if stage not in STAGES:
        sys.exit(__doc__)
    return STAGES[stage](argv)


if __name__ == "__main__":
    sys.exit(main())
