//! Path-traced Final for a spatial group, through Cycles when this build has it.
//! The request is one evaluated instant; nothing here reads the timeline.
use crate::graphics::{FinalPicture, FinalRequest};
use anyhow::Result;
use nothing_spatial::Mesh;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) type Meshes = HashMap<(PathBuf, String), Arc<Mesh>>;

/// Why a path-traced scene cannot be finished in this build, or `None` when it can.
pub(crate) fn unavailable() -> Option<&'static str> {
    #[cfg(feature = "cycles")]
    {
        (!nothing_spatial_cycles::available()).then_some(
            "this build has no Cycles bundle; run spikes/spatial/cycles/tools/provision.sh and rebuild",
        )
    }
    #[cfg(not(feature = "cycles"))]
    {
        Some("this build has no path tracer; rebuild n4-engine with --features cycles")
    }
}

/// N4's display transform, as in the rasterizer's film pass: exposure, identity below 0.7, a
/// continuous shoulder above. Returns the bytes of a premultiplied sRGB group texture.
#[cfg_attr(not(feature = "cycles"), allow(dead_code))]
pub(crate) fn finish(linear_premultiplied: &[f32], exposure_ev: f32) -> Vec<u8> {
    let gain = exposure_ev.exp2();
    let encode = |c: f32| {
        let c = c.clamp(0., 1.);
        let e = if c <= 0.003_130_8 {
            12.92 * c
        } else {
            1.055 * c.powf(1. / 2.4) - 0.055
        };
        (e * 255.).round() as u8
    };
    let mut out = Vec::with_capacity(linear_premultiplied.len());
    for p in linear_premultiplied.chunks_exact(4) {
        let a = p[3].clamp(0., 1.);
        for &c in &p[..3] {
            let straight = (c / a.max(1e-7)).max(0.) * gain;
            let excess = (straight - 0.7).max(0.);
            out.push(encode(
                (straight.min(0.7) + 0.3 * excess / (0.3 + excess)) * a,
            ));
        }
        out.push((a * 255.).round() as u8);
    }
    out
}

#[cfg(feature = "cycles")]
mod cycles {
    use super::*;
    use crate::graphics::{FinalModel, SurfaceRole};
    use anyhow::{Context, ensure};
    use nothing_spatial_cycles::{
        Artwork, ImageId, LightId, MaterialId, ObjectId, Recipe, Session, pose,
    };
    use std::hash::{Hash, Hasher};

    struct Model {
        materials: Vec<MaterialId>,
        objects: Vec<ObjectId>,
        images: Vec<ImageId>,
    }

    struct Scene {
        signature: u64,
        session: Session,
        models: Vec<Model>,
        lights: Vec<LightId>,
    }

    /// Live Cycles sessions, one per set of models and surface roles. Geometry and materials are
    /// uploaded once; a request then moves parts, swaps artwork and renders.
    #[derive(Default)]
    pub(crate) struct PathTracer {
        scenes: Vec<Scene>,
    }

    /// What must be rebuilt rather than updated when it changes.
    fn signature(request: &FinalRequest, meshes: &Meshes) -> u64 {
        let mut h = crate::graphics::StableHasher::new();
        for model in &request.models {
            model.mesh.hash(&mut h);
            meshes
                .get(&model.mesh)
                .map(|m| Arc::as_ptr(m) as usize)
                .hash(&mut h);
            for s in &model.surfaces {
                s.slot.hash(&mut h);
                (s.role as u8).hash(&mut h);
                s.emission.to_bits().hash(&mut h);
                s.glow.to_bits().hash(&mut h);
            }
        }
        request.rects.len().hash(&mut h);
        h.finish()
    }

