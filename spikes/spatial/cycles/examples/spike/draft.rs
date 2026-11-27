//! The Draft: the existing rasterizer on the same evaluated shot.
//! It has distant lights and an analytic studio only, so each rect light is reduced to a distant
//! light along its aim with the irradiance it delivers at its aim point. Output is display-referred.
use crate::{M34, arg, dot, key, load_png, norm, part_rotation, v3};
use anyhow::{Context, Result};
use nothing_spatial_wgpu::{
    Geometry, Light, MeshBuffers, Object, PartPose, Pose, RectLight, Renderer, Rig, Studio,
    SurfaceBinding, SurfaceMode, View,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    rgba: &[u8],
) -> wgpu::TextureView {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("artwork"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    queue.write_texture(
        t.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 4),
            rows_per_image: None,
        },
        t.size(),
    );
    t.create_view(&Default::default())
}

fn apply(m: &M34, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| {
        m[r * 4] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3]
    })
}

/// The display's rectangle in rest pose, from the three UV corners of its slot: centre, edges, normal.
fn display_quad(
    mesh: &nothing_spatial::Mesh,
    slot: &str,
) -> Result<([f32; 3], [f32; 3], [f32; 3], [f32; 3])> {
    let index = mesh
        .materials
        .iter()
        .position(|m| m.slot == slot)
        .context("display slot")? as u32;
    let verts: Vec<_> = mesh
        .vertices
        .iter()
        .filter(|v| v.material == index)
        .collect();
    let pick = |f: &dyn Fn(&[f32; 2]) -> f32| {
        verts
            .iter()
            .min_by(|a, b| f(&a.uv).total_cmp(&f(&b.uv)))
            .map(|v| v.position)
            .unwrap()
    };
    let (o, pu, pv) = (
        pick(&|uv| uv[0] + uv[1]),
        pick(&|uv| -uv[0] + uv[1]),
        pick(&|uv| uv[0] - uv[1]),
    );
    let u: [f32; 3] = std::array::from_fn(|i| pu[i] - o[i]);
    let v: [f32; 3] = std::array::from_fn(|i| pv[i] - o[i]);
    let c = std::array::from_fn(|i| o[i] + 0.5 * (u[i] + v[i]));
    Ok((c, u, v, verts[0].normal))
}

