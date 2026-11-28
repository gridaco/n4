//! Immutable rendered material inputs retained by last use across seeks and placements.
use super::{LiveSurface, SampleTime, TRANSIENT_TEXTURE_BYTES, TRANSIENT_TEXTURE_ENTRIES};
use std::{collections::VecDeque, sync::Arc};

#[derive(Default)]
pub(crate) struct LiveSurfaceCache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

struct Entry {
    surface: Arc<LiveSurface>,
    local_frame: u32,
    sample: SampleTime,
    view: wgpu::TextureView,
    bytes: usize,
}

impl LiveSurfaceCache {
    pub(crate) fn get(
        &mut self,
        surface: &LiveSurface,
        frame: u32,
        time: SampleTime,
    ) -> Option<wgpu::TextureView> {
        let index = self.entries.iter().rposition(|entry| {
            entry.local_frame == frame && entry.sample == time && entry.surface.as_ref() == surface
        })?;
        let entry = self.entries.remove(index)?;
        let view = entry.view.clone();
        self.entries.push_back(entry);
        Some(view)
    }

    pub(crate) fn insert(
        &mut self,
        surface: &LiveSurface,
        frame: u32,
        time: SampleTime,
        view: wgpu::TextureView,
    ) {
        // N4's live output is single-sampled RGBA8, with one mip and array layer.
        let texture = view.texture();
        let bytes = texture.width() as usize * texture.height() as usize * 4;
        if bytes > TRANSIENT_TEXTURE_BYTES {
            return;
        }
        let surface = self
            .entries
            .iter()
            .rev()
            .find(|entry| entry.surface.as_ref() == surface)
            .map(|entry| Arc::clone(&entry.surface))
            .unwrap_or_else(|| Arc::new(surface.clone()));
        while self.entries.len() >= TRANSIENT_TEXTURE_ENTRIES
            || self.bytes + bytes > TRANSIENT_TEXTURE_BYTES
        {
            let entry = self
                .entries
                .pop_front()
                .expect("occupied live surface cache");
            self.bytes -= entry.bytes;
        }
        self.entries.push_back(Entry {
            surface,
            local_frame: frame,
            sample: time,
            view,
            bytes,
        });
        self.bytes += bytes;
    }

    pub(super) fn retain(&mut self, keep: impl Fn(&LiveSurface) -> bool) {
        self.entries.retain(|entry| keep(&entry.surface));
        self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::SurfaceRole;
    use anyhow::{Context, Result};
    #[cfg(all(n4_openusd, feature = "native-decode"))]
    #[test]
    fn changing_fps_discards_prepared_live_video_samples() -> Result<()> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/field-unit");
        let mut project = crate::Project::load(&root)?;
        project.size = [384, 216];
        project.audio.clear(); // Isolate video sampling from the six-second soundtrack.
        let mut engine = crate::Engine::new(project.clone())?;
        let _ = engine.render(67)?;
        assert!(!engine.graphics.live_surfaces.entries.is_empty());

        project.fps = crate::project::FrameRate(30, 1);
        engine.replace_project(project.clone())?;
        assert!(engine.graphics.live_surfaces.entries.is_empty());
        let after = engine.render(67)?;
        assert_eq!(after.rgba, crate::Engine::new(project)?.render(67)?.rgba);
        Ok(())
    }

    #[test]
    fn live_surfaces_bound_entries_and_bytes_and_keep_active_views_valid() -> Result<()> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .context("request render device")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        let view = |size: u32| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("live sample"),
                    size: wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    sample_count: 1,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let surface = |n| LiveSurface {
            slot: "/Display".into(),
            role: SurfaceRole::Unlit,
            size: [n, n],
            layers: vec![],
            emission: 1f32.try_into().unwrap(),
            glow: 1f32.try_into().unwrap(),
        };
        let time = |f| SampleTime::at(f, None, 0);
        let mut cache = LiveSurfaceCache::default();
        let s = surface(1);
        for f in 0..64 {
            cache.insert(&s, f, time(f), view(1));
        }
        let active = cache.get(&s, 0, time(0)).unwrap();
        cache.insert(&s, 64, time(64), view(1));
        assert!(
            cache.get(&s, 1, time(1)).is_none(),
            "least recently used entry was evicted"
        );
        assert!(cache.get(&s, 0, time(0)).is_some());
        assert!(
            cache.get(&s, 0, time(10)).is_none(),
            "owner time participates in the key"
        );
        assert_eq!(cache.entries.len(), 64);
        let large = surface(1024);
        for f in 0..9 {
            cache.insert(&large, f, time(f), view(1024));
        }
        assert_eq!(cache.bytes, 32 << 20);
        assert_eq!(cache.entries.len(), 8);
        assert!(cache.get(&large, 0, time(0)).is_none());
        assert!(cache.get(&s, 0, time(0)).is_none());
        let huge = surface(3072);
        cache.insert(&huge, 0, time(0), view(3072));
        assert!(cache.get(&huge, 0, time(0)).is_none());
        cache.retain(|item| item != &large);
        assert_eq!((cache.entries.len(), cache.bytes), (0, 0));
        // Releasing the cache's handle leaves an already returned view valid on the GPU.
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &active,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        let id = queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(id),
            timeout: None,
        })?;
        Ok(())
    }
}
