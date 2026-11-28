//! Recently presented full-quality pictures, private to one transport revision.
use crate::{Engine, Frame, RevisionChange};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const PIXEL_BUDGET: usize = 64 << 20;
// Bound trace metadata and the LRU scan even on tiny test surfaces.
const MAX_PICTURES: usize = 256;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameCacheStats {
    /// Interactive render requests served from this revision's cache.
    pub hits: u64,
    pub misses: u64,
    /// Clean entries transferred into this revision, whether requested again or not.
    pub inherited: u64,
    pub entries: usize,
    /// Allocated RGBA bytes retained by the cache. Presenters may hold other pictures.
    pub bytes: usize,
    pub budget_bytes: usize,
}

#[derive(Default)]
struct Pictures {
    lru: VecDeque<Frame>,
    bytes: usize,
    hits: u64,
    misses: u64,
    inherited: u64,
}

/// Cloning the handle observes the same revision. A candidate always opens its own cache.
#[derive(Clone)]
pub struct ComposedFrameCache {
    revision: u64,
    budget: usize,
    pictures: Arc<Mutex<Pictures>>,
}

impl ComposedFrameCache {
    pub(super) fn new(revision: u64) -> Self {
        Self::with_budget(revision, PIXEL_BUDGET)
    }

    fn with_budget(revision: u64, budget: usize) -> Self {
        Self {
            revision,
            budget,
            pictures: Arc::default(),
        }
    }

    pub fn stats(&self) -> FrameCacheStats {
        let pictures = self.pictures.lock().unwrap();
        FrameCacheStats {
            hits: pictures.hits,
            misses: pictures.misses,
            inherited: pictures.inherited,
            entries: pictures.lru.len(),
            bytes: pictures.bytes,
            budget_bytes: self.budget,
        }
    }

    fn get(&self, frame: u32) -> Option<Frame> {
        let mut pictures = self.pictures.lock().unwrap();
        let Some(index) = pictures
            .lru
            .iter()
            .position(|p| p.trace.request.timeline_frame == frame)
        else {
            pictures.misses += 1;
            return None;
        };
        pictures.hits += 1;
        let picture = pictures.lru.remove(index).expect("entry found above");
        let result = picture.clone();
        pictures.lru.push_back(picture);
        Some(result)
    }

    fn put(&self, picture: &Frame) {
        let bytes = picture.rgba.capacity();
        if bytes > self.budget {
            return;
        }
        let mut pictures = self.pictures.lock().unwrap();
        // A concurrently finishing candidate render takes precedence over a transferred picture.
        if let Some(index) = pictures
            .lru
            .iter()
            .position(|p| p.trace.request.timeline_frame == picture.trace.request.timeline_frame)
        {
            pictures.bytes -= pictures.lru.remove(index).unwrap().rgba.capacity();
        }
        while pictures.lru.len() >= MAX_PICTURES || bytes > self.budget - pictures.bytes {
            pictures.bytes -= pictures.lru.pop_front().unwrap().rgba.capacity();
        }
        pictures.lru.push_back(picture.clone());
        pictures.bytes += bytes;
    }

    pub(super) fn render(&self, engine: &mut Engine, frame: u32) -> Result<Frame> {
        if let Some(mut picture) = self.get(frame) {
            picture.trace.reused_composite = true;
            picture.trace.reused_decode = false;
            picture.trace.reused_paragraph = false;
            picture.trace.graphics = Default::default();
            if picture.trace.revision != self.revision {
                // Damage describes pixels. Trace the current authoring and VFR ordinals independently.
                picture.trace.request = engine.resolved_request(frame)?;
                picture.trace.revision = self.revision;
                self.put(&picture);
            }
            return Ok(picture);
        }
        let picture = engine.render(frame)?;
        // A Draft of a path-traced scene is superseded as soon as its Final is traced.
        if picture.trace.graphics.path_traced_draft == 0 {
            self.put(&picture);
        }
        Ok(picture)
    }

    /// Trace the frame's path-traced scenes and keep the finished picture.
    pub(super) fn finish(&self, engine: &mut Engine, frame: u32) -> Result<Frame> {
        let (picture, _) = engine.render_final(frame)?;
        self.put(&picture);
        Ok(picture)
    }

