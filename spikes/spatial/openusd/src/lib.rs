//! Rigid scene geometry at the asset's native reference root, normalized to Y-up meters.
use anyhow::{Result, bail};
use std::path::Path;

use nothing_spatial::Mesh;

/// Load the visible rigid mesh subtree rooted at an absolute USD prim path.
#[cfg(not(openusd_sdk))]
pub fn load(path: &Path, prim: &str) -> Result<Mesh> {
    bail!(
        "USD {} at {prim}: mesh import requires an OpenUSD SDK. Enable the sdk feature, install OpenUSD and rebuild (see spikes/spatial/openusd/PROFILE.md)",
        path.display()
    )
}

#[test]
#[cfg(not(openusd_sdk))]
fn an_sdk_free_build_identifies_the_missing_dependency() {
    let error = load(Path::new("example.usda"), "/Object").unwrap_err();
    assert!(error.to_string().contains("requires an OpenUSD SDK"));
}

#[cfg(openusd_sdk)]
mod native {
    use super::*;
    use anyhow::Context;
    use nothing_spatial::{
        Image, Material, MaterialMaps, Part, TextureChannel, TextureMap, Vertex,
    };
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::slice;

    #[repr(C)]
    struct NativeMap {
        image: i32,
        channel: u32,
        srgb: u32,
        scale: [f32; 4],
        bias: [f32; 4],
    }

    #[repr(C)]
    struct NativeImage {
        source: *const c_char,
        data: *const u8,
        size: usize,
    }

    #[repr(C)]
    struct NativePart {
        path: *const c_char,
        parent: i32,
        first_index: u32,
        index_count: u32,
        frame: [[f32; 4]; 4],
        pivot: [f32; 3],
    }

    #[repr(C)]
    struct NativeMaterial {
        diffuse: [f32; 3],
        emissive: [f32; 3],
        specular: [f32; 3],
        metallic: f32,
        roughness: f32,
        clearcoat: f32,
        clearcoat_roughness: f32,
        use_specular_workflow: u32,
        slot: *const c_char,
        maps: [NativeMap; 5],
    }

    #[repr(C)]
    struct View {
        vertices: *const Vertex,
        vertex_count: usize,
        indices: *const u32,
        index_count: usize,
        materials: *const NativeMaterial,
        material_count: usize,
        images: *const NativeImage,
        image_count: usize,
        parts: *const NativePart,
        part_count: usize,
    }

    unsafe extern "C" {
        fn nothing_usd_mesh_load(
            path: *const c_char,
            prim: *const c_char,
            sample: f64,
            timed: u32,
            user: *mut c_void,
            receive: unsafe extern "C" fn(*mut c_void, *const View),
            error: *mut c_char,
            error_size: usize,
        ) -> i32;
    }

    unsafe fn name(value: *const c_char) -> String {
        // The C++ reader keeps every string alive until the callback returns.
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    }

    unsafe fn receive_mesh(v: &View) -> Result<Mesh> {
        let vertices = unsafe { slice::from_raw_parts(v.vertices, v.vertex_count) };
        let indices = unsafe { slice::from_raw_parts(v.indices, v.index_count) };
        let materials = unsafe { slice::from_raw_parts(v.materials, v.material_count) };
        let parts = unsafe { slice::from_raw_parts(v.parts, v.part_count) };
        let mut images = Vec::with_capacity(v.image_count);
        if v.image_count > 0 {
            for source in unsafe { slice::from_raw_parts(v.images, v.image_count) } {
                let path = unsafe { name(source.source) };
                let bytes = unsafe { slice::from_raw_parts(source.data, source.size) };
                let image = image::load_from_memory(bytes)
                    .with_context(|| format!("decode texture {path} as PNG or JPEG"))?
                    .to_rgba8();
                images.push(Image {
                    source: path,
                    width: image.width(),
                    height: image.height(),
                    rgba: image.into_raw(),
                });
            }
        }
        fn binding(map: &NativeMap) -> Result<Option<TextureMap>> {
            if map.image < 0 {
                return Ok(None);
            }
            let channel = match map.channel {
                0 => TextureChannel::Rgb,
                1 => TextureChannel::R,
                2 => TextureChannel::G,
                3 => TextureChannel::B,
                4 => TextureChannel::A,
                n => bail!("invalid texture channel {n}"),
            };
            Ok(Some(TextureMap {
                image: map.image as usize,
                channel,
                srgb: map.srgb != 0,
                scale: map.scale,
                bias: map.bias,
            }))
        }
        Ok(Mesh {
            vertices: vertices.to_vec(),
            indices: indices.to_vec(),
            images,
            parts: parts
                .iter()
                .map(|p| Part {
                    path: unsafe { name(p.path) },
                    parent: usize::try_from(p.parent).ok(),
                    indices: p.first_index..p.first_index + p.index_count,
                    frame: p.frame,
                    pivot: p.pivot,
                })
                .collect(),
            materials: materials
                .iter()
                .map(|m| {
                    Ok(Material {
                        diffuse: m.diffuse,
                        emissive: m.emissive,
                        specular: m.specular,
                        metallic: m.metallic,
                        roughness: m.roughness,
                        clearcoat: m.clearcoat,
                        clearcoat_roughness: m.clearcoat_roughness,
                        use_specular_workflow: m.use_specular_workflow != 0,
                        slot: unsafe { name(m.slot) },
                        maps: MaterialMaps {
                            diffuse: binding(&m.maps[0])?,
                            normal: binding(&m.maps[1])?,
                            roughness: binding(&m.maps[2])?,
                            metallic: binding(&m.maps[3])?,
                            occlusion: binding(&m.maps[4])?,
                        },
                    })
                })
                .collect::<Result<_>>()?,
        })
    }

    unsafe extern "C" fn receive(user: *mut c_void, view: *const View) {
        // Nonempty mesh buffers and the optional encoded images are borrowed for this call only.
        let out = unsafe { &mut *user.cast::<Option<Result<Mesh>>>() };
        *out = Some(unsafe { receive_mesh(&*view) });
    }

    /// Load the visible rigid mesh subtree rooted at an absolute USD prim path.
    pub fn load(path: &Path, prim: &str) -> Result<Mesh> {
        read(path, prim, None)
    }

    /// Sample authored rigid transforms at a USD stage time code.
    pub fn load_at(path: &Path, prim: &str, time_code: f64) -> Result<Mesh> {
        anyhow::ensure!(time_code.is_finite(), "USD time code must be finite");
        read(path, prim, Some(time_code))
    }

    fn read(path: &Path, prim: &str, time: Option<f64>) -> Result<Mesh> {
        let filename = CString::new(path.as_os_str().as_encoded_bytes())?;
        let prim_path = CString::new(prim)?;
        let mut out: Option<Result<Mesh>> = None;
        let mut error = [0 as c_char; 2048];
        let ok = unsafe {
            nothing_usd_mesh_load(
                filename.as_ptr(),
                prim_path.as_ptr(),
                time.unwrap_or_default(),
                u32::from(time.is_some()),
                (&mut out as *mut Option<Result<Mesh>>).cast(),
                receive,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
            bail!("USD {} at {prim}: {message}", path.display());
        }
        out.context("USD importer produced no mesh")?
            .with_context(|| format!("USD {} at {prim}", path.display()))
    }
}

#[cfg(openusd_sdk)]
pub use native::{load, load_at};

#[cfg(not(openusd_sdk))]
pub fn load_at(path: &Path, prim: &str, _time_code: f64) -> Result<Mesh> {
    load(path, prim)
}
