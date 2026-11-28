//! Sample the N4 timeline, prepare its artwork and submit one spatial group to the compositor.
use super::{Gpu, ScenePainter};
use crate::graphics::{
    FinalKey, FinalModel, FinalPicture, FinalRect, FinalRequest, FinalSurface, LiveSurface,
    MatteCoverage, ObjectMatte, Scene3d, StableHasher, SurfaceRole,
};
use anyhow::{Context, Result, ensure};
pub(super) use nothing_spatial_wgpu::matte_scratch_bytes;
use nothing_spatial_wgpu::{
    Geometry, Light, Matte, MatteOutput, MeshBuffers, Object, PartPose, Pose, RectLight, Renderer,
    Rig, Studio, SurfaceBinding, SurfaceMode, View,
};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};

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

/// Authored rect lights as the renderers take them: a centre and two full edge vectors whose cross
/// product is the emission direction. The long edge is horizontal.
pub(crate) fn rect_lights(scene: &Scene3d) -> Vec<RectLight> {
    scene
        .rect_lights
        .iter()
        .map(|light| {
            let center = light.center.map(f32::from);
            let aim = light.aim.map(f32::from);
            let n = unit(std::array::from_fn(|i| aim[i] - center[i]));
            let up = if n[1].abs() > 0.999 {
                [1., 0., 0.]
            } else {
                [0., 1., 0.]
            };
            let u = unit(cross(n, up));
            let v = cross(n, u);
            let [w, h] = light.size.map(f32::from);
            RectLight {
                center,
                edge_u: u.map(|x| x * w),
                edge_v: v.map(|x| x * h),
                radiance: light.color.map(|c| linear(c) * f32::from(light.intensity)),
            }
        })
        .collect()
}

/// The resolved group and matte array for a finished picture, as fresh textures.
fn final_textures(gpu: &Gpu, picture: &FinalPicture) -> (wgpu::Texture, Option<wgpu::Texture>) {
    let [width, height] = picture.size;
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let color = gpu.render_texture("path-traced spatial group", size);
    gpu.queue.write_texture(
        color.as_image_copy(),
        &picture.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: None,
        },
        size,
    );
    if picture.mattes.is_empty() {
        return (color, None);
    }
    let layers = wgpu::Extent3d {
        depth_or_array_layers: picture.mattes.len() as u32,
        ..size
    };
    let mattes = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("path-traced object mattes"),
        size: layers,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: nothing_spatial_wgpu::MATTE_FORMAT,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let bytes: Vec<u8> = picture.mattes.concat();
    gpu.queue.write_texture(
        mattes.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        },
        layers,
    );
    (color, Some(mattes))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    a.map(|x| x / l)
}

