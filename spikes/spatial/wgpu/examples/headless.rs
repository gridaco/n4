//! A host-owned camera, mesh, texture, output targets and command encoder.
//! Draw three views before consuming any of them, then verify one final readback.
use anyhow::{Context, Result, ensure};
use nothing_spatial::{Material, Mesh, Vertex};
use nothing_spatial_wgpu::{
    COLOR_FORMAT, Geometry, MATTE_FORMAT, Matte, MatteOutput, MeshBuffers, Object, Pose, Renderer,
    View,
};
use std::{sync::mpsc, time::Duration};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 96;
const ROW_BYTES: u32 = WIDTH * 4; // Multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
const VIEW_BYTES: usize = (ROW_BYTES * HEIGHT) as usize;
const FOV: f32 = 50.;
const DISTANCE: f32 = 4.;
const CAMERA_X: [f32; 3] = [0., 0.45, 0.];

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
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

fn foil() -> Mesh {
    // The left half is behind the card at Z=0, and the right half is in front.
    let vertices = [
        [-1.25, -0.9, -0.5],
        [1.25, -0.9, 0.5],
        [1.25, 0.9, 0.5],
        [-1.25, 0.9, -0.5],
    ]
    .map(|position| Vertex {
        position,
        normal: [0., 0., 1.],
        material: 0,
        uv: [0.; 2],
    });
    Mesh {
        vertices: vertices.to_vec(),
        indices: vec![0, 1, 2, 0, 2, 3],
        materials: vec![Material {
            diffuse: [0.; 3],
            emissive: [1., 0., 0.],
            specular: [0.; 3],
            metallic: 0.,
            roughness: 1.,
            clearcoat: 0.,
            clearcoat_roughness: 1.,
            use_specular_workflow: false,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn artwork(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    let image = texture(
        device,
        "card artwork",
        [16, 16],
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let color = if (2..6).contains(&x) && (6..10).contains(&y) {
                [0, 0, 0, 0]
            } else if y < 8 {
                [0, 255, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            rgba.extend(color);
        }
    }
    queue.write_texture(
        image.as_image_copy(),
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16 * 4),
            rows_per_image: None,
        },
        image.size(),
    );
    image
}

fn compositor(device: &wgpu::Device) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("host overprint"),
        source: wgpu::ShaderSource::Wgsl(include_str!("headless.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("host compositor"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn composite(
    device: &wgpu::Device,
    commands: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    beauty: &wgpu::Texture,
    hidden: &wgpu::Texture,
    output: &wgpu::Texture,
) {
    let beauty = beauty.create_view(&Default::default());
    let hidden = hidden.create_view(&Default::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("host reads scene color and matte"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&beauty),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&hidden),
            },
        ],
    });
    let view = output.create_view(&Default::default());
    let mut pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("host overprint and background"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.draw(0..3, 0..1);
}

fn verify(pixels: &[u8]) -> Result<usize> {
    let views = pixels.as_chunks::<VIEW_BYTES>().0;
    ensure!(views.len() == CAMERA_X.len());
    ensure!(
        views[0] == views[2],
        "returning the camera changed the picture"
    );
    let changed = views[0]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(views[1].as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    ensure!(
        changed > 200,
        "the second camera moved only {changed} pixels"
    );
    let pixels_per_meter = HEIGHT as f32 / (2. * DISTANCE * (FOV.to_radians() / 2.).tan());
    for (view, eye_x) in views.iter().zip(CAMERA_X) {
        // Project known points on the card. Probes are well inside their regions,
        // away from multisample boundaries and filtered texture transitions.
        for (point, expected, meaning) in [
            (
                [-0.65, 0.55],
                [0, 255, 0, 255],
                "top half of card in front of foil",
            ),
            (
                [-0.65, -0.55],
                [0, 0, 255, 255],
                "bottom half of card in front of foil",
            ),
            (
                [-0.82, 0.],
                [255, 0, 0, 255],
                "foil through the transparent hole",
            ),
            (
                [0.60, 0.30],
                [255, 255, 0, 255],
                "hidden card overprinted on front of foil",
            ),
            (
                [0., 1.55],
                [255, 0, 255, 255],
                "host background outside the scene",
            ),
        ] {
            let x = (WIDTH as f32 / 2. + (point[0] - eye_x) * pixels_per_meter) as usize;
            let y = (HEIGHT as f32 / 2. - point[1] * pixels_per_meter) as usize;
            let offset = 4 * (y * WIDTH as usize + x);
            let pixel = &view[offset..offset + 4];
            ensure!(
                pixel == expected,
                "view x={eye_x} at ({x},{y}): {meaning}, expected {expected:?}, got {pixel:?}"
            );
        }
    }
    Ok(changed)
}

fn main() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .context("request headless adapter")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
        .context("request device")?;
    let mut renderer = Renderer::new(&device);
    let foil = MeshBuffers::upload(&device, &queue, &foil())?;
    let card = artwork(&device, &queue).create_view(&Default::default());
    let objects = [
        Object {
            id: Some("foil"),
            pose: Pose::rigid([0.; 3], [0.; 3]),
            geometry: Geometry::Mesh(&foil),
        },
        Object {
            id: Some("card"),
            pose: Pose::plane([-1.5, 1., 0.], [3., 0., 0.], [0., -2., 0.]),
            geometry: Geometry::Plane(&card),
        },
    ];
    let matte = [Matte {
        object: "card",
        owner: Some("foil"),
    }];
    let size = [WIDTH, HEIGHT];
    let sampled_target =
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    let mut commands = device.create_command_encoder(&Default::default());
    let mut views = Vec::new();
    for x in CAMERA_X {
        let view = View {
            position: [x, 0., DISTANCE],
            rotation_degrees: [0.; 3],
            fov_y_degrees: FOV,
        };
        let beauty = texture(
            &device,
            "host color target",
            size,
            COLOR_FORMAT,
            sampled_target,
        );
        let hidden = texture(
            &device,
            "host matte target",
            size,
            MATTE_FORMAT,
            sampled_target,
        );
        renderer.record(
            &mut commands,
            &beauty,
            view,
            &objects,
            &[],
            Some(MatteOutput {
                target: &hidden,
                requests: &matte,
            }),
        )?;
        views.push((beauty, hidden));
    }
    let pipeline = compositor(&device);
    let finished = texture(
        &device,
        "host finished picture",
        size,
        COLOR_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read back three finished views once"),
        size: (VIEW_BYTES * views.len()) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for (i, (beauty, hidden)) in views.iter().enumerate() {
        composite(&device, &mut commands, &pipeline, beauty, hidden, &finished);
        commands.copy_texture_to_buffer(
            finished.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: (VIEW_BYTES * i) as u64,
                    bytes_per_row: Some(ROW_BYTES),
                    rows_per_image: None,
                },
            },
            finished.size(),
        );
    }
    let submission = queue.submit([commands.finish()]);
    let (send, receive) = mpsc::sync_channel(1);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(Duration::from_secs(30)),
    })?;
    receive.recv_timeout(Duration::from_secs(1))??;
    let mapped = staging.slice(..).get_mapped_range()?;
    let changed = verify(&mapped)?;
    drop(mapped);
    staging.unmap();
    let info = adapter.get_info();
    println!(
        "{} ({:?}): 3 views, {changed} camera-dependent pixels, 15 pixel checks passed; first and last views identical.",
        info.name, info.backend
    );
    Ok(())
}

#[test]
fn host_composites_independent_spatial_views() -> Result<()> {
    main()
}
