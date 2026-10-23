//! Playing music. A track of a music folder on this machine is played where
//! it is. A track of a library on a server is fetched into a cache on this
//! machine first, over the SSH connection the machine already has to it, and
//! played from there. Nothing listens on the server for this: the librarian
//! there is asked for one file by its path in the index, and gives only what
//! is inside the music folders.
//!
//! One thread owns the sound device and everything played through it. The
//! window sends it requests and reads back where things stand.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use eframe::egui;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink};

/// How often the thread looks at what is playing or being fetched.
const TICK: Duration = Duration::from_millis(200);
/// The most the cache may hold; the files fetched longest ago go first.
const CACHE_LIMIT: u64 = 4 << 30;
/// Formats the decoder has nothing for.
const UNPLAYABLE: [&str; 5] = ["wma", "ape", "wv", "mpc", "opus"];

/// Where the files of the library are.
#[derive(Clone, PartialEq, Debug)]
pub enum Origin {
    /// In a folder on this machine; the index's paths are relative to it.
    Folder(PathBuf),
    /// On a server, handed over by the librarian there.
    Server {
        /// The server, as `ssh` knows it.
        host: String,
        /// The librarian's folder there.
        home: String,
    },
    /// Not known yet: nothing can be played.
    Nowhere,
}

/// A file to play: its path in the index, and its size where the index gives it.
#[derive(Clone)]
pub struct Request {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    Idle,
    /// Bytes fetched so far, of the file's size.
    Fetching(u64, u64),
    Playing,
    Paused,
    /// The track played to its end.
    Ended,
    Failed(String),
}

#[derive(Clone)]
pub struct Status {
    /// The request this is about, as numbered by [`Player::play`].
    pub serial: u64,
    pub state: State,
    pub position: Duration,
}

enum Command {
    /// Where the files are from now on.
    From(Origin),
    Play(u64, Request),
    /// Fetch a file that is likely to be asked for next.
    Prefetch(Request),
    Pause,
    Resume,
    Seek(Duration),
    /// How loud to play, as the volume slider stands.
    Volume(f32),
}

pub struct Player {
    commands: Sender<Command>,
    status: Arc<Mutex<Status>>,
    serial: u64,
}

