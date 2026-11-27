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
const FORMAT: u64 = 1;
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

pub(crate) struct FinalSurface {
    pub slot: String,
    pub texture: wgpu::Texture,
    pub role: super::spatial::SurfaceRole,
    pub emission: f32,
    pub glow: f32,
}

pub(crate) struct FinalModel {
    pub id: Option<String>,
    pub mesh: (PathBuf, String),
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub parts: Vec<(String, [f32; 3])>,
    pub surfaces: Vec<FinalSurface>,
}

pub(crate) struct FinalRect {
    pub center: [f32; 3],
    pub edge_u: [f32; 3],
    pub edge_v: [f32; 3],
    pub radiance: [f32; 3],
}

/// One evaluated instant of a path-traced scene: everything a renderer needs, and nothing that
/// refers back to the timeline.
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
}

impl FinalStore {
    /// Keep finished pictures on disk as well, so they survive the process and a new revision.
    pub(crate) fn persist_in(&mut self, directory: PathBuf) -> Result<()> {
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("create final store {}", directory.display()))?;
        self.directory = Some(directory);
        Ok(())
    }

    fn path(&self, key: FinalKey) -> Option<PathBuf> {
        self.directory
            .as_ref()
            .map(|d| d.join(format!("{:016x}.n4final", key.0)))
    }

    pub(crate) fn get(&mut self, key: FinalKey) -> Option<Arc<FinalPicture>> {
        if let Some(picture) = self.pictures.get(&key) {
            return Some(Arc::clone(picture));
        }
        let picture = Arc::new(read(&self.path(key)?).ok()?);
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
            write(&path, &picture)?;
        }
        self.remember(key, Arc::new(picture));
        Ok(())
    }

    /// A scene was drawn as Draft because its Final is missing.
    pub(crate) fn want(&mut self, request: FinalRequest) {
        if !self.pending.iter().any(|p| p.key == request.key) {
            self.pending.push(request);
        }
    }

    pub(crate) fn take_pending(&mut self) -> Vec<FinalRequest> {
        std::mem::take(&mut self.pending)
    }
}

fn write(path: &PathBuf, picture: &FinalPicture) -> Result<()> {
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
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes).with_context(|| format!("write {}", partial.display()))?;
    std::fs::rename(&partial, path).with_context(|| format!("store {}", path.display()))
}

fn read(path: &PathBuf) -> Result<FinalPicture> {
    let bytes = std::fs::read(path)?;
    ensure!(
        bytes.len() >= 20 && &bytes[..8] == MAGIC,
        "not a final picture"
    );
    let field = |i: usize| u32::from_le_bytes(bytes[8 + i * 4..12 + i * 4].try_into().unwrap());
    let (size, count) = ([field(0), field(1)], field(2) as usize);
    let pixels = size[0] as usize * size[1] as usize;
    ensure!(
        bytes.len() == 20 + pixels * (4 + count),
        "final picture has an unexpected length"
    );
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
        store.persist_in(directory.path().to_path_buf())?;
        let key = FinalKey(0x1234);
        assert!(store.get(key).is_none());
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
        reopened.persist_in(directory.path().to_path_buf())?;
        let picture = reopened.get(key).context("stored picture")?;
        assert_eq!(picture.size, [2, 1]);
        assert_eq!(picture.rgba, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(picture.mattes, [vec![9, 10]]);
        let mut a = StableHasher::new();
        a.write(b"scene");
        assert_eq!(a.finish(), {
            let mut b = StableHasher::new();
            b.write(b"scene");
            b.finish()
        });
        Ok(())
    }
}
