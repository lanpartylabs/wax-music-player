//! wax: a keyboard-first music player.

mod app;
mod art;
mod devtools;
mod discard;
mod draw;
mod format;
mod history;
mod list;
mod player;
mod playlists;
mod shuffle;
mod state;
mod theme;

use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;
use wax_core::Library;

use crate::app::{Settings, Source, Start, data_dir, folder_index, open_index};

const USAGE: &str = "\
usage: wax [--index FILE] [--art DIR] [SEARCH WORDS...]

  --index FILE   library index to open, as wax-scan or the librarian writes it
                 (default: the list of the music folder chosen in Settings, else
                 a catalogue at $XDG_DATA_HOME/wax/library.jsonl.gz)
  --art DIR      cover thumbnails for that index
                 (default: the art folder next to the index)
  SEARCH WORDS   start with this search already applied";

fn main() -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let settings = Settings::load();
    let mut index = None;
    let mut art = None;
    let mut search = Vec::new();
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--index" => index = Some(PathBuf::from(argv.next().ok_or(USAGE)?)),
            "--art" => art = Some(PathBuf::from(argv.next().ok_or(USAGE)?)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => search.push(arg),
        }
    }

    // A music folder chosen in Settings is listed in a file of its own; without one, a librarian's catalogue is read.
    let named = index.is_some();
    let index = index.unwrap_or_else(|| if settings.music.is_empty() { data_dir().join("library.jsonl.gz") } else { folder_index() });
    let gone = discard::load();
    let (lib, source) = if named || index.exists() {
        open_index(&index, art, &gone)?
    } else {
        // Nothing yet: the window starts empty, and the music folder's scan or its choosing fills it.
        let art = art.unwrap_or_else(|| index.with_file_name("art"));
        (Library::new(Vec::new()), Source { index, art, generated: 0, ready: false, root: PathBuf::new() })
    };
    if std::env::var_os("WAX_TIMING").is_some() {
        eprintln!("library of {} tracks ready after {} ms", lib.len(), started.elapsed().as_millis());
    }
    let welcome = !named && lib.is_empty() && settings.music.is_empty();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("wax")
            .with_app_id("wax")
            .with_icon(icon())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([560.0, 320.0]),
        ..Default::default()
    };
    let start = Start { lib, source, settings, gone, search: search.join(" "), started, welcome };
    eframe::run_native("wax", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, start)))))?;
    Ok(())
}

/// The icon on the window, where the desktop takes it from there: X11, Windows, macOS. Wayland desktops
/// find it by the app id instead, in the desktop entry and icons under packaging/linux.
fn icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_is_a_square_picture() {
        let icon = super::icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
