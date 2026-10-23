# Contributing to wax

Thank you for wanting to help. Bug reports, ideas and code are all welcome.

## Reporting a bug or asking for something

Open an issue and say what you did, what you expected and what happened.
Please do not paste lists of your own music, file paths from your machine or
logs full of them: describe the case, or show it with the made-up library
(below). The same goes for screenshots.

## Building and testing

```sh
sudo apt install build-essential pkg-config libasound2-dev   # Debian, Ubuntu
cargo build --release
cargo test --workspace
```

The player test that plays a real song through the sound device is left out
of `cargo test`; run it by hand with any song:

```sh
WAX_TEST_AUDIO=/path/to/a/song.mp3 cargo test -p wax plays -- --ignored --nocapture
```

The importer needs Python 3.11 or later, `mutagen` and `ffmpeg`; see
[importer/README.md](importer/README.md).

## Music to test with

[`tools/make_demo_library.py`](tools/make_demo_library.py) makes a small
library of invented albums, with tags and covers, using only Python:

```sh
python3 tools/make_demo_library.py /tmp/wax-demo
```

To try wax on it without touching your own settings, give it folders of its
own:

```sh
export XDG_CONFIG_HOME=/tmp/wax-try/config XDG_DATA_HOME=/tmp/wax-try/data \
       XDG_STATE_HOME=/tmp/wax-try/state XDG_CACHE_HOME=/tmp/wax-try/cache
./target/release/wax
```

## Driving the window from a script

wax can be driven without a person at the keyboard, which is how its
screenshots are made and how a change to the window can be checked:

```sh
WAX_MUTE=1 WAX_KEYS="wait:800 1 Enter wait:300 Enter wait:2000" \
WAX_SCREENSHOT=/tmp/shot.png ./target/release/wax
```

`WAX_KEYS` is a list of steps: key names (`Enter`, `Ctrl+N`, `Shift+ArrowDown`),
`text:` and what to type, `wait:` and milliseconds, and `move:`, `press:`,
`release:` or `rclick:` with a point of the window. `WAX_SCREENSHOT` saves the
window once the steps are done, then quits; `WAX_MUTE` plays at no volume.
See [`crates/app/src/devtools.rs`](crates/app/src/devtools.rs).

## Writing code for wax

- **Keyboard first.** Everything the mouse can do has a key, and a key press
  is answered in the frame it arrives.
- **Fast.** The whole library is kept in memory, and a search or a sort is a
  single pass over it. Keep it that way: nothing that waits on the disk or the
  network runs in the window's frame.
- **The music is only read.** Neither the player nor the importer may write,
  move or delete a music file.
- **Nothing leaves the machine unasked.** The player sends nothing anywhere.
  A new outside service in the importer is off until its settings turn it on,
  and goes in the table in [importer/README.md](importer/README.md#what-is-sent-where).
- **Read like the code around it.** Match its naming, its comments and its
  idiom; comments say why, in plain words.
- Run `cargo test --workspace` before you open a pull request, and add a test
  for what you changed where one can be written.

## License

wax is under the [GNU General Public License, version 3 or later](LICENSE).
By contributing you agree that your contribution is shared under the same
license.
