//! Songs dropped from the library by hand. Dropping one hides it here at
//! once and has the librarian leave it out of the catalogue from then on. No
//! file is deleted by this: what was dropped is kept as a list, on this
//! machine and with the librarian, for its owner to remove for good or not.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// The list kept on this machine: one path of the index a line.
fn file() -> PathBuf {
    crate::state::file("discarded.txt")
}

/// Everything dropped so far.
pub fn load() -> HashSet<String> {
    std::fs::read_to_string(file()).unwrap_or_default().lines().filter(|line| !line.is_empty()).map(str::to_owned).collect()
}

/// Adds to the list on this machine. Says why not when it cannot be written.
pub fn record(paths: &[String]) -> Result<(), String> {
    let file = file();
    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
        let mut list = std::fs::OpenOptions::new().create(true).append(true).open(&file)?;
        paths.iter().try_for_each(|path| writeln!(list, "{path}"))
    });
    written.map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// Tells the librarian, in the background, of everything dropped; sending what it already knows does no harm. What
/// came of it is left in `said` for the status bar.
pub fn send(host: String, home: String, paths: Vec<String>, said: Arc<Mutex<Option<String>>>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let outcome = (|| -> Result<String, String> {
            let command = format!("cd '{}' && exec python3 -m librarian discard", home.replace('\'', "'\\''"));
            let mut ssh = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host.as_str(), command.as_str()])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("ssh could not be started: {e}"))?;
            let mut lines = paths.join("\n");
            lines.push('\n');
            ssh.stdin.take().unwrap().write_all(lines.as_bytes()).map_err(|e| e.to_string())?;
            let done = ssh.wait_with_output().map_err(|e| e.to_string())?;
            if done.status.success() {
                Ok(String::from_utf8_lossy(&done.stdout).trim().to_owned())
            } else {
                Err(String::from_utf8_lossy(&done.stderr).lines().last().unwrap_or("it did not answer").trim().to_owned())
            }
        })();
        *said.lock().unwrap() = Some(match outcome {
            Ok(_) => "the library on the server has been told".to_owned(),
            Err(why) => format!("the server could not be told yet ({why}); it will be told at the next start"),
        });
        ctx.request_repaint();
    });
}