pub fn run(args: &[String]) -> Result<()> {
    let shot_path = PathBuf::from(arg(args, "--shot").context("--shot")?);
    let out = PathBuf::from(arg(args, "--out").context("--out")?);
    std::fs::create_dir_all(&out)?;
    let dir = shot_path.parent().unwrap();
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(&shot_path)?)?;
    let art = arg(args, "--artwork")
        .map(PathBuf::from)
        .unwrap_or(dir.join("artwork/base"));
    let light_scale: f32 = arg(args, "--light-scale").map_or(1., |s| s.parse().unwrap());
    let env: f32 = arg(args, "--env").map_or(0.3, |s| s.parse().unwrap());
    let (a, b) = arg(args, "--frames")
        .unwrap_or("0..1")
        .split_once("..")
        .context("--frames a..b")?;
    let size = [
        spec["size"][0].as_u64().unwrap() as u32,
        spec["size"][1].as_u64().unwrap() as u32,
    ];

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
    let asset = dir.join(spec["asset"].as_str().unwrap());
    let product = nothing_openusd::load(&asset, spec["product"].as_str().unwrap())?;
    let ground = nothing_openusd::load(&asset, spec["ground"].as_str().unwrap())?;
    let product_gpu = MeshBuffers::upload(&device, &queue, &product)?;
    let ground_gpu = MeshBuffers::upload(&device, &queue, &ground)?;
    let mut renderer = Renderer::new(&device);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("draft"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: nothing_spatial_wgpu::COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row = size[0] * 4;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * size[1]) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let (psize, plate) = load_png(&dir.join(spec["nameplate"]["image"].as_str().unwrap()))?;
    let plate = upload(&device, &queue, psize, &plate);

    let lights: Vec<Light> = spec["lights"]["rects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let (c, aim, rad) = (v3(&l["center"]), v3(&l["aim"]), v3(&l["radiance"]));
            let d = [aim[0] - c[0], aim[1] - c[1], aim[2] - c[2]];
            let dist2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let area = (l["width"].as_f64().unwrap() * l["height"].as_f64().unwrap()) as f32;
            let k = light_scale * area / dist2;
            Light {
                direction: norm(d),
                radiance: [rad[0] * k, rad[1] * k, rad[2] * k],
            }
        })
        .collect();
    let studio = Studio {
        environment_rotation_degrees: 0.,
        environment_intensity: env,
        exposure_ev: 0.,
        shadow_center: [0., 0.061, 0.],
        shadow_extent: 0.39,
        shadow_bias: 0.0003,
        shadow_softness: 4.,
    };
    let use_rig = args.iter().any(|a| a == "--rig");
    let rects: Vec<RectLight> = spec["lights"]["rects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| RectLight {
            center: v3(&l["center"]),
            edge_u: v3(&l["u"]),
            edge_v: v3(&l["v"]),
            radiance: v3(&l["radiance"]),
        })
        .collect();
    let ambient = v3(&spec["lights"]["background"]);
    let (qc, qu, qv, qn) = display_quad(&product, spec["display"]["slot"].as_str().unwrap())?;
    let hinge = product
        .parts
        .iter()
        .find(|p| p.path == spec["hinge"]["part"].as_str().unwrap())
        .context("hinge part")?;
    let glow = (spec["display"]["emission"].as_f64().unwrap()
        * spec["display"]["glow"].as_f64().unwrap()) as f32;
    let cam = &spec["camera"];
    let mut rows = Vec::new();
    for f in a.parse::<u32>()?..b.parse::<u32>()? {
        let t = Instant::now();
        let (asize, rgba) = load_png(&art.join(format!("signal-{f:03}.png")))?;
        let display = upload(&device, &queue, asize, &rgba);
        let mut deg = [0f32; 3];
        deg[spec["hinge"]["axis"].as_u64().unwrap_or(0) as usize] =
            key(&spec["hinge"]["keys"], f as f32);
        let parts = [PartPose {
            path: spec["hinge"]["part"].as_str().unwrap(),
            rotation_degrees: deg,
        }];
        let surfaces = [
            SurfaceBinding {
                slot: spec["display"]["slot"].as_str().unwrap(),
                texture: &display,
                mode: SurfaceMode::Unlit,
            },
            SurfaceBinding {
                slot: spec["nameplate"]["slot"].as_str().unwrap(),
                texture: &plate,
                mode: SurfaceMode::BaseColor,
            },
        ];
        let objects = [
            Object {
                id: Some("ground"),
                pose: Pose::rigid([0.; 3], [0.; 3]),
                geometry: Geometry::Model {
                    mesh: &ground_gpu,
                    parts: &[],
                    surfaces: &[],
                },
            },
            Object {
                id: Some("recorder"),
                pose: Pose::rigid([0.; 3], [0.; 3]),
                geometry: Geometry::Model {
                    mesh: &product_gpu,
                    parts: &parts,
                    surfaces: &surfaces,
                },
            },
        ];
        let view = View {
            position: v3(&cam["position"]),
            rotation_degrees: v3(&cam["rotation"]),
            fov_y_degrees: cam["fov_y"].as_f64().unwrap() as f32,
        };
        // The screen as a fourth rect light: its mean colour, moved with the lid.
        let m = part_rotation(&hinge.frame, hinge.pivot, deg);
        let origin = apply(&m, [0.; 3]);
        let turn = |v: [f32; 3]| -> [f32; 3] {
            let p = apply(&m, v);
            std::array::from_fn(|i| p[i] - origin[i])
        };
        let (u, v, n) = (turn(qu), turn(qv), turn(qn));
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let (u, v) = if dot(cross, n) >= 0. { (u, v) } else { (v, u) };
        let lin = |c: u8| {
            let x = c as f32 / 255.;
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        let mut mean = [0f32; 3];
        for px in rgba.chunks_exact(4) {
            for k in 0..3 {
                mean[k] += lin(px[k]);
            }
        }
        let count = (rgba.len() / 4) as f32;
        let mut frame_rects = rects.clone();
        frame_rects.push(RectLight {
            center: apply(&m, qc),
            edge_u: u,
            edge_v: v,
            radiance: mean.map(|x| x / count * glow),
        });
        let rig = Rig {
            rects: &frame_rects,
            ambient,
        };
        let mut commands = device.create_command_encoder(&Default::default());
        if use_rig {
            renderer.record_studio_rig(
                &mut commands,
                &target,
                view,
                &objects,
                &[],
                None,
                &studio,
                Some(&rig),
            )?;
        } else {
            renderer.record_studio(
                &mut commands,
                &target,
                view,
                &objects,
                &lights,
                None,
                &studio,
            )?;
        }
        commands.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        let submission = queue.submit([commands.finish()]);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = send.send(r);
        });
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })?;
        receive.recv_timeout(Duration::from_secs(1))??;
        let bytes = staging.slice(..).get_mapped_range()?.to_vec();
        staging.unmap();
        let wall = t.elapsed().as_secs_f64();
        write_png(&out.join(format!("frame-{f:03}.png")), size, &bytes)?;
        println!("draft frame {f}: {:.1} ms", wall * 1000.);
        rows.push(json!({"frame": f, "wall_seconds": wall}));
    }
    std::fs::write(
        out.join("receipt.json"),
        serde_json::to_string_pretty(
            &json!({"size": size, "rig": use_rig, "light_scale": light_scale, "environment_intensity": env, "frames": rows}),
        )?,
    )?;
    Ok(())
}

fn write_png(path: &Path, size: [u32; 2], premultiplied: &[u8]) -> Result<()> {
    image::RgbaImage::from_raw(size[0], size[1], premultiplied.to_vec())
        .context("image size")?
        .save(path)?;
    Ok(())
}