impl ScenePainter<'_> {
    /// The identity of a path-traced picture: the authored scene, the instant, the view, the
    /// output size, the geometry's bytes and the mattes asked of it. Edits elsewhere in the
    /// project leave it unchanged.
    fn final_key(
        &mut self,
        scene: &Scene3d,
        target: &wgpu::Texture,
        frame: u32,
        mattes: &[ObjectMatte],
        camera: crate::camera::PerspectiveView,
    ) -> Result<FinalKey> {
        ensure!(
            mattes
                .iter()
                .all(|m| m.coverage == MatteCoverage::Visible && m.occluded_by.is_none()),
            "a path-traced scene supplies visible-coverage mattes; projected and occluded-by mattes are not path traced yet"
        );
        // The scene as authored, with every file it names replaced by a digest of the file's
        // bytes: a picture does not depend on where a project is or how its path was spelled,
        // and it does depend on footage or an image replaced under the same name. Geometry is
        // keyed by what the importer produced, below.
        let mut authored = serde_json::to_value(scene)?;
        let mut clocked = false;
        for model in authored["models"].as_array_mut().into_iter().flatten() {
            model["source"] = serde_json::Value::Null;
            identify_files(
                &mut model["surfaces"],
                &mut self.cache.file_digests,
                &mut clocked,
            )?;
        }
        let mut hasher = StableHasher::new();
        hasher.write(serde_json::to_string(&authored)?.as_bytes());
        frame.hash(&mut hasher);
        // Video, a wipe and a periodic shader are sampled on the owning shot's clock, which a
        // slide or a trim moves against the scene's own frame.
        if clocked {
            self.time.frame.hash(&mut hasher);
        }
        self.time.fraction().to_bits().hash(&mut hasher);
        [target.width(), target.height()].hash(&mut hasher);
        for value in camera
            .position
            .into_iter()
            .chain(camera.rotation_degrees)
            .chain([camera.fov_y_degrees])
        {
            value.to_bits().hash(&mut hasher);
        }
        mattes.len().hash(&mut hasher);
        for matte in mattes {
            matte.object.hash(&mut hasher);
        }
        // Devices agree to within noise, not exactly.
        crate::render::path_trace::device().hash(&mut hasher);
        for model in &scene.models {
            let key = (model.source.clone(), model.prim.clone());
            let mesh = self
                .cache
                .meshes
                .get(&key)
                .context("mesh was not prepared before drawing")?;
            let content = match self.cache.mesh_hashes.get(&key) {
                Some((seen, content)) if std::sync::Arc::ptr_eq(seen, mesh) => *content,
                _ => {
                    let content = mesh_content(mesh);
                    self.cache
                        .mesh_hashes
                        .insert(key, (std::sync::Arc::clone(mesh), content));
                    content
                }
            };
            content.hash(&mut hasher);
        }
        Ok(FinalKey(hasher.finish()))
    }

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
        // A path-traced scene is its finished Final when one exists, and a Draft otherwise.
        let final_key = match &scene.path_trace {
            Some(_) => Some(self.final_key(scene, target, frame, mattes, camera)?),
            None => None,
        };
        if let Some(picture) = final_key.and_then(|key| {
            self.cache
                .finals
                .get(key, [target.width(), target.height()], mattes.len())
        }) {
            self.stats.path_traced_final += 1;
            return Ok(final_textures(gpu, &picture));
        }
        let mut final_models = Vec::new();
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
                // The rasterizer shows a screen as authored. Its light on the scene is Final's work.
                let mode = match surface.role {
                    SurfaceRole::Unlit | SurfaceRole::Emissive => SurfaceMode::Unlit,
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
            if final_key.is_some() {
                final_models.push(FinalModel {
                    id: model.id.clone(),
                    mesh: key.clone(),
                    content: self.cache.mesh_hashes.get(&key).map_or(0, |m| m.1),
                    position: model.position.map(f32::from),
                    rotation_degrees: rotation,
                    parts: parts
                        .iter()
                        .map(|p| (p.path.to_string(), p.rotation_degrees))
                        .collect(),
                    surfaces: model
                        .surfaces
                        .iter()
                        .zip(&surfaces)
                        .map(|(surface, (_, view, _))| FinalSurface {
                            slot: surface.slot.clone(),
                            texture: view.texture().clone(),
                            role: surface.role,
                            emission: surface.emission.into(),
                            glow: surface.glow.into(),
                        })
                        .collect(),
                });
            }
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
            let geometry = if scene.studio.is_some()
                || !scene.rect_lights.is_empty()
                || !surfaces.is_empty()
                || !parts.is_empty()
            {
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
        let rects = rect_lights(scene);
        let rig = (!rects.is_empty()).then(|| Rig {
            rects: &rects,
            ambient: scene.ambient.as_ref().map_or([0.; 3], |a| {
                a.color.map(|v| linear(v) * f32::from(a.intensity))
            }),
        });
        let sample = |channel: &crate::render::channels::AnimatedScalar<
            crate::render::channels::Scalar,
        >| f32::from(channel.at(frame, self.time.fraction()));
        if let (Some(key), Some(recipe), Some(rig)) = (final_key, &scene.path_trace, &rig) {
            self.stats.path_traced_draft += 1;
            self.cache.finals.want(FinalRequest {
                key,
                size: [target.width(), target.height()],
                camera_position: view.position,
                camera_rotation_degrees: view.rotation_degrees,
                fov_y_degrees: view.fov_y_degrees,
                models: std::mem::take(&mut final_models),
                rects: rects
                    .iter()
                    .map(|r| FinalRect {
                        center: r.center,
                        edge_u: r.edge_u,
                        edge_v: r.edge_v,
                        radiance: r.radiance,
                    })
                    .collect(),
                ambient: rig.ambient,
                exposure_ev: scene
                    .studio
                    .as_ref()
                    .map_or(0., |look| sample(&look.exposure)),
                recipe: recipe.clone(),
                mattes: mattes.iter().map(|m| m.object.clone()).collect(),
            });
        }
        if scene.studio.is_some() || rig.is_some() {
            let studio = scene
                .studio
                .as_ref()
                .map_or_else(Studio::default, |look| Studio {
                    environment_rotation_degrees: sample(&look.environment_rotation),
                    environment_intensity: sample(&look.environment_intensity),
                    exposure_ev: sample(&look.exposure),
                    shadow_center: look.shadow_center.map(f32::from),
                    shadow_extent: look.shadow_extent.into(),
                    shadow_bias: look.shadow_bias.into(),
                    shadow_softness: look.shadow_softness.into(),
                });
            pass.renderer.record_studio_rig(
                self.commands,
                &pass.output,
                view,
                &objects,
                &lights,
                matte_output,
                &studio,
                rig.as_ref(),
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

/// Everything of an imported mesh that a renderer reads.
pub(crate) fn mesh_content(mesh: &nothing_spatial::Mesh) -> u64 {
    let mut h = StableHasher::new();
    mesh.vertices.len().hash(&mut h);
    for v in &mesh.vertices {
        for f in v.position.into_iter().chain(v.normal).chain(v.uv) {
            h.write_u32(f.to_bits());
        }
        h.write_u32(v.material);
    }
    mesh.indices.hash(&mut h);
    // Their debug form names every field, including ones added later.
    format!("{:?}{:?}", mesh.materials, mesh.parts).hash(&mut h);
    mesh.images.len().hash(&mut h);
    for image in &mesh.images {
        [image.width, image.height].hash(&mut h);
        image.rgba.hash(&mut h);
    }
    h.finish()
}

/// A digest of a file's bytes, remembered until its size or modification time changes.
pub(crate) type FileDigests =
    std::collections::HashMap<std::path::PathBuf, (u64, Option<std::time::SystemTime>, u64)>;

/// Replace every string in `value` that names a file with a digest of that file, and report
/// whether the layers sample the shot's clock rather than only their own frame.
fn identify_files(
    value: &mut serde_json::Value,
    digests: &mut FileDigests,
    clocked: &mut bool,
) -> Result<()> {
    use serde_json::Value;
    match value {
        Value::String(text) => {
            let path = std::path::Path::new(text.as_str());
            let Ok(file) = std::fs::metadata(path) else {
                return Ok(());
            };
            if !file.is_file() {
                return Ok(());
            }
            let stamp = (file.len(), file.modified().ok());
            let digest = match digests.get(path) {
                Some(&(len, modified, digest)) if (len, modified) == stamp => digest,
                _ => {
                    use std::io::Read;
                    let mut h = StableHasher::new();
                    let mut reader = std::fs::File::open(path)
                        .with_context(|| format!("read {}", path.display()))?;
                    let mut block = vec![0; 1 << 20];
                    loop {
                        let n = reader.read(&mut block)?;
                        if n == 0 {
                            break;
                        }
                        h.write(&block[..n]);
                    }
                    let digest = h.finish();
                    digests.insert(path.to_path_buf(), (stamp.0, stamp.1, digest));
                    digest
                }
            };
            *text = format!("file:{digest:016x}");
        }
        Value::Array(items) => {
            for item in items {
                identify_files(item, digests, clocked)?;
            }
        }
        Value::Object(fields) => {
            let kind = fields.get("type").and_then(Value::as_str);
            ensure!(
                !matches!(kind, Some("footage" | "backdrop")),
                "a path-traced surface cannot show the shot's footage or a backdrop yet"
            );
            *clocked |= kind == Some("video")
                || fields.contains_key("wipe")
                || fields.contains_key("period_frames");
            for item in fields.values_mut() {
                identify_files(item, digests, clocked)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
