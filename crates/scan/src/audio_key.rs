//! A key for a file's audio data that survives retagging, so two copies of
//! a track with different tags can be recognised as the same audio.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::art::fnv1a;

/// Bytes hashed per file.
const WINDOW: u64 = 64 * 1024;
/// How far into the audio the hashed slice starts, past any silent lead-in.
const SKIP: u64 = 1024 * 1024;

/// Hashes [`WINDOW`] bytes of audio.
///
/// For MP3 the slice is placed relative to the end of the leading ID3v2 tag,
/// so the key ignores tag changes. Other formats keep their metadata in
/// places that are costly to skip; for them the slice is taken from the
/// middle of the file and the size is mixed in, which only matches
/// byte-identical copies.
pub fn audio_key(path: &Path, size: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let is_mp3 = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp3"));
    let (offset, salt) = if is_mp3 {
        let mut head = [0u8; 10];
        file.read_exact(&mut head).ok()?;
        let audio_start = if &head[..3] == b"ID3" { id3v2_len(&head) } else { 0 };
        (if size > audio_start + SKIP + WINDOW { audio_start + SKIP } else { audio_start }, 0)
    } else {
        (size.saturating_sub(WINDOW) / 2, size)
    };

    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut slice = Vec::with_capacity(WINDOW as usize);
    file.take(WINDOW).read_to_end(&mut slice).ok()?;
    (!slice.is_empty()).then(|| format!("{:016x}", fnv1a(&slice) ^ salt))
}

/// Total length of an ID3v2 tag given its 10-byte header.
fn id3v2_len(head: &[u8; 10]) -> u64 {
    // The size is four 7-bit bytes and excludes the header and optional footer.
    let body = head[6..10].iter().fold(0u64, |n, &b| (n << 7) | (b & 0x7f) as u64);
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    10 + body + footer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp3(tag_body: usize, audio: &[u8]) -> Vec<u8> {
        let mut bytes = b"ID3\x03\x00\x00".to_vec();
        bytes.extend((0..4).rev().map(|i| ((tag_body >> (7 * i)) & 0x7f) as u8));
        bytes.resize(10 + tag_body, 0);
        bytes.extend_from_slice(audio);
        bytes
    }

    fn key_of(name: &str, bytes: &[u8]) -> Option<String> {
        let path = std::env::temp_dir().join(format!("wax-key-test-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let key = audio_key(&path, bytes.len() as u64);
        std::fs::remove_file(&path).unwrap();
        key
    }

    #[test]
    fn mp3_key_ignores_the_tag_but_not_the_audio() {
        let audio: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        let small_tag = key_of("a.mp3", &mp3(300, &audio)).unwrap();
        let big_tag = key_of("b.mp3", &mp3(250_000, &audio)).unwrap();
        assert_eq!(small_tag, big_tag);

        let mut other = audio.clone();
        other[SKIP as usize + 10] ^= 0xff;
        assert_ne!(key_of("c.mp3", &mp3(300, &other)).unwrap(), small_tag);
    }

    #[test]
    fn other_formats_match_only_identical_files() {
        let data = vec![7u8; 200_000];
        assert_eq!(key_of("a.flac", &data), key_of("b.flac", &data));
        assert_ne!(key_of("c.flac", &data[..199_999]), key_of("a2.flac", &data));
        assert_eq!(key_of("empty.flac", &[]), None);
    }
}
