//! Pixel controls for the product-shot surface, lighting and view-transform contract.
use anyhow::{Context, Result};
use nothing_spatial::{Image, Material, Mesh, Part, TextureChannel, TextureMap, Vertex};
use nothing_spatial_wgpu::*;
use std::sync::mpsc;
const N: u32 = 128;
struct Host {
    d: wgpu::Device,
    q: wgpu::Queue,
    r: Renderer,
}
impl Host {
    fn new() -> Result<Self> {
        let i = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let a =
            pollster::block_on(i.request_adapter(&Default::default())).context("Metal adapter")?;
        let (d, q) = pollster::block_on(a.request_device(&Default::default()))?;
        let r = Renderer::new(&d);
        Ok(Self { d, q, r })
    }
    fn texture(
        &self,
        n: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.d.create_texture(&wgpu::TextureDescriptor {
            label: Some("studio test target"),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }
    fn upload(&self, mesh: &Mesh) -> Result<MeshBuffers> {
        MeshBuffers::upload(&self.d, &self.q, mesh)
    }
    fn image(&self, data: &[u8]) -> wgpu::TextureView {
        let t = self.texture(
            4,
            COLOR_FORMAT,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        self.q.write_texture(
            t.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(16),
                rows_per_image: None,
            },
            t.size(),
        );
        t.create_view(&Default::default())
    }
    fn frame(
        &mut self,
        objects: &[Object<'_>],
        lights: &[Light],
        studio: &Studio,
    ) -> Result<Vec<u8>> {
        let t = self.texture(
            N,
            COLOR_FORMAT,
            wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let mut e = self.d.create_command_encoder(&Default::default());
        self.r.record_studio(
            &mut e,
            &t,
            View {
                position: [0., 0., 4.],
                rotation_degrees: [0.; 3],
                fov_y_degrees: 45.,
            },
            objects,
            lights,
            None,
            studio,
        )?;
        self.readback(e, &t, 4)
    }
    fn readback(
        &self,
        mut e: wgpu::CommandEncoder,
        t: &wgpu::Texture,
        bytes: u32,
    ) -> Result<Vec<u8>> {
        let stride = (N * bytes).next_multiple_of(256);
        let b = self.d.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (N * stride) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        e.copy_texture_to_buffer(
            t.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &b,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            t.size(),
        );
        let id = self.q.submit([e.finish()]);
        let (tx, rx) = mpsc::channel();
        b.slice(..)
            .map_async(wgpu::MapMode::Read, move |x| tx.send(x).unwrap());
        self.d.poll(wgpu::PollType::Wait {
            submission_index: Some(id),
            timeout: None,
        })?;
        rx.recv()??;
        let result = b
            .slice(..)
            .get_mapped_range()?
            .chunks_exact(stride as usize)
            .flat_map(|row| row[..(N * bytes) as usize].iter().copied())
            .collect();
        b.unmap();
        Ok(result)
    }
}
fn card(material: Material) -> Mesh {
    let vertices = [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]]
        .map(|[x, y]| Vertex {
            position: [x, y, 0.],
            normal: [0., 0., 1.],
            uv: [x * 0.5 + 0.5, y * 0.5 + 0.5],
            material: 0,
        })
        .to_vec();
    Mesh {
        vertices,
        indices: vec![0, 1, 2, 0, 2, 3],
        materials: vec![material],
        ..Default::default()
    }
}
fn paint() -> Material {
    Material {
        diffuse: [0.65; 3],
        roughness: 1.,
        use_specular_workflow: true,
        slot: "/Paint".into(),
        ..Default::default()
    }
}
fn pix(img: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * N + x) * 4) as usize;
    img[i..i + 4].try_into().unwrap()
}
fn object(g: Geometry<'_>, p: [f32; 3]) -> Object<'_> {
    Object {
        id: Some("case"),
        geometry: g,
        pose: Pose::rigid(p, [0.; 3]),
    }
}
fn key(x: f32) -> Light {
    Light {
        direction: [x, 0., -1.],
        radiance: [3.; 3],
    }
}

#[test]
fn real_uvs_slots_rigid_pivot_and_display_curve() -> Result<()> {
    let mut h = Host::new()?;
    let base = paint();
    let mut mesh = card(base);
    mesh.parts = vec![Part {
        path: "/Lid".into(),
        parent: None,
        indices: 0..6,
        pivot: [-1., 0., 0.],
        frame: [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [-1., 0., 0., 1.],
        ],
    }];
    let model = h.upload(&mesh)?;
    let mut bytes = Vec::new();
    for y in 0..4 {
        for x in 0..4 {
            bytes.extend(if x < 2 && y < 2 {
                [160, 4, 4, 255]
            } else if x >= 2 && y >= 2 {
                [0, 0, 0, 0]
            } else {
                [8, 120, 8, 255]
            });
        }
    }
    let art = h.image(&bytes);
    let unlit = [SurfaceBinding {
        slot: "/Paint",
        texture: &art,
        mode: SurfaceMode::Unlit,
    }];
    let printed = [SurfaceBinding {
        slot: "/Paint",
        texture: &art,
        mode: SurfaceMode::BaseColor,
    }];
    let studio = Studio::default();
    let make = |surfaces, parts| {
        object(
            Geometry::Model {
                mesh: &model,
                surfaces,
                parts,
            },
            [0.; 3],
        )
    };
    let a = h.frame(&[make(&unlit, &[])], &[key(0.)], &studio)?;
    assert_eq!(
        pix(&a, 42, 42),
        [160, 4, 4, 255],
        "unlit sRGB red mapped to upper-left"
    );
    assert_eq!(pix(&a, 85, 85)[3], 0, "transparent screen region");
    let moved_light = h.frame(&[make(&unlit, &[])], &[key(2.)], &studio)?;
    assert_eq!(a, moved_light, "screen is independent of direct light");
    let b = h.frame(&[make(&printed, &[])], &[key(0.)], &studio)?;
    assert_eq!(
        pix(&b, 85, 85)[3],
        255,
        "paint remains under transparent lettering"
    );
    let c = h.frame(&[make(&printed, &[])], &[], &studio)?;
    assert!(
        pix(&b, 42, 42)[0] > pix(&c, 42, 42)[0] + 15,
        "printed artwork receives light"
    );
    let angles = [PartPose {
        path: "/Lid",
        rotation_degrees: [0., 65., 0.],
    }];
    let lid = h.frame(&[make(&unlit, &angles)], &[], &studio)?;
    assert_eq!(
        pix(&lid, 86, 42)[3],
        0,
        "right edge moved around the left hinge"
    );
    assert!(
        pix(&lid, 30, 60)[3] > 0,
        "hinge stays at authored left edge"
    );
    let invalid = [SurfaceBinding {
        slot: "/Renamed",
        texture: &art,
        mode: SurfaceMode::Unlit,
    }];
    assert!(
        h.frame(&[make(&invalid, &[])], &[], &studio)
            .unwrap_err()
            .to_string()
            .contains("/Renamed")
    );

    let mut raw = vec![];
    for y in 0..4 {
        for _ in 0..4 {
            raw.extend(if y < 2 {
                [128, 0, 0, 255]
            } else {
                [0, 128, 0, 255]
            });
        }
    }
    mesh.images.push(Image {
        source: "calibration".into(),
        width: 4,
        height: 4,
        rgba: raw,
    });
    let mat = &mut mesh.materials[0];
    mat.diffuse = [1.; 3];
    mat.maps.diffuse = Some(TextureMap {
        image: 0,
        channel: TextureChannel::Rgb,
        srgb: true,
        scale: [1.; 4],
        bias: [0.; 4],
    });
    let map = h.upload(&mesh)?;
    let lit = h.frame(
        &[object(Geometry::Mesh(&map), [0.; 3])],
        &[],
        &Studio {
            environment_intensity: 0.,
            ..studio
        },
    )?;
    let top = pix(&lit, 64, 42);
    let bottom = pix(&lit, 64, 85);
    // sRGB 128 = 0.21586 linear; 7% ambient yields about sRGB 33.
    assert!(
        top[0].abs_diff(33) <= 2 && top[1] == 0 && bottom[1].abs_diff(33) <= 2 && bottom[0] == 0,
        "decoded texture and flip: {top:?}, {bottom:?}"
    );

    let emission = |v| {
        card(Material {
            diffuse: [0.; 3],
            emissive: [v; 3],
            use_specular_workflow: true,
            ..Default::default()
        })
    };
    let mid = h.upload(&emission(2.))?;
    let high = h.upload(&emission(4.))?;
    let exposed = Studio {
        exposure_ev: -2.,
        environment_intensity: 0.,
        ..studio
    };
    let lo = h.frame(&[object(Geometry::Mesh(&mid), [0.; 3])], &[], &exposed)?;
    let hi = h.frame(&[object(Geometry::Mesh(&high), [0.; 3])], &[], &exposed)?;
    assert!(
        pix(&lo, 64, 64)[0].abs_diff(188) < 2 && pix(&hi, 64, 64)[0] > pix(&lo, 64, 64)[0] + 40,
        "highlights retain energy before exposure"
    );
    Ok(())
}
#[test]
fn key_shadows_and_rotating_environment_are_independent() -> Result<()> {
    let mut h = Host::new()?;
    let plate = h.upload(&card(paint()))?;
    let mut blocker = card(paint());
    for v in &mut blocker.vertices {
        for p in &mut v.position {
            *p *= 0.22;
        }
    }
    let front = h.upload(&blocker)?;
    let objects = [
        object(Geometry::Mesh(&plate), [0.; 3]),
        object(Geometry::Mesh(&front), [0., 0., 0.45]),
    ];
    let studio = Studio {
        environment_intensity: 0.,
        shadow_extent: 1.8,
        ..Studio::default()
    };
    let a = h.frame(&objects, &[key(1.)], &studio)?;
    let b = h.frame(&objects, &[key(-1.)], &studio)?;
    let left = 46;
    let right = 82;
    assert!(
        pix(&a, left, 64)[0] > pix(&a, right, 64)[0] + 50
            && pix(&b, right, 64)[0] > pix(&b, left, 64)[0] + 50,
        "cast shadow changes sides: {:?} {:?} {:?} {:?}",
        pix(&a, left, 64),
        pix(&a, right, 64),
        pix(&b, left, 64),
        pix(&b, right, 64)
    );
    let metal = h.upload(&card(Material {
        diffuse: [0.8; 3],
        metallic: 1.,
        roughness: 0.16,
        ..Default::default()
    }))?;
    let metal = [object(Geometry::Mesh(&metal), [0.; 3])];
    let a = h.frame(
        &metal,
        &[],
        &Studio {
            environment_rotation_degrees: 28.,
            ..Studio::default()
        },
    )?;
    let b = h.frame(
        &metal,
        &[],
        &Studio {
            environment_rotation_degrees: 120.,
            ..Studio::default()
        },
    )?;
    assert!(
        a.as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .filter(|(x, y)| x[0].abs_diff(y[0]) > 16)
            .count()
            > 1000,
        "environment moves highlights without the key"
    );
    Ok(())
}

#[test]
fn packed_pbr_channels_and_tangent_normal_change_the_lit_surface() -> Result<()> {
    let mut h = Host::new()?;
    let mut m = card(Material {
        diffuse: [0.7, 0.4, 0.15],
        roughness: 1.,
        metallic: 1.,
        ..Default::default()
    });
    let tex = |channel, scale, bias| TextureMap {
        image: 0,
        channel,
        srgb: false,
        scale,
        bias,
    };
    let mut rgba = Vec::new();
    for _ in 0..4 {
        for x in 0..4 {
            rgba.extend(if x < 2 {
                [0, 255, 32, 255]
            } else {
                [255, 0, 224, 255]
            });
        }
    }
    m.images.push(Image {
        source: "packed linear values".into(),
        width: 4,
        height: 4,
        rgba,
    });
    let studio = Studio {
        environment_rotation_degrees: 30.,
        ..Default::default()
    };
    fn sample(h: &mut Host, m: &Mesh, studio: &Studio, lights: &[Light]) -> Result<Vec<u8>> {
        let model = h.upload(m)?;
        h.frame(&[object(Geometry::Mesh(&model), [0.; 3])], lights, studio)
    }
    m.materials[0].maps.occlusion = Some(tex(TextureChannel::G, [1.; 4], [0.; 4]));
    let ao = sample(&mut h, &m, &studio, &[])?;
    assert!(
        pix(&ao, 43, 64)[0] > 60 && pix(&ao, 85, 64)[0] < 4,
        "ambient occlusion reads green independently of red"
    );
    m.materials[0].maps.occlusion = None;
    m.materials[0].maps.metallic = Some(tex(TextureChannel::R, [1.; 4], [0.; 4]));
    let metal = sample(&mut h, &m, &studio, &[key(0.)])?;
    m.materials[0].maps.metallic = Some(tex(TextureChannel::G, [1.; 4], [0.; 4]));
    let reversed = sample(&mut h, &m, &studio, &[key(0.)])?;
    assert!(
        pix(&metal, 43, 64)[0].abs_diff(pix(&reversed, 43, 64)[0]) > 25,
        "channel selection changes the metal mask"
    );
    m.materials[0].maps.metallic = None;
    m.materials[0].maps.roughness = Some(tex(TextureChannel::B, [1.; 4], [0.; 4]));
    let rough = sample(&mut h, &m, &studio, &[key(0.)])?;
    m.materials[0].maps.roughness = Some(tex(TextureChannel::B, [-1.; 4], [1.; 4]));
    let inverted = sample(&mut h, &m, &studio, &[key(0.)])?;
    assert!(
        rough
            .as_chunks::<4>()
            .0
            .iter()
            .zip(inverted.as_chunks::<4>().0)
            .filter(|(a, b)| a[0].abs_diff(b[0]) > 12)
            .count()
            > 100,
        "roughness uses raw blue and its authored scale/bias"
    );
    m.materials[0].maps.roughness = None;
    m.materials[0].metallic = 0.;
    // Tangents tilt the normals toward +X and −X on opposite halves.
    for pixel in m.images[0].rgba.as_chunks_mut::<4>().0 {
        *pixel = if pixel[0] == 0 {
            [242, 128, 184, 255]
        } else {
            [14, 128, 184, 255]
        };
    }
    m.materials[0].maps.normal = Some(tex(
        TextureChannel::Rgb,
        [2., 2., 2., 1.],
        [-1., -1., -1., 0.],
    ));
    let dim = Studio {
        environment_intensity: 0.,
        ..studio
    };
    let a = sample(&mut h, &m, &dim, &[key(-1.)])?;
    let b = sample(&mut h, &m, &dim, &[key(1.)])?;
    assert!(
        pix(&a, 43, 64)[0] > pix(&a, 85, 64)[0] + 30
            && pix(&b, 85, 64)[0] > pix(&b, 43, 64)[0] + 30,
        "tangent-space map responds to the key from either side"
    );
    Ok(())
}

#[test]
fn named_occlusion_matte_respects_unlit_surface_cutout() -> Result<()> {
    let mut h = Host::new()?;
    let card = h.upload(&card(paint()))?;
    let mut image = Vec::new();
    for _ in 0..4 {
        for x in 0..4 {
            image.extend([0, 0, 0, if x < 2 { 255 } else { 0 }]);
        }
    }
    let art = h.image(&image);
    let bindings = [SurfaceBinding {
        slot: "/Paint",
        mode: SurfaceMode::Unlit,
        texture: &art,
    }];
    let objects = [
        Object {
            id: Some("back"),
            pose: Pose::rigid([0.; 3], [0.; 3]),
            geometry: Geometry::Mesh(&card),
        },
        Object {
            id: Some("screen"),
            pose: Pose::rigid([0., 0., 0.25], [0.; 3]),
            geometry: Geometry::Model {
                mesh: &card,
                parts: &[],
                surfaces: &bindings,
            },
        },
    ];
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    let beauty = h.texture(N, COLOR_FORMAT, usage);
    let matte = h.texture(N, MATTE_FORMAT, usage);
    let mut encoder = h.d.create_command_encoder(&Default::default());
    h.r.record_studio(
        &mut encoder,
        &beauty,
        View {
            position: [0., 0., 4.],
            rotation_degrees: [0.; 3],
            fov_y_degrees: 45.,
        },
        &objects,
        &[],
        Some(MatteOutput {
            target: &matte,
            requests: &[Matte {
                object: "back",
                owner: Some("screen"),
            }],
        }),
        &Studio::default(),
    )?;
    let data = h.readback(encoder, &matte, 1)?;
    assert_eq!(
        data[(64 * N + 43) as usize],
        255,
        "foreground occludes the left half"
    );
    assert_eq!(
        data[(64 * N + 85) as usize],
        0,
        "hole reveals the subject on the right"
    );
    let transition = &data[(64 * N + 54) as usize..(64 * N + 74) as usize];
    assert!(
        transition.iter().any(|&v| v > 0 && v < 255),
        "coverage survives MSAA resolve around the cutout"
    );
    Ok(())
}

#[test]
fn sloping_receiver_does_not_reveal_the_shadow_camera_bounds() -> Result<()> {
    let mut host = Host::new()?;
    let ground = host.upload(&card(paint()))?;
    let objects = [object(Geometry::Mesh(&ground), [0.; 3])];
    // A shadow camera narrower than the receiver deliberately cuts through the image.
    let lights = [key(2.)];
    let studio = Studio {
        environment_intensity: 0.,
        shadow_extent: 0.38,
        shadow_bias: 0.0001,
        shadow_softness: 4.,
        ..Studio::default()
    };
    let frame = host.frame(&objects, &lights, &studio)?;
    let reference = pix(&frame, 31, 64);
    for y in 33..93 {
        for x in 32..94 {
            let p = pix(&frame, x, y);
            assert!(
                p[0].abs_diff(reference[0]) <= 1,
                "self-shadow at ({x}, {y}): {p:?} vs {reference:?}"
            );
        }
    }
    Ok(())
}
