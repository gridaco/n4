//! CPU geometry shared by spatial importers, renderers and host applications.

/// Vertex in right-handed, Y-up meters. The material index addresses the mesh table.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
    /// USD-style st, with the origin at the lower left.
    pub uv: [f32; 2],
}

/// Opaque linear-light material for the rigid spatial surface profile.
#[derive(Clone, Debug)]
pub struct Material {
    pub diffuse: [f32; 3],
    pub emissive: [f32; 3],
    pub specular: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub clearcoat: f32,
    pub clearcoat_roughness: f32,
    pub use_specular_workflow: bool,
    pub slot: String,
    pub maps: MaterialMaps,
}

/// Indexed triangles with shared vertex and material tables.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub materials: Vec<Material>,
    pub parts: Vec<Part>,
    pub images: Vec<Image>,
}

/// Rigid hierarchy node. Its index range covers only the node's own triangles.
/// Vertices use the common asset frame, so a rest pose needs no extra transform.
#[derive(Clone, Debug)]
pub struct Part {
    pub path: String,
    pub parent: Option<usize>,
    pub indices: std::ops::Range<u32>,
    /// Column-major transform from normalized node-local meters to asset space.
    pub frame: [[f32; 4]; 4],
    /// Center of an authored pivot (or node origin), in asset-space meters.
    pub pivot: [f32; 3],
}

#[derive(Clone)]
pub struct Image {
    /// Resolved source identity; may name a member inside a package.
    pub source: String,
    pub width: u32,
    pub height: u32,
    /// Unassociated RGBA8 pixels, top row first. Color interpretation belongs to the binding.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("source", &self.source)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("byte_count", &self.rgba.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureChannel {
    Rgb,
    R,
    G,
    B,
    A,
}

#[derive(Clone, Debug)]
pub struct TextureMap {
    pub image: usize,
    pub channel: TextureChannel,
    pub srgb: bool,
    /// USD UVTexture applies color decoding before scale and bias.
    pub scale: [f32; 4],
    pub bias: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct MaterialMaps {
    pub diffuse: Option<TextureMap>,
    pub normal: Option<TextureMap>,
    pub roughness: Option<TextureMap>,
    pub metallic: Option<TextureMap>,
    pub occlusion: Option<TextureMap>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            diffuse: [0.18; 3],
            emissive: [0.; 3],
            specular: [0.; 3],
            metallic: 0.,
            roughness: 0.5,
            clearcoat: 0.,
            clearcoat_roughness: 0.01,
            use_specular_workflow: false,
            slot: String::new(),
            maps: MaterialMaps::default(),
        }
    }
}