impl Player {
    /// `muted` plays at no volume: everything happens but the sound.
    pub fn new(origin: Origin, cache: PathBuf, muted: bool, ctx: egui::Context) -> Self {
        let (commands, inbox) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status { serial: 0, state: State::Idle, position: Duration::ZERO }));
        let shared = status.clone();
        let deck = Deck { origin, cache, muted, volume: 1.0, ctx, status: shared, output: None, sink: None, now: None, ahead: None };
        std::thread::spawn(move || deck.run(inbox));
        Player { commands, status, serial: 0 }
    }

    /// Where the files of the library are from now on. What is playing plays on.
    pub fn set_origin(&self, origin: Origin) {
        self.send(Command::From(origin));
    }

    /// Plays a file in place of whatever was playing. Returns the number its [`Status`] will carry.
    pub fn play(&mut self, request: Request) -> u64 {
        self.serial += 1;
        self.send(Command::Play(self.serial, request));
        self.serial
    }

    pub fn prefetch(&self, request: Request) {
        self.send(Command::Prefetch(request));
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    pub fn seek(&self, to: Duration) {
        self.send(Command::Seek(to));
    }

    /// How loud to play, from 0 (not at all) to 1 (as recorded), as the volume slider stands.
    pub fn set_volume(&self, level: f32) {
        self.send(Command::Volume(level));
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn send(&self, command: Command) {
        // The thread only ends with the window.
        let _ = self.commands.send(command);
    }
}

/// A file on its way from the server into the cache.
struct Fetch {
    path: String,
    target: PathBuf,
    size: u64,
    got: Arc<AtomicU64>,
    /// Set when it is over: Ok when the whole file is in the cache.
    done: Arc<Mutex<Option<Result<(), String>>>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Fetch {
    fn start(host: &str, home: &str, cache: &Path, request: &Request) -> Fetch {
        let fetch = Fetch {
            path: request.path.clone(),
            target: cached(cache, &request.path),
            size: request.size,
            got: Arc::new(AtomicU64::new(0)),
            done: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
        };
        let (got, done, child, target, size) = (fetch.got.clone(), fetch.done.clone(), fetch.child.clone(), fetch.target.clone(), fetch.size);
        // The command goes through a shell on the server: quoted so that nothing in a file's name is read as anything else.
        let quoted = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
        let command = format!("cd {} && exec python3 -m librarian file {}", quoted(home), quoted(&request.path));
        // The socket that keeps one connection open between fetches; its path has to be short, so not in the cache.
        let socket = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from).join("wax-ssh-%C");
        let host = host.to_owned();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let part = target.with_extension("part");
                let mut ssh = Process::new("ssh")
                    .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ControlMaster=auto", "-o", "ControlPersist=600", "-o"])
                    .arg(format!("ControlPath={}", socket.display()))
                    .args([host.as_str(), command.as_str()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("ssh could not be started: {e}"))?;
                let (mut out, mut err) = (ssh.stdout.take().unwrap(), ssh.stderr.take().unwrap());
                *child.lock().unwrap() = Some(ssh);

                let mut file = File::create(&part).map_err(|e| format!("cannot write to the cache: {e}"))?;
                let mut buffer = vec![0u8; 1 << 16];
                loop {
                    let n = out.read(&mut buffer).map_err(|e| format!("the connection broke: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    file.write_all(&buffer[..n]).map_err(|e| format!("cannot write to the cache: {e}"))?;
                    got.fetch_add(n as u64, Ordering::Relaxed);
                }
                let mut said = String::new();
                let _ = err.read_to_string(&mut said);
                // None: it was stopped because something else was asked for.
                let Some(mut ssh) = child.lock().unwrap().take() else { return Err("stopped".to_owned()) };
                let ended = ssh.wait().map_err(|e| e.to_string())?;
                let fetched = got.load(Ordering::Relaxed);
                if !ended.success() || fetched == 0 || (size > 0 && fetched != size) {
                    let _ = std::fs::remove_file(&part);
                    let why = said.lines().last().unwrap_or("the file did not arrive whole").trim().to_owned();
                    return Err(format!("could not fetch it: {why}"));
                }
                std::fs::rename(&part, &target).map_err(|e| format!("cannot write to the cache: {e}"))
            })();
            *done.lock().unwrap() = Some(result);
        });
        fetch
    }

    /// Ends the fetch; what it had is thrown away.
    fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(self.target.with_extension("part"));
    }
}

/// What is being played, or fetched to be played.
struct Now {
    serial: u64,
    request: Request,
    /// The file played: where it is in a music folder, or in the cache.
    file: PathBuf,
    /// While the file is not in the cache yet.
    fetch: Option<Fetch>,
    paused: bool,
}

/// The thread's side: the sound device and what is on it.
struct Deck {
    origin: Origin,
    cache: PathBuf,
    muted: bool,
    /// Where the volume slider stands, from 0 to 1.
    volume: f32,
    ctx: egui::Context,
    status: Arc<Mutex<Status>>,
    /// Opened at the first track, so that a window that plays nothing holds no device.
    output: Option<MixerDeviceSink>,
    sink: Option<rodio::Player>,
    now: Option<Now>,
    /// A file being fetched before it is asked for.
    ahead: Option<Fetch>,
}