    /// Live surfaces are premultiplied sRGB; Cycles takes unassociated pixels.
    fn unassociated(mut rgba: Vec<u8>) -> Vec<u8> {
        let decode = |v: u8| {
            let x = f32::from(v) / 255.;
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        for p in rgba.chunks_exact_mut(4) {
            if p[3] == 0 || p[3] == 255 {
                continue;
            }
            let a = f32::from(p[3]) / 255.;
            for c in &mut p[..3] {
                let straight = (decode(*c) / a).clamp(0., 1.);
                let e = if straight <= 0.003_130_8 {
                    12.92 * straight
                } else {
                    1.055 * straight.powf(1. / 2.4) - 0.055
                };
                *c = (e * 255.).round() as u8;
            }
        }
        rgba
    }

    impl Scene {
        fn build(request: &FinalRequest, meshes: &Meshes, signature: u64) -> Result<Self> {
            let device = std::env::var("N4_CYCLES_DEVICE").unwrap_or_else(|_| "METAL".into());
            let mut session = Session::new(&device, 0)?;
            let mut models = Vec::new();
            for (index, model) in request.models.iter().enumerate() {
                let mesh = meshes
                    .get(&model.mesh)
                    .context("mesh for a path-traced model")?;
                let mut images = Vec::new();
                for _ in &model.surfaces {
                    images.push(session.image([2, 2], true, &[0; 16])?);
                }
                let artwork = |slot: &str| {
                    let Some(i) = model.surfaces.iter().position(|s| s.slot == slot) else {
                        return Artwork::None;
                    };
                    let s = &model.surfaces[i];
                    match s.role {
                        SurfaceRole::BaseColor => Artwork::BaseColor(images[i]),
                        SurfaceRole::Emissive => Artwork::Emission {
                            image: images[i],
                            scale: s.emission,
                            glow: s.glow,
                        },
                        // Shown as authored, lighting nothing.
                        SurfaceRole::Unlit => Artwork::Emission {
                            image: images[i],
                            scale: 1.,
                            glow: 0.,
                        },
                    }
                };
                let materials = session.materials(mesh, &artwork)?;
                let objects = session.model(mesh, &materials, index as u32 + 1)?;
                models.push(Model {
                    materials,
                    objects,
                    images,
                });
            }
            let lights = request
                .rects
                .iter()
                .map(|r| session.rect_light(r.center, r.edge_u, r.edge_v, r.radiance, false))
                .collect();
            Ok(Self {
                signature,
                session,
                models,
                lights,
            })
        }

        fn place(&mut self, model: &FinalModel, state: usize, mesh: &Mesh) {
            let instance = pose::rigid(model.position, model.rotation_degrees);
            let overrides: Vec<(&str, [f32; 3])> =
                model.parts.iter().map(|(p, r)| (p.as_str(), *r)).collect();
            let world = pose::part_transforms(mesh, &instance, &overrides);
            for (object, m) in self.models[state].objects.iter().zip(&world) {
                self.session.transform(*object, m);
            }
        }
    }

    impl PathTracer {
        /// `artwork[m][s]` holds the read-back pixels of model m's surface s.
        pub(crate) fn render(
            &mut self,
            request: &FinalRequest,
            meshes: &Meshes,
            artwork: Vec<Vec<Vec<u8>>>,
        ) -> Result<FinalPicture> {
            let signature = signature(request, meshes);
            let index = match self.scenes.iter().position(|s| s.signature == signature) {
                Some(index) => index,
                None => {
                    // Each session holds device memory; keep the two most recent scenes.
                    if self.scenes.len() >= 2 {
                        self.scenes.remove(0);
                    }
                    self.scenes.push(Scene::build(request, meshes, signature)?);
                    self.scenes.len() - 1
                }
            };
            let scene = &mut self.scenes[index];
            for (m, (model, pixels)) in request.models.iter().zip(artwork).enumerate() {
                let mesh = meshes
                    .get(&model.mesh)
                    .context("mesh for a path-traced model")?;
                scene.place(model, m, mesh);
                for (s, (surface, rgba)) in model.surfaces.iter().zip(pixels).enumerate() {
                    let size = [surface.texture.width(), surface.texture.height()];
                    scene.session.update_image(
                        scene.models[m].images[s],
                        size,
                        &unassociated(rgba),
                    )?;
                }
                let _ = &scene.models[m].materials;
            }
            for (light, r) in scene.lights.iter().zip(&request.rects) {
                scene
                    .session
                    .update_rect_light(*light, r.center, r.edge_u, r.edge_v, r.radiance);
            }
            scene.session.background(request.ambient, true);
            let [x, y, z] = pose::axes(request.camera_rotation_degrees);
            scene.session.camera(
                request.camera_position,
                x,
                y,
                z.map(|v| -v),
                request.fov_y_degrees,
            );
            let ids: Vec<u32> = request
                .mattes
                .iter()
                .map(|object| {
                    request
                        .models
                        .iter()
                        .position(|m| m.id.as_deref() == Some(object))
                        .map_or(u32::MAX, |i| i as u32 + 1)
                })
                .collect();
            let recipe = Recipe {
                samples: request.recipe.samples,
                max_bounces: request.recipe.bounces,
                clamp_indirect: request.recipe.clamp.into(),
                denoise: request.recipe.denoise,
                // A frame's sample pattern follows from its identity, never from render order.
                seed: (request.recipe.seed ^ request.key.0 as u32) & 0x7fff_ffff,
            };
            let frame = scene.session.render(request.size, &recipe, &ids)?;
            ensure!(
                frame.color.len() == (request.size[0] * request.size[1] * 4) as usize,
                "Cycles returned an unexpected picture size"
            );
            Ok(FinalPicture {
                size: request.size,
                rgba: finish(&frame.color, request.exposure_ev),
                mattes: frame
                    .masks
                    .iter()
                    .map(|m| {
                        m.iter()
                            .map(|&v| (v.clamp(0., 1.) * 255.).round() as u8)
                            .collect()
                    })
                    .collect(),
            })
        }
    }
}

#[cfg(feature = "cycles")]
pub(crate) use cycles::PathTracer;

#[cfg(not(feature = "cycles"))]
#[derive(Default)]
pub(crate) struct PathTracer;

#[cfg(not(feature = "cycles"))]
impl PathTracer {
    pub(crate) fn render(
        &mut self,
        _: &FinalRequest,
        _: &Meshes,
        _: Vec<Vec<Vec<u8>>>,
    ) -> Result<FinalPicture> {
        anyhow::bail!("{}", unavailable().unwrap())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_cpu_finish_matches_the_film_curve() {
        // Opaque: 0.5 is below the knee; 1.7 is 0.7 + 0.3 * 1.0 / 1.3.
        let bytes = super::finish(&[0.5, 1.7, 0., 1., 0.25, 0.25, 0.25, 0.5], 0.);
        assert_eq!(bytes[0], 188);
        assert_eq!(
            bytes[1],
            (255. * (1.055 * (0.7f32 + 0.3 / 1.3).powf(1. / 2.4) - 0.055)).round() as u8
        );
        assert_eq!(&bytes[2..4], [0, 255]);
        // Half coverage of straight 0.5 stays premultiplied 0.25.
        assert_eq!(bytes[4], 137);
        assert_eq!(bytes[7], 128);
        // One stop up doubles the straight value before the curve.
        assert_eq!(super::finish(&[0.25, 0., 0., 1.], 1.)[0], 188);
    }
}
