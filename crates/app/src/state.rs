//! Where wax keeps what it learns as it is used: the playlists, the songs dropped, what has been played and where each
//! shuffle stands. That is `$XDG_STATE_HOME/wax`, else `~/.local/state/wax`; the library's own files, which a new
//! catalogue overwrites, are kept elsewhere.

use std::path::PathBuf;

/// The file of that name in wax's state folder.
pub fn file(name: &str) -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_default();
    state.join("wax").join(name)
}