impl Deck {
    fn run(mut self, inbox: Receiver<Command>) {
        let _ = std::fs::create_dir_all(&self.cache);
        loop {
            match inbox.recv_timeout(TICK) {
                Ok(command) => self.obey(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.look();
        }
        for fetch in self.now.take().and_then(|now| now.fetch).into_iter().chain(self.ahead.take()) {
            fetch.stop();
        }
    }

    fn obey(&mut self, command: Command) {
        match command {
            Command::From(origin) => {
                if let Some(fetch) = self.ahead.take() {
                    fetch.stop();
                }
                self.origin = origin;
            }
            Command::Play(serial, request) => {
                self.sink = None;
                if let Some(fetch) = self.now.take().and_then(|now| now.fetch) {
                    fetch.stop();
                }
                let ext = request.path.rsplit_once('.').map_or(String::new(), |(_, ext)| ext.to_ascii_lowercase());
                if UNPLAYABLE.contains(&ext.as_str()) {
                    return self.say(serial, State::Failed(format!("this player has no decoder for .{ext} files")), Duration::ZERO);
                }
                let (file, fetch) = match &self.origin {
                    Origin::Folder(root) => (root.join(&request.path), None),
                    Origin::Server { host, home } => {
                        let target = cached(&self.cache, &request.path);
                        let fetch = if whole(&target, request.size) {
                            None
                        } else if self.ahead.as_ref().is_some_and(|fetch| fetch.path == request.path) {
                            self.ahead.take()
                        } else {
                            Some(Fetch::start(host, home, &self.cache, &request))
                        };
                        (target, fetch)
                    }
                    Origin::Nowhere => {
                        let why = "wax does not know where the music is: set your music folder in Settings";
                        return self.say(serial, State::Failed(why.to_owned()), Duration::ZERO);
                    }
                };
                self.say(serial, State::Fetching(0, request.size), Duration::ZERO);
                self.now = Some(Now { serial, request, file, fetch, paused: false });
            }
            // Only a file on a server is worth fetching ahead.
            Command::Prefetch(request) => {
                let Origin::Server { host, home } = &self.origin else { return };
                let wanted = !whole(&cached(&self.cache, &request.path), request.size)
                    && self.ahead.as_ref().is_none_or(|fetch| fetch.path != request.path)
                    && self.now.as_ref().is_none_or(|now| now.request.path != request.path);
                if wanted {
                    if let Some(fetch) = self.ahead.take() {
                        fetch.stop();
                    }
                    self.ahead = Some(Fetch::start(host, home, &self.cache, &request));
                }
            }
            Command::Pause | Command::Resume => {
                let pause = matches!(command, Command::Pause);
                if let Some(now) = &mut self.now {
                    now.paused = pause;
                }
                if let Some(sink) = &self.sink {
                    if pause { sink.pause() } else { sink.play() }
                }
            }
            Command::Seek(to) => {
                if let Some(sink) = &self.sink {
                    let _ = sink.try_seek(to);
                }
            }
            Command::Volume(level) => {
                self.volume = level.clamp(0.0, 1.0);
                if let Some(sink) = &self.sink {
                    sink.set_volume(self.gain());
                }
            }
        }
    }

    /// Moves things on: a finished fetch starts playing, a finished track is reported.
    fn look(&mut self) {
        if self.ahead.as_ref().is_some_and(|fetch| fetch.done.lock().unwrap().is_some()) {
            self.ahead = None;
            tidy(&self.cache);
        }
        let Some(now) = &mut self.now else { return };
        let serial = now.serial;
        if let Some(fetch) = &now.fetch {
            let done = fetch.done.lock().unwrap().take();
            match done {
                None => {
                    let state = State::Fetching(fetch.got.load(Ordering::Relaxed), fetch.size);
                    return self.say(serial, state, Duration::ZERO);
                }
                Some(Err(why)) => {
                    self.now = None;
                    return self.say(serial, State::Failed(why), Duration::ZERO);
                }
                Some(Ok(())) => {
                    now.fetch = None;
                    tidy(&self.cache);
                }
            }
        }
        if self.sink.is_none() {
            let (file, paused) = (now.file.clone(), now.paused);
            match self.start(&file, paused) {
                Ok(()) => {}
                Err(why) => {
                    self.now = None;
                    return self.say(serial, State::Failed(why), Duration::ZERO);
                }
            }
        }
        let Some(sink) = &self.sink else { return };
        let (position, over, paused) = (sink.get_pos(), sink.empty(), sink.is_paused());
        if over {
            self.sink = None;
            self.now = None;
            self.say(serial, State::Ended, position);
        } else {
            self.say(serial, if paused { State::Paused } else { State::Playing }, position);
        }
    }

    fn start(&mut self, file: &Path, paused: bool) -> Result<(), String> {
        if self.output.is_none() {
            let mut output = DeviceSinkBuilder::open_default_sink().map_err(|e| format!("no sound device could be opened: {e}"))?;
            output.log_on_drop(false);
            self.output = Some(output);
        }
        let opened = File::open(file).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "the file is not there any more; scan the music folder again".to_owned(),
            _ => format!("the file cannot be opened: {e}"),
        })?;
        let source = Decoder::try_from(opened).map_err(|e| format!("the file cannot be played: {e}"))?;
        let sink = rodio::Player::connect_new(self.output.as_ref().unwrap().mixer());
        sink.set_volume(self.gain());
        if paused {
            sink.pause();
        }
        sink.append(source);
        self.sink = Some(sink);
        Ok(())
    }

