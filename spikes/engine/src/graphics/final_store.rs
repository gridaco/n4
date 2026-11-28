//! Finished path-traced pictures of a spatial group, by the identity of everything that made them.
//! A picture found here is the Final; a scene whose picture is missing is drawn as Draft and
//! leaves a request behind for whoever can render it.
use super::spatial::PathTrace;
use anyhow::{Context, Result, ensure};
use std::collections::{HashMap, VecDeque};
use std::hash::Hasher;
use std::path::PathBuf;
use std::sync::Arc;

/// Bump when the meaning of a stored picture changes.
const FORMAT: u64 = 2;
/// What traces the pictures. It is part of every key, so pictures from another Cycles release or
/// another bridge are not mistaken for this one's. It must equal the bridge crate's `REVISION`.
pub(crate) const TRACER: &str = "cycles 5.3.0, n4 bridge 1";
const MAGIC: &[u8; 8] = b"N4FINAL1";
const MEMORY_BUDGET: usize = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FinalKey(pub u64);

/// FNV-1a. Unlike the standard hasher its output is fixed across runs, so keys can name files.
pub(crate) struct StableHasher(u64);

impl StableHasher {
    pub(crate) fn new() -> Self {
        let mut hasher = Self(0xcbf2_9ce4_8422_2325);
        hasher.write_u64(FORMAT);
        hasher.write(TRACER.as_bytes());
        hasher
    }
}

impl Hasher for StableHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// The spatial group as the 2D stack receives it.
pub(crate) struct FinalPicture {
    pub size: [u32; 2],
    /// Premultiplied, sRGB-encoded RGBA8, top row first: the bytes of the resolved group texture.
    pub rgba: Vec<u8>,
    /// One R8 coverage layer per requested matte, in request order.
    pub mattes: Vec<Vec<u8>>,
}

// Only a build with a path tracer reads a request.
#[cfg_attr(not(feature = "cycles"), allow(dead_code))]
pub(crate) struct FinalSurface {
    pub slot: String,
    pub texture: wgpu::Texture,
    pub role: super::spatial::SurfaceRole,
    pub emission: f32,
    pub glow: f32,
}

// Only a build with a path tracer reads a request.
#[cfg_attr(not(feature = "cycles"), allow(dead_code))]
pub(crate) struct FinalModel {
    pub id: Option<String>,
    pub mesh: (PathBuf, String),
    /// Identity of the mesh's content: the same path can come to hold another mesh.
    pub content: u64,
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub parts: Vec<(String, [f32; 3])>,
    pub surfaces: Vec<FinalSurface>,
}

// Only a build with a path tracer reads a request.
#[cfg_attr(not(feature = "cycles"), allow(dead_code))]
pub(crate) struct FinalRect {
    pub center: [f32; 3],
    pub edge_u: [f32; 3],
    pub edge_v: [f32; 3],
    pub radiance: [f32; 3],
}

/// One evaluated instant of a path-traced scene: everything a renderer needs, and nothing that
/// refers back to the timeline.
#[cfg_attr(not(feature = "cycles"), allow(dead_code))]
pub(crate) struct FinalRequest {
    pub key: FinalKey,
    pub size: [u32; 2],
    pub camera_position: [f32; 3],
    pub camera_rotation_degrees: [f32; 3],
    pub fov_y_degrees: f32,
    pub models: Vec<FinalModel>,
    pub rects: Vec<FinalRect>,
    pub ambient: [f32; 3],
    pub exposure_ev: f32,
    pub recipe: PathTrace,
    /// Scene object ids whose visible coverage was requested, in matte order.
    pub mattes: Vec<String>,
}

#[derive(Default)]
pub(crate) struct FinalStore {
    pictures: HashMap<FinalKey, Arc<FinalPicture>>,
    order: VecDeque<FinalKey>,
    bytes: usize,
    pending: Vec<FinalRequest>,
    directory: Option<PathBuf>,
    /// The most the directory may hold, and what it holds now once that has been measured.
    budget: Option<u64>,
    stored: Option<u64>,
}

