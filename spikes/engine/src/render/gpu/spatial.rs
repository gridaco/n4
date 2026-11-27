//! Sample the N4 timeline, prepare its artwork and submit one spatial group to the compositor.
use super::{Gpu, ScenePainter};
use crate::graphics::{LiveSurface, ObjectMatte, Scene3d, SurfaceRole};
use anyhow::{Context, Result};
pub(super) use nothing_spatial_wgpu::matte_scratch_bytes;
use nothing_spatial_wgpu::{
    Geometry, Light, Matte, MatteOutput, MeshBuffers, Object, PartPose, Pose, Renderer, Studio,
    SurfaceBinding, SurfaceMode, View,
};
use std::borrow::Cow;

pub(super) struct SpatialPass {
    renderer: Renderer,
    output: wgpu::Texture,
    mattes: Option<wgpu::Texture>,
}

impl SpatialPass {
    fn new(gpu: &Gpu, size: wgpu::Extent3d) -> Self {
        Self {
            renderer: Renderer::new(&gpu.device),
            output: gpu.render_texture("resolved spatial group", size),
            mattes: None,
        }
    }

    fn targets(&mut self, gpu: &Gpu, size: wgpu::Extent3d, matte_count: u32) {
        if self.output.size() != size {
            self.output = gpu.render_texture("resolved spatial group", size);
        }
        if matte_count == 0 {
            return;
        }
        let size = wgpu::Extent3d {
            depth_or_array_layers: matte_count,
            ..size
        };
        if self
            .mattes
            .as_ref()
            .is_none_or(|target| target.size() != size)
        {
            self.mattes = Some(gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("resolved spatial object mattes"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: nothing_spatial_wgpu::MATTE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
        }
    }
}

fn linear(value: u8) -> f32 {
    let x = f32::from(value) / 255.;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

impl ScenePainter<'_> {
    fn live_surface(&mut self, surface: &LiveSurface, frame: u32) -> Result<wgpu::TextureView> {
        if let Some(view) = self.cache.live_surfaces.get(surface, frame, self.time) {
            return Ok(view);
        }
        // A prior view can still be bound by a recorded pass or another placement.
        // A different sample gets its own immutable result until its last user drops it.
        let texture = self.gpu.render_texture(
            "live material artwork",
            wgpu::Extent3d {
                width: surface.size[0],
                height: surface.size[1],
                depth_or_array_layers: 1,
            },
        );
        Gpu::clear(self.commands, &texture, wgpu::Color::TRANSPARENT);
        self.layers(&surface.layers, &texture, [0, 0], None, frame)?;
        let view = texture.create_view(&Default::default());
        self.cache
            .live_surfaces
            .insert(surface, frame, self.time, view.clone());
        Ok(view)
    }

    pub(super) fn spatial_source(
        &mut self,
        scene: &Scene3d,
        target: &wgpu::Texture,
        frame: u32,
        mattes: &[ObjectMatte],
    ) -> Result<(wgpu::Texture, Option<wgpu::Texture>)> {
        let camera = self
            .perspective
            .context("a scene3d requires a selected perspective camera")?;
        let gpu = self.gpu;
        let mut planes = Vec::new();
        for plane in &scene.planes {
            let label = plane.text.as_ref();
            let Some(local) = label.local_frame(frame) else {
                continue;
            };
            let Some(content) = label.content_at(local, self.time.fraction()) else {
                continue;
            };
            let anim = (!label.text_animators.is_empty()).then_some((
                label.text_animators.as_slice(),
                local,
                self.time.fraction(),
            ));
            let (art, offset) = if anim.is_some() || matches!(content, Cow::Owned(_)) {
                self.sampled_texture(&content, anim)?
            } else {
                self.raster_texture(&content)?
            };
            let px = f32::from(scene.pixels_per_meter);
            let mut p = plane.position.map(f32::from);
            p[0] += (offset[0] as f32 + label.position[0] as f32) / px;
            p[1] -= (offset[1] as f32 + label.position[1] as f32) / px;
            let pose = Pose::plane(
                p,
                [art.width() as f32 / px, 0., 0.],
                [0., -(art.height() as f32) / px, 0.],
            );
            planes.push((
                plane.id.as_deref(),
                art.create_view(&Default::default()),
                pose,
            ));
        }

        let mut models = Vec::with_capacity(scene.models.len());
        for model in &scene.models {
            let key = (model.source.clone(), model.prim.clone());
            if !self.cache.mesh_buffers.contains_key(&key) {
                let mesh = self.cache.meshes.get(&key).with_context(|| {
                    format!(
                        "mesh {} from {} was not prepared before drawing",
                        key.1,
                        key.0.display()
                    )
                })?;
                let buffers = MeshBuffers::upload(&gpu.device, &gpu.queue, mesh)?;
                self.cache.mesh_buffers.insert(key.clone(), buffers);
                self.stats.mesh_uploads += 1;
            }
            let rotation = [&model.rotation_x, &model.rotation_y, &model.rotation_z]
                .map(|channel| f32::from(channel.at(frame, self.time.fraction())));
            let mut surfaces = Vec::with_capacity(model.surfaces.len());
            for surface in &model.surfaces {
                let texture = self.live_surface(surface, frame)?;
                let mode = match surface.role {
                    SurfaceRole::Unlit => SurfaceMode::Unlit,
                    SurfaceRole::BaseColor => SurfaceMode::BaseColor,
                };
                surfaces.push((surface.slot.as_str(), texture, mode));
            }
            let parts: Vec<_> = model
                .parts
                .iter()
                .map(|p| PartPose {
                    path: &p.prim,
                    rotation_degrees: [&p.rotation_x, &p.rotation_y, &p.rotation_z]
                        .map(|c| f32::from(c.at(frame, self.time.fraction()))),
                })
                .collect();
            models.push((
                model.id.as_deref(),
                key,
                Pose::rigid(model.position.map(f32::from), rotation),
                surfaces,
                parts,
            ));
        }
        let bindings: Vec<Vec<_>> = models
            .iter()
            .map(|(_, _, _, surfaces, _)| {
                surfaces
                    .iter()
                    .map(|(slot, texture, mode)| SurfaceBinding {
                        slot,
                        texture,
                        mode: *mode,
                    })
                    .collect()
            })
            .collect();
        let mut objects = Vec::with_capacity(models.len() + planes.len());
        for ((id, key, pose, _, parts), surfaces) in models.iter().zip(&bindings) {
            let mesh = &self.cache.mesh_buffers[key];
            let geometry = if scene.studio.is_some() || !surfaces.is_empty() || !parts.is_empty() {
                Geometry::Model {
                    mesh,
                    surfaces,
                    parts,
                }
            } else {
                Geometry::Mesh(mesh)
            };
            objects.push(Object {
                id: *id,
                pose: *pose,
                geometry,
            });
        }
        for (id, texture, pose) in &planes {
            objects.push(Object {
                id: *id,
                pose: *pose,
                geometry: Geometry::Plane(texture),
            });
        }

        let view = View {
            position: camera.position,
            rotation_degrees: camera.rotation_degrees,
            fov_y_degrees: camera.fov_y_degrees,
        };
        let lights: Vec<_> = scene
            .lights
            .iter()
            .map(|light| Light {
                direction: light.direction.map(f32::from),
                radiance: light.color.map(|v| linear(v) * f32::from(light.intensity)),
            })
            .collect();
        let requests: Vec<_> = mattes
            .iter()
            .map(|matte| Matte {
                object: &matte.object,
                owner: matte.owner(),
            })
            .collect();
        let matte_bytes = matte_scratch_bytes(
            [target.width(), target.height()],
            requests.len(),
            requests.iter().any(|m| m.owner.is_some()),
        )?;
        let mut state = gpu.spatial.borrow_mut();
        let pass = state.get_or_insert_with(|| SpatialPass::new(gpu, target.size()));
        pass.targets(gpu, target.size(), requests.len() as u32);
        let resolved_mattes = if requests.is_empty() {
            None
        } else {
            pass.mattes.as_ref()
        };
        let matte_output = resolved_mattes.map(|target| MatteOutput {
            target,
            requests: &requests,
        });
        if let Some(look) = &scene.studio {
            let sample = |channel: &crate::render::channels::AnimatedScalar<
                crate::render::channels::Scalar,
            >| f32::from(channel.at(frame, self.time.fraction()));
            let studio = Studio {
                environment_rotation_degrees: sample(&look.environment_rotation),
                environment_intensity: sample(&look.environment_intensity),
                exposure_ev: sample(&look.exposure),
                shadow_center: look.shadow_center.map(f32::from),
                shadow_extent: look.shadow_extent.into(),
                shadow_bias: look.shadow_bias.into(),
                shadow_softness: look.shadow_softness.into(),
            };
            pass.renderer.record_studio(
                self.commands,
                &pass.output,
                view,
                &objects,
                &lights,
                matte_output,
                &studio,
            )?;
        } else {
            pass.renderer.record(
                self.commands,
                &pass.output,
                view,
                &objects,
                &lights,
                matte_output,
            )?;
        }
        self.stats.spatial_passes += 1;
        self.stats.object_matte_passes += requests.len() as u32;
        self.stats.object_matte_scratch_bytes =
            self.stats.object_matte_scratch_bytes.max(matte_bytes);
        Ok((pass.output.clone(), resolved_mattes.cloned()))
    }
}

#[cfg(test)]
mod tests;
