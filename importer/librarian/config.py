"""Where the librarian keeps its things, and its settings.

Everything lives under one folder (~/wax, or $WAX_HOME). Settings come from
`librarian.toml` there and override the defaults below. Keys for outside
services come from `secrets.env` there, which only its owner may read.
"""

import functools
import os
import stat
import subprocess
import tomllib

HOME = os.path.expanduser(os.environ.get("WAX_HOME", "~/wax"))

DEFAULTS = {
    # The folder the music is in; paths in the catalogue are relative to it. Nothing under it is ever changed.
    "root": "~/Music",
    # Only these folders under the root are read; all of it when the list is empty.
    "include": [],
    # A server the music folders are on, as "user@host": each folder of `include` is then mounted from it over SSH
    # (sshfs), read-only, under `root`, so that every file is seen under the name it really has. Empty to read `root`
    # as it is.
    "remote": "",
    "remote_port": 22,
    "fpcalc": os.path.join(HOME, "bin", "fpcalc"),
    "jobs": 2,
    # Models asked in turn when the reference alone cannot settle something; a later one is the harder-case escalation.
    # None by default. "openrouter:<model>" is paid per use from OPENROUTER_API_KEY in secrets.env; "claude:<model>" asks
    # the Claude Code installed here, on its owner's subscription.
    "judges": [],
    "listener": "",  # a model that accepts audio, as "openrouter:<model>": it hears a clip of what is still unknown
    # Whether Deezer may be asked, by artist and title, for the cover of a record that has none anywhere else.
    "deezer": False,
    # Who runs this librarian, for MusicBrainz, which asks every program to say how to reach whoever runs it: an email
    # address or a web page. The project's page when empty.
    "contact": "",
    # What the nightly run does, in order. "hear" (AcoustID fingerprints) is left out unless its key is wanted.
    "stages": ["gather", "identify", "judge", "listen", "genre", "artwork", "ready", "report"],
    # The most one stage of a run may spend on paid models; "<stage>_budget_usd" (as in listen_budget_usd) sets it for that stage alone.
    "budget_usd": 1.0,
    "claude_questions": 150,  # the most one run may ask of Claude Code, to leave the subscription's allowance for its owner
    "clip_seconds": 45,
}


def path(name):
    return os.path.join(HOME, name)


def library():
    """The folder the music is read from. With a remote, each music folder is mounted from it first if it is not already."""
    settings = load()
    root = os.path.expanduser(settings["root"])
    remote = settings["remote"]
    if remote and not settings["include"]:
        raise SystemExit("librarian.toml names a remote but no folders to mount from it: list them in include")
    for folder in settings["include"] if remote else []:
        here = os.path.join(root, folder)
        if not os.path.ismount(here):
            os.makedirs(here, exist_ok=True)
            mount = subprocess.run(["sshfs", "-p", str(settings["remote_port"]), "-o", "ro,reconnect,ServerAliveInterval=15,BatchMode=yes",
                                    f"{remote}:{folder}", here], capture_output=True, timeout=60)
            if mount.returncode != 0:
                raise SystemExit(f"cannot mount {folder} from {remote} over SSH: {mount.stderr.decode('utf-8', 'replace').strip()[:300]}")
    return root


def music_folders():
    """The folders under the library the music is in, as paths relative to it: the root itself when none are listed."""
    return [folder.strip("/") for folder in load()["include"]] or [""]


@functools.cache
def _longest_first():
    return sorted((folder for folder in music_folders() if folder), key=len, reverse=True)


def inner(path):
    """A path of the library without the music folder it is in: "Artist/Album" of "music/Artist/Album"."""
    for folder in _longest_first():
        if path == folder or path.startswith(folder + "/"):
            return path[len(folder) + 1:]
    return path


def load():
    settings = dict(DEFAULTS)
    try:
        with open(path("librarian.toml"), "rb") as f:
            settings.update(tomllib.load(f))
    except FileNotFoundError:
        pass
    return settings


def secrets():
    """Keys from secrets.env as {NAME: value}; refuses a file other accounts can read."""
    file = path("secrets.env")
    try:
        mode = os.stat(file).st_mode
    except FileNotFoundError:
        return {}
    if mode & (stat.S_IRWXG | stat.S_IRWXO):
        raise SystemExit(f"{file} is readable by other accounts; run: chmod 600 {file}")
    found = {}
    with open(file, encoding="utf-8") as f:
        for line in f:
            name, eq, value = line.strip().partition("=")
            if eq and name and not name.startswith("#") and value.strip():
                found[name.strip()] = value.strip().strip("\"'")
    return found