impl FinalStore {
    /// Keep finished pictures on disk as well, so they survive the process and a new revision.
    /// With a budget, the pictures used longest ago are deleted once the directory outgrows it.
    pub(crate) fn persist_in(&mut self, directory: PathBuf, budget: Option<u64>) -> Result<()> {
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("create final store {}", directory.display()))?;
        self.directory = Some(directory);
        self.budget = budget;
        self.stored = None;
        Ok(())
    }

    /// Delete the pictures used longest ago until the directory is a tenth under its budget.
    /// Another process may be deleting too; a picture that disappears is only traced again.
    fn evict(&mut self, added: u64) {
        let (Some(budget), Some(directory)) = (self.budget, &self.directory) else {
            return;
        };
        if let Some(stored) = &mut self.stored {
            *stored += added;
            if *stored <= budget {
                return;
            }
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut files: Vec<_> = entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let data = std::fs::metadata(&path).ok()?;
                (path.extension()? == "n4final").then_some((
                    data.modified().ok()?,
                    data.len(),
                    path,
                ))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total > budget {
            files.sort();
            for (_, size, path) in files {
                if total <= budget - budget / 10 {
                    break;
                }
                if std::fs::remove_file(path).is_ok() {
                    total -= size;
                }
            }
        }
        self.stored = Some(total);
    }

    fn path(&self, key: FinalKey) -> Option<PathBuf> {
        self.directory
            .as_ref()
            .map(|d| d.join(format!("{:016x}.n4final", key.0)))
    }

    /// The picture for `key`, provided it has the size and the number of mattes its scene
    /// asks for. A file is only trusted as far as it fits the request.
    pub(crate) fn get(
        &mut self,
        key: FinalKey,
        size: [u32; 2],
        mattes: usize,
    ) -> Option<Arc<FinalPicture>> {
        let fits = |p: &FinalPicture| p.size == size && p.mattes.len() == mattes;
        if let Some(picture) = self.pictures.get(&key) {
            return fits(picture).then(|| Arc::clone(picture));
        }
        let path = self.path(key)?;
        let picture = Arc::new(read(&path).ok().filter(fits)?);
        if self.budget.is_some() {
            // Mark the use, so eviction keeps what is still being looked at.
            if let Ok(file) = std::fs::File::options().write(true).open(&path) {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
        }
        self.remember(key, Arc::clone(&picture));
        Some(picture)
    }

    fn remember(&mut self, key: FinalKey, picture: Arc<FinalPicture>) {
        let size = picture.rgba.len() + picture.mattes.iter().map(Vec::len).sum::<usize>();
        while self.bytes + size > MEMORY_BUDGET {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(p) = self.pictures.remove(&old) {
                self.bytes -= p.rgba.len() + p.mattes.iter().map(Vec::len).sum::<usize>();
            }
        }
        if self.pictures.insert(key, picture).is_none() {
            self.order.push_back(key);
            self.bytes += size;
        }
    }

    pub(crate) fn insert(&mut self, key: FinalKey, picture: FinalPicture) -> Result<()> {
        if let Some(path) = self.path(key) {
            self.evict(write(&path, &picture)?);
        }
        self.remember(key, Arc::new(picture));
        Ok(())
    }

    /// A scene was drawn as Draft because its Final is missing.
    pub(crate) fn want(&mut self, request: FinalRequest) {
        if !self.pending.iter().any(|p| p.key == request.key) {
            // A host that only draws Drafts never collects these; keep the newest few.
            if self.pending.len() == 8 {
                self.pending.remove(0);
            }
            self.pending.push(request);
        }
    }

    pub(crate) fn take_pending(&mut self) -> Vec<FinalRequest> {
        std::mem::take(&mut self.pending)
    }
}

/// Returns the number of bytes stored.
fn write(path: &PathBuf, picture: &FinalPicture) -> Result<u64> {
    let mut bytes = Vec::with_capacity(picture.rgba.len() + 32);
    bytes.extend_from_slice(MAGIC);
    for value in [
        picture.size[0],
        picture.size[1],
        picture.mattes.len() as u32,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&picture.rgba);
    for matte in &picture.mattes {
        bytes.extend_from_slice(matte);
    }
    // Write beside the destination and rename, so a reader never sees a partial picture.
    // Every write has its own partial file; two engines may finish the same picture at once.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let partial = path.with_extension(format!(
        "{}-{}.partial",
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&partial, &bytes).with_context(|| format!("write {}", partial.display()))?;
    std::fs::rename(&partial, path).with_context(|| format!("store {}", path.display()))?;
    Ok(bytes.len() as u64)
}