    pub(super) fn inherit(&self, previous: &Self, change: &RevisionChange) -> Result<()> {
        ensure!(
            previous.revision == change.from_revision
                && self.revision == change.to_revision
                && change.from_revision.checked_add(1) == Some(change.to_revision),
            "cache transfer needs consecutive matching revisions"
        );
        // Take shared pixel references under one lock at a time. A still-running old worker
        // can finish a seek without modifying the candidate's cache.
        let clean: Vec<_> = previous
            .pictures
            .lock()
            .unwrap()
            .lru
            .iter()
            .rev()
            .filter(|p| {
                !change
                    .damage
                    .picture
                    .contains(p.trace.request.timeline_frame)
            })
            .cloned()
            .collect();
        let mut pictures = self.pictures.lock().unwrap();
        for picture in clean {
            if pictures.lru.len() >= MAX_PICTURES {
                break;
            }
            let bytes = picture.rgba.capacity();
            if bytes > self.budget - pictures.bytes
                || pictures
                    .lru
                    .iter()
                    .any(|p| p.trace.request.timeline_frame == picture.trace.request.timeline_frame)
            {
                continue;
            }
            // Protect freshly validated candidate pictures and retain the old recency order.
            pictures.lru.push_front(picture);
            pictures.bytes += bytes;
            pictures.inherited += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OutputDamage, Project, ProjectDamage, RenderTrace, transport::Transport};
    use serde_json::json;
    use std::{
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn budget_tracks_allocations_and_evicts_least_recent_pictures() -> Result<()> {
        let project: Project = serde_json::from_value(json!({
            "size":[1,1], "fps":[30,1], "shots":[{"duration":9,"background":{"color":[0,0,0]}}]
        }))?;
        let picture = |frame, capacity| -> Result<Frame> {
            let mut rgba = Vec::with_capacity(capacity);
            rgba.resize(4, 255);
            Ok(Frame {
                size: [1, 1],
                rgba: Arc::new(rgba),
                trace: RenderTrace {
                    revision: 0,
                    request: project.request(frame)?,
                    proxy: None,
                    paragraph_lines: None,
                    backplate_rect: None,
                    reused_decode: false,
                    reused_paragraph: false,
                    reused_composite: false,
                    graphics: Default::default(),
                },
            })
        };
        let cache = ComposedFrameCache::with_budget(0, 12);
        cache.put(&picture(0, 8)?);
        cache.put(&picture(1, 4)?);
        assert_eq!(cache.stats().bytes, 12);
        assert!(cache.get(0).is_some());
        cache.put(&picture(2, 4)?);
        assert!(cache.get(1).is_none());
        cache.put(&picture(4, 32)?);
        assert!(cache.get(4).is_none());
        assert_eq!((cache.stats().bytes, cache.stats().entries), (12, 2));
        let candidate = ComposedFrameCache::with_budget(1, 8);
        candidate.put(&picture(3, 4)?);
        let change = RevisionChange::after(
            0,
            ProjectDamage {
                picture: OutputDamage::None,
                audio: OutputDamage::None,
            },
        )?;
        candidate.inherit(&cache, &change)?;
        assert_eq!(candidate.stats().inherited, 1);
        assert!(candidate.get(2).is_some());
        assert!(candidate.get(3).is_some());
        assert!(candidate.get(0).is_none());
        assert!(cache.get(0).is_some());
        let fallback = ComposedFrameCache::new(1);
        fallback.inherit(&cache, &RevisionChange::whole_project_after(0)?)?;
        assert_eq!(fallback.stats().entries, 0);
        assert!(cache.inherit(&candidate, &change).is_err());
        Ok(())
    }

    fn wait(transport: &Transport) -> Result<Frame> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(picture) = transport.try_next()? {
                return Ok(picture);
            }
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for transport frame"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn seek_and_play_reuse_only_clean_pixels_and_trace_the_current_revision() -> Result<()> {
        let shot =
            |duration, color: [u8; 3]| json!({"duration":duration, "background":{"color":color}});
        let project = |shots| -> Result<Project> {
            Ok(serde_json::from_value(json!({
                "size":[32,16], "fps":[30,1], "shots":shots
            }))?)
        };
        let red = [255, 0, 0];
        let old = Transport::open_revision(project(vec![shot(4, red), shot(2, [0, 0, 255])])?, 7)?;
        old.seek(3)?;
        let prior = wait(&old)?;
        assert_eq!(prior.trace.request.shot_frame, 3);
        old.seek(4)?;
        let dirty = wait(&old)?;
        let new = Transport::open_revision(
            project(vec![shot(2, red), shot(2, red), shot(2, [255, 255, 0])])?,
            8,
        )?;
        let change = RevisionChange::after(
            7,
            ProjectDamage {
                picture: OutputDamage::Ranges {
                    ranges: vec![[4, 6]],
                },
                audio: OutputDamage::None,
            },
        )?;
        new.inherit_frame_cache(&old.frame_cache(), &change)?;
        let stats = new.frame_cache().stats();
        assert_eq!((stats.inherited, stats.hits, stats.misses), (1, 0, 0));
        new.seek(3)?;
        let clean = wait(&new)?;
        assert!(Arc::ptr_eq(&clean.rgba, &prior.rgba));
        assert!(clean.trace.reused_composite);
        assert_eq!(
            (clean.trace.revision, clean.trace.request.shot_frame),
            (8, 1)
        );
        assert_eq!(clean.trace.graphics.temporal_samples, 0);
        assert_eq!(new.frame_cache().stats().hits, 1);
        new.seek(4)?;
        let changed = wait(&new)?;
        assert!(!changed.trace.reused_composite);
        assert_ne!(changed.rgba, dirty.rgba);
        assert_eq!(&changed.rgba[..4], &[255, 255, 0, 255]);
        assert_eq!(new.frame_cache().stats().misses, 1);
        new.play_from(3)?;
        assert!(wait(&new)?.trace.reused_composite);
        assert!(wait(&new)?.trace.reused_composite);
        Ok(())
    }
}