    /// What the sound is multiplied by.
    fn gain(&self) -> f32 {
        if self.muted { 0.0 } else { gain(self.volume) }
    }

    /// Publishes where things stand, and has the window redrawn if that is news.
    fn say(&self, serial: u64, state: State, position: Duration) {
        let mut status = self.status.lock().unwrap();
        let news = status.serial != serial || status.state != state || status.position.as_secs() != position.as_secs();
        *status = Status { serial, state, position };
        if news {
            self.ctx.request_repaint();
        }
    }
}

/// What the sound is multiplied by with the volume slider at `level`. How loud a sound is heard goes roughly with
/// its pressure to the power 0.6 (Stevens's law), so the gain is the level to the power 1/0.6: the slider halfway
/// sounds half as loud, about 10 dB down.
fn gain(level: f32) -> f32 {
    level.clamp(0.0, 1.0).powf(1.0 / 0.6)
}

/// Where a file of the library is kept in the cache: named by a hash of its path, with its own extension.
fn cached(cache: &Path, path: &str) -> PathBuf {
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, b| (hash ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    let ext = path.rsplit_once('.').map_or("", |(_, ext)| ext).to_ascii_lowercase();
    cache.join(format!("{hash:016x}.{ext}"))
}

/// Whether the cache holds the whole of a file.
fn whole(file: &Path, size: u64) -> bool {
    std::fs::metadata(file).is_ok_and(|meta| meta.len() > 0 && (size == 0 || meta.len() == size))
}

/// Keeps the cache within its limit by dropping the files fetched longest ago.
fn tidy(cache: &Path) {
    let Ok(entries) = std::fs::read_dir(cache) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            (meta.is_file() && entry.path().extension().is_some_and(|ext| ext != "part")).then(|| (meta.modified().ok(), meta.len(), entry.path()))
        })
        .filter_map(|(modified, len, path)| Some((modified?, len, path)))
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= CACHE_LIMIT {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays a real file through the sound device at no volume and checks every control.
    /// Run by hand: `WAX_TEST_AUDIO=/path/to/song.m4a cargo test -p wax plays -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn plays_pauses_seeks_and_ends() {
        let song = PathBuf::from(std::env::var_os("WAX_TEST_AUDIO").expect("WAX_TEST_AUDIO names a file to play"));
        let cache = std::env::temp_dir().join(format!("wax-player-test-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        let path = format!("test/song.{}", song.extension().unwrap().to_str().unwrap());
        std::fs::copy(&song, cached(&cache, &path)).unwrap();

        let server = Origin::Server { host: "nowhere.invalid".to_owned(), home: "wax".to_owned() };
        let mut player = Player::new(server, cache.clone(), true, egui::Context::default());
        let until = |player: &Player, what: &str, ok: &dyn Fn(&Status) -> bool| {
            let began = std::time::Instant::now();
            loop {
                let status = player.status();
                if ok(&status) {
                    println!("{what}: {:?} at {:.1}s, after {:.1}s", status.state, status.position.as_secs_f32(), began.elapsed().as_secs_f32());
                    return status;
                }
                assert!(began.elapsed() < Duration::from_secs(8), "{what} did not happen; stuck at {:?} {:?}", status.state, status.position);
                std::thread::sleep(Duration::from_millis(50));
            }
        };

        let serial = player.play(Request { path: path.clone(), size: 0 });
        until(&player, "plays", &|s| s.serial == serial && s.state == State::Playing && s.position > Duration::from_millis(600));
        player.seek(Duration::from_secs(30));
        until(&player, "seeks while playing", &|s| s.position >= Duration::from_secs(30));
        player.pause();
        let paused = until(&player, "pauses", &|s| s.state == State::Paused);
        std::thread::sleep(Duration::from_millis(700));
        assert!(player.status().position < paused.position + Duration::from_millis(300), "the position moved while paused");
        player.seek(Duration::from_secs(50));
        until(&player, "seeks while paused", &|s| s.state == State::Paused && s.position >= Duration::from_secs(50));
        player.resume();
        until(&player, "resumes", &|s| s.state == State::Playing && s.position > Duration::from_secs(51));
        player.seek(Duration::from_secs(100_000));
        until(&player, "ends when sent past the end", &|s| s.state == State::Ended);
        let serial = player.play(Request { path: "test/other.wma".to_owned(), size: 0 });
        until(&player, "refuses what it cannot decode", &|s| s.serial == serial && matches!(s.state, State::Failed(_)));
        let serial = player.play(Request { path: "test/not-there.mp3".to_owned(), size: 0 });
        until(&player, "reports a failed fetch", &|s| s.serial == serial && matches!(s.state, State::Failed(_)));
        println!("the fetch failed with: {:?}", player.status().state);

        // From a music folder the file is played where it is, and one that is gone says so.
        player.set_origin(Origin::Folder(song.parent().unwrap().to_owned()));
        let serial = player.play(Request { path: song.file_name().unwrap().to_str().unwrap().to_owned(), size: 0 });
        until(&player, "plays from a folder", &|s| s.serial == serial && s.state == State::Playing && s.position > Duration::from_millis(600));
        let serial = player.play(Request { path: "not-there.mp3".to_owned(), size: 0 });
        until(&player, "says a file is gone", &|s| s.serial == serial && matches!(&s.state, State::Failed(why) if why.contains("not there")));
        player.set_origin(Origin::Nowhere);
        let serial = player.play(Request { path: "any.mp3".to_owned(), size: 0 });
        until(&player, "plays nothing from nowhere", &|s| s.serial == serial && matches!(&s.state, State::Failed(why) if why.contains("Settings")));
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn the_volume_slider_follows_loudness_as_heard() {
        let db = |level: f32| 20.0 * gain(level).log10();
        assert_eq!((gain(0.0), gain(1.0)), (0.0, 1.0));
        assert!((db(0.5) + 10.0).abs() < 0.1, "halfway sounds half as loud: {} dB", db(0.5));
        assert!((1..=20).all(|i| gain(i as f32 / 20.0) > gain((i - 1) as f32 / 20.0)), "every step up is louder");
        assert_eq!((gain(-1.0), gain(2.0)), (0.0, 1.0), "the slider's ends hold");
    }

    #[test]
    fn cache_names_keep_the_extension_and_differ_by_path() {
        let cache = Path::new("/c");
        let a = cached(cache, "music/A/01 One.MP3");
        assert_eq!(a.extension().unwrap(), "mp3");
        assert_ne!(a, cached(cache, "music/A/02 Two.MP3"));
        assert_eq!(a, cached(cache, "music/A/01 One.MP3"));
    }
}