fn read(path: &PathBuf) -> Result<FinalPicture> {
    let bytes = std::fs::read(path)?;
    ensure!(
        bytes.len() >= 20 && &bytes[..8] == MAGIC,
        "not a final picture"
    );
    let field = |i: usize| u32::from_le_bytes(bytes[8 + i * 4..12 + i * 4].try_into().unwrap());
    let (size, count) = ([field(0), field(1)], field(2) as usize);
    // The header is not trusted: sizes that overflow are simply not this file's length.
    let pixels = (size[0] as usize)
        .checked_mul(size[1] as usize)
        .filter(|&pixels| pixels > 0);
    let length = pixels
        .zip(count.checked_add(4))
        .and_then(|(pixels, planes)| pixels.checked_mul(planes))
        .and_then(|body| body.checked_add(20));
    ensure!(
        length == Some(bytes.len()),
        "final picture has an unexpected length"
    );
    let pixels = pixels.unwrap();
    let rgba = bytes[20..20 + pixels * 4].to_vec();
    let mattes = (0..count)
        .map(|i| bytes[20 + pixels * (4 + i)..20 + pixels * (5 + i)].to_vec())
        .collect();
    Ok(FinalPicture { size, rgba, mattes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_round_trip_through_disk_and_keys_are_stable() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = FinalStore::default();
        store.persist_in(directory.path().to_path_buf(), None)?;
        let key = FinalKey(0x1234);
        assert!(store.get(key, [2, 1], 1).is_none());
        store.insert(
            key,
            FinalPicture {
                size: [2, 1],
                rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
                mattes: vec![vec![9, 10]],
            },
        )?;
        // A new store over the same directory finds the picture without having rendered it.
        let mut reopened = FinalStore::default();
        reopened.persist_in(directory.path().to_path_buf(), None)?;
        // A picture of another size or matte count is not the one a scene asks for.
        assert!(reopened.get(key, [1, 2], 1).is_none());
        assert!(reopened.get(key, [2, 1], 0).is_none());
        let picture = reopened.get(key, [2, 1], 1).context("stored picture")?;
        assert_eq!(picture.size, [2, 1]);
        assert_eq!(picture.rgba, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(picture.mattes, [vec![9, 10]]);
        // A header that lies about its size is rejected, not followed.
        let lying = directory.path().join(format!("{:016x}.n4final", 0x99));
        let mut bytes = MAGIC.to_vec();
        for field in [0x8000_0000u32, 0x8000_0000, 0] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        std::fs::write(&lying, bytes)?;
        assert!(read(&lying).is_err());
        // Keys name files, so the digest is the same in every run, build and process. This value
        // changes only with FORMAT or TRACER, which is when stored pictures must stop matching.
        let mut a = StableHasher::new();
        a.write(b"scene");
        assert_eq!(a.finish(), 0x5f674d8c04bf9be6);
        Ok(())
    }

    #[test]
    fn a_budgeted_store_deletes_the_pictures_used_longest_ago() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let picture = || FinalPicture {
            size: [5, 4],
            rgba: vec![7; 80],
            mattes: Vec::new(),
        };
        // Each file is 100 bytes; four fit.
        let mut store = FinalStore::default();
        store.persist_in(directory.path().to_path_buf(), Some(400))?;
        let age = |key: u64, seconds: u64| -> Result<()> {
            let path = directory.path().join(format!("{key:016x}.n4final"));
            let then = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds);
            Ok(std::fs::File::options()
                .write(true)
                .open(path)?
                .set_modified(then)?)
        };
        for key in 1..=4 {
            store.insert(FinalKey(key), picture())?;
            age(key, 100 - key)?;
        }
        // A second process reads the oldest picture, which marks it as used.
        let mut reader = FinalStore::default();
        reader.persist_in(directory.path().to_path_buf(), Some(400))?;
        assert!(reader.get(FinalKey(1), [5, 4], 0).is_some());
        store.insert(FinalKey(5), picture())?;
        let mut left: Vec<_> = std::fs::read_dir(directory.path())?
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "0000000000000001.n4final",
                "0000000000000004.n4final",
                "0000000000000005.n4final"
            ]
        );
        // What this process still holds in memory is unaffected; the deleted file is a miss
        // only for a process that never read it.
        assert!(reader.get(FinalKey(2), [5, 4], 0).is_none());
        Ok(())
    }
}
