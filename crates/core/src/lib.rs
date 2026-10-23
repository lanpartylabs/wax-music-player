//! Shared library model for the scanner and the player.

pub mod index;
pub mod library;

pub use index::{Header, Track};
pub use library::{Album, Column, Facet, Library, Query, Sound, View};
