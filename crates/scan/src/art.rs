//! Cover thumbnails: every distinct image is stored once as `<id>.jpg`,
//! where the id is a hash of the original image bytes.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;

/// Edge length of the square thumbnails, in pixels.
const SIZE: u32 = 320;
const JPEG_QUALITY: u8 = 82;

type Once<T> = Arc<OnceLock<T>>;

pub struct ArtStore {
    dir: PathBuf,
    /// One slot per image so the tracks of an album, which are read in
    /// parallel and share a cover, decode it only once.
    images: Mutex<HashMap<u64, Once<bool>>>,
    /// Cover image files already looked at, by path.
    files: Mutex<HashMap<PathBuf, Once<Option<String>>>>,
}

impl ArtStore {
    pub fn new(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(ArtStore { dir, images: Mutex::default(), files: Mutex::default() })
    }

    /// Stores a thumbnail of the encoded image `bytes` and returns its id,
    /// or `None` when the image cannot be decoded.
    pub fn add(&self, bytes: &[u8]) -> Option<String> {
        let hash = fnv1a(bytes);
        let id = format!("{hash:016x}");
        let slot = self.images.lock().unwrap().entry(hash).or_default().clone();
        let stored = *slot.get_or_init(|| {
            let path = self.dir.join(format!("{id}.jpg"));
            path.exists() || write_thumbnail(bytes, &path).is_some()
        });
        stored.then_some(id)
    }

    /// Like [`Self::add`] for an image file, read at most once per run.
    pub fn add_file(&self, path: &Path) -> Option<String> {
        let slot = self.files.lock().unwrap().entry(path.to_owned()).or_default().clone();
        slot.get_or_init(|| self.add(&std::fs::read(path).ok()?)).clone()
    }
}

fn write_thumbnail(bytes: &[u8], path: &Path) -> Option<()> {
    let thumbnail = image::load_from_memory(bytes).ok()?.resize_to_fill(SIZE, SIZE, FilterType::CatmullRom).into_rgb8();
    // Written under a temporary name so a reader never sees half a file.
    let tmp = path.with_extension("tmp");
    let mut out = BufWriter::new(File::create(&tmp).ok()?);
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&thumbnail).ok()?;
    out.into_inner().ok()?;
    std::fs::rename(&tmp, path).ok()
}

/// How good a file name is as an album cover; lower is better, `None` means not a candidate.
pub fn cover_rank(file_name: &str) -> Option<u8> {
    let lower = file_name.to_ascii_lowercase();
    let (stem, ext) = lower.rsplit_once('.')?;
    if !["jpg", "jpeg", "png"].contains(&ext) || stem.contains("small") || stem.contains("back") {
        return None;
    }
    Some(match stem {
        "cover" => 0,
        "folder" => 1,
        "front" => 2,
        _ if stem.contains("cover") || stem.contains("front") => 3,
        _ if stem.contains("albumart") => 4,
        _ => 5,
    })
}

pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| (hash ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_names_are_ranked() {
        assert_eq!(cover_rank("cover.jpg"), Some(0));
        assert_eq!(cover_rank("Folder.JPG"), Some(1));
        assert_eq!(cover_rank("00 - front cover.png"), Some(3));
        assert_eq!(cover_rank("AlbumArt_{5FA05D35}_Large.jpg"), Some(4));
        assert_eq!(cover_rank("AlbumArtSmall.jpg"), None);
        assert_eq!(cover_rank("back.jpg"), None);
        assert_eq!(cover_rank("scan.jpeg"), Some(5));
        assert_eq!(cover_rank("notes.txt"), None);
    }

    #[test]
    fn identical_images_share_one_thumbnail() {
        let dir = std::env::temp_dir().join(format!("wax-art-test-{}", std::process::id()));
        let store = ArtStore::new(dir.clone()).unwrap();
        let mut png = Vec::new();
        image::RgbImage::from_pixel(8, 4, image::Rgb([200, 30, 30]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let id = store.add(&png).unwrap();
        assert_eq!(store.add(&png).as_deref(), Some(id.as_str()));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let saved = image::open(dir.join(format!("{id}.jpg"))).unwrap();
        assert_eq!((saved.width(), saved.height()), (SIZE, SIZE));
        assert_eq!(store.add(b"not an image"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
