//! A long-lived Cycles session fed from `nothing-spatial` data.
//! The crate builds without Cycles; `available()` says whether this build can render.
//! Coordinates are right-handed, Y-up, meters. Colour out is scene-linear, premultiplied, top row first.
use anyhow::{Result, bail, ensure};
use nothing_spatial::{Mesh, TextureChannel};
use std::ffi::{CStr, CString};

mod sys {
    use std::ffi::c_char;
    #[repr(C)]
    pub struct N4Cycles {
        _private: [u8; 0],
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct N4Material {
        pub base_color: [f32; 3],
        pub metallic: f32,
        pub roughness: f32,
        pub specular_ior_level: f32,
        pub coat_weight: f32,
        pub coat_roughness: f32,
        pub emission: [f32; 3],
        pub base_image: i32,
        pub normal_image: i32,
        pub roughness_image: i32,
        pub roughness_channel: i32,
        pub metallic_image: i32,
        pub metallic_channel: i32,
        pub artwork_image: i32,
        pub artwork_role: i32,
        pub emission_scale: f32,
        pub glow_scale: f32,
    }
    #[repr(C)]
    pub struct N4Render {
        pub width: i32,
        pub height: i32,
        pub samples: i32,
        pub max_bounces: i32,
        pub clamp_indirect: f32,
        pub denoise: i32,
        pub seed: i32,
        pub mask_count: i32,
        pub mask_pass_ids: *const i32,
        pub depth: i32,
        pub cryptomatte: i32,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy, Debug)]
    pub struct N4Timing {
        pub sync_seconds: f64,
        pub render_seconds: f64,
        pub total_seconds: f64,
        pub samples: i32,
        pub cancelled: i32,
    }
    #[cfg(cycles_sdk)]
    unsafe extern "C" {
        pub fn n4c_create(device: *const c_char, threads: i32) -> *mut N4Cycles;
        pub fn n4c_destroy(s: *mut N4Cycles);
        pub fn n4c_last_error() -> *const c_char;
        pub fn n4c_device_description(s: *mut N4Cycles) -> *const c_char;
        pub fn n4c_image_create(
            s: *mut N4Cycles,
            w: i32,
            h: i32,
            srgb: i32,
            rgba: *const u8,
        ) -> i32;
        pub fn n4c_image_update(
            s: *mut N4Cycles,
            image: i32,
            w: i32,
            h: i32,
            rgba: *const u8,
        ) -> i32;
        pub fn n4c_material_create(s: *mut N4Cycles, m: *const N4Material) -> i32;
        pub fn n4c_material_update(s: *mut N4Cycles, material: i32, m: *const N4Material) -> i32;
        pub fn n4c_object_create(
            s: *mut N4Cycles,
            name: *const c_char,
            vertex_count: i32,
            positions: *const f32,
            normals: *const f32,
            uvs: *const f32,
            triangle_count: i32,
            indices: *const u32,
            tri_material: *const u32,
            material_count: i32,
            materials: *const i32,
            pass_id: i32,
        ) -> i32;
        pub fn n4c_object_transform(s: *mut N4Cycles, object: i32, m3x4: *const f32) -> i32;
        pub fn n4c_object_camera_visible(s: *mut N4Cycles, object: i32, visible: i32) -> i32;
        pub fn n4c_light_rect(
            s: *mut N4Cycles,
            c: *const f32,
            u: *const f32,
            v: *const f32,
            radiance: *const f32,
            camera_visible: i32,
        ) -> i32;
        pub fn n4c_light_rect_update(
            s: *mut N4Cycles,
            light: i32,
            c: *const f32,
            u: *const f32,
            v: *const f32,
            radiance: *const f32,
        ) -> i32;
        pub fn n4c_light_distant(
            s: *mut N4Cycles,
            direction: *const f32,
            irradiance: *const f32,
            angle_degrees: f32,
        ) -> i32;
        pub fn n4c_background(s: *mut N4Cycles, radiance: *const f32, transparent_film: i32);
        pub fn n4c_camera(
            s: *mut N4Cycles,
            p: *const f32,
            r: *const f32,
            u: *const f32,
            f: *const f32,
            fov_y_degrees: f32,
            near_clip: f32,
        );
        pub fn n4c_render(s: *mut N4Cycles, r: *const N4Render, timing: *mut N4Timing) -> i32;
        pub fn n4c_read_color(s: *mut N4Cycles, rgba: *mut f32) -> i32;
        pub fn n4c_read_mask(s: *mut N4Cycles, i: i32, coverage: *mut f32) -> i32;
        pub fn n4c_peak_memory(s: *mut N4Cycles) -> i64;
        pub fn n4c_read_depth(s: *mut N4Cycles, depth: *mut f32) -> i32;
        pub fn n4c_read_cryptomatte(s: *mut N4Cycles, layer: i32, rgba: *mut f32) -> i32;
        pub fn n4c_cancel(s: *mut N4Cycles);
        pub fn n4c_progress(s: *mut N4Cycles) -> f32;
        pub fn n4c_snapshot_count(s: *mut N4Cycles) -> i32;
    }
    // Without the bundle `n4c_create` fails, so nothing below it is ever reached.
    #[cfg(not(cycles_sdk))]
    mod absent {
        use super::*;
        pub unsafe fn n4c_create(_device: *const c_char, _threads: i32) -> *mut N4Cycles {
            std::ptr::null_mut()
        }
        pub unsafe fn n4c_destroy(_s: *mut N4Cycles) {}
        pub unsafe fn n4c_last_error() -> *const c_char {
            c"Cycles is not provisioned in this build. Run spikes/spatial/cycles/tools/provision.sh and rebuild.".as_ptr()
        }
        pub unsafe fn n4c_device_description(_s: *mut N4Cycles) -> *const c_char {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_image_create(
            _s: *mut N4Cycles,
            _w: i32,
            _h: i32,
            _srgb: i32,
            _rgba: *const u8,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_image_update(
            _s: *mut N4Cycles,
            _image: i32,
            _w: i32,
            _h: i32,
            _rgba: *const u8,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_material_create(_s: *mut N4Cycles, _m: *const N4Material) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_material_update(
            _s: *mut N4Cycles,
            _material: i32,
            _m: *const N4Material,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_object_create(
            _s: *mut N4Cycles,
            _name: *const c_char,
            _vertex_count: i32,
            _positions: *const f32,
            _normals: *const f32,
            _uvs: *const f32,
            _triangle_count: i32,
            _indices: *const u32,
            _tri_material: *const u32,
            _material_count: i32,
            _materials: *const i32,
            _pass_id: i32,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_object_transform(
            _s: *mut N4Cycles,
            _object: i32,
            _m3x4: *const f32,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_object_camera_visible(
            _s: *mut N4Cycles,
            _object: i32,
            _visible: i32,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_light_rect(
            _s: *mut N4Cycles,
            _c: *const f32,
            _u: *const f32,
            _v: *const f32,
            _radiance: *const f32,
            _camera_visible: i32,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_light_rect_update(
            _s: *mut N4Cycles,
            _light: i32,
            _c: *const f32,
            _u: *const f32,
            _v: *const f32,
            _radiance: *const f32,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_light_distant(
            _s: *mut N4Cycles,
            _direction: *const f32,
            _irradiance: *const f32,
            _angle_degrees: f32,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_background(
            _s: *mut N4Cycles,
            _radiance: *const f32,
            _transparent_film: i32,
        ) {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_camera(
            _s: *mut N4Cycles,
            _p: *const f32,
            _r: *const f32,
            _u: *const f32,
            _f: *const f32,
            _fov_y_degrees: f32,
            _near_clip: f32,
        ) {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_render(
            _s: *mut N4Cycles,
            _r: *const N4Render,
            _timing: *mut N4Timing,
        ) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_read_color(_s: *mut N4Cycles, _rgba: *mut f32) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_read_mask(_s: *mut N4Cycles, _i: i32, _coverage: *mut f32) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_peak_memory(_s: *mut N4Cycles) -> i64 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_read_depth(_s: *mut N4Cycles, _depth: *mut f32) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_read_cryptomatte(_s: *mut N4Cycles, _layer: i32, _rgba: *mut f32) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_cancel(_s: *mut N4Cycles) {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_progress(_s: *mut N4Cycles) -> f32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
        pub unsafe fn n4c_snapshot_count(_s: *mut N4Cycles) -> i32 {
            unreachable!("no Cycles session can exist without the bundle")
        }
    }
    #[cfg(not(cycles_sdk))]
    pub use absent::*;
}

pub use sys::N4Timing as Timing;

/// Whether this build links a provisioned Cycles bundle.
pub const fn available() -> bool {
    cfg!(cycles_sdk)
}

/// How live artwork bound to a slot behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Artwork {
    None,
    /// Composited over the slot's paint by the artwork's coverage, then lit.
    BaseColor(ImageId),
    /// A screen. The camera sees `scale` times the artwork; other paths see `scale * glow`.
    Emission {
        image: ImageId,
        scale: f32,
        glow: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageId(i32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialId(i32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectId(i32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightId(i32);

/// Everything that fixes a frame's sampling. Stored with the shot.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub samples: u32,
    pub max_bounces: u32,
    pub clamp_indirect: f32,
    pub denoise: bool,
    pub seed: u32,
}

pub struct Frame {
    pub size: [u32; 2],
    /// Scene-linear premultiplied RGBA.
    pub color: Vec<f32>,
    /// One coverage layer per requested mask.
    pub masks: Vec<Vec<f32>>,
    /// Camera distance per pixel, when requested. Cycles does not antialias it.
    pub depth: Option<Vec<f32>>,
    /// Cryptomatte asset layers, each RGBA holding two (id, coverage) pairs, when requested.
    pub cryptomatte: Vec<Vec<f32>>,
    pub timing: Timing,
}

/// Optional outputs beyond colour.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outputs<'a> {
    pub mask_pass_ids: &'a [u32],
    pub depth: bool,
    pub cryptomatte: bool,
}

pub struct Session {
    raw: *mut sys::N4Cycles,
    descs: Vec<sys::N4Material>,
}

/// Watches or stops a render from another thread while `Session::render` blocks.
#[derive(Clone, Copy)]
pub struct Monitor(usize);
// The three calls behind it take Cycles' own locks.
unsafe impl Send for Monitor {}
unsafe impl Sync for Monitor {}

impl Monitor {
    /// Stop the render in flight. `render` then returns an error and the session stays usable.
    pub fn cancel(&self) {
        unsafe { sys::n4c_cancel(self.0 as *mut sys::N4Cycles) }
    }
    /// 0..1.
    pub fn progress(&self) -> f32 {
        unsafe { sys::n4c_progress(self.0 as *mut sys::N4Cycles) }
    }
    /// How many in-progress pictures Cycles has handed over in this render.
    pub fn snapshots(&self) -> u32 {
        unsafe { sys::n4c_snapshot_count(self.0 as *mut sys::N4Cycles) as u32 }
    }
    /// Copy the latest in-progress picture. `rgba` holds width * height * 4 floats.
    pub fn snapshot(&self, rgba: &mut [f32]) -> bool {
        unsafe { sys::n4c_read_color(self.0 as *mut sys::N4Cycles, rgba.as_mut_ptr()) == 0 }
    }
}

fn last_error() -> String {
    unsafe { CStr::from_ptr(sys::n4c_last_error()) }
        .to_string_lossy()
        .into_owned()
}

fn channel(c: TextureChannel) -> i32 {
    match c {
        TextureChannel::Rgb | TextureChannel::R => 0,
        TextureChannel::G => 1,
        TextureChannel::B => 2,
        TextureChannel::A => 3,
    }
}

impl Session {
    /// `device` is "METAL" or "CPU". `threads` 0 lets Cycles choose.
    pub fn new(device: &str, threads: u32) -> Result<Self> {
        let name = CString::new(device)?;
        let raw = unsafe { sys::n4c_create(name.as_ptr(), threads as i32) };
        ensure!(!raw.is_null(), "{}", last_error());
        Ok(Self {
            raw,
            descs: Vec::new(),
        })
    }

    pub fn device(&self) -> String {
        unsafe { CStr::from_ptr(sys::n4c_device_description(self.raw)) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn image(&mut self, size: [u32; 2], srgb: bool, rgba: &[u8]) -> Result<ImageId> {
        ensure!(
            rgba.len() == (size[0] * size[1] * 4) as usize,
            "image byte count"
        );
        Ok(ImageId(unsafe {
            sys::n4c_image_create(
                self.raw,
                size[0] as i32,
                size[1] as i32,
                srgb as i32,
                rgba.as_ptr(),
            )
        }))
    }

    pub fn update_image(&mut self, image: ImageId, size: [u32; 2], rgba: &[u8]) -> Result<()> {
        ensure!(
            rgba.len() == (size[0] * size[1] * 4) as usize,
            "image byte count"
        );
        unsafe {
            sys::n4c_image_update(
                self.raw,
                image.0,
                size[0] as i32,
                size[1] as i32,
                rgba.as_ptr(),
            )
        };
        Ok(())
    }

    /// Upload a mesh's images and materials. Returns one material per `mesh.materials` entry.
    pub fn materials(
        &mut self,
        mesh: &Mesh,
        artwork: &dyn Fn(&str) -> Artwork,
    ) -> Result<Vec<MaterialId>> {
        let mut images: Vec<Option<[Option<ImageId>; 2]>> = vec![None; mesh.images.len()];
        let mut ids = Vec::new();
        for m in &mesh.materials {
            let mut lookup = |map: &Option<nothing_spatial::TextureMap>,
                              this: &mut Self|
             -> Result<(i32, i32)> {
                let Some(map) = map else { return Ok((-1, 0)) };
                let slot = images[map.image].get_or_insert([None, None]);
                let k = map.srgb as usize;
                if slot[k].is_none() {
                    let im = &mesh.images[map.image];
                    slot[k] = Some(this.image([im.width, im.height], map.srgb, &im.rgba)?);
                }
                Ok((slot[k].unwrap().0, channel(map.channel)))
            };
            let (base_image, _) = lookup(&m.maps.diffuse, self)?;
            let (normal_image, _) = lookup(&m.maps.normal, self)?;
            let (roughness_image, roughness_channel) = lookup(&m.maps.roughness, self)?;
            let (metallic_image, metallic_channel) = lookup(&m.maps.metallic, self)?;
            let desc = material_desc(
                m,
                artwork(&m.slot),
                base_image,
                normal_image,
                roughness_image,
                roughness_channel,
                metallic_image,
                metallic_channel,
            );
            let id = unsafe { sys::n4c_material_create(self.raw, &desc) };
            self.descs.resize(id as usize + 1, desc);
            ids.push(MaterialId(id));
        }
        Ok(ids)
    }

    /// Change one material's values or artwork binding. Its texture maps and geometry are kept.
    pub fn update_material(
        &mut self,
        id: MaterialId,
        m: &nothing_spatial::Material,
        artwork: Artwork,
    ) {
        let old = self.descs[id.0 as usize];
        let desc = material_desc(
            m,
            artwork,
            old.base_image,
            old.normal_image,
            old.roughness_image,
            old.roughness_channel,
            old.metallic_image,
            old.metallic_channel,
        );
        self.descs[id.0 as usize] = desc;
        unsafe { sys::n4c_material_update(self.raw, id.0, &desc) };
    }

    pub fn monitor(&self) -> Monitor {
        Monitor(self.raw as usize)
    }

    /// One Cycles object per rigid part, so a part pose is a transform update only.
    /// `pass_id` > 0 makes the whole model addressable by a mask.
    pub fn model(
        &mut self,
        mesh: &Mesh,
        materials: &[MaterialId],
        pass_id: u32,
    ) -> Result<Vec<ObjectId>> {
        ensure!(
            materials.len() == mesh.materials.len(),
            "one material per slot"
        );
        let mats: Vec<i32> = materials.iter().map(|m| m.0).collect();
        let mut out = Vec::new();
        for part in &mesh.parts {
            let range = part.indices.start as usize..part.indices.end as usize;
            let mut remap = std::collections::HashMap::new();
            let (mut p, mut n, mut uv, mut idx, mut tri) = (vec![], vec![], vec![], vec![], vec![]);
            for (k, &i) in mesh.indices[range].iter().enumerate() {
                let v = &mesh.vertices[i as usize];
                let next = remap.len() as u32;
                let j = *remap.entry(i).or_insert_with(|| {
                    p.extend(v.position);
                    n.extend(v.normal);
                    uv.extend(v.uv);
                    next
                });
                idx.push(j);
                if k % 3 == 0 {
                    tri.push(v.material);
                }
            }
            let name = CString::new(part.path.as_str())?;
            let id = if idx.is_empty() {
                -1
            } else {
                unsafe {
                    sys::n4c_object_create(
                        self.raw,
                        name.as_ptr(),
                        (p.len() / 3) as i32,
                        p.as_ptr(),
                        n.as_ptr(),
                        uv.as_ptr(),
                        (idx.len() / 3) as i32,
                        idx.as_ptr(),
                        tri.as_ptr(),
                        mats.len() as i32,
                        mats.as_ptr(),
                        pass_id as i32,
                    )
                }
            };
            out.push(ObjectId(id));
        }
        Ok(out)
    }

    /// Row-major 3x4 object-to-world transform. Parts without triangles are skipped.
    pub fn transform(&mut self, object: ObjectId, rows: &[f32; 12]) {
        if object.0 >= 0 {
            unsafe { sys::n4c_object_transform(self.raw, object.0, rows.as_ptr()) };
        }
    }

    pub fn camera_visible(&mut self, object: ObjectId, visible: bool) {
        if object.0 >= 0 {
            unsafe { sys::n4c_object_camera_visible(self.raw, object.0, visible as i32) };
        }
    }

    /// A rectangle centred at `center` spanning the two full edge vectors, emitting `radiance` along cross(u, v).
    pub fn rect_light(
        &mut self,
        center: [f32; 3],
        edge_u: [f32; 3],
        edge_v: [f32; 3],
        radiance: [f32; 3],
        camera_visible: bool,
    ) -> LightId {
        LightId(unsafe {
            sys::n4c_light_rect(
                self.raw,
                center.as_ptr(),
                edge_u.as_ptr(),
                edge_v.as_ptr(),
                radiance.as_ptr(),
                camera_visible as i32,
            )
        })
    }

    pub fn update_rect_light(
        &mut self,
        light: LightId,
        center: [f32; 3],
        edge_u: [f32; 3],
        edge_v: [f32; 3],
        radiance: [f32; 3],
    ) {
        unsafe {
            sys::n4c_light_rect_update(
                self.raw,
                light.0,
                center.as_ptr(),
                edge_u.as_ptr(),
                edge_v.as_ptr(),
                radiance.as_ptr(),
            )
        };
    }

    /// Light travelling along `direction` with the given irradiance on a facing surface.
    pub fn distant_light(
        &mut self,
        direction: [f32; 3],
        irradiance: [f32; 3],
        angle_degrees: f32,
    ) -> LightId {
        LightId(unsafe {
            sys::n4c_light_distant(
                self.raw,
                direction.as_ptr(),
                irradiance.as_ptr(),
                angle_degrees,
            )
        })
    }

    pub fn background(&mut self, radiance: [f32; 3], transparent_film: bool) {
        unsafe { sys::n4c_background(self.raw, radiance.as_ptr(), transparent_film as i32) };
    }

    pub fn camera(
        &mut self,
        position: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        forward: [f32; 3],
        fov_y_degrees: f32,
    ) {
        unsafe {
            sys::n4c_camera(
                self.raw,
                position.as_ptr(),
                right.as_ptr(),
                up.as_ptr(),
                forward.as_ptr(),
                fov_y_degrees,
                0.01,
            )
        };
    }

    /// Render one frame of the current scene to completion.
    pub fn render(
        &mut self,
        size: [u32; 2],
        recipe: &Recipe,
        mask_pass_ids: &[u32],
    ) -> Result<Frame> {
        self.render_with(
            size,
            recipe,
            Outputs {
                mask_pass_ids,
                ..Default::default()
            },
        )
    }

    pub fn render_with(
        &mut self,
        size: [u32; 2],
        recipe: &Recipe,
        outputs: Outputs<'_>,
    ) -> Result<Frame> {
        let ids: Vec<i32> = outputs.mask_pass_ids.iter().map(|&i| i as i32).collect();
        let r = sys::N4Render {
            width: size[0] as i32,
            height: size[1] as i32,
            samples: recipe.samples as i32,
            max_bounces: recipe.max_bounces as i32,
            clamp_indirect: recipe.clamp_indirect,
            denoise: recipe.denoise as i32,
            seed: recipe.seed as i32,
            mask_count: ids.len() as i32,
            mask_pass_ids: ids.as_ptr(),
            depth: outputs.depth as i32,
            cryptomatte: outputs.cryptomatte as i32,
        };
        let mut timing = Timing::default();
        if unsafe { sys::n4c_render(self.raw, &r, &mut timing) } != 0 {
            bail!("Cycles render failed: {}", last_error());
        }
        ensure!(timing.cancelled == 0, "render cancelled");
        let pixels = (size[0] * size[1]) as usize;
        let mut color = vec![0f32; pixels * 4];
        ensure!(
            unsafe { sys::n4c_read_color(self.raw, color.as_mut_ptr()) } == 0,
            "{}",
            last_error()
        );
        let mut masks = Vec::new();
        for i in 0..ids.len() {
            let mut m = vec![0f32; pixels];
            ensure!(
                unsafe { sys::n4c_read_mask(self.raw, i as i32, m.as_mut_ptr()) } == 0,
                "{}",
                last_error()
            );
            masks.push(m);
        }
        let depth = if outputs.depth {
            let mut d = vec![0f32; pixels];
            ensure!(
                unsafe { sys::n4c_read_depth(self.raw, d.as_mut_ptr()) } == 0,
                "{}",
                last_error()
            );
            Some(d)
        } else {
            None
        };
        let mut cryptomatte = Vec::new();
        for layer in 0..if outputs.cryptomatte { 3 } else { 0 } {
            let mut c = vec![0f32; pixels * 4];
            ensure!(
                unsafe { sys::n4c_read_cryptomatte(self.raw, layer, c.as_mut_ptr()) } == 0,
                "{}",
                last_error()
            );
            cryptomatte.push(c);
        }
        Ok(Frame {
            size,
            color,
            masks,
            depth,
            cryptomatte,
            timing,
        })
    }

    pub fn peak_device_memory(&self) -> u64 {
        unsafe { sys::n4c_peak_memory(self.raw) as u64 }
    }
}

#[allow(clippy::too_many_arguments)]
fn material_desc(
    m: &nothing_spatial::Material,
    artwork: Artwork,
    base_image: i32,
    normal_image: i32,
    roughness_image: i32,
    roughness_channel: i32,
    metallic_image: i32,
    metallic_channel: i32,
) -> sys::N4Material {
    let (artwork_image, artwork_role, emission_scale, glow_scale) = match artwork {
        Artwork::None => (-1, 0, 1., 1.),
        Artwork::BaseColor(i) => (i.0, 1, 1., 1.),
        Artwork::Emission { image, scale, glow } => (image.0, 2, scale, glow),
    };
    sys::N4Material {
        base_color: m.diffuse,
        metallic: m.metallic,
        roughness: m.roughness,
        specular_ior_level: 0.5,
        coat_weight: m.clearcoat,
        coat_roughness: m.clearcoat_roughness,
        emission: m.emissive,
        base_image,
        normal_image,
        roughness_image,
        roughness_channel,
        metallic_image,
        metallic_channel,
        artwork_image,
        artwork_role,
        emission_scale,
        glow_scale,
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { sys::n4c_destroy(self.raw) }
    }
}
