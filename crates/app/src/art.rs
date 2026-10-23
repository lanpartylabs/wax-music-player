//! Cover artwork: thumbnails are decoded on background threads and kept as
//! textures for as long as they are being shown.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};

use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};

const WORKERS: usize = 4;
/// Textures kept beyond the ones on screen, so scrolling back is instant.
const KEEP: usize = 400;

enum Slot {
    Ready { texture: TextureHandle, used: u64 },
    /// The file is absent or not an image; do not ask again.
    Missing,
}

#[derive(Default)]
struct Pending {
    queue: VecDeque<u64>,
    in_flight: HashSet<u64>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    wake: Condvar,
}

pub struct Art {
    slots: HashMap<u64, Slot>,
    shared: Arc<Shared>,
    loaded: Receiver<(u64, Option<ColorImage>)>,
    frame: u64,
    /// Covers asked for this frame that are not loaded yet, in paint order.
    wanted: Vec<u64>,
}

impl Art {
    /// `dir` holds one `<id>.jpg` per cover, as written by wax-scan.
    pub fn new(dir: PathBuf, ctx: &egui::Context) -> Self {
        let shared = Arc::new(Shared::default());
        let (sender, loaded) = mpsc::channel();
        for _ in 0..WORKERS {
            let (shared, sender, dir, ctx) = (shared.clone(), sender.clone(), dir.clone(), ctx.clone());
            std::thread::spawn(move || {
                while let Some(id) = next_job(&shared) {
                    let image = std::fs::read(dir.join(format!("{id:016x}.jpg"))).ok().and_then(|bytes| decode(&bytes));
                    if sender.send((id, image)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
        Art { slots: HashMap::new(), shared, loaded, frame: 0, wanted: Vec::new() }
    }

    /// Turns finished decodes into textures. Call before painting.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        self.wanted.clear();
        let finished: Vec<_> = self.loaded.try_iter().collect();
        if finished.is_empty() {
            return;
        }
        let mut pending = self.shared.pending.lock().unwrap();
        for (id, image) in finished {
            pending.in_flight.remove(&id);
            let slot = match image {
                Some(image) => {
                    let texture = ctx.load_texture(format!("cover-{id:016x}"), image, TextureOptions::LINEAR);
                    Slot::Ready { texture, used: self.frame }
                }
                None => Slot::Missing,
            };
            self.slots.insert(id, slot);
        }
    }

    /// The texture for cover `id` if it is loaded; otherwise it is queued and
    /// the caller draws a placeholder. Id 0 means "no cover".
    pub fn get(&mut self, id: u64) -> Option<TextureId> {
        if id == 0 {
            return None;
        }
        match self.slots.get_mut(&id) {
            Some(Slot::Ready { texture, used }) => {
                *used = self.frame;
                Some(texture.id())
            }
            Some(Slot::Missing) => None,
            None => {
                self.wanted.push(id);
                None
            }
        }
    }

    /// Hands this frame's requests to the workers and frees textures that
    /// have not been shown for a while. Call after painting.
    pub fn end_frame(&mut self) {
        {
            // Replacing the queue each frame drops requests for covers that
            // have scrolled out of view before a worker got to them.
            let mut pending = self.shared.pending.lock().unwrap();
            let Pending { queue, in_flight, .. } = &mut *pending;
            queue.clear();
            queue.extend(self.wanted.iter().copied().filter(|id| !in_flight.contains(id)));
        }
        self.shared.wake.notify_all();

        let ready = self.slots.values().filter(|s| matches!(s, Slot::Ready { .. })).count();
        if ready > KEEP {
            let mut idle: Vec<(u64, u64)> = self
                .slots
                .iter()
                .filter_map(|(&id, slot)| match slot {
                    Slot::Ready { used, .. } if *used < self.frame => Some((*used, id)),
                    _ => None,
                })
                .collect();
            idle.sort_unstable();
            for (_, id) in idle.into_iter().take(ready - KEEP) {
                self.slots.remove(&id);
            }
        }
    }
}

impl Drop for Art {
    fn drop(&mut self) {
        self.shared.pending.lock().unwrap().closed = true;
        self.shared.wake.notify_all();
    }
}

fn next_job(shared: &Shared) -> Option<u64> {
    let mut pending = shared.pending.lock().unwrap();
    loop {
        if pending.closed {
            return None;
        }
        if let Some(id) = pending.queue.pop_front() {
            pending.in_flight.insert(id);
            return Some(id);
        }
        pending = shared.wake.wait(pending).unwrap();
    }
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.into_rgb8();
    Some(ColorImage::from_rgb([image.width() as usize, image.height() as usize], image.as_raw()))
}
