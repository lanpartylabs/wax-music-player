//! wax-scan: walk a music tree, read the tags, write the library index.
//!
//! The player scans its music folder itself; this is the same scan from the
//! command line, for a library it should not walk itself, such as one on a
//! server: run it next to the storage, where file access is cheap.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Mutex;

use wax_scan::{Options, Progress, scan};

const USAGE: &str = "\
usage: wax-scan --root DIR --out FILE [--include SUBDIR]... [--art DIR] [--audio-key] [--reuse FILE] [--jobs N]

  --root DIR        library root; paths in the index are relative to it
  --include SUBDIR  only scan this folder under the root (repeatable)
  --out FILE        index to write (gzip-compressed JSON lines)
  --art DIR         also save a cover thumbnail per album into this folder
  --audio-key       also hash a slice of each file's audio, to find duplicates
  --reuse FILE      previous index to take unchanged files from (default: --out)
  --jobs N          parallel readers (default 16; file access is I/O-bound)";

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_args()?;
    let last = Mutex::new(Progress::default());
    let summary = scan(&options, &|progress| {
        let mut last = last.lock().unwrap();
        if progress.found > 0 && progress.read == 0 {
            eprintln!("found {} audio files", progress.found);
        } else if progress.read / 1000 > last.read / 1000 && progress.read < progress.found {
            eprintln!("  {}/{}", progress.read, progress.found);
        }
        *last = progress;
    })
    .map_err(|e| e.to_string())?;
    eprintln!(
        "wrote {} tracks ({} h of audio) to {} in {:.1}s; {} reused, {} without readable tags, {} with artwork",
        summary.tracks,
        summary.hours,
        options.out.display(),
        summary.seconds,
        summary.reused,
        summary.unreadable,
        summary.with_art,
    );
    Ok(())
}

fn parse_args() -> Result<Options, Box<dyn Error>> {
    let (mut root, mut out, mut art, mut reuse, mut include, mut jobs) = (None, None, None, None, Vec::new(), 16);
    let mut audio_key = false;
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().ok_or(format!("{flag} needs a value\n\n{USAGE}"));
        match flag.as_str() {
            "--root" => root = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--art" => art = Some(PathBuf::from(value()?)),
            "--audio-key" => audio_key = true,
            "--reuse" => reuse = Some(PathBuf::from(value()?)),
            "--include" => include.push(value()?.trim_matches('/').to_owned()),
            "--jobs" => jobs = value()?.parse()?,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {flag}\n\n{USAGE}").into()),
        }
    }
    match (root, out) {
        (Some(root), Some(out)) => Ok(Options { root, include, out, art, audio_key, reuse, jobs }),
        _ => Err(format!("--root and --out are required\n\n{USAGE}").into()),
    }
}
